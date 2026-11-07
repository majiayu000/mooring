//! Codex child logs feed the same sidebar list as Claude's subagent metadata.
use super::*;
use crate::content::Official;
use mooring_core::content::{Subagent, Turn};
use serde_json::json;

fn event(at: u64, kind: &str) -> String {
    format!(
        "{}\n",
        json!({"timestamp": at, "type": "event_msg", "payload": {"type": kind}})
    )
}
fn seed(f: &Fixture, id: &str, parent: Option<&str>, at: u64) -> PathBuf {
    let path = f.0.join(format!("logs/{id}.jsonl"));
    let source = parent.map_or(json!("cli"), |p| {
        json!({"subagent": {"thread_spawn": {
            "parent_thread_id": p, "agent_path": format!("/root/{id}"), "depth": 1
        }}})
    });
    let header = json!({"timestamp": at, "type": "session_meta", "payload": {
        "id": id, "cwd": "/tmp", "source": source, "agent_nickname": id
    }});
    fs::write(&path, format!("{header}\n{}", event(at, "task_started"))).unwrap();
    path
}
fn append(i: &mut Index, path: &Path, at: u64, kind: &str) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(event(at, kind).as_bytes())
        .unwrap();
    i.update_many([path.to_owned()], at).unwrap();
}
fn list(i: &Index, path: &Path) -> Vec<Subagent> {
    i.card(path.to_str().unwrap()).unwrap().unwrap().subagents
}

#[test]
fn codex_children_refresh_sidebar_without_parent_output_and_survive_restart() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let parent = seed(&f, "parent", None, now - 100);
    let child = seed(&f, "review", Some("parent"), now - 80);
    let other = seed(&f, "unrelated", None, now - 70);
    let mut i = f.index();
    // Child discovered first, including while its parent is not indexed yet.
    i.update_many([child.clone(), other, parent.clone()], 1)
        .unwrap();
    let subs = list(&i, &parent);
    assert_eq!(subs.len(), 1, "same cwd does not establish parenthood");
    assert_eq!(
        (subs[0].id.as_str(), subs[0].name.as_str()),
        ("review", "review")
    );
    assert_eq!(subs[0].started, now - 80);
    assert!(subs[0].running());
    append(&mut i, &parent, now - 60, "task_complete");
    assert!(
        list(&i, &parent)[0].running(),
        "parent stopping does not finish its child"
    );
    append(&mut i, &child, now - 40, "turn_aborted");
    let sub = &list(&i, &parent)[0];
    assert!(sub.stopped);
    assert_eq!(sub.ended, Some(now - 40));
    append(&mut i, &child, now - 30, "task_started");
    let sub = &list(&i, &parent)[0];
    assert!(sub.running() && !sub.stopped, "resumed child runs again");
    assert_eq!(sub.started, now - 30);
    append(&mut i, &child, now - 10, "task_complete");
    assert_eq!(list(&i, &parent)[0].ended, Some(now - 10));
    assert!(!list(&i, &parent)[0].stopped);
    assert_eq!(
        i.card(parent.to_str().unwrap()).unwrap().unwrap().turn,
        Turn::Completed
    );
    i.flush().unwrap();
    drop(i);
    let mut i = f.index();
    assert_eq!(list(&i, &parent)[0].ended, Some(now - 10));
    append(&mut i, &parent, now, "task_started");
    assert!(
        list(&i, &parent).is_empty(),
        "previous turn's finished children leave the list"
    );
}

#[test]
fn codex_children_keep_this_turns_old_completions_and_mark_stale_work_stopped() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let parent = seed(&f, "parent", None, now - 3600);
    let done = seed(&f, "done", Some("parent"), now - 3500);
    let stale = seed(&f, "stale", Some("parent"), now - 3400);
    let mut i = f.index();
    i.update_many([parent.clone(), done.clone(), stale], 1)
        .unwrap();
    append(&mut i, &done, now - 3300, "task_complete");
    let subs = list(&i, &parent);
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0].id, "done");
    assert_eq!(subs[0].ended, Some(now - 3300));
    assert!(
        !subs[0].stopped,
        "completion remains visible beyond the freshness window"
    );
    assert_eq!(subs[1].id, "stale");
    assert!(
        subs[1].stopped && !subs[1].running(),
        "silent work is not reported as successful"
    );
}

