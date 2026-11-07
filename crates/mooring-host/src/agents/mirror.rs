//! A declared CLI that keeps its conversations in SQLite (`AgentSpec::source`) gets them
//! written out as JSON lines, one file per session, under a directory Mooring owns. From
//! there the ordinary transcript path takes over: watching, cursors, parsing, binding and
//! recovery are the same as for a CLI that writes its own files.
//!
//! The database is only ever opened read-only, kept open between polls, waits briefly on a
//! writer's lock and is dropped on any error to be found again on the next poll, so a busy
//! or migrating CLI never stalls the daemon.
use mooring_core::agent::RecordSource;
use mooring_core::model::Agent;
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where a sourced CLI's sessions are written: the root its adapter reports.
pub fn dir(home: &Path, agent: Agent) -> PathBuf {
    home.join(".local/share/mooring/sources").join(agent.key())
}

/// Preserve byte cursors when the snapshot only grows. A rewritten prefix gets a new
/// inode, so the index can invalidate the old records and chat pagination together.
pub(super) fn publish_snapshot(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    use std::io::Write;
    match std::fs::read(path) {
        Ok(old) if old == bytes => return Ok(false),
        Ok(old) if bytes.starts_with(&old) => {
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)?
                .write_all(&bytes[old.len()..])?;
        }
        Ok(_) => {
            let temp = path.with_extension("tmp");
            std::fs::write(&temp, bytes)?;
            std::fs::rename(temp, path)?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::write(path, bytes)?,
        Err(e) => return Err(e),
    }
    Ok(true)
}

/// One running mirror: its source, and the flag that ends its thread.
struct Running {
    key: &'static str,
    source: RecordSource,
    stop: Arc<AtomicBool>,
}
static RUNNING: Mutex<Vec<Running>> = Mutex::new(Vec::new());

/// One thread per registered CLI with a SQLite source; none when there is none. Called at
/// start and whenever plugins change: a mirror whose CLI went away or changed its source ends,
/// a new one starts.
pub fn sync(home: &Path) {
    super::hermes_store::start(home);
    super::dsh_store::start(home);
    let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
    let wanted: Vec<(Agent, RecordSource)> = Agent::all()
        .into_iter()
        .filter_map(|a| Some((a, a.spec()?.source.clone()?)))
        .collect();
    running.retain(|r| {
        let keep = wanted
            .iter()
            .any(|(a, s)| a.key() == r.key && *s == r.source);
        if !keep {
            r.stop.store(true, Ordering::SeqCst);
        }
        keep
    });
    for (agent, source) in wanted {
        if running.iter().any(|r| r.key == agent.key()) {
            continue;
        }
        let dir = dir(home, agent);
        let stop = Arc::new(AtomicBool::new(false));
        let (thread_source, thread_stop) = (source.clone(), stop.clone());
        let started = std::thread::Builder::new()
            .name(format!("mirror-{}", agent.key()))
            .spawn(move || run(&thread_source, &dir, &thread_stop));
        if started.is_ok() {
            running.push(Running {
                key: agent.key(),
                source,
                stop,
            });
        }
    }
}

