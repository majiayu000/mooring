//! What the phone was still missing against its design, through `mooring remote --stdio`:
//! cards with branch, running time, model and permission; the board's todos; 「接着上次的
//! 对话」; 设为空间; 今天的 tokens; a diff's test results (docs/phone-app/v2-plan.md).
#[path = "remote_v2/common.rs"]
mod common;
use common::*;
use fibo_core::{content::Source, model::*, protocol::*};
use fibo_host::content::Parser;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn card(r: &mut Remote, terminal: &str) -> Value {
    let list = r.ok(json!({ "op": "list" }));
    let cards = list["terminals"].as_array().unwrap();
    (cards.iter().find(|c| c["id"] == terminal).cloned()).unwrap_or(Value::Null)
}
fn until(what: &str, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !ok() {
        assert!(Instant::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn user(uuid: &str, cwd: &Path, text: &str) -> Value {
    json!({"type":"user","uuid":uuid,"cwd":cwd,"entrypoint":"cli","userType":"external","message":{"content":text}})
}
fn iso(at: u64) -> String {
    chrono::DateTime::from_timestamp(at as i64, 0)
        .unwrap()
        .to_rfc3339()
}

/// `list` is called on every burst of events: the branch comes from the repository's files.
#[test]
fn cards_say_branch_time_doing_model_and_permission_for_a_running_turn() {
    let h = Host::start();
    let repo = scratch(&h, "repo");
    git(&repo, &["init", "-q"]);
    git(&repo, &["checkout", "-q", "-b", "feat/card"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
    let linked = h.root.join("linked");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/linked",
            &linked.to_string_lossy(),
        ],
    );
    let began = fibo_core::content::now() - 720;
    let rows = [
        json!({"type":"permission-mode","permissionMode":"acceptEdits"}),
        {
            let mut u = user("u1", &linked, "fix the keyboard");
            u["timestamp"] = iso(began).into();
            u
        },
        json!({"type":"assistant","uuid":"a1","message":{"model":"claude-opus-5-5","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build -p app"}}]}}),
        json!({"type":"user","uuid":"r1","toolUseResult":{},"message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}),
    ];
    h.claude("work", &linked, &rows, "sleep 60");
    h.launch("plain", &repo, "sleep 60");
    h.launch("elsewhere", Path::new("/tmp"), "sleep 60");
    let mut r = Remote::open(&h, Some("view"), &[]);
    until("the turn never ran", || {
        card(&mut r, "work")["status"] == "running"
    });
    let c = card(&mut r, "work");
    assert_eq!(c["branch"], "feat/linked", "a linked work tree: {c}");
    assert_eq!(c["running_since"], began, "{c}");
    assert_eq!(c["doing"], "运行 cargo build -p app", "{c}");
    assert_eq!(c["model"], "claude-opus-5-5", "{c}");
    assert_eq!(c["permission"], "自动接受编辑", "{c}");
    assert_eq!(card(&mut r, "plain")["branch"], "feat/card");
    assert!(card(&mut r, "plain").get("running_since").is_none());
    // A checkout shows within the cache's few seconds.
    git(&repo, &["checkout", "-q", "-b", "feat/next"]);
    until("the checkout never showed", || {
        card(&mut r, "plain")["branch"] == "feat/next"
    });
    assert!(
        card(&mut r, "elsewhere").get("branch").is_none(),
        "/tmp is no repository"
    );
}

/// Claude Code's and Codex's own words for a mode, from lines as real logs have them.
#[test]
fn model_and_permission_come_from_each_clis_log() {
    let codex = |p: &mut Parser, policy: &str, sandbox: &str| {
        let line = json!({"type":"turn_context","timestamp":"2026-11-05T00:00:00Z","payload":{"model":"gpt-6.1-sol","approval_policy":policy,"sandbox_policy":{"type":sandbox}}});
        p.add(Agent::Codex, line.to_string().as_bytes(), Source::default());
    };
    let mut p = Parser::default();
    codex(&mut p, "never", "danger-full-access");
    assert_eq!(
        (p.card.model.as_str(), p.card.permission.as_str()),
        ("gpt-6.1-sol", "全部放行")
    );
    codex(&mut p, "on-request", "workspace-write");
    assert_eq!(p.card.permission, "逐个确认", "a later turn's policy wins");
    assert_eq!(p.card.unknown_records, 0);

    let claude = |p: &mut Parser, v: Value| {
        p.add(Agent::Claude, v.to_string().as_bytes(), Source::default());
    };
    let mut p = Parser::default();
    claude(
        &mut p,
        json!({"type":"permission-mode","permissionMode":"bypassPermissions"}),
    );
    assert_eq!(p.card.permission, "全部放行");
    claude(
        &mut p,
        json!({"type":"user","permissionMode":"plan","timestamp":"2026-11-05T00:00:00Z","message":{"content":"hi"}}),
    );
    assert_eq!(p.card.permission, "只做计划");
    let reply = |model: &str| json!({"type":"assistant","timestamp":"2026-11-05T00:00:00Z","message":{"model":model,"content":[{"type":"text","text":"ok"}]}});
    claude(&mut p, reply("claude-opus-5-5"));
    claude(&mut p, reply("<synthetic>"));
    assert_eq!(p.card.model, "claude-opus-5-5", "a canned line names none");
    claude(
        &mut p,
        json!({"type":"assistant","isSidechain":true,"message":{"model":"claude-haiku","content":[]}}),
    );
    assert_eq!(p.card.model, "claude-opus-5-5", "a sub-agent's is its own");
}

#[test]
fn chat_tools_say_how_long_the_work_took() {
    let h = Host::start();
    let work = scratch(&h, "work");
    let t = fibo_core::content::now() - 3600;
    let at = |mut v: Value, s: u64| {
        v["timestamp"] = iso(t + s).into();
        v
    };
    let said = |text: &str| json!({"type":"assistant","message":{"content":[{"type":"text","text":text}]}});
    let call = |id: &str| json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":"ls"}}]}});
    let rows = [
        at(user("u1", &work, "fix it"), 0),
        at(call("c1"), 30),
        at(said("done"), 150),
        // Slept for half an hour and woke by itself: only its own work counts.
        at(call("c2"), 1950),
        at(said("woke"), 1980),
    ];
    h.claude("chat", &work, &rows, "sleep 60");
    let mut r = Remote::open(&h, Some("view"), &[]);
    let mut page = Value::Null;
    until("chat never filled", || {
        page = r.ok(json!({ "op": "chat", "terminal": "chat", "tools": true }));
        page["messages"].as_array().unwrap().len() == 3
    });
    let m = &page["messages"];
    assert_eq!(m[1]["tools"]["took"], 150, "from the ask: {page}");
    assert_eq!(m[2]["tools"]["took"], 30, "from its first call: {page}");
    // A page starting there still knows the message before it.
    let seq = m[1]["seq"].as_i64().unwrap();
    let one = r.ok(
        json!({ "op": "chat", "terminal": "chat", "tools": true, "before": seq + 1, "limit": 1 }),
    );
    assert_eq!(one["messages"][0]["tools"]["took"], 150);
}

#[test]
fn open_resumes_the_last_conversation_as_the_window_does_and_never_runs_it_twice() {
    let h = Host::start();
    let work = scratch(&h, "work");
    let cwd = work.to_string_lossy().into_owned();
    let bin = scratch(&h, "bin");
    let said = h.root.join("said.txt");
    let pwned = h.root.join("pwned");
    let claude = bin.join("claude");
    fs::write(
        &claude,
        format!(
            "#!/bin/sh\npwd >> {0}\nprintf '%s\\n' \"$#\" \"$@\" >> {0}\necho RESUMED\nexec sleep 60\n",
            shell_quote(&said.to_string_lossy())
        ),
    )
    .unwrap();
    let shell = bin.join("fake-shell");
    fs::write(
        &shell,
        format!(
            "#!/bin/sh\nwhile [ \"$1\" = -l ] || [ \"$1\" = -i ]; do shift; done\nPATH={}:$PATH exec /bin/sh \"$@\"\n",
            shell_quote(&bin.to_string_lossy())
        ),
    )
    .unwrap();
    for f in [&claude, &shell] {
        Command::new("chmod").arg("+x").arg(f).status().unwrap();
    }
    let mut r = Remote::open(&h, None, &[("SHELL", shell.as_path())]);
    let last = json!({ "op": "lastsession", "cwd": cwd, "agent": "claude" });
    assert!(r.ok(last.clone())["session"].is_null(), "nothing here yet");

    // A conversation that ran here and has ended.
    let ended = [
        user("u1", &work, "the old task"),
        json!({"type":"assistant","uuid":"a1","message":{"model":"claude","stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}}),
    ];
    h.claude("old", &work, &ended, "sleep 60");
    h.call(Request::Stop { id: "old".into() }).unwrap();
    let mut found = Value::Null;
    until("the ended conversation never showed", || {
        found = r.ok(last.clone())["session"].clone();
        found.is_object() && found.get("terminal").is_none()
    });
    assert_eq!(found["title"], "the old task", "{found}");
    assert!(found["updated"].as_u64().unwrap() > 0);
    let id = found["id"].as_str().unwrap().to_owned();
    let other = r.ok(json!({ "op": "lastsession", "cwd": cwd, "agent": "codex" }));
    assert!(other["session"].is_null(), "another Agent's is not it");

    let prompt = format!("it's \"quoted\" $(touch {}) `id`\nsecond", pwned.display());
    let open = json!({ "op": "open", "resume": id, "cwd": "/tmp", "agent": "codex", "prompt": prompt, "request": "r1" });
    let first = r.ok(open.clone());
    let terminal = first["terminal"].as_str().unwrap().to_owned();
    assert!(first.get("existing").is_none(), "{first}");
    h.wait_screen(&terminal, "RESUMED");
    let again = r.ok(open);
    assert_eq!(
        again["terminal"], first["terminal"],
        "a resend opens nothing"
    );
    std::thread::sleep(Duration::from_millis(300));
    let w = h.workspace();
    let session = w.agent_sessions.iter().find(|s| s.id == id).unwrap();
    let args = fs::read_to_string(&said).unwrap();
    let dir = fs::canonicalize(&work).unwrap();
    assert_eq!(
        args,
        format!(
            "{}\n3\n--resume\n{}\n{prompt}\n",
            dir.display(),
            session.cli_session_id
        ),
        "its own directory, its own Agent, the prompt one argument, once"
    );
    assert!(!pwned.exists(), "nothing in the prompt ran");
    assert!(!args.contains("--no-daemon"));
    assert!(w
        .tabs
        .iter()
        .any(|t| t.layout.as_ref().is_some_and(|l| l.contains(&terminal))));

    // Still running: its terminal comes back and nothing opens.
    let live = [user("u2", &work, "still going")];
    h.claude("live", &work, &live, "sleep 60");
    let mut now = Value::Null;
    until("the live conversation never led", || {
        now = r.ok(last.clone())["session"].clone();
        now["terminal"] == "live"
    });
    let count = h.workspace().terminals.len();
    let open = r.ok(json!({ "op": "open", "resume": now["id"], "request": "r2" }));
    assert_eq!(
        (&open["terminal"], &open["existing"]),
        (&json!("live"), &json!(true))
    );
    assert_eq!(h.workspace().terminals.len(), count);

    let e = r.error(json!({ "op": "open", "resume": "nope", "request": "r3" }));
    assert!(e.contains("找不到"), "{e}");
    let mut answer = Remote::open(&h, Some("answer"), &[]);
    assert!(answer.error(last).contains("不能 lastsession"));
    assert!(answer
        .error(json!({ "op": "open", "resume": id, "request": "r4" }))
        .contains("不能 open"));
}

/// The folder a resumed conversation belongs to is gone: no starting it anywhere else.
#[test]
fn a_conversation_whose_folder_is_gone_does_not_resume_elsewhere() {
    let h = Host::start();
    let work = scratch(&h, "gone");
    h.claude("old", &work, &[user("u1", &work, "task")], "sleep 60");
    let id = h.workspace().current_agent("old").unwrap().id.clone();
    h.call(Request::Stop { id: "old".into() }).unwrap();
    let mut r = Remote::open(&h, None, &[]);
    until("never ended", || {
        h.workspace()
            .terminals
            .iter()
            .any(|t| t.id == "old" && t.state != ProcessState::Running)
    });
    fs::remove_dir_all(&work).unwrap();
    let e = r.error(json!({ "op": "open", "resume": id, "request": "r1" }));
    assert!(e.contains("目录已经不在"), "{e}");
}

#[test]
fn addspace_makes_one_space_for_a_folder_for_the_full_role_only() {
    let h = Host::start();
    let dir = scratch(&h, "my-project");
    let path = dir.to_string_lossy().into_owned();
    let mut r = Remote::open(&h, None, &[]);
    assert!(r.ops().contains(&"addspace".into()));
    let add = json!({ "op": "addspace", "path": path, "request": "a1" });
    let space = r.ok(add.clone());
    assert_eq!(space["name"], "my-project");
    assert_eq!(space["cwd"], path);
    assert_eq!(r.ok(add), space, "a resend makes nothing new");
    let again = r.ok(json!({ "op": "addspace", "path": format!("{path}/"), "request": "a2" }));
    assert_eq!(again["id"], space["id"], "a folder has one space");
    let w = h.workspace();
    assert_eq!(w.spaces.iter().filter(|s| s.cwd == path).count(), 1);
    let list = r.ok(json!({ "op": "list" }));
    assert!(list["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == space["id"]));
    let e = r.error(
        json!({ "op": "addspace", "path": h.root.join("nope").to_string_lossy(), "request": "a3" }),
    );
    assert!(e.contains("目录不存在"), "{e}");
    let mut answer = Remote::open(&h, Some("answer"), &[]);
    assert!(!answer.ops().contains(&"addspace".into()));
    let e = answer.error(json!({ "op": "addspace", "path": path, "request": "a4" }));
    assert!(e.contains("不能 addspace"), "{e}");
}

/// The phone names the space: the tab goes there even when the folder is outside it (it used
/// to fall into the first space). Without one, as from an older phone, the folder decides.
#[test]
fn open_puts_the_tab_in_the_space_named_and_without_one_where_the_folder_is() {
    let h = Host::start();
    let a = scratch(&h, "alpha").to_string_lossy().into_owned();
    let elsewhere = scratch(&h, "elsewhere").to_string_lossy().into_owned();
    let mut r = Remote::open(&h, None, &[]);
    let alpha = r.ok(json!({ "op": "addspace", "path": a, "request": "s1" }));
    let tab_space = |h: &Host, tab: &Value| {
        let w = h.workspace();
        (w.tabs.iter().find(|t| t.id == tab.as_str().unwrap())).map(|t| t.space.clone())
    };

    let picked =
        r.ok(json!({ "op": "open", "cwd": elsewhere, "space": alpha["id"], "request": "o1" }));
    assert_eq!(picked["space"], alpha["id"]);
    assert_eq!(
        tab_space(&h, &picked["tab"]).as_deref(),
        alpha["id"].as_str()
    );

    let inside = r.ok(json!({ "op": "open", "cwd": a, "request": "o2" }));
    assert_eq!(
        inside["space"], alpha["id"],
        "no space named: the folder's space"
    );

    let outside = r.ok(json!({ "op": "open", "cwd": elsewhere, "request": "o3" }));
    assert_ne!(
        outside["space"], alpha["id"],
        "no space named, folder in none: the first space"
    );

    let e = r.error(json!({ "op": "open", "cwd": elsewhere, "space": "gone", "request": "o4" }));
    assert!(e.contains("空间已经不在"), "{e}");

    // A space with no folder matches no `cwd`: only its name puts the tab there, as the
    // window's 「＋」 and 「在这里开一个终端」 send it.
    h.call(Request::Edit {
        edit: Edit::AddSpace(Space {
            id: "blank".into(),
            name: "Blank".into(),
            cwd: String::new(),
        }),
        request: None,
    })
    .unwrap();
    let blank = r.ok(json!({ "op": "open", "space": "blank", "request": "o5" }));
    assert_eq!(blank["space"], "blank");
    assert_eq!(tab_space(&h, &blank["tab"]).as_deref(), Some("blank"));
    let unnamed = r.ok(json!({ "op": "open", "request": "o6" }));
    assert_ne!(unnamed["space"], "blank", "no space named: the first space");
}

/// Today's tokens from logs shaped as the CLIs write them, read once and then by their growth.
#[test]
fn usage_counts_each_reply_once_and_only_since_the_moment_asked() {
    let h = Host::start();
    let claude_home = scratch(&h, "claude-home");
    let codex_home = scratch(&h, "codex-home");
    let project = claude_home.join("projects/p/session/subagents");
    fs::create_dir_all(&project).unwrap();
    let day = codex_home.join("sessions/2026/11/05");
    fs::create_dir_all(&day).unwrap();
    let now = fibo_core::content::now();
    let since = now - 3600;
    let reply = |id: &str, at: u64, output: u64| {
        json!({"type":"assistant","timestamp":iso(at),"message":{"id":id,"usage":{"input_tokens":1,"cache_creation_input_tokens":2,"cache_read_input_tokens":100,"output_tokens":output}}}).to_string()
    };
    // One reply over two lines, one from yesterday, one in a sub-agent's log.
    let main = claude_home.join("projects/p/main.jsonl");
    let lines = [
        reply("m1", now - 60, 7),
        reply("m1", now - 60, 7),
        reply("old", now - 90_000, 1000),
    ];
    fs::write(&main, lines.join("\n") + "\n").unwrap();
    fs::write(
        project.join("agent-1.jsonl"),
        reply("m2", now - 30, 3) + "\n",
    )
    .unwrap();
    let count = |at: u64, total: u64, input: u64, cached: u64, output: u64| {
        json!({"timestamp":iso(at),"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":total},"last_token_usage":{"input_tokens":input,"cached_input_tokens":cached,"output_tokens":output}}}}).to_string()
    };
    let rollout = [
        count(now - 90_000, 100, 90, 0, 10),
        count(now - 50, 160, 50, 30, 10),
        count(now - 50, 160, 50, 30, 10),
    ];
    fs::write(day.join("rollout.jsonl"), rollout.join("\n") + "\n").unwrap();
    let env = [
        ("CLAUDE_CONFIG_DIR", claude_home.as_path()),
        ("CODEX_HOME", codex_home.as_path()),
    ];
    let mut view = Remote::open(&h, Some("view"), &env);
    assert!(view.ops().contains(&"usage".into()));
    let u = view.ok(json!({ "op": "usage", "since": since }));
    assert_eq!(u["partial"], false, "{u}");
    assert_eq!(u["since"], since);
    assert_eq!(
        u["agents"],
        json!([
            { "agent": "claude", "agent_name": "Claude Code", "tokens": 10 + 6, "cached": 200 },
            { "agent": "codex", "agent_name": "Codex", "tokens": 30, "cached": 30 },
        ]),
        "{u}"
    );
    assert!(view.error(json!({ "op": "usage" })).contains("since"));
}

/// A diff says what the conversation last tested, from the exit code in its log, and whether
/// it changed files after.
#[test]
fn a_diff_names_the_tests_its_conversation_ran_and_whether_edits_came_after() {
    let h = Host::start();
    let repo = scratch(&h, "repo");
    git(&repo, &["init", "-q"]);
    fs::write(repo.join("a.txt"), "one\n").unwrap();
    let t = fibo_core::content::now() - 600;
    let timed = |mut v: Value, s: u64| {
        v["timestamp"] = iso(t + s).into();
        v
    };
    let call = |id: &str, name: &str, input: Value| json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":name,"input":input}]}});
    let result = |id: &str, text: &str, error: bool| json!({"type":"user","toolUseResult":{},"message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":error,"content":text}]}});
    let rows = [
        timed(user("u1", &repo, "fix it"), 0),
        // The Agent saying it passed is no record of anything.
        timed(
            json!({"type":"assistant","message":{"content":[{"type":"text","text":"所有测试都通过了"}]}}),
            5,
        ),
        timed(
            call("c1", "Bash", json!({"command":"cargo test -p app"})),
            10,
        ),
        timed(
            result("c1", "test result: ok. 8 passed; 0 failed", false),
            20,
        ),
        timed(
            call(
                "e1",
                "Edit",
                json!({"file_path":"/r/a.rs","old_string":"x","new_string":"y"}),
            ),
            30,
        ),
        timed(result("e1", "ok", false), 31),
        timed(
            call("c2", "Bash", json!({"command":"bun run typecheck"})),
            40,
        ),
        timed(result("c2", "src/a.ts: error", true), 50),
    ];
    h.claude("work", &repo, &rows, "sleep 60");
    let mut r = Remote::open(&h, Some("view"), &[]);
    let mut d = Value::Null;
    until("no checks showed", || {
        d = r.ok(json!({ "op": "diff", "terminal": "work" }));
        d["checks"].as_array().is_some_and(|c| c.len() == 2)
    });
    let c = &d["checks"];
    assert_eq!(c[0]["command"], "bun run typecheck", "{d}");
    assert_eq!(
        (&c[0]["ok"], &c[0]["stale"]),
        (&json!(false), &json!(false))
    );
    assert_eq!(c[1]["command"], "cargo test -p app", "{d}");
    assert_eq!((&c[1]["ok"], &c[1]["stale"]), (&json!(true), &json!(true)));
    assert!(c[1]["counts"].as_str().unwrap().contains("8 passed"), "{d}");
    // A lesser role is told what the turn changed, from its log: names and counts only.
    assert_eq!(d["summary"], true, "{d}");
    let files = d["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{d}");
    assert_eq!(files[0]["path"], "/r/a.rs", "{d}");
    assert_eq!(
        (&files[0]["added"], &files[0]["removed"]),
        (&json!(1), &json!(1))
    );
    assert_eq!(files[0]["patch"], "", "{d}");
    // A directory names no conversation, and a lesser role reads no folder by git (#125).
    let by_cwd = r.error(json!({ "op": "diff", "cwd": repo.to_string_lossy() }));
    assert_eq!(by_cwd, "这台设备只能看终端里这一轮改了哪些文件");
    let mut full = Remote::open(&h, Some("full"), &[]);
    let by_cwd = full.ok(json!({ "op": "diff", "cwd": repo.to_string_lossy() }));
    assert_eq!(
        by_cwd["checks"],
        json!([]),
        "a directory names no conversation"
    );
}

/// Opening by hand in a space put aside brings it back, or what opened would vanish from the
/// phone at once; with no directory named it opens somewhere the phone already sees.
#[test]
fn open_in_a_space_put_aside_brings_it_back() {
    let h = Host::start();
    let aside = scratch(&h, "aside");
    let keep = scratch(&h, "keep");
    for (id, dir) in [("a-aside", &aside), ("z-keep", &keep)] {
        h.call(Request::Edit {
            edit: Edit::AddSpace(Space {
                id: id.into(),
                name: id.into(),
                cwd: dir.to_string_lossy().into_owned(),
            }),
            request: None,
        })
        .unwrap();
    }
    h.call(Request::Edit {
        edit: Edit::SetSpaceQuiet {
            id: "a-aside".into(),
            quiet: true,
        },
        request: None,
    })
    .unwrap();
    let mut r = Remote::open(&h, None, &[]);
    let list = r.ok(json!({ "op": "list" }));
    assert!(
        (list["spaces"].as_array().unwrap().iter()).all(|s| s["id"] != "a-aside"),
        "{list}"
    );
    let opened = r.ok(json!({ "op": "open", "request": "r1" }));
    let terminal = opened["terminal"].as_str().unwrap().to_owned();
    let w = h.workspace();
    assert!(
        w.space_quiet("a-aside"),
        "opening elsewhere leaves it alone"
    );
    assert!(!w.terminal_quiet(&terminal), "opened in an attended space");

    let opened = r.ok(json!({ "op": "open", "cwd": aside.to_string_lossy(), "request": "r2" }));
    assert!(!h.workspace().space_quiet("a-aside"), "it is back");
    let terminal = opened["terminal"].as_str().unwrap();
    assert_eq!(h.workspace().terminal_home(terminal), Some("a-aside"));
    let list = r.ok(json!({ "op": "list" }));
    assert!(
        (list["terminals"].as_array().unwrap().iter()).any(|t| t["id"] == terminal),
        "and the phone sees what it opened: {list}"
    );
}
