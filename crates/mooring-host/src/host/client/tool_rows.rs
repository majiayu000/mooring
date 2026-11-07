//! A conversation's rows for a phone. `Request::Chat` reads them under the host's lock;
//! `ToolCalls`, `ToolCall` and `Backfill` on a connection of their own, so a log's many
//! records never wait on the host's lock nor hold up the terminals behind it.
use super::*;

/// One log read back at a time, whoever asks: a second phone waits, it does not double the
/// disk and the writes.
static READING_BACK: Mutex<()> = Mutex::new(());

/// The calling thread gives way to everything else. On a thread of its own, so nothing else
/// that runs on a client's thread is slowed by it afterwards.
fn give_way() {
    #[cfg(target_os = "macos")]
    // SAFETY: only changes this thread's scheduling class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
    #[cfg(not(target_os = "macos"))]
    // SAFETY: on Linux this names the calling thread; only lowers its priority.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

/// A conversation's raw log, and a connection to read its rows on once the lock is let go.
fn tool_log(shared: &Shared, session: &str) -> io::Result<(ToolLog, crate::log_index::Index)> {
    let host = lock(shared)?;
    let s = (host.workspace.agent_sessions.iter())
        .find(|s| s.id == session)
        .ok_or_else(|| io::Error::other("会话不存在"))?;
    let log = ToolLog {
        path: s.transcript.clone(),
        runtime: s.runtime,
    };
    Ok((log, host.index.fork_current()?))
}

pub(super) fn dispatch(shared: &Shared, request: Request) -> io::Result<Reply> {
    match request {
        Request::Chat {
            session,
            after,
            before,
            limit,
            tools,
        } => {
            let host = lock(shared)?;
            let s = host
                .workspace
                .agent_sessions
                .iter()
                .find(|s| s.id == session)
                .ok_or_else(|| io::Error::other("会话不存在"))?;
            let floor = match host.workspace.content.get(&session) {
                Some(c) if c.partial_history => {
                    host.index.read_from(&s.transcript, c.from_offset)?
                }
                _ => 0,
            };
            let (messages, more) =
                (host.index).chat(&s.transcript, floor, after, before, limit, tools)?;
            // A card that says part of its log was skipped but not from where cannot read it
            // back: it goes on saying so.
            let skipped =
                |c: &mooring_core::content::Content| c.partial_history && c.from_offset == 0;
            let partial = floor > 0 || host.workspace.content.get(&session).is_some_and(skipped);
            Ok(Reply::Chat {
                messages,
                generation: host.index.chat_generation(&s.transcript)?,
                more,
                partial,
            })
        }
        Request::ToolCalls {
            session,
            seq,
            after,
            limit,
        } => {
            let (log, index) = tool_log(shared, &session)?;
            let (tools, rows, next) = index.tool_calls(&log.path, seq, after, limit)?;
            Ok(Reply::ToolCalls {
                log,
                tools,
                rows,
                next,
            })
        }
        Request::ToolCall { session, call } => {
            let (log, index) = tool_log(shared, &session)?;
            let row = index
                .tool_call(&log.path, &call)?
                .ok_or_else(|| io::Error::other("这条工具调用不在记录里"))?;
            Ok(Reply::ToolCall { log, row })
        }
        Request::Backfill { session } => {
            let (log, index) = tool_log(shared, &session)?;
            let _one = READING_BACK.lock().unwrap_or_else(|e| e.into_inner());
            let read = std::thread::spawn(move || {
                give_way();
                index.backfill(log.runtime, &log.path)
            })
            .join()
            .map_err(|_| io::Error::other("读更早的消息时出错"))??;
            Ok(Reply::Backfilled {
                added: read.added,
                done: read.done,
            })
        }
        other => Err(io::Error::other(format!("not a tool request: {other:?}"))),
    }
}
