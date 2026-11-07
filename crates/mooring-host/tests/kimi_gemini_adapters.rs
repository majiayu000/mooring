//! Current Kimi Code wire records and Gemini CLI append-only recording shapes.
use mooring_core::{
    content::{Content, Turn},
    model::Agent,
};
use mooring_host::{agent_adapter, log_index::Index};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const SESSION: &str = "session_c19a5748-897d-4836-a555-fce8a7797222";
const PARENT: &str = "c19a5748-897d-4836-a555-fce8a7797222";
const CHILD: &str = "19817791-63b7-441c-ad06-25fe5e5293ba";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("mooring-kimi-gemini-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn append(path: &Path, record: Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "{record}"
    )
    .unwrap();
}
fn card(index: &Index, path: &Path) -> Content {
    index.card(path.to_str().unwrap()).unwrap().unwrap()
}
fn wire(path: &Path, at: u64, kind: &str, fields: Value) {
    let mut record = fields;
    record["type"] = json!(kind);
    record["time"] = json!(at * 1000);
    append(path, record);
}
fn kimi_session(root: &Path, session: &str, now: u64) -> (PathBuf, PathBuf) {
    let dir = root.join(format!("wd_fixture/{session}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("state.json"),json!({"workDir":"/repo","title":"Kimi review",
        "agents":{"main":{"type":"main","parentAgentId":null},"agent-0":{"type":"sub","parentAgentId":"main"},
        "agent-1":{"type":"sub","parentAgentId":"agent-0"}}}).to_string()).unwrap();
    let main = dir.join("agents/main/wire.jsonl");
    let child = dir.join("agents/agent-0/wire.jsonl");
    for (path, at) in [(&main, now - 100), (&child, now - 80)] {
        append(
            path,
            json!({"type":"metadata","protocol_version":"1.4","created_at":at*1000}),
        );
        wire(path, at, "config.update", json!({"cwd":"/repo"}));
        wire(
            path,
            at,
            "turn.prompt",
            json!({"input":[{"type":"text","text":"Review parser"}]}),
        );
    }
    (main, child)
}
#[test]
fn kimi_links_namespaced_children_and_recovers_wait_end_and_nested_parent() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let root = f.0.join(".kimi-code/sessions");
    let (main, child) = kimi_session(&root, SESSION, now);
    let other = "session_19817791-63b7-441c-ad06-25fe5e5293ba";
    let (other_main, other_child) = kimi_session(&root, other, now);
    let nested = child
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("agent-1/wire.jsonl");
    wire(
        &nested,
        now - 60,
        "turn.prompt",
        json!({"input":"Nested work"}),
    );
    let mut index = Index::open(&f.0, vec![(Agent::Kimi, root.clone())]).unwrap();
    index.initialize().unwrap();
    index.bootstrap().unwrap();
    let c = card(&index, &main);
    assert_eq!(c.subagents.len(), 1);
    assert!(c.subagents[0].running());
    assert_eq!(c.subagents[0].id, format!("{SESSION}/agent-0"));
    assert_eq!(c.subagents[0].name, "Review parser");
    assert_eq!(
        card(&index, &other_main).subagents[0].id,
        format!("{other}/agent-0")
    );
    assert_eq!(card(&index, &nested).parent, format!("{SESSION}/agent-0"));
    assert!(!card(&index, &child).interactive);
    wire(
        &child,
        now - 50,
        "interaction.request",
        json!({"kind":"approval","request":{"action":"Run tests?"}}),
    );
    index.update_many([child.clone()], 1).unwrap();
    assert_eq!(card(&index, &main).turn, Turn::Waiting);
    wire(
        &child,
        now - 40,
        "interaction.resolved",
        json!({"response":{"decision":"approve"}}),
    );
    wire(&nested, now - 35, "turn.cancel", json!({}));
    wire(
        &child,
        now - 30,
        "context.append_loop_event",
        json!({"event":{"type":"content.part","part":{"type":"text","text":"Done"}}}),
    );
    wire(
        &child,
        now - 29,
        "context.append_loop_event",
        json!({"event":{"type":"step.end","finishReason":"tool_use"}}),
    );
    index.update_many([child.clone(), nested], 2).unwrap();
    assert_eq!(card(&index, &child).turn, Turn::Running);
    wire(
        &child,
        now - 20,
        "turn.ended",
        json!({"reason":"completed"}),
    );
    let changed = index.update_many([child.clone()], 3).unwrap();
    assert!(changed.iter().any(|t| t.path == main.to_string_lossy()));
    assert_eq!(card(&index, &main).subagents[0].ended, Some(now - 20));
    assert!(!card(&index, &main).subagents[0].stopped);
    assert!(card(&index, &other_main).subagents[0].running());
    // The parent can open its own approval while a child dialog is up. Closing
    // the child's dialog must not dismiss the parent's question.
    wire(
        &other_child,
        now - 16,
        "interaction.request",
        json!({"request":{"action":"Child approval"}}),
    );
    index.update_many([other_child.clone()], 4).unwrap();
    wire(
        &other_main,
        now - 15,
        "interaction.request",
        json!({"request":{"action":"Parent approval"}}),
    );
    index.update_many([other_main.clone()], 5).unwrap();

    wire(
        &other_child,
        now - 10,
        "turn.ended",
        json!({"reason":"failed"}),
    );
    index.update_many([other_child], 4).unwrap();
    assert!(card(&index, &other_main).subagents[0].stopped);
    assert_eq!(card(&index, &other_main).turn, Turn::Waiting);
    assert_eq!(card(&index, &other_main).waiting, "Parent approval");
    index.flush().unwrap();
    drop(index);
    let index = Index::open(&f.0, vec![(Agent::Kimi, root)]).unwrap();
    assert_eq!(card(&index, &child).turn, Turn::Completed);
    assert_eq!(card(&index, &main).subagents.len(), 1);
}
fn gemini(path: &Path, at: u64, kind: &str, fields: Value) {
    let mut r = fields;
    r["type"] = json!(kind);
    r["timestamp"] = json!(chrono::DateTime::from_timestamp(at as i64, 0)
        .unwrap()
        .to_rfc3339());
    append(path, r);
}
#[test]
fn gemini_discovers_children_and_tool_snapshots_do_not_finish_the_turn() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let root = f.0.join(".gemini/tmp");
    let project = root.join("repo");
    let chats = project.join("chats");
    fs::create_dir_all(&chats).unwrap();
    fs::write(project.join(".project_root"), "/repo").unwrap();
    let main = chats.join("session-2026-11-07T09-00-c19a5748.jsonl");
    let child = chats.join(PARENT).join(format!("{CHILD}.jsonl"));
    for (path, id, kind, at) in [
        (&main, PARENT, "main", now - 100),
        (&child, CHILD, "subagent", now - 80),
    ] {
        append(
            path,
            json!({"sessionId":id,"projectHash":"hash","kind":kind}),
        );
        gemini(
            path,
            at,
            "user",
            json!({"id":format!("ask-{id}"),"content":[{"text":"Review parser"}]}),
        );
    }
    let mut index = Index::open(&f.0, vec![(Agent::Gemini, root.clone())]).unwrap();
    index.initialize().unwrap();
    index.bootstrap().unwrap();
    assert_eq!(card(&index, &main).subagents.len(), 1);
    assert_eq!(card(&index, &child).parent, PARENT);
    assert!(card(&index, &main).interactive);
    assert!(!card(&index, &child).interactive);
    for (event, status, turn) in [
        (1, "awaiting_approval", Turn::Waiting),
        (2, "executing", Turn::Running),
        (3, "success", Turn::Running),
    ] {
        gemini(
            &child,
            now - 60 + event,
            "gemini",
            json!({"id":"same-message","content":"","toolCalls":[{"id":"call","name":"run_shell_command","status":status,"description":"Run tests?"}]}),
        );
        index.update_many([child.clone()], event).unwrap();
        assert_eq!(card(&index, &child).turn, turn);
        assert_eq!(
            card(&index, &main).turn == Turn::Waiting,
            turn == Turn::Waiting
        );
        assert!(card(&index, &main).subagents[0].running());
    }
    gemini(
        &child,
        now - 30,
        "gemini",
        json!({"id":"same-message","content":"","toolCalls":[{"id":"call","status":"awaiting_approval","description":"Child approval"}]}),
    );
    index.update_many([child.clone()], 4).unwrap();
    gemini(
        &main,
        now - 29,
        "gemini",
        json!({"id":"parent-tool","content":"","toolCalls":[{"id":"own-call","status":"awaiting_approval","description":"Parent approval"}]}),
    );
    index.update_many([main.clone()], 5).unwrap();
    gemini(
        &child,
        now - 25,
        "gemini",
        json!({"id":"same-message","content":"","toolCalls":[{"id":"call","status":"success"}]}),
    );
    index.update_many([child.clone()], 6).unwrap();
    assert_eq!(card(&index, &main).turn, Turn::Waiting);
    assert_eq!(card(&index, &main).waiting, "Parent approval");
    gemini(
        &main,
        now - 24,
        "gemini",
        json!({"id":"parent-tool","content":"","toolCalls":[{"id":"own-call","status":"success"}]}),
    );
    index.update_many([main.clone()], 7).unwrap();
    gemini(
        &child,
        now - 20,
        "gemini",
        json!({"id":"reply","content":[{"text":"Done"}]}),
    );
    index.update_many([child.clone()], 4).unwrap();
    assert_eq!(card(&index, &child).turn, Turn::Running);
    gemini(
        &child,
        now - 20,
        "gemini",
        json!({"id":"reply","content":"Done","toolCalls":[{"id":"finish","name":"complete_task","status":"success"}]}),
    );
    index.update_many([child.clone()], 5).unwrap();
    assert_eq!(card(&index, &main).subagents[0].ended, Some(now - 20));
    append(
        &child,
        json!({"$set":{"lastUpdated":"2026-11-07T09:10:00Z","summary":"Renamed"}}),
    );
    index.update_many([child.clone()], 5).unwrap();
    assert_eq!(card(&index, &child).turn, Turn::Completed);
    index.flush().unwrap();
    drop(index);
    let mut index = Index::open(&f.0, vec![(Agent::Gemini, root)]).unwrap();
    assert!(!card(&index, &main).subagents[0].stopped);
    append(&child, json!({"$rewindTo":"reply"}));
    index.update_many([child.clone()], 6).unwrap();
    assert_eq!(card(&index, &child).turn, Turn::Unknown);
    assert!(card(&index, &child).last_reply.is_empty());
    gemini(
        &child,
        now - 10,
        "user",
        json!({"id":"again","content":"Continue"}),
    );
    gemini(
        &child,
        now - 5,
        "gemini",
        json!({"id":"cancelled","content":"","toolCalls":[{"id":"call2","status":"cancelled"}]}),
    );
    index.update_many([child.clone()], 7).unwrap();
    assert_eq!(
        card(&index, &child).turn,
        Turn::Running,
        "cancelled tool can be a soft rejection; no invented turn end"
    );
    gemini(
        &child,
        now - 3,
        "error",
        json!({"id":"failure","content":"Agent failed"}),
    );
    index.update_many([child.clone()], 8).unwrap();
    assert!(card(&index, &main).subagents[0].stopped);
    assert!(!agent_adapter(Agent::Gemini).is_transcript(&child.with_extension("json")));
}
#[test]
fn commands_and_processes_name_the_cli_and_resume_exact_sessions() {
    for (agent, executable, flag, id) in [
        (Agent::Kimi, "kimi", "--session", SESSION),
        (Agent::Gemini, "gemini", "--resume", PARENT),
    ] {
        let adapter = agent_adapter(agent);
        assert!(adapter.runs(executable, None));
        let args = vec![executable.into(), flag.into(), id.into()];
        assert_eq!(adapter.process_session(1, &args).0.as_deref(), Some(id));
        assert!(agent.resume_command(id, &[]).contains(id));
        assert_eq!(agent.command(), executable);
        assert_eq!(
            adapter
                .process_session(1, &[executable.into(), flag.into(), "latest".into()])
                .0,
            None
        );
    }
    assert!(agent_adapter(Agent::Gemini).runs(
        "node",
        Some("/npm/node_modules/@google/gemini-cli/dist/index.js")
    ));
}

