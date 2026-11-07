//! Kimi Code 2.x wire logs. Agent ids are local to a session; `state.json`
//! names their parents. A forked session is not a subagent.
use super::Adapter;
use crate::content::{text, Parser};
use mooring_core::content::{short, Source, Turn};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Kimi;

fn location(path: &Path) -> Option<(&Path, &str)> {
    let agent = path.parent()?;
    let agents = agent.parent()?;
    let session = agents.parent()?;
    (path.file_name()? == "wire.jsonl"
        && agents.file_name()? == "agents"
        && session.file_name()?.to_str()?.starts_with("session_"))
    .then_some((session, agent.file_name()?.to_str()?))
}
fn id(session: &Path, agent: &str) -> String {
    let session = session.file_name().unwrap().to_string_lossy();
    if agent == "main" {
        session.into_owned()
    } else {
        format!("{session}/{agent}")
    }
}
fn state(session: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(session.join("state.json")).ok()?).ok()
}
impl Adapter for Kimi {
    fn root(&self, home: &Path) -> PathBuf {
        std::env::var_os("KIMI_CODE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".kimi-code"))
            .join("sessions")
    }
    fn runs(&self, executable: &str, _script: Option<&str>) -> bool {
        executable == "kimi"
    }
    fn process_session(&self, _pid: u32, args: &[String]) -> (Option<String>, bool) {
        let session = args
            .windows(2)
            .find(|w| matches!(w[0].as_str(), "-S" | "--session"))
            .map(|w| w[1].as_str())
            .or_else(|| args.iter().find_map(|a| a.strip_prefix("--session=")));
        (
            session
                .filter(|s| s.starts_with("session_"))
                .map(str::to_owned),
            false,
        )
    }
    fn is_transcript(&self, path: &Path) -> bool {
        location(path).is_some()
    }
    fn descend(&self, dir: &Path) -> bool {
        let parent = dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str());
        matches!(parent, Some("sessions" | "agents"))
            || dir
                .file_name()
                .is_some_and(|n| n == "agents" || n.to_string_lossy().starts_with("session_"))
    }
    fn links_children(&self) -> bool {
        true
    }
    fn identity(&self, path: &Path, record: &Value) -> Option<(String, String)> {
        let (session, agent) = location(path)?;
        let cwd = record["cwd"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| state(session)?["workDir"].as_str().map(str::to_owned))?;
        Some((id(session, agent), cwd))
    }
    fn observe_file(&self, parser: &mut Parser, path: &Path) {
        let Some((session, agent)) = location(path) else {
            return;
        };
        if agent == "main" {
            parser.card.interactive = true;
            return;
        }
        parser.hide("Kimi 子代理");
        if let Some(meta) = state(session) {
            if let Some(parent) = meta["agents"][agent]["parentAgentId"].as_str() {
                parser.card.parent = id(session, parent);
            }
        }
    }
    fn name(&self, _root: &Path, path: &Path) -> Option<String> {
        let (session, agent) = location(path)?;
        (agent == "main").then(|| state(session)?["title"].as_str().map(str::to_owned))?
    }
    fn metadata(&self, _parser: &mut Parser, _record: &Value) {}
    fn is_user(&self, record: &Value) -> bool {
        record["type"] == "turn.prompt"
    }
    fn add(&self, p: &mut Parser, r: &Value, _at: u64, source: &Source) {
        let at = r["time"].as_u64().unwrap_or(0) / 1000;
        match r["type"].as_str().unwrap_or("") {
            "turn.prompt" => {
                p.reply.clear();
                p.phase(Turn::Running, "", at, source);
                p.message(true, text(&r["input"]), at, source, false);
                if !p.card.parent.is_empty() && p.card.agent_name.is_empty() {
                    p.card.agent_name = short(&text(&r["input"]), 100);
                }
            }
            "turn.cancel" | "prompt.aborted" => p.phase(Turn::Interrupted, "", at, source),
            "interaction.request" => {
                p.child_wait_from = None;
                p.card.waiting = r["request"]["action"]
                    .as_str()
                    .unwrap_or("需要在 Kimi 终端回答")
                    .into();
                p.phase(Turn::Waiting, "", at, source);
            }
            "interaction.resolved" => {
                p.card.waiting.clear();
                p.card.turn = Turn::Running;
                p.card.updated = at;
            }
            "turn.ended" => match r["reason"].as_str() {
                Some("completed") => p.phase(Turn::Completed, "", at, source),
                Some("cancelled") => p.phase(Turn::Interrupted, "", at, source),
                Some("failed") => p.phase(Turn::Error, "", at, source),
                _ => (),
            },
            "context.append_loop_event" => {
                let e = &r["event"];
                match e["type"].as_str().unwrap_or("") {
                    "content.part" if e["part"]["type"] == "text" => {
                        p.reply.push_str(e["part"]["text"].as_str().unwrap_or(""));
                        p.card.updated = at;
                    }
                    "tool.call" => p.tool(
                        e["toolCallId"].as_str().unwrap_or(""),
                        e["name"].as_str().unwrap_or(""),
                        e["args"].clone(),
                        at,
                        source,
                    ),
                    "tool.result" => p.result(
                        e["toolCallId"].as_str().unwrap_or(""),
                        &e["result"],
                        None,
                        false,
                        at,
                        source,
                    ),
                    "step.end" => {
                        let final_answer = e["finishReason"] == "end_turn";
                        let reply = std::mem::take(&mut p.reply);
                        p.message(false, reply, at, source, final_answer);
                        match e["finishReason"].as_str() {
                            Some("end_turn") => p.phase(Turn::Completed, "", at, source),
                            Some("interrupted") => p.phase(Turn::Interrupted, "", at, source),
                            Some("error") => p.phase(Turn::Error, "", at, source),
                            _ => (),
                        }
                    }
                    _ => (),
                }
            }
            _ => (),
        }
    }
}
