//! Per-CLI knowledge. Everything else in the daemon is CLI-agnostic and asks here: where a
//! CLI keeps its transcripts, how a transcript names its conversation, how its records map
//! onto a card, how a live process names the conversation it runs, and what to do with
//! one no transcript has claimed yet. Supporting another CLI is one `AgentSpec` (start and
//! resume commands) plus one `Adapter` here, or a declaration file and no code at all
//! (`declared`). See docs/architecture/PLUGIN_SYSTEM.md.
use crate::content::{Official, Parser};
use mooring_core::protocol::TerminalNote;
use mooring_core::{content::Source, model::Agent, model::AgentSession};
use serde_json::Value;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

pub(crate) mod claude;
mod claude_suggestion;
pub(crate) mod codex;
mod codex_background;
mod codex_items;
mod codex_script;
mod cursor;
mod declared;
mod droid;
mod dsh;
mod dsh_store;
mod grok;
mod grok_subagents;
mod hermes;
mod hermes_store;
pub mod hooks;
pub mod modes;
pub mod modes_drive;
pub use tool_lines::{describe_tool, Described, ToolCallLine, ToolOutput};
pub mod mirror;
mod permission;
mod pi;
mod qoder;
mod tool_lines;
mod trust;
mod turn_edits;
mod usage_lines;

/// What the host says on a terminal about an Agent it sees but cannot read: `text` is the
/// recognition hint, `note` the pane's words for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hint {
    pub text: String,
    pub note: TerminalNote,
}

/// What the host does with a recognised process no session has claimed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    /// Its transcript will name it.
    Wait,
    /// It writes no transcript: say so on its terminal and only watch its exit.
    Unrecorded(Hint),
    /// Its conversation may run outside the PTY, so only screen evidence binds a log to it
    /// (`outside_pty`). With `title_status`, the terminal title paints its state meanwhile.
    Unbound { title_status: bool },
    /// Unbound like `Unbound { title_status: false }`, with nothing to say on its terminal
    /// yet (`--no-daemon` Codex before its first prompt).
    Quiet,
}

/// Any CLI's live process no session has claimed: unbound, so screen association and hooks
/// get their chance, unless a child of the same CLI is the one that will own the session.
pub fn unclaimed(
    process: &crate::recognition::Process,
    processes: &[crate::recognition::Process],
) -> Pending {
    if processes
        .iter()
        .any(|p| p.parent == process.pid && p.runtime == process.runtime)
    {
        Pending::Wait
    } else {
        Pending::Unbound {
            title_status: false,
        }
    }
}

/// The terminal's note for a recognised Agent whose command line names no conversation.
pub fn unbound_hint(agent: Agent) -> Hint {
    let cli = agent.executable();
    Hint {
        text: format!("{}{cli}", mooring_core::protocol::AGENT_UNBOUND_PREFIX),
        note: TerminalNote {
            note: format!("{cli} · 发出第一句后对上对话"),
            why: format!(
                "Mooring 看到这个 {cli} 在跑，但它的命令行没写是哪段对话。发出第一句话后，按画面上的提示词对上；想启动就认出，运行一次 mooring hooks install 装上 Hook。"
            ),
        },
    }
}

