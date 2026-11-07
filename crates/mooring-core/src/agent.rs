//! Which CLIs Mooring knows, as data. The rest of Mooring asks here or the daemon's
//! `agents` adapters and never names a CLI itself (`tests/architecture_boundary.rs`).
//!
//! The built-in three are compiled in. Any other comes from a declaration in
//! `~/.config/mooring/agents/*.json`, loaded once per process by `install`; a later file
//! with the same id replaces an earlier one, and a file that cannot be used is reported,
//! never fatal. See docs/architecture/PLUGIN_SYSTEM.md.
use crate::agent_answers::Answers;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// A CLI's identity: the key stored in the database and sent over the protocol. The
/// built-in keys are the names the enum this replaced serialized as, so nothing stored
/// changes. A key no longer registered (its declaration was removed) still reads back,
/// with no adapter and its key as its name.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Agent(&'static str);

#[allow(non_upper_case_globals)]
impl Agent {
    pub const Codex: Agent = Agent("Codex");
    pub const Claude: Agent = Agent("Claude");
    pub const Grok: Agent = Agent("Grok");
    pub const Cursor: Agent = Agent("Cursor");
    pub const Qoder: Agent = Agent("Qoder");
    pub const Hermes: Agent = Agent("Hermes");
    pub const Droid: Agent = Agent("Droid");
    pub const Pi: Agent = Agent("Pi");
    pub const Omp: Agent = Agent("Omp");
    pub const Dsh: Agent = Agent("Dsh");

    /// Every registered CLI, built-in first, in the order the app offers them.
    pub fn all() -> Vec<Agent> {
        registry().specs.iter().map(|s| Agent(s.key)).collect()
    }
    pub fn key(self) -> &'static str {
        self.0
    }
    /// The key as stored, registered or not.
    pub fn from_key(key: &str) -> Agent {
        Agent(intern(key))
    }
    pub fn spec(self) -> Option<&'static AgentSpec> {
        registry().specs.iter().find(|s| s.key == self.0)
    }
    pub fn name(self) -> &'static str {
        self.spec().map_or(self.0, |s| s.name.as_str())
    }
    pub fn executable(self) -> &'static str {
        self.spec().map_or(self.0, |s| s.executable.as_str())
    }
    /// The CLI's mark as a one-fill SVG the app tints with its text color
    /// (`assets/agent-icons/LICENSE`). A declared CLI has none and shows its initial.
    pub fn icon_svg(self) -> Option<&'static [u8]> {
        match self.0 {
            "Claude" => Some(include_bytes!("../assets/agent-icons/claude.svg")),
            "Codex" => Some(include_bytes!("../assets/agent-icons/codex.svg")),
            _ => None,
        }
    }
    /// The CLI's own color as 0xRRGGBB, which its mark is drawn in (v3 mock,
    /// `docs/ui-v3/v3-4.png`: Claude Code's clay, Codex's blue). Others have none and
    /// keep the ink they are drawn with.
    pub fn brand_color(self) -> Option<u32> {
        match self.0 {
            "Claude" => Some(0xd97757),
            "Codex" => Some(0x3d4fd8),
            _ => None,
        }
    }
    pub fn from_executable(name: &str) -> Option<Self> {
        registry()
            .specs
            .iter()
            .find(|s| s.executable == name)
            .map(|s| Agent(s.key))
    }
    /// The command a new conversation starts with.
    pub fn command(self) -> String {
        render(self.spec().map_or(self.0, |s| s.start.as_str()), None)
    }
    /// With `launch` flags, they go right after the executable, and `command` skips a
    /// shell function of the same name: such a wrapper usually adds the same flags, and
    /// Codex rejects a flag given twice. A template not starting with the executable
    /// resumes without them.
    pub fn resume_command(self, cli_id: &str, launch: &[String]) -> String {
        let Some(s) = self.spec() else {
            return self.command();
        };
        let plain = render(&s.resume, Some(cli_id));
        if launch.is_empty() {
            return plain;
        }
        match plain
            .strip_prefix(s.executable.as_str())
            .filter(|rest| rest.is_empty() || rest.starts_with(' '))
        {
            Some(rest) => {
                let flags: Vec<_> = launch.iter().map(|a| shell_word(a)).collect();
                format!("command {} {}{rest}", s.executable, flags.join(" "))
            }
            None => plain,
        }
    }
}
impl Default for Agent {
    fn default() -> Self {
        Agent::Codex
    }
}
impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl Serialize for Agent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}
impl<'de> Deserialize<'de> for Agent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Agent::from_key(&String::deserialize(d)?))
    }
}