#[test]
fn codex_children_count_each_waiting_child_and_keep_the_parent_question() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let parent = seed(&f, "parent", None, now - 100);
    let a = seed(&f, "a", Some("parent"), now - 80);
    let b = seed(&f, "b", Some("parent"), now - 70);
    let mut i = f.index();
    i.update_many([parent.clone(), a.clone(), b.clone()], 1)
        .unwrap();
    append(&mut i, &parent, now - 60, "task_complete");
    for (n, child) in [a, b].iter().enumerate() {
        let ask = json!({"timestamp": now - 50 + n as u64, "type": "response_item", "payload": {
            "type": "function_call", "name": "request_user_input", "call_id": "q",
            "arguments": json!({"questions": [{"question": "是否继续？"}]}).to_string()
        }});
        fs::OpenOptions::new()
            .append(true)
            .open(child)
            .unwrap()
            .write_all(format!("{ask}\n").as_bytes())
            .unwrap();
        i.update_many([child.clone()], 3 + n as u64).unwrap();
    }
    let card = i.card(parent.to_str().unwrap()).unwrap().unwrap();
    assert_eq!(card.turn, Turn::Waiting);
    assert_eq!(card.background.unwrap().agents, 2);
    assert!(card.waiting.contains("是否继续？"));
    assert_eq!(card.subagents.len(), 2);
    assert!(card.subagents.iter().all(Subagent::running));
}

/// A child stuck on a dialog it will never leave: a command that needs approval.
fn child_needs_approval(i: &mut Index, child: &Path, at: u64) {
    let item = |payload: serde_json::Value| {
        format!(
            "{}\n",
            json!({"timestamp": at, "type": "response_item", "payload": payload})
        )
    };
    let lines = item(
        json!({"type": "function_call", "name": "exec_command", "call_id": "c1",
        "arguments": json!({"cmd": "npm test"}).to_string()}),
    ) + &item(json!({"type": "function_call_output", "call_id": "c1",
            "output": "Process exited with code 1: requires approval"}));
    fs::OpenOptions::new()
        .append(true)
        .open(child)
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
    i.update_many([child.to_owned()], at).unwrap();
}
/// As if the child's log went silent past `CHILD_FRESH` (15 minutes), too long to wait for.
fn age_links(i: &Index) {
    i.db.execute("UPDATE subagent_links SET updated=updated-1000", [])
        .unwrap();
}
fn poll(i: &Index, path: &str, status: Option<&Official>) -> bool {
    i.official(Agent::Codex, "parent", path, status).unwrap()
}
fn turn(i: &Index, path: &Path) -> Turn {
    i.card(path.to_str().unwrap()).unwrap().unwrap().turn
}

#[test]
fn a_silent_child_stops_holding_a_completed_parent_at_the_next_official_poll() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let parent = seed(&f, "parent", None, now - 100);
    let child = seed(&f, "review", Some("parent"), now - 80);
    let mut i = f.index();
    i.update_many([parent.clone(), child.clone()], 1).unwrap();
    child_needs_approval(&mut i, &child, now - 60);
    append(&mut i, &child, now - 50, "task_complete");
    append(&mut i, &parent, now - 40, "task_complete");
    assert_eq!(turn(&i, &parent), Turn::Waiting);
    let key = parent.to_str().unwrap();
    // The child may really be asking: the parent's Idle says nothing about it (#160).
    poll(&i, key, Some(&Official::Idle));
    assert_eq!(turn(&i, &parent), Turn::Waiting);
    // Neither log grows again, so only the poll can notice the child has gone silent.
    age_links(&i);
    poll(&i, key, Some(&Official::Idle));
    let card = i.card(key).unwrap().unwrap();
    assert_eq!(card.turn, Turn::Completed, "{:?}", card.waiting);
    assert!(card.waiting.is_empty());
    assert!(card.subagents.iter().all(|s| !s.running()));
}

