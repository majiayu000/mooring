use crate::model::{Edit, Workspace};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const MAJOR: u32 = 3;
pub const MAX_FRAME: usize = 16 * 1024 * 1024;
pub const CONTROL: u8 = 1;
pub const SNAPSHOT: u8 = 2;
pub const READY: u8 = 3;
pub const OUTPUT: u8 = 4;
pub const RESIZED: u8 = 5;
pub const STATUS: u8 = 6;
pub const ACK: u8 = 7;
pub const WRITABLE: u8 = 8;

/// The few hook payload facts the status rules read. Prompt and reply text stay out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookFacts {
    /// From a Stop or SubagentStop payload that carried the task registry.
    pub background: Option<crate::content::Background>,
    pub notification: Option<String>,
    /// The event came from inside a subagent (`agent_id` present), not the parent turn.
    pub subagent: bool,
    /// The running background shells a Stop or SubagentStop listed. Their commands only
    /// ride to the host, which matches them to processes; the stored card keeps the
    /// descriptions alone, since a command line may hold a secret.
    #[serde(default)]
    pub shells: Vec<HookShell>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookShell {
    pub id: String,
    pub description: String,
    pub command: String,
}
/// An Agent asking to be trusted with a folder before it does anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustPrompt {
    pub agent: crate::agent::Agent,
    /// Its words.
    pub question: String,
    /// The folder as its screen writes it.
    pub folder: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Hello {
        major: u32,
    },
    /// From a newer host: hand over every running terminal, then exit.
    Handoff,
    Workspace,
    ClientLeft {
        at: u64,
    },
    ClaimNotification {
        session: String,
        key: String,
    },
    RefreshAgents,
    /// Plugins were installed, removed or switched: read them again. The host swaps its
    /// rules and hooks; agents stay as registered at its start.
    ReloadPlugins,
    RecognitionStatus,
    WatchRecognition,
    /// A fifteen-second measurement stream, cancelled by disconnecting this socket.
    Diagnose,
    Hook {
        runtime: crate::model::Agent,
        pid: u32,
        cli_session_id: String,
        transcript: String,
        ended: bool,
        event: Option<String>,
        #[serde(default)]
        facts: HookFacts,
        /// The terminal the hook's environment names (`TERMINAL_ENV`); absent outside one.
        #[serde(default)]
        terminal: Option<String>,
        /// The working directory the hook reports; stands in until the transcript exists.
        #[serde(default)]
        cwd: String,
    },
    /// A change to the workspace. With `request` it takes effect once however often it
    /// is sent: a resend gets the first outcome back (the workspace if it was applied,
    /// the same error if it was refused), across disconnects and host restarts.
    Edit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<RequestId>,
        edit: Edit,
    },
    Launch {
        id: String,
        space: String,
        argv: Vec<String>,
        cwd: String,
        cols: u16,
        rows: u16,
        /// Hosts before it start every terminal dark.
        #[serde(default)]
        scheme: crate::terminal::Scheme,
        /// A user's theme; hosts before it read `scheme`, the nearest built-in.
        #[serde(default)]
        colors: Option<crate::terminal::TermColors>,
    },
    Attach {
        id: String,
        takeover: bool,
        /// Only looks: never becomes the writer and never sizes the terminal, however
        /// many writers come and go (`mooring remote`'s watch). Hosts before it ignore it.
        #[serde(default)]
        watch: bool,
    },
    Input {
        bytes: Vec<u8>,
        /// Mouse reports: dropped while the program is behind on reading, so a full
        /// input queue never cuts one in half. Hosts before it write them all.
        #[serde(default)]
        lossy: bool,
    },
    Resize {
        cols: u16,
        rows: u16,
        /// One cell in device pixels, for size reports and the pty's pixel size; 0 if unknown.
        #[serde(default)]
        cell_width: u32,
        #[serde(default)]
        cell_height: u32,
    },
    /// The window switched between light and dark: repaint this terminal's defaults.
    Scheme(crate::terminal::Scheme),
    /// The same for a user's theme. Sent only to a host of this build: one before it
    /// would drop the connection on a request it cannot read.
    Colors(crate::terminal::TermColors),
    /// Cmd+K: drop scrollback and the rows above the cursor (no-op in the alternate screen).
    ClearHistory,
    TerminalCwd {
        id: String,
    },
    StopAgent {
        id: String,
    },
    Stop {
        id: String,
    },
    /// Reads the next stretch of a conversation's log that its first read skipped, on a thread
    /// and a connection of its own at low priority, so a long log holds nothing else up.
    Backfill {
        session: String,
    },
    /// A phone's chat view: one conversation's user and Agent messages from the index,
    /// oldest first. `after` gives those newer than a `seq`; otherwise the newest `limit`,
    /// or those older than `before`.
    Chat {
        session: String,
        #[serde(default)]
        after: Option<i64>,
        #[serde(default)]
        before: Option<i64>,
        limit: usize,
        /// Each Agent message also carries its [`ChatTools`].
        #[serde(default)]
        tools: bool,
    },
    /// A phone opening a stretch of work: the tool calls logged before the Agent message
    /// `seq` and after the message before it (no `seq`: after the last message), oldest
    /// first, from past the `after` cursor. Rows only: their raw lines are the caller's to
    /// read, without the host's lock.
    ToolCalls {
        session: String,
        #[serde(default)]
        seq: Option<i64>,
        #[serde(default)]
        after: Option<i64>,
        limit: usize,
    },
    /// One tool call by its id, and where its raw lines are.
    ToolCall {
        session: String,
        call: String,
    },
    /// The files this session's current turn changed, read from its log when asked.
    TurnChanges {
        session: String,
    },
    /// What the turn did to one of them, read from the places `TurnChanges` named.
    TurnFile {
        session: String,
        log: u64,
        path: String,
        lines: Vec<u64>,
    },
    /// Scripts and other Agents: the last `rows` rows of a terminal as plain text.
    ReadScreen {
        id: String,
        rows: usize,
    },
    /// Which conversation an unbound Agent's screen shows, tied to it when beyond doubt:
    /// [`Reply::ScreenAssociated`].
    AssociateScreen {
        id: String,
    },
    /// What the Agent in a terminal offers as your next prompt: [`Reply::InputSuggestion`].
    InputSuggestion {
        id: String,
    },
    /// Scripts and other Agents: text placed in a terminal as a paste, then Enter when
    /// `submit`. Goes in beside whoever is typing there.
    SendText {
        /// Restricts a chat message to this current conversation; None permits shell input.
        #[serde(default)]
        session: Option<String>,
        id: String,
        text: String,
        submit: bool,
    },
    /// `mooring run`: each time the Agent in `child` ends a turn (done, a question, an
    /// error), its last reply goes into `parent`'s Agent input, wrapped as
    /// `<mooring from=…>`, once that Agent is idle. Ends when either terminal exits.
    ReportTo {
        child: String,
        parent: String,
    },
    /// The workbench's 允许一次 / 拒绝 on an approval the screen shows. The host reads
    /// the screen again and answers only while that same approval is still there.
    AnswerApproval {
        id: String,
        question: String,
        allow: bool,
    },
    /// As `AnswerApproval` allowing, but takes the menu's 「don't ask again」 option
    /// (this session, this command); refused when the menu shows none.
    AnswerAlways {
        id: String,
        question: String,
    },
    /// Answers an Agent's 「trust this folder?」, read off the screen before and after the
    /// keys go out; refused when that prompt about `folder` is not what it shows.
    AnswerTrust {
        id: String,
        folder: String,
        trust: bool,
    },
    /// Interrupts the turn of the conversation in the terminal, with the key its CLI takes
    /// for that (`Adapter::interrupt_key`), only while it is working: what its turn was at
    /// that moment decides, not what a client saw earlier. The window's 中断这一轮 and a
    /// phone's 停下 are both this and [`Request::AwaitInterrupt`].
    Interrupt {
        id: String,
    },
    /// After [`Request::Interrupt`]: replies [`Reply::Interrupted`] once the state says the
    /// turn ended, with the Agent still there, and closes a command the turn left running.
    /// Refused if it does not end within seconds or the Agent left. A key sent is not a turn
    /// stopped. Outside `Once`: its one lock would hold every other answer up meanwhile.
    AwaitInterrupt {
        id: String,
    },
    Modes(crate::modes::ModeRequest),
    /// Typing, keys or an approval answer (`SendText`, `SendKeys`, `SendInput`,
    /// `AnswerApproval`, `AnswerAlways`, `AnswerTrust`, `Interrupt`) run
    /// at most once under `request`: a resend after a lost reply gets the first outcome
    /// back (a phone or another computer that reconnected). Edits name themselves.
    Once {
        request: RequestId,
        inner: Box<Request>,
    },
    /// Named keys (`enter`, `esc`, `up`, `ctrl-c`, a single character…) typed into a
    /// terminal in order, beside whoever is typing there; arrows follow the program's
    /// cursor-key mode. For clients with no keyboard of their own, such as a phone.
    SendKeys {
        id: String,
        keys: Vec<String>,
    },
    /// Bytes as a terminal emulator encodes keys and pastes, typed into a terminal beside
    /// whoever types there: another desktop's pane on this machine (`mooring remote`).
    SendInput {
        id: String,
        bytes: Vec<u8>,
    },
    /// Scripts, other Agents and other clients: the sessions' states now, then every
    /// change as it happens, until this socket closes (`mooring events`).
    Subscribe,
    /// `mooring split` asks before it starts anything: `Size`, the new pane's cells,
    /// when the pane, at the size it is shown now, has room for both halves of that split.
    SplitRoom {
        target: String,
        axis: crate::layout::Axis,
        share: u16,
    },
    /// Turns phones on a relay on or off, or moves them to another relay. The host keeps
    /// `mooring remote --relay` running while it is on.
    RemoteConfigure {
        enabled: bool,
        relay: String,
        /// The relay's registration key: None keeps the one saved, empty clears it.
        key: Option<String>,
    },
    /// Pairs one more phone; replies `Text`, its pairing link.
    RemotePair {
        name: String,
        /// view, answer or full.
        role: String,
    },
    RemoteDevice {
        device: String,
        change: RemoteChange,
    },
    /// From `mooring remote --relay`: who is on now; the host passes it to the app.
    RemoteReport(RemoteLive),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteChange {
    /// view, answer or full; its open sessions end and it reconnects with the new one.
    Role(String),
    /// Ends its open sessions now; it stays paired and may connect again.
    Kick,
    Revoke,
    /// Changed outside the host (`mooring remote --revoke`): show the app again.
    Refresh,
}

