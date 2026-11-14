//! Driving Fibo terminals from scripts and other Agents: list them, read a screen, send text, prompt an Agent and wait.
use fibo_core::delivery::{deliver, PATIENCE};
use fibo_core::i18n::{self, Lang};
use fibo_core::status::{self, Status};
use fibo_core::{model::*, protocol::*};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod events_text;
mod explain;
mod inspect;
mod list_text;
mod open;
mod prompt;
mod run_agent;
mod todo;
mod todo_text;
use explain::explain;
use inspect::{events, list, read};
use open::{close, split};
use prompt::{prompt, send, wait_cmd};
use run_agent::run_agent;
use todo::todo;

/// One `fibo` command: the flags it takes and its lines in the usage text. The full
/// usage is put together from this table, `-h`/`--help` prints a command's own lines,
/// and a test checks that each command's lines name exactly its flags, so what is
/// documented and what is accepted cannot drift apart again. `--runtime <目录>` is
/// accepted by every command and is not listed.
pub(crate) struct Command {
    pub(crate) name: &'static str,
    /// Flags followed by a value.
    pub(crate) valued: &'static [&'static str],
    /// Flags that stand alone.
    pub(crate) switches: &'static [&'static str],
    /// The command's lines (as an i18n key): each starts `fibo <name>`, descriptions are indented.
    pub(crate) usage: &'static str,
}
impl Command {
    pub(crate) fn usage(&self) -> &'static str {
        i18n::say(self.usage)
    }
}
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        name: "list",
        valued: &[],
        switches: &["json"],
        usage: tk!("\
fibo list [--json]                         终端列表：要你处理的排前面，状态、Agent、标签、目录、编号"),
    },
    Command {
        name: "read",
        valued: &["lines"],
        switches: &[],
        usage: tk!("\
fibo read <终端> [--lines N]               画面最后 N 行（默认 50）"),
    },
    Command {
        name: "send",
        valued: &[],
        switches: &["enter"],
        usage: tk!("\
fibo send <终端> <文字|-> [--enter]        把文字作为粘贴放进终端；- 从标准输入读（最多 64 KiB）"),
    },
    Command {
        name: "prompt",
        valued: &["timeout"],
        switches: &["wait", "force"],
        usage: tk!("\
fibo prompt <终端> <文字|-> [--wait] [--timeout 秒] [--force]
                                             给 Agent 发一条并回车；它正等你回答时拒绝，除非 --force"),
    },
    Command {
        name: "wait",
        valued: &["until", "timeout"],
        switches: &[],
        usage: tk!("\
fibo wait <终端> [--until idle|waiting|working] [--timeout 秒]
                                             等 Agent 进入某个状态（默认 idle：这一轮做完）"),
    },
    Command {
        name: "explain",
        valued: &["file", "agent"],
        switches: &["json"],
        usage: tk!("\
fibo explain --file <画面.txt> --agent <claude|codex> [--json]
fibo explain <终端> [--json]               画面规则怎么读这块屏：状态、命中的规则、规则版本和来源、看到的行；
                                             还有每个后台 shell 算任务还是服务、依据哪条"),
    },
    Command {
        name: "events",
        valued: &[],
        switches: &["json"],
        usage: tk!("\
fibo events [--json]                       先列出现在的状态，再每有变化输出一行，直到 Ctrl-C：
                                             started / waiting / error / done、approval / approval_gone、
                                             terminal_started / terminal_exited、command_done / command_failed；
                                             lagged 表示跟不上丢了事件，应重新开一次取现状"),
    },
    Command {
        name: "split",
        valued: &["size", "cwd"],
        switches: &["down", "tab", "here", "json"],
        usage: tk!("\
fibo split <终端> [--down] [--size 0.5] [--tab|--here] [--cwd 目录] [--json] [-- 命令…]
                                             在这个终端右边（--down 下面）开一块新分屏，打印新终端编号；
                                             标签已有 4 块或切开太窄时开到「旁屏」标签（--here 改为拒绝，
                                             --tab 直接开到旁屏）；给了命令就在新 shell 里跑，跑完留在 shell"),
    },
    Command {
        name: "run",
        valued: &["cwd", "from", "timeout"],
        switches: &["split", "no-report", "json"],
        usage: tk!("\
fibo run <agent> <任务|-> [--split] [--cwd 目录] [--no-report] [--from <终端>] [--timeout 秒] [--json]
                                             派一个 Agent 去做事：开在旁屏标签（--split 开在旁边），等它出现输入框
                                             再交任务；它每结束一轮，结果自动发回你的输入框（<fibo from=…>）；
                                             不在 Fibo 终端里时用 --from 说开在谁旁边、汇报给谁"),
    },
    Command {
        name: "close",
        valued: &[],
        switches: &[],
        usage: tk!("\
fibo close <终端>                          关掉这块分屏并结束里面的程序"),
    },
    Command {
        name: "todo",
        valued: &["task", "said", "space"],
        switches: &["all", "work", "json"],
        usage: tk!("\
fibo todo list [--all] [--json]            没做完的待办；--all 连已收尾的
fibo todo show [--task <待办>] [--json]      看一条：用户写的、验收标准、进展
fibo todo add <标题|-> [--said <原话>] [--space <空间>] [--work] [--json]
                                           记一件事，默认只记下来自哪个会话；--work 表示这个会话就在做它
fibo todo note <文字|-> [--task <待办>]      写进展：做了什么、剩什么、要用户定什么；完成由用户标记"),
    },
    Command {
        name: "remote",
        valued: &[
            "relay",
            "relay-key",
            "name",
            "role",
            "revoke",
            "device",
            "kick",
            "machine-name",
        ],
        switches: &[
            "stdio", "pair", "new-code", "devices", "enable", "disable", "status", "report", "json",
        ],
        usage: tk!("\
fibo remote --stdio [--role view|answer|full]     给另一台机器或手机 App 用：一行一个 JSON 请求和回复（看画面、打字、按键、
                                             答审批、事件），经 ssh 连过来：ssh 主机 fibo remote --stdio
fibo remote --relay <中继地址> [--relay-key <密钥>] [--report]
                                             连上中继并一直挂着，配对过的手机在哪都能连过来；内容端到端加密，
                                             中继看不到。中继要密钥时也可以设 MOORING_RELAY_KEY；
                                             --report 把连接状态报给后台（后台自己启动它时用）
fibo remote --pair --relay <中继地址> [--name 名字] [--role view|answer|full] [--new-code]
                                             配对一台新手机，打印它的二维码和链接；权限分只看 / 能答审批 / 完全控制（默认）。
                                             加 --new-code 换掉机器码，已配对的手机全部失效
fibo remote --enable --relay <中继地址> [--relay-key <密钥>] | --disable
                                             让后台一直挂在中继上（重启后也会自动连回去），或者断开；配对保留
fibo remote --status [--json]              中继连接状态、配对的手机、现在谁在线、在看哪个终端
fibo remote --kick <设备>                  立刻断开这台手机正连着的会话，配对保留
fibo remote --devices                      列出配对过的手机、权限和最近连接时间；操作记在运行目录的 remote-log.jsonl
fibo remote --device <设备> --role <view|answer|full>
                                             改一台手机的权限；它正连着的会话一秒内断开，重连后按新权限
fibo remote --revoke <设备编号或名字>      取消一台手机的配对，它正连着的会话一秒内断开
fibo remote --machine-name <名字>          手机上这台电脑叫什么；给空字符串换回系统主机名，手机重连后显示新名字"),
    },
    Command {
        name: "themes",
        valued: &[],
        switches: &[],
        usage: crate::themes_cmd::USAGE,
    },
    Command {
        name: "plugins",
        valued: &["dir", "event", "cwd", "key", "name"],
        switches: &["json", "remote", "check", "yes", "send", "force"],
        usage: crate::plugins_cmd::COMMAND_USAGE,
    },
    Command {
        name: "hooks",
        valued: &["agent"],
        switches: &["json", "outdated-only"],
        usage: tk!("\
fibo hooks install|uninstall [--agent <cli>]
                                             把 Fibo 的 hook 写进（或移出）Agent CLI 自己的设置；不写 --agent 就是全部
fibo hooks status [--agent <cli>] [--json] [--outdated-only]
                                             各 CLI 的 hook 装了没有、和现在 install 会写的差在哪"),
    },
    Command {
        name: "hook",
        valued: &["agent"],
        switches: &[],
        usage: tk!("\
fibo hook --agent <claude|codex|grok>      Agent CLI 的 hook 调用的入口：从标准输入读一条 hook JSON（最多 1 MiB）
                                             报给后台；不在 Fibo 终端里什么都不做。不用手动跑"),
    },
    Command {
        name: "serve",
        valued: &["recent-days"],
        switches: &["takeover"],
        usage: tk!("\
fibo serve --runtime <目录> [--takeover] [--recent-days N]
                                             启动后台（App 自己会启动，不用手动跑）；--takeover 从正在跑的后台
                                             接过全部终端，--recent-days 只读最近 N 天的会话（默认 7）"),
    },
    Command {
        name: "socket",
        valued: &[],
        switches: &[],
        usage: tk!("\
fibo socket                                打印后台的 socket 路径；另一台电脑的窗口经 ssh 连这台时用"),
    },
    Command {
        name: "scan",
        valued: &[],
        switches: &[],
        usage: tk!("\
fibo scan --runtime <目录>                 让后台重新扫一遍 Agent 会话，打印会话数和终端数"),
    },
    Command {
        name: "--skill",
        valued: &[],
        switches: &[],
        usage: tk!("\
fibo --skill                               给 Coding Agent 看的用法说明（Markdown）
fibo --skill plugins                       给 Coding Agent 看的写插件、改外观说明（Markdown）
fibo --skill session                       Fibo 在会话开始时告诉 Agent 的话"),
    },
    Command {
        name: "--version",
        valued: &[],
        switches: &[],
        usage: tk!("\
fibo --version                             打印协议版本"),
    },
];
const FOOTER: &str = tk!("\
<终端> 可以是编号的前几位或标签名；全部命令都接受 --runtime <目录>；-h / --help 只打印用法、什么都不做。
Agent 藏在包装器（沙盒、虚拟机、nono 等）后面认不出时，启动前设 MOORING_AGENT=<claude|codex|…>。");
/// Every command's lines, then what holds for all of them.
pub(crate) fn usage() -> String {
    let mut text: String = COMMANDS
        .iter()
        .map(|c| format!("{}\n", c.usage()))
        .collect();
    text.push_str(i18n::say(FOOTER));
    text
}
pub(crate) fn command(name: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|c| c.name == name)
}
fn help_flag(arg: &str) -> bool {
    arg == "-h" || arg == "--help"
}
/// Checks `args` against what `cmd` takes, before anything acts: `Ok(true)` when they
/// ask for help (`-h`/`--help` anywhere before a bare `--`), an error naming the flag
/// and showing the command's usage for an unknown flag or a missing value.
pub(crate) fn check(cmd: &Command, args: &[String]) -> io::Result<bool> {
    let mut args = args.iter();
    let mut wrong = None;
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if help_flag(arg) {
            return Ok(true);
        }
        let Some(name) = arg.strip_prefix("--") else {
            continue;
        };
        if name == "runtime" || cmd.valued.contains(&name) {
            match args.next() {
                Some(v) if help_flag(v) => return Ok(true),
                Some(_) => (),
                None => {
                    wrong.get_or_insert_with(|| t!("--{name} 需要一个值", name));
                }
            }
        } else if !cmd.switches.contains(&name) {
            wrong.get_or_insert_with(|| {
                t!("fibo {cmd} 没有 --{name} 这个选项", cmd = cmd.name, name)
            });
        }
    }
    match wrong {
        Some(e) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            t!("{e}\n用法：\n{usage}", e, usage = cmd.usage()),
        )),
        None => Ok(false),
    }
}
/// `fibo --skill`: how a coding Agent drives these commands, and when it must not.
pub const SKILL: &str = include_str!("skill.md");
pub const SKILL_EN: &str = include_str!("skill_en.md");
/// `fibo --skill plugins`: how a coding Agent changes Fibo with plugins.
pub const PLUGINS_SKILL: &str = include_str!("plugins.md");
pub const PLUGINS_SKILL_EN: &str = include_str!("plugins_en.md");
/// What every conversation in a Fibo terminal is told when it starts. It costs every
/// conversation these words, so it only says when to write something down.
pub const SESSION_SKILL: &str = "你在 Fibo 终端里。用户说「记一下」，或有这次不处理但之后要跟进的事时：先 fibo todo list 查重，有就 fibo todo note --task <待办> 写进展，没有就 fibo todo add \"标题\" --said \"用户原话\"。当场问完答完的事不记。完成由用户标记，你不能标。\n";
pub const SESSION_SKILL_EN: &str = "You are in a Fibo terminal. When the user says 'note this down' or out-of-scope follow-up work surfaces: run fibo todo list first to check for duplicates; if tracked, run fibo todo note --task <todo> with progress, else run fibo todo add \"title\" --said \"user's words\". Skip questions answered on the spot. Completion is marked by the user — you cannot mark todos done.\n";

pub fn skill() -> &'static str {
    match i18n::lang() {
        Lang::Zh => SKILL,
        Lang::En => SKILL_EN,
    }
}
pub fn plugins_skill() -> &'static str {
    match i18n::lang() {
        Lang::Zh => PLUGINS_SKILL,
        Lang::En => PLUGINS_SKILL_EN,
    }
}
pub fn session_skill() -> &'static str {
    match i18n::lang() {
        Lang::Zh => SESSION_SKILL,
        Lang::En => SESSION_SKILL_EN,
    }
}

/// What the Agent in a terminal is doing, read like the workbench's pane states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Working,
    Waiting,
    Idle,
    /// No Agent conversation is current in this terminal: a shell or another program.
    None,
}
impl Phase {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Idle => "idle",
            Self::None => "-",
        }
    }
}
/// Read from `status::of`, the one place a session's state is decided, so `list`,
/// `wait` and the phone never disagree with the window.
pub fn phase(w: &Workspace, terminal: &str) -> Phase {
    match status(w, terminal).map(Status::phase) {
        None => Phase::None,
        Some(status::Phase::Working) => Phase::Working,
        Some(status::Phase::Waiting) => Phase::Waiting,
        Some(status::Phase::Idle) => Phase::Idle,
    }
}
/// The state of the conversation current in a terminal, as the window shows it.
pub fn status(w: &Workspace, terminal: &str) -> Option<Status> {
    w.current_agent(terminal)
        .map(|s| status::of(w, s, fibo_core::content::now()))
}
/// The registry key in lower case, the way the CLI is usually typed.
pub(crate) fn agent_label(agent: fibo_core::agent::Agent) -> String {
    agent.key().to_lowercase()
}
pub(crate) fn tab_title(w: &Workspace, terminal: &str) -> String {
    w.terminal_tab_title(terminal)
}
/// A full id, a unique id prefix, or a unique tab title, among running terminals.
pub fn resolve(w: &Workspace, needle: &str) -> io::Result<String> {
    let running: Vec<&Terminal> = w
        .terminals
        .iter()
        .filter(|t| t.state == ProcessState::Running)
        .collect();
    if let Some(t) = running.iter().find(|t| t.id == needle) {
        return Ok(t.id.clone());
    }
    let by_prefix: Vec<_> = running
        .iter()
        .filter(|t| t.id.starts_with(needle))
        .collect();
    let found: Vec<_> = if by_prefix.is_empty() {
        running
            .iter()
            .filter(|t| tab_title(w, &t.id) == needle)
            .collect()
    } else {
        by_prefix
    };
    match found.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(io::Error::other(t!(
            "没有正在运行的终端匹配 {needle}",
            needle
        ))),
        many => Err(io::Error::other(t!(
            "{needle} 匹配到 {n} 个终端，请写更长的编号",
            needle,
            n = many.len()
        ))),
    }
}

pub(crate) struct Args {
    pub(crate) socket: PathBuf,
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
    /// Everything after a bare `--`, untouched: a command and its own flags.
    rest: Vec<String>,
    /// The command's own usage lines (as an i18n key), for errors about its arguments.
    usage: &'static str,
}
impl Args {
    pub(crate) fn usage(&self) -> &'static str {
        i18n::say(self.usage)
    }
    /// Rejects a flag `cmd` does not take before anything acts; help is answered by the
    /// caller (`main`) before this runs.
    pub(crate) fn parse(args: impl Iterator<Item = String>, cmd: &Command) -> io::Result<Self> {
        let args: Vec<String> = args.collect();
        if check(cmd, &args)? {
            return Err(io::Error::other(t!("用法：\n{usage}", usage = cmd.usage())));
        }
        let valued = cmd.valued;
        let mut runtime = std::env::var_os("MOORING_RUNTIME").map(PathBuf::from);
        let mut positional = vec![];
        let mut flags = vec![];
        let mut rest = vec![];
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--" {
                rest.extend(args.by_ref());
            } else if arg == "--runtime" {
                runtime = Some(
                    args.next()
                        .ok_or_else(|| io::Error::other(t!("--runtime 需要目录")))?
                        .into(),
                );
            } else if let Some(name) = arg.strip_prefix("--") {
                let value = if valued.contains(&name) {
                    Some(
                        args.next()
                            .ok_or_else(|| io::Error::other(t!("--{name} 需要一个值", name)))?,
                    )
                } else {
                    None
                };
                flags.push((name.to_owned(), value));
            } else {
                positional.push(arg);
            }
        }
        let runtime = match runtime {
            Some(r) => r,
            // Where the app keeps its host, so a script needs no flag next to it.
            // An install from before the rename keeps serving the old directory.
            None => fibo_core::legacy::runtime_dir(
                &fibo_platform::dirs::home().ok_or_else(|| io::Error::other("HOME unavailable"))?,
            ),
        };
        Ok(Self {
            socket: runtime.join("workbench.sock"),
            positional,
            flags,
            rest,
            usage: cmd.usage,
        })
    }
    pub(crate) fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }
    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }
    fn number(&self, name: &str, default: u64) -> io::Result<u64> {
        self.value(name)
            .map(|v| {
                v.parse()
                    .map_err(|_| io::Error::other(t!("--{name} 需要数字", name)))
            })
            .unwrap_or(Ok(default))
    }
    fn arg(&self, i: usize, what: &str) -> io::Result<&str> {
        self.positional.get(i).map(String::as_str).ok_or_else(|| {
            io::Error::other(t!(
                "缺少{what}\n用法：\n{usage}",
                what,
                usage = self.usage()
            ))
        })
    }
    /// `-` reads the text from standard input, so long prompts need no shell quoting.
    fn text(&self, i: usize) -> io::Result<String> {
        let text = self.arg(i, t!("文字"))?;
        if text != "-" {
            return Ok(self.positional[i..].join(" "));
        }
        read_limited(io::stdin(), STDIN_LIMIT).map(|s| s.trim_end_matches('\n').to_owned())
    }
}
/// The most `-` reads from standard input; more is an error, never a silent cut.
const STDIN_LIMIT: usize = 64 * 1024;
fn read_limited(input: impl Read, limit: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    input.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::other(t!(
            "标准输入超过 {kb} KiB，没有发送；拆成几段再发",
            kb = limit / 1024
        )));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::other(t!("标准输入不是 UTF-8 文字")))
}
fn workspace(socket: &Path) -> io::Result<Workspace> {
    match rpc(socket, &Request::Workspace)? {
        Reply::Workspace(w) => Ok(w),
        other => Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    }
}
fn expect_ok(reply: Reply) -> io::Result<()> {
    match reply {
        Reply::Ok => Ok(()),
        other => Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    }
}

/// The commands this module carries out; `main` sends each of them here.
pub(crate) const DRIVES: &[&str] = &[
    "list", "read", "send", "prompt", "wait", "explain", "events", "split", "run", "close", "todo",
];
pub fn run(cmd: &Command, args: impl Iterator<Item = String>) -> io::Result<()> {
    let a = Args::parse(args, cmd)?;
    match cmd.name {
        "list" => list(a),
        "read" => read(a),
        "send" => send(a),
        "prompt" => prompt(a),
        "wait" => wait_cmd(a),
        "explain" => explain(a),
        "events" => events(a),
        "split" => split(a),
        "run" => run_agent(a),
        "close" => close(a),
        "todo" => todo(a),
        _ => Err(io::Error::other(usage())),
    }
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