/// `{id}` is the conversation to resume, shell-quoted; `{uuid}` a fresh one for a CLI
/// that takes its id up front.
fn render(template: &str, id: Option<&str>) -> String {
    let mut out = template.replace("{uuid}", &uuid::Uuid::new_v4().to_string());
    if let Some(id) = id {
        out = out.replace("{id}", &crate::model::shell_quote(id));
    }
    out
}

/// Quoted only when the shell would read it differently: flags stay legible.
fn shell_word(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=./:,@+".contains(c));
    if plain {
        value.to_owned()
    } else {
        crate::model::shell_quote(value)
    }
}

/// Keys are few and live for the process, so an unknown one read back is leaked once.
fn intern(key: &str) -> &'static str {
    if let Some(s) = registry().specs.iter().find(|s| s.key == key) {
        return s.key;
    }
    static UNKNOWN: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut unknown = UNKNOWN
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match unknown.get(key) {
        Some(k) => k,
        None => {
            let k: &'static str = Box::leak(key.to_owned().into_boxed_str());
            unknown.insert(k);
            k
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    pub key: &'static str,
    pub name: String,
    pub executable: String,
    /// Path suffixes of the script an interpreter (`node`, `bun`, `python*`) runs.
    pub scripts: Vec<String>,
    pub start: String,
    pub resume: String,
    /// Built-ins read their own transcripts in code; a declaration says where they are.
    pub transcripts: Option<Transcripts>,
    /// api 2: where the conversation lives when the CLI keeps no transcript files.
    pub source: Option<RecordSource>,
    /// How the CLI shows a sent ask in its terminal, for pinning the turn's ask.
    pub prompt: PromptLine,
    /// The declaration it came from; `None` for a built-in.
    pub file: Option<PathBuf>,
    /// api 2: its screen rules, the text of a `screen_rules` file. `None`: Claude Code's.
    pub screen_rules: Option<String>,
    /// api 2: what interrupts a turn and leaves it alive. `None`: Ctrl-C.
    pub interrupt: Option<Interrupt>,
    /// api 2: how it takes answers over a local API. `None`: only by its screen.
    pub answers: Option<Answers>,
    /// api 2: text its empty input box shows, for a CLI whose box has no arrow.
    pub input_hint: Vec<String>,
}

/// Keys that interrupt a CLI's running turn, for one where Ctrl-C does something else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interrupt {
    pub key: &'static [u8],
    /// How many times, for a CLI that asks to press again.
    pub presses: u32,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InterruptDeclared {
    key: String,
    #[serde(default = "one_press")]
    presses: u32,
}
fn one_press() -> u32 {
    1
}

/// How a CLI draws an ask the user sent, in the terminal's history: the mark the row
/// starts with (Claude Code `❯`, Codex `›`), and whether the CLI already keeps the turn's
/// ask pinned on its own screen, in which case Mooring draws no pin of its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptLine {
    /// Marks a sent ask's row starts with, after any indent. Empty: the row starts with
    /// the ask's text itself.
    #[serde(default)]
    pub markers: Vec<String>,
    #[serde(default)]
    pub pins_own: bool,
}

pub use crate::agent_transcripts::{RecordSource, Transcripts};