/// Phones on a relay, as the app shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteStatus {
    pub enabled: bool,
    pub relay: String,
    /// A registration key is saved; the key itself stays with the host.
    #[serde(default)]
    pub keyed: bool,
    #[serde(default)]
    pub devices: Vec<RemoteDevice>,
    #[serde(flatten)]
    pub live: RemoteLive,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLive {
    pub state: RemoteState,
    /// Why it is not connected, in the user's words; empty when it is.
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub online: Vec<RemotePhone>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RemoteState {
    #[default]
    Off,
    Connecting,
    Connected,
    /// Stopped and not retrying, e.g. the relay refused this machine; `detail` says why.
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDevice {
    pub id: String,
    pub name: String,
    pub role: String,
    /// Unix seconds.
    pub paired: u64,
    pub last_seen: Option<u64>,
}

/// One phone connected now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePhone {
    pub device: String,
    /// Unix milliseconds.
    pub since: u64,
    /// Terminals it watches.
    #[serde(default)]
    pub watching: Vec<String>,
    pub last: Option<RemoteAct>,
}

/// The last thing a phone did: send, keys or answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAct {
    pub op: String,
    pub terminal: Option<String>,
    /// Unix milliseconds.
    pub at: u64,
}

/// Names one write: `client` stays the same for the sender's life, `id` is unique
/// within it. The host remembers each outcome for a day (`REQUEST_KEPT_SECS`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId {
    pub client: String,
    pub id: String,
}
pub const REQUEST_KEPT_SECS: u64 = 24 * 60 * 60;
impl RequestId {
    /// A new id from this process: one client for the process's life, ids in order.
    pub fn fresh() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static CLIENT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            client: CLIENT
                .get_or_init(|| uuid::Uuid::new_v4().to_string())
                .clone(),
            id: NEXT.fetch_add(1, Ordering::Relaxed).to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Reply {
    Hello {
        major: u32,
        instance: String,
        /// Absent from hosts built before it existed, which are stale by definition.
        #[serde(default)]
        build: Build,
    },
    Workspace(Workspace),
    Cwd(String),
    NotificationClaimed(bool),
    Recognition(RecognitionStatus),
    Diagnostics(crate::diagnostics::Diagnostics),
    Attached {
        writable: bool,
        cols: u16,
        rows: u16,
    },
    /// The terminals that follow, each as a snapshot (SNAPSHOT frames, then READY), and
    /// then their PTY descriptors in this order in one SCM_RIGHTS message.
    Handoff {
        terminals: Vec<Handed>,
    },
    Ok,
    Text(String),
    /// [`Request::AssociateScreen`]: the conversation that terminal now holds; None when
    /// the screen did not say which.
    ScreenAssociated {
        session: Option<String>,
    },
    Modes(crate::modes::ModeReply),
    /// [`Request::InputSuggestion`]: the conversation in that terminal, and what it offers.
    InputSuggestion {
        session: Option<String>,
        text: Option<String>,
    },
    /// [`Request::AwaitInterrupt`]: the turn ended; whether a command it left was closed too.
    Interrupted {
        closed_background: bool,
    },
    /// `Chat`: the messages, oldest first, and whether older ones exist before them.
    /// `partial`: older lines of its log have not been read in yet ([`Request::Backfill`]).
    Chat {
        messages: Vec<ChatMessage>,
        /// A byte-offset cursor is valid only within this history generation.
        generation: String,
        more: bool,
        #[serde(default)]
        partial: bool,
    },
    /// [`Request::Backfill`]: messages found; `done` once the log is read back to its start.
    Backfilled {
        added: u32,
        done: bool,
    },
    /// `ToolCalls`; `tools` on the stretch's first page, `next` the cursor while more follow.
    ToolCalls {
        log: ToolLog,
        tools: Option<ChatTools>,
        rows: Vec<ToolRow>,
        next: Option<i64>,
    },
    ToolCall {
        log: ToolLog,
        row: ToolRow,
    },
    TurnChanges(crate::turn_diff::TurnChanges),
    TurnFile(crate::turn_diff::TurnFile),
    /// A terminal size in cells: what a split's new pane starts at (`SplitRoom`).
    Size {
        cols: u16,
        rows: u16,
    },
    /// A `Subscribe` stream's frames: first the state now, then each batch of changes.
    Events(Vec<crate::hook::Notice>),
    Error(String),
}

/// One message of a conversation as the index recorded it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Grows with each message recorded; a cursor for `after` and `before`. A reindexed
    /// log gets new ones, so a client that sees a gap reloads the newest.
    pub seq: i64,
    /// Seconds since the epoch.
    pub at: u64,
    /// The user's words; otherwise the Agent's.
    pub user: bool,
    pub text: String,
    /// Asked for with `Chat`'s `tools`, on the Agent's messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ChatTools>,
}

