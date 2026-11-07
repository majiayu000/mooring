//! Mooring's CLI boundary, not a test of model quality. Run in ordinary CI; no accounts.
use mooring_core::model::Agent;
use mooring_host::agent_adapter;
use std::{fs, path::PathBuf, process::Command};

const ID: &str = "c19a5748-897d-4836-a555-fce8a7797222";
const KIMI_ID: &str = "session_c19a5748-897d-4836-a555-fce8a7797222";

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn registered_executables_have_adapters_and_export_probe_commands() {
    for agent in Agent::all() {
        assert!(
            agent_adapter(agent).runs(agent.executable(), None),
            "{agent:?}"
        );
        assert!(
            !agent_adapter(agent).runs("unrelated-tool", None),
            "{agent:?}"
        );
    }
    // The optional local probe consumes the actual registry, not a second command list.
    if let Some(path) = std::env::var_os("MOORING_CLI_MANIFEST") {
        let tmp = Temp::new();
        fs::write(
            tmp.0.join("opencode.json"),
            include_str!("../../../plugins/opencode/agent.json"),
        )
        .unwrap();
        let (specs, report) = mooring_core::agent::load(&tmp.0, &tmp.0);
        assert!(report.iter().all(|r| r.result.is_ok()));
        let manifest: Vec<_> = specs
            .iter()
            .map(|s| {
                serde_json::json!({
                    "key":s.key, "executable":s.executable, "start":s.start,
            "resume":s.resume.replace("{id}", &mooring_core::model::shell_quote(if s.key=="Kimi" {KIMI_ID} else {ID}))
                })
            })
            .collect();
        fs::write(path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    }
}

#[test]
fn model_and_permission_shortcuts_are_only_offered_for_supported_clis() {
    for agent in Agent::all() {
        assert_eq!(
            mooring_host::agent_modes_switch(agent),
            matches!(agent, Agent::Codex | Agent::Claude),
            "{agent:?}"
        );
    }
}
fn selectors() -> Vec<(Agent, &'static str, &'static str)> {
    vec![
        (Agent::Claude, "--resume", ID),
        (Agent::Cursor, "--resume", ID),
        (Agent::Qoder, "--resume", ID),
        (Agent::Kimi, "--session", KIMI_ID),
        (Agent::Gemini, "--resume", ID),
        (Agent::Hermes, "--resume", ID),
        (Agent::Droid, "--resume", ID),
        (Agent::Pi, "--session", ID),
        (Agent::Omp, "--resume", ID),
        (Agent::Dsh, "--resume", ID),
    ]
}

#[test]
fn explicit_resume_selectors_bind_but_prompts_and_pickers_do_not() {
    let mut failures = vec![];
    for (agent, flag, id) in selectors() {
        let adapter = agent_adapter(agent);
        let empty_equal = format!("{flag}=");
        for (name, words, expected) in [
            ("explicit", vec![agent.executable(), flag, id], Some(id)),
            ("picker", vec![agent.executable(), flag], None),
            ("empty value", vec![agent.executable(), flag, "", id], None),
            (
                "empty equals",
                vec![agent.executable(), &empty_equal, id],
                None,
            ),
            (
                "next option",
                vec![agent.executable(), flag, "--model", "example"],
                None,
            ),
            (
                "literal prompt",
                vec![agent.executable(), "--", flag, id],
                None,
            ),
            (
                "conflicting selectors",
                vec![agent.executable(), flag, id, flag, "other"],
                None,
            ),
        ] {
            let actual = adapter.process_session(0, &args(&words)).0;
            if actual.as_deref() != expected {
                failures.push(format!("{agent:?} {name}: {actual:?} != {expected:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn alternate_selector_spellings_follow_the_cli_parser() {
    // Pi's own parser only supports a separate --session argument; do not invent = support.
    let mut failures = vec![];
    for (agent, flag, id) in selectors().into_iter().filter(|(a, _, _)| *a != Agent::Pi) {
        let equal = format!("{flag}={id}");
        let actual = agent_adapter(agent)
            .process_session(0, &args(&[agent.executable(), &equal]))
            .0;
        if actual.as_deref() != Some(id) {
            failures.push(format!("{agent:?} {equal}: {actual:?}"));
        }
    }
    for (agent, flag, id) in [
        (Agent::Claude, "-r", ID),
        (Agent::Qoder, "-r", ID),
        (Agent::Kimi, "-S", KIMI_ID),
        (Agent::Gemini, "-r", ID),
        (Agent::Hermes, "-r", ID),
        (Agent::Omp, "-r", ID),
    ] {
        let actual = agent_adapter(agent)
            .process_session(0, &args(&[agent.executable(), flag, id]))
            .0;
        if actual.as_deref() != Some(id) {
            failures.push(format!("{agent:?} {flag}: {actual:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fork_and_selector_shaped_option_values_are_not_the_running_session() {
    let claude = agent_adapter(Agent::Claude);
    assert_eq!(
        claude
            .process_session(0, &args(&["claude", "--resume", ID, "--fork-session"]))
            .0,
        None
    );
    // Valid free-form string values, not CLI selectors. The following real selector wins.
    for (agent, value_flag, selector, id) in [
        (Agent::Claude, "--append-system-prompt", "--resume", ID),
        (Agent::Kimi, "--prompt", "--session", KIMI_ID),
        (Agent::Pi, "--system-prompt", "--session", ID),
        (Agent::Hermes, "--model", "--resume", ID),
    ] {
        let literal = format!("{selector}={id}");
        assert_eq!(
            agent_adapter(agent)
                .process_session(0, &args(&[agent.executable(), value_flag, &literal]))
                .0,
            None,
            "{agent:?} option value"
        );
    }
}

#[test]
fn codex_resume_distinguishes_last_fork_and_option_values() {
    let adapter = agent_adapter(Agent::Codex);
    for words in [
        vec!["codex", "--model", "resume", "a prompt"],
        vec!["codex", "--", "resume", ID],
        vec!["codex", "resume", "--last"],
        vec!["codex", "fork", ID],
    ] {
        assert_eq!(
            adapter.process_session(0, &args(&words)).0,
            None,
            "{words:?}"
        );
    }
    assert_eq!(
        adapter
            .process_session(0, &args(&["codex", "-m", "example", "resume", ID]))
            .0
            .as_deref(),
        Some(ID)
    );
}

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("mooring CLI 空格 {}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn default_and_overridden_roots_are_isolated_per_process() {
    // Environment is process-global. Never set_var while another test is running.
    if let Ok(case) = std::env::var("MOORING_ROOT_CASE") {
        let (key, home, expected): (String, PathBuf, PathBuf) =
            serde_json::from_str(&case).unwrap();
        assert_eq!(
            agent_adapter(Agent::from_key(&key)).root(&home),
            expected,
            "{key}"
        );
        return;
    }
    let tmp = Temp::new();
    let home = tmp.0.join("home");
    let custom = tmp.0.join("custom 中文 空格");
    fs::create_dir_all(custom.join("omp")).unwrap();
    for (agent, var, default, suffix) in [
        (Agent::Codex, "CODEX_HOME", ".codex/sessions", "sessions"),
        (
            Agent::Claude,
            "CLAUDE_CONFIG_DIR",
            ".claude/projects",
            "projects",
        ),
        (Agent::Grok, "GROK_HOME", ".grok/sessions", "sessions"),
        (Agent::Cursor, "", ".cursor/projects", ""),
        (Agent::Qoder, "", ".qoder/projects", ""),
        (
            Agent::Kimi,
            "KIMI_CODE_HOME",
            ".kimi-code/sessions",
            "sessions",
        ),
        (
            Agent::Gemini,
            "GEMINI_CLI_HOME",
            ".gemini/tmp",
            ".gemini/tmp",
        ),
        (
            Agent::Droid,
            "FACTORY_HOME_OVERRIDE",
            ".factory/sessions",
            ".factory/sessions",
        ),
        (
            Agent::Pi,
            "PI_CODING_AGENT_DIR",
            ".pi/agent/sessions",
            "sessions",
        ),
        (
            Agent::Omp,
            "XDG_DATA_HOME",
            ".omp/agent/sessions",
            "omp/sessions",
        ),
        (Agent::Hermes, "", ".local/share/mooring/sources/Hermes", ""),
        (Agent::Dsh, "", ".local/share/mooring/sources/Dsh", ""),
    ] {
        for overridden in [false, true].into_iter().filter(|b| !*b || !var.is_empty()) {
            let expected = if overridden {
                custom.join(suffix)
            } else {
                home.join(default)
            };
            let mut cmd = Command::new(std::env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "default_and_overridden_roots_are_isolated_per_process",
                "--nocapture",
            ])
            .env_clear()
            .env(
                "MOORING_ROOT_CASE",
                serde_json::to_string(&(agent.key(), &home, &expected)).unwrap(),
            );
            if overridden {
                cmd.env(var, &custom);
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{agent:?} override={overridden}: {} {}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn launch_and_resume_reach_the_executable_with_literal_arguments() {
    use std::os::unix::fs::PermissionsExt;
    // Plugin installation is process-global too. This child runs this test alone.
    if std::env::var_os("MOORING_COMMAND_CHILD").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "launch_and_resume_reach_the_executable_with_literal_arguments",
                "--nocapture",
            ])
            .env("MOORING_COMMAND_CHILD", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let tmp = Temp::new();
    fs::write(
        tmp.0.join("opencode.json"),
        include_str!("../../../plugins/opencode/agent.json"),
    )
    .unwrap();
    assert!(mooring_core::agent::install_from(&tmp.0, &tmp.0));
    fs::write(
        tmp.0.join("uuidgen"),
        "#!/bin/sh\nprintf 'fixture-password\\n'\n",
    )
    .unwrap();
    fs::set_permissions(tmp.0.join("uuidgen"), fs::Permissions::from_mode(0o755)).unwrap();
    // Execute generated commands against inert argv recorders, never a real model CLI.
    for agent in Agent::all() {
        let bin = tmp.0.join(agent.executable());
        let source = if agent.executable() == "opencode" {
            "#!/bin/sh\n[ \"$OPENCODE_SERVER_PASSWORD\" = fixture-password ] || exit 43\nprintf '%s\\0' \"$@\"\n"
        } else {
            "#!/bin/sh\nprintf '%s\\0' \"$@\"\n"
        };
        fs::write(&bin, source).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let run = |command: &str| {
            let out = Command::new("/bin/sh")
                .args(["-c", command])
                .env_clear()
                .env("PATH", &tmp.0)
                .current_dir(&tmp.0)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{agent:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
                .split(|b| *b == 0)
                .filter(|b| !b.is_empty())
                .map(|s| String::from_utf8(s.to_vec()).unwrap())
                .collect::<Vec<_>>()
        };
        let start = if agent.executable() == "opencode" {
            vec!["--port", "0"]
        } else if agent == Agent::Dsh {
            vec!["tui"]
        } else {
            vec![]
        };
        // printf with no arguments emits an empty value; the recorder ignores it above.
        assert_eq!(run(&agent.command()), start, "{agent:?} start");
        let hostile_id = "会话 ' $(touch injected) `touch injected` ; &";
        let expected = match agent {
            _ if agent.executable() == "opencode" => vec!["--port", "0", "--session", hostile_id],
            Agent::Codex => vec!["resume", hostile_id],
            Agent::Kimi | Agent::Pi => vec!["--session", hostile_id],
            Agent::Dsh => vec!["tui", "--resume", hostile_id],
            _ => vec!["--resume", hostile_id],
        };
        assert_eq!(
            run(&agent.resume_command(hostile_id, &[])),
            expected,
            "{agent:?} resume"
        );
        assert!(!tmp.0.join("injected").exists());
        if matches!(agent, Agent::Codex | Agent::Claude) {
            let value = "example model ; $(touch injected)";
            let launch =
                agent_adapter(agent).launch_flags(&args(&[agent.executable(), "--model", value]));
            let mut words = vec!["--model", value];
            words.extend(expected);
            assert_eq!(run(&agent.resume_command(hostile_id, &launch)), words);
            assert!(!tmp.0.join("injected").exists());
        }
    }
}

#[test]
fn carried_model_and_permission_flags_exclude_prompts_selectors_and_remote_routing() {
    for (agent, words, expected) in [
        (
            Agent::Codex,
            vec![
                "codex",
                "--remote",
                "ws://example",
                "-c",
                "model_reasoning_effort=\"high\"",
                "--model=example",
                "resume",
                ID,
                "--",
                "--yolo",
            ],
            vec!["-c", "model_reasoning_effort=\"high\"", "--model=example"],
        ),
        (
            Agent::Claude,
            vec![
                "claude",
                "--model",
                "example",
                "--permission-mode",
                "plan",
                "--resume",
                ID,
                "--",
                "--dangerously-skip-permissions",
            ],
            vec!["--model", "example", "--permission-mode", "plan"],
        ),
    ] {
        assert_eq!(
            agent_adapter(agent).launch_flags(&args(&words)),
            expected,
            "{agent:?}"
        );
    }
}
