//! Claude Code: `~/.claude/projects` transcripts, every record naming its session.
use super::{home_dir, temporary, Adapter, Hint, OfficialAnswer, Pending, UsageState};
use crate::content::{claims_done, text, user_text, Parser};
use mooring_core::content::{short, Source, Turn};
use mooring_core::model::AgentSession;
use mooring_core::protocol::{TerminalNote, CLAUDE_TRANSCRIPT_OFF};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Why a conversation's older session is not listed: it went on in another one.
const CONTINUED: &str = "Claude 会话已转到另一个会话接着跑";

pub struct Claude;
impl Adapter for Claude {
    fn trust_screen(&self, lines: &[&str]) -> Option<crate::screen_ask::TrustScreen> {
        super::trust::claude(lines)
    }
    /// A Braille (up to 2.1.227) or half-circle (2.1.228 on) spinner opens the title while
    /// a turn runs; "✳" stands there between turns.
    fn title_busy(&self, title: &str) -> Option<bool> {
        let mut chars = title.trim_start().chars();
        let glyph = chars.next()?;
        if !chars.next().is_some_and(char::is_whitespace) {
            return None;
        }
        match glyph {
            '\u{2800}'..='\u{28ff}' | '◐' | '◓' | '◑' | '◒' => Some(true),
            '✳' => Some(false),
            _ => None,
        }
    }
    /// Above the input box while a turn runs: "· Seasoning… (53s · ↓ 2.0k tokens)", its
    /// glyph cycling. A finished turn leaves "✻ Worked for 16m 1s", with no ellipsis.
    fn screen_busy(&self, tail: &str) -> Option<bool> {
        Some(super::bottom_lines(tail, 12).any(|l| {
            let mut chars = l.chars();
            let spinner = chars.next().is_some_and(|c| "·✢✳✶✻✽*".contains(c))
                && chars.next() == Some(' ')
                && chars.as_str().split_once('…').is_some_and(|(verb, rest)| {
                    !verb.is_empty() && rest.trim_start().starts_with('(')
                });
            spinner || l.contains("esc to interrupt")
        }))
    }
    fn screen_rules(&self) -> &'static str {
        include_str!("screen/claude.json")
    }
    fn input_suggestion(&self, vt: &mooring_core::terminal::Vt) -> std::io::Result<Option<String>> {
        super::claude_suggestion::read(vt)
    }
    fn root(&self, home: &Path) -> PathBuf {
        home_dir(home, "CLAUDE_CONFIG_DIR", ".claude").join("projects")
    }
    fn reads_tools(&self) -> bool {
        true
    }
    fn edit_marker(&self) -> Option<&'static [u8]> {
        Some(b"\"structuredPatch\"")
    }
    fn logged_edits(&self, line: &Value) -> Vec<(String, mooring_core::turn_diff::Edit)> {
        super::turn_edits::claude(line)
    }
    fn tool_call_line(&self, line: &Value, id: &str) -> Option<super::ToolCallLine> {
        super::tool_lines::claude_call(line, id)
    }
    fn tool_result_line(&self, line: &Value, id: &str) -> Option<super::ToolOutput> {
        super::tool_lines::claude_result(line, id)
    }
    fn usage_marker(&self) -> Option<&'static [u8]> {
        Some(b"\"usage\"")
    }
    fn usage_line(&self, line: &[u8], since: u64, state: &mut UsageState) -> Option<(u64, u64)> {
        super::usage_lines::claude_line(line, since, state.seen)
    }
    fn runs(&self, executable: &str, script: Option<&str>) -> bool {
        executable == "claude"
            || script.is_some_and(|s| s.ends_with("/@anthropic-ai/claude-code/cli.js"))
    }
    fn process_session(&self, _pid: u32, args: &[String]) -> (Option<String>, bool) {
        (claude_session_id(args), false)
    }
    fn launch_flags(&self, args: &[String]) -> Vec<String> {
        super::carried(
            args,
            "claude",
            &[
                "--dangerously-skip-permissions",
                "--allow-dangerously-skip-permissions",
                "--strict-mcp-config",
                "--chrome",
                "--no-chrome",
                "--ide",
                "--verbose",
                "--bare",
                "--safe-mode",
                "--restricted",
                "--disable-slash-commands",
                "--brief",
            ],
            &[
                "--model",
                "--permission-mode",
                "--effort",
                "--fallback-model",
                "--agent",
                "--agents",
                "--settings",
                "--setting-sources",
                "--append-system-prompt",
                "--system-prompt",
                "--plugin-dir",
                "--autocompact",
            ],
            &[
                "--add-dir",
                "--mcp-config",
                "--allowedTools",
                "--allowed-tools",
                "--disallowedTools",
                "--disallowed-tools",
                "--tools",
                "--betas",
            ],
        )
    }
    /// A hand-started Claude names no session and closes its transcript between writes;
    /// its hooks are the only evidence before the first prompt reaches the screen.
    fn hooks(&self, home: &Path) -> Option<super::Hooks> {
        Some(super::Hooks {
            settings: home_dir(home, "CLAUDE_CONFIG_DIR", ".claude").join("settings.json"),
            events: &[
                "SessionStart",
                "UserPromptSubmit",
                "Notification",
                "PermissionRequest",
                "Stop",
                "SessionEnd",
            ],
        })
    }
    fn subagent_parent(&self, path: &Path) -> Option<PathBuf> {
        subagent_parent(path)
    }
    fn subagents(&self, transcript: &Path) -> Option<Vec<mooring_core::content::Subagent>> {
        Some(subagents(&transcript.with_extension("").join("subagents")))
    }
    fn identity(&self, _path: &Path, value: &Value) -> Option<(String, String)> {
        if value["isSidechain"] == true {
            return Some((
                format!(
                    "{}:sidechain:{}",
                    value["sessionId"].as_str()?,
                    value["agentId"]
                        .as_str()
                        .or_else(|| value["uuid"].as_str())?
                ),
                value["cwd"].as_str()?.into(),
            ));
        }
        Some((
            value["sessionId"].as_str()?.into(),
            value["cwd"].as_str()?.into(),
        ))
    }
    fn metadata(&self, parser: &mut Parser, m: &Value) {
        if temporary(m["cwd"].as_str().unwrap_or("")) {
            parser.hide(mooring_core::content::TEMPORARY_DIRECTORY);
        } else {
            let entry = m["entrypoint"].as_str().unwrap_or("");
            let user_type = m["userType"].as_str().unwrap_or("");
            if m["isSidechain"] == true
                || m.get("agentId")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
            {
                parser.hide("Claude 子代理 / sidechain");
            } else if matches!(entry, "hook" | "sdk-cli")
                || m["isInteractive"] == false
                || m["argv"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|x| x == "-p" || x == "--print"))
                || entry.contains("sdk")
                || entry.contains("print")
                || m["isNonInteractive"] == true
                || m["print"] == true
            {
                parser.hide("Claude 非交互 / SDK");
            } else if !user_type.is_empty() && user_type != "external" {
                parser.hide("Claude 内部 / Hook 用户来源");
            } else if matches!(entry, "cli" | "claude-desktop" | "vscode")
                && user_type == "external"
            {
                parser.card.interactive = true;
            }
        }
        // `/rename` and the desktop app write the conversation's title; the last one wins.
        // The CLI's own generated title only names a conversation nobody titled.
        if m["type"] == "custom-title" {
            if let Some(name) = m["customTitle"].as_str() {
                parser.card.name = name.trim().to_owned();
                parser.custom_name = true;
            }
        } else if m["type"] == "ai-title" && !parser.custom_name {
            if let Some(name) = m["aiTitle"].as_str() {
                parser.card.name = name.trim().to_owned();
            }
        }
        let said = match m["type"].as_str() {
            Some("queue-operation") => text(&m["content"]),
            Some("user") => text(&m["message"]["content"]),
            _ => return,
        };
        parser.machine_prompt(&said);
        parser.card.note_task_notification(&said);
    }
    fn is_user(&self, v: &Value) -> bool {
        v["type"] == "user"
    }
    fn add(&self, parser: &mut Parser, v: &Value, at: u64, source: &Source) {
        let ty = v["type"].as_str().unwrap_or("");
        if v["isSidechain"] == true {
            return;
        }
        parser.inherited = inherited(v, source);
        // Written whenever the mode is in force, and again on each prompt.
        if ty == "permission-mode" || ty == "user" {
            if let Some(mode) = v["permissionMode"].as_str() {
                parser.card.permission = super::permission::claude(mode);
            }
        }
        // Handed to Claude Code's background daemon: the conversation goes on in a forked
        // session, which holds this one's history. This turn is over here, and this copy
        // would show the same conversation twice.
        if ty == "continued-in" {
            if let Some(next) = v["continuedInSessionId"].as_str().filter(|s| !s.is_empty()) {
                parser.card.continued_in = next.to_owned();
                parser.hide(CONTINUED);
                parser.phase(Turn::Completed, "", at, source);
            }
            return;
        }
        let msg = &v["message"];
        let blocks = &msg["content"];
        // The summary a compaction writes is shaped like a prompt but asks nothing:
        // a conversation compacted after its last reply has still finished.
        if ty == "user" && v["isMeta"] != true && v["isCompactSummary"] != true {
            let said = text(blocks);
            // Esc: Claude Code records this line and, most times, nothing else until the
            // next prompt; no Stop hook fires. The turn ended there.
            if said
                .trim_start()
                .starts_with("[Request interrupted by user")
            {
                parser.phase(Turn::Interrupted, "", at, source);
            }
            let s = user_text(&said);
            if !s.is_empty() {
                // Resumed here again: this session is the one in use once more.
                if !parser.card.continued_in.is_empty() {
                    parser.card.continued_in.clear();
                    if parser.card.hidden_reason == CONTINUED {
                        parser.card.hidden_reason.clear();
                    }
                }
                parser.phase(Turn::Running, v["uuid"].as_str().unwrap_or(""), at, source);
                parser.message(true, s, at, source, false);
            }
        }
        // Only the conversation's own lines carry its calls. `api-request-blob` lines replay
        // earlier messages, calls long answered included; read as new, every replayed call
        // would wait for a result that already came.
        let own = matches!(ty, "user" | "assistant");
        if let Some(blocks) = blocks.as_array().filter(|_| own) {
            for b in blocks {
                match b["type"].as_str() {
                    Some("tool_use") => parser.tool(
                        b["id"].as_str().unwrap_or(""),
                        b["name"].as_str().unwrap_or(""),
                        b["input"].clone(),
                        at,
                        source,
                    ),
                    Some("tool_result") => parser.result(
                        b["tool_use_id"].as_str().unwrap_or(""),
                        &b["content"],
                        if b["is_error"] == true { Some(1) } else { None },
                        v["toolUseResult"].get("backgroundTaskId").is_none(),
                        at,
                        source,
                    ),
                    _ => (),
                }
            }
        }
        if v["isApiErrorMessage"] == true {
            parser.card.error = short(&text(blocks), 600);
            parser.phase(Turn::Error, "", at, source);
            return;
        }
        if ty == "assistant" && msg["model"] != "<synthetic>" {
            if let Some(model) = msg["model"].as_str() {
                parser.card.model = model.to_owned();
            }
            let final_answer = matches!(
                msg["stop_reason"].as_str(),
                Some("end_turn" | "stop_sequence")
            );
            parser.message(false, text(blocks), at, source, final_answer);
            if final_answer {
                parser.phase(Turn::Completed, "", at, source);
            }
        }
        if ty == "system" && matches!(v["subtype"].as_str(), Some("turn_duration" | "stop")) {
            parser.card.completed_claim = claims_done(&parser.card.last_reply);
            parser.phase(Turn::Completed, "", at, source);
        }
    }
    /// Claude Code writes no transcript when it believes it is a nested child session, so
    /// nothing can ever bind it.
    fn pending(
        &self,
        process: &crate::recognition::Process,
        processes: &[crate::recognition::Process],
    ) -> Pending {
        if !crate::recognition::claude_transcript_off(process.pid) {
            return super::unclaimed(process, processes);
        }
        Pending::Unrecorded(Hint {
            text: CLAUDE_TRANSCRIPT_OFF.into(),
            note: TerminalNote {
                note: "claude · 没有保存记录".into(),
                why: "这个 Claude 继承了 CLAUDE_CODE_CHILD_SESSION，自认为是别的会话的子会话，不写对话记录，Mooring 读不到也统计不了。关掉这个终端重新打开，再运行 claude。".into(),
            },
        })
    }
    fn official(&self, due: &[&(AgentSession, Option<u32>)]) -> Option<OfficialAnswer> {
        Some(("claude agents --json", claude_status(due)))
    }
    /// A Claude started without an id on its command line keeps no transcript open, so
    /// neither argv nor lsof names it. `claude agents --json` lists each live session with
    /// its pid: exact evidence. A failed query names nothing.
    fn sessions_by_pid(&self, pids: &[u32]) -> Vec<(u32, String)> {
        let Some(program) = std::env::var("MOORING_CLAUDE_AGENTS").ok().or_else(|| {
            pids.iter()
                .find_map(|&p| crate::official::claude_program(p).ok())
        }) else {
            return vec![];
        };
        let Ok(entries) = crate::official::query(&program) else {
            return vec![];
        };
        pids.iter()
            .filter_map(|&pid| {
                let mut named = entries.iter().filter(|e| e.pid == Some(pid));
                match (named.next(), named.next()) {
                    (Some(entry), None) => Some((pid, entry.session_id.clone())),
                    _ => None,
                }
            })
            .collect()
    }
}