/// The CLI's own account of its sessions: what was asked, and per entry of `due` it
/// answered for, that session's status.
pub type OfficialAnswer = (&'static str, io::Result<Vec<(usize, Official)>>);

pub trait Adapter: Sync {
    /// The directory this CLI writes transcripts under, honouring its own home override.
    fn root(&self, home: &Path) -> PathBuf;
    /// Bytes every line that reports tokens holds, to skip the rest of a long log unparsed;
    /// `None` when this CLI's log does not report them.
    fn usage_marker(&self) -> Option<&'static [u8]> {
        None
    }
    /// `(new tokens, cache reads)` a line says were used since `since`.
    fn usage_line(&self, _line: &[u8], _since: u64, _state: &mut UsageState) -> Option<(u64, u64)> {
        None
    }
    /// The key that interrupts its running turn and keeps it alive. Ctrl-C does on most
    /// CLIs; on one where it does something else this says which key does.
    fn interrupt_key(&self) -> &'static [u8] {
        b"\x03"
    }
    /// How it takes answers over a local API, for one that has it.
    fn answers(&self) -> Option<&'static mooring_core::agent_answers::Answers> {
        None
    }
    /// How many times to press it, for a CLI that asks to press again.
    fn interrupt_presses(&self) -> u32 {
        1
    }
    /// Its 「trust this folder?」 prompt, from a screen's last filled lines.
    fn trust_screen(&self, _lines: &[&str]) -> Option<crate::screen_ask::TrustScreen> {
        None
    }
    /// What to type when an interrupted turn left its command running on screen, ending it.
    /// None: nothing to do, or this CLI never leaves one.
    fn closes_background(&self, _tail: &str) -> Option<&'static str> {
        None
    }
    /// Whether a phone can read this CLI's tool calls back from its log: the two below.
    fn reads_tools(&self) -> bool {
        false
    }
    /// The call `id` as a log line of its holds it.
    fn tool_call_line(&self, _line: &Value, _id: &str) -> Option<ToolCallLine> {
        None
    }
    /// The output of call `id` as a log line of its holds it.
    fn tool_result_line(&self, _line: &Value, _id: &str) -> Option<ToolOutput> {
        None
    }
    /// Bytes every log line holding a finished file edit has; `None`: its log holds none.
    fn edit_marker(&self) -> Option<&'static [u8]> {
        None
    }
    /// The file edits a log line holds, with their paths.
    fn logged_edits(&self, _line: &Value) -> Vec<(String, mooring_core::turn_diff::Edit)> {
        Vec::new()
    }
    /// Whether a process is this CLI: its executable, or the script an interpreter runs.
    fn runs(&self, executable: &str, script: Option<&str>) -> bool;
    /// The conversation a live process names, and whether it opens its own transcript.
    fn process_session(&self, pid: u32, args: &[String]) -> (Option<String>, bool);
    /// The flags in a live process's argv that resuming its conversation should repeat.
    fn launch_flags(&self, _args: &[String]) -> Vec<String> {
        Vec::new()
    }
    /// A file the CLI rewrites when a process opens or closes a session. A process can
    /// write its transcript before registering, so a change here identifies processes again.
    fn registry(&self, _root: &Path) -> Option<PathBuf> {
        None
    }
    /// The account's usage limits as this CLI last recorded them beside its transcripts
    /// (a status-line snapshot, its own log), asked each time conversation `id`'s
    /// transcript grows. None: it records none outside the transcript.
    fn account(&self, _transcript: &Path, _id: &str) -> Option<mooring_core::quota::Quota> {
        None
    }
    /// Whether a file under the root is a transcript.
    fn is_transcript(&self, path: &Path) -> bool {
        path.extension().is_some_and(|e| e == "jsonl")
    }
    /// Whether a directory under the root can hold transcripts; the walk skips the rest.
    fn descend(&self, _dir: &Path) -> bool {
        true
    }
    /// A record, decoded leniently enough to read metadata from a truncated header line.
    fn header_value(&self, line: &[u8]) -> Option<Value> {
        serde_json::from_slice(line).ok()
    }
    /// The conversation id and working directory a transcript record names.
    fn identity(&self, path: &Path, record: &Value) -> Option<(String, String)>;
    /// Facts kept beside the transcript rather than in it. Asked again until the session is
    /// known to be interactive or hidden.
    fn observe_file(&self, _parser: &mut Parser, _path: &Path) {}
    /// The name the CLI keeps for a transcript's conversation outside the transcript.
    fn name(&self, _root: &Path, _transcript: &Path) -> Option<String> {
        None
    }
    /// Session metadata a record carries: why it is hidden, whether it is interactive.
    fn metadata(&self, parser: &mut Parser, record: &Value);
    /// Whether a record is the user speaking; the bounded first-request search reads these.
    fn is_user(&self, record: &Value) -> bool;
    /// One transcript record onto the card.
    fn add(&self, parser: &mut Parser, record: &Value, at: u64, source: &Source);
    /// Whether subagents keep their own transcripts that name this one as their parent.
    fn links_children(&self) -> bool {
        false
    }
    /// The subagents this CLI keeps on disk beside a transcript, oldest first; `None`
    /// when it keeps none there.
    fn subagents(&self, _transcript: &Path) -> Option<Vec<mooring_core::content::Subagent>> {
        None
    }
    /// The parent transcript whose child list changed when this metadata file moved.
    fn subagent_parent(&self, _metadata: &Path) -> Option<PathBuf> {
        None
    }
    /// A live process of this CLI that no session has claimed yet.
    fn pending(
        &self,
        process: &crate::recognition::Process,
        processes: &[crate::recognition::Process],
    ) -> Pending {
        unclaimed(process, processes)
    }
    /// For a `Pending::Unbound { title_status: true }` process: its state from the title.
    fn title_status(&self, _title: &str) -> Option<Hint> {
        None
    }
    /// Whether the terminal title this CLI sets says a turn is running (`Some(true)`), says
    /// it is not (`Some(false)`), or says nothing either way.
    fn title_busy(&self, _title: &str) -> Option<bool> {
        None
    }
    /// The same from the bottom of the screen, `tail` as `Vt::tail_text` gives it: the
    /// line a CLI ticks while a turn runs ("Working (12s • esc to interrupt)").
    fn input_suggestion(&self, _vt: &mooring_core::terminal::Vt) -> io::Result<Option<String>> {
        Ok(None)
    }
    fn screen_busy(&self, _tail: &str) -> Option<bool> {
        None
    }
    /// Where the commands a turn left running are known from.
    fn background_commands(&self) -> BackgroundCommands {
        BackgroundCommands::Hooks
    }
    /// Whether a conversation can run outside the PTY (a shared daemon), leaving no process
    /// or open-file evidence: then a log binds to a terminal only by what its screen shows,
    /// and logs the daemon keeps open are followed while one of its terminals is unbound.
    fn outside_pty(&self) -> bool {
        false
    }
    /// Asks the CLI itself how `due`, its sessions waiting on a dialog, stand. `None`: it
    /// has no such source. An error falls back to logs and screens, never fails a request.
    fn official(&self, _due: &[&(AgentSession, Option<u32>)]) -> Option<OfficialAnswer> {
        None
    }
    /// For live processes whose command line names no conversation, the one the CLI's own
    /// list of live sessions gives each pid, when exactly one. Empty when it keeps no such
    /// list or could not be asked; the screen and hooks then get their chance.
    fn sessions_by_pid(&self, _pids: &[u32]) -> Vec<(u32, String)> {
        vec![]
    }
    /// Where this CLI reads command hooks from and which events report to the host. A CLI
    /// whose processes or files already name their conversation needs none.
    fn hooks(&self, _home: &Path) -> Option<Hooks> {
        None
    }
    /// The variable this CLI sets on every command it runs, naming the conversation that
    /// ran it. Better than the terminal's: its commands may come from a shared daemon.
    fn conversation_env(&self) -> Option<&'static str> {
        None
    }
    /// Where this CLI finds skills, for one with no hook to be told things through.
    fn skills_dir(&self, _home: &Path) -> Option<PathBuf> {
        None
    }
    /// The bundled rules (`screen/<cli>.json`) that read a blocked question off this CLI's
    /// quiet screen. See `crate::screen_rules`.
    fn screen_rules(&self) -> &'static str {
        DEFAULT_SCREEN_RULES
    }
}
/// A CLI without its own screen rules is read with Claude Code's shapes, as every CLI was
/// before the rules became files.
pub(crate) const DEFAULT_SCREEN_RULES: &str = include_str!("screen/claude.json");
/// A CLI's hook settings: a JSON file with `hooks.<Event>[].hooks[]` command entries, the
/// shape Claude Code reads.
pub struct Hooks {
    pub settings: PathBuf,
    pub events: &'static [&'static str],
}

