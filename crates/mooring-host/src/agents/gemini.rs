//! Gemini's append-only chat records (JSONL). Child files live under the full
//! root session id, even when one subagent delegates to another.
use super::Adapter;
use crate::content::{seconds, text, Parser};
use mooring_core::content::{short, Source, Turn};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Gemini;

fn chats(path: &Path) -> Option<&Path> {
    let dir = path.parent()?;
    if dir.file_name()? == "chats" {
        Some(dir)
    } else {
        let parent = dir.parent()?;
        (parent.file_name()? == "chats"
            && uuid::Uuid::parse_str(dir.file_name()?.to_str()?).is_ok())
        .then_some(parent)
    }
}
impl Adapter for Gemini {
    fn root(&self, home: &Path) -> PathBuf {
        std::env::var_os("GEMINI_CLI_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.to_owned())
            .join(".gemini/tmp")
    }
    fn runs(&self, executable: &str, script: Option<&str>) -> bool {
        executable == "gemini"
            || script
                .is_some_and(|s| s.contains("/@google/gemini-cli/") && s.ends_with("/index.js"))
    }
    fn process_session(&self, _pid: u32, args: &[String]) -> (Option<String>, bool) {
        let value = args
            .windows(2)
            .find(|w| matches!(w[0].as_str(), "-r" | "--resume"))
            .map(|w| w[1].as_str())
            .or_else(|| args.iter().find_map(|a| a.strip_prefix("--resume=")));
        (
            value
                .and_then(|s| uuid::Uuid::parse_str(s).ok())
                .map(|u| u.to_string()),
            false,
        )
    }
    fn is_transcript(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "jsonl")
            && chats(path).is_some()
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with("session-") || uuid::Uuid::parse_str(s).is_ok())
    }
    fn descend(&self, dir: &Path) -> bool {
        dir.file_name().is_some_and(|n| n == "chats")
            || dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "tmp")
            || (dir
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "chats")
                && dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| uuid::Uuid::parse_str(n).is_ok()))
    }
    fn links_children(&self) -> bool {
        true
    }
    fn identity(&self, path: &Path, record: &Value) -> Option<(String, String)> {
        let session = record["sessionId"].as_str()?;
        let cwd = std::fs::read_to_string(chats(path)?.parent()?.join(".project_root")).ok()?;
        Some((session.into(), cwd.trim().into()))
    }
    fn observe_file(&self, p: &mut Parser, path: &Path) {
        let Some(chats) = chats(path) else { return };
        if path.parent() != Some(chats) {
            p.card.parent = path
                .parent()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            p.hide("Gemini 子代理");
        }
    }
    fn metadata(&self, p: &mut Parser, r: &Value) {
        if r["kind"] == "subagent" {
            p.hide("Gemini 子代理");
        }
        if r["kind"] == "main" && p.card.hidden_reason.is_empty() {
            p.card.interactive = true;
        }
    }
    fn is_user(&self, r: &Value) -> bool {
        r["type"] == "user"
    }
    fn add(&self, p: &mut Parser, r: &Value, at: u64, source: &Source) {
        // History patches/rewinds are not lifecycle events. Invalidate the last answer
        // until a new user/assistant record settles it; never retain a false completion.
        if r.get("$rewindTo").is_some() || r.get("$patch").is_some() {
            p.card.last_reply.clear();
            p.card.waiting.clear();
            p.phase(Turn::Unknown, "", at, source);
            return;
        }
        match r["type"].as_str().unwrap_or("") {
            "user" => {
                p.phase(Turn::Running, r["id"].as_str().unwrap_or(""), at, source);
                p.message(true, text(&r["content"]), at, source, false);
                if p.card.hidden_reason.is_empty() {
                    p.card.interactive = true;
                }
                if !p.card.parent.is_empty() && p.card.agent_name.is_empty() {
                    p.card.agent_name = short(&text(&r["content"]), 100);
                }
            }
            "gemini" => {
                if let Some(calls) = r["toolCalls"].as_array().filter(|c| !c.is_empty()) {
                    // Upstream appends the entire same message again on every tool-state
                    // change. Read its current aggregate, not duplicate tool receipts.
                    let at = calls
                        .iter()
                        .map(|c| seconds(&c["timestamp"]))
                        .max()
                        .unwrap_or(0)
                        .max(at);
                    p.card.waiting.clear();
                    p.card.choices.clear();
                    let turn = if let Some(call) =
                        calls.iter().find(|c| c["status"] == "awaiting_approval")
                    {
                        p.child_wait_from = None;
                        p.card.waiting = call["description"]
                            .as_str()
                            .unwrap_or("需要在 Gemini 终端审批")
                            .into();
                        Turn::Waiting
                    } else if calls
                        .iter()
                        .any(|c| c["name"] == "complete_task" && c["status"] == "success")
                    {
                        Turn::Completed
                    } else {
                        // Even successful tools are followed by more model work.
                        Turn::Running
                    };
                    p.card.turn = turn;
                    p.card.updated = p.card.updated.max(at);
                } else {
                    let reply = text(&r["content"]);
                    if !reply.is_empty() {
                        p.message(false, reply, at, source, true);
                        // Text is also recorded before tool dispatch; it does not prove
                        // that the agent loop ended. complete_task above does.
                        if p.card.parent.is_empty() {
                            p.phase(Turn::Unknown, "", at, source);
                        }
                    }
                }
            }
            "error" => {
                p.card.error = short(&text(&r["content"]), 600);
                p.phase(Turn::Error, "", at, source);
            }
            _ => (),
        }
    }
}
