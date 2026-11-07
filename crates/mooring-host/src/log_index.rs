//! Durable identity/content index. First import is bounded; subsequent reads share one cursor.
use crate::recognition::Transcript;
use mooring_core::model::Agent;
use mooring_platform::file_watch::{normalized, Watch};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};
mod chat;
mod elide;
mod recovery;
mod tools;
mod update;

use elide::Elide;
pub use recovery::Recovery;
/// How long a turn may run before its prompt is logged and still be the one it opened.
const TURN_LEAD: u64 = 120;
fn err(e: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::other(e)
}
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct IoCounts {
    pub directories: u64,
    pub opens: u64,
    pub bytes: u64,
    pub writes: u64,
}
#[derive(Default)]
struct Cursor {
    dev: u64,
    inode: u64,
    offset: u64,
    pending: Vec<u8>,
    discard: bool,
    identity: Option<(String, String)>,
    updated: u64,
    /// Where `pending` stands inside a JSON string; rebuilt from it, never stored.
    elide: Elide,
}

/// How long a moving log's card and cursor may stay ahead of the database.
const FLUSH_AFTER: Duration = Duration::from_secs(1);
/// Clean entries kept for logs that stopped moving.
const HELD_CLEAN: usize = 64;
/// One log's state while it is being read: the card and the cursor it was read to. The
/// database holds the same pair as of the last flush, so a restart that replays events
/// from the checkpoint continues from a consistent snapshot.
struct Entry {
    runtime: Agent,
    cursor: Cursor,
    parser: crate::content::Parser,
    /// Since when this entry is ahead of the database.
    dirty: Option<std::time::Instant>,
    /// The subagents on disk, read again only when their meta files change: each line
    /// filters them against the card, but listing and parsing every meta per line costs
    /// as much as the line itself once a conversation has run a dozen.
    subagents: Option<Vec<mooring_core::content::Subagent>>,
    used: std::time::Instant,
}
/// The single owner of the cards being read, shared by every fork of an index and
/// guarded by its gate: each line updates the entry in memory instead of loading and
/// saving the whole card. The checkpoint never passes an event whose entry is unflushed.
#[derive(Default)]
pub struct Held {
    entries: std::collections::HashMap<String, Entry>,
    /// The newest event folded into an unflushed entry.
    pending_event: Option<u64>,
}
impl Held {
    fn oldest_dirty(&self) -> Option<std::time::Instant> {
        self.entries.values().filter_map(|e| e.dirty).min()
    }
    /// Forget clean entries beyond the limit, least recently used first.
    fn trim(&mut self) {
        while self.entries.len() > HELD_CLEAN {
            let Some(key) = self
                .entries
                .iter()
                .filter(|(_, e)| e.dirty.is_none())
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
                return;
            };
            self.entries.remove(&key);
        }
    }
}
fn write_cursor(db: &Connection, key: &str, runtime: Agent, c: &Cursor) -> io::Result<()> {
    db.prepare_cached("INSERT INTO log_files VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT(path) DO UPDATE SET runtime=excluded.runtime,dev=excluded.dev,inode=excluded.inode,offset=excluded.offset,pending=excluded.pending,discard=excluded.discard,cli_id=excluded.cli_id,cwd=excluded.cwd,updated=excluded.updated").map_err(err)?.execute(
        params![key,runtime.executable(),i64::try_from(c.dev).map_err(err)?,i64::try_from(c.inode).map_err(err)?,i64::try_from(c.offset).map_err(err)?,c.pending,c.discard,c.identity.as_ref().map(|v| &v.0),c.identity.as_ref().map(|v| &v.1),i64::try_from(c.updated).map_err(err)?]).map_err(err)?;
    Ok(())
}
pub struct Index {
    db: Connection,
    pub roots: Vec<(Agent, PathBuf)>,
    pub counts: IoCounts,
    pub recent_days: u64,
    gate: std::sync::Arc<std::sync::Mutex<Held>>,
    /// Parents whose card a subagent's log changed during the current batch.
    touched: Vec<Transcript>,
}
impl Index {
    pub fn open(root: &Path, roots: Vec<(Agent, PathBuf)>) -> io::Result<Self> {
        let db = Connection::open(root.join("workbench.sqlite")).map_err(err)?;
        db.busy_timeout(Duration::from_secs(5)).map_err(err)?;
        // NORMAL keeps WAL commits atomic and ordered; a power loss can drop only the last ones,
        // whose lines are read again from the logs.
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
            CREATE TABLE IF NOT EXISTS log_index (id INTEGER PRIMARY KEY CHECK(id=1), event_id INTEGER NOT NULL, error TEXT);
            CREATE TABLE IF NOT EXISTS log_scan_dirs(path TEXT PRIMARY KEY,runtime TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS log_generations(path TEXT PRIMARY KEY,generation TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS log_files(path TEXT PRIMARY KEY,runtime TEXT NOT NULL,dev INTEGER NOT NULL,inode INTEGER NOT NULL,offset INTEGER NOT NULL,pending BLOB NOT NULL,discard INTEGER NOT NULL,cli_id TEXT,cwd TEXT,updated INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS log_identity ON log_files(runtime,cli_id);
            CREATE TABLE IF NOT EXISTS log_scan_roots(runtime TEXT NOT NULL,path TEXT NOT NULL,PRIMARY KEY(runtime,path));").map_err(err)?;
        crate::content::schema(&db)?;
        let roots = roots
            .into_iter()
            .map(|(a, p)| Ok((a, normalized(&p)?)))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self {
            db,
            roots,
            recent_days: 7,
            counts: IoCounts::default(),
            gate: std::sync::Arc::new(std::sync::Mutex::new(Held::default())),
            touched: Vec::new(),
        })
    }
    pub fn fork_current(&self) -> io::Result<Self> {
        let path = self
            .db
            .path()
            .ok_or_else(|| io::Error::other("index database path unavailable"))?;
        self.fork(
            Path::new(path)
                .parent()
                .ok_or_else(|| io::Error::other("index database directory unavailable"))?,
        )
    }
    pub fn fork(&self, root: &Path) -> io::Result<Self> {
        let mut index = Self::open(root, self.roots.clone())?;
        index.gate = self.gate.clone();
        index.recent_days = self.recent_days;
        Ok(index)
    }
    /// Durable directory worklist: a crash can repeat only its unfinished directory, not the tree.
    pub fn initialize(&mut self) -> io::Result<u64> {
        if let Some((event, error)) = self
            .db
            .query_row("SELECT event_id,error FROM log_index WHERE id=1", [], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, Option<String>>(1)?))
            })
            .optional()
            .map_err(err)?
        {
            if error.is_some() {
                // Errors are advisory state, not a permanent latch. Resume the durable worklist/events.
                self.db
                    .execute("UPDATE log_index SET error=NULL WHERE id=1", [])
                    .map_err(err)?;
            }
            self.seed_new_roots()?;
            return Ok(event);
        }
        let event = Watch::current_id();
        let tx = self.db.transaction().map_err(err)?;
        tx.execute(
            "INSERT INTO log_index VALUES(1,?1,NULL)",
            [i64::try_from(event).map_err(err)?],
        )
        .map_err(err)?;
        for (agent, path) in &self.roots {
            tx.execute(
                "INSERT OR IGNORE INTO log_scan_dirs VALUES(?1,?2)",
                params![path.to_string_lossy(), agent.executable()],
            )
            .map_err(err)?;
            tx.execute(
                "INSERT OR IGNORE INTO log_scan_roots VALUES(?1,?2)",
                params![agent.executable(), path.to_string_lossy()],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        self.counts.writes += 1;
        Ok(event)
    }
    /// Each runtime/root pair gets one directory walk. The runtime alone cannot say
    /// that a different directory's existing files have already been imported.
    fn seed_new_roots(&mut self) -> io::Result<()> {
        let tx = self.db.transaction().map_err(err)?;
        let mut seeded = false;
        for (agent, path) in &self.roots {
            let known: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM log_scan_roots WHERE runtime=?1 AND path=?2)",
                    params![agent.executable(), path.to_string_lossy()],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if !known {
                tx.execute(
                    "INSERT OR IGNORE INTO log_scan_dirs VALUES(?1,?2)",
                    params![path.to_string_lossy(), agent.executable()],
                )
                .map_err(err)?;
                seeded = true;
            }
            tx.execute(
                "INSERT OR IGNORE INTO log_scan_roots VALUES(?1,?2)",
                params![agent.executable(), path.to_string_lossy()],
            )
            .map_err(err)?;
        }
        tx.commit().map_err(err)?;
        if seeded {
            self.counts.writes += 1;
        }
        Ok(())
    }
    pub fn bootstrap(&mut self) -> io::Result<()> {
        loop {
            let dir: Option<(String, String)> = self
                .db
                .query_row("SELECT path,runtime FROM log_scan_dirs LIMIT 1", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()
                .map_err(err)?;
            let Some((path, runtime)) = dir else {
                // Content is a newly introduced object. Seed only cached recent identities that
                // have no projection, once, without another directory walk or replacing cursors.
                let missing = {
                    let mut q=self.db.prepare("SELECT runtime,path FROM log_files WHERE updated>=?1 AND NOT EXISTS(SELECT 1 FROM session_content WHERE session_content.path=log_files.path AND json_extract(parser,'$._rules')=?2)").map_err(err)?;
                    let rows = q
                        .query_map(
                            [
                                mooring_core::content::now()
                                    .saturating_sub(self.recent_days.saturating_mul(86400))
                                    as i64,
                                i64::from(crate::content::RULES),
                            ],
                            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                        )
                        .map_err(err)?;
                    rows.collect::<Result<Vec<_>, _>>().map_err(err)?
                };
                for (runtime, path) in missing {
                    self.update(agent(&runtime)?, Path::new(&path), true)?;
                }
                return Ok(());
            };
            let agent = agent(&runtime)?;
            self.counts.directories += 1;
            let mut children = vec![];
            match fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = entry?;
                        let kind = entry.file_type()?;
                        if kind.is_dir() {
                            if crate::agents::adapter(agent).descend(&entry.path()) {
                                children.push(entry.path());
                            }
                        } else if kind.is_file()
                            && crate::agents::adapter(agent).is_transcript(&entry.path())
                        {
                            self.update(agent, &entry.path(), true)?;
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
            let tx = self.db.transaction().map_err(err)?;
            for child in children {
                tx.execute(
                    "INSERT OR IGNORE INTO log_scan_dirs VALUES(?1,?2)",
                    params![child.to_string_lossy(), runtime],
                )
                .map_err(err)?;
            }
            tx.execute("DELETE FROM log_scan_dirs WHERE path=?1", [path])
                .map_err(err)?;
            tx.commit().map_err(err)?;
            self.counts.writes += 1;
        }
    }
    pub fn reset_checkpoint(&mut self, event: u64) -> io::Result<()> {
        // A checkpoint moved to now skips what came before it: nothing may be unflushed.
        self.flush()?;
        self.db
            .execute(
                "UPDATE log_index SET event_id=?1 WHERE id=1",
                [i64::try_from(event).map_err(err)?],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn checkpoint(&mut self, event: u64) -> io::Result<()> {
        self.db
            .prepare_cached(
                "UPDATE log_index SET event_id=MAX(event_id,?1) WHERE id=1 AND event_id<?1",
            )
            .map_err(err)?
            .execute([i64::try_from(event).map_err(err)?])
            .map_err(err)?;
        self.counts.writes += 1;
        Ok(())
    }
    pub fn fail(&self, error: &io::Error) -> io::Result<()> {
        self.db
            .execute(
                "UPDATE log_index SET error=?1 WHERE id=1",
                [error.to_string()],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn clear_error(&self) -> io::Result<()> {
        self.db
            .execute(
                "UPDATE log_index SET error=NULL WHERE id=1 AND error IS NOT NULL",
                [],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn queue_recovery(&self, scopes: &[PathBuf]) -> io::Result<()> {
        for path in scopes {
            if let Some(runtime) = self.runtime(path) {
                self.db
                    .execute(
                        "INSERT OR IGNORE INTO log_scan_dirs VALUES(?1,?2)",
                        params![path.to_string_lossy(), runtime.executable()],
                    )
                    .map_err(err)?;
            }
        }
        Ok(())
    }
    pub fn recovery_scopes(&self) -> io::Result<Vec<PathBuf>> {
        let mut statement = self
            .db
            .prepare_cached("SELECT path FROM log_scan_dirs")
            .map_err(err)?;
        let rows = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(err)?;
        rows.map(|r| r.map(PathBuf::from).map_err(err)).collect()
    }
    pub fn finish_recovery(&mut self, scopes: &[PathBuf]) -> io::Result<()> {
        // Reconcile deletions missed during the gap, only inside the repaired subtree.
        // Removing an index entry never deletes an AgentSession or its user-owned outcome.
        for scope in scopes {
            let prefix = format!("{}/", scope.to_string_lossy());
            let paths = {
                let mut q = self
                    .db
                    .prepare("SELECT path FROM log_files WHERE substr(path,1,length(?1))=?1")
                    .map_err(err)?;
                let rows = q
                    .query_map(params![prefix], |r| r.get::<_, String>(0))
                    .map_err(err)?;
                rows.collect::<Result<Vec<_>, _>>().map_err(err)?
            };
            for path in paths {
                let remove = match fs::symlink_metadata(&path) {
                    Ok(m) => !m.is_file(),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => true,
                    Err(e) => return Err(e),
                };
                if remove {
                    self.db
                        .execute("DELETE FROM log_files WHERE path=?1", [path])
                        .map_err(err)?;
                    self.counts.writes += 1;
                }
            }
        }
        for path in scopes {
            self.db
                .execute(
                    "DELETE FROM log_scan_dirs WHERE path=?1",
                    [path.to_string_lossy()],
                )
                .map_err(err)?;
        }
        self.clear_error()
    }
    pub fn runtime(&self, path: &Path) -> Option<Agent> {
        self.roots
            .iter()
            .find(|(_, root)| path.starts_with(root))
            .map(|(a, _)| *a)
    }
    /// Every identified log, as recent as memory has it: a held log's database row lags
    /// by up to `FLUSH_AFTER`, and its update time decides which thread is newer.
    pub fn cached(&self) -> io::Result<Vec<Transcript>> {
        let held = self
            .gate
            .lock()
            .map_err(|_| io::Error::other("log cursor lock poisoned"))?;
        let mut out = cached(&self.db)?;
        for (path, e) in &held.entries {
            let Some((cli_id, cwd)) = &e.cursor.identity else {
                continue;
            };
            let t = Transcript {
                runtime: e.runtime,
                cli_id: cli_id.clone(),
                path: path.clone(),
                cwd: cwd.clone(),
                updated: e.cursor.updated,
            };
            match out.iter_mut().find(|o| &o.path == path) {
                Some(o) => *o = t,
                None => out.push(t),
            }
        }
        Ok(out)
    }
    pub fn original_words(&self, path: &str) -> io::Result<String> {
        let mut q=self.db.prepare_cached("SELECT text FROM session_records WHERE path=?1 AND kind='user' ORDER BY at,offset,ordinal").map_err(err)?;
        let rows = q
            .query_map([path], |r| r.get::<_, String>(0))
            .map_err(err)?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?
            .join("\n\n追加要求："))
    }
    /// The newest user message only; screen association compares it, never stores it.
    /// The newest prompt and when it was logged, seconds since the epoch.
    pub fn latest_user(&self, path: &str) -> io::Result<Option<(String, u64)>> {
        let mut q = self
            .db
            .prepare_cached("SELECT text,at FROM session_records WHERE path=?1 AND kind='user' ORDER BY at DESC,offset DESC,ordinal DESC LIMIT 1")
            .map_err(err)?;
        let mut rows = q.query([path]).map_err(err)?;
        Ok(match rows.next().map_err(err)? {
            Some(row) => Some((
                row.get(0).map_err(err)?,
                u64::try_from(row.get::<_, i64>(1).map_err(err)?).unwrap_or(0),
            )),
            None => None,
        })
    }
    /// When the newest turn that began by `at` began: the turn a prompt logged at `at`
    /// opened. Codex starts it at Enter and may log the prompt seconds later. A turn begun
    /// over `TURN_LEAD` earlier is one the prompt was typed into, not one it opened.
    pub fn turn_began(&self, path: &str, at: u64) -> io::Result<Option<u64>> {
        let mut q = self
            .db
            .prepare_cached("SELECT MAX(at) FROM session_records WHERE path=?1 AND kind='turn' AND text='Running' AND at<=?2 AND at>=?3")
            .map_err(err)?;
        let began: Option<i64> = q
            .query_row(
                params![path, at as i64, at.saturating_sub(TURN_LEAD) as i64],
                |row| row.get(0),
            )
            .map_err(err)?;
        Ok(began.and_then(|b| u64::try_from(b).ok()))
    }
    pub fn latest_user_text(&self, path: &str) -> io::Result<Option<String>> {
        let mut q = self
            .db
            .prepare_cached("SELECT text FROM session_records WHERE path=?1 AND kind='user' ORDER BY at DESC,offset DESC,ordinal DESC LIMIT 1")
            .map_err(err)?;
        let mut rows = q.query([path]).map_err(err)?;
        Ok(match rows.next().map_err(err)? {
            Some(row) => Some(row.get(0).map_err(err)?),
            None => None,
        })
    }
    /// Parents whose card a subagent's log changed since the last call.
    pub fn take_touched(&mut self) -> Vec<Transcript> {
        std::mem::take(&mut self.touched)
    }
    /// Logs of recent subagents of these threads, newest first, at most `limit`.
    pub fn child_logs(&self, parents: &[(Agent, String)], limit: usize) -> io::Result<Vec<String>> {
        let since = mooring_core::content::now().saturating_sub(86400) as i64;
        let mut q = self
            .db
            .prepare_cached(
                "SELECT l.path FROM subagent_links l JOIN log_files f ON f.path=l.path WHERE l.parent=?1 AND l.updated>=?2 AND f.runtime=?3 ORDER BY l.updated DESC",
            )
            .map_err(err)?;
        let mut out = Vec::new();
        for (runtime, parent) in parents {
            let rows = q
                .query_map(params![parent, since, runtime.executable()], |r| {
                    r.get::<_, String>(0)
                })
                .map_err(err)?;
            for row in rows {
                out.push(row.map_err(err)?);
            }
        }
        out.truncate(limit);
        Ok(out)
    }
    pub fn card(&self, path: &str) -> io::Result<Option<mooring_core::content::Content>> {
        let held = self
            .gate
            .lock()
            .map_err(|_| io::Error::other("log cursor lock poisoned"))?
            .entries
            .get(path)
            .map(|e| e.parser.card.clone());
        let mut card = match held {
            Some(card) => Some(card),
            None => crate::content::card(&self.db, path)?,
        };
        if let Some(c) = card.as_mut().filter(|c| c.name.is_empty()) {
            let path = Path::new(path);
            if let Some((agent, root)) = self.roots.iter().find(|(_, r)| path.starts_with(r)) {
                c.name = crate::agents::adapter(*agent)
                    .name(root, path)
                    .unwrap_or_default();
            }
        }
        Ok(card)
    }
    pub fn recent(&self, updated: u64) -> bool {
        updated
            >= mooring_core::content::now().saturating_sub(self.recent_days.saturating_mul(86400))
    }
}
pub fn cached(db: &Connection) -> io::Result<Vec<Transcript>> {
    let mut query = db
        .prepare("SELECT runtime,cli_id,path,cwd,updated FROM log_files WHERE cli_id IS NOT NULL")
        .map_err(err)?;
    let mut rows = query.query([]).map_err(err)?;
    let mut out = vec![];
    while let Some(r) = rows.next().map_err(err)? {
        out.push(Transcript {
            runtime: agent(&r.get::<_, String>(0).map_err(err)?)?,
            cli_id: r.get(1).map_err(err)?,
            path: r.get(2).map_err(err)?,
            cwd: r.get(3).map_err(err)?,
            updated: r.get::<_, i64>(4).map_err(err)? as u64,
        });
    }
    Ok(out)
}
/// A row of a CLI no longer registered (its declaration was removed) reads back as an
/// Agent with no adapter, so the index skips it rather than failing every query.
fn agent(s: &str) -> io::Result<Agent> {
    Ok(Agent::from_executable(s).unwrap_or_else(|| Agent::from_key(s)))
}

/// The transcript whose subagent `path` describes: `<dir>/<session>.jsonl` for
/// `<dir>/<session>/subagents/agent-<id>.meta.json`.
#[cfg(test)]
mod tests;