/// How a CLI tells what a turn that ended left running. Either way the commands get the
/// same judgment, job or server (`background_shells::judge`).
#[derive(Clone, Copy)]
pub enum BackgroundCommands {
    /// Its Stop hook lists them with their commands; a CLI with no such hook lists none.
    Hooks,
    /// Only its screen says so, read by this function. Which commands is found among the
    /// children of its processes (`cli_commands`), claimed by the log that started them.
    Screen(fn(&str) -> bool),
}

pub fn adapter(agent: Agent) -> &'static dyn Adapter {
    match agent {
        Agent::Codex => &codex::Codex,
        Agent::Claude => &claude::Claude,
        Agent::Grok => &grok::Grok,
        Agent::Cursor => &cursor::Cursor,
        Agent::Qoder => &qoder::Qoder,
        Agent::Hermes => &hermes::Hermes,
        Agent::Droid => &droid::Droid,
        Agent::Pi => &pi::Pi(false),
        Agent::Omp => &pi::Pi(true),
        Agent::Dsh => &dsh::Dsh,
        _ => declared()
            .iter()
            .find(|d| d.agent() == agent)
            .map_or(&declared::Missing as &dyn Adapter, |d| d as &dyn Adapter),
    }
}

/// Every registered CLI that is not built in, read from its declaration; again whenever
/// the registry was replaced. A replaced set stays alive for whoever still holds an adapter.
fn declared() -> &'static [declared::Declared] {
    static DECLARED: mooring_core::plugin::Slot<(u64, Vec<declared::Declared>)> =
        mooring_core::plugin::Slot::new();
    let now = mooring_core::agent::generation();
    if let Some((at, set)) = DECLARED.get() {
        if *at == now {
            return set;
        }
    }
    let set = Agent::all()
        .into_iter()
        .filter_map(declared::Declared::new)
        .collect();
    &DECLARED.set((now, set)).1
}

