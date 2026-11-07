use super::*;

/// A home every platform calls absolute.
fn home() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\home\u")
    } else {
        PathBuf::from("/home/u")
    }
}

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mooring-agents-{name}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
const AIDER: &str = r#"{"api":1,"id":"aider","name":"Aider","executable":"aider",
    "start":"aider","resume":"aider --restore {id}",
    "transcripts":{"root":"~/.aider/s","session":"/sid","cwd":"/cwd","role":"/role","text":"/content"}}"#;

#[test]
fn built_in_keys_and_commands_are_what_was_stored_before() {
    assert_eq!(serde_json::to_string(&Agent::Claude).unwrap(), "\"Claude\"");
    assert_eq!(
        serde_json::from_str::<Agent>("\"Grok\"").unwrap(),
        Agent::Grok
    );
    assert_eq!(format!("{:?}", Agent::Codex), "Codex");
    assert_eq!(Agent::Codex.command(), "codex");
    assert_eq!(
        Agent::Claude.resume_command("x", &[]),
        "claude --resume 'x'"
    );
    let launch = ["-c", "model_reasoning_effort=\"high\"", "-m", "gpt-5"].map(String::from);
    assert_eq!(
        Agent::Codex.resume_command("id", &launch),
        "command codex -c 'model_reasoning_effort=\"high\"' -m gpt-5 resume 'id'"
    );
    assert_eq!(
        Agent::Claude.resume_command("id", &["--dangerously-skip-permissions".into()]),
        "command claude --dangerously-skip-permissions --resume 'id'"
    );
    assert_eq!(Agent::Claude.command(), "claude");
}

#[test]
fn added_agents_resume_with_their_actual_cli_flags() {
    for (agent, start, resume) in [
        (Agent::Hermes, "hermes", "hermes --resume 'session'"),
        (Agent::Droid, "droid", "droid --resume 'session'"),
        (Agent::Pi, "pi", "pi --session 'session'"),
        (Agent::Omp, "omp", "omp --resume 'session'"),
        (Agent::Dsh, "dsh tui", "dsh tui --resume 'session'"),
    ] {
        assert_eq!(agent.command(), start);
        assert_eq!(agent.resume_command("session", &[]), resume);
        assert_eq!(Agent::from_executable(agent.executable()), Some(agent));
    }
}

#[test]
fn a_key_no_longer_registered_reads_back_as_itself() {
    let gone: Agent = serde_json::from_str("\"removed-cli\"").unwrap();
    assert_eq!(gone.key(), "removed-cli");
    assert_eq!(gone.name(), "removed-cli");
    assert!(gone.spec().is_none());
    assert_eq!(gone, Agent::from_key("removed-cli"));
}

#[test]
fn a_declaration_registers_after_the_built_ins() {
    let d = dir("ok");
    std::fs::write(d.join("aider.json"), AIDER).unwrap();
    let (specs, report) = load(&d, &home());
    assert_eq!(
        specs.iter().map(|s| s.key).collect::<Vec<_>>(),
        [
            "Codex", "Claude", "Grok", "Cursor", "Qoder", "Hermes", "Droid", "Pi", "Omp", "Dsh",
            "aider"
        ]
    );
    let aider = &specs[builtin().len()];
    assert_eq!(
        aider.transcripts.as_ref().unwrap().root,
        home().join(".aider/s")
    );
    assert_eq!(render(&aider.resume, Some("a b")), "aider --restore 'a b'");
    assert_eq!(report[0].result, Ok("aider".into()));
}

#[test]
fn a_later_file_replaces_an_earlier_one_with_the_same_id() {
    let d = dir("override");
    std::fs::write(d.join("a.json"), AIDER).unwrap();
    std::fs::write(
        d.join("b.json"),
        AIDER.replace("\"Aider\"", "\"Aider (mine)\""),
    )
    .unwrap();
    let (specs, report) = load(&d, &home());
    assert_eq!(specs.len(), builtin().len() + 1);
    assert_eq!(specs[builtin().len()].name, "Aider (mine)");
    assert!(report[0].result.as_ref().unwrap_err().contains("b.json"));
    assert_eq!(report[1].result, Ok("aider".into()));
}

