//! Other machines the window shows beside this one (docs/multi-machine/spec.md, step 1:
//! look only). Each runs `ssh <host> <fibo> remote --stdio` and speaks the remote
//! protocol, the same one the phone does. A supervisor thread per machine keeps its list
//! of terminals fresh from `events` and reconnects after a drop. Nothing runs when no
//! machine is configured.
use fibo_remote::client::Client;
use fibo_remote::protocol::{Card, Hello, List, Machine, Op, Space, StreamBody};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const REPLY: Duration = Duration::from_secs(10);
/// Events in a burst are one fresh list.
const SETTLE: Duration = Duration::from_millis(150);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How stale the list may get when no event says anything changed.
const REFRESH: Duration = Duration::from_secs(3);

/// One machine as the user wrote it down, kept in `<config>/machines.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    /// What `ssh` connects to: a name from `~/.ssh/config`, or `user@host`.
    pub host: String,
    /// `fibo` on that machine; unset tries `fibo` on its PATH, then the app bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 先不管这台: it stays connected but asks nothing of you here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub quiet: bool,
}

impl Spec {
    pub fn new(host: String) -> Self {
        Self {
            host,
            path: None,
            quiet: false,
        }
    }
    pub fn command(&self) -> Vec<String> {
        self.ssh(&match &self.path {
            Some(path) => format!("exec {} remote --stdio", quote(path)),
            None => format!("{}; exec \"$p\" remote --stdio", self.find_program()),
        })
    }
    /// Shell that sets `$p` to `fibo` on that machine: the path written down, else on its
    /// PATH (by the new name, then the old), else the app bundle in ~/Applications, else
    /// /Applications; Fibo.app, else one installed as Mooring.app.
    pub fn find_program(&self) -> String {
        match &self.path {
            Some(path) => format!("p={}", quote(path)),
            None => [
                "p=$(command -v fibo || command -v mooring)",
                "for d in \"$HOME/Applications\" /Applications; do for a in Fibo Mooring; do [ -x \"$p\" ] || p=\"$d/$a.app/Contents/MacOS/mooring\"; done; done",
            ]
            .join("; "),
        }
    }
    /// `ssh` to this machine running `script` there, with the options every link uses.
    pub fn ssh(&self, script: &str) -> Vec<String> {
        let mut argv = self.ssh_options("-T");
        argv.extend([self.host.clone(), script.to_owned()]);
        argv
    }
    /// `ssh <mode>` and the options every link to this machine uses, before the host.
    pub fn ssh_options(&self, mode: &str) -> Vec<String> {
        [
            "ssh",
            mode,
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
        ]
        .map(str::to_owned)
        .to_vec()
    }
}

use fibo_platform::shell::quote_posix as quote;

pub fn file() -> Option<PathBuf> {
    fibo_core::agent::config_dir().map(|d| d.join("machines.json"))
}

pub fn load() -> io::Result<Vec<Spec>> {
    load_from(&file().ok_or_else(|| io::Error::other(t!("找不到配置目录")))?)
}

fn load_from(file: &std::path::Path) -> io::Result<Vec<Spec>> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(io::Error::new(
                e.kind(),
                t!("读不了 {display}：{e}", display = file.display(), e),
            ));
        }
    };
    serde_json::from_slice(&bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            t!("{display} 格式不对：{e}", display = file.display(), e),
        )
    })
}

pub fn save(specs: &[Spec]) -> io::Result<()> {
    save_to(
        &file().ok_or_else(|| io::Error::other(t!("找不到配置目录")))?,
        specs,
    )
}

fn save_to(f: &std::path::Path, specs: &[Spec]) -> io::Result<()> {
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::ui::status_colors::write_atomic(
        f,
        &serde_json::to_vec_pretty(specs).map_err(io::Error::other)?,
    )
}

