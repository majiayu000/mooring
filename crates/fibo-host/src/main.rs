#[macro_use]
extern crate fibo_core;

mod control;
mod hooks_cmd;
mod plugins_cmd;
mod relay;
mod remote;
mod themes_cmd;

fn main() {
    let mut args = std::env::args().skip(1);
    let ran = if args.next().as_deref() == Some("claude-quota") {
        fibo_host::claude_quota::run(args)
    } else {
        run()
    };
    let code = match ran {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("fibo: {err}");
            1
        }
    };
    std::process::exit(code);
}

/// Apps opened from Finder inherit launchd's soft limit of 256 descriptors. Each terminal
/// holds a PTY plus its stream sockets, so about fifty terminals exhausted it and the host
/// exited with EMFILE, leaving every pane black. Raise the soft limit as far as allowed.
fn raise_open_file_limit() {
    match fibo_platform::process::raise_open_file_limit() {
        Ok(Some(wanted)) => eprintln!("fibo: open file limit raised to {wanted}"),
        Ok(None) => (),
        Err(e) => eprintln!("fibo: open file limit: {e}"),
    }
}

fn apply_language() {
    use fibo_core::i18n::{self, Choice};
    let env_choice = std::env::var("MOORING_LANG")
        .ok()
        .and_then(|v| Choice::from_key(&v));
    let saved_choice = fibo_core::agent::config_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("language")).ok())
        .and_then(|text| Choice::from_key(&text));
    let bundled = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .is_ok_and(|exe| fibo_core::cli_entry::in_bundle(&exe));
    let Some(choice) = env_choice
        .or(saved_choice)
        .or_else(|| bundled.then_some(Choice::Auto))
    else {
        return;
    };
    i18n::set(choice.resolve(&fibo_platform::desktop::preferred_languages()));
}

fn run() -> std::io::Result<()> {
    // An install from before the rename to Mooring: its variables and config first.
    fibo_core::legacy::adopt_old_env();
    fibo_core::legacy::move_old_config();
    apply_language();
    // Declared CLIs first: every later lookup of an Agent reads the registry.
    fibo_core::agent::install();
    fibo_core::rule::install();
    fibo_core::hook::install();
    let mut args = std::env::args().skip(1);
    let name = args.next();
    let cmd = match name.as_deref() {
        Some("-h" | "--help" | "help") => {
            println!("{}", control::usage());
            return Ok(());
        }
        None => return Err(usage_error(t!("缺少命令").into())),
        Some(name) => {
            control::command(name).ok_or_else(|| usage_error(t!("没有 {name} 这个命令", name)))?
        }
    };
    // Every command is checked against its declared flags before it acts, so a typo or a
    // `--help` never runs it with defaults.
    let rest: Vec<String> = args.collect();
    if control::check(cmd, &rest)? {
        println!("{}", cmd.usage());
        return Ok(());
    }
    let mut args = rest.into_iter();
    match cmd.name {
        "serve" => {
            raise_open_file_limit();
            let mut remaining = Vec::new();
            let mut days = 7;
            let mut takeover = false;
            while let Some(arg) = args.next() {
                if arg == "--takeover" {
                    takeover = true;
                } else if arg == "--recent-days" {
                    days = args
                        .next()
                        .ok_or_else(|| std::io::Error::other("missing days"))?
                        .parse::<u64>()
                        .map_err(std::io::Error::other)?;
                } else {
                    remaining.push(arg);
                }
            }
            let runtime = runtime_dir(remaining.into_iter())?;
            fibo_core::plugin_context::set_runtime(std::path::Path::new(&runtime));
            let serve = if takeover {
                fibo_host::serve_taking_over
            } else {
                fibo_host::serve_with_options
            };
            plugins_cmd::write_agents_md();
            fibo_host::cli_setup::install(std::path::Path::new(&runtime));
            fibo_host::start_mirrors();
            serve(
                std::path::Path::new(&runtime),
                fibo_host::recognition::roots()?,
                days,
            )
        }
        "scan" => scan(args),
        // Where another machine's window forwards to: this host's socket, as every
        // command finds it (`--runtime`, MOORING_RUNTIME, else the app's own runtime).
        "socket" => {
            println!("{}", control::Args::parse(args, cmd)?.socket.display());
            Ok(())
        }
        name if control::DRIVES.contains(&name) => control::run(cmd, args),
        "remote" => remote::run(args),
        "hook" => hook(args),
        "hooks" => hooks_cmd::run(args),
        "plugins" => plugins_cmd::run(args),
        "themes" => themes_cmd::run(args),
        "--skill" => {
            match args.next().as_deref() {
                None => print!("{}", control::skill()),
                Some("plugins") => print!("{}", control::plugins_skill()),
                Some("session") => print!("{}", control::session_skill()),
                Some(other) => {
                    return Err(std::io::Error::other(t!(
                        "没有叫 {other} 的说明；有：fibo --skill、fibo --skill plugins、fibo --skill session",
                        other
                    )))
                }
            }
            Ok(())
        }
        "--version" => {
            println!(
                "fibo protocol {}.{} experimental",
                fibo_host::PROTOCOL_MAJOR,
                fibo_host::PROTOCOL_MINOR
            );
            Ok(())
        }
        other => Err(usage_error(t!("{other} 还没有接上", other))),
    }
}
fn usage_error(what: String) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        t!("{what}\n用法：\n{usage}", what, usage = control::usage()),
    )
}

