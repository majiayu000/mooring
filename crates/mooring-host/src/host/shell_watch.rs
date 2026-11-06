//! Keeps looking at the background shells of turns that stopped with some left, so the
//! snapshot can tell a job the agent waits for from a server it left running. Looks only
//! while such a turn exists; otherwise the thread waits for the next commit.
use super::*;
use crate::background_shells::{self as shells, Judged, Seen};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

/// How often to check for new targets when nothing has poked.
const IDLE: Duration = Duration::from_secs(30);
/// After a restart, how long a stored shell with no verdict yet still counts as a job.
/// Agents are recognized and logs searched in this time; without an end, a shell whose
/// agent is never recognized would keep its session spinning.
const SETTLE: Duration = Duration::from_secs(120);
/// (task id, command) for each shell one Stop listed.
type Tasks = Vec<(String, String)>;
#[derive(Default)]
pub(super) struct ShellWatch {
    /// Session -> (task id, command) from its last Stop. Memory only: a command line may
    /// hold a secret, so it is never stored or sent.
    commands: HashMap<String, Tasks>,
    /// (session, task id) -> the judgment so far.
    judged: HashMap<(String, String), Judged>,
    /// Sessions whose log was already searched for commands a restart lost.
    searched: HashSet<String>,
    /// When this host started looking; `SETTLE` runs from here.
    started: Option<Instant>,
    /// `SETTLE` has ended and clients were told: unjudged shells stop counting as jobs.
    settled: bool,
    /// Terminal -> since when its screen says a command was left running.
    pub(super) on_screen: HashMap<String, Instant>,
    /// Session -> the pids of the commands found for it, each judged in `judged`.
    pub(super) found: HashMap<String, Vec<String>>,
    wake: Option<SyncSender<()>>,
}
impl ShellWatch {
    /// A hook listed `session`'s shells: these replace what it listed before.
    pub(super) fn listed(&mut self, session: &str, facts: &HookFacts) {
        if facts.background.is_none() {
            return;
        }
        let tasks: Vec<_> = facts
            .shells
            .iter()
            .map(|t| (t.id.clone(), t.command.clone()))
            .collect();
        self.judged
            .retain(|(s, id), _| s != session || tasks.iter().any(|(t, _)| t == id));
        if tasks.is_empty() {
            self.commands.remove(session);
        } else {
            self.commands.insert(session.into(), tasks);
        }
        self.poke();
    }
    /// Its screen stopped saying so: nothing to look at or remember for this session.
    pub(super) fn forget(&mut self, session: &str) {
        self.found.remove(session);
        self.judged.retain(|(s, _), _| s != session);
    }
    /// One look at a session's found commands; whether the snapshot reads differently.
    fn found_now(
        &mut self,
        session: &str,
        pids: Vec<String>,
        seen: &HashMap<String, Seen>,
    ) -> bool {
        self.judged
            .retain(|(s, id), _| s != session || pids.contains(id));
        let tasks = pids.iter().map(|p| (p.clone(), String::new())).collect();
        let targets = [(session.to_owned(), 0, tasks)];
        let changed = self.judge(&targets, seen);
        let before = self.found.insert(session.to_owned(), pids);
        changed || before != self.found.get(session).cloned()
    }
    pub(super) fn poke(&self) {
        if let Some(tx) = &self.wake {
            let _ = tx.try_send(());
        }
    }
    /// What a session's log says its listed shells run. A Stop that came in the meantime
    /// knows better, so it stays.
    fn recovered(&mut self, session: &str, tasks: Tasks) {
        if !tasks.is_empty() && !self.commands.contains_key(session) {
            self.commands.insert(session.into(), tasks);
        }
    }
    /// One look's verdicts; whether any shell changed sides.
    fn judge(&mut self, targets: &[(String, u32, Tasks)], seen: &HashMap<String, Seen>) -> bool {
        let mut changed = false;
        for (session, _, tasks) in targets {
            for (task, _) in tasks {
                let key = (session.clone(), task.clone());
                let before = self.judged.get(&key).copied();
                let now = shells::judge(before, seen.get(task).copied());
                changed |= before.map(|b| b.job) != Some(now.job);
                self.judged.insert(key, now);
            }
        }
        changed
    }
    /// Whether this task of `session` is a job, by what the looks so far found. With no
    /// look yet, a host that just restarted takes it for one: claiming work for a minute
    /// is better than calling you over to a session that is still working.
    pub(super) fn job(&self, session: &str, task: &str) -> bool {
        match self.judged.get(&(session.to_owned(), task.to_owned())) {
            Some(j) => j.job,
            None => self.settling() && !self.search_found_nothing(session),
        }
    }
    fn settling(&self) -> bool {
        self.started.is_some_and(|at| at.elapsed() < SETTLE)
    }
    fn search_found_nothing(&self, session: &str) -> bool {
        self.searched.contains(session) && !self.commands.contains_key(session)
    }
    /// True once, when `SETTLE` has just ended and the snapshot may read differently.
    fn just_settled(&mut self) -> bool {
        if self.settled || self.started.is_none() || self.settling() {
            return false;
        }
        self.settled = true;
        true
    }
}
impl Host {
    /// Live agents whose stored shells have no command in memory: the host restarted since
    /// their Stop. Each (session, log, task ids) comes up once; reading the log takes long
    /// enough that the caller lets go of the host first.
    fn shells_lost(&mut self) -> Vec<(String, PathBuf, Vec<String>)> {
        let live: HashSet<&String> = self.agent_pids.values().collect();
        let watch = &mut self.shell_watch;
        self.workspace
            .agent_sessions
            .iter()
            .filter(|s| live.contains(&s.id) && !s.transcript.is_empty())
            .filter(|s| !watch.commands.contains_key(&s.id))
            .filter_map(|s| {
                let c = self.workspace.content.get(&s.id)?;
                let ids: Vec<_> = c.shells.iter().map(|t| t.id.clone()).collect();
                (!ids.is_empty() && watch.searched.insert(s.id.clone()))
                    .then(|| (s.id.clone(), PathBuf::from(&s.transcript), ids))
            })
            .collect()
    }
    /// Sessions to look at: a live agent whose stopped turn left shells it listed.
    fn shell_targets(&self) -> Vec<(String, u32, Tasks)> {
        self.agent_pids
            .iter()
            .filter_map(|(pid, id)| {
                let c = self.workspace.content.get(id)?;
                // A turn that starts again clears its shells (see `Parser::phase`).
                if c.shells.is_empty() {
                    return None;
                }
                let tasks: Vec<_> = self
                    .shell_watch
                    .commands
                    .get(id)?
                    .iter()
                    .filter(|(t, _)| c.shells.iter().any(|s| &s.id == t))
                    .cloned()
                    .collect();
                (!tasks.is_empty()).then(|| (id.clone(), *pid, tasks))
            })
            .collect()
    }
    /// Marks each listed shell of a live session a job or not, and counts the jobs.
    pub(super) fn overlay_shells(&self, w: &mut Workspace) {
        for s in w.agent_sessions.iter().filter(|s| s.current) {
            let Some(c) = w.content.get_mut(&s.id) else {
                continue;
            };
            if c.shells.is_empty() {
                continue;
            }
            for t in &mut c.shells {
                t.job = self.shell_watch.job(&s.id, &t.id);
            }
            if let Some(b) = &mut c.background {
                b.jobs = (c.shells.iter().filter(|t| t.job).count() as u32).min(b.shells);
            }
        }
    }
}
/// A new host has seen no screen go quiet yet, and a quiet one will not again.
fn read_screens(shared: &Shared) {
    let Ok(screens) = lock(shared).map(|host| host.screens_to_read()) else {
        return;
    };
    for (id, live) in screens {
        let tail = lock(&live).and_then(|l| l.vt.tail_text(usize::from(l.rows)));
        let (Ok(tail), Ok(mut host)) = (tail, lock(shared)) else {
            continue;
        };
        if let Err(e) = host.screen_background(&id, &tail) {
            eprintln!("background shells: {e}");
        }
    }
}
/// Finds and judges the commands of `looking`. `claimed` remembers what each log said about
/// a process, since reading a log is the slow part; a young one may not be logged yet.
fn look_for_commands(
    shared: &Shared,
    looking: &found_shells::Looking,
    claimed: &mut HashMap<(String, u32), bool>,
) {
    if looking.is_empty() {
        claimed.clear();
        return;
    }
    let executable = crate::agents::codex_background::process_name();
    let commands = crate::cli_commands::commands_of(&executable);
    for (session, log) in looking {
        let mine: Vec<(String, u32)> = commands
            .iter()
            .filter(|(pid, command)| {
                let key = (session.clone(), *pid);
                if let Some(known) = claimed.get(&key) {
                    return *known;
                }
                let ran = crate::cli_commands::log_ran(log, command).unwrap_or(false);
                if ran || shells::age(*pid) > 60 {
                    claimed.insert(key, ran);
                }
                ran
            })
            .map(|(pid, _)| (pid.to_string(), *pid))
            .collect();
        let seen = match shells::look_at(&mine) {
            Ok(seen) => seen,
            Err(e) => {
                eprintln!("background shells: {e}");
                continue;
            }
        };
        let pids = mine.into_iter().map(|(id, _)| id).collect();
        let Ok(mut host) = lock(shared) else {
            return;
        };
        if host.shell_watch.found_now(session, pids, &seen) {
            let next = host.workspace.clone();
            if let Err(e) = host.commit(next) {
                eprintln!("background shells: {e}");
            }
        }
    }
}
pub(super) fn start(shared: &Shared) -> io::Result<()> {
    let (tx, rx) = sync_channel(1);
    {
        let mut host = lock(shared)?;
        host.shell_watch.wake = Some(tx);
        host.shell_watch.started = Some(Instant::now());
    }
    let weak = Arc::downgrade(shared);
    let mut screens_read = false;
    let mut claimed = HashMap::new();
    thread::spawn(move || loop {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        let Ok(lost) = lock(&shared).map(|mut host| {
            if host.shell_watch.just_settled() {
                let next = host.workspace.clone();
                if let Err(e) = host.commit(next) {
                    eprintln!("background shells: {e}");
                }
            }
            host.shells_lost()
        }) else {
            return;
        };
        for (session, log, ids) in lost {
            let tasks = shells::commands_in_log(&log, &ids).unwrap_or_else(|e| {
                eprintln!("background shells: {}: {e}", log.display());
                vec![]
            });
            let Ok(mut host) = lock(&shared) else {
                return;
            };
            host.shell_watch.recovered(&session, tasks);
            // A search that found nothing changes what the snapshot says.
            if host.shell_watch.search_found_nothing(&session) {
                let next = host.workspace.clone();
                if let Err(e) = host.commit(next) {
                    eprintln!("background shells: {e}");
                }
            }
        }
        if !screens_read {
            screens_read = true;
            read_screens(&shared);
        }
        let Ok((targets, looking)) =
            lock(&shared).map(|host| (host.shell_targets(), host.looking()))
        else {
            return;
        };
        look_for_commands(&shared, &looking, &mut claimed);
        drop(shared);
        if targets.is_empty() && looking.is_empty() {
            // Not forever: an agent recognized after a restart announces itself to no one.
            match rx.recv_timeout(IDLE) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            }
            continue;
        }
        let looks: Vec<(u32, Tasks)> = targets
            .iter()
            .map(|(_, pid, tasks)| (*pid, tasks.clone()))
            .collect();
        match shells::look(&looks) {
            Ok(seen) => {
                let Some(shared) = weak.upgrade() else {
                    return;
                };
                let Ok(mut host) = lock(&shared) else {
                    return;
                };
                let changed = host.shell_watch.judge(&targets, &seen);
                // Nothing to store; a new revision makes clients fetch the overlaid snapshot.
                if changed {
                    let next = host.workspace.clone();
                    if let Err(e) = host.commit(next) {
                        eprintln!("background shells: {e}");
                    }
                }
            }
            Err(e) => eprintln!("background shells: {e}"),
        }
        match rx.recv_timeout(Duration::from_secs(shells::EVERY)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooring_core::content::{Background, Content, ShellTask};
    fn watch_started(ago: Duration) -> ShellWatch {
        ShellWatch {
            started: Instant::now().checked_sub(ago),
            ..Default::default()
        }
    }
    /// Undoing `settling` in `job` fails the first assert: the session shows 等后台 under
    /// 要你处理 for the seconds after every restart.
    #[test]
    fn a_shell_with_no_verdict_is_a_job_only_while_a_restart_settles() {
        let young = watch_started(Duration::from_secs(1));
        assert!(young.job("s", "b1"), "just restarted, not looked at yet");

        let mut empty = watch_started(Duration::from_secs(1));
        empty.searched.insert("s".into());
        assert!(!empty.job("s", "b1"), "its log does not know this shell");

        let mut found = watch_started(Duration::from_secs(1));
        found.searched.insert("s".into());
        found.recovered("s", vec![("b1".into(), "cargo test".into())]);
        assert!(found.job("s", "b1"), "found, the first look is coming");

        let old = watch_started(SETTLE + Duration::from_secs(1));
        assert!(
            !old.job("s", "b1"),
            "an agent never recognized: no spinning"
        );
        assert!(!ShellWatch::default().job("s", "b1"));

        // A server's verdict wins at once, even while settling.
        let mut judged = watch_started(Duration::from_secs(1));
        let server = shells::Seen {
            listening: true,
            own_client: false,
            worker: false,
            age: 60,
        };
        let targets = vec![(
            "s".to_owned(),
            1,
            vec![("b1".to_owned(), "python3 -m http.server".to_owned())],
        )];
        judged.judge(&targets, &HashMap::from([("b1".to_owned(), server)]));
        assert!(!judged.job("s", "b1"));
    }
    #[test]
    fn the_end_of_settling_is_announced_once() {
        let mut watch = watch_started(Duration::from_secs(1));
        assert!(!watch.just_settled());
        let mut watch_old = watch_started(SETTLE + Duration::from_secs(1));
        assert!(watch_old.just_settled());
        assert!(!watch_old.just_settled());
        watch.started = None;
        assert!(!watch.just_settled());
    }
    /// A host just restarted: the session's shell is alive and listed in the store, and
    /// the command is known only to the log. Undoing the search leaves `jobs` at 0, which
    /// is the session showing 等后台 under 要你处理 while its polling loop still runs.
    #[test]
    fn a_job_still_running_when_the_host_restarts_stays_a_job() {
        let (mut host, root) = crate::host::tests::fixture();
        let command = "sleep 30; echo restart-4e1a";
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("eval '{command}'")])
            .spawn()
            .unwrap();
        let log = root.join("s.jsonl");
        let rows = [
            serde_json::json!({"message":{"content":[
                {"type":"tool_use","id":"t1","name":"Bash","input":{"command":command}}]}}),
            serde_json::json!({"message":{"content":[
                {"type":"tool_result","tool_use_id":"t1",
                 "content":"Command running in background with ID: b1. Output is"}]}}),
        ]
        .map(|r| r.to_string());
        fs::write(&log, rows.join("\n")).unwrap();
        let mut session = crate::host::tests::session("s", 1, Some("t"), true);
        session.transcript = log.to_string_lossy().into_owned();
        host.workspace.agent_sessions = vec![session];
        host.workspace.content.insert(
            "s".into(),
            Content {
                shells: vec![ShellTask {
                    id: "b1".into(),
                    description: "wait".into(),
                    job: false,
                }],
                background: Some(Background {
                    shells: 1,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        host.agent_pids.insert(std::process::id(), "s".into());
        let jobs = |host: &Host| {
            let mut w = host.workspace.clone();
            host.overlay_shells(&mut w);
            w.content["s"].background.unwrap().jobs
        };
        assert!(host.shell_targets().is_empty(), "the restart lost it");
        assert_eq!(jobs(&host), 0);

        let lost = host.shells_lost();
        assert_eq!(lost.len(), 1);
        assert!(host.shells_lost().is_empty(), "the log is read only once");
        let (id, path, ids) = &lost[0];
        let tasks = shells::commands_in_log(path, ids).unwrap();
        host.shell_watch.recovered(id, tasks);
        let targets = host.shell_targets();
        let looks: Vec<_> = targets.iter().map(|(_, p, t)| (*p, t.clone())).collect();
        let seen = shells::look(&looks).unwrap();
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&root);
        assert!(host.shell_watch.judge(&targets, &seen));
        assert_eq!(jobs(&host), 1);
    }
}