/// The tool calls logged between an Agent message and the message before it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatTools {
    pub commands: u32,
    /// `(path, added, removed)`, in the order first touched.
    pub files: Vec<(String, u32, u32)>,
    /// Seconds this stretch of work took: from your message, or from its first call when
    /// the message before was the Agent's own (it may have slept since). 0 when unknown.
    #[serde(default)]
    pub took: u64,
    /// Every tool call in it, whatever kind: a stretch of reading has no command or edit.
    #[serde(default)]
    pub calls: u32,
}

pub use crate::tool_rows::{ToolLog, ToolResultRow, ToolRow};

/// A running terminal passed from one host to the next.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handed {
    pub id: String,
    pub pid: u32,
    pub cols: u16,
    pub rows: u16,
    pub title: String,
    pub cell: (u32, u32),
}
/// The program a host runs, fixed when it started: a rebuilt binary does not change a
/// running host, so a client compares this with the host it would start itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    pub binary: String,
    /// Content digest of `binary` when the host started; empty from hosts before it.
    #[serde(default)]
    pub digest: String,
    /// The host's process id, to stop it for a restart; 0 from hosts before it.
    #[serde(default)]
    pub pid: u32,
    /// The host can hand its running terminals to a newer one, so updating it ends none.
    #[serde(default)]
    pub handoff: bool,
}
impl Build {
    pub fn current() -> Self {
        let binary = std::env::current_exe().unwrap_or_default();
        Self {
            digest: Self::digest(&binary).unwrap_or_default(),
            binary: binary.to_string_lossy().into_owned(),
            pid: std::process::id(),
            handoff: true,
        }
    }
    /// Identifies a file by its bytes (FNV-1a and length), so a re-signed or copied
    /// binary with the same code is the same build whatever its timestamps say.
    pub fn digest(path: &std::path::Path) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        });
        Some(format!("{hash:016x}-{}", bytes.len()))
    }
    /// Whether this host runs other code than `sibling`, the host this client would
    /// start. A host that reports no digest cannot show it is the same.
    pub fn stale(&self, sibling: &std::path::Path) -> bool {
        self.digest.is_empty() || Self::digest(sibling).is_some_and(|d| d != self.digest)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecognitionPhase {
    Starting,
    Healthy,
    Interrupted,
    Recovering,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionMetrics {
    pub ps_spawns: u64,
    pub lsof_spawns: u64,
    pub directories: u64,
    pub log_opens: u64,
    pub log_bytes: u64,
    pub index_writes: u64,
}
/// Set in every managed terminal's environment, so whatever an Agent runs there (its
/// hooks above all) can say which terminal it is in and reach this host. A hook command
/// installed once for good stays a no-op outside Mooring, where these are unset.
pub const TERMINAL_ENV: &str = "MOORING_TERMINAL";
pub const RUNTIME_ENV: &str = "MOORING_RUNTIME";
/// The host executable: hooks run `"$MOORING_HOOK" hook --agent <cli>`.
pub const HOOK_ENV: &str = "MOORING_HOOK";
/// `1` in every managed terminal: what `mooring --skill` tells an Agent to check first.
pub const MARKER_ENV: &str = "MOORING_ENV";
/// Set by the user before a wrapper (a sandbox, a VM, `nono`) that hides the Agent's own
/// process: names the Agent it runs.
pub const AGENT_ENV: &str = "MOORING_AGENT";
/// The terminal hint for an Agent the host sees running but cannot tie to a conversation yet.
pub const AGENT_UNBOUND_PREFIX: &str = "未对上对话：";
/// The terminal hint for a Claude Code that saves no transcript: it inherited the nested
/// session marker, so Mooring can never read or count it.
pub const CLAUDE_TRANSCRIPT_OFF: &str = "Claude 没有保存对话记录（继承了 CLAUDE_CODE_CHILD_SESSION），无法统计；关掉这个终端重开后再运行 claude";
/// A short note for a terminal's pane, and why, for an Agent seen there but not read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalNote {
    pub note: String,
    pub why: String,
}
/// What the program in a terminal says about itself through OSC 7501, the Program Status
/// Protocol: its own words, kept as reported. A program that says nothing has none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramReport {
    /// idle, working, done, blocked or error.
    pub state: String,
    /// permission, question or auth, when a blocked program said which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// 0..=100, when the program said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// Who reported, e.g. `claude-code` or a script's name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub msg: String,
    /// When the state word last changed, seconds since the epoch.
    #[serde(default)]
    pub since: u64,
}
/// A command the shell marked finished (OSC 133 `C` then `D`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LastCommand {
    /// None when the shell did not say.
    pub exit: Option<i32>,
    pub took_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecognitionStatus {
    pub phase: RecognitionPhase,
    pub detail: String,
    pub scopes: Vec<String>,
    pub metrics: RecognitionMetrics,
    pub terminal_hints: std::collections::HashMap<String, String>,
    /// Terminal -> what its Agent's adapter says about it, in the user's words. Kept beside
    /// `terminal_hints` so the app shows any CLI's note without knowing which CLI it is.
    #[serde(default)]
    pub terminal_notes: std::collections::HashMap<String, TerminalNote>,
    /// Terminal -> the recognized unbound Agent, even before any transcript or note.
    #[serde(default)]
    pub terminal_agents: std::collections::HashMap<String, crate::agent::Agent>,
    /// Terminal -> the question an Agent asks before anything else: whether to trust the
    /// folder. Read off its screen; there may be no conversation yet. Live state, not stored.
    #[serde(default)]
    pub terminal_trust: std::collections::HashMap<String, TrustPrompt>,
    /// Terminal -> the title its program set, spinner dropped. Live state, not stored.
    #[serde(default)]
    pub terminal_titles: std::collections::HashMap<String, String>,
    /// Terminal -> the last command its shell marked finished. Live state, not stored.
    #[serde(default)]
    pub terminal_commands: std::collections::HashMap<String, LastCommand>,
    /// Terminal -> what its program last reported through OSC 7501, until the program
    /// ends or clears it. Live state, not stored.
    #[serde(default)]
    pub terminal_reports: std::collections::HashMap<String, ProgramReport>,
    #[serde(default)]
    pub terminal_modes: crate::modes::Modes,
    pub workspace_revision: u64,
    /// Bumped whenever the host reloads plugins: a `mooring plugins` in a terminal changed
    /// them, or a window did, and every window has to read them again.
    #[serde(default)]
    pub plugins_revision: u64,
    /// CLI -> why its official status query failed last; absent once one succeeds. The
    /// status rules then fall back to logs, hooks and the screen.
    #[serde(default)]
    pub official: std::collections::BTreeMap<String, String>,
    /// Phones on a relay. Live state, not stored.
    #[serde(default)]
    pub remote: RemoteStatus,
}
impl Default for RecognitionStatus {
    fn default() -> Self {
        Self {
            phase: RecognitionPhase::Starting,
            detail: "正在建立身份索引".into(),
            scopes: vec![],
            metrics: RecognitionMetrics::default(),
            terminal_hints: Default::default(),
            terminal_notes: Default::default(),
            terminal_trust: Default::default(),
            terminal_agents: Default::default(),
            terminal_titles: Default::default(),
            terminal_commands: Default::default(),
            terminal_reports: Default::default(),
            terminal_modes: Default::default(),
            workspace_revision: 0,
            plugins_revision: 0,
            official: Default::default(),
            remote: Default::default(),
        }
    }
}