/// `roots` after plugins changed: the built-ins' as they are (a fixture chooses its own),
/// and each declared CLI's as its declaration says now; one no longer declared has none.
pub fn roots_after_reload(
    roots: &[(Agent, std::path::PathBuf)],
    home: &Path,
) -> Vec<(Agent, std::path::PathBuf)> {
    let declared_now = |a: &Agent| a.spec().is_some_and(|s| s.file.is_some());
    let mut out: Vec<_> = roots
        .iter()
        .filter(|(a, _)| a.spec().is_some() && !declared_now(a))
        .cloned()
        .collect();
    out.extend(
        Agent::all()
            .into_iter()
            .filter(declared_now)
            .map(|a| (a, adapter(a).root(home)))
            .filter(|(_, root)| !root.as_os_str().is_empty())
            // As the index keeps roots: file events name the real path, not a symlink's.
            .map(|(a, root)| {
                let real = mooring_platform::file_watch::normalized(&root).unwrap_or(root);
                (a, real)
            }),
    );
    out
}

/// Each CLI's transcript root under `home`; a CLI that writes none has no root.
/// What counting a log's tokens remembers between lines.
pub struct UsageState<'a> {
    /// Replies already counted, across files.
    pub seen: &'a mut HashSet<String>,
    /// The running total this file's CLI logs.
    pub running: &'a mut u64,
}
pub fn usage_marker(agent: Agent) -> Option<&'static [u8]> {
    adapter(agent).usage_marker()
}
pub fn usage_line(
    agent: Agent,
    line: &[u8],
    since: u64,
    state: &mut UsageState,
) -> Option<(u64, u64)> {
    adapter(agent).usage_line(line, since, state)
}

