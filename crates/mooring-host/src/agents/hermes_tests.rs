use super::*;
use crate::log_index::Index;
use mooring_core::{content::Turn, model::Agent};
use rusqlite::params;

#[test]
fn hermes_question_and_reply_need_their_own_completion_evidence() {
    let mut p = crate::content::Parser::default();
    let mut offset = 0;
    let mut feed = |p: &mut crate::content::Parser, v: Value| {
        offset += 1;
        p.add(
            Agent::Hermes,
            &serde_json::to_vec(&v).unwrap(),
            mooring_core::content::Source {
                offset,
                ..Default::default()
            },
        );
    };
    feed(&mut p, json!({"role":"user","content":"Check code"}));
    feed(
        &mut p,
        json!({"role":"assistant","tool_calls":[{"id":"q","function":{"name":"clarify","arguments":"{\"question\":\"Continue?\"}"}}]}),
    );
    assert_eq!(p.card.turn, Turn::Waiting);
    assert!(p.card.waiting.contains("Continue?"));
    feed(
        &mut p,
        json!({"role":"tool","tool_call_id":"q","content":"Yes"}),
    );
    assert_eq!(p.card.turn, Turn::Running);
    feed(&mut p, json!({"role":"assistant","content":"Checking"}));
    assert_eq!(p.card.turn, Turn::Unknown);
    feed(
        &mut p,
        json!({"role":"assistant","content":"Checked","finish_reason":"stop"}),
    );
    assert_eq!(p.card.turn, Turn::Completed);
}

#[test]
fn hermes_rewrites_replay_without_stale_answers_or_invented_children() {
    let root = std::env::temp_dir().join(format!("mooring-hermes-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let source = root.join("state.db");
    let db = Connection::open(&source).unwrap();
    db.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY,cwd TEXT,source TEXT,parent_session_id TEXT,title TEXT,started_at REAL,ended_at REAL);
        CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT,role TEXT,content TEXT,tool_calls TEXT,tool_call_id TEXT,finish_reason TEXT,timestamp REAL,active INTEGER DEFAULT 1,compacted INTEGER DEFAULT 0);").unwrap();
    let now = mooring_core::content::now() as i64;
    for (id, kind, parent) in [
        ("main", "cli", None),
        ("child", "subagent", Some("main")),
        ("fork", "cli", Some("main")),
        ("compressed", "tui", Some("main")),
        ("cron", "cron", None),
    ] {
        db.execute(
            "INSERT INTO sessions VALUES(?1,'/repo',?2,?3,?1,?4,NULL)",
            params![id, kind, parent, now - 10],
        )
        .unwrap();
        db.execute("INSERT INTO messages(session_id,role,content,timestamp) VALUES(?1,'user','Check the change',?2)",params![id,now-9]).unwrap();
    }
    db.execute("INSERT INTO messages(session_id,role,content,finish_reason,timestamp) VALUES('child','assistant','checked','stop',?1)",[now-8]).unwrap();
    let out = root.join("mirror");
    let paths = snapshot(&source, &out).unwrap();
    assert_eq!(paths.len(), 4, "cron is not an interactive desktop session");
    let mut index = Index::open(&root, vec![(Agent::Hermes, out.clone())]).unwrap();
    index.update_many(paths.clone(), 1).unwrap();
    let path = |id: &str| {
        out.join(format!(
            "{}.jsonl",
            id.bytes().map(|b| format!("{b:02x}")).collect::<String>()
        ))
    };
    let card = |i: &Index, id: &str| i.card(path(id).to_str().unwrap()).unwrap().unwrap();
    let main = card(&index, "main");
    assert_eq!(main.subagents.len(), 1);
    assert_eq!(main.subagents[0].id, "child");
    assert_eq!(card(&index, "child").turn, Turn::Completed);
    assert!(card(&index, "fork").parent.is_empty());
    assert!(card(&index, "compressed").parent.is_empty());
    assert!(
        snapshot(&source, &out).unwrap().is_empty(),
        "unchanged snapshots do not churn inodes"
    );

    // A same-length in-place edit must be noticed, not just count/length fingerprints.
    db.execute(
        "UPDATE messages SET content='revised' WHERE role='assistant'",
        [],
    )
    .unwrap();
    let changed = snapshot(&source, &out).unwrap();
    assert_eq!(changed, vec![path("child")]);
    index.update_many(changed, 2).unwrap();
    assert_eq!(card(&index, "child").last_reply, "revised");
    // Rewind hides old output even when no new row or later timestamp exists.
    db.execute("UPDATE messages SET active=0 WHERE role='assistant'", [])
        .unwrap();
    index
        .update_many(snapshot(&source, &out).unwrap(), 3)
        .unwrap();
    assert!(card(&index, "child").last_reply.is_empty());
    assert_eq!(card(&index, "child").turn, Turn::Running);
    assert!(card(&index, "main").subagents[0].running());
    drop(index);
    let mut index = Index::open(&root, vec![(Agent::Hermes, out.clone())]).unwrap();
    index.update_many([path("child"), path("main")], 4).unwrap();
    assert!(
        card(&index, "child").last_reply.is_empty(),
        "restart does not resurrect withdrawn output"
    );
    db.execute("DELETE FROM sessions WHERE id='child'", [])
        .unwrap();
    snapshot(&source, &out).unwrap();
    assert!(!path("child").exists());
    // The core CLI schema omits frontend cwd/visibility metadata. Its absence
    // must not prevent reading sessions, nor cause a guessed working directory.
    db.execute_batch("ALTER TABLE sessions DROP COLUMN cwd; ALTER TABLE messages DROP COLUMN active; ALTER TABLE messages DROP COLUMN compacted;").unwrap();
    assert!(!snapshot(&source, &out).unwrap().is_empty());
    let first: Value = serde_json::from_str(
        fs::read_to_string(path("main"))
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(first["cwd"], "");
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "set MOORING_REPLAY_HERMES_DB to a private read-only database copy"]
fn replay_hermes_database_copy() {
    let db = PathBuf::from(std::env::var_os("MOORING_REPLAY_HERMES_DB").expect("database copy"));
    let root = std::env::temp_dir().join(format!("mooring-hermes-replay-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let out = root.join("mirror");
    let files = snapshot(&db, &out).unwrap();
    assert!(!files.is_empty());
    let mut index = Index::open(&root, vec![(Agent::Hermes, out)]).unwrap();
    index.update_many(files.clone(), 1).unwrap();
    let cards = files
        .iter()
        .filter_map(|p| index.card(p.to_str().unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(!cards.is_empty());
    assert!(cards.iter().any(|c| !c.asks.is_empty()));
    println!(
        "Hermes private replay: {} source files, {} indexed cards; no conversation text printed",
        files.len(),
        cards.len()
    );
    fs::remove_dir_all(root).unwrap();
}
