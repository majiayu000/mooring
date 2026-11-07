use mooring_core::{
    content::{Content, Turn},
    model::Agent,
};
use mooring_host::log_index::Index;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
#[path = "support/tied.rs"]
mod tied;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("mooring-more-agents-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        Self(p.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn append(path: &Path, mut v: Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    v["timestamp"] = mooring_core::content::now().into();
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "{v}"
    )
    .unwrap();
}
fn card(index: &Index, path: &Path) -> Content {
    index.card(path.to_str().unwrap()).unwrap().unwrap()
}

#[test]
fn omp_nested_artifacts_update_parents_and_pi_forks_stay_independent() {
    let f = Fixture::new();
    let omp = f.0.join("omp");
    let pi = f.0.join("pi");
    let main = omp.join("project/date_main.jsonl");
    let child = omp.join("project/date_main/0-Explore.jsonl");
    let nested = omp.join("project/date_main/0-Explore/0-Read.jsonl");
    let fork = pi.join("date_fork.jsonl");
    for (p, id, parent) in [
        (&main, "main", None),
        (&child, "child", None),
        (&nested, "nested", None),
        (&fork, "child", Some("main")),
    ] {
        append(p, json!({"type":"title","title":"Stored name"}));
        append(
            p,
            json!({"type":"session","id":id,"cwd":"/repo","parentSession":parent}),
        );
        append(
            p,
            json!({"type":"message","id":"u","message":{"role":"user","content":"Check code"}}),
        );
    }
    let mut index = Index::open(&f.0, vec![(Agent::Omp, omp), (Agent::Pi, pi)]).unwrap();
    index
        .update_many(
            [nested.clone(), child.clone(), main.clone(), fork.clone()],
            1,
        )
        .unwrap();
    assert_eq!(card(&index, &main).subagents.len(), 1);
    assert_eq!(card(&index, &child).parent, "main");
    assert_eq!(card(&index, &nested).parent, "child");
    assert!(
        card(&index, &fork).parent.is_empty(),
        "Pi parentSession is fork lineage"
    );
    append(
        &child,
        json!({"type":"message","id":"a","message":{"role":"assistant","content":[{"type":"toolCall","id":"q","name":"ask","arguments":{"questions":[{"question":"Continue?","options":[{"label":"Yes"}]}]}}],"stopReason":"toolUse"}}),
    );
    index.update_many([child.clone()], 2).unwrap();
    assert_eq!(card(&index, &main).turn, Turn::Waiting);
    assert!(card(&index, &main).waiting.contains("Continue?"));
    append(
        &child,
        json!({"type":"message","id":"r","message":{"role":"toolResult","toolCallId":"q","content":[{"type":"text","text":"Yes"}],"isError":false}}),
    );
    append(
        &nested,
        json!({"type":"message","id":"done","message":{"role":"assistant","content":[{"type":"text","text":"Done"}],"stopReason":"stop"}}),
    );
    append(
        &child,
        json!({"type":"message","id":"done","message":{"role":"assistant","content":[{"type":"text","text":"Done"}],"stopReason":"stop"}}),
    );
    index
        .update_many([nested.clone(), child.clone()], 3)
        .unwrap();
    assert_eq!(card(&index, &child).turn, Turn::Completed);
    assert!(card(&index, &main).subagents[0].ended.is_some());
    assert!(
        card(&index, &fork).subagents.is_empty(),
        "same child id across CLIs cannot link"
    );
    drop(index);
    let mut index = Index::open(
        &f.0,
        vec![(Agent::Omp, f.0.join("omp")), (Agent::Pi, f.0.join("pi"))],
    )
    .unwrap();
    index
        .update_many([nested.clone(), child.clone(), main.clone()], 4)
        .unwrap();
    assert_eq!(card(&index, &child).parent, "main");
    assert_eq!(card(&index, &child).turn, Turn::Completed);
}