pub fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    if payload.len() >= MAX_FRAME {
        return Err(io::Error::other("frame too large"));
    }
    w.write_all(&((payload.len() + 1) as u32).to_be_bytes())?;
    w.write_all(&[kind])?;
    w.write_all(payload)
}
pub fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut size = [0; 4];
    r.read_exact(&mut size)?;
    let size = u32::from_be_bytes(size) as usize;
    if size == 0 || size > MAX_FRAME {
        return Err(io::Error::other("invalid frame size"));
    }
    let mut bytes = vec![0; size];
    r.read_exact(&mut bytes)?;
    Ok((bytes[0], bytes[1..].to_vec()))
}
/// Serialize one queued user input as ordered bounded packets. UTF-8 and bracketed
/// paste delimiters may span packets: the PTY remains a byte stream. Never retry.
pub fn send_input(w: &mut impl Write, bytes: &[u8], lossy: bool) -> io::Result<()> {
    for chunk in bytes.chunks(16 * 1024) {
        send(
            w,
            CONTROL,
            &Request::Input {
                bytes: chunk.to_vec(),
                lossy,
            },
        )?;
    }
    Ok(())
}

pub fn send<T: Serialize>(w: &mut impl Write, kind: u8, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    write_frame(w, kind, &bytes)
}
pub fn receive<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> io::Result<T> {
    let (kind, bytes) = read_frame(r)?;
    if kind != CONTROL {
        return Err(io::Error::other("expected control frame"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

/// One try, without resending: `delivery::deliver` waits out a dropped link.
pub fn rpc(socket: &std::path::Path, request: &Request) -> io::Result<Reply> {
    crate::delivery::attempt(socket, request).into_result()
}

#[cfg(test)]
mod tests {
    use super::Build;

    #[test]
    fn a_host_is_stale_only_when_the_code_differs() {
        let dir = std::env::temp_dir().join(format!("kl-build-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("mooring");
        std::fs::write(&binary, "v1").unwrap();
        let started = Build {
            binary: binary.to_string_lossy().into_owned(),
            digest: Build::digest(&binary).unwrap(),
            pid: 1,
            handoff: true,
        };
        assert!(!started.stale(&dir.join("absent")));
        // A copy with other timestamps (re-signed, touched) is the same build.
        let copy = dir.join("bundled");
        std::fs::write(&copy, "v1").unwrap();
        assert!(!started.stale(&copy));
        std::fs::write(&copy, "v2").unwrap();
        assert!(started.stale(&copy));
        assert!(Build::default().stale(&binary), "hosts before digests");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
