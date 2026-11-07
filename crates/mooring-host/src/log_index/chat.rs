//! A conversation as chat for a phone (`Request::Chat`): its user and Agent messages, and
//! with `tools` what the Agent did before each of its messages.
use super::{err, Index};
use crate::agents::codex::script_patches;
use crate::content::{edit_lines, patch_lines, patch_text, LINES, PATCHED};
use mooring_core::protocol::{ChatMessage, ChatTools};
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::io;

mod backfill;

/// A record's `seq`: where it is in its log, so reading older lines later (`backfill`) keeps
/// the order and every seq already handed out. Past zero, and under 2^53 for a log under
/// 2 TiB, so a phone's JavaScript holds it exactly; no line makes 4096 records.
pub(super) const SEQ: &str = "(offset*4096+MIN(ordinal,4095)+1)";

/// The inode of the log a path's records were read from. A live signal has no place in a log,
/// so `Index::hook` stores its record under inode 0, which sorts first: taking the first row
/// instead of the largest found no messages at all once a session had one such signal.
pub(super) const LOG_INODE: &str = "(SELECT MAX(inode) FROM session_records WHERE path=?1)";

/// A call's failure may arrive beyond the page. Seek by this call's id rather than
/// collecting every later failure. ?2 and ?3 name the stretch's lower bound.
pub(super) const FAILED: &str = "EXISTS(SELECT 1 FROM session_records AS result WHERE result.path=call.path AND result.call_id=call.call_id AND result.kind='tool_result' AND result.exit_code<>0 AND (result.offset,result.ordinal)>(?2,?3))";

/// The `(offset, ordinal)` a seq names.
pub(super) fn place(seq: i64) -> (i64, i64) {
    let at = seq.saturating_sub(1);
    (at >> 12, at & 4095)
}

impl Index {
    /// Offset cursors belong to one history generation, preserved across append/restart.
    /// Reindexing a rewritten/truncated log changes it in the same transaction as records.
    pub fn chat_generation(&self, path: &str) -> io::Result<String> {
        self.db
            .query_row(
                "SELECT generation FROM log_generations WHERE path=?1",
                [path],
                |r| r.get(0),
            )
            .optional()
            .map(|s| s.unwrap_or_default())
            .map_err(err)
    }
    /// A conversation's user and Agent messages, oldest first, and whether older ones
    /// exist. See `Request::Chat`. `floor` is where the unbroken stretch up to its end begins
    /// (`read_from`): a first ask kept from a log's head would sit on the far side of what has
    /// not been read, and paging back from it would skip all of that.
    pub fn chat(
        &self,
        path: &str,
        floor: u64,
        after: Option<i64>,
        before: Option<i64>,
        limit: usize,
        tools: bool,
    ) -> io::Result<(Vec<ChatMessage>, bool)> {
        let limit = limit.clamp(1, 500) as i64;
        let (range, order, cursor) = match (after, before) {
            (Some(a), _) => (">", "ASC", a),
            (None, Some(b)) => ("<", "DESC", b),
            (None, None) => ("<", "DESC", i64::MAX),
        };
        let (offset, ordinal) = place(cursor);
        // SQLite seeks using the explicit offset bound, then tests the tuple. Leaving
        // this at the history floor makes an empty poll walk all the old records.
        let floor = if after.is_some() {
            (floor as i64).max(offset)
        } else {
            floor as i64
        };
        let sql = format!("SELECT {SEQ},at,kind,text FROM session_records WHERE path=?1 AND inode={LOG_INODE} AND kind IN ('user','assistant') AND offset>=?5 AND (offset,ordinal){range}(?2,?3) ORDER BY offset {order},ordinal {order} LIMIT ?4");
        let mut q = self.db.prepare_cached(&sql).map_err(err)?;
        let mut messages = q
            .query_map(
                rusqlite::params![path, offset, ordinal, limit + 1, floor],
                |r| {
                    Ok(ChatMessage {
                        seq: r.get(0)?,
                        at: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                        user: r.get::<_, String>(2)? == "user",
                        text: r.get(3)?,
                        tools: None,
                    })
                },
            )
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        let more = messages.len() as i64 > limit;
        if after.is_none() {
            messages.reverse();
        }
        drop_repeats(&mut messages);
        // Without `after` the row past the limit is the oldest: it spoke for `more` and for
        // the first message's repeat, and is not shown.
        let over = messages.len().saturating_sub(limit as usize);
        match after {
            None => drop(messages.drain(..over)),
            Some(_) => messages.truncate(limit as usize),
        }
        if tools {
            self.attach_tools(path, &mut messages)?;
        }
        // Newer ones beyond the limit are fetched again with `after`; `more` speaks of older.
        Ok((messages, more && after.is_none()))
    }

