//! The write path: reading a log's new lines into its card and flushing them.
use super::*;

impl Index {
    pub fn hook(
        &self,
        path: &str,
        event: &str,
        facts: &mooring_core::protocol::HookFacts,
    ) -> io::Result<()> {
        self.amend(path, |parser, at, source| {
            parser.hook(event, facts, at, source)
        })
    }
    pub fn official(&self, path: &str, status: &crate::content::Official) -> io::Result<()> {
        self.amend(path, |parser, at, source| {
            parser.official(status, at, source)
        })
    }
    /// Applies a live signal that has no transcript offset; its source is the arrival time.
    fn amend(
        &self,
        path: &str,
        apply: impl FnOnce(&mut crate::content::Parser, u64, mooring_core::content::Source),
    ) -> io::Result<()> {
        let mut held = self
            .gate
            .lock()
            .map_err(|_| io::Error::other("log cursor lock poisoned"))?;
        let source = mooring_core::content::Source {
            path: path.into(),
            offset: std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(err)?
                .as_nanos() as u64,
            ..Default::default()
        };
        // A log being read: its entry takes the signal and is written with its cursor.
        if let Some(e) = held.entries.get_mut(path) {
            apply(&mut e.parser, mooring_core::content::now(), source);
            let tx = self.db.unchecked_transaction().map_err(err)?;
            let written = write_cursor(&tx, path, e.runtime, &e.cursor)
                .and_then(|()| crate::content::save(&tx, path, &e.parser))
                .and_then(|()| tx.commit().map_err(err));
            e.parser.records.clear();
            e.parser.transitions.clear();
            e.dirty = None;
            if written.is_err() {
                held.entries.remove(path);
            }
            return written;
        }
        let mut parser = crate::content::load(&self.db, path)?;
        apply(&mut parser, mooring_core::content::now(), source);
        let tx = self.db.unchecked_transaction().map_err(err)?;
        crate::content::save(&tx, path, &parser)?;
        tx.commit().map_err(err)?;
        drop(held);
        Ok(())
    }
    /// A first read of a recent log takes its last 128 KiB. When that tail is the middle
    /// of a turn (replies or tool results, but no start or end), the turn began earlier:
    /// look back, up to 8 MiB before the tail, for the last record that settled the turn.
    /// Otherwise a turn that wrote more than the tail reads as unknown, and a running Agent
    /// shows as finished. A tail with no turn activity costs nothing more.
    fn find_turn_start(
        &mut self,
        runtime: Agent,
        file: &mut File,
        tail: u64,
        parser: &mut crate::content::Parser,
    ) -> io::Result<()> {
        let mid_turn = parser
            .records
            .iter()
            .any(|r| matches!(r.kind.as_str(), "assistant" | "tool_result"));
        if parser.card.turn != mooring_core::content::Turn::Unknown || !mid_turn {
            return Ok(());
        }
        for window in [1024 * 1024u64, 8 * 1024 * 1024] {
            let start = tail.saturating_sub(window);
            file.seek(SeekFrom::Start(start))?;
            let mut body = Vec::new();
            (&mut *file).take(tail - start).read_to_end(&mut body)?;
            self.counts.bytes += body.len() as u64;
            // A window that starts mid-line drops that line; the tail's own first line,
            // cut where the tail began, was never read.
            let lines = if start == 0 {
                &body[..]
            } else {
                body.iter()
                    .position(|b| *b == b'\n')
                    .map_or(&body[..0], |i| &body[i + 1..])
            };
            let mut probe = crate::content::Parser::default();
            for line in lines.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                probe.add(runtime, line, mooring_core::content::Source::default());
            }
            // The tail settled nothing, so the turn stands where this stretch left it.
            if probe.card.turn != mooring_core::content::Turn::Unknown {
                parser.card.turn = probe.card.turn;
                parser.card.turn_started = probe.card.turn_started;
                parser.card.turn_key = probe.card.turn_key;
                return Ok(());
            }
            if start == 0 {
                return Ok(());
            }
        }
        Ok(())
    }
    /// Read [offset, captured size) only; a bounded partial line survives restart.
    /// First import reads a 16 KiB identity header and at most 128 KiB of recent content.
    pub fn update(
        &mut self,
        runtime: Agent,
        path: &Path,
        historical: bool,
    ) -> io::Result<Option<Transcript>> {
        let gate = self.gate.clone();
        let mut held = gate
            .lock()
            .map_err(|_| io::Error::other("log cursor lock poisoned"))?;
        // Walks and repairs write through: their directory leaves the worklist once done.
        let result = self.update_unlocked(&mut held, runtime, path, historical, false);
        if result.is_err() {
            held.entries.clear();
            held.pending_event = None;
        }
        result
    }
    /// Writes every entry that is ahead of the database, then the checkpoint it held back.
    pub fn flush(&mut self) -> io::Result<()> {
        let gate = self.gate.clone();
        let mut held = gate
            .lock()
            .map_err(|_| io::Error::other("log cursor lock poisoned"))?;
        self.db.execute_batch("BEGIN IMMEDIATE").map_err(err)?;
        let result = self.flush_unlocked(&mut held);
        self.finish(&mut held, result)
    }
    /// How long until an unflushed entry is due; `None` when nothing waits, so an idle
    /// subscriber keeps blocking on events alone.
    pub fn flush_due(&self) -> Option<Duration> {
        let oldest = self.gate.lock().ok()?.oldest_dirty()?;
        Some(FLUSH_AFTER.saturating_sub(oldest.elapsed()))
    }
    fn flush_unlocked(&mut self, held: &mut Held) -> io::Result<()> {
        for (key, e) in held.entries.iter_mut() {
            if e.dirty.take().is_some() {
                write_cursor(&self.db, key, e.runtime, &e.cursor)?;
                crate::content::save_state(&self.db, key, &e.parser)?;
                self.counts.writes += 1;
            }
        }
        if let Some(event) = held.pending_event.take() {
            self.checkpoint(event)?;
        }
        held.trim();
        Ok(())
    }
    /// Commits a batch, or rolls it back and forgets what memory held beyond the
    /// database: the subscriber restarts from the checkpoint and reads it again.
    fn finish<T>(&mut self, held: &mut Held, result: io::Result<T>) -> io::Result<T> {
        let result = result.and_then(|v| self.db.execute_batch("COMMIT").map_err(err).map(|()| v));
        if result.is_err() {
            let _ = self.db.execute_batch("ROLLBACK");
            held.entries.clear();
            held.pending_event = None;
        }
        result
    }
    /// Coalesced native notifications share one durable commit, with no timer or durability loss.
    pub fn update_many(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        event: u64,
    ) -> io::Result<Vec<Transcript>> {
        let gate = self.gate.clone();
        let mut held = gate
            .lock()
            .map_err(|_| io::Error::other("log cursor lock poisoned"))?;
        self.db.execute_batch("BEGIN IMMEDIATE").map_err(err)?;
        let result = (|| {
            let mut found = vec![];
            for path in paths {
                if let Some(t) = self.refresh_subagents(&mut held, &path)? {
                    if !found.iter().any(|f: &Transcript| f.path == t.path) {
                        found.push(t);
                    }
                    continue;
                }
                if let Some(runtime) = self.runtime(&path) {
                    if let Some(t) = self.update_unlocked(&mut held, runtime, &path, true, true)? {
                        found.push(t);
                    }
                }
            }
            held.pending_event = held.pending_event.max(Some(event));
            if held
                .oldest_dirty()
                .is_none_or(|t| t.elapsed() >= FLUSH_AFTER)
            {
                self.flush_unlocked(&mut held)?;
            }
            // A subagent's log moved its parent's card; the parent is refreshed with it.
            for t in std::mem::take(&mut self.touched) {
                if !found.iter().any(|f: &Transcript| f.path == t.path) {
                    found.push(t);
                }
            }
            Ok(found)
        })();
        self.finish(&mut held, result)
    }
    /// The adapter maps its own metadata layout back to the parent transcript. A child
    /// can start or finish without appending anything to that parent's log.
    fn refresh_subagents(
        &mut self,
        held: &mut Held,
        meta: &Path,
    ) -> io::Result<Option<Transcript>> {
        let Some(runtime) = self.runtime(meta) else {
            return Ok(None);
        };
        let Some(parent) = crate::agents::adapter(runtime).subagent_parent(meta) else {
            return Ok(None);
        };
        let Some(all) = crate::agents::adapter(runtime).subagents(&parent) else {
            return Ok(None);
        };
        let key = parent.to_string_lossy().into_owned();
        let row = self
            .db
            .prepare_cached(
                "SELECT cli_id,cwd,updated FROM log_files WHERE path=?1 AND cli_id IS NOT NULL",
            )
            .map_err(err)?
            .query_row([&key], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? as u64,
                ))
            })
            .optional()
            .map_err(err)?;
        // A parent not indexed yet reads its subagents when its log is.
        let Some((cli_id, cwd, updated)) = row else {
            return Ok(None);
        };
        if let Some(e) = held.entries.get_mut(&key) {
            e.subagents = Some(all.clone());
            let before = e.parser.card.subagents.clone();
            e.parser.card.keep_subagents(all);
            if e.parser.card.subagents == before {
                return Ok(None);
            }
            e.dirty.get_or_insert_with(std::time::Instant::now);
        } else {
            let mut parser = crate::content::load(&self.db, &key)?;
            let before = parser.card.subagents.clone();
            parser.card.keep_subagents(all);
            if parser.card.subagents == before {
                return Ok(None);
            }
            crate::content::save(&self.db, &key, &parser)?;
            self.counts.writes += 1;
        }
        Ok(Some(Transcript {
            runtime,
            cli_id,
            path: key,
            cwd,
            updated,
        }))
    }
    /// `defer`: a log that keeps moving stays ahead of the database in `held` for up to
    /// `FLUSH_AFTER`; its first read, a replacement and a newly known identity are
    /// written at once, since walks and `cached` read them from the database.
    fn update_unlocked(
        &mut self,
        held: &mut Held,
        runtime: Agent,
        path: &Path,
        historical: bool,
        defer: bool,
    ) -> io::Result<Option<Transcript>> {
        let adapter = crate::agents::adapter(runtime);
        if !adapter.is_transcript(path) {
            return Ok(None);
        }
        let key = path.to_string_lossy().into_owned();
        // The entry, when this log is held, is newer than the database.
        let entry = held.entries.remove(&key);
        let ahead = entry.as_ref().and_then(|e| e.dirty);
        let (mut cursor, stored, mut subagents) = match entry {
            Some(e) => (Some(e.cursor), Some(e.parser), e.subagents),
            None => (None, None, None),
        };
        let in_db = ahead.is_some()
            || cursor.is_some()
            || {
                cursor = self.db.prepare_cached("SELECT dev,inode,offset,pending,discard,cli_id,cwd,updated FROM log_files WHERE path=?1").map_err(err)?.query_row([&key],|r| {
            let id: Option<String> = r.get(5)?; let cwd: Option<String> = r.get(6)?;
            Ok(Cursor { dev:r.get::<_,i64>(0)? as u64,inode:r.get::<_,i64>(1)? as u64,offset:r.get::<_,i64>(2)? as u64,pending:r.get(3)?,discard:r.get(4)?,identity:id.zip(cwd),updated:r.get::<_,i64>(7)? as u64, elide: Elide::default() })
        }).optional().map_err(err)?;
                cursor.is_some()
            };
        // No symlink traversal in event handling, just as in the initial directory walk.
        let meta = match fs::symlink_metadata(path) {
            Ok(m) if m.is_file() => m,
            Ok(_) => return Ok(None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.db
                    .execute("DELETE FROM log_files WHERE path=?1", [&key])
                    .map_err(err)?;
                self.counts.writes += 1;
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let updated = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(err)?
            .as_secs();
        // The saved state is read once: whether it is current decides both the early return
        // and whether reading continues from it. A held entry was saved under these rules.
        let stored = match stored {
            Some(p) => p,
            None => crate::content::load(&self.db, &key)?,
        };
        let seeded = stored.rules == crate::content::RULES;
        let known = cursor.as_ref().is_some_and(|c| c.identity.is_some());
        // Replaced or rewritten shorter: what was read from it before is gone too.
        let mut replaced = false;
        if let Some(c) = &cursor {
            if c.dev != meta.dev() || c.inode != meta.ino() || meta.len() < c.offset {
                cursor = None;
                replaced = true;
            } else if c.offset == meta.len() && (seeded || !self.recent(updated)) {
                let found = c.identity.as_ref().map(|(id, cwd)| Transcript {
                    runtime,
                    cli_id: id.clone(),
                    cwd: cwd.clone(),
                    path: key.clone(),
                    updated: c.updated,
                });
                if in_db && seeded && (ahead.is_some() || defer) {
                    held.entries.insert(
                        key,
                        Entry {
                            runtime,
                            cursor: cursor.unwrap_or_default(),
                            parser: stored,
                            dirty: ahead,
                            subagents,
                            used: std::time::Instant::now(),
                        },
                    );
                }
                return Ok(found);
            }
        }
        let fresh = cursor.is_none();
        let mut c = cursor.unwrap_or_default();
        c.elide = Elide::over(&c.pending);
        self.counts.opens += 1;
        let mut file = match File::open(path) {
            Ok(f) => f,
            // One log this user cannot read (written by `sudo claude`, say) is skipped;
            // failing here would stop every other session from updating.
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("跳过读不了的对话记录 {}：{e}", path.display());
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let opened = file.metadata()?;
        if opened.dev() != meta.dev() || opened.ino() != meta.ino() {
            return Err(io::Error::other(
                "log rotated while opening; cursor not advanced",
            ));
        }
        let mut parser = if fresh || !seeded {
            crate::content::Parser::default()
        } else {
            stored
        };
        if !parser.card.interactive && parser.card.hidden_reason.is_empty() {
            adapter.observe_file(&mut parser, path);
        }
        let seed = (fresh && historical) || (!seeded && !fresh);
        // Where a first read of a recent log's body began, when it skipped earlier records.
        let mut tail_from = None;
        if seed {
            // Identity requires only the header, even when old bodies are deliberately skipped.
            file.seek(SeekFrom::Start(0))?;
            // Recent files get one cached prefix search: 64 KiB steps, at most 384 KiB.
            // Real Codex developer/environment records often push the first user past 128 KiB.
            // Old files retain the cheap identity-only 16 KiB read.
            let recent = self.recent(updated);
            let budget = meta.len().min(if recent { 384 * 1024 } else { 16 * 1024 });
            let mut head = Vec::new();
            let mut parsed = 0usize;
            while (head.len() as u64) < budget {
                let end = budget.min(head.len() as u64 + if recent { 64 * 1024 } else { 16 * 1024 })
                    as usize;
                let old = head.len();
                head.resize(end, 0);
                file.read_exact(&mut head[old..])?;
                self.counts.bytes += (end - old) as u64;
                for line in head[parsed..].split_inclusive(|b| *b == b'\n') {
                    parser.observe_header(runtime, line);
                    if c.identity.is_none() {
                        c.identity = crate::agents::identity(runtime, path, line);
                    }
                    if line.last() != Some(&b'\n') {
                        break;
                    }
                    if recent && !parser.card.first_ask_known {
                        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(line) {
                            if adapter.is_user(&v) {
                                parser.add(
                                    runtime,
                                    line,
                                    mooring_core::content::Source {
                                        path: key.clone(),
                                        inode: meta.ino(),
                                        offset: parsed as u64,
                                    },
                                );
                            }
                        }
                    }
                    parsed += line.len();
                }
                if parser.card.first_ask_known || !parser.card.hidden_reason.is_empty() || !recent {
                    break;
                }
            }
            c.offset = if recent {
                meta.len().saturating_sub(128 * 1024)
            } else {
                meta.len()
            };
            tail_from = (recent && c.offset > 0).then_some(c.offset);
            // The prefix is original intent, never proof of the current turn.
            parser.card.turn = mooring_core::content::Turn::Unknown;
            c.pending.clear();
            c.elide = Elide::default();
            c.discard = c.offset > 0;
            parser.card.partial_history = c.offset > 0;
            parser.card.from_offset = c.offset;
            // One preceding byte lets a tail beginning exactly on a line boundary remain usable.
            if c.offset > 0 {
                file.seek(SeekFrom::Start(c.offset - 1))?;
                let mut b = [0];
                file.read_exact(&mut b)?;
                self.counts.bytes += 1;
                c.discard = b[0] != b'\n';
            }
        }
        file.seek(SeekFrom::Start(c.offset))?;
        let end = meta.len();
        let mut buffer = [0u8; 16 * 1024];
        let mut line_offset = c.offset.saturating_sub(c.pending.len() as u64);
        while c.offset < end {
            let want = ((end - c.offset) as usize).min(buffer.len());
            let n = file.read(&mut buffer[..want])?;
            if n == 0 {
                break;
            }
            self.counts.bytes += n as u64;
            for byte in &buffer[..n] {
                c.offset += 1;
                if *byte == b'\n' {
                    if !c.discard {
                        if c.identity.is_none() {
                            c.identity = crate::agents::identity(runtime, path, &c.pending);
                        }
                        parser.add(
                            runtime,
                            &c.pending,
                            mooring_core::content::Source {
                                path: key.clone(),
                                inode: meta.ino(),
                                offset: line_offset,
                            },
                        );
                    } else {
                        parser.card.partial_history = true;
                    }
                    c.pending.clear();
                    c.elide = Elide::default();
                    c.discard = false;
                    line_offset = c.offset;
                } else if !c.discard {
                    if c.pending.len() == 256 * 1024 {
                        c.pending.clear();
                        c.discard = true;
                        parser.card.unknown_records += 1;
                    } else if c.elide.keep(*byte) {
                        c.pending.push(*byte);
                    }
                }
            }
        }
        if let Some(from) = tail_from {
            self.find_turn_start(runtime, &mut file, from, &mut parser)?;
        }
        if self.recent(updated) {
            if let Some(q) = c
                .identity
                .as_ref()
                .and_then(|(id, _)| adapter.account(path, id))
            {
                if parser.card.quota.as_ref().is_none_or(|old| q.at >= old.at) {
                    parser.card.quota = Some(q);
                }
            }
        }
        if replaced {
            subagents = None;
        }
        if subagents.is_none() {
            subagents = adapter.subagents(path);
        }
        if let Some(all) = &subagents {
            parser.card.keep_subagents(all.clone());
        }
        c.dev = meta.dev();
        c.inode = meta.ino();
        c.updated = updated;
        let tx = self.db.savepoint().map_err(err)?;
        // Records are keyed by where they were read: written now, read again harmlessly.
        let now_written =
            !defer || !in_db || replaced || !seeded || (c.identity.is_some() && !known);
        if now_written {
            write_cursor(&tx, &key, runtime, &c)?;
        }
        if !seeded || replaced {
            tx.execute("DELETE FROM session_records WHERE path=?1", [&key])
                .map_err(err)?;
            tx.execute("INSERT INTO log_generations VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET generation=excluded.generation",
                rusqlite::params![key, uuid::Uuid::new_v4().to_string()]).map_err(err)?;
        }
        // Subagents with their own log: the child's log is the evidence of what it is doing.
        let now = mooring_core::content::now();
        let live = |path: &str| mooring_core::content::Source {
            path: path.into(),
            offset: now,
            ..Default::default()
        };
        let mut parent = None;
        if adapter.links_children() {
            if !parser.card.parent.is_empty() {
                crate::content::link_child(&tx, &key, &parser.card)?;
                parent = Some(parser.card.parent.clone());
            } else if let Some((id, _)) = &c.identity {
                let kids =
                    crate::content::children(&tx, runtime, id, now, parser.card.turn_started)?;
                parser.children(&kids, now, live(&key));
            }
        }
        crate::content::save_records(&tx, &key, &parser)?;
        if now_written {
            crate::content::save_state(&tx, &key, &parser)?;
        }
        if let Some(parent) = parent {
            let row = tx
                .prepare_cached("SELECT cli_id,path,cwd,updated FROM log_files WHERE runtime=?2 AND cli_id=?1 LIMIT 1")
                .map_err(err)?
                .query_row([parent.as_str(), runtime.executable()], |r| {
                    Ok(Transcript {
                        runtime,
                        cli_id: r.get(0)?,
                        path: r.get(1)?,
                        cwd: r.get(2)?,
                        updated: r.get::<_, i64>(3)? as u64,
                    })
                })
                .optional()
                .map_err(err)?;
            if let Some(t) = row.filter(|t| t.path != key) {
                if let Some(e) = held.entries.get_mut(&t.path) {
                    let kids = crate::content::children(
                        &tx,
                        runtime,
                        &parent,
                        now,
                        e.parser.card.turn_started,
                    )?;
                    e.parser.children(&kids, now, live(&t.path));
                    e.dirty.get_or_insert_with(std::time::Instant::now);
                } else {
                    let mut owner = crate::content::load(&tx, &t.path)?;
                    let kids = crate::content::children(
                        &tx,
                        runtime,
                        &parent,
                        now,
                        owner.card.turn_started,
                    )?;
                    owner.children(&kids, now, live(&t.path));
                    crate::content::save(&tx, &t.path, &owner)?;
                }
                self.touched.push(t);
            }
        }
        tx.commit().map_err(err)?;
        self.counts.writes += 1;
        let found = c.identity.clone().map(|(cli_id, cwd)| Transcript {
            runtime,
            cli_id,
            path: key.clone(),
            cwd,
            updated,
        });
        // Only moving logs are held; a walk's write-through does not fill memory.
        if defer || ahead.is_some() {
            parser.records.clear();
            parser.transitions.clear();
            parser.rules = crate::content::RULES;
            held.entries.insert(
                key,
                Entry {
                    runtime,
                    cursor: c,
                    parser,
                    dirty: if now_written {
                        None
                    } else {
                        Some(ahead.unwrap_or_else(std::time::Instant::now))
                    },
                    subagents,
                    used: std::time::Instant::now(),
                },
            );
        }
        Ok(found)
    }
}