fn run(source: &RecordSource, dir: &Path, stop: &AtomicBool) {
    let mut mirror: Option<Mirror> = None;
    let mut db: Option<Connection> = None;
    let mut last_error = String::new();
    while !stop.load(Ordering::SeqCst) {
        let polled = match &mut mirror {
            Some(m) => m.poll(source, &mut db),
            None => Mirror::open(dir).map(|m| mirror = Some(m)),
        };
        match polled {
            Ok(()) => last_error.clear(),
            Err(e) => {
                db = None;
                let e = e.to_string();
                if e != last_error {
                    eprintln!("mooring: {} 读不了：{e}", source.sqlite.display());
                    last_error = e;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(source.poll_ms));
    }
}

/// A changed session: its id, working directory, change stamp and source metadata.
type Changed = (String, String, i64, Value);

/// How far each session has been written, so a restart neither repeats nor skips rows.
#[derive(Default, Serialize, Deserialize)]
struct Cursors {
    sessions: i64,
    rows: HashMap<String, (i64, String)>,
    /// The source metadata last written per session, so a repeated poll writes nothing.
    #[serde(default)]
    headers: HashMap<String, Value>,
}

pub struct Mirror {
    dir: PathBuf,
    cursors: Cursors,
}

impl Mirror {
    /// Only a missing file starts from nothing: cursors that cannot be read would write
    /// every session's rows out again after the ones already there.
    pub fn open(dir: &Path) -> io::Result<Self> {
        let path = dir.join(".cursors.json");
        let bad = |e: &dyn std::fmt::Display| {
            io::Error::other(format!("抄写进度 {} 读不了，暂停抄写：{e}", path.display()))
        };
        let cursors = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| bad(&e))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Cursors::default(),
            Err(e) => return Err(bad(&e)),
        };
        Ok(Self {
            dir: dir.to_owned(),
            cursors,
        })
    }

    /// One pass: sessions changed since the last, and each one's rows after its cursor.
    pub fn poll(&mut self, source: &RecordSource, db: &mut Option<Connection>) -> io::Result<()> {
        if db.is_none() {
            let conn = Connection::open_with_flags(
                &source.sqlite,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(io::Error::other)?;
            conn.busy_timeout(Duration::from_millis(50))
                .map_err(io::Error::other)?;
            *db = Some(conn);
        }
        let conn = db.as_ref().expect("opened above");
        let changed = self.changed(conn, source).map_err(io::Error::other)?;
        if changed.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut moved = false;
        for (session, cwd, t, header) in changed {
            // Identity and parent precede the first prompt, so a child is never briefly
            // published as an independent session, including when it has no messages yet.
            if self.cursors.headers.get(&session) != Some(&header) {
                let mut record = header.clone();
                record["_t"] = t.into();
                self.append(&session, format!("{record}\n").as_bytes())?;
                self.cursors.headers.insert(session.clone(), header);
                moved = true;
            }
            moved |= self.write_session(conn, source, &session, &cwd)?;
            moved |= t > self.cursors.sessions;
            self.cursors.sessions = self.cursors.sessions.max(t);
        }
        // The boundary is read again on every poll: only something new is worth a write.
        if moved {
            self.save()
        } else {
            Ok(())
        }
    }

    fn changed(&self, conn: &Connection, source: &RecordSource) -> rusqlite::Result<Vec<Changed>> {
        let mut stmt = conn.prepare_cached(&source.sessions)?;
        let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        // Sessions are asked for by `updated > cursor`, in the CLI's own clock: another write in
        // the very moment the cursor stands on would never come up again. So that moment is
        // always read once more; the row cursors keep what was written from being repeated.
        let rows = stmt.query_map([self.cursors.sessions.saturating_sub(1)], |r| {
            Ok((
                r.get::<_, String>("_session")?,
                r.get::<_, String>("_cwd")?,
                r.get::<_, i64>("_t")?,
                {
                    let mut header = Map::new();
                    for (i, name) in names.iter().enumerate() {
                        if name != "_t" {
                            header.insert(name.clone(), column(r.get_ref(i)?));
                        }
                    }
                    Value::Object(header)
                },
            ))
        })?;
        rows.collect()
    }

    fn write_session(
        &mut self,
        conn: &Connection,
        source: &RecordSource,
        session: &str,
        cwd: &str,
    ) -> io::Result<bool> {
        let (mut t, mut id) = self
            .cursors
            .rows
            .get(session)
            .cloned()
            .unwrap_or((i64::MIN, String::new()));
        let mut lines = Vec::new();
        {
            let mut stmt = conn
                .prepare_cached(&source.records)
                .map_err(io::Error::other)?;
            let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
            let mut rows = stmt
                .query(rusqlite::params![session, t, id])
                .map_err(io::Error::other)?;
            while let Some(row) = rows.next().map_err(io::Error::other)? {
                let mut record = Map::new();
                for (i, name) in names.iter().enumerate() {
                    let value = row.get_ref(i).map_err(io::Error::other)?;
                    record.insert(name.clone(), column(value));
                }
                t = record.get("_t").and_then(Value::as_i64).unwrap_or(t);
                id = record
                    .get("_id")
                    .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned))
                    .unwrap_or_default();
                record.insert("_session".into(), session.into());
                record.insert("_cwd".into(), cwd.into());
                lines.extend(serde_json::to_vec(&record).map_err(io::Error::other)?);
                lines.push(b'\n');
            }
        }
        if lines.is_empty() {
            return Ok(false);
        }
        self.append(session, &lines)?;
        self.cursors.rows.insert(session.to_owned(), (t, id));
        Ok(true)
    }

    /// One write per batch: the watcher never sees half a row.
    fn append(&self, session: &str, lines: &[u8]) -> io::Result<()> {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(format!("{}.jsonl", file_name(session))))?
            .write_all(lines)
    }

    fn save(&self) -> io::Result<()> {
        let tmp = self.dir.join(".cursors.json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec(&self.cursors).map_err(io::Error::other)?,
        )?;
        std::fs::rename(tmp, self.dir.join(".cursors.json"))
    }
}