#[test]
fn droid_called_sessions_reach_the_shared_parent_projection() {
    let f = Fixture::new();
    let root = f.0.join("droid");
    let main = root.join("main.jsonl");
    let child = root.join("child.jsonl");
    let fork = root.join("fork.jsonl");
    for (path, id, key) in [
        (&main, "main", "none"),
        (&child, "child", "callingSessionId"),
        (&fork, "fork", "parent"),
    ] {
        let mut h = json!({"type":"session_start","id":id,"cwd":"/repo","title":"Review"});
        h[key] = "main".into();
        append(path, h);
        append(
            path,
            json!({"type":"message","id":"u","message":{"role":"user","content":"Review code"}}),
        );
    }
    let mut index = Index::open(&f.0, vec![(Agent::Droid, root)]).unwrap();
    index
        .update_many([child.clone(), main.clone(), fork.clone()], 1)
        .unwrap();
    assert_eq!(card(&index, &main).subagents.len(), 1);
    assert_eq!(card(&index, &child).parent, "main");
    assert!(card(&index, &fork).parent.is_empty());
    append(
        &child,
        json!({"type":"message","id":"a","message":{"role":"assistant","content":[{"type":"text","text":"Still checking"}]}}),
    );
    index.update_many([child.clone()], 2).unwrap();
    assert_eq!(card(&index, &child).last_reply, "Still checking");
    assert!(
        card(&index, &main).subagents[0].running(),
        "intermediate text does not complete a child"
    );
}

