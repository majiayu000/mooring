//! One client connection: request dispatch, typed input and handing the socket to a
//! newer host.
use super::*;
use fibo_platform::local;
mod edit;
mod last_dir;
mod modes;
mod once;
mod stop;
mod suggestion;
mod tool_rows;
mod turn_changes;
mod typing;
pub(super) use typing::*;

pub(super) fn serve_client(
    mut stream: local::Stream,
    host: Shared,
    connection: u64,
) -> io::Result<()> {
    if !stream.peer_is_this_account()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "peer uid mismatch",
        ));
    }
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    match receive::<Request>(&mut stream)? {
        Request::Hello { major: MAJOR } => send(
            &mut stream,
            CONTROL,
            &Reply::Hello {
                major: MAJOR,
                instance: lock(&host)?.instance.clone(),
                build: BUILD.clone(),
            },
        )?,
        _ => {
            send(
                &mut stream,
                CONTROL,
                &Reply::Error(format!("protocol major {MAJOR} required")),
            )?;
            return Ok(());
        }
    }
    loop {
        let request = receive::<Request>(&mut stream)?;
        if matches!(request, Request::WatchRecognition) {
            return watch_recognition(stream, host, connection);
        }
        if matches!(request, Request::Subscribe) {
            return super::events::subscribe(stream, host, connection);
        }
        if matches!(request, Request::Diagnose) {
            let terminals = lock(&host)?
                .workspace
                .terminals
                .iter()
                .filter(|t| t.state == ProcessState::Running)
                .cloned()
                .collect::<Vec<_>>();
            // No workspace/terminal lock is held during process enumeration or waiting.
            if let Err(error) = crate::diagnostics::serve(&mut stream, &terminals) {
                send(&mut stream, CONTROL, &Reply::Error(error.to_string()))?;
            }
            return Ok(());
        }
        if matches!(request, Request::Handoff) {
            return hand_over(stream, &host);
        }
        if let Request::Attach {
            id,
            takeover,
            watch,
        } = request
        {
            let terminal = lock(&host)?.terminals.get(&id).cloned();
            match terminal {
                Some(terminal) => {
                    return attach(
                        stream,
                        terminal,
                        connection,
                        takeover && !watch,
                        watch,
                        host,
                        id,
                    )
                }
                None => send(
                    &mut stream,
                    CONTROL,
                    &Reply::Error("terminal is not available in this daemon instance".into()),
                )?,
            }
            continue;
        }
        let result = dispatch(&host, request);
        let reply = result.unwrap_or_else(|e| Reply::Error(e.to_string()));
        send(&mut stream, CONTROL, &reply)?;
    }
}

/// A newer host takes over: every running terminal goes to it as a snapshot plus its PTY
/// descriptor, and this host exits without signalling the programs. Global and terminal
/// locks stay held to the end, so no output is fed, no state is committed and no
/// terminal is started after the snapshots are taken.
pub(super) fn hand_over(mut stream: local::Stream, shared: &Shared) -> io::Result<()> {
    if !term::CAN_HAND_OVER {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this system cannot hand a running terminal over",
        ));
    }
    let host = lock(shared)?;
    let mut held = vec![];
    for (id, terminal) in &host.terminals {
        let live = lock(terminal)?;
        if !live.exited {
            held.push((id.clone(), live));
        }
    }
    let terminals: Vec<_> = held
        .iter()
        .map(|(id, live)| Handed {
            id: id.clone(),
            pid: live.pid,
            cols: live.cols,
            rows: live.rows,
            title: live.title.clone(),
            cell: live.cell.get(),
        })
        .collect();
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    send(&mut stream, CONTROL, &Reply::Handoff { terminals })?;
    for (_, live) in &mut held {
        for chunk in live.vt.snapshot()?.chunks(MAX_FRAME - 1) {
            write_frame(&mut stream, SNAPSHOT, chunk)?;
        }
        write_frame(&mut stream, READY, &[])?;
    }
    let masters: Vec<_> = held.iter().map(|(_, live)| &live.master).collect();
    term::send_masters(&stream, &masters)?;
    let count = masters.len();
    // Only a confirmed takeover lets this host go: closing our copies before the new host
    // holds its own would hang up every terminal. Anything else keeps serving.
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    match read_frame(&mut stream) {
        Ok((ACK, taken)) if taken == HANDOFF_TAKEN => {
            eprintln!("handed {count} terminals to a newer host; exiting");
            std::process::exit(0);
        }
        other => Err(io::Error::other(format!(
            "handoff not confirmed, still serving: {other:?}"
        ))),
    }
}
/// What a new host answers once it holds every handed-over descriptor.
pub(super) const HANDOFF_TAKEN: &[u8] = b"taken";