    /// Each Agent message's tool calls: those logged after the message before it, user's or
    /// Agent's, so a turn's calls are counted once, on the message they led to. One read of
    /// the page's whole range: this runs under the host's lock.
    fn attach_tools(&self, path: &str, messages: &mut [ChatMessage]) -> io::Result<()> {
        let (Some(first), Some(last)) = (messages.first(), messages.last()) else {
            return Ok(());
        };
        let (first, last) = (first.seq, last.seq);
        // The message before this page: where its first message's work began.
        let (from, mut before_at, mut before_user): (i64, i64, bool) = self
            .db
            .prepare_cached(&format!("SELECT {SEQ},at,kind='user' FROM session_records WHERE path=?1 AND inode={LOG_INODE} AND kind IN ('user','assistant') AND (offset,ordinal)<(?2,?3) ORDER BY offset DESC,ordinal DESC LIMIT 1"))
            .map_err(err)?
            .query_row(rusqlite::params![path, place(first).0, place(first).1], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .map_err(err)?
            .unwrap_or_default();
        let (from, until) = (place(from), place(last));
        let calls = self
            .db
            .prepare_cached(&format!("SELECT {SEQ},text,{FAILED},at FROM session_records AS call WHERE path=?1 AND kind='tool_call' AND (offset,ordinal)>(?2,?3) AND (offset,ordinal)<(?4,?5) ORDER BY offset,ordinal"))
            .map_err(err)?
            .query_map(rusqlite::params![path, from.0, from.1, until.0, until.1], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut calls = calls.into_iter().peekable();
        for m in messages.iter_mut() {
            let mut before = Vec::new();
            let mut first_call = 0;
            while let Some((_, text, failed, at)) = calls.next_if(|c| c.0 < m.seq) {
                if before.is_empty() {
                    first_call = at;
                }
                before.push((text, failed));
            }
            if !m.user {
                let began = if before_user { before_at } else { first_call };
                let mut tools = summary(&before);
                tools.took = took(began, m.at);
                m.tools = Some(tools);
            }
            (before_at, before_user) = (m.at as i64, m.user);
        }
        Ok(())
    }
}

/// How close a repeat of an Agent's words has to be to count as the same message logged
/// again: `task_complete` follows its message within seconds.
const REPEAT_WITHIN: u64 = 60;

/// Logs indexed before the parser told them apart hold a Codex turn's last message twice;
/// reindexing them would lose what only the old rows still have, so this reads past them.
fn drop_repeats(messages: &mut Vec<ChatMessage>) {
    let mut before: Option<(String, u64)> = None;
    messages.retain(|m| {
        let repeat = !m.user
            && before.as_ref().is_some_and(|(text, at)| {
                *text == m.text && m.at.saturating_sub(*at) <= REPEAT_WITHIN
            });
        if !repeat {
            before = (!m.user).then(|| (m.text.clone(), m.at));
        }
        !repeat
    });
}

/// The seconds between two logged times; a record with no time says nothing.
fn took(began: i64, at: u64) -> u64 {
    match u64::try_from(began) {
        Ok(began) if began > 0 => at.saturating_sub(began),
        _ => 0,
    }
}

/// Commands run and files changed by `tool_call` records (`Name {arguments}`), each with
/// whether it failed: a failed edit changed nothing, a failed command still ran.
pub(super) fn summary(calls: &[(String, bool)]) -> ChatTools {
    let mut out = ChatTools {
        calls: calls.len() as u32,
        ..Default::default()
    };
    // Files a Codex change item names: the script that made the change names them too.
    let items: Vec<String> = (calls.iter())
        .filter(|(c, _)| c.starts_with("apply_patch "))
        .filter_map(|(c, _)| serde_json::from_str::<Value>(c.split_once(' ')?.1).ok())
        .flat_map(|args| {
            let files: Vec<(String, u32, u32)> = serde_json::from_value(args[PATCHED].clone())
                .unwrap_or_else(|_| patch_lines(patch_text(&args)));
            files.into_iter().map(|f| f.0)
        })
        .collect();
    for (call, failed) in calls {
        let (name, args) = call.split_once(' ').unwrap_or((call, ""));
        let args: Value = serde_json::from_str(args).unwrap_or(Value::Null);
        let short = name.rsplit(['.', ':']).next().unwrap_or(name);
        if matches!(short, "Bash" | "shell" | "local_shell") || short.ends_with("exec_command") {
            out.commands += 1;
        }
        if *failed {
            continue;
        }
        let mut touched = Vec::new();
        if short == "exec" {
            // A Codex script: whole, or when long its files as counted at record time.
            touched = serde_json::from_value(args[PATCHED].clone()).unwrap_or_default();
            if let Some(script) = args["input"].as_str() {
                touched = script_patches(script);
            }
            touched.retain(|(path, _, _)| {
                !(items.iter()).any(|i| i == path || i.ends_with(&format!("/{path}")))
            });
        } else if matches!(short, "Edit" | "MultiEdit" | "Write" | "NotebookEdit") {
            let (added, removed) =
                serde_json::from_value(args[LINES].clone()).unwrap_or_else(|_| edit_lines(&args));
            for key in ["file_path", "notebook_path", "path"] {
                if let Some(path) = args[key].as_str() {
                    touched.push((path.to_owned(), added, removed));
                    break;
                }
            }
        } else if short.ends_with("apply_patch") {
            touched = serde_json::from_value(args[PATCHED].clone())
                .unwrap_or_else(|_| patch_lines(patch_text(&args)));
        }
        for (path, added, removed) in touched {
            match out.files.iter_mut().find(|f| f.0 == path) {
                Some(f) => {
                    f.1 += added;
                    f.2 += removed;
                }
                None => out.files.push((path, added, removed)),
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_summary_counts_commands_and_sums_lines_per_file() {
        let call = |name: &str, args: Value| format!("{name} {args}");
        let patch =
            "*** Begin Patch\n*** Update File: src/b.rs\n@@\n-old\n+new\n+more\n*** End Patch";
        let calls = [
            call("Bash", json!({"command": "cargo test"})),
            call(
                "Edit",
                json!({"file_path": "/r/a.rs", "old_string": "x", "new_string": "y\nz"}),
            ),
            call(
                "Edit",
                json!({"file_path": "/r/a.rs", "old_string": "q", "new_string": ""}),
            ),
            call("Read", json!({"file_path": "/r/c.rs"})),
            call("exec_command", json!({"cmd": "ls"})),
            call("apply_patch", json!({"input": patch})),
            // A long Write as `logged_args` keeps it: its counted lines, not its text.
            call("Write", json!({"file_path": "/r/big.txt", LINES: [300, 0]})),
            "Bash {\"command\":\"cut off".to_owned(),
        ];
        let mut calls: Vec<_> = calls.into_iter().map(|c| (c, false)).collect();
        // A failed edit changed nothing; a failed command still ran.
        calls.push((
            call(
                "Edit",
                json!({"file_path": "/r/failed.rs", "new_string": "y"}),
            ),
            true,
        ));
        calls.push((call("Bash", json!({"command": "false"})), true));
        let s = summary(&calls);
        assert_eq!(s.commands, 4);
        assert_eq!(
            s.files,
            [
                ("/r/a.rs".to_owned(), 2, 2),
                ("src/b.rs".to_owned(), 2, 1),
                ("/r/big.txt".to_owned(), 300, 0),
            ]
        );
    }

    /// A script's patch and the change item it produced are one change; a patch built at
    /// run time, which the script's text cannot say, is known from the item alone.
    #[test]
    fn a_change_item_and_its_script_count_once() {
        let call = |name: &str, args: Value| (format!("{name} {args}"), false);
        let patch = "*** Begin Patch\n*** Update File: src/a.rs\n@@\n-x\n+y\n*** End Patch";
        let script = format!(
            "await tools.apply_patch(\"{}\");",
            patch.replace('\n', "\\n")
        );
        let item = "*** Update File: /r/src/a.rs\n@@ -1 +1 @@\n-x\n+y\n";
        let s = summary(&[
            call("exec", json!({ "input": script })),
            call("apply_patch", json!({ "input": item })),
        ]);
        assert_eq!(s.files, [("/r/src/a.rs".to_owned(), 1, 1)]);
        let s = summary(&[
            call("exec", json!({ "input": "await tools.apply_patch(p);" })),
            call("apply_patch", json!({ "input": item })),
        ]);
        assert_eq!(s.files, [("/r/src/a.rs".to_owned(), 1, 1)]);
    }

    /// A log indexed before the parser told the copies apart: the second copy is not shown,
    /// on any page, and the same words later in the conversation still are.
    #[test]
    fn a_turns_last_message_indexed_twice_reads_as_one() {
        let dir = std::env::temp_dir().join(format!("mooring-chat-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let i = Index::open(&dir, vec![]).unwrap();
        let rows = [
            (10, "user", "改一下"),
            (20, "assistant", "好了"),
            (25, "assistant", "好了"),
            (100, "user", "再改"),
            (200, "assistant", "好了"),
            (205, "assistant", "好了"),
            (900, "assistant", "好了"),
        ];
        for (n, (at, kind, text)) in rows.iter().enumerate() {
            i.db.execute(
                "INSERT INTO session_records VALUES('p',1,?1,0,?2,?3,?4,'',NULL)",
                rusqlite::params![n as i64, at, kind, text],
            )
            .unwrap();
        }
        let words = |m: &[ChatMessage]| m.iter().map(|m| m.at).collect::<Vec<_>>();
        let (all, more) = i.chat("p", 0, None, None, 50, false).unwrap();
        assert_eq!(words(&all), [10, 20, 100, 200, 900]);
        assert!(!more);
        // A page that starts on the second copy does not show it either.
        let (page, more) = i.chat("p", 0, None, None, 2, false).unwrap();
        assert_eq!(words(&page), [200, 900]);
        assert!(more);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Codex's `exec` scripts, as real logs have them, short and past the 8 KiB record. Each
    /// command inside has a `CommandExecution` of its own, so the script itself counts none.
    #[test]
    fn a_codex_script_says_what_it_patched_and_its_commands_count_once() {
        use crate::content::Parser;
        use mooring_core::{content::Source, model::Agent};
        let patch = "*** Begin Patch\\n*** Add File: /r/new.md\\n+tools.exec_command(not a call)\\n+\\\"two\\\"\\n*** Update File: /r/old.rs\\n@@\\n-a\\n+b\\n*** End Patch";
        let script = format!("text(await tools.exec_command({{cmd:\"ls\"}}));\ntext(await tools.apply_patch(\"{patch}\"));\ntext(await tools.exec_command({{cmd:\"pwd\"}}));\n");
        let long = format!("{script}// {}", "x".repeat(9000));
        let dynamic = "const p = make(); await tools.apply_patch(p);";
        let mut p = Parser::default();
        for (id, s) in [("1", script.as_str()), ("2", long.as_str()), ("3", dynamic)] {
            let line = json!({"type":"response_item","timestamp":"2026-11-04T00:00:00Z","payload":{"type":"custom_tool_call","call_id":id,"name":"exec","input":s}});
            p.add(Agent::Codex, line.to_string().as_bytes(), Source::default());
        }
        let done = json!({"type":"event_msg","timestamp":"2026-11-04T00:00:00Z","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"exec-1","command":["/bin/zsh","-lc","ls"],"exit_code":0}}});
        p.add(Agent::Codex, done.to_string().as_bytes(), Source::default());
        let calls: Vec<_> = (p.records.iter())
            .filter(|r| r.kind == "tool_call")
            .map(|r| (r.text.clone(), false))
            .collect();
        assert!(calls[1].0.len() < 1000, "kept short: {}", calls[1].0.len());
        for call in &calls[..2] {
            let s = summary(std::slice::from_ref(call));
            assert_eq!(s.commands, 0, "{}", call.0);
            assert_eq!(
                s.files,
                [
                    ("/r/new.md".to_owned(), 2, 0),
                    ("/r/old.rs".to_owned(), 1, 1)
                ]
            );
        }
        // Built at run time: nothing to read, nothing made up; the call itself still counts.
        assert_eq!(
            summary(&calls[2..3]),
            ChatTools {
                calls: 1,
                ..Default::default()
            }
        );
        // The command the script ran, as Codex logs it once it is done.
        assert_eq!(summary(&calls[3..]).commands, 1);
    }

    /// What the content parser records for real Codex and Claude lines reads back: the
    /// tool names as each CLI logs them, and a Write too long to keep whole.
    #[test]
    fn records_from_both_clis_summarize() {
        use crate::content::Parser;
        use mooring_core::{content::Source, model::Agent};
        let mut p = Parser::default();
        let codex = |p: &mut Parser, v: Value| {
            let line =
                json!({"type":"response_item","timestamp":"2026-11-04T00:00:00Z","payload":v});
            p.add(Agent::Codex, line.to_string().as_bytes(), Source::default());
        };
        let args = json!({"cmd": "cargo test"}).to_string();
        codex(
            &mut p,
            json!({"type":"function_call","call_id":"c1","name":"exec_command","arguments":args}),
        );
        let patch = "*** Begin Patch\n*** Add File: new.rs\n+fn a() {}\n*** End Patch";
        codex(
            &mut p,
            json!({"type":"custom_tool_call","call_id":"c2","name":"apply_patch","input":patch}),
        );
        let body = "line\n".repeat(5000);
        let write = json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"w","name":"Write","input":{"file_path":"/r/long.txt","content":body}}]}});
        p.add(
            Agent::Claude,
            write.to_string().as_bytes(),
            Source::default(),
        );
        let calls: Vec<(String, bool)> = (p.records.iter())
            .filter(|r| r.kind == "tool_call")
            .map(|r| (r.text.clone(), false))
            .collect();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert!(
            calls[2].0.len() < 1000,
            "a long call is kept short: {}",
            calls[2].0.len()
        );
        let s = summary(&calls);
        assert_eq!(s.commands, 1);
        assert_eq!(
            s.files,
            [
                ("new.rs".to_owned(), 1, 0),
                ("/r/long.txt".to_owned(), 5000, 0)
            ]
        );
    }
}
