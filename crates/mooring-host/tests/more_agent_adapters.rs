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
