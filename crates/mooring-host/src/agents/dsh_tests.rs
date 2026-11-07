use super::*;
use crate::log_index::Index;
use mooring_core::{content::Turn, model::Agent};
use serde_json::json;
use std::io::Write;

fn line(v: Value) -> Vec<u8> {
    format!("{v}\n").into_bytes()
}
#[test]
fn compressed_generations_and_partial_frames_preserve_child_lifecycle() {
    let root = std::env::temp_dir().join(format!("mooring-dsh-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let source = root.join("sessions");
    let out = root.join("mirror");
    let now = mooring_core::content::now() * 1000;
    let main = source.join("project/main/session.v4.jsonl");
    let child = source.join("project/child/session.v4.jsonl.zstd");
    fs::create_dir_all(main.parent().unwrap()).unwrap();
    fs::create_dir_all(child.parent().unwrap()).unwrap();
    let header = |id: &str, parent: Option<&str>| json!({"type":"session","version":4,"id":id,"cwd":"/repo","createdAt":now-5000,"isSeeded":false,"origin":if parent.is_some(){Some("subagent")}else{None},"delegationDepth":if parent.is_some(){1}else{0},"parentSession":parent});
    let event = |ty: &str, data: Value| json!({"type":ty,"time":now,"data":data});
    fs::write(
        &main,
        [
            line(header("main", None)),
            line(event("turn/start", json!({"turn":1}))),
        ]
        .concat(),
    )
    .unwrap();
    let initial = [
        line(header("child", Some("main"))),
        line(event("turn/start", json!({"turn":1}))),
        line(event(
            "user/message",
            json!({"source":{"kind":"user"},"content":[{"type":"text","text":"Review code"}]}),
        )),
        line(event(
            "approval/asked",
            json!({"id":"q","toolName":"bash","reason":"Run tests?"}),
        )),
    ]
    .concat();
    fs::write(
        &child,
        zstd::stream::encode_all(initial.as_slice(), 1).unwrap(),
    )
    .unwrap();
    fs::write(
        child.with_file_name("session.v3.jsonl"),
        b"old generation is not read\n",
    )
    .unwrap();
    let mut store = Store::default();
    let files = store.poll(&source, &out).unwrap();
    assert_eq!(files.len(), 2);
    let mut index = Index::open(&root, vec![(Agent::Dsh, out.clone())]).unwrap();
    index.update_many(files, 1).unwrap();
    let main_out = out.join("6d61696e.jsonl");
    let child_out = out.join("6368696c64.jsonl");
    let card = |i: &Index, p: &Path| i.card(p.to_str().unwrap()).unwrap().unwrap();
    assert_eq!(card(&index, &main_out).turn, Turn::Waiting);
    assert!(card(&index, &main_out).waiting.contains("Run tests?"));
    assert_eq!(card(&index, &child_out).parent, "main");
    assert!(store.poll(&source, &out).unwrap().is_empty());
    let done = [
        line(event(
            "approval/decided",
            json!({"id":"q","outcome":"allowed-once"}),
        )),
        line(event(
            "assistant/message",
            json!({"message":{"content":[{"type":"text","text":"Done"}]}}),
        )),
        line(event(
            "turn/end",
            json!({"turn":1,"reason":{"kind":"completed"}}),
        )),
    ]
    .concat();
    let frame = zstd::stream::encode_all(done.as_slice(), 1).unwrap();
    let mid = frame.len() / 2;
    let mut writer = fs::OpenOptions::new().append(true).open(&child).unwrap();
    writer.write_all(&frame[..mid]).unwrap();
    let before = fs::read(&child_out).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&main)
        .unwrap()
        .write_all(&line(event(
            "session/title",
            json!({"title":"Other session still updates"}),
        )))
        .unwrap();
    assert!(
        store.poll(&source, &out).is_err(),
        "half a zstd frame must not publish a false tail"
    );
    assert_eq!(fs::read(&child_out).unwrap(), before);
    assert!(
        fs::read_to_string(&main_out)
            .unwrap()
            .contains("Other session still updates"),
        "a partial child frame must not starve its parent's log"
    );
    writer.write_all(&frame[mid..]).unwrap();
    drop(writer);
    index
        .update_many(store.poll(&source, &out).unwrap(), 2)
        .unwrap();
    assert_eq!(card(&index, &child_out).turn, Turn::Completed);
    assert_eq!(card(&index, &child_out).last_reply, "Done");
    assert!(card(&index, &main_out).subagents[0].ended.is_some());
    // A newer unsupported generation must not silently fall back to stale v4 data.
    fs::write(child.with_file_name("session.v5.jsonl"), b"{}\n").unwrap();
    assert!(store.poll(&source, &out).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn inherited_seed_and_fork_parent_do_not_become_a_new_task_or_child() {
    let bytes=[line(json!({"type":"session","version":4,"id":"fork","cwd":"/repo","createdAt":1000,"isSeeded":true,"parentSession":"old","delegationDepth":0})),line(json!({"type":"user/message","time":1000,"data":{"source":{"kind":"user"},"content":"Old prompt"}})),line(json!({"type":"session/end-seed","data":{"inherited":true}})),line(json!({"type":"turn/start","time":2000,"data":{"turn":2}})),line(json!({"type":"user/message","time":2000,"data":{"source":{"kind":"user"},"content":[{"type":"text","text":"New prompt"}]}})),line(json!({"type":"turn/end","time":3000,"data":{"reason":{"kind":"aborted"}}}))].concat();
    let (id, normalized) = normalize(&bytes).unwrap();
    assert_eq!(id, "fork");
    let mut parser = crate::content::Parser::default();
    for (offset, l) in normalized.split(|b| *b == b'\n').enumerate() {
        parser.add(
            Agent::Dsh,
            l,
            mooring_core::content::Source {
                offset: offset as u64,
                ..Default::default()
            },
        );
    }
    assert!(parser.card.parent.is_empty());
    assert_eq!(parser.card.asks.len(), 1);
    assert_eq!(parser.card.asks[0].text, "New prompt");
    assert_eq!(parser.card.turn, Turn::Interrupted);
    assert_eq!(parser.card.updated, 3, "milliseconds are normalized once");
}