/// The host's relay supervisor, taken out of the lock: it calls back into the host.
fn supervisor(shared: &Shared) -> io::Result<std::sync::Arc<super::remote::Supervisor>> {
    lock(shared)?
        .remote
        .clone()
        .ok_or_else(|| io::Error::other("远程还没准备好"))
}

pub(super) fn dispatch(shared: &Shared, request: Request) -> io::Result<Reply> {
    match request {
        Request::RecognitionStatus => Ok(Reply::Recognition(recognition_status(&*lock(shared)?))),
        Request::RefreshAgents => {
            let transcripts = {
                let host = lock(shared)?;
                if let Some(error) = host.store.subscription_error()? {
                    return Err(io::Error::other(error));
                }
                host.index.cached()?
            };
            refresh_agents(shared, &transcripts)?;
            let mut host = lock(shared)?;
            let waiting: Vec<_> = host
                .workspace
                .agent_sessions
                .iter()
                .filter(|s| s.current)
                .filter(|s| {
                    host.workspace
                        .content
                        .get(&s.id)
                        .is_some_and(|c| c.turn == fibo_core::content::Turn::Waiting)
                })
                .cloned()
                .collect();
            if !waiting.is_empty() {
                host.reconcile(shared, waiting);
            }
            Ok(Reply::Workspace(host.snapshot()))
        }
        Request::Hook {
            runtime,
            pid,
            cli_session_id,
            transcript,
            ended,
            event,
            facts,
            terminal,
            cwd,
        } => {
            let (workspace, mut index, cached) = {
                let host = lock(shared)?;
                (
                    host.workspace.clone(),
                    host.index.fork_current()?,
                    host.index.cached()?,
                )
            };
            let processes = crate::recognition::processes(&workspace)?;
            // Reject unrelated callers before opening any supplied transcript. A hook whose
            // caller already exited still names its terminal through the environment.
            let named = terminal.as_deref().filter(|id| {
                workspace
                    .terminals
                    .iter()
                    .any(|t| &t.id == id && t.state == ProcessState::Running)
            });
            if crate::recognition::terminal_for_process(pid, &processes, &workspace).is_none()
                && named.is_none()
                && crate::recognition::continued_terminal(&workspace, runtime, &cli_session_id)
                    .is_none()
            {
                return Err(io::Error::other(
                    "Hook process is not a descendant of a managed terminal",
                ));
            }
            let path = if transcript.is_empty() {
                let matches: Vec<_> = cached
                    .iter()
                    .filter(|t| t.runtime == runtime && t.cli_id == cli_session_id)
                    .collect();
                if matches.len() != 1 {
                    return Err(io::Error::other(
                        "Hook transcript absent or ambiguous in index",
                    ));
                }
                std::path::PathBuf::from(&matches[0].path)
            } else {
                fibo_platform::file_watch::normalized(Path::new(&transcript))?
            };
            // Grok reads Claude's settings and runs its hooks as `--agent claude`; such a
            // hook would bind a Grok conversation as Claude and stamp Claude's events on it.
            if let Some(owner) = crate::recognition::transcript_owner(&path, &index.roots)
                .filter(|owner| *owner != runtime)
            {
                return Err(io::Error::other(format!(
                    "Hook for {} names a {} transcript",
                    runtime.name(),
                    owner.name()
                )));
            }
            // A new Claude announces itself before its first prompt creates the transcript:
            // the hook's own identity binds it now, and the index fills it in once written.
            let written = path.exists();
            let observed = if written {
                index
                    .update(runtime, &path, true)?
                    .ok_or_else(|| io::Error::other("transcript has no CLI identity"))?
            } else if !cwd.is_empty() && !transcript.is_empty() {
                crate::recognition::Transcript {
                    runtime,
                    cli_id: cli_session_id.clone(),
                    path: path.to_string_lossy().into_owned(),
                    cwd: cwd.clone(),
                    updated: fibo_core::content::now(),
                }
            } else {
                return Err(io::Error::other("Hook transcript not written yet"));
            };
            let mut host = lock(shared)?;
            let mut next = host.workspace.clone();
            crate::recognition::apply_hook(
                &mut next,
                runtime,
                pid,
                named,
                &cli_session_id,
                &observed,
                &processes,
                ended,
            )?;
            if let Some(event) = event.filter(|_| written) {
                index.hook(&observed.path, &event, &facts)?;
            }
            if let Some(s) = next
                .agent_sessions
                .iter()
                .find(|s| s.runtime == runtime && s.cli_session_id == cli_session_id)
            {
                if let Some(card) = index.card(&observed.path)? {
                    next.content.insert(s.id.clone(), card);
                }
                if written {
                    host.shell_watch.listed(&s.id, &facts);
                }
            }
            host.commit(next)?;
            let mut ancestor = pid;
            let mut hook_files = HashMap::new();
            while let Some(process) = processes.iter().find(|p| p.pid == ancestor) {
                if process.runtime == Some(runtime) {
                    hook_files.insert(ancestor, vec![observed.path.clone()]);
                    break;
                }
                if ancestor == process.parent {
                    break;
                }
                ancestor = process.parent;
            }
            // A hook run in the background has left its CLI's process tree; the terminal
            // it names then has to hold exactly one such CLI for its exit to be watched.
            if hook_files.is_empty() && !ended {
                let mut runs = processes.iter().filter(|p| {
                    p.runtime == Some(runtime)
                        && !processes
                            .iter()
                            .any(|c| c.parent == p.pid && c.runtime == p.runtime)
                        && named.is_some_and(|named| {
                            crate::recognition::terminal_for_process(p.pid, &processes, &workspace)
                                .as_deref()
                                == Some(named)
                        })
                });
                if let (Some(only), None) = (runs.next(), runs.next()) {
                    hook_files.insert(only.pid, vec![observed.path.clone()]);
                }
            }
            host.observe_processes(&processes, &hook_files)?;
            if let Some(events) = &host.log_events {
                events.file(path);
            }
            Ok(Reply::Workspace(host.snapshot()))
        }
        Request::ClaimNotification { session, key } => {
            let host = lock(shared)?;
            if !host.workspace.settings.notifications
                || !host
                    .workspace
                    .agent_sessions
                    .iter()
                    .any(|s| s.id == session)
            {
                return Ok(Reply::NotificationClaimed(false));
            }
            Ok(Reply::NotificationClaimed(
                host.store.claim_notification(&session, &key)?,
            ))
        }
        Request::ReloadPlugins => {
            fibo_core::plugin::reload();
            let home = fibo_platform::dirs::home().unwrap_or_default();
            crate::agents::mirror::sync(&home);
            let mut host = lock(shared)?;
            // A CLI that arrived or left changes which directories are read. The subscription
            // restarts from its checkpoint with the new roots, and walks the new one once.
            let roots = crate::agents::roots_after_reload(&host.index.roots, &home);
            if roots != host.index.roots {
                host.index.roots = roots.clone();
                host.new_roots = Some(roots);
                if let Some(events) = &host.log_events {
                    events.file(std::path::PathBuf::new());
                }
            }
            host.recognition.plugins_revision += 1;
            host.publish_status()?;
            Ok(Reply::Ok)
        }
        Request::ClientLeft { at } => {
            let mut host = lock(shared)?;
            let mut next = host.workspace.clone();
            next.last_left = at;
            host.commit(next)?;
            Ok(Reply::Ok)
        }
        Request::Workspace => Ok(Reply::Workspace(lock(shared)?.snapshot())),
        Request::SplitRoom {
            target,
            axis,
            share,
        } => {
            let (cols, rows) = split_room(&*lock(shared)?, &target, axis, share)?;
            Ok(Reply::Size { cols, rows })
        }
        Request::Edit { edit, request } => edit::edit(shared, edit, request),
        Request::Once { request, inner } => once::once(shared, request, *inner),
        Request::Launch {
            id,
            space,
            argv,
            cwd,
            cols,
            rows,
            scheme,
            colors,
        } => {
            let colors = colors.unwrap_or(scheme.colors());
            geometry(cols, rows)?;
            let mut host = lock(shared)?;
            // A folder that is gone here (another machine's window asked for it, and only
            // this host can look) starts in its space's folder, else home, as the window
            // does for its own host.
            // A resend is matched by what was asked, which the folder fallback never changes.
            let asked = cwd.clone();
            let cwd = super::start_dir::start_dir(&host.workspace, &space, cwd);
            if let Some(existing) = host.workspace.terminals.iter().find(|s| s.id == id) {
                let same_dir = existing.cwd == asked || existing.cwd == cwd;
                if existing.argv != argv || !same_dir || existing.space != space {
                    return Err(io::Error::other(
                        "launch ID was already used with different arguments",
                    ));
                }
                if existing.pid.is_none() {
                    return Err(io::Error::other(
                        existing.error.as_deref().unwrap_or("终端启动结果未知"),
                    ));
                }
                return Ok(Reply::Workspace(host.snapshot()));
            }
            if id.is_empty()
                || argv.is_empty()
                || argv[0].is_empty()
                || !Path::new(&cwd).is_absolute()
                || !Path::new(&cwd).is_dir()
            {
                return Err(io::Error::other(
                    "launch needs an ID, argv and an absolute existing working directory",
                ));
            }
            if !host.workspace.spaces.iter().any(|s| s.id == space) {
                return Err(io::Error::other("space not found"));
            }
            let mut vt = Vt::new(cols, rows)?;
            vt.set_colors(colors)?;
            let replies = Arc::new(Mutex::new(Vec::new()));
            let out = Arc::clone(&replies);
            vt.on_reply(move |bytes| {
                if let Ok(mut out) = out.lock() {
                    out.extend_from_slice(bytes);
                }
            })?;
            let cell = CellPixels::default();
            vt.answer_queries(cell.clone())?;
            let mut next = host.workspace.clone();
            next.terminals.push(Terminal {
                id: id.clone(),
                space,
                argv: argv.clone(),
                cwd: cwd.clone(),
                pid: None,
                state: ProcessState::Starting,
                exit_code: None,
                error: None,
            });
            host.commit(next)?;
            let (mut child, master) =
                match pty::spawn(&argv, Path::new(&cwd), cols, rows, &id, colors.colorfgbg()) {
                    Ok(result) => result,
                    Err(error) => {
                        let mut next = host.workspace.clone();
                        let session = next.terminals.last_mut().unwrap();
                        session.state = ProcessState::Failed;
                        session.error = Some(error.to_string());
                        host.commit(next)?;
                        return Err(error);
                    }
                };
            let reader = match master.reader() {
                Ok(reader) => reader,
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e);
                }
            };
            let pid = child.id();
            let mut next = host.workspace.clone();
            let session = next.terminals.last_mut().unwrap();
            session.pid = Some(pid);
            session.state = ProcessState::Running;
            if let Err(e) = host.commit(next) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            run_live(
                &mut host,
                shared,
                id,
                Live {
                    vt,
                    master,
                    cols,
                    rows,
                    pid,
                    owner: None,
                    subscribers: HashMap::new(),
                    watchers: Default::default(),
                    exited: false,
                    drained: false,
                    status: None,
                    exit_code: None,
                    title: String::new(),
                    cell,
                    input_at: None,
                    marks: Default::default(),
                    reports: Default::default(),
                },
                reader,
                replies,
                Reaper::Child(child),
            );
            Ok(Reply::Workspace(host.snapshot()))
        }
        request @ (Request::Chat { .. }
        | Request::ToolCalls { .. }
        | Request::ToolCall { .. }
        | Request::Backfill { .. }) => tool_rows::dispatch(shared, request),
        Request::TurnChanges { session } => turn_changes::read(shared, &session),
        Request::TurnFile {
            session,
            log,
            path,
            lines,
        } => turn_changes::read_file(shared, &session, log, &path, &lines),
        Request::ReadScreen { id, rows } => {
            let host = lock(shared)?;
            let terminal = host
                .terminals
                .get(&id)
                .ok_or_else(|| io::Error::other("terminal not found"))?;
            let text = lock(terminal)?.vt.tail_text(rows.clamp(1, 10_000))?;
            Ok(Reply::Text(text))
        }
        Request::InputSuggestion { id } => suggestion::read(shared, &id),
        Request::Modes(ask) => modes::run(shared, ask),
        Request::AssociateScreen { id } => super::screen_match::associate_pane(shared, &id),
        Request::SendText {
            id,
            text,
            submit,
            session,
        } => send_text(shared, &id, &text, submit, session.as_deref()).map(|()| Reply::Ok),
        Request::ReportTo { child, parent } => {
            let mut host = lock(shared)?;
            for id in [&child, &parent] {
                if !host.terminals.contains_key(id) {
                    return Err(io::Error::other(format!("终端 {id} 不存在")));
                }
            }
            if child == parent {
                return Err(io::Error::other("不能让终端向自己汇报"));
            }
            let w = host.snapshot();
            host.report_back.link(&w, &child, &parent);
            Ok(Reply::Ok)
        }
        Request::AnswerApproval {
            id,
            question,
            allow,
        } => {
            answer_approval(shared, &id, &question, allow, false)?;
            Ok(Reply::Ok)
        }
        Request::AnswerAlways { id, question } => {
            answer_approval(shared, &id, &question, true, true)?;
            Ok(Reply::Ok)
        }
        Request::AnswerTrust { id, folder, trust } => {
            answer_trust(shared, &id, &folder, trust)?;
            Ok(Reply::Ok)
        }
        Request::SendKeys { id, keys } => {
            send_keys(shared, &id, &keys)?;
            Ok(Reply::Ok)
        }
        Request::SendInput { id, bytes } => {
            type_input(shared, &id, &bytes)?;
            Ok(Reply::Ok)
        }
        Request::Interrupt { id } => {
            interrupt(shared, &id)?;
            Ok(Reply::Ok)
        }
        Request::AwaitInterrupt { id } => stop::await_interrupt(shared, &id),
        Request::RemoteConfigure {
            enabled,
            relay,
            key,
        } => {
            let relay = relay.trim().to_owned();
            let remote = supervisor(shared)?;
            let key = match key {
                None => remote.key(),
                Some(k) => Some(k.trim().to_owned()).filter(|k| !k.is_empty()),
            };
            remote.configure(super::remote::Config {
                enabled,
                relay,
                key,
            })?;
            Ok(Reply::Ok)
        }
        Request::RemotePair { name, role } => {
            let remote = supervisor(shared)?;
            let relay = remote.relay();
            if relay.is_empty() {
                return Err(io::Error::other("先填中继地址"));
            }
            let role = crate::devices::parse_role(&role)?;
            let name = Some(name.trim())
                .filter(|n| !n.is_empty())
                .unwrap_or("手机");
            let (_, link) = crate::devices::pair_link(&remote.socket(), &relay, name, role)?;
            remote.refresh();
            Ok(Reply::Text(link))
        }
        Request::RemoteDevice { device, change } => {
            let remote = supervisor(shared)?;
            let socket = remote.socket();
            match change {
                RemoteChange::Role(role) => {
                    crate::devices::set_role(&socket, &device, crate::devices::parse_role(&role)?)?;
                }
                RemoteChange::Kick => {
                    crate::devices::kick(&socket, &device)?;
                }
                RemoteChange::Revoke => {
                    crate::devices::remove(&socket, &device)?;
                }
                RemoteChange::Refresh => (),
            }
            remote.refresh();
            Ok(Reply::Ok)
        }
        Request::RemoteReport(live) => {
            supervisor(shared)?.report(live);
            Ok(Reply::Ok)
        }
        Request::TerminalCwd { id } => {
            let host = lock(shared)?;
            let terminal = host
                .terminals
                .get(&id)
                .ok_or_else(|| io::Error::other("terminal not found"))?;
            let live = lock(terminal)?;
            if live.exited {
                return Err(io::Error::other("terminal has exited"));
            }
            // What the shell reports (OSC 7) is the truth for it; the process directory
            // is the fallback for a shell without the integration.
            if let Some(path) = live.vt.pwd(&hostname()).filter(|p| Path::new(p).is_dir()) {
                return Ok(Reply::Cwd(path));
            }
            let pid = host
                .workspace
                .terminals
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| t.pid)
                .ok_or_else(|| io::Error::other("terminal PID missing"))?;
            // Explicit split action only, querying this managed shell, never polling.
            let path = fibo_platform::inspect::cwd(pid).ok_or_else(io::Error::last_os_error)?;
            Ok(Reply::Cwd(path))
        }
        Request::StopAgent { id } => {
            let host = lock(shared)?;
            let session = host
                .workspace
                .current_agent(&id)
                .ok_or_else(|| io::Error::other("没有已识别的活动 Agent"))?;
            let pid = host
                .agent_pids
                .iter()
                .find(|(_, sid)| *sid == &session.id)
                .map(|(pid, _)| *pid)
                .ok_or_else(|| io::Error::other("Agent 进程身份未确认，未发送停止信号"))?;
            let processes = crate::recognition::processes(&host.workspace)?;
            if !processes
                .iter()
                .any(|p| p.pid == pid && p.runtime == Some(session.runtime))
                || crate::recognition::terminal_for_process(pid, &processes, &host.workspace)
                    .as_deref()
                    != Some(id.as_str())
            {
                return Err(io::Error::other("Agent 进程身份已变化，未发送停止信号"));
            }
            // A freshly verified Agent PID only; never kill the PTY shell or group.
            fibo_platform::process::terminate(pid as i32)?;
            Ok(Reply::Ok)
        }
        Request::Stop { id } => {
            let terminal = lock(shared)?
                .terminals
                .get(&id)
                .cloned()
                .ok_or_else(|| io::Error::other("terminal not found"))?;
            stop_terminal(&terminal)?;
            Ok(Reply::Ok)
        }
        _ => Err(io::Error::other("operation requires an attached terminal")),
    }
}

