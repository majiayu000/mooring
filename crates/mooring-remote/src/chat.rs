//! The replies about a conversation's messages: `chat` and `backfill`.
use crate::protocol::{FileLines, Speaker};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    pub session: String,
    /// Changes when indexed history is replaced, invalidating all message cursors.
    pub generation: String,
    /// Oldest first.
    pub messages: Vec<Message>,
    /// Older messages exist before the first: ask again with `before`.
    pub more: bool,
    /// Its log was first read from near the end, so what came before is not here at all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Grows with each message; the cursor for `after` and `before`.
    pub seq: i64,
    /// Seconds since the epoch.
    pub at: u64,
    pub role: Speaker,
    pub text: String,
    /// With `chat`'s `tools`, on Agent messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Tools>,
}

/// The tool calls between an Agent message and the message before it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tools {
    /// Shell commands run.
    pub commands: u32,
    /// Files changed, in the order first touched.
    pub files: Vec<FileLines>,
    /// Seconds this stretch of work took; absent when the log cannot say.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub took: u64,
    /// Every tool call, whatever kind.
    #[serde(default, skip_serializing_if = "none")]
    pub calls: u32,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}
fn none(n: &u32) -> bool {
    *n == 0
}

/// The reply to `suggestion`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    /// The conversation it belongs to; absent when the terminal runs no Agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// Absent when there is none right now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The reply to `backfill`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backfilled {
    /// Messages this call found.
    pub added: u32,
    /// The log has been read back to its start: nothing older exists.
    pub done: bool,
}