/// One kind of record and what it means. `when` maps JSON pointers to the value each
/// must hold; `"*"` only asks that the field exist.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventRule {
    pub when: std::collections::BTreeMap<String, serde_json::Value>,
    /// The turn's state from this record on.
    #[serde(default)]
    pub turn: Option<TurnEvent>,
    /// Pointer to the turn's id, for a record that starts one.
    #[serde(default)]
    pub id: Option<String>,
    /// Pointer to what the CLI is asking, for a `waiting` record.
    #[serde(default)]
    pub waiting: Option<String>,
    #[serde(default)]
    pub tool: Option<ToolEvent>,
    #[serde(default)]
    pub result: Option<ResultEvent>,
}
impl EventRule {
    pub fn matches(&self, record: &serde_json::Value) -> bool {
        self.when.iter().all(|(pointer, want)| {
            record
                .pointer(pointer)
                .is_some_and(|got| want == "*" || got == want)
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TurnEvent {
    Running,
    Waiting,
    Completed,
    Interrupted,
    Error,
}
/// A tool call: pointers to its id (pairs it with its result), name and arguments. With
/// `kind`, the call counts as evidence the way a built-in CLI's does: `command` reads the
/// command line at `command` (tests and commits are recognised from it), `edit` the file
/// at `path`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolEvent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub args: Option<String>,
    #[serde(default)]
    pub kind: Option<ToolKind>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Command,
    Edit,
}
/// A tool result: pointers to the call it answers, its output and its exit code.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultEvent {
    pub id: String,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub exit_code: Option<String>,
}
pub(super) fn jsonl() -> String {
    "jsonl".into()
}
pub(super) fn user() -> String {
    "user".into()
}
pub(super) fn assistant() -> String {
    "assistant".into()
}

/// The declaration format's major version; a file of another is refused, not guessed at.
pub const API: u32 = 2;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    api: u32,
    id: String,
    name: String,
    executable: String,
    #[serde(default)]
    scripts: Vec<String>,
    start: String,
    resume: String,
    transcripts: Option<Transcripts>,
    #[serde(default)]
    source: Option<RecordSource>,
    #[serde(default)]
    prompt: PromptLine,
    #[serde(default)]
    screen_rules: Option<serde_json::Value>,
    #[serde(default)]
    interrupt: Option<InterruptDeclared>,
    #[serde(default)]
    answers: Option<Answers>,
    #[serde(default)]
    input_hint: Vec<String>,
}

/// One declaration file's outcome, for settings and diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub file: PathBuf,
    /// The key it registered, or why it did not.
    pub result: Result<String, String>,
}

pub struct Registry {
    specs: Vec<AgentSpec>,
    report: Vec<Loaded>,
}

static REGISTRY: crate::plugin::Slot<Registry> = crate::plugin::Slot::new();
/// Where this process registered from, once it did: what `reload` reads again.
static FROM: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
/// Bumped when the registry is replaced, for whatever is built from it.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Registry {
        specs: builtin(),
        report: vec![],
    })
}

/// How many times the registry has been replaced: compare to know a copy of it is stale.
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::SeqCst)
}

/// Reads the declarations again after plugins changed (`plugin::reload`). An Agent that went
/// away keeps its `Agent` value in stored sessions, with no spec, as one read back from a
/// database always could; one that arrived is known from now on. Does nothing in a process
/// that never registered.
pub(crate) fn reload() {
    let Some((dir, home)) = FROM.get() else {
        return;
    };
    let mut files = json_files(dir);
    files.extend(crate::plugin::files(crate::plugin::Kind::Agent));
    let (specs, report) = load_files(files, home);
    REGISTRY.set(Registry { specs, report });
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// Each declaration file's outcome since `install`; empty before it or without files.
pub fn report() -> &'static [Loaded] {
    &registry().report
}