/// Real daemon + automatic discovery + changed children + restart. No injected workspace,
/// real accounts, or installed CLIs. Overrides deliberately contain spaces and non-ASCII.
#[cfg(unix)]
#[test]
fn daemon_discovers_all_seven_sources_with_overrides_and_restarts() {
    use mooring_core::{
        model::Workspace,
        protocol::{rpc, Reply, Request},
    };
    use rusqlite::{params, Connection};
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    use tied::SpawnTied;
    const MAIN: &str = "c19a5748-897d-4836-a555-fce8a7797222";
    const CHILD: &str = "19817791-63b7-441c-ad06-25fe5e5293ba";
    // A short socket path on macOS (sockaddr_un has a 104-byte limit).
    let root = PathBuf::from("/tmp").join(format!(
        "mr-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    ));
    fs::create_dir_all(&root).unwrap();
    let f = Fixture(root.canonicalize().unwrap());
    let home = f.0.join("home");
    let cwd = f.0.join("work");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let custom = f.0.join("自定义 目录");
    let now = mooring_core::content::now();
    let database = custom.join("hermes/state.db");
    fs::create_dir_all(database.parent().unwrap()).unwrap();
    let db = Connection::open(&database).unwrap();
    db.execute_batch("CREATE TABLE sessions(id TEXT,cwd TEXT,source TEXT,parent_session_id TEXT,title TEXT,started_at REAL,ended_at REAL);
        CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT,role TEXT,content TEXT,tool_calls TEXT,tool_call_id TEXT,finish_reason TEXT,timestamp REAL);").unwrap();
    for (id, parent, source) in [(MAIN, None, "cli"), (CHILD, Some(MAIN), "subagent")] {
        db.execute(
            "INSERT INTO sessions VALUES(?1,?2,?3,?4,'Hermes',?5,NULL)",
            params![id, cwd.to_str().unwrap(), source, parent, now as i64],
        )
        .unwrap();
        db.execute(
            "INSERT INTO messages(session_id,role,content,timestamp) VALUES(?1,'user','Review',?2)",
            params![id, now as i64],
        )
        .unwrap();
    }
    let mut finishes = vec![];
    for (id, parent) in [(MAIN, None), (CHILD, Some(MAIN))] {
        let droid = custom.join(format!("factory/.factory/sessions/work/{id}.jsonl"));
        append(
            &droid,
            json!({"type":"session_start","id":id,"cwd":cwd,"callingSessionId":parent}),
        );
        append(
            &droid,
            json!({"type":"message","id":"u","message":{"role":"user","content":"Review"}}),
        );
        let dsh = custom.join(format!("dsh/sessions/work/{id}/session.v4.jsonl"));
        append(
            &dsh,
            json!({"type":"session","version":4,"id":id,"cwd":cwd,"createdAt":now*1000,
            "isSeeded":false,"origin":if parent.is_some(){"subagent"}else{"tui"},"delegationDepth":u8::from(parent.is_some()),"parentSession":parent}),
        );
        append(
            &dsh,
            json!({"type":"turn/start","time":now*1000,"data":{"turn":1}}),
        );
        append(
            &dsh,
            json!({"type":"user/message","time":now*1000,"data":{"source":{"kind":"user"},"content":"Review"}}),
        );
        if parent.is_some() {
            finishes.push((droid,json!({"type":"message","id":"a","message":{"role":"assistant","content":[{"type":"text","text":"Done"}],"stop_reason":"end_turn"}})));
            finishes.push((dsh,json!({"type":"turn/end","time":(now+1)*1000,"data":{"reason":{"kind":"completed"}}})));
        }
    }
    let pi = custom.join("pi/sessions/work/main.jsonl");
    let omp = custom.join("xdg/omp/sessions/work/main.jsonl");
    let omp_child = custom.join("xdg/omp/sessions/work/main/task.jsonl");
    for (path, id) in [(&pi, MAIN), (&omp, MAIN), (&omp_child, CHILD)] {
        append(path, json!({"type":"session","id":id,"cwd":cwd}));
        append(
            path,
            json!({"type":"message","id":"u","message":{"role":"user","content":"Review"}}),
        );
    }
    finishes.push((omp_child,json!({"type":"message","id":"a","message":{"role":"assistant","content":"Done","stopReason":"stop"}})));
    let kimi_session = custom.join(format!("kimi/sessions/work/session_{MAIN}"));
    fs::create_dir_all(&kimi_session).unwrap();
    fs::write(kimi_session.join("state.json"),json!({"workDir":cwd,"agents":{"main":{"type":"main"},"agent-0":{"type":"sub","parentAgentId":"main"}}}).to_string()).unwrap();
    for who in ["main", "agent-0"] {
        let path = kimi_session.join(format!("agents/{who}/wire.jsonl"));
        append(
            &path,
            json!({"type":"config.update","cwd":cwd,"time":now*1000}),
        );
        append(
            &path,
            json!({"type":"turn.prompt","input":"Review","time":now*1000}),
        );
        if who == "agent-0" {
            finishes.push((
                path,
                json!({"type":"turn.ended","reason":"completed","time":(now+1)*1000}),
            ));
        }
    }
    let gemini = custom.join("gemini/.gemini/tmp/project");
    fs::create_dir_all(&gemini).unwrap();
    fs::write(gemini.join(".project_root"), cwd.to_str().unwrap()).unwrap();
    for (path, id, kind) in [
        (gemini.join("chats/session-main.jsonl"), MAIN, "main"),
        (
            gemini.join(format!("chats/{MAIN}/{CHILD}.jsonl")),
            CHILD,
            "subagent",
        ),
    ] {
        append(&path, json!({"sessionId":id,"kind":kind}));
        append(&path, json!({"type":"user","id":"u","content":"Review"}));
        if kind == "subagent" {
            finishes.push((path,json!({"type":"gemini","id":"a","content":"","toolCalls":[{"id":"finish","name":"complete_task","status":"success"}]})));
        }
    }
    let start = || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_mooring"));
        cmd.arg("serve")
            .arg("--runtime")
            .arg(f.0.join("runtime"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("HOME", &home)
            .env("LANG", "en_US.UTF-8")
            .current_dir(&cwd)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        for (var, dir) in [
            ("HERMES_HOME", "hermes"),
            ("DSH_HOME", "dsh"),
            ("FACTORY_HOME_OVERRIDE", "factory"),
            ("PI_CODING_AGENT_DIR", "pi"),
            ("XDG_DATA_HOME", "xdg"),
            ("KIMI_CODE_HOME", "kimi"),
            ("GEMINI_CLI_HOME", "gemini"),
        ] {
            cmd.env(var, custom.join(dir));
        }
        cmd.spawn_tied().unwrap()
    };
    let wait = |child: &mut tied::Tied, completed: bool| {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(child.try_wait().unwrap().is_none(), "isolated host exited");
            let ready = |w: &Workspace| {
                [
                    Agent::Hermes,
                    Agent::Droid,
                    Agent::Pi,
                    Agent::Omp,
                    Agent::Dsh,
                    Agent::Kimi,
                    Agent::Gemini,
                ]
                .iter()
                .all(|agent| {
                    let id = if *agent == Agent::Kimi {
                        format!("session_{MAIN}")
                    } else {
                        MAIN.into()
                    };
                    w.agent_sessions
                        .iter()
                        .find(|s| s.runtime == *agent && s.cli_session_id == id)
                        .and_then(|s| w.content.get(&s.id))
                        .is_some_and(|c| {
                            let n = usize::from(*agent != Agent::Pi);
                            c.subagents.len() == n
                                && c.subagents
                                    .iter()
                                    .all(|s| s.ended.is_some() == completed && !s.stopped)
                        })
                })
            };
            if let Ok(Reply::Workspace(w)) =
                rpc(&f.0.join("runtime/workbench.sock"), &Request::Workspace)
            {
                if ready(&w) {
                    return w;
                }
                assert!(
                    Instant::now() < until,
                    "seven source discovery completed={completed}: sessions={:?}, content={:?}",
                    w.agent_sessions
                        .iter()
                        .map(|s| (&s.runtime, &s.cli_session_id))
                        .collect::<Vec<_>>(),
                    w.content
                        .values()
                        .map(|c| (&c.title, &c.subagents))
                        .collect::<Vec<_>>()
                );
            } else {
                assert!(Instant::now() < until, "host socket unavailable");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let mut host = start();
    wait(&mut host, false);
    // Parents do not change. Watchers and polling must bring child finishes to the parent.
    for (path, event) in finishes {
        append(&path, event);
    }
    db.execute("INSERT INTO messages(session_id,role,content,finish_reason,timestamp) VALUES(?1,'assistant','Done','stop',?2)",params![CHILD,(now+1) as i64]).unwrap();
    let completed = wait(&mut host, true);
    let hermes_id = completed
        .agent_sessions
        .iter()
        .find(|s| s.runtime == Agent::Hermes && s.cli_session_id == CHILD)
        .unwrap()
        .id
        .clone();
    let read_chat = |after| match rpc(
        &f.0.join("runtime/workbench.sock"),
        &Request::Chat {
            session: hermes_id.clone(),
            after,
            before: None,
            limit: 50,
            tools: false,
        },
    )
    .unwrap()
    {
        Reply::Chat {
            generation,
            messages,
            ..
        } => (generation, messages),
        other => panic!("unexpected chat: {other:?}"),
    };
    let (mut generation, messages) = read_chat(None);
    let cursor = messages.last().unwrap().seq;
    assert_eq!(messages.last().unwrap().text, "Done");
    for corrected in [Some("Redo"), None] {
        if let Some(text) = corrected {
            db.execute(
                "UPDATE messages SET content=?1 WHERE session_id=?2 AND role='assistant'",
                params![text, CHILD],
            )
            .unwrap();
        } else {
            db.execute(
                "DELETE FROM messages WHERE session_id=?1 AND role='assistant'",
                [CHILD],
            )
            .unwrap();
        }
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let (next, _) = read_chat(Some(cursor));
            if next != generation {
                generation = next;
                break;
            }
            assert!(
                Instant::now() < until,
                "rewrite did not invalidate chat cursor"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let (_, current) = read_chat(None);
        assert_eq!(
            current
                .iter()
                .filter(|m| !m.user)
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            corrected.into_iter().collect::<Vec<_>>()
        );
    }
    db.execute("INSERT INTO messages(session_id,role,content,finish_reason,timestamp) VALUES(?1,'assistant','Done','stop',?2)",params![CHILD,(now+1) as i64]).unwrap();
    wait(&mut host, true);
    host.kill_tree();
    drop(host);
    let mut restarted = start();
    wait(&mut restarted, true);
    eprintln!(
        "Observed seven sources, six children: running -> completed -> same results after restart"
    );
    restarted.kill_tree();
}

#[test]
fn persisted_cli_names_win_over_the_first_prompt_in_the_shared_title() {
    use mooring_core::model::{AgentSession, Workspace};
    for (agent, header, ask, rename) in [
        (
            Agent::Pi,
            json!({"type":"session","id":"main","cwd":"/repo"}),
            json!({"type":"message","id":"u","message":{"role":"user","content":"Original prompt"}}),
            json!({"type":"session_info","name":"Renamed work"}),
        ),
        (
            Agent::Omp,
            json!({"type":"session","id":"main","cwd":"/repo"}),
            json!({"type":"message","id":"u","message":{"role":"user","content":"Original prompt"}}),
            json!({"type":"session_info","name":"Renamed work"}),
        ),
        (
            Agent::Hermes,
            json!({"type":"session","session_id":"main","cwd":"/repo","source":"cli"}),
            json!({"role":"user","content":"Original prompt"}),
            json!({"type":"session","session_id":"main","cwd":"/repo","source":"cli","title":"Renamed work"}),
        ),
        (
            Agent::Dsh,
            json!({"type":"session","id":"main","cwd":"/repo"}),
            json!({"type":"user/message","data":{"source":{"kind":"user"},"content":"Original prompt"}}),
            json!({"type":"session/title","data":{"title":"Renamed work"}}),
        ),
    ] {
        let f = Fixture::new();
        let root = f.0.join("logs");
        let path = root.join("main.jsonl");
        append(&path, header);
        append(&path, ask);
        let mut index = Index::open(&f.0, vec![(agent, root.clone())]).unwrap();
        index.update_many([path.clone()], 1).unwrap();
        let session = AgentSession {
            id: "s".into(),
            runtime: agent,
            cli_session_id: "main".into(),
            terminal: None,
            current: false,
            task: None,
            outcome: Default::default(),
            updated: 0,
            cwd: "/repo".into(),
            transcript: path.to_string_lossy().into(),
            name: String::new(),
            binding: None,
        };
        let mut w = Workspace::default();
        w.content.insert(session.id.clone(), card(&index, &path));
        assert_eq!(w.session_title(&session), "Original prompt", "{agent:?}");
        append(&path, rename);
        index.update_many([path.clone()], 2).unwrap();
        w.content.insert(session.id.clone(), card(&index, &path));
        assert_eq!(w.session_title(&session), "Renamed work", "{agent:?}");
        index.flush().unwrap();
        drop(index);
        // A previous build persisted the name only in title, under parser rules 13.
        let db = rusqlite::Connection::open(f.0.join("workbench.sqlite")).unwrap();
        db.execute_batch("UPDATE session_content SET projection=json_set(projection,'$.name',''), parser=json_set(parser,'$.card.name','','$._rules',13)").unwrap();
        drop(db);
        let mut index = Index::open(&f.0, vec![(agent, root)]).unwrap();
        assert_eq!(card(&index, &path).name, "");
        index.initialize().unwrap();
        index.bootstrap().unwrap();
        w.content.insert(session.id.clone(), card(&index, &path));
        assert_eq!(
            w.session_title(&session),
            "Renamed work",
            "upgrade {agent:?}"
        );
    }
}