/// The conversations the calling process says it runs in, in its CLI's own ids, for the
/// CLIs that say so.
pub fn calling_conversations() -> Vec<(Agent, String)> {
    Agent::all()
        .into_iter()
        .filter_map(|a| Some((a, std::env::var(adapter(a).conversation_env()?).ok()?)))
        .filter(|(_, id)| !id.is_empty())
        .collect()
}
pub fn edit_marker(agent: Agent) -> Option<&'static [u8]> {
    adapter(agent).edit_marker()
}
pub fn logged_edits(agent: Agent, line: &Value) -> Vec<(String, mooring_core::turn_diff::Edit)> {
    adapter(agent).logged_edits(line)
}
pub fn reads_tools(agent: Agent) -> bool {
    adapter(agent).reads_tools()
}
pub fn tool_call_line(agent: Agent, line: &Value, id: &str) -> Option<ToolCallLine> {
    adapter(agent).tool_call_line(line, id)
}
pub fn tool_result_line(agent: Agent, line: &Value, id: &str) -> Option<ToolOutput> {
    adapter(agent).tool_result_line(line, id)
}

pub fn roots(home: &Path) -> Vec<(Agent, PathBuf)> {
    Agent::all()
        .into_iter()
        .map(|a| (a, adapter(a).root(home)))
        .filter(|(_, root)| !root.as_os_str().is_empty())
        .collect()
}

/// The CLI a `ps` command line runs, if any.
pub fn from_command(command: &str) -> Option<Agent> {
    let mut args = command.split_whitespace();
    let executable = Path::new(args.next()?).file_name()?.to_str()?;
    let interpreter = matches!(executable, "node" | "bun")
        || executable.to_ascii_lowercase().starts_with("python");
    let script = interpreter.then(|| args.next()).flatten();
    Agent::all()
        .into_iter()
        .find(|a| adapter(*a).runs(executable, script))
}

/// The conversation id and working directory a transcript line names.
pub fn identity(agent: Agent, path: &Path, line: &[u8]) -> Option<(String, String)> {
    let adapter = adapter(agent);
    adapter
        .identity(path, &adapter.header_value(line)?)
        .filter(|(id, _)| !id.is_empty())
}

/// The environment variable a CLI's home can be moved with, or `home/default`.
/// The last `n` non-empty lines of a screen, trimmed, bottom first.
fn bottom_lines(tail: &str, n: usize) -> impl Iterator<Item = &str> {
    tail.lines()
        .rev()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(n)
}
fn home_dir(home: &Path, var: &str, default: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(default))
}

/// Cards hide sessions run in throwaway directories; a managed terminal still shows them.
fn temporary(cwd: &str) -> bool {
    [
        "/tmp",
        "/private/tmp",
        "/var/tmp",
        "/private/var/tmp",
        "/var/folders",
        "/private/var/folders",
    ]
    .iter()
    .any(|root| cwd == *root || cwd.starts_with(&format!("{root}/")))
}

/// The flags after `executable` in an argv that a resume repeats: `flags` stand alone,
/// `values` take the next argument (or `=value`), `lists` every argument up to the next
/// flag. Subcommands, prompts, ids and unlisted flags stay behind.
pub(crate) fn carried(
    args: &[String],
    executable: &str,
    flags: &[&str],
    values: &[&str],
    lists: &[&str],
) -> Vec<String> {
    let Some(start) = args
        .iter()
        .position(|a| Path::new(a).file_name().is_some_and(|n| n == executable))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut rest = args[start + 1..].iter().peekable();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        let name = arg.split_once('=').map_or(arg.as_str(), |(name, _)| name);
        if flags.contains(&arg.as_str()) {
            out.push(arg.clone());
        } else if values.contains(&name) || lists.contains(&name) {
            out.push(arg.clone());
            if name.len() != arg.len() {
                continue;
            }
            if lists.contains(&name) {
                while let Some(value) = rest.next_if(|v| !v.starts_with('-')) {
                    out.push(value.clone());
                }
            } else if let Some(value) = rest.next() {
                out.push(value.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod codex_header_tests;