#[test]
fn unusable_declarations_are_reported_and_the_rest_still_load() {
    let d = dir("bad");
    let cases = [
        ("1.json", "not json".to_owned()),
        ("2.json", AIDER.replace("\"api\":1", "\"api\":3")),
        (
            "3.json",
            AIDER.replace("\"id\":\"aider\"", "\"id\":\"claude\""),
        ),
        (
            "4.json",
            AIDER.replace("\"executable\":\"aider\"", "\"executable\":\"codex\""),
        ),
        (
            "5.json",
            AIDER.replace("\"id\":\"aider\"", "\"id\":\"Bad Id\""),
        ),
        ("6.json", AIDER.replace("\"/sid\"", "\"sid\"")),
        ("7.json", AIDER.replace("\"~/.aider/s\"", "\"rel/s\"")),
        (
            "8.json",
            AIDER.replace("\"api\":1,", "\"api\":1,\"extra\":true,"),
        ),
        ("9.json", AIDER.to_owned()),
    ];
    for (name, body) in &cases {
        std::fs::write(d.join(name), body).unwrap();
    }
    let (specs, report) = load(&d, &home());
    assert_eq!(
        specs.len(),
        builtin().len() + 1,
        "only the good one registers"
    );
    let refused = report.iter().filter(|r| r.result.is_err()).count();
    assert_eq!(refused, 8, "{report:#?}");
    assert!(report[1].result.as_ref().unwrap_err().contains("api 3"));
    assert!(
        report[2]
            .result
            .as_ref()
            .unwrap_err()
            .contains("Claude Code")
    );
}

#[test]
fn api_2_events_load_and_are_checked() {
    let d = dir("events");
    let base = |api: u32, events: &str| {
        format!(
            r#"{{"api":{api},"id":"gemini","name":"Gemini","executable":"gemini","start":"gemini","resume":"gemini --resume {{id}}",
            "transcripts":{{"root":"~/c","session":"/s","cwd":"/c","events":{events}}}}}"#
        )
    };
    let ok = r#"[{"when":{"/type":"turn_end"},"turn":"completed"},
        {"when":{"/type":"tool_call"},"tool":{"id":"/id","name":"/n","kind":"command","command":"/cmd"}}]"#;
    std::fs::write(d.join("a.json"), base(2, ok)).unwrap();
    let (specs, report) = load(&d, &home());
    assert!(report[0].result.is_ok(), "{report:?}");
    let events = &specs.last().unwrap().transcripts.as_ref().unwrap().events;
    assert_eq!(events.len(), 2);
    assert!(events[0].matches(&serde_json::json!({"type": "turn_end"})));
    assert!(!events[0].matches(&serde_json::json!({"type": "turn_start"})));
    for (body, why) in [
        (base(1, ok), "需要 api 2"),
        (base(3, ok), "只认 api 1 到 2"),
        (
            base(2, r#"[{"when":{},"turn":"completed"}]"#),
            "when 不能为空",
        ),
        (base(2, r#"[{"when":{"/t":"x"}}]"#), "turn、tool 或 result"),
        (
            base(
                2,
                r#"[{"when":{"/t":"x"},"tool":{"id":"/i","name":"/n","kind":"command"}}]"#,
            ),
            "command 指针",
        ),
        (
            base(2, r#"[{"when":{"t":"x"},"turn":"running"}]"#),
            "JSON 指针",
        ),
    ] {
        std::fs::write(d.join("a.json"), &body).unwrap();
        let (_, report) = load(&d, &home());
        let err = report[0].result.as_ref().unwrap_err();
        assert!(err.contains(why), "{why}: {err}");
    }
}

#[test]
fn no_directory_means_built_ins_only() {
    let (specs, report) = load(Path::new("/nonexistent/mooring/agents"), &home());
    assert_eq!(specs.len(), builtin().len());
    assert!(report.is_empty());
}
