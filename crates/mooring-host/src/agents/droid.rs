//! Factory's session_start envelope and Anthropic-shaped messages.
use super::{claude::Claude, Adapter};
use crate::content::Parser;
use mooring_core::content::{Source, Turn};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Droid;
impl Adapter for Droid {
    fn root(&self, home: &Path) -> PathBuf {
        std::env::var_os("FACTORY_HOME_OVERRIDE")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.to_owned())
            .join(".factory/sessions")
    }
    fn runs(&self, executable: &str, script: Option<&str>) -> bool {
        executable == "droid"
            || script.is_some_and(|s| Path::new(s).file_name().is_some_and(|n| n == "droid"))
    }
    fn process_session(&self, _: u32, args: &[String]) -> (Option<String>, bool) {
        (
            super::session_arg(args, &["--resume", "--resume="], &["-m", "--model"])
                .map(str::to_owned),
            false,
        )
    }
    fn identity(&self, _: &Path, v: &Value) -> Option<(String, String)> {
        if v["type"] != "session_start" {
            return None;
        }
        Some((
            v["id"].as_str()?.into(),
            v["lastCwd"].as_str().or_else(|| v["cwd"].as_str())?.into(),
        ))
    }
    fn metadata(&self, p: &mut Parser, v: &Value) {
        if v["type"] != "session_start" {
            return;
        }
        if let Some(parent) = v["callingSessionId"]
            .as_str()
            .filter(|s| !s.is_empty() && Some(*s) != v["id"].as_str())
        {
            // Also covers /btw side conversations: both are explicitly called sessions.
            // `parent` is only fork lineage, and never enters the child index.
            p.card.parent = parent.into();
            p.hide("Droid 子会话");
            p.card.agent_name = v["title"].as_str().unwrap_or("").into();
        } else {
            p.card.interactive = true;
        }
    }
    fn links_children(&self) -> bool {
        true
    }
    fn is_user(&self, v: &Value) -> bool {
        v["type"] == "message" && v["message"]["role"] == "user"
    }
    fn add(&self, p: &mut Parser, v: &Value, at: u64, src: &Source) {
        if v["type"] != "message" {
            return;
        }
        let mut normalized = v.clone();
        normalized["type"] = v["message"]["role"].clone();
        normalized["uuid"] = v["id"].clone();
        Claude.add(p, &normalized, at, src);
        if v["message"]["role"] == "assistant"
            && v["message"]["stop_reason"].is_null()
            && !v["message"]["content"]
                .as_array()
                .is_some_and(|a| a.iter().any(|b| b["type"] == "tool_use"))
            && p.card.parent.is_empty()
            && p.card.turn != Turn::Waiting
        {
            // Persisted text alone cannot prove that the turn ended.
            p.phase(Turn::Unknown, "", at, src);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooring_core::model::Agent;
    use serde_json::json;
    #[test]
    fn caller_is_a_child_but_fork_is_not_and_text_is_not_completion() {
        for (field, child) in [("callingSessionId", true), ("parent", false)] {
            let mut p = Parser::default();
            let mut header =
                json!({"type":"session_start","id":"child","cwd":"/repo","title":"Explore"});
            header[field] = "main".into();
            p.add(
                Agent::Droid,
                &serde_json::to_vec(&header).unwrap(),
                Source::default(),
            );
            for (i,v) in [json!({"type":"message","id":"u","message":{"role":"user","content":"Explore the repo"}}),json!({"type":"message","id":"a","message":{"role":"assistant","content":[{"type":"text","text":"Reading files"}]}})].into_iter().enumerate() {
                p.add(Agent::Droid,&serde_json::to_vec(&v).unwrap(),Source{offset:i as u64+1,..Default::default()});
            }
            assert_eq!(!p.card.parent.is_empty(), child);
            assert_eq!(p.card.last_reply, "Reading files");
            assert_eq!(
                p.card.turn,
                if child { Turn::Running } else { Turn::Unknown }
            );
        }
    }
}
