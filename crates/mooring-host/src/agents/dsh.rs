//! DeepSeek Harness v4 events. The store reader handles generations and zstd;
//! this adapter owns only the event semantics and explicit delegation metadata.
use super::Adapter;
use crate::content::{input, text, Parser};
use mooring_core::{
    content::{Source, Turn},
    model::Agent,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Dsh;
impl Adapter for Dsh {
    fn root(&self, home: &Path) -> PathBuf {
        super::mirror::dir(home, Agent::Dsh)
    }
    fn runs(&self, executable: &str, script: Option<&str>) -> bool {
        executable == "dsh" || script.is_some_and(|s| s.ends_with("/@deepseek-ai/dsh/lib/bin.js"))
    }
    fn process_session(&self, _: u32, args: &[String]) -> (Option<String>, bool) {
        (
            args.windows(2)
                .find(|a| matches!(a[0].as_str(), "--resume" | "--session-id"))
                .map(|a| a[1].clone()),
            false,
        )
    }
    fn identity(&self, _: &Path, v: &Value) -> Option<(String, String)> {
        if v["type"] != "session" {
            return None;
        }
        Some((
            v["id"].as_str()?.into(),
            v["cwd"].as_str().unwrap_or("").into(),
        ))
    }
    fn metadata(&self, p: &mut Parser, v: &Value) {
        if v["type"] != "session" {
            return;
        }
        if v["origin"] == "subagent" && v["delegationDepth"].as_u64().is_some_and(|d| d > 0) {
            p.card.parent = v["parentSession"].as_str().unwrap_or("").into();
            p.card.agent_name = v["agentPreset"].as_str().unwrap_or("").into();
            p.hide("DSH 子代理");
        } else {
            p.card.interactive = true;
        }
    }
    fn links_children(&self) -> bool {
        true
    }
    fn is_user(&self, v: &Value) -> bool {
        v["type"] == "user/message" && v["data"]["source"]["kind"] == "user"
    }
    fn add(&self, p: &mut Parser, v: &Value, at: u64, src: &Source) {
        let d = &v["data"];
        match v["type"].as_str() {
            Some("turn/start") => p.phase(Turn::Running, &d["turn"].to_string(), at, src),
            Some("user/message") if self.is_user(v) => {
                p.message(true, text(&d["content"]), at, src, false)
            }
            Some("assistant/message") => {
                p.message(false, text(&d["message"]["content"]), at, src, false)
            }
            Some("tool/call") => p.tool(
                d["callId"].as_str().unwrap_or(""),
                d["name"].as_str().unwrap_or(""),
                input(&d["arguments"]),
                at,
                src,
            ),
            Some("tool/result") => p.result(
                d["message"]["toolCallId"].as_str().unwrap_or(""),
                &d["message"]["content"],
                if d["message"]["isError"] == true {
                    Some(1)
                } else {
                    None
                },
                false,
                at,
                src,
            ),
            Some("approval/asked") => {
                p.phase(Turn::Waiting, "", at, src);
                p.card.waiting = d["reason"]
                    .as_str()
                    .or_else(|| d["toolName"].as_str())
                    .unwrap_or("等待审批")
                    .into();
            }
            Some("approval/decided") => {
                // The turn/end event owns cancellation/failure, including denied approval.
                p.card.waiting.clear();
                p.card.turn = Turn::Running;
            }
            Some("turn/end") => {
                let turn = match d["reason"]["kind"].as_str() {
                    Some("completed") => Turn::Completed,
                    Some("aborted" | "interrupted" | "forked") => Turn::Interrupted,
                    Some("error") => Turn::Error,
                    _ => Turn::Unknown,
                };
                p.card.waiting.clear();
                p.phase(turn, "", at, src);
            }
            Some("session/title") => {
                if let Some(title) = d["title"].as_str().filter(|s| !s.trim().is_empty()) {
                    p.card.title = mooring_core::content::short(title, 70);
                }
            }
            _ => (),
        }
    }
}
