use crate::{pty, store::Store};
use fibo_core::{
    model::*,
    protocol::*,
    terminal::{CellPixels, Vt},
};
use fibo_platform::terminal as term;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{sync_channel, SyncSender},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

mod association;
mod client;
mod events;
mod found_shells;
mod http_ask;
mod live;
mod lock;
pub(crate) mod remote;
mod report_back;
mod runtime;
mod screen_match;
mod shell_watch;
mod start_dir;
mod status;
mod wait_log;
use association::*;
use client::*;
use live::*;
use lock::lock;
use runtime::serve_runtime;
use status::*;
type Shared = Arc<Mutex<Host>>;
type LiveTerminal = Arc<Mutex<Live>>;
/// Captured at startup: rebuilding the binary later must not hide that this host is old.
static BUILD: std::sync::LazyLock<fibo_core::protocol::Build> =
    std::sync::LazyLock::new(fibo_core::protocol::Build::current);
struct Host {
    /// Keeps phones on a relay; None until the host has started it.
    remote: Option<Arc<remote::Supervisor>>,
    store: Store,
    workspace: Workspace,
    terminals: HashMap<String, LiveTerminal>,
    /// Terminals whose tab or pane was closed, let go of once they have ended.
    closed: std::collections::HashSet<String>,
    instance: String,
    index: crate::log_index::Index,
    recognition: RecognitionStatus,
    log_events: Option<fibo_platform::file_watch::Notifier>,
    /// Transcript roots after plugins changed, for the log subscription to restart with.
    new_roots: Option<Vec<(Agent, std::path::PathBuf)>>,
    recognition_subscribers: HashMap<u64, Subscriber>,
    /// `mooring events` connections, and what they were last told.
    event_subscribers: HashMap<u64, events::EventSubscriber>,
    events: events::Tracker,
    exits: Option<fibo_platform::exit_watch::ExitWatch>,
    agent_pids: HashMap<u32, String>,
    /// Agent processes no evidence has tied to a conversation yet: their terminal and CLI.
    unbound_pids: HashMap<u32, (String, Agent)>,
    /// Logs of conversations whose pane just ended. Another pane may be showing one
    /// (`codex resume` there), however long ago its last prompt: compare screens once.
    released_logs: std::collections::HashSet<String>,
    /// Agent processes that save no transcript, so their hint goes when they exit.
    unrecorded_pids: HashMap<u32, String>,
    /// Recent Enter presses per terminal, seconds since the epoch: evidence for which
    /// pane a shared-daemon Codex prompt came from.
    submits: HashMap<String, std::collections::VecDeque<u64>>,
    input_probes: std::collections::HashSet<String>,
    deadlines: Option<SyncSender<()>>,
    /// This host behind its lock, for work started from inside a commit.
    myself: std::sync::Weak<Mutex<Host>>,
    /// Terminal -> what its last quiet screen showed: the question and the rule that read
    /// it, or why none did. Live fallback only: never stored, cleared by the user's input;
    /// only a question appearing or going away is a new revision.
    /// Terminal -> what its CLI's own API says it asks.
    http_asks: HashMap<String, crate::cli_http::Asked>,
    screen_waits: HashMap<String, crate::screen_rules::Explained>,
    /// Terminal -> the conversation whose title spins there and since when (seconds since
    /// the epoch). Live evidence only, like `screen_waits`: never stored.
    title_busy: HashMap<String, (String, u64)>,
    /// The same read off the screen's ticking working line, for CLIs whose title is off.
    screen_busy: HashMap<String, (String, u64)>,
    /// Session -> last `claude agents --json` run for it, the per-session rate limit.
    reconciled: HashMap<String, Instant>,
    /// What the plugins' hooks last heard, so each state change reaches them once.
    hooks: crate::plugin_hooks::Hooks,
    /// `fibo run`'s children and the reports waiting for their parents.
    report_back: report_back::ReportBack,
    /// Which background shells of stopped turns are jobs the agent waits for.
    shell_watch: shell_watch::ShellWatch,
}
/// Records which conversation a live cue says is running on `terminal`, since when;
/// whether that changed. Only a turn starting or stopping is news.
fn cue(cues: &mut HashMap<String, (String, u64)>, terminal: &str, busy: Option<String>) -> bool {
    if cues.get(terminal).map(|(id, _)| id) == busy.as_ref() {
        return false;
    }
    match busy {
        Some(id) => cues.insert(terminal.into(), (id, fibo_core::content::now())),
        None => cues.remove(terminal),
    };
    true
}
fn geometry(cols: u16, rows: u16) -> io::Result<()> {
    if !(2..=400).contains(&cols) || !(1..=200).contains(&rows) {
        return Err(io::Error::other("terminal size outside 2..400 × 1..200"));
    }
    Ok(())
}
impl Host {
    fn snapshot(&self) -> Workspace {
        let mut w = self.workspace.clone();
        w.agent_sessions
            .retain(|s| s.terminal.is_some() || self.index.recent(s.updated));
        let retained: std::collections::HashSet<_> =
            w.agent_sessions.iter().map(|s| s.id.as_str()).collect();
        w.content.retain(|id, _| retained.contains(id.as_str()));
        // A job left in the background keeps a stopped turn working; a server does not.
        self.overlay_shells(&mut w);
        // A spinning title is a turn running even where the log shows none, as when only
        // the tail of a long turn was read. Never the other way: a still title proves
        // nothing the log has not said.
        for s in w.agent_sessions.iter().filter(|s| s.current) {
            let busy = |cues: &HashMap<String, (String, u64)>| {
                s.terminal
                    .as_ref()
                    .and_then(|t| cues.get(t))
                    .filter(|(id, _)| *id == s.id)
                    .map(|(_, since)| *since)
            };
            let (since, cue, detail) = match (busy(&self.title_busy), busy(&self.screen_busy)) {
                (Some(since), _) => (since, "标题", "终端标题在转圈，日志里还没看到这一轮"),
                (None, Some(since)) => (
                    since,
                    "画面在跑",
                    "画面上有在跑的计时行，日志里还没看到这一轮",
                ),
                (None, None) => continue,
            };
            let Some(c) = w.content.get_mut(&s.id) else {
                continue;
            };
            let working = matches!(
                c.turn,
                fibo_core::content::Turn::Running | fibo_core::content::Turn::Waiting
            ) || c.background.is_some_and(|b| b.keeps_working());
            if !working {
                c.turn = fibo_core::content::Turn::Running;
                c.turn_started = since;
                c.signal = cue.into();
                c.signal_at = since;
                c.signal_detail = detail.into();
            }
            // A dialog answered on the screen: the prompt is gone and the turn is ticking
            // again, started after the dialog was reported. No hook says so until the
            // approved tool finishes, which can be minutes.
            let asking = s
                .terminal
                .as_ref()
                .and_then(|t| self.screen_waits.get(t))
                .is_some_and(|e| e.question().is_some());
            if c.turn == fibo_core::content::Turn::Waiting && !asking && since >= c.signal_at {
                c.turn = fibo_core::content::Turn::Running;
                c.waiting.clear();
                c.choices.clear();
                c.signal = cue.into();
                c.signal_at = since;
                c.signal_detail = "确认框已答，画面上的计时行又出现了".into();
            }
        }
        // A prompt visible on a live agent's screen outranks a log that has not caught up.
        // The source stays marked so it is never mistaken for a transcript record.
        for s in w.agent_sessions.iter().filter(|s| s.current) {
            let Some(seen) = s.terminal.as_ref().and_then(|t| self.screen_waits.get(t)) else {
                continue;
            };
            let Some(c) = w.content.get_mut(&s.id) else {
                continue;
            };
            c.approval = seen.approval().unwrap_or_default().to_owned();
            if let Some(t) = &s.terminal {
                self.overlay_http_ask(t, c);
            }
            match seen.question() {
                Some(question) if c.turn != fibo_core::content::Turn::Waiting => {
                    c.turn = fibo_core::content::Turn::Waiting;
                    c.waiting = format!("{question}（画面上的提示）");
                    // The episode the log or a hook will record for this same dialog.
                    c.waiting_episode += 1;
                    c.signal = "画面".into();
                    c.signal_detail = seen.detail();
                }
                Some(_) => (),
                None => c.screen_check = seen.fallback(),
            }
        }
        // What the program itself reports outranks every guess above and the log: it is
        // the program's own word, not a reading of its output.
        for s in w.agent_sessions.iter().filter(|s| s.current) {
            let Some(r) = s
                .terminal
                .as_ref()
                .and_then(|t| self.recognition.terminal_reports.get(t))
            else {
                continue;
            };
            let Some(c) = w.content.get_mut(&s.id) else {
                continue;
            };
            fibo_core::program_state::apply(c, r);
        }
        self.keep_ended_turns(&mut w);
        w
    }
    fn set_screen_wait(
        &mut self,
        terminal: &str,
        seen: Option<crate::screen_rules::Explained>,
    ) -> io::Result<()> {
        let before = self.screen_waits.get(terminal).and_then(|e| e.question());
        let changed = before != seen.as_ref().and_then(|e| e.question());
        if let Some(question) = before.filter(|_| seen.as_ref().is_none_or(|e| !e.waiting())) {
            wait_log::ended(terminal, question);
        }
        match seen {
            Some(e) => self.screen_waits.insert(terminal.into(), e),
            None => self.screen_waits.remove(terminal),
        };
        // Why nothing matched rides along with the next revision; it is no change itself.
        if !changed {
            return Ok(());
        }
        // Nothing to store; a new revision makes clients fetch the overlaid snapshot.
        let next = self.workspace.clone();
        self.commit(next)
    }
    /// Asks the CLIs off the lock (`claude agents --json`, Codex `thread/read`) and applies the
    /// result. Returns when to look again for sessions held back by the rate limit.
    /// Current sessions showing a dialog projected from one of their subagents.
    fn child_dialogs(&self) -> Vec<AgentSession> {
        let w = &self.workspace;
        let now = fibo_core::content::now();
        let projected = |s: &&AgentSession| {
            fibo_core::status::of(w, s, now) == fibo_core::status::Status::Waiting
                && w.content
                    .get(&s.id)
                    .is_some_and(|c| c.signal == crate::content::CHILD_SIGNAL)
        };
        w.agent_sessions
            .iter()
            .filter(|s| s.current)
            .filter(projected)
            .cloned()
            .collect()
    }
    fn reconcile(&mut self, shared: &Shared, sessions: Vec<AgentSession>) -> Option<Instant> {
        let now = Instant::now();
        let mut later = None;
        let mut due = Vec::new();
        for s in sessions {
            match self.reconciled.get(&s.id) {
                Some(at) if now < *at + RECONCILE_EVERY => {
                    let next = *at + RECONCILE_EVERY;
                    later = Some(later.map_or(next, |l: Instant| l.min(next)));
                }
                _ => {
                    let pid = self
                        .agent_pids
                        .iter()
                        .find(|(_, id)| **id == s.id)
                        .map(|(pid, _)| *pid);
                    self.reconciled.insert(s.id.clone(), now);
                    due.push((s, pid));
                }
            }
        }
        if due.is_empty() {
            return later;
        }
        let Ok(index) = self.index.fork_current() else {
            return later;
        };
        let shared = shared.clone();
        std::thread::spawn(move || {
            if let Err(e) = apply_official(&shared, &index, &due) {
                eprintln!("fibo: official status unavailable: {e}");
            }
        });
        later
    }
    /// Phones on a relay changed: show the app.
    fn remote_changed(&mut self) -> io::Result<()> {
        let Some(remote) = self.remote.clone() else {
            return Ok(());
        };
        let status = remote.status();
        if status != self.recognition.remote {
            self.recognition.remote = status;
            self.publish_status()?;
        }
        Ok(())
    }
    fn publish_status(&mut self) -> io::Result<()> {
        self.recognition.workspace_revision = self.workspace.revision;
        let payload = Arc::new(
            serde_json::to_vec(&Reply::Recognition(recognition_status(self)))
                .map_err(io::Error::other)?,
        );
        status::offer_recognition(self, payload);
        Ok(())
    }
    fn agent_exited(&mut self, pid: u32) -> io::Result<()> {
        if let Some(terminal) = self
            .unbound_pids
            .remove(&pid)
            .map(|(terminal, _)| terminal)
            .or_else(|| self.unrecorded_pids.remove(&pid))
        {
            self.unhint(&terminal);
            self.publish_status()?;
        }
        // A fork that carries the CLI's command line (a shell's subshell, a node wrapper's
        // child) can end first: the conversation ends with the last process running it.
        if let Some(id) = self
            .agent_pids
            .remove(&pid)
            .filter(|id| !self.agent_pids.values().any(|other| other == id))
        {
            let mut next = self.workspace.clone();
            if let Some(s) = next
                .agent_sessions
                .iter_mut()
                .find(|s| s.id == id && s.current)
            {
                s.current = false;
                self.released_logs.insert(s.transcript.clone());
                if let Some(events) = &self.log_events {
                    events.file(s.transcript.clone().into());
                }
                self.commit(next)?;
            }
        }
        Ok(())
    }
    fn observe_processes(
        &mut self,
        processes: &[crate::recognition::Process],
        files: &HashMap<u32, Vec<String>>,
    ) -> io::Result<()> {
        let old_seen = (
            self.recognition.terminal_hints.clone(),
            self.recognition.terminal_notes.clone(),
            self.recognition.terminal_agents.clone(),
        );
        self.recognition.terminal_hints.clear();
        self.recognition.terminal_notes.clear();
        self.recognition.terminal_agents.clear();
        for process in processes.iter().filter(|p| p.runtime.is_some()) {
            let Some(terminal) =
                crate::recognition::terminal_for_process(process.pid, processes, &self.workspace)
            else {
                continue;
            };
            if let Some(session) = self
                .workspace
                .current_agent(&terminal)
                .filter(|s| Some(s.runtime) == process.runtime)
            {
                if process.cli_session_id.as_deref() != Some(&session.cli_session_id)
                    && !files
                        .get(&process.pid)
                        .is_some_and(|paths| paths.contains(&session.transcript))
                    && !crate::recognition::only_cli_of_screen_bound(
                        session,
                        process,
                        processes,
                        &self.workspace,
                    )
                {
                    continue;
                }
                let id = session.id.clone();
                if self.agent_pids.get(&process.pid) != Some(&id) {
                    self.agent_pids.insert(process.pid, id);
                    if let Some(exits) = &self.exits {
                        if !exits.watch(process.pid)? {
                            self.agent_exited(process.pid)?;
                        }
                    }
                }
            } else if let Some(runtime) = process.runtime {
                let adapter = crate::agents::adapter(runtime);
                let pending = adapter.pending(process, processes);
                // A CLI exists before its first transcript or explanatory note.
                if !matches!(pending, crate::agents::Pending::Wait) {
                    self.recognition
                        .terminal_agents
                        .insert(terminal.clone(), runtime);
                }
                let watch = match pending {
                    crate::agents::Pending::Wait => false,
                    crate::agents::Pending::Unrecorded(hint) => {
                        self.hint(&terminal, hint);
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            self.unrecorded_pids.entry(process.pid)
                        {
                            entry.insert(terminal);
                            true
                        } else {
                            false
                        }
                    }
                    // Unbound, whatever the CLI: screen association and hooks get their
                    // chance. The terminal says so unless the adapter reads a state from
                    // the title instead, or has nothing to say yet.
                    pending @ (crate::agents::Pending::Unbound { .. }
                    | crate::agents::Pending::Quiet) => {
                        if let crate::agents::Pending::Unbound { title_status: true } = pending {
                            let title = match self.terminals.get(&terminal) {
                                Some(live) => lock(live)?.title.clone(),
                                None => String::new(),
                            };
                            if let Some(hint) = adapter.title_status(&title) {
                                self.hint(&terminal, hint);
                            }
                        } else if pending
                            == (crate::agents::Pending::Unbound {
                                title_status: false,
                            })
                        {
                            self.hint(&terminal, crate::agents::unbound_hint(runtime));
                        }
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            self.unbound_pids.entry(process.pid)
                        {
                            entry.insert((terminal, runtime));
                            true
                        } else {
                            false
                        }
                    }
                };
                if watch {
                    if let Some(exits) = &self.exits {
                        if !exits.watch(process.pid)? {
                            self.agent_exited(process.pid)?;
                        }
                    }
                }
            }
        }
        if old_seen
            != (
                self.recognition.terminal_hints.clone(),
                self.recognition.terminal_notes.clone(),
                self.recognition.terminal_agents.clone(),
            )
        {
            self.publish_status()?;
        }
        Ok(())
    }
    /// Every tab names itself from its program's title, front or not. An unbound Agent
    /// whose adapter reads state from the title also paints it into its note.
    /// Commands the shell in `terminal` marked finished: the last one goes to the app (for
    /// pane decorations), each one to the hooks and event subscribers that listen for them.
    /// No title: shells title the terminal with the running command, which may hold a secret.
    fn commands_finished(
        &mut self,
        terminal: &str,
        pwd: Option<String>,
        finished: Vec<crate::command_marks::Finished>,
    ) {
        use fibo_core::hook::{Event, Payload};
        let placed = self.workspace.terminals.iter().find(|t| t.id == terminal);
        let space = placed
            .and_then(|t| self.workspace.spaces.iter().find(|p| p.id == t.space))
            .map(|p| p.name.clone());
        let cwd = pwd
            .or_else(|| placed.map(|t| t.cwd.clone()))
            .unwrap_or_default();
        if let Some(f) = finished.last() {
            let last = fibo_core::protocol::LastCommand {
                exit: f.exit,
                took_ms: u64::try_from(f.took.as_millis()).unwrap_or(u64::MAX),
            };
            if self
                .recognition
                .terminal_commands
                .insert(terminal.into(), last)
                != Some(last)
            {
                if let Err(e) = self.publish_status() {
                    eprintln!("terminal {terminal}: last command: {e}");
                }
            }
        }
        // One frame per read of output, however many commands it finished.
        let mut notices = vec![];
        for f in finished {
            let payload = Payload {
                event: match f.exit {
                    Some(code) if code != 0 => Event::CommandFailed,
                    _ => Event::CommandDone,
                },
                session: String::new(),
                agent: String::new(),
                title: String::new(),
                cwd: cwd.clone(),
                space: space.clone(),
                detail: String::new(),
                at: fibo_core::content::now(),
                terminal: Some(terminal.into()),
                exit_code: f.exit,
                duration_ms: Some(u64::try_from(f.took.as_millis()).unwrap_or(u64::MAX)),
            };
            if !self.event_subscribers.is_empty() {
                notices.push(payload.clone().into());
            }
            // Scripts subscribed above still hear it; a plugin may notify you.
            if !self.workspace.terminal_quiet(terminal) {
                crate::plugin_hooks::command(payload);
            }
        }
        self.send_events(notices);
    }
    /// The program in `terminal` reported its status, or its report ended. Viewers get
    /// it with the recognition status; a terminal with a live conversation also gets a
    /// new revision, since the report outranks the log in the snapshot.
    fn program_said(
        &mut self,
        terminal: &str,
        said: Option<crate::program_status::Said>,
    ) -> io::Result<()> {
        let before = self.recognition.terminal_reports.get(terminal);
        let next = said.map(|s| fibo_core::protocol::ProgramReport {
            since: before
                .filter(|b| b.state == s.state)
                .map_or_else(fibo_core::content::now, |b| b.since),
            state: s.state,
            kind: s.kind,
            progress: s.progress,
            app: s.app,
            title: s.title,
            msg: s.msg,
        });
        if before == next.as_ref() {
            return Ok(());
        }
        let turn_changed = before.map(|b| &b.state) != next.as_ref().map(|n| &n.state);
        match next {
            Some(next) => self
                .recognition
                .terminal_reports
                .insert(terminal.into(), next),
            None => self.recognition.terminal_reports.remove(terminal),
        };
        self.publish_status()?;
        if turn_changed && self.workspace.current_agent(terminal).is_some() {
            let next = self.workspace.clone();
            self.commit(next)?;
        }
        Ok(())
    }
    fn title_changed(&mut self, terminal: &str, title: &str) -> io::Result<()> {
        let shown = fibo_core::terminal::shown_title(title);
        let mut changed = self.recognition.terminal_titles.get(terminal) != shown.as_ref();
        match shown {
            Some(shown) => self
                .recognition
                .terminal_titles
                .insert(terminal.into(), shown),
            None => self.recognition.terminal_titles.remove(terminal),
        };
        let runtime = self
            .unbound_pids
            .values()
            .find(|(t, _)| t == terminal)
            .map(|(_, runtime)| *runtime);
        if let Some(runtime) =
            runtime.filter(|_| self.recognition.terminal_hints.contains_key(terminal))
        {
            if let Some(hint) = crate::agents::adapter(runtime).title_status(title) {
                changed |= self.hint(terminal, hint);
            }
        }
        if changed {
            self.publish_status()?;
        }
        // Checked on every title change, but only a turn starting or stopping is news.
        let busy = self.workspace.current_agent(terminal).and_then(|s| {
            crate::agents::adapter(s.runtime)
                .title_busy(title)
                .filter(|busy| *busy)
                .map(|_| s.id.clone())
        });
        if cue(&mut self.title_busy, terminal, busy) {
            // Nothing to store; a new revision makes clients fetch the overlaid snapshot.
            let next = self.workspace.clone();
            self.commit(next)?;
        }
        Ok(())
    }
    /// What the bottom of a bound terminal's screen says about its turn; see `title_changed`.
    fn screen_busy(&mut self, terminal: &str, tail: &str) -> io::Result<()> {
        let busy = self.workspace.current_agent(terminal).and_then(|s| {
            crate::agents::adapter(s.runtime)
                .screen_busy(tail)
                .filter(|busy| *busy)
                .map(|_| s.id.clone())
        });
        if cue(&mut self.screen_busy, terminal, busy) {
            let next = self.workspace.clone();
            self.commit(next)?;
        }
        Ok(())
    }
    /// Sets a terminal's hint and note together; whether either changed.
    fn hint(&mut self, terminal: &str, hint: crate::agents::Hint) -> bool {
        let changed = self.recognition.terminal_hints.get(terminal) != Some(&hint.text)
            || self.recognition.terminal_notes.get(terminal) != Some(&hint.note);
        self.recognition
            .terminal_hints
            .insert(terminal.into(), hint.text);
        self.recognition
            .terminal_notes
            .insert(terminal.into(), hint.note);
        changed
    }
    fn unhint(&mut self, terminal: &str) {
        self.recognition.terminal_hints.remove(terminal);
        self.recognition.terminal_notes.remove(terminal);
        self.recognition.terminal_agents.remove(terminal);
    }
    /// Types the reports `fibo run`'s children left for starters that are idle now.
    /// Off this thread: typing takes the host lock, which a commit holds.
    fn deliver_reports(&mut self, w: &Workspace) {
        for (parent, text) in self.report_back.plan(w) {
            let Some(shared) = self.myself.upgrade() else {
                return;
            };
            thread::spawn(move || {
                if let Err(e) = client::send_text(&shared, &parent, &text, true, None) {
                    eprintln!("report to {parent}: {e}");
                }
            });
        }
    }
    fn commit(&mut self, next: Workspace) -> io::Result<()> {
        self.commit_applying(next, None)
    }
    /// `commit`, recording `applied` as applied in the transaction that saves `next`.
    fn commit_applying(
        &mut self,
        mut next: Workspace,
        applied: Option<&fibo_core::protocol::RequestId>,
    ) -> io::Result<()> {
        next.revision = self.workspace.revision + 1;
        self.store.save_applying(&self.workspace, &next, applied)?;
        // Terminals that just got their Agent conversation. The quiet-screen check after
        // the last output skipped them while unbound; a dialog drawn before binding (an
        // Agent that starts on an approval, or a slow identification under load) would
        // otherwise never be seen, since the screen stays still.
        let bound: Vec<String> = next
            .agent_sessions
            .iter()
            .filter(|s| s.current)
            .filter_map(|s| s.terminal.clone())
            .filter(|t| {
                self.workspace.current_agent(t).map(|s| &s.id)
                    != next.current_agent(t).map(|s| &s.id)
            })
            .collect();
        let hooked = !fibo_core::hook::all().is_empty();
        if hooked && !self.hooks.seeded() {
            // The state before the first change is where hooks start listening.
            let before = self.snapshot();
            self.hooks.changes(&before);
        }
        self.workspace = next;
        let reporting = !self.report_back.is_empty();
        if hooked || !self.event_subscribers.is_empty() || reporting {
            let after = self.snapshot();
            if hooked {
                self.hooks.observe(&after);
            }
            if !self.event_subscribers.is_empty() {
                let notices = self.events.changes(&after);
                self.send_events(notices);
            }
            if reporting {
                self.deliver_reports(&after);
            }
        }
        for id in bound {
            let (Some(shared), Some(terminal)) =
                (self.myself.upgrade(), self.terminals.get(&id).cloned())
            else {
                continue;
            };
            thread::spawn(move || {
                let mut at = Instant::now() + SCREEN_QUIET;
                loop {
                    thread::sleep(at.saturating_duration_since(Instant::now()));
                    match check_screen_wait(&terminal, &shared, &id) {
                        Ok(Some(next)) => at = next,
                        Ok(None) => break,
                        Err(e) => {
                            eprintln!("screen check after binding {id}: {e}");
                            break;
                        }
                    }
                }
            });
        }
        if let Some(tx) = &self.deadlines {
            let _ = tx.try_send(());
        }
        self.shell_watch.poke();
        self.publish_status()?;
        Ok(())
    }
}
pub fn serve(root: &Path) -> io::Result<()> {
    serve_with_roots(root, crate::recognition::roots()?)
}
/// Explicit roots keep fixture execution independent of the user's CLI history.
pub fn serve_with_roots(root: &Path, roots: Vec<(Agent, std::path::PathBuf)>) -> io::Result<()> {
    serve_with_options(root, roots, 7)
}
pub fn serve_with_options(
    root: &Path,
    roots: Vec<(Agent, std::path::PathBuf)>,
    recent_days: u64,
) -> io::Result<()> {
    serve_runtime(root, roots, recent_days, false)
}
/// Starts by taking every running terminal over from the host already serving `root`,
/// which then exits; its shells and Agents keep running under this host.
pub fn serve_taking_over(
    root: &Path,
    roots: Vec<(Agent, std::path::PathBuf)>,
    recent_days: u64,
) -> io::Result<()> {
    serve_runtime(root, roots, recent_days, true)
}

/// This machine's name, as the shell's $HOST spells it in OSC 7 reports.
fn hostname() -> String {
    fibo_platform::system::host_name()
}

#[cfg(test)]
mod recognition_tests;
#[cfg(test)]
mod tests;