/// A column as JSON; text that is a JSON object or array is read as one.
fn column(value: ValueRef) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => n.into(),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t);
            let trimmed = s.trim_start();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                serde_json::from_str(&s).unwrap_or_else(|_| Value::String(s.into_owned()))
            } else {
                Value::String(s.into_owned())
            }
        }
        ValueRef::Blob(_) => Value::Null,
    }
}

/// A session id as a file name: anything outside [A-Za-z0-9._-] becomes `_`.
fn file_name(session: &str) -> String {
    session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> (RecordSource, Connection) {
        let db = root.join("cli.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE session(id TEXT, dir TEXT, updated INTEGER);
             CREATE TABLE part(id TEXT, session_id TEXT, t INTEGER, role TEXT, data TEXT);",
        )
        .unwrap();
        let source = RecordSource {
            sqlite: db,
            sessions: "SELECT id AS _session, dir AS _cwd, updated AS _t FROM session WHERE updated > ?1".into(),
            records: "SELECT t AS _t, id AS _id, role, data FROM part WHERE session_id = ?1 AND (t, id) > (?2, ?3) ORDER BY t, id".into(),
            poll_ms: 200,
        };
        (source, conn)
    }

    fn lines(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// A session written in the same instant the cursor already reached still comes up, and
    /// reading that instant again repeats nothing and saves nothing.
    #[test]
    fn a_write_in_the_cursors_own_instant_is_not_lost() {
        let root = std::env::temp_dir().join(format!("kl-mirror-same-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (source, writer) = fixture(&root);
        let out = root.join("out");
        writer
            .execute_batch(
                "INSERT INTO session VALUES('a','/w',100);
                 INSERT INTO part VALUES('1','a',1,'user','{}');",
            )
            .unwrap();
        let mut mirror = Mirror::open(&out).unwrap();
        let mut db = None;
        mirror.poll(&source, &mut db).unwrap();
        writer
            .execute_batch(
                "INSERT INTO session VALUES('b','/w',100);
                 INSERT INTO part VALUES('2','b',1,'user','{}');
                 INSERT INTO part VALUES('3','a',2,'assistant','{}');",
            )
            .unwrap();
        mirror.poll(&source, &mut db).unwrap();
        assert_eq!(
            lines(&out.join("b.jsonl")).len(),
            2,
            "the new session came up"
        );
        assert_eq!(
            lines(&out.join("a.jsonl")).len(),
            3,
            "and the old one's new row, once"
        );
        let saved = std::fs::metadata(out.join(".cursors.json"))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        mirror.poll(&source, &mut db).unwrap();
        assert_eq!(lines(&out.join("a.jsonl")).len(), 3);
        assert_eq!(
            std::fs::metadata(out.join(".cursors.json"))
                .unwrap()
                .modified()
                .unwrap(),
            saved,
            "nothing new, nothing saved"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rows_are_written_once_in_order_and_survive_a_restart() {
        let root = std::env::temp_dir().join(format!("kl-mirror-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (source, writer) = fixture(&root);
        let out = root.join("out");
        writer
            .execute_batch(
                "INSERT INTO session VALUES('s/1','/work',10);
                 INSERT INTO part VALUES('a','s/1',1,'user','{\"type\":\"text\",\"text\":\"hi\"}');
                 INSERT INTO part VALUES('b','s/1',2,'assistant','{\"type\":\"step-finish\",\"reason\":\"stop\"}');",
            )
            .unwrap();
        let mut mirror = Mirror::open(&out).unwrap();
        let mut db = None;
        mirror.poll(&source, &mut db).unwrap();
        let file = out.join("s_1.jsonl");
        let first = lines(&file);
        assert_eq!(first.len(), 3);
        assert_eq!(first[1]["data"]["text"], "hi", "JSON text is read as JSON");
        assert_eq!(first[0]["_session"], "s/1");
        assert_eq!(first[0]["_cwd"], "/work");
        assert_eq!(first[2]["data"]["reason"], "stop");

        // Nothing new: nothing written.
        mirror.poll(&source, &mut db).unwrap();
        assert_eq!(lines(&file).len(), 3);

        // A restart resumes from the saved cursors; only the new row is appended.
        writer
            .execute_batch(
                "UPDATE session SET updated = 20 WHERE id = 's/1';
                 INSERT INTO part VALUES('c','s/1',3,'user','{\"type\":\"text\",\"text\":\"again\"}');",
            )
            .unwrap();
        let mut restarted = Mirror::open(&out).unwrap();
        restarted.poll(&source, &mut None).unwrap();
        let all = lines(&file);
        assert_eq!(all.len(), 4);
        assert_eq!(all[3]["data"]["text"], "again");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_title_is_written_once_and_again_only_when_it_changes() {
        let root = std::env::temp_dir().join(format!("kl-mirror-title-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (mut source, writer) = fixture(&root);
        writer
            .execute_batch(
                "ALTER TABLE session ADD COLUMN title TEXT;
                 INSERT INTO session VALUES('s','/w',1,'整理脚本');",
            )
            .unwrap();
        source.sessions = "SELECT id AS _session, dir AS _cwd, updated AS _t, title AS _title FROM session WHERE updated > ?1".into();
        let out = root.join("out");
        let mut mirror = Mirror::open(&out).unwrap();
        let mut db = None;
        mirror.poll(&source, &mut db).unwrap();
        let titles = |file: &Path| -> Vec<String> {
            lines(file)
                .iter()
                .filter_map(|l| l["_title"].as_str().map(str::to_owned))
                .collect()
        };
        assert_eq!(titles(&out.join("s.jsonl")), ["整理脚本"]);
        // Touched with the same title: nothing new.
        writer
            .execute_batch("UPDATE session SET updated = 2")
            .unwrap();
        mirror.poll(&source, &mut db).unwrap();
        assert_eq!(titles(&out.join("s.jsonl")), ["整理脚本"]);
        // Renamed without a new message: the rename alone is written.
        writer
            .execute_batch("UPDATE session SET updated = 3, title = '整理并提交脚本'")
            .unwrap();
        mirror.poll(&source, &mut db).unwrap();
        assert_eq!(titles(&out.join("s.jsonl")), ["整理脚本", "整理并提交脚本"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unreadable_cursors_write_nothing_again() {
        let root = std::env::temp_dir().join(format!("kl-mirror-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let (source, writer) = fixture(&root);
        writer
            .execute_batch(
                "INSERT INTO session VALUES('s','/w',1);
                 INSERT INTO part VALUES('a','s',1,'user','{}');",
            )
            .unwrap();
        let out = root.join("out");
        Mirror::open(&out)
            .unwrap()
            .poll(&source, &mut None)
            .unwrap();
        let before = std::fs::read(out.join("s.jsonl")).unwrap();
        std::fs::write(out.join(".cursors.json"), "{").unwrap();
        let err = Mirror::open(&out)
            .err()
            .expect("broken cursors are an error");
        assert!(err.to_string().contains(".cursors.json"), "{err}");
        assert_eq!(
            std::fs::read(out.join("s.jsonl")).unwrap(),
            before,
            "no row written twice"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_database_is_an_error_not_a_panic() {
        let source = RecordSource {
            sqlite: PathBuf::from("/nonexistent/mooring/cli.db"),
            sessions: "SELECT 1".into(),
            records: "SELECT 1".into(),
            poll_ms: 200,
        };
        let mut mirror = Mirror::open(Path::new("/nonexistent/mooring/out")).unwrap();
        let mut db = None;
        assert!(mirror.poll(&source, &mut db).is_err());
        assert!(db.is_none());
    }
}
