//! Hermes' SQLite snapshot is normalized by hermes_store, then read by the ordinary index.
use super::Adapter;
use crate::content::{input, text, Parser};
use mooring_core::{
    content::{Source, Turn},
    model::Agent,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Hermes;
impl Adapter for Hermes {
    fn root(&self, home: &Path) -> PathBuf {
        super::mirror::dir(home, Agent::Hermes)
    }
    fn runs(&self, executable: &str, script: Option<&str>) -> bool {
        executable == "hermes"
            || script.is_some_and(|s| Path::new(s).file_name().is_some_and(|n| n == "hermes"))
    }
    fn process_session(&self, _: u32, args: &[String]) -> (Option<String>, bool) {
        (
            args.windows(2)
                .find(|a| matches!(a[0].as_str(), "--resume" | "-r"))
                .map(|a| a[1].clone()),
            false,
        )
    }
    fn identity(&self, _: &Path, v: &Value) -> Option<(String, String)> {
        Some((v["session_id"].as_str()?.into(), v["cwd"].as_str()?.into()))
    }
    fn metadata(&self, p: &mut Parser, v: &Value) {
        if v["type"] != "session" {
            return;
        }
        // parent_session_id alone also denotes forks and compression. Only the explicit
        // persisted source='subagent' proves delegation; no prompt or timing heuristic.
        if v["source"] == "subagent" {
            p.card.parent = v["parent_session_id"].as_str().unwrap_or("").into();
            p.hide("Hermes 子代理");
        } else {
            p.card.interactive = true;
        }
    }
    fn links_children(&self) -> bool {
        true
    }
    fn is_user(&self, v: &Value) -> bool {
        v["role"] == "user"
    }
    fn add(&self, p: &mut Parser, v: &Value, at: u64, src: &Source) {
        match v["role"].as_str() {
            Some("user") => {
                p.phase(Turn::Running, v["id"].as_str().unwrap_or(""), at, src);
                p.message(true, text(&v["content"]), at, src, false);
            }
            Some("assistant") => {
                let calls = v["tool_calls"].as_array();
                p.message(false, text(&v["content"]), at, src, false);
                if let Some(calls) = calls {
                    for c in calls {
                        p.tool(
                            c["id"].as_str().unwrap_or(""),
                            match c["function"]["name"].as_str().unwrap_or("") {
                                "clarify" => "AskUserQuestion",
                                name => name,
                            },
                            input(&c["function"]["arguments"]),
                            at,
                            src,
                        );
                    }
                }
                // A stored assistant message may be an intermediate response. Hermes
                // persists finish_reason; never use the mere presence of text as stop.
                if calls.is_none_or(Vec::is_empty) {
                    match v["finish_reason"].as_str() {
                        Some("stop") => p.phase(Turn::Completed, "", at, src),
                        Some("error") => p.phase(Turn::Error, "", at, src),
                        Some("cancelled" | "interrupted") => {
                            p.phase(Turn::Interrupted, "", at, src)
                        }
                        _ if p.card.parent.is_empty() => p.phase(Turn::Unknown, "", at, src),
                        _ => (),
                    }
                }
            }
            Some("tool") => p.result(
                v["tool_call_id"].as_str().unwrap_or(""),
                &v["content"],
                None,
                false,
                at,
                src,
            ),
            _ => {}
        }
        if let Some(title) = v["title"].as_str().filter(|s| !s.trim().is_empty()) {
            p.card.title = mooring_core::content::short(title, 70);
        }
    }
}