fn claude_status(
    due: &[&(AgentSession, Option<u32>)],
) -> std::io::Result<Vec<(usize, crate::content::Official)>> {
    let program = match std::env::var("MOORING_CLAUDE_AGENTS") {
        Ok(program) => program,
        Err(_) => due
            .iter()
            .find_map(|(_, pid)| pid.and_then(|p| crate::official::claude_program(p).ok()))
            .ok_or_else(|| std::io::Error::other("no live Claude process to ask"))?,
    };
    let entries = crate::official::query(&program)?;
    Ok(due
        .iter()
        .enumerate()
        .filter_map(|(i, (session, pid))| {
            let entry = entries.iter().find(|e| {
                e.session_id == session.cli_session_id || (pid.is_some() && e.pid == *pid)
            })?;
            Some((i, entry.status.clone()))
        })
        .collect())
}

pub(crate) fn claude_session_id(args: &[String]) -> Option<String> {
    let mut found = None;
    let mut args = args.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        // A fork receives a new id. Its resume argument names the source, not this process.
        if arg == "--fork-session" {
            return None;
        }
        if matches!(
            arg.as_str(),
            "--model"
                | "--agent"
                | "--agents"
                | "--settings"
                | "--setting-sources"
                | "--append-system-prompt"
                | "--system-prompt"
                | "--permission-mode"
                | "--effort"
                | "--fallback-model"
                | "--output-format"
                | "--input-format"
                | "--json-schema"
        ) {
            args.next();
            continue;
        }
        let value = match arg.as_str() {
            "--session-id" | "--resume" | "-r" => args.next().map(String::as_str),
            _ => arg
                .strip_prefix("--session-id=")
                .or_else(|| arg.strip_prefix("--resume=")),
        };
        if let Some(value) = value {
            let id = uuid::Uuid::parse_str(value).ok()?.to_string();
            if found.is_some() {
                return None;
            }
            found = Some(id);
        }
    }
    found
}