/// Enter for 允许一次, Esc for 拒绝, sent only while the screen still shows `question`
/// as an approval, read again under the same lock the key is written under: an approval
/// answered in the pane meanwhile gets nothing typed into what replaced it.
pub(super) fn answer_approval(
    shared: &Shared,
    id: &str,
    question: &str,
    allow: bool,
    always: bool,
) -> io::Result<()> {
    if let Some(done) = http_ask::answer(shared, id, question, allow, always) {
        return done;
    }
    let (terminal, rules) = {
        let host = lock(shared)?;
        let terminal = host
            .terminals
            .get(id)
            .cloned()
            .ok_or_else(|| io::Error::other("terminal not found"))?;
        let rules = host
            .workspace
            .current_agent(id)
            .map(|s| s.runtime)
            .map_or_else(
                crate::screen_rules::default_rules,
                crate::screen_rules::rules,
            );
        (terminal, rules)
    };
    {
        let mut live = lock(&terminal)?;
        if live.exited {
            return Err(io::Error::other("terminal has exited"));
        }
        let tail = live.vt.tail_text(usize::from(live.rows))?;
        let seen = crate::screen_rules::explain(rules, &tail);
        if seen.approval() != Some(question) {
            return Err(io::Error::other("画面上已经没有这个审批了，可能已经答过"));
        }
        // Enter takes whichever option the cursor is on; only the first one is 允许一次.
        let key = if always {
            let key = seen.always_key();
            key.ok_or_else(|| io::Error::other("这个审批没有「不再询问」这一项"))?
        } else if allow && !seen.cursor_on_first() {
            return Err(io::Error::other("光标不在第一个选项上，请打开画面回答"));
        } else {
            (if allow { "\r" } else { "\x1b" }).to_owned()
        };
        live.master.write(key.as_bytes())?;
        live.input_at = Some(Instant::now());
    }
    lock(shared)?.set_screen_wait(id, None)
}

/// Room for both halves of a split of `target`, at the size it is shown now, and the
/// new pane's size, so it starts close to how it will show. A terminal this host does
/// not run has no size to judge; the edit refuses it.
fn split_room(
    host: &Host,
    target: &str,
    axis: fibo_core::layout::Axis,
    share: u16,
) -> io::Result<(u16, u16)> {
    let Some(terminal) = host.terminals.get(target) else {
        return Err(io::Error::other("终端不存在"));
    };
    let (cols, rows) = {
        let live = lock(terminal)?;
        (live.cols, live.rows)
    };
    fibo_core::layout::Layout::split_room(cols, rows, axis, share).map_err(io::Error::other)
}