fn runtime_dir(mut args: impl Iterator<Item = String>) -> std::io::Result<String> {
    let mut runtime = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--runtime" => {
                runtime = Some(args.next().ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "missing --runtime directory",
                    )
                })?);
            }
            other => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown argument {other}"),
                ));
            }
        }
    }
    runtime.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "missing --runtime directory",
        )
    })
}

fn scan(args: impl Iterator<Item = String>) -> std::io::Result<()> {
    use fibo_core::protocol::*;
    let runtime = runtime_dir(args)?;
    match rpc(
        &std::path::Path::new(&runtime).join("workbench.sock"),
        &Request::RefreshAgents,
    )? {
        Reply::Workspace(w) => println!(
            "{} Agent sessions, {} managed terminals",
            w.agent_sessions.len(),
            w.terminals.len()
        ),
        other => {
            return Err(std::io::Error::other(format!(
                "unexpected scan response: {other:?}"
            )));
        }
    }
    Ok(())
}
fn hook(mut args: impl Iterator<Item = String>) -> std::io::Result<()> {
    use fibo_core::{model::Agent, protocol::*};
    use std::io::Read;
    let mut runtime = None;
    let mut agent = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--runtime" => runtime = args.next(),
            "--agent" => agent = args.next(),
            _ => {
                return Err(std::io::Error::other(format!(
                    "unknown hook argument: {arg}"
                )));
            }
        }
    }
    // Installed hooks run for every session; outside a Fibo terminal there is no host.
    let Some(runtime) = runtime.or_else(|| std::env::var(RUNTIME_ENV).ok()) else {
        return Ok(());
    };
    let agent = agent
        .as_deref()
        .and_then(Agent::from_executable)
        .ok_or_else(|| std::io::Error::other("hook requires --agent codex|claude|grok"))?;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(std::io::Error::other("hook payload exceeds 1 MiB"));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
    let cli_id = value["session_id"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("Hook has no session_id"))?;
    // An absent path is resolved from the daemon's SQLite index, never by walking HOME.
    let transcript = value["transcript_path"].as_str().unwrap_or("").to_owned();
    let request = Request::Hook {
        runtime: agent,
        pid: fibo_platform::process::parent(),
        cli_session_id: cli_id.into(),
        transcript,
        ended: value["hook_event_name"] == "SessionEnd",
        event: value["hook_event_name"].as_str().map(str::to_owned),
        facts: fibo_host::content::hook_facts(&value),
        terminal: std::env::var(TERMINAL_ENV).ok(),
        cwd: value["cwd"].as_str().unwrap_or("").to_owned(),
    };
    match rpc(
        &std::path::Path::new(&runtime).join("workbench.sock"),
        &request,
    )? {
        Reply::Workspace(_) => Ok(()),
        other => Err(std::io::Error::other(format!(
            "unexpected hook response: {other:?}"
        ))),
    }
}
