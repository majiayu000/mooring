use super::*;

#[test]
fn daemon_codex_dialog_gone_from_screen_is_settled_by_a_read_only_thread_read() {
    use std::io::Write;
    let h = Host::start();
    h.setup();
    let methods = fake_codex_control(
        &h.root.join("codex-control.sock"),
        serde_json::json!({"type": "idle"}),
    );
    let [a] = ids();
    daemon_codex(
        &h,
        "p0",
        "proj",
        &[step("请检查发布脚本里的回滚步骤是否完整", &a, None)],
        "",
    );
    wait_for(&h, "the thread binds to its pane", |w| {
        current_is(w, "p0", &a)
    });
    // The log asks the user; the TUI never draws the question.
    {
        let mut log = fs::OpenOptions::new()
            .append(true)
            .open(h.root.join(format!("logs/codex/{a}.jsonl")))
            .unwrap();
        let row = serde_json::json!({"timestamp": mooring_core::content::now(), "type": "event_msg",
            "payload": {"type": "request_user_input", "questions": [{"question": "回滚到哪个版本？"}]}});
        writeln!(log, "{row}").unwrap();
    }
    let turn = |w: &Workspace| {
        w.current_agent("p0")
            .and_then(|s| w.content.get(&s.id))
            .map(|c| (c.turn, c.signal.clone()))
    };
    wait_for(&h, "the logged question waits on the user", |w| {
        turn(w).is_some_and(|t| t.0 == mooring_core::content::Turn::Waiting)
    });
    // A keystroke echoes, so the screen settles again without the question on it.
    let (mut stream, _vt, writable) = h.attach("p0", true);
    assert!(writable);
    send(
        &mut stream,
        CONTROL,
        &Request::Input {
            bytes: b"x".to_vec(),
            lossy: false,
        },
    )
    .unwrap();
    assert!(matches!(ack(&mut stream), Reply::Ok));
    let until = Instant::now() + Duration::from_secs(20);
    let settled = loop {
        let t = turn(&h.workspace()).unwrap();
        if t.0 != mooring_core::content::Turn::Waiting {
            break t;
        }
        assert!(
            Instant::now() < until,
            "idle thread status must settle the dialog"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_eq!(
        settled,
        (mooring_core::content::Turn::Interrupted, "官方状态".into())
    );
    let methods = methods.lock().unwrap().clone();
    assert!(methods.contains(&"thread/read".to_owned()), "{methods:?}");
    assert!(
        methods
            .iter()
            .all(|m| matches!(m.as_str(), "initialize" | "initialized" | "thread/read")),
        "read-only: never subscribes, got {methods:?}"
    );
    assert!(
        recognition(&h).official.is_empty(),
        "a working query leaves no notice"
    );
}

#[test]
fn an_unreachable_codex_daemon_is_a_visible_notice_not_a_silent_guess() {
    use std::io::Write;
    let h = Host::start();
    h.setup();
    // No control socket at all: the daemon is not running.
    let [a] = ids();
    daemon_codex(
        &h,
        "p0",
        "proj",
        &[step("检查一下回滚脚本是否覆盖了数据库迁移", &a, None)],
        "",
    );
    wait_for(&h, "the thread binds to its pane", |w| {
        current_is(w, "p0", &a)
    });
    {
        let mut log = fs::OpenOptions::new()
            .append(true)
            .open(h.root.join(format!("logs/codex/{a}.jsonl")))
            .unwrap();
        let row = serde_json::json!({"timestamp": mooring_core::content::now(), "type": "event_msg",
            "payload": {"type": "request_user_input", "questions": [{"question": "回滚到哪个版本？"}]}});
        writeln!(log, "{row}").unwrap();
    }
    let (mut stream, _vt, _) = h.attach("p0", true);
    send(
        &mut stream,
        CONTROL,
        &Request::Input {
            bytes: b"x".to_vec(),
            lossy: false,
        },
    )
    .unwrap();
    assert!(matches!(ack(&mut stream), Reply::Ok));
    let until = Instant::now() + Duration::from_secs(20);
    let notice = loop {
        if let Some(n) = recognition(&h).official.get("Codex").cloned() {
            break n;
        }
        assert!(Instant::now() < until, "the failed query must be reported");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        notice.starts_with("Codex 官方状态查询不可用，已改为只看日志和画面"),
        "{notice}"
    );
    let w = h.workspace();
    let s = w.current_agent("p0").unwrap();
    assert_eq!(
        w.content[&s.id].turn,
        mooring_core::content::Turn::Waiting,
        "without the official answer the logged question stands"
    );
}

#[test]
fn codex_subagent_logs_keep_the_parent_working_and_carry_its_dialog() {
    use std::io::Write;
    let h = Host::start();
    h.setup();
    let [a] = ids();
    daemon_codex(
        &h,
        "p0",
        "proj",
        &[step("把三个模块的迁移分给子 Agent 并行处理", &a, None)],
        "",
    );
    wait_for(&h, "the thread binds to its pane", |w| {
        current_is(w, "p0", &a)
    });
    let logs = h.root.join("logs/codex");
    let row = |payload: serde_json::Value| {
        format!(
            "{}\n",
            serde_json::json!({"timestamp": mooring_core::content::now(), "type": "event_msg", "payload": payload})
        )
    };
    // The parent's turn ends with its subagent still running.
    fs::OpenOptions::new()
        .append(true)
        .open(logs.join(format!("{a}.jsonl")))
        .unwrap()
        .write_all(
            row(serde_json::json!({"type": "task_complete", "last_agent_message": "已派出子任务"}))
                .as_bytes(),
        )
        .unwrap();
    // Like the real daemon, the subagent's log stays open while it works.
    let kid = uuid::Uuid::new_v4().to_string();
    let mut child = fs::File::create(logs.join(format!("{kid}.jsonl"))).unwrap();
    let header = serde_json::json!({"type": "session_meta", "payload": {"id": kid, "cwd": "/tmp",
        "source": {"subagent": {"thread_spawn": {"parent_thread_id": a, "depth": 1, "agent_path": "/root/migrate"}}},
        "agent_nickname": "Hypatia", "parent_thread_id": a}});
    child.write_all(format!("{header}\n").as_bytes()).unwrap();
    child
        .write_all(row(serde_json::json!({"type": "task_started", "turn_id": "k1"})).as_bytes())
        .unwrap();
    let card = |w: &Workspace| {
        w.current_agent("p0")
            .and_then(|s| w.content.get(&s.id).map(|c| (s.clone(), c.clone())))
    };
    wait_for(
        &h,
        "a running subagent keeps the stopped parent working",
        |w| {
            card(w).is_some_and(|(s, c)| {
                c.turn == mooring_core::content::Turn::Completed
                    && c.subagents.len() == 1
                    && c.subagents[0].id == kid
                    && c.subagents[0].name == "Hypatia"
                    && c.subagents[0].running()
                    && mooring_core::status::of(w, &s, mooring_core::content::now())
                        == mooring_core::status::Status::Running
            })
        },
    );
    let item = |payload: serde_json::Value| {
        format!(
            "{}\n",
            serde_json::json!({"timestamp": mooring_core::content::now(), "type": "response_item", "payload": payload})
        )
    };
    child
        .write_all(
            item(serde_json::json!({"type": "function_call", "name": "request_user_input", "call_id": "q1",
                "arguments": serde_json::json!({"questions": [{"question": "是否运行迁移？"}]}).to_string()}))
                .as_bytes(),
        )
        .unwrap();
    wait_for(&h, "the subagent's question is the parent's", |w| {
        card(w).is_some_and(|(_, c)| {
            c.turn == mooring_core::content::Turn::Waiting
                && c.waiting.starts_with("子 Agent Hypatia：")
        })
    });
    // The user answers in the parent's TUI; the subagent records the answer and finishes.
    child
        .write_all(
            item(serde_json::json!({"type": "function_call_output", "call_id": "q1", "output": "运行"}))
                .as_bytes(),
        )
        .unwrap();
    child
        .write_all(
            row(serde_json::json!({"type": "task_complete", "last_agent_message": "迁移完成"}))
                .as_bytes(),
        )
        .unwrap();
    wait_for(&h, "the finished subagent releases the parent", |w| {
        card(w).is_some_and(|(s, c)| {
            c.turn == mooring_core::content::Turn::Completed
                && c.subagents.len() == 1
                && !c.subagents[0].running()
                && !c.subagents[0].stopped
                && mooring_core::status::of(w, &s, mooring_core::content::now())
                    != mooring_core::status::Status::Running
        })
    });
    drop(child);
}

#[test]
fn daemon_codex_same_cwd_panes_bind_to_their_own_screen_and_exit_clears_current() {
    let h = Host::start();
    h.setup();
    let [a, b] = ids();
    daemon_codex(
        &h,
        "p0",
        "proj",
        &[step("请检查登录页跳转逻辑并修复回归问题", &a, None)],
        "",
    );
    daemon_codex(
        &h,
        "p1",
        "proj",
        &[step("给设置页补上深色模式的截图测试用例", &b, None)],
        "",
    );
    wait_for(
        &h,
        "each same-cwd thread binds to the pane showing its prompt",
        |w| current_is(w, "p0", &a) && current_is(w, "p1", &b),
    );
    assert!(recognition(&h).terminal_hints.is_empty());
    h.call(Request::Stop { id: "p0".into() }).unwrap();
    wait_for(&h, "a screen-bound Codex exit clears current", |w| {
        w.current_agent("p0").is_none() && current_is(w, "p1", &b)
    });
}

#[test]
fn a_spinning_title_shows_a_turn_the_log_has_not_and_stops_with_it() {
    use mooring_core::content::Turn;
    use std::io::Write;
    let h = Host::start();
    h.setup();
    let [a] = ids();
    daemon_codex(
        &h,
        "p0",
        "proj",
        &[step("排查左栏滚动卡顿的原因并修复这个性能回归", &a, None)],
        "",
    );
    let turn = |w: &Workspace| {
        w.current_agent("p0")
            .and_then(|s| w.content.get(&s.id))
            .map(|c| (c.turn, c.signal.clone()))
    };
    wait_for(&h, "the prompt binds and its turn runs", |w| {
        turn(w).is_some_and(|(t, _)| t == Turn::Running)
    });
    let log = h.root.join(format!("logs/codex/{a}.jsonl"));
    fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(
            format!(
                "{}\n",
                serde_json::json!({"timestamp":mooring_core::content::now(),"type":"event_msg","payload":{"type":"task_complete"}})
            )
            .as_bytes(),
        )
        .unwrap();
    wait_for(&h, "the log ends the turn", |w| {
        turn(w).is_some_and(|(t, _)| t == Turn::Completed)
    });
    let (mut stream, _, writable) = h.attach("p0", false);
    assert!(writable);
    let mut typed = |line: &str| {
        send(
            &mut stream,
            CONTROL,
            &Request::Input {
                bytes: format!("{line}\r").into_bytes(),
                lossy: false,
            },
        )
        .unwrap();
        assert!(matches!(ack(&mut stream), Reply::Ok));
    };
    // A turn the log has not recorded yet, or whose start fell before the part it read.
    typed("title:⠋ proj");
    wait_for(&h, "a spinning title shows the turn running", |w| {
        turn(w) == Some((Turn::Running, "标题".into()))
    });
    // The log never said this turn ran, so a still title leaves the log's word standing.
    typed("title:proj");
    wait_for(&h, "a still title gives the log its say back", |w| {
        turn(w).is_some_and(|(t, _)| t == Turn::Completed)
    });
    // With the title off, the working line the TUI ticks on screen says the same.
    typed("say:• Working (3s • esc to interrupt)");
    wait_for(&h, "a working line on screen shows the turn running", |w| {
        turn(w) == Some((Turn::Running, "画面在跑".into()))
    });
    typed("clear");
    wait_for(
        &h,
        "the working line gone gives the log its say back",
        |w| turn(w).is_some_and(|(t, _)| t == Turn::Completed),
    );
}

#[test]
fn daemon_codex_short_prompts_bind_to_the_pane_where_enter_was_pressed() {
    let h = Host::start();
    h.setup();
    let [a, b] = ids();
    let gates = [h.root.join("gate-0"), h.root.join("gate-1")];
    // "继续" is too short to be screen evidence; only the Enter press can tell the panes apart.
    daemon_codex(&h, "p0", "proj", &[step("继续", &a, Some(&gates[0]))], "");
    daemon_codex(&h, "p1", "proj", &[step("继续", &b, Some(&gates[1]))], "");
    std::thread::sleep(Duration::from_secs(2));
    for (pane, gate) in ["p0", "p1"].iter().zip(&gates) {
        let (mut stream, _, writable) = h.attach(pane, false);
        assert!(writable);
        send(
            &mut stream,
            CONTROL,
            &Request::Input {
                bytes: b"\r".to_vec(),
                lossy: false,
            },
        )
        .unwrap();
        assert!(matches!(ack(&mut stream), Reply::Ok));
        // Codex timestamps the prompt when Enter arrives, not while preparing its TUI.
        let at = mooring_core::content::now();
        let seed = h.root.join(format!("{pane}-seed-0"));
        let rows = fs::read_to_string(&seed)
            .unwrap()
            .lines()
            .map(|line| {
                let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
                row["timestamp"] = at.into();
                row.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(seed, format!("{rows}\n")).unwrap();
        fs::write(gate, "").unwrap();
        // Far enough apart that each logged prompt meets only its own Enter.
        std::thread::sleep(Duration::from_secs(4));
    }
    wait_for(
        &h,
        "each short prompt binds to the pane that submitted it",
        |w| current_is(w, "p0", &a) && current_is(w, "p1", &b),
    );
}

#[test]
fn daemon_codex_threads_started_in_the_same_millisecond_bind_correctly() {
    let h = Host::start();
    h.setup();
    let gate = h.root.join("go");
    let [a, b] = ids();
    daemon_codex(
        &h,
        "p0",
        "proj",
        &[step(
            "同一时刻启动的第一个线程：整理发布说明",
            &a,
            Some(&gate),
        )],
        "",
    );
    daemon_codex(
        &h,
        "p1",
        "proj",
        &[step(
            "同一时刻启动的第二个线程：修复构建脚本",
            &b,
            Some(&gate),
        )],
        "",
    );
    wait_for(&h, "both panes running before the shared start", |w| {
        w.terminals
            .iter()
            .filter(|t| t.state == ProcessState::Running)
            .count()
            == 2
    });
    fs::write(&gate, "").unwrap();
    wait_for(&h, "simultaneous threads bind to their own panes", |w| {
        current_is(w, "p0", &a) && current_is(w, "p1", &b)
    });
}

#[test]
fn daemon_codex_new_hands_the_pane_to_the_newer_thread_only() {
    let h = Host::start();
    h.setup();
    let gate = h.root.join("new");
    let [old, new] = ids();
    daemon_codex(
        &h,
        "p0",
        "proj",
        &[
            step("先排查通知点击后没有跳转的原因", &old, None),
            step("新会话：把通知投递日志整理成证据表格", &new, Some(&gate)),
        ],
        "",
    );
    // An idle Codex in the same cwd never received a prompt and must not take the /new thread.
    daemon_codex(&h, "idle", "proj", &[], "");
    wait_for(&h, "first thread binds", |w| current_is(w, "p0", &old));
    fs::write(&gate, "").unwrap();
    wait_for(&h, "/new thread takes over its own pane", |w| {
        current_is(w, "p0", &new)
            && w.agent_sessions
                .iter()
                .any(|s| s.cli_session_id == old && !s.current)
    });
    assert!(h.workspace().current_agent("idle").is_none());
}

#[test]
fn daemon_codex_identical_prompts_stay_unbound_until_a_distinct_follow_up() {
    let h = Host::start();
    h.setup();
    let gate = h.root.join("follow");
    let [a, b] = ids();
    let same = "请对比两个模型对这段代码的重构建议";
    daemon_codex(
        &h,
        "p0",
        "proj",
        &[
            step(same, &a, None),
            step("第一个窗格的追问：只保留性能相关建议", &a, Some(&gate)),
        ],
        "",
    );
    daemon_codex(
        &h,
        "p1",
        "proj",
        &[
            step(same, &b, None),
            step("第二个窗格的追问：只保留可读性相关建议", &b, Some(&gate)),
        ],
        "",
    );
    wait_for(&h, "both identical threads are indexed", |w| {
        [&a, &b]
            .iter()
            .all(|id| w.agent_sessions.iter().any(|s| &s.cli_session_id == *id))
    });
    std::thread::sleep(Duration::from_secs(2));
    let w = h.workspace();
    assert!(
        w.current_agent("p0").is_none() && w.current_agent("p1").is_none(),
        "identical prompts must not bind"
    );
    fs::write(&gate, "").unwrap();
    wait_for(
        &h,
        "distinct follow-ups bind each thread to its own pane",
        |w| current_is(w, "p0", &a) && current_is(w, "p1", &b),
    );
}

#[test]
fn unbound_daemon_codex_shows_title_state_and_no_daemon_shows_nothing() {
    let h = Host::start();
    h.setup();
    daemon_codex(&h, "busy", "⠋ proj", &[], "");
    daemon_codex(&h, "plain", "⠋ proj", &[], "--no-daemon");
    // Hints are computed after a submitted command; the fixture has none, so probe via input.
    for pane in ["busy", "plain"] {
        let (mut stream, _, _) = h.attach(pane, false);
        send(
            &mut stream,
            CONTROL,
            &Request::Input {
                bytes: b"\n".to_vec(),
                lossy: false,
            },
        )
        .unwrap();
        assert!(matches!(ack(&mut stream), Reply::Ok));
    }
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if recognition(&h)
            .terminal_hints
            .get("busy")
            .map(String::as_str)
            == Some("仅显示状态：工作中；发下一句后自动关联对话")
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "{:?}",
            recognition(&h).terminal_hints
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(!recognition(&h).terminal_hints.contains_key("plain"));
}

/// Neither log moves and nothing draws: only the host's own timer can end this dialog.
#[test]
fn a_subagent_dialog_ends_by_itself_once_the_subagent_has_gone_silent() {
    let h = Host::start();
    h.setup();
    let _methods = fake_codex_control(
        &h.root.join("codex-control.sock"),
        serde_json::json!({"type": "idle"}),
    );
    silent_subagent_releases_parent(&h);
}
/// The same with no daemon to ask (`--no-daemon`, or it stopped).
#[test]
fn a_subagent_dialog_ends_by_itself_without_an_official_answer() {
    let h = Host::start();
    h.setup();
    silent_subagent_releases_parent(&h);
}
fn silent_subagent_releases_parent(h: &Host) {
    use std::io::Write;
    let [a] = ids();
    daemon_codex(
        h,
        "p0",
        "proj",
        &[step("派一个子 Agent 跑测试", &a, None)],
        "",
    );
    wait_for(h, "the thread binds to its pane", |w| {
        current_is(w, "p0", &a)
    });
    let logs = h.root.join("logs/codex");
    let row = |at: u64, ty: &str, payload: serde_json::Value| {
        format!(
            "{}\n",
            serde_json::json!({"timestamp": at, "type": ty, "payload": payload})
        )
    };
    let now = mooring_core::content::now();
    fs::OpenOptions::new()
        .append(true)
        .open(logs.join(format!("{a}.jsonl")))
        .unwrap()
        .write_all(
            row(
                now,
                "event_msg",
                serde_json::json!({"type": "task_complete"}),
            )
            .as_bytes(),
        )
        .unwrap();
    wait_for(h, "the parent's turn ends", |w| {
        let s = w.current_agent("p0");
        s.and_then(|s| w.content.get(&s.id))
            .is_some_and(|c| c.turn == mooring_core::content::Turn::Completed)
    });
    // The child wrote its last line just under 15 minutes ago: it goes stale during the test,
    // after the checks that follow a screen change or a binding (about 5 s) are over.
    let last = now - (900 - 20);
    let kid = uuid::Uuid::new_v4().to_string();
    let header = serde_json::json!({"type": "session_meta", "payload": {"id": kid, "cwd": "/tmp",
        "source": {"subagent": {"thread_spawn": {"parent_thread_id": a, "depth": 1, "agent_path": "/root/test"}}},
        "agent_nickname": "Hypatia", "parent_thread_id": a}});
    let lines = format!("{header}\n")
        + &row(
            last,
            "event_msg",
            serde_json::json!({"type": "task_started", "turn_id": "k1"}),
        )
        + &row(
            last,
            "response_item",
            serde_json::json!({"type": "function_call", "name": "exec_command",
            "call_id": "c1", "arguments": serde_json::json!({"cmd": "npm test"}).to_string()}),
        )
        + &row(
            last,
            "response_item",
            serde_json::json!({"type": "function_call_output", "call_id": "c1",
            "output": "Process exited with code 1: requires approval"}),
        );
    fs::write(logs.join(format!("{kid}.jsonl")), lines).unwrap();
    let turn = |w: &Workspace| {
        w.current_agent("p0")
            .and_then(|s| w.content.get(&s.id))
            .map(|c| c.turn)
    };
    wait_for(h, "the subagent's dialog is the parent's", |w| {
        turn(w) == Some(mooring_core::content::Turn::Waiting)
    });
    let until = Instant::now() + Duration::from_secs(60);
    while turn(&h.workspace()) == Some(mooring_core::content::Turn::Waiting) {
        assert!(
            Instant::now() < until,
            "the parent stays on a dialog nobody is asking"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    // It stood until the child really went stale; the parent's own Idle did not end it.
    assert!(mooring_core::content::now() >= last + 900);
    assert_eq!(
        turn(&h.workspace()),
        Some(mooring_core::content::Turn::Completed)
    );
}
