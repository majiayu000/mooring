//! Pi and oh-my-pi share message records. Only omp's sibling artifact logs prove
//! child ownership; parentSession and entry.parentId are lineage, not delegation.
use super::Adapter;
use crate::content::{text, Parser};
use mooring_core::content::{Source, Turn};
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

pub struct Pi(pub bool);
fn header(path: &Path) -> Option<Value> {
    BufReader::new(File::open(path).ok()?.take(65536))
        .lines()
        .take(4)
        .filter_map(Result::ok)
        .filter_map(|s| serde_json::from_str::<Value>(&s).ok())
        .find(|v| v["type"] == "session")
}
impl Adapter for Pi {
    fn root(&self, home: &Path) -> PathBuf {
        if self.0 {
            if let Some(root) = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .map(|p| p.join("omp"))
                .filter(|p| p.is_dir())
            {
                return root.join("sessions");
            }
            home.join(".omp/agent/sessions")
        } else {
            super::home_dir(home, "PI_CODING_AGENT_DIR", ".pi/agent").join("sessions")
        }
    }
    fn runs(&self, executable: &str, script: Option<&str>) -> bool {
        executable == if self.0 { "omp" } else { "pi" }
            || script.is_some_and(|s| {
                if self.0 {
                    s.ends_with("/@oh-my-pi/pi-coding-agent/src/cli.ts")
                } else {
                    s.ends_with("/@earendil-works/pi-coding-agent/dist/bundle/cli.js")
                }
            })
    }
    fn process_session(&self, _: u32, args: &[String]) -> (Option<String>, bool) {
        // --resume also accepts a partial id/picker search, so only bind an actual UUID.
        let id = args
            .windows(2)
            .find(|a| a[0] == if self.0 { "--resume" } else { "--session" })
            .map(|a| a[1].clone())
            .filter(|s| uuid::Uuid::parse_str(s).is_ok());
        (id, false)
    }
    fn is_transcript(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "jsonl")
            && path.file_name().is_some_and(|n| n != "btw-history.jsonl")
    }
    fn identity(&self, _: &Path, v: &Value) -> Option<(String, String)> {
        if v["type"] != "session" {
            return None;
        }
        Some((v["id"].as_str()?.into(), v["cwd"].as_str()?.into()))
    }
    fn observe_file(&self, p: &mut Parser, path: &Path) {
        if !self.0 {
            return;
        }
        let Some(dir) = path.parent() else {
            return;
        };
        let Some(name) = dir.file_name() else {
            return;
        };
        // <session stem>/<task>.jsonl belongs to <session stem>.jsonl. The same
        // layout holds for a nested task; read its header id, never the task filename.
        let owner = dir.with_file_name(format!("{}.jsonl", name.to_string_lossy()));
        if let Some(h) = header(&owner) {
            if let Some(parent) = h["id"].as_str().filter(|s| !s.is_empty()) {
                p.card.parent = parent.into();
                p.card.interactive = false;
                p.hide("omp 子代理");
            }
        }
    }
    fn metadata(&self, p: &mut Parser, v: &Value) {
        if v["type"] == "session" && p.card.parent.is_empty() {
            p.card.interactive = true;
        }
    }
    fn links_children(&self) -> bool {
        self.0
    }
    fn is_user(&self, v: &Value) -> bool {
        v["type"] == "message" && v["message"]["role"] == "user"
    }
    fn add(&self, p: &mut Parser, v: &Value, at: u64, src: &Source) {
        if matches!(
            v["type"].as_str(),
            Some("session_info" | "title_change" | "title")
        ) {
            if let Some(title) = v["name"]
                .as_str()
                .or_else(|| v["title"].as_str())
                .filter(|s| !s.trim().is_empty())
            {
                p.card.title = mooring_core::content::short(title, 70);
            }
        }
        if v["type"] != "message" {
            return;
        }
        let m = &v["message"];
        match m["role"].as_str() {
            Some("user") => {
                p.phase(Turn::Running, v["id"].as_str().unwrap_or(""), at, src);
                p.message(true, text(&m["content"]), at, src, false);
            }
            Some("assistant") => {
                p.message(
                    false,
                    text(&m["content"]),
                    at,
                    src,
                    m["stopReason"] == "stop",
                );
                if let Some(blocks) = m["content"].as_array() {
                    for b in blocks.iter().filter(|b| b["type"] == "toolCall") {
                        let name = b["name"].as_str().unwrap_or("");
                        p.tool(
                            b["id"].as_str().unwrap_or(""),
                            if self.0 && name == "ask" {
                                "AskUserQuestion"
                            } else {
                                name
                            },
                            b["arguments"].clone(),
                            at,
                            src,
                        );
                    }
                }
                let turn = match m["stopReason"].as_str() {
                    Some("stop") => Some(Turn::Completed),
                    Some("aborted") => Some(Turn::Interrupted),
                    Some("error") => Some(Turn::Error),
                    Some("toolUse") => None,
                    _ if p.card.parent.is_empty() => Some(Turn::Unknown),
                    _ => None,
                };
                if let Some(turn) = turn {
                    p.phase(turn, "", at, src);
                }
            }
            Some("toolResult") => p.result(
                m["toolCallId"].as_str().unwrap_or(""),
                &m["content"],
                if m["isError"] == true { Some(1) } else { None },
                false,
                at,
                src,
            ),
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use mooring_core::model::Agent;
    #[test]
    fn published_cli_entrypoints_and_resume_flags_are_recognized() {
        for (command, agent) in [
            ("python3 /opt/hermes/venv/bin/hermes", Agent::Hermes),
            ("droid --resume id", Agent::Droid),
            (
                "node /opt/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js",
                Agent::Pi,
            ),
            (
                "bun /opt/node_modules/@oh-my-pi/pi-coding-agent/src/cli.ts",
                Agent::Omp,
            ),
            (
                "node /opt/node_modules/@deepseek-ai/dsh/lib/bin.js tui",
                Agent::Dsh,
            ),
        ] {
            assert_eq!(super::super::from_command(command), Some(agent));
        }
        let id = "11111111-1111-4111-8111-111111111111";
        let adapter = super::super::adapter(Agent::Pi);
        assert_eq!(
            adapter
                .process_session(1, &["pi".into(), "--session".into(), id.into()])
                .0
                .as_deref(),
            Some(id)
        );
        assert_eq!(
            adapter
                .process_session(1, &["pi".into(), "--resume".into(), id.into()])
                .0,
            None,
            "--resume opens a picker; the following word is not a session id"
        );
    }
}