/// `specs` with `host` quiet or not; `None` when it is not among them, so a file that could
/// not be read is never written back empty.
fn with_quiet(mut specs: Vec<Spec>, host: &str, quiet: bool) -> Option<Vec<Spec>> {
    specs.iter_mut().find(|s| s.host == host)?.quiet = quiet;
    Some(specs)
}
/// Sets 先不管 on what the file holds now, leaving everything else in it alone.
pub fn set_quiet(host: &str, quiet: bool) -> io::Result<()> {
    let specs = with_quiet(load()?, host, quiet)
        .ok_or_else(|| io::Error::other(t!("machines.json 里没有 {host}", host)))?;
    save(&specs)
}
/// `next` with what `saved` says about 先不管: a list held since before the switch does not
/// undo it.
fn keep_quiet(mut next: Vec<Spec>, saved: &[Spec]) -> Vec<Spec> {
    for spec in &mut next {
        if let Some(s) = saved.iter().find(|s| s.host == spec.host) {
            spec.quiet = s.quiet;
        }
    }
    next
}
/// Saves the list settings edits, which adds and removes machines and nothing else.
pub fn save_keeping_quiet(next: &[Spec]) -> io::Result<()> {
    save_keeping_quiet_to(
        &file().ok_or_else(|| io::Error::other(t!("找不到配置目录")))?,
        next,
    )
}

fn save_keeping_quiet_to(file: &std::path::Path, next: &[Spec]) -> io::Result<()> {
    save_to(file, &keep_quiet(next.to_vec(), &load_from(file)?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Online,
    /// Why it is not connected; it keeps retrying.
    Offline(String),
}

/// What the window draws for one machine.
#[derive(Debug, Clone)]
pub struct View {
    pub spec: Spec,
    pub link: Link,
    /// Its own name and id, once it said hello.
    pub machine: Option<Machine>,
    pub spaces: Vec<Space>,
    pub terminals: Vec<Card>,
}

/// A machine's live connection and what its `hello` lets this desktop do there.
#[derive(Clone)]
pub struct Linked {
    pub client: Arc<Client>,
    /// Typing into its terminals (`input`).
    pub typing: bool,
    /// Answering its approvals (`answer`).
    pub answering: bool,
    /// Taking its terminals over and sizing them (`resize`).
    pub resizing: bool,
    /// Opening and ending its terminals and settling its conversations (`open`, `end`,
    /// `outcome`).
    pub managing: bool,
    /// Listing its folders and making a space in one (`dirs`, `addspace`).
    pub adding_spaces: bool,
}

/// Where a machine's live connection is, if it has one now.
pub type Slot = Arc<Mutex<Option<Linked>>>;

/// One machine's connection, kept running by its supervisor until dropped.
pub struct Remote {
    pub view: Arc<Mutex<View>>,
    client: Slot,
    stop: Arc<AtomicBool>,
}

impl Remote {
    /// Starts the supervisor. `connect` makes a fresh client each attempt; `changed` is
    /// called after every change to `view`, from the supervisor's thread.
    pub fn start(
        spec: Spec,
        connect: impl Fn(&Spec) -> io::Result<Client> + Send + 'static,
        changed: impl Fn() + Send + 'static,
    ) -> Self {
        let view = Arc::new(Mutex::new(View {
            spec: spec.clone(),
            link: Link::Connecting,
            machine: None,
            spaces: Vec::new(),
            terminals: Vec::new(),
        }));
        let client = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let (v, c, s) = (Arc::clone(&view), Arc::clone(&client), Arc::clone(&stop));
        std::thread::spawn(move || supervise(spec, connect, changed, v, c, s));
        Self { view, client, stop }
    }

    /// A machine that lists `terminals` and never connects: for screenshots only.
    #[cfg(feature = "visual-test")]
    pub fn fixed(spec: Spec, spaces: Vec<Space>, terminals: Vec<Card>) -> Self {
        let view = View {
            spec,
            link: Link::Online,
            machine: None,
            spaces,
            terminals,
        };
        Self {
            view: Arc::new(Mutex::new(view)),
            client: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(true)),
        }
    }
    /// Where its live connection is, for a terminal's screen.
    pub fn slot(&self) -> Slot {
        Arc::clone(&self.client)
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Dropping the last handle kills the ssh child, which ends the supervisor's wait.
        self.client.lock().unwrap().take();
    }
}

fn supervise(
    spec: Spec,
    connect: impl Fn(&Spec) -> io::Result<Client>,
    changed: impl Fn(),
    view: Arc<Mutex<View>>,
    slot: Slot,
    stop: Arc<AtomicBool>,
) {
    let mut backoff = Duration::from_secs(1);
    while !stop.load(Ordering::Relaxed) {
        let why = match run_once(&spec, &connect, &changed, &view, &slot, &stop) {
            Ok(()) => return,
            Err(e) => e.to_string(),
        };
        slot.lock().unwrap().take();
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let was_online = matches!(view.lock().unwrap().link, Link::Online);
        if was_online {
            backoff = Duration::from_secs(1);
        }
        view.lock().unwrap().link = Link::Offline(why);
        changed();
        // Sleep in small steps so a removed machine stops promptly.
        let mut slept = Duration::ZERO;
        while slept < backoff && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            slept += Duration::from_millis(100);
        }
        backoff = (backoff * 2).min(BACKOFF_MAX);
        if !stop.load(Ordering::Relaxed) {
            view.lock().unwrap().link = Link::Connecting;
            changed();
        }
    }
}

