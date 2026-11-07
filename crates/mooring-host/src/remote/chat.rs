//! `chat`: a conversation as chat, and with `tools` what the Agent did before each of its
//! messages (commands run, files changed), for the phone's folded tool line.
use mooring_core::protocol::*;
use mooring_remote::protocol as rp;
use serde_json::{json, Value};
use std::io;
use std::path::Path;

#[cfg(test)]
#[path = "chat_poll_tests.rs"]
mod tests;

/// The conversation a request names, by its session id or the terminal running it.
pub(super) fn session_of(socket: &Path, request: &Value) -> io::Result<String> {
    match (request["session"].as_str(), request["terminal"].as_str()) {
        (Some(s), _) => Ok(s.to_owned()),
        (None, Some(t)) => match rpc(socket, &Request::Workspace)? {
            Reply::Workspace(w) => w
                .current_agent(t)
                .map(|s| s.id.clone())
                .ok_or_else(|| io::Error::other("这个终端里没有 Agent 对话")),
            other => Err(io::Error::other(format!("unexpected reply: {other:?}"))),
        },
        (None, None) => Err(io::Error::other(format!(
            "{} 需要 terminal 或 session",
            request["op"].as_str().unwrap_or("chat")
        ))),
    }
}

/// A conversation as chat, named by its terminal or its session id.
pub(super) fn chat(socket: &Path, request: &Value) -> io::Result<Value> {
    let session = session_of(socket, request)?;
    let reply = rpc(
        socket,
        &Request::Chat {
            session: session.clone(),
            after: request["after"].as_i64(),
            before: request["before"].as_i64(),
            limit: request["limit"].as_u64().unwrap_or(50).clamp(1, 500) as usize,
            tools: request["tools"].as_bool().unwrap_or(false),
        },
    )?;
    match reply {
        // The page without anything older is where a reader would otherwise take a log read
        // part way for the conversation's start, or for an empty one.
        Reply::Chat {
            messages,
            generation,
            more,
            partial,
        } => Ok(json!(rp::Chat {
            session,
            generation,
            messages: messages.into_iter().map(message).collect(),
            more,
            partial: partial && !more,
        })),
        Reply::Error(e) => Err(io::Error::other(e)),
        other => Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    }
}

/// What the terminal's Agent offers as the next prompt. A phone asks every couple of seconds
/// while it waits on you, so this names the terminal and never reads the whole `Workspace`.
pub(super) fn suggestion(socket: &Path, terminal: &str) -> io::Result<Value> {
    let read = Request::InputSuggestion {
        id: terminal.to_owned(),
    };
    match rpc(socket, &read)? {
        Reply::InputSuggestion { session, text } => Ok(json!(rp::Suggestion { session, text })),
        Reply::Error(e) => Err(io::Error::other(e)),
        other => Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    }
}

/// `backfill`: reads the next stretch of a conversation's log. Replies once there is about a
/// page more of its messages, or a second has passed: the client asks until `done`.
pub(super) fn backfill(socket: &Path, request: &Value) -> io::Result<Value> {
    let session = session_of(socket, request)?;
    match rpc(socket, &Request::Backfill { session })? {
        Reply::Backfilled { added, done } => Ok(json!(rp::Backfilled { added, done })),
        other => Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    }
}

fn message(m: ChatMessage) -> rp::Message {
    rp::Message {
        seq: m.seq,
        at: m.at,
        role: if m.user {
            rp::Speaker::User
        } else {
            rp::Speaker::Agent
        },
        text: m.text,
        tools: m.tools.map(tools),
    }
}

pub(super) fn tools(t: ChatTools) -> rp::Tools {
    rp::Tools {
        commands: t.commands,
        files: (t.files.into_iter())
            .map(|(path, added, removed)| rp::FileLines {
                path,
                added,
                removed,
            })
            .collect(),
        took: t.took,
        calls: t.calls,
    }
}