#[test]
#[ignore = "read-only local replay; set MOORING_REPLAY_KIMI_CHILD"]
fn replay_kimi_wire_without_changing_user_data() {
    let source = PathBuf::from(std::env::var_os("MOORING_REPLAY_KIMI_CHILD").expect("sample path"));
    let session = source.ancestors().nth(3).unwrap();
    let f = Fixture::new();
    let root = f.0.join("sessions");
    let target = root.join("wd_fixture").join(session.file_name().unwrap());
    fs::create_dir_all(&target).unwrap();
    fs::copy(session.join("state.json"), target.join("state.json")).unwrap();
    let agent = source.parent().unwrap().file_name().unwrap();
    for (from, to) in [
        (
            source.clone(),
            target.join("agents").join(agent).join("wire.jsonl"),
        ),
        (
            session.join("agents/main/wire.jsonl"),
            target.join("agents/main/wire.jsonl"),
        ),
    ] {
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        fs::copy(from, to).unwrap();
    }
    let mut index = Index::open(&f.0, vec![(Agent::Kimi, root)]).unwrap();
    index.recent_days = 3650; // Old real sample only; production keeps its seven-day window.
    index.initialize().unwrap();
    index.bootstrap().unwrap();
    let child = card(
        &index,
        &target.join("agents").join(agent).join("wire.jsonl"),
    );
    assert!(!child.parent.is_empty());
    assert!(!child.hidden_reason.is_empty());
    assert_eq!(child.turn, Turn::Completed);
    println!(
        "Kimi actual wire: parent relationship present; child {:?}; asks {}",
        child.turn,
        child.asks.len()
    );
}