fn builtin() -> Vec<AgentSpec> {
    let spec = |key, name: &str, executable: &str, start: &str, resume: &str| AgentSpec {
        key,
        name: name.into(),
        executable: executable.into(),
        scripts: vec![],
        start: start.into(),
        resume: resume.into(),
        transcripts: None,
        source: None,
        prompt: PromptLine::default(),
        file: None,
        screen_rules: None,
        interrupt: None,
        answers: None,
        input_hint: vec![],
    };
    let marked = |spec: AgentSpec, markers: &[&str], pins_own| AgentSpec {
        prompt: PromptLine {
            markers: markers.iter().map(|m| (*m).to_owned()).collect(),
            pins_own,
        },
        ..spec
    };
    vec![
        // Temporary until daemon-backed Codex can prove a TUI-to-session association.
        // Codex keeps the turn's ask on its first row while the answer runs on.
        marked(
            spec("Codex", "Codex", "codex", "codex", "codex resume {id}"),
            &["›"],
            true,
        ),
        marked(
            spec(
                "Claude",
                "Claude Code",
                "claude",
                // Started bare: its SessionStart hook names the conversation, and keeps
                // naming it after `/clear` or `/resume`, which an id on argv cannot.
                "claude",
                "claude --resume {id}",
            ),
            &["❯", ">"],
            false,
        ),
        // The TUI cannot be given an id; the daemon reads it from Grok's own registry.
        spec("Grok", "Grok Build", "grok", "grok", "grok --resume {id}"),
        // Cursor names a new chat itself; the daemon reads it from the transcript folder.
        spec(
            "Cursor",
            "Cursor Agent",
            "cursor-agent",
            "cursor-agent",
            "cursor-agent --resume {id}",
        ),
        // No way to choose the id up front; the transcript names it.
        spec(
            "Qoder",
            "Qoder CLI",
            "qodercli",
            "qodercli",
            "qodercli --resume {id}",
        ),
        spec(
            "Hermes",
            "Hermes Agent",
            "hermes",
            "hermes",
            "hermes --resume {id}",
        ),
        spec(
            "Droid",
            "Factory Droid",
            "droid",
            "droid",
            "droid --resume {id}",
        ),
        spec("Pi", "Pi", "pi", "pi", "pi --session {id}"),
        spec("Omp", "oh-my-pi", "omp", "omp", "omp --resume {id}"),
        spec(
            "Dsh",
            "DeepSeek Harness",
            "dsh",
            "dsh tui",
            "dsh tui --resume {id}",
        ),
    ]
}

/// `$XDG_CONFIG_HOME/mooring`, else `~/.config/mooring`.
pub fn config_dir() -> Option<PathBuf> {
    mooring_platform::dirs::config_home().map(|base| base.join("mooring"))
}

/// Registers the declarations in `<config_dir>/agents` and in every plugin that provides
/// agents (`plugins/<id>/agent.json`, `plugins/<id>/agents/*.json`). Call once, before
/// any agent is looked up; a process that never calls it knows the built-ins only.
/// Returns false when the registry was already in use.
pub fn install() -> bool {
    let (Some(dir), Some(home)) = (config_dir(), mooring_platform::dirs::home()) else {
        return false;
    };
    crate::plugin::install(&dir);
    let mut files = json_files(&dir.join("agents"));
    files.extend(crate::plugin::files(crate::plugin::Kind::Agent));
    let (specs, report) = load_files(files, &home);
    if REGISTRY.get().is_some() {
        return false;
    }
    let _ = FROM.set((dir.join("agents"), home));
    REGISTRY.set(Registry { specs, report });
    true
}

pub fn install_from(dir: &Path, home: &Path) -> bool {
    let (specs, report) = load(dir, home);
    if REGISTRY.get().is_some() {
        return false;
    }
    let _ = FROM.set((dir.to_owned(), home.to_owned()));
    REGISTRY.set(Registry { specs, report });
    true
}

/// Each `*.json` in `dir`, by file name.
fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .collect(),
        Err(_) => vec![],
    };
    files.sort();
    files
}

/// Built-ins, then each `*.json` in `dir` by file name.
pub fn load(dir: &Path, home: &Path) -> (Vec<AgentSpec>, Vec<Loaded>) {
    load_files(json_files(dir), home)
}