/// One connection, until it drops (an error) or the machine is removed (`Ok`).
fn run_once(
    spec: &Spec,
    connect: &impl Fn(&Spec) -> io::Result<Client>,
    changed: &impl Fn(),
    view: &Mutex<View>,
    slot: &Mutex<Option<Linked>>,
    stop: &AtomicBool,
) -> io::Result<()> {
    let client = Arc::new(connect(spec)?);
    let hello: Hello = parse(client.request(
        Op::Hello {
            version: Some(1),
            // One name for this app's life, so a write resent after a reconnect is known.
            client: Some(fibo_core::protocol::RequestId::fresh().client),
        },
        REPLY,
    )?)?;
    let list: List = parse(client.request(Op::List, REPLY).map_err(not_running)?)?;
    let (tx, rx) = channel();
    let ends = tx.clone();
    client.stream(
        Op::Events,
        move |body| {
            let _ = match body {
                StreamBody::End { reason } => ends.send(Some(reason)),
                _ => ends.send(None),
            };
        },
        REPLY,
    )?;
    *slot.lock().unwrap() = Some(Linked {
        client: Arc::clone(&client),
        typing: hello.ops.iter().any(|o| o == "input"),
        answering: hello.ops.iter().any(|o| o == "answer"),
        resizing: hello.ops.iter().any(|o| o == "resize"),
        managing: ["open", "end", "outcome"]
            .iter()
            .all(|op| hello.ops.iter().any(|o| o == op)),
        adding_spaces: ["dirs", "addspace"]
            .iter()
            .all(|op| hello.ops.iter().any(|o| o == op)),
    });
    apply(view, Some(hello.machine), list);
    changed();
    let mut listed = std::time::Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Some(reason)) => return Err(io::Error::other(reason)),
            Ok(None) => {
                // Let the burst settle, then one list for all of it.
                let deadline = std::time::Instant::now() + SETTLE;
                while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
                    match rx.recv_timeout(left) {
                        Ok(Some(reason)) => return Err(io::Error::other(reason)),
                        Ok(None) => continue,
                        Err(_) => break,
                    }
                }
                let list: List = parse(client.request(Op::List, REPLY)?)?;
                listed = std::time::Instant::now();
                if apply(view, None, list) {
                    changed();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(why) = client.ended() {
                    return Err(io::Error::other(why));
                }
                // `events` only says when a session changes state; an Agent started in a
                // plain terminal, a new title or a closed terminal is not one. Ask again.
                if listed.elapsed() >= REFRESH {
                    let list: List = parse(client.request(Op::List, REPLY)?)?;
                    listed = std::time::Instant::now();
                    if apply(view, None, list) {
                        changed();
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other(t!("事件流断了")));
            }
        }
    }
}