fn subagent_parent(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    if !(name.starts_with("agent-") && name.ends_with(".meta.json")) {
        return None;
    }
    let dir = path.parent()?;
    if dir.file_name()? != "subagents" {
        return None;
    }
    let session = dir.parent()?;
    let mut parent = session.as_os_str().to_owned();
    parent.push(".jsonl");
    Some(PathBuf::from(parent))
}

/// A forked or branched conversation starts with the original's records copied in, each
/// still naming the original `sessionId`; the transcript itself is named by its own id.
fn inherited(v: &Value, source: &Source) -> bool {
    let own = Path::new(&source.path)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| s.len() == 36 && s.matches('-').count() == 4);
    match (own, v["sessionId"].as_str()) {
        (Some(own), Some(id)) => own != id,
        _ => false,
    }
}
fn unix(t: std::io::Result<std::time::SystemTime>) -> Option<u64> {
    t.ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}
/// Claude Code writes `agent-<id>.meta.json` (its description, type, how it was requested,
/// and `finished` once done, or `stoppedByUser`) and `agent-<id>.jsonl` (its own log) for
/// each subagent. It started when its log was created; it ended when its meta was last
/// written with either mark.
pub(crate) fn subagents(dir: &Path) -> Vec<mooring_core::content::Subagent> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?;
            let id = name.strip_prefix("agent-")?.strip_suffix(".meta.json")?;
            let meta = std::fs::metadata(&path).ok()?;
            let v: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
            let log = std::fs::metadata(dir.join(format!("agent-{id}.jsonl"))).ok();
            let written = unix(meta.modified())?;
            let started = log
                .as_ref()
                .and_then(|m| unix(m.created()))
                .or_else(|| unix(meta.created()))
                .unwrap_or(written)
                .min(written);
            let name = [&v["description"], &v["agentType"]]
                .into_iter()
                .filter_map(|n| n.as_str().map(str::trim))
                .find(|n| !n.is_empty())
                .unwrap_or("")
                .to_owned();
            let stopped = v["stoppedByUser"] == true;
            Some(mooring_core::content::Subagent {
                id: id.to_owned(),
                name,
                started,
                ended: (v["finished"] == true || stopped).then_some(written),
                // Older Claude Code wrote no shape; its subagents ran in the foreground.
                foreground: v["requestShape"].as_str() != Some("background"),
                stopped,
            })
        })
        .collect();
    out.sort_by_key(|s| s.started);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagents_come_from_their_meta_files_oldest_first() {
        let dir = std::env::temp_dir().join(format!("mooring-subagents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let meta = |id: &str, v: Value| {
            std::fs::write(dir.join(format!("agent-{id}.jsonl")), "{}\n").unwrap();
            std::fs::write(dir.join(format!("agent-{id}.meta.json")), v.to_string()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
        };
        meta(
            "a1",
            serde_json::json!({"agentType": "general-purpose", "description": "Audit drift",
            "requestShape": "background", "finished": true}),
        );
        meta(
            "a2",
            serde_json::json!({"agentType": "Explore", "description": " "}),
        );
        meta(
            "a4",
            serde_json::json!({"description": "Stopped", "requestShape": "background",
            "stoppedByUser": true}),
        );
        std::fs::write(dir.join("agent-a3.jsonl"), "{}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let subs = subagents(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            subs.len(),
            3,
            "a log without its meta is not listed: {subs:?}"
        );
        assert_eq!(subs[0].name, "Audit drift");
        assert_eq!(subs[0].id, "a1");
        assert!(!subs[0].running() && !subs[0].foreground && !subs[0].stopped);
        // No description: its type names it.
        assert_eq!(subs[1].name, "Explore");
        assert!(subs[1].running() && subs[1].foreground);
        assert!(subs[2].stopped && !subs[2].running());
        assert!(subagents(Path::new("/nonexistent/subagents")).is_empty());
    }

    #[test]
    fn a_queued_or_delivered_notification_marks_subagents_stopped() {
        let note = "<task-notification>\n<task-id>a1</task-id>\n<task-id>a2</task-id>\n<status>stopped</status>\n<summary>2 background agents didn't finish</summary>";
        let mut p = crate::content::Parser::default();
        Claude.metadata(
            &mut p,
            &serde_json::json!({"type": "queue-operation", "operation": "enqueue", "content": note}),
        );
        assert_eq!(p.card.stopped_subagents, ["a1", "a2"]);
        // Delivered, the same report changes nothing; a good one takes its mark back.
        Claude.metadata(
            &mut p,
            &serde_json::json!({"type": "user", "message": {"role": "user", "content": note}}),
        );
        Claude.metadata(
            &mut p,
            &serde_json::json!({"type": "user", "message": {"role": "user",
            "content": "<task-notification><task-id>a1</task-id><status>completed</status>"}}),
        );
        assert_eq!(p.card.stopped_subagents, ["a2"]);
    }

    #[test]
    fn the_title_glyph_says_whether_a_turn_runs() {
        assert_eq!(Claude.title_busy("◐ 修复滚动卡顿"), Some(true));
        assert_eq!(Claude.title_busy("⠂ 修复滚动卡顿"), Some(true));
        assert_eq!(Claude.title_busy("✳ 修复滚动卡顿"), Some(false));
        // A title the user or a shell set says nothing about the turn.
        assert_eq!(Claude.title_busy("◐修复"), None);
        assert_eq!(Claude.title_busy("vim main.rs"), None);
        assert_eq!(Claude.title_busy(""), None);
    }
    #[test]
    fn the_ticking_line_above_the_input_box_is_a_running_turn() {
        let screen = |line: &str| {
            format!("● Bash(cargo test)\n  ⎿ ok\n\n{line}\n\n────\n❯ \n────\n  ⏵⏵ auto mode on\n")
        };
        for running in [
            "· Seasoning… (53s · ↓ 2.0k tokens)",
            "✻ Percolating… (19s · ↓ 1.3k tokens · thinking)",
            "✳ Running PreCompact hooks… (running PreCompact hook · 1s)",
        ] {
            assert_eq!(
                Claude.screen_busy(&screen(running)),
                Some(true),
                "{running}"
            );
        }
        for done in [
            "✻ Worked for 16m 1s · done 2:10 AM",
            "❯ 这个是怎么回事… (看一下)",
            "",
        ] {
            assert_eq!(Claude.screen_busy(&screen(done)), Some(false), "{done}");
        }
    }
}