/// Built-ins, then `files` in order. A later file with an id already declared replaces
/// it, as a later patch row does; a built-in is never replaced.
pub fn load_files(files: Vec<PathBuf>, home: &Path) -> (Vec<AgentSpec>, Vec<Loaded>) {
    let mut specs = builtin();
    let mut report = Vec::new();
    for file in files {
        let result = declare(&file, home, &specs);
        report.push(Loaded {
            file: file.clone(),
            result: result
                .as_ref()
                .map(|s| s.key.to_owned())
                .map_err(Clone::clone),
        });
        let Ok(spec) = result else { continue };
        if let Some(old) = specs.iter_mut().find(|s| s.key == spec.key) {
            for r in report.iter_mut() {
                if r.file.as_path() == old.file.as_deref().unwrap_or(Path::new(""))
                    && r.result.is_ok()
                {
                    r.result = Err(format!("被后面的 {} 取代", file.display()));
                }
            }
            *old = spec;
        } else {
            specs.push(spec);
        }
    }
    (specs, report)
}

fn declare(file: &Path, home: &Path, known: &[AgentSpec]) -> Result<AgentSpec, String> {
    let bytes = std::fs::read(file).map_err(|e| format!("读不了：{e}"))?;
    let d: Declaration = serde_json::from_slice(&bytes).map_err(|e| format!("格式不对：{e}"))?;
    if d.api == 0 || d.api > API {
        return Err(format!(
            "声明格式是 api {}，这个版本只认 api 1 到 {API}",
            d.api
        ));
    }
    let id_ok = !d.id.is_empty()
        && d.id.len() <= 40
        && d.id
            .starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && d.id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !id_ok {
        return Err(format!(
            "id \"{}\" 只能用小写字母、数字和 -，不超过 40 个字",
            d.id
        ));
    }
    let builtin = known.iter().filter(|s| s.file.is_none());
    if let Some(s) = builtin
        .clone()
        .find(|s| s.key.eq_ignore_ascii_case(&d.id) || s.executable == d.id)
    {
        return Err(format!("id \"{}\" 和内置的 {} 重名", d.id, s.name));
    }
    if d.executable.trim().is_empty() || d.executable.contains('/') {
        return Err("executable 要写可执行文件名，不带路径".into());
    }
    if let Some(s) = known
        .iter()
        .find(|s| s.executable == d.executable && s.key != d.id)
    {
        return Err(format!(
            "executable \"{}\" 已经是 {} 的，同一个程序认不出是谁",
            d.executable, s.name
        ));
    }
    if d.start.trim().is_empty() || d.resume.trim().is_empty() {
        return Err("start 和 resume 都要写命令".into());
    }
    let mut transcripts = d.transcripts;
    let mut source = d.source;
    if let Some(s) = source.as_mut() {
        if d.api < 2 {
            return Err("source 需要 api 2".into());
        }
        if let Ok(rest) = s.sqlite.strip_prefix("~") {
            s.sqlite = home.join(rest);
        }
        if !s.sqlite.is_absolute() {
            return Err("source.sqlite 要写绝对路径或 ~/ 开头".into());
        }
        for (name, q) in [("sessions", &s.sessions), ("records", &s.records)] {
            if !q.trim_start().to_ascii_lowercase().starts_with("select") {
                return Err(format!("source.{name} 要写一条 SELECT 查询"));
            }
        }
        if !(200..=60_000).contains(&s.poll_ms) {
            return Err("source.poll_ms 要在 200 到 60000 之间".into());
        }
        match transcripts.as_ref() {
            None => return Err("写了 source 也要写 transcripts，说明每行记录里的字段".into()),
            Some(t) if !t.root.as_os_str().is_empty() => {
                return Err("用 source 时不写 transcripts.root，记录由 Mooring 自己存放".into());
            }
            _ => {}
        }
    }
    if let Some(t) = transcripts.as_mut() {
        if source.is_none() {
            if let Ok(rest) = t.root.strip_prefix("~") {
                t.root = home.join(rest);
            }
            if !t.root.is_absolute() {
                return Err("transcripts.root 要写绝对路径或 ~/ 开头".into());
            }
        }
        let pointers = [
            Some(&t.session),
            Some(&t.cwd),
            t.role.as_ref(),
            t.text.as_ref(),
            t.parent.as_ref(),
        ];
        if let Some(p) = pointers.into_iter().flatten().find(|p| !p.starts_with('/')) {
            return Err(format!(
                "transcripts 里的 \"{p}\" 要写成 / 开头的 JSON 指针"
            ));
        }
        if (!t.events.is_empty() || t.parent.is_some()) && d.api < 2 {
            return Err("transcripts.events / parent 需要 api 2".into());
        }
        if let Some(p) = &t.title {
            if d.api < 2 {
                return Err("transcripts.title 需要 api 2".into());
            }
            if !p.starts_with('/') {
                return Err(format!(
                    "transcripts.title \"{p}\" 要写成 / 开头的 JSON 指针"
                ));
            }
        }
        for (i, e) in t.events.iter().enumerate() {
            let n = i + 1;
            if e.when.is_empty() {
                return Err(format!("events 第 {n} 条的 when 不能为空"));
            }
            if e.turn.is_none() && e.tool.is_none() && e.result.is_none() {
                return Err(format!("events 第 {n} 条要写 turn、tool 或 result 之一"));
            }
            if let Some(t) = &e.tool {
                match t.kind {
                    Some(ToolKind::Command) if t.command.is_none() => {
                        return Err(format!(
                            "events 第 {n} 条的 tool 是 command，要写 command 指针"
                        ));
                    }
                    Some(ToolKind::Edit) if t.path.is_none() => {
                        return Err(format!("events 第 {n} 条的 tool 是 edit，要写 path 指针"));
                    }
                    _ => {}
                }
            }
            let pointers = e
                .when
                .keys()
                .chain(e.id.iter())
                .chain(e.waiting.iter())
                .chain(
                    e.tool
                        .iter()
                        .flat_map(|t| {
                            [
                                Some(&t.id),
                                Some(&t.name),
                                t.args.as_ref(),
                                t.command.as_ref(),
                                t.path.as_ref(),
                            ]
                        })
                        .flatten(),
                )
                .chain(
                    e.result
                        .iter()
                        .flat_map(|r| [Some(&r.id), r.output.as_ref(), r.exit_code.as_ref()])
                        .flatten(),
                );
            if let Some(p) = pointers.into_iter().find(|p| !p.starts_with('/')) {
                return Err(format!(
                    "events 第 {n} 条的 \"{p}\" 要写成 / 开头的 JSON 指针"
                ));
            }
        }
    }
    if d.api < 2
        && (d.screen_rules.is_some()
            || d.interrupt.is_some()
            || d.answers.is_some()
            || !d.input_hint.is_empty())
    {
        return Err("screen_rules、interrupt、answers 和 input_hint 需要 api 2".into());
    }
    if d.input_hint.iter().any(|h| h.trim().is_empty()) {
        return Err("input_hint 不能有空字符串，否则任何画面都算输入框就绪".into());
    }
    if let Some(a) = &d.answers {
        a.check()?;
    }
    let screen_rules = d.screen_rules.as_ref().map(|r| r.to_string());
    let interrupt = match d.interrupt {
        None => None,
        Some(i) => {
            let key: &'static [u8] = match i.key.as_str() {
                "esc" => b"\x1b",
                "ctrl-c" => b"\x03",
                other => return Err(format!("interrupt.key 只能是 esc 或 ctrl-c，不是 {other}")),
            };
            if !(1..=3).contains(&i.presses) {
                return Err("interrupt.presses 要在 1 到 3 之间".into());
            }
            Some(Interrupt {
                key,
                presses: i.presses,
            })
        }
    };
    let name = if d.name.trim().is_empty() {
        d.id.clone()
    } else {
        d.name
    };
    Ok(AgentSpec {
        key: intern_declared(&d.id),
        name,
        executable: d.executable,
        scripts: d.scripts,
        start: d.start,
        resume: d.resume,
        transcripts,
        source,
        prompt: d.prompt,
        file: Some(file.to_owned()),
        screen_rules,
        interrupt,
        answers: d.answers,
        input_hint: d.input_hint,
    })
}

/// Declared keys are leaked once at load; there are only as many as files.
fn intern_declared(id: &str) -> &'static str {
    Box::leak(id.to_owned().into_boxed_str())
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