/// The machine answered but its host is not there to ask: Fibo is not open on it.
fn not_running(e: io::Error) -> io::Error {
    if e.to_string().starts_with("RPC connect") {
        io::Error::other(t!("那台电脑上的 Fibo 没打开"))
    } else {
        e
    }
}

/// Takes in a fresh list; says whether anything the window draws changed.
fn apply(view: &Mutex<View>, machine: Option<Machine>, list: List) -> bool {
    let mut v = view.lock().unwrap();
    let machine = machine.or(Some(list.machine));
    let same = v.link == Link::Online
        && v.machine == machine
        && v.spaces == list.spaces
        && v.terminals == list.terminals;
    v.link = Link::Online;
    v.machine = machine;
    v.spaces = list.spaces;
    v.terminals = list.terminals;
    !same
}

fn parse<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> io::Result<T> {
    serde_json::from_value(v).map_err(|e| io::Error::other(t!("对方的回复读不懂：{e}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc::Receiver;

    struct Fake {
        /// Terminals the next `list` returns.
        terminals: Arc<Mutex<Vec<Value>>>,
        /// Pushes an event line into the open events stream.
        events: Arc<Mutex<Option<(u64, fibo_platform::local::Stream)>>>,
        /// The far end of every connection made; shutting one down drops that link.
        links: Arc<Mutex<Vec<fibo_platform::local::Stream>>>,
    }

    fn card(id: &str, status: &str) -> Value {
        json!({ "id": id, "space": "s", "tab": "t", "cwd": "/w", "phase": "working",
            "agent": "claude", "session": null, "title": id, "approval": null,
            "waiting": null, "status": status, "status_label": status, "needs_you": false })
    }

    fn fake() -> (Fake, impl Fn(&Spec) -> io::Result<Client>) {
        let terminals = Arc::new(Mutex::new(vec![card("a", "running")]));
        let events = Arc::new(Mutex::new(None));
        let links = Arc::new(Mutex::new(Vec::new()));
        let fake = Fake {
            terminals: Arc::clone(&terminals),
            events: Arc::clone(&events),
            links: Arc::clone(&links),
        };
        let connect = move |_: &Spec| {
            let (ours, theirs) = fibo_platform::local::Stream::pair()?;
            links.lock().unwrap().push(theirs.try_clone()?);
            let (terminals, events) = (Arc::clone(&terminals), Arc::clone(&events));
            let mut out = theirs.try_clone()?;
            std::thread::spawn(move || {
                for line in BufReader::new(theirs).lines().map_while(Result::ok) {
                    let r: Value = serde_json::from_str(&line).unwrap();
                    let id = r["id"].as_u64().unwrap();
                    let ok = match r["op"].as_str().unwrap() {
                        "hello" => json!({ "version": 1, "machine": { "id": "m2", "name": "mac2" },
                            "ops": ["list", "events", "watch"], "role": "full" }),
                        "list" => json!({ "machine": { "id": "m2", "name": "mac2" }, "spaces": [],
                            "terminals": terminals.lock().unwrap().clone() }),
                        "events" => {
                            *events.lock().unwrap() = Some((id, out.try_clone().unwrap()));
                            json!({ "stream": id })
                        }
                        _ => json!({}),
                    };
                    writeln!(out, "{}", json!({ "id": id, "ok": ok })).unwrap();
                }
            });
            Ok(Client::new(ours.try_clone()?, ours))
        };
        (fake, connect)
    }

    fn wait(rx: &Receiver<()>, view: &Mutex<View>, until: impl Fn(&View) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !until(&view.lock().unwrap()) {
            assert!(
                std::time::Instant::now() < deadline,
                "{:?}",
                view.lock().unwrap()
            );
            let _ = rx.recv_timeout(Duration::from_millis(100));
        }
    }

    #[test]
    fn lists_the_machine_and_follows_its_events() {
        let (fake, connect) = fake();
        let (tx, rx) = channel();
        let spec = Spec {
            host: "mac2".into(),
            path: None,
            quiet: false,
        };
        let r = Remote::start(spec, connect, move || {
            let _ = tx.send(());
        });
        wait(&rx, &r.view, |v| v.link == Link::Online);
        assert_eq!(
            r.view.lock().unwrap().machine.as_ref().unwrap().name,
            "mac2"
        );
        assert_eq!(r.view.lock().unwrap().terminals[0].id, "a");
        fake.terminals.lock().unwrap().push(card("b", "waiting"));
        let (stream, mut out) = fake.events.lock().unwrap().take().unwrap();
        writeln!(
            out,
            "{}",
            json!({ "stream": stream, "type": "event", "event": { "event": "waiting", "terminal": "b" } })
        )
        .unwrap();
        wait(&rx, &r.view, |v| v.terminals.len() == 2);
        assert_eq!(
            r.view.lock().unwrap().terminals[1].status.as_deref(),
            Some("waiting")
        );
    }

    /// Step 1's acceptance: a dropped link shows 断开 and comes back by itself within 5
    /// seconds, keeping the last cards meanwhile (the board fades them).
    #[test]
    fn a_dropped_link_comes_back_within_five_seconds() {
        let (fake, connect) = fake();
        let (tx, rx) = channel();
        let spec = Spec {
            host: "mac2".into(),
            path: None,
            quiet: false,
        };
        let r = Remote::start(spec, connect, move || {
            let _ = tx.send(());
        });
        wait(&rx, &r.view, |v| v.link == Link::Online);
        assert!(r.slot().lock().unwrap().is_some());
        for link in fake.links.lock().unwrap().drain(..) {
            link.shutdown(std::net::Shutdown::Both).unwrap();
        }
        wait(&rx, &r.view, |v| matches!(v.link, Link::Offline(_)));
        assert_eq!(
            r.view.lock().unwrap().terminals.len(),
            1,
            "cards kept while away"
        );
        assert!(
            r.slot().lock().unwrap().is_none(),
            "nothing writes over a dead link"
        );
        let dropped = std::time::Instant::now();
        wait(&rx, &r.view, |v| v.link == Link::Online);
        assert!(
            dropped.elapsed() < Duration::from_secs(5),
            "{:?}",
            dropped.elapsed()
        );
        assert!(r.slot().lock().unwrap().is_some());
    }

    /// An Agent started in a plain terminal changes no session's state, so no event says so.
    #[test]
    fn a_change_without_an_event_still_shows_up() {
        let (fake, connect) = fake();
        let (tx, rx) = channel();
        let spec = Spec {
            host: "mac2".into(),
            path: None,
            quiet: false,
        };
        let r = Remote::start(spec, connect, move || {
            let _ = tx.send(());
        });
        wait(&rx, &r.view, |v| v.link == Link::Online);
        fake.terminals.lock().unwrap().push(card("b", "review"));
        wait(&rx, &r.view, |v| v.terminals.len() == 2);
    }

    #[test]
    fn a_failed_connection_shows_why_and_retries() {
        let (tx, rx) = channel();
        let tries = Arc::new(Mutex::new(0));
        let counted = Arc::clone(&tries);
        let spec = Spec {
            host: "mac2".into(),
            path: None,
            quiet: false,
        };
        let r = Remote::start(
            spec,
            move |_| {
                *counted.lock().unwrap() += 1;
                Err(io::Error::other("ssh: Could not resolve hostname mac2"))
            },
            move || {
                let _ = tx.send(());
            },
        );
        wait(
            &rx,
            &r.view,
            |v| matches!(&v.link, Link::Offline(w) if w.contains("resolve")),
        );
        wait(&rx, &r.view, |_| *tries.lock().unwrap() >= 2);
        drop(r);
    }

    #[test]
    fn the_command_runs_mooring_through_ssh() {
        let named = Spec {
            host: "mac2".into(),
            path: Some("/opt/My Apps/mooring".into()),
            quiet: false,
        };
        let argv = named.command();
        assert_eq!(argv[0], "ssh");
        assert!(argv.contains(&"BatchMode=yes".to_owned()));
        assert_eq!(argv[argv.len() - 2], "mac2");
        assert_eq!(
            argv.last().unwrap(),
            "exec '/opt/My Apps/mooring' remote --stdio"
        );
        let unnamed = Spec {
            host: "mac2".into(),
            path: None,
            quiet: false,
        };
        let probe = unnamed.command().last().unwrap().clone();
        assert!(probe.contains("command -v fibo || command -v mooring"));
        assert!(probe.contains("for a in Fibo Mooring"));
    }

    fn spec(host: &str, quiet: bool) -> Spec {
        Spec {
            quiet,
            ..Spec::new(host.to_owned())
        }
    }
    #[test]
    fn a_file_from_before_quiet_reads_as_attending_and_attending_is_not_written() {
        let old: Vec<Spec> = serde_json::from_str(r#"[{"host":"apple"}]"#).unwrap();
        assert!(!old[0].quiet);
        let text = serde_json::to_string(&old).unwrap();
        assert!(!text.contains("quiet"), "{text}");
        let text = serde_json::to_string(&[spec("apple", true)]).unwrap();
        let back: Vec<Spec> = serde_json::from_str(&text).unwrap();
        assert!(back[0].quiet, "先不管 did not survive a save: {text}");
    }
    #[test]
    fn quiet_goes_both_ways_and_touches_one_machine() {
        let specs = vec![spec("apple", false), spec("pear", false)];
        let quiet = with_quiet(specs, "apple", true).unwrap();
        assert!(quiet[0].quiet && !quiet[1].quiet);
        let back = with_quiet(quiet, "apple", false).unwrap();
        assert!(!back[0].quiet && !back[1].quiet);
    }
    #[test]
    fn an_unreadable_file_is_never_written_back_empty() {
        assert_eq!(with_quiet(Vec::new(), "apple", true), None);
    }
    #[test]
    fn settings_adding_a_machine_does_not_undo_quiet() {
        let stale = vec![spec("apple", false), spec("pear", false)];
        let saved = [spec("apple", true)];
        let next = keep_quiet(stale, &saved);
        assert!(next[0].quiet, "the list settings held undid 先不管");
        assert!(!next[1].quiet);
    }
    #[test]
    fn invalid_machine_config_is_not_overwritten_by_settings() {
        let root = std::env::temp_dir().join(format!(
            "fibo-machines-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("machines.json");
        let invalid = br#"[{"host":"apple"}, broken]"#;
        std::fs::write(&file, invalid).unwrap();
        let next = vec![spec("new-machine", false)];
        assert_eq!(
            load_from(&file).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(save_keeping_quiet_to(&file, &next).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), invalid);
        // An unreadable path is not an initial empty configuration either.
        assert!(load_from(&root).is_err());
        assert!(save_keeping_quiet_to(&root, &next).is_err());
        assert!(root.is_dir());
        // A genuinely absent file remains a supported first-run case.
        let missing = root.join("new/machines.json");
        assert!(load_from(&missing).unwrap().is_empty());
        save_keeping_quiet_to(&missing, &next).unwrap();
        assert_eq!(load_from(&missing).unwrap(), next);
        std::fs::remove_dir_all(root).unwrap();
    }
}