#[test]
fn a_child_that_vanished_stops_holding_an_aborted_parent_at_the_next_official_poll() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let parent = seed(&f, "parent", None, now - 100);
    let child = seed(&f, "review", Some("parent"), now - 80);
    let mut i = f.index();
    i.update_many([parent.clone(), child.clone()], 1).unwrap();
    append(&mut i, &parent, now - 70, "turn_aborted");
    assert_eq!(turn(&i, &parent), Turn::Interrupted);
    // The child's dialog is projected onto the parent again; the child then writes nothing.
    child_needs_approval(&mut i, &child, now - 60);
    assert_eq!(turn(&i, &parent), Turn::Waiting);
    age_links(&i);
    poll(&i, parent.to_str().unwrap(), Some(&Official::Idle));
    assert_eq!(turn(&i, &parent), Turn::Interrupted);
}

#[test]
fn a_parent_that_finished_before_its_childs_dialog_stays_finished() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let parent = seed(&f, "parent", None, now - 100);
    let child = seed(&f, "review", Some("parent"), now - 80);
    let mut i = f.index();
    i.update_many([parent.clone(), child.clone()], 1).unwrap();
    append(&mut i, &parent, now - 70, "task_complete");
    child_needs_approval(&mut i, &child, now - 60);
    assert_eq!(turn(&i, &parent), Turn::Waiting);
    age_links(&i);
    poll(&i, parent.to_str().unwrap(), Some(&Official::Idle));
    assert_eq!(turn(&i, &parent), Turn::Completed);
}

#[test]
fn a_child_dialog_ends_on_a_poll_the_cli_cannot_answer_and_only_once_it_went_silent() {
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let parent = seed(&f, "parent", None, now - 100);
    let child = seed(&f, "review", Some("parent"), now - 80);
    let mut i = f.index();
    i.update_many([parent.clone(), child.clone()], 1).unwrap();
    child_needs_approval(&mut i, &child, now - 60);
    let key = parent.to_str().unwrap();
    // A `--no-daemon` thread or a stopped daemon: no status, and the child is still fresh.
    assert!(poll(&i, key, None));
    assert_eq!(turn(&i, &parent), Turn::Waiting);
    age_links(&i);
    assert!(poll(&i, key, None));
    assert_eq!(turn(&i, &parent), Turn::Running, "back to the turn it held");
    // Nothing to recheck now; the poll leaves a session it has no answer for untouched.
    assert!(!poll(&i, key, None));
}

#[test]
fn an_official_answer_settles_a_running_parent_once_its_childs_dialog_is_over() {
    for (status, settled) in [
        (Official::Idle, Turn::Interrupted),
        (Official::Busy, Turn::Running),
    ] {
        let f = Fixture::new();
        let now = mooring_core::content::now();
        let parent = seed(&f, "parent", None, now - 100);
        let child = seed(&f, "review", Some("parent"), now - 80);
        let mut i = f.index();
        i.update_many([parent.clone(), child.clone()], 1).unwrap();
        child_needs_approval(&mut i, &child, now - 60);
        age_links(&i);
        poll(&i, parent.to_str().unwrap(), Some(&status));
        assert_eq!(turn(&i, &parent), settled, "{status:?}");
    }
    // The parent itself is asking: the old child's text must not stand for its question.
    let f = Fixture::new();
    let now = mooring_core::content::now();
    let parent = seed(&f, "parent", None, now - 100);
    let child = seed(&f, "review", Some("parent"), now - 80);
    let mut i = f.index();
    i.update_many([parent.clone(), child.clone()], 1).unwrap();
    child_needs_approval(&mut i, &child, now - 60);
    age_links(&i);
    let key = parent.to_str().unwrap();
    poll(&i, key, Some(&Official::Waiting("等待审批".into())));
    let card = i.card(key).unwrap().unwrap();
    assert_eq!(card.turn, Turn::Waiting);
    assert!(card.waiting.starts_with("等待审批"), "{}", card.waiting);
}
