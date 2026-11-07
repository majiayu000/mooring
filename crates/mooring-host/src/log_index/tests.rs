use super::elide::STRING_KEEP;
use super::*;
use mooring_platform::file_watch::flag;
use std::io::Write;
#[path = "codex_children_tests.rs"]
mod codex_children;
#[path = "query_perf_tests.rs"]
mod query_perf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kl-index-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("logs")).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn index(&self) -> Index {
        Index::open(&self.0, vec![(Agent::Codex, self.0.join("logs"))]).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn line(id: &str) -> String {
    format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"cwd\":\"/tmp\"}}}}\n")
}
/// `MOORING_BENCH_LOG=<real Claude transcript> cargo test --release -p mooring-host
/// --lib append_cost -- --ignored --nocapture`: indexes all but the last 500 lines, then
/// appends them one at a time, flushing every 10 as a log moving at 10 lines a second
/// would be, and reports the cost per line.
#[test]
#[ignore]
fn append_cost() {
    let Some(src) = std::env::var_os("MOORING_BENCH_LOG") else {
        return;
    };
    let text = fs::read_to_string(src).unwrap();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let tail = 500.min(lines.len() / 2);
    let (head, rest) = lines.split_at(lines.len() - tail);
    let f = Fixture::new();
    let root = f.0.join("claude");
    let log = root.join("p/0b1c2d3e-0000-4000-8000-000000000009.jsonl");
    fs::create_dir_all(log.parent().unwrap()).unwrap();
    fs::write(&log, head.concat()).unwrap();
    // MOORING_BENCH_SUBAGENTS=<n>: the conversation has run n subagents.
    let subs = log.with_extension("").join("subagents");
    fs::create_dir_all(&subs).unwrap();
    let n: usize = std::env::var("MOORING_BENCH_SUBAGENTS").map_or(0, |v| v.parse().unwrap());
    for k in 0..n {
        fs::write(subs.join(format!("agent-a{k}.jsonl")), "{}\n").unwrap();
        fs::write(
            subs.join(format!("agent-a{k}.meta.json")),
            format!(r#"{{"agentType":"Explore","description":"task {k}","finished":true}}"#),
        )
        .unwrap();
    }
    let mut i = Index::open(&f.0, vec![(Agent::Claude, root.clone())]).unwrap();
    i.initialize().unwrap();
    i.update_many([log.clone()], 1).unwrap();
    let cpu = || {
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        u.ru_utime.tv_sec as f64
            + u.ru_utime.tv_usec as f64 / 1e6
            + u.ru_stime.tv_sec as f64
            + u.ru_stime.tv_usec as f64 / 1e6
    };
    let (c0, t0) = (cpu(), std::time::Instant::now());
    for (n, line) in rest.iter().enumerate() {
        fs::OpenOptions::new()
            .append(true)
            .open(&log)
            .unwrap()
            .write_all(line.as_bytes())
            .unwrap();
        i.update_many([log.clone()], n as u64 + 2).unwrap();
        // A log moving at 10 lines a second is flushed once a second.
        if n % 10 == 9 {
            i.flush().unwrap();
        }
    }
    i.flush().unwrap();
    let (c, t) = (cpu() - c0, t0.elapsed().as_secs_f64());
    let blob: i64 =
        i.db.query_row(
            "SELECT length(parser)+length(projection) FROM session_content",
            [],
            |r| r.get(0),
        )
        .unwrap();
    eprintln!(
        "BENCH appends={tail} cpu/append={:.1}us wall/append={:.1}us stored={blob}B",
        c / tail as f64 * 1e6,
        t / tail as f64 * 1e6
    );
}
#[test]
fn a_crash_before_the_flush_replays_from_a_consistent_snapshot() {
    let msg = |kind: &str, text: &str| {
        format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"{kind}\",\"message\":\"{text}\"}}}}\n")
    };
    let lines = [
        line("crash"),
        msg("user_message", "改一下"),
        msg("agent_message", "第一段"),
        msg("agent_message", "第二段"),
        msg("user_message", "再看看"),
        msg("agent_message", "第三段"),
    ];
    let state = |root: &Path, n: usize, crash: bool| {
        let log = root.join("logs/crash.jsonl");
        fs::write(&log, lines[..2].concat()).unwrap();
        let mut i = Index::open(root, vec![(Agent::Codex, root.join("logs"))]).unwrap();
        i.initialize().unwrap();
        i.reset_checkpoint(10).unwrap();
        i.update_many([log.clone()], 11).unwrap();
        for (k, l) in lines[2..n].iter().enumerate() {
            fs::OpenOptions::new()
                .append(true)
                .open(&log)
                .unwrap()
                .write_all(l.as_bytes())
                .unwrap();
            i.update_many([log.clone()], 12 + k as u64).unwrap();
        }
        let key = log.to_str().unwrap().to_owned();
        if crash {
            // Held ahead of the database: the checkpoint stayed behind it.
            assert!(i.flush_due().is_some());
            let event: i64 =
                i.db.query_row("SELECT event_id FROM log_index", [], |r| r.get(0))
                    .unwrap();
            assert_eq!(event, 11);
            drop(i);
            // A new host replays the events after the checkpoint.
            let mut j = Index::open(root, vec![(Agent::Codex, root.join("logs"))]).unwrap();
            j.initialize().unwrap();
            j.update_many([log.clone()], 20).unwrap();
            j.flush().unwrap();
            i = j;
        } else {
            i.flush().unwrap();
        }
        assert!(i.flush_due().is_none());
        let card = i.card(&key).unwrap().unwrap();
        let records: Vec<(String, String)> =
            i.db.prepare("SELECT kind,text FROM session_records ORDER BY offset,ordinal")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
        let offset: i64 =
            i.db.query_row("SELECT offset FROM log_files", [], |r| r.get(0))
                .unwrap();
        let asks: Vec<String> = card.asks.into_iter().map(|q| q.text).collect();
        (card.turn, card.last_reply, asks, records, offset)
    };
    let calm = Fixture::new();
    let crashed = Fixture::new();
    let expected = state(&calm.0, lines.len(), false);
    assert_eq!(expected.4 as usize, lines.concat().len());
    assert_eq!(state(&crashed.0, lines.len(), true), expected);
}
#[test]
fn a_moving_log_reads_its_subagents_again_only_when_their_meta_changes() {
    let f = Fixture::new();
    let root = f.0.join("claude");
    let id = "0b1c2d3e-0000-4000-8000-000000000002";
    let log = root.join(format!("p/{id}.jsonl"));
    let subs = root.join(format!("p/{id}/subagents"));
    fs::create_dir_all(&subs).unwrap();
    let ask = |text: &str| {
        format!(
            "{}\n",
            serde_json::json!({"type": "user", "sessionId": id, "cwd": "/repo",
            "timestamp": "2026-11-03T10:00:00Z", "message": {"role": "user", "content": text}})
        )
    };
    fs::write(&log, ask("查一下")).unwrap();
    let mut i = Index::open(&f.0, vec![(Agent::Claude, root.clone())]).unwrap();
    i.initialize().unwrap();
    i.update_many([log.clone()], 1).unwrap();
    i.update_many([log.clone()], 2).unwrap();
    let key = log.to_str().unwrap();
    let meta = subs.join("agent-a1.meta.json");
    fs::write(subs.join("agent-a1.jsonl"), "{}\n").unwrap();
    fs::write(
        &meta,
        r#"{"agentType":"Explore","description":"Audit drift"}"#,
    )
    .unwrap();
    // The parent's own lines reuse the list held in memory.
    fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(ask("继续").as_bytes())
        .unwrap();
    i.update_many([log.clone()], 3).unwrap();
    assert!(i.card(key).unwrap().unwrap().subagents.is_empty());
    // The meta's own event reads it.
    i.update_many([meta.clone()], 4).unwrap();
    assert_eq!(i.card(key).unwrap().unwrap().subagents.len(), 1);
    // And later lines keep it.
    fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .unwrap()
        .write_all(ask("再看看").as_bytes())
        .unwrap();
    i.update_many([log.clone()], 5).unwrap();
    assert_eq!(i.card(key).unwrap().unwrap().subagents.len(), 1);
}
#[test]
fn a_subagent_meta_change_refreshes_its_parents_card_whose_log_did_not_move() {
    let f = Fixture::new();
    let root = f.0.join("claude");
    let id = "0b1c2d3e-0000-4000-8000-000000000001";
    let log = root.join(format!("p/{id}.jsonl"));
    let subs = root.join(format!("p/{id}/subagents"));
    fs::create_dir_all(&subs).unwrap();
    let ask = serde_json::json!({"type": "user", "sessionId": id, "cwd": "/repo",
        "timestamp": "2026-11-03T10:00:00Z", "message": {"role": "user", "content": "查一下"}});
    fs::write(&log, format!("{ask}\n")).unwrap();
    let mut i = Index::open(&f.0, vec![(Agent::Claude, root.clone())]).unwrap();
    i.initialize().unwrap();
    i.update_many([log.clone()], 1).unwrap();
    let key = log.to_str().unwrap();
    assert!(i.card(key).unwrap().unwrap().subagents.is_empty());
    // Started: its meta and log appear, the parent's log stays as it was.
    let meta = subs.join("agent-a1.meta.json");
    fs::write(subs.join("agent-a1.jsonl"), "{}\n").unwrap();
    fs::write(
        &meta,
        r#"{"agentType":"Explore","description":"Audit drift","requestShape":"background"}"#,
    )
    .unwrap();
    let found = i.update_many([meta.clone()], 2).unwrap();
    assert_eq!(
        found.iter().map(|t| t.path.as_str()).collect::<Vec<_>>(),
        [key]
    );
    let card = i.card(key).unwrap().unwrap();
    assert_eq!(card.subagents.len(), 1);
    assert!(card.subagents[0].running());
    // Finished: the same meta rewritten.
    fs::write(&meta, r#"{"agentType":"Explore","description":"Audit drift","requestShape":"background","finished":true}"#).unwrap();
    i.update_many([meta.clone()], 3).unwrap();
    assert!(!i.card(key).unwrap().unwrap().subagents[0].running());
    // Nothing new: the parent is not reported again.
    assert!(i.update_many([meta], 4).unwrap().is_empty());
    assert_eq!(
        crate::agents::adapter(Agent::Claude).subagent_parent(&subs.join("notes.json")),
        None
    );
}
#[test]
fn a_removed_declarations_rows_read_back_without_failing_the_index() {
    let f = Fixture::new();
    let index = f.index();
    index
        .db
        .execute(
            "INSERT INTO log_files VALUES('/gone.jsonl','gone-cli',0,0,0,x'',0,'g1','/tmp',1)",
            [],
        )
        .unwrap();
    let cached = index.cached().unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].runtime.key(), "gone-cli");
    assert!(cached[0].runtime.spec().is_none());
}
#[test]
fn original_request_beyond_16k_is_read_once_within_bounded_prefix() {
    let f = Fixture::new();
    let path = f.0.join("logs/head.jsonl");
    let mut log = format!(
        "{}\n",
        serde_json::json!({"type":"session_meta","payload":{"id":"head","cwd":"/repo","source":"vscode","thread_source":"user","originator":"Codex Desktop","base_instructions":"x".repeat(24*1024)}})
    );
    let offset = log.len() as u64;
    log.push_str("{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"最初真实要求\"}}\n");
    log.push_str(&" ".repeat(200 * 1024));
    log.push_str("\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"追加要求\"}}\n");
    fs::write(&path, log).unwrap();
    let mut i = f.index();
    i.initialize().unwrap();
    i.bootstrap().unwrap();
    let card = i.card(path.to_str().unwrap()).unwrap().unwrap();
    assert_eq!(card.title, "最初真实要求");
    assert_eq!(card.asks[0].source.offset, offset);
    assert!(card.first_ask_known && card.hidden_reason().is_none());
    assert!(i.counts.bytes <= 192 * 1024 + 1);
    i.counts = IoCounts::default();
    i.bootstrap().unwrap();
    assert_eq!(i.counts.bytes, 0);
    assert_eq!(i.counts.opens, 0);
}
#[test]
fn changed_content_rules_reseed_once_without_a_directory_walk() {
    let f = Fixture::new();
    let path = f.0.join("logs/rules.jsonl");
    fs::write(&path,concat!("{\"type\":\"session_meta\",\"payload\":{\"id\":\"r\",\"cwd\":\"/repo\",\"originator\":\"codex-tui\",\"source\":\"cli\"}}\n",
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"真实要求\"}}\n")).unwrap();
    let mut i = f.index();
    i.initialize().unwrap();
    i.bootstrap().unwrap();
    i.db.execute(
        "UPDATE session_content SET parser=json_set(parser,'$._rules',6)",
        [],
    )
    .unwrap();
    i.db.execute(
        "UPDATE session_records SET text='old injected context' WHERE kind='user'",
        [],
    )
    .unwrap();
    i.counts = IoCounts::default();
    i.bootstrap().unwrap();
    assert_eq!(i.counts.directories, 0);
    assert!(i.counts.bytes > 0);
    let c = i.card(path.to_str().unwrap()).unwrap().unwrap();
    assert!(c.interactive);
    assert_eq!(c.title, "真实要求");
    let old: i64 =
        i.db.query_row(
            "SELECT count(*) FROM session_records WHERE text='old injected context'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(old, 0);
    i.counts = IoCounts::default();
    i.bootstrap().unwrap();
    assert_eq!(i.counts.bytes, 0);
    assert_eq!(i.counts.opens, 0);
    assert_eq!(
        crate::agents::identity(
            Agent::Claude,
            Path::new("/logs/a.jsonl"),
            br#"{"sessionId":"parent","cwd":"/repo","isSidechain":true,"agentId":"child"}"#
        )
        .unwrap()
        .0,
        "parent:sidechain:child"
    );
}
#[test]
fn a_first_prompt_logged_late_still_knows_when_its_turn_began() {
    // Codex 0.166 through its shared daemon, as logged: the turn starts at Enter and
    // the first prompt lands once the thread is built, 5.5 s later.
    let f = Fixture::new();
    let path = f.0.join("logs/late.jsonl");
    let at =
        |t: &str, payload: &str| format!("{{\"timestamp\":\"2026-11-03T13:{t}Z\",{payload}}}\n");
    let user = |text: &str| {
        format!("\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"{text}\"}}]}}")
    };
    let started = "\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}";
    let done = "\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}";
    let log = [
        at("12:54.164", "\"type\":\"session_meta\",\"payload\":{\"id\":\"late\",\"cwd\":\"/repo\",\"originator\":\"codex-tui\",\"source\":\"cli\"}"),
        at("12:54.164", started),
        at("12:59.704", &user("看看")),
        at("13:06.394", done),
        at("13:47.347", started),
        at("13:47.407", &user("再看看")),
        at("13:53.882", done),
    ]
    .concat();
    fs::write(&path, log).unwrap();
    let mut index = f.index();
    index.update(Agent::Codex, &path, true).unwrap();
    let path = path.to_str().unwrap();
    let (text, logged) = index.latest_user(path).unwrap().unwrap();
    assert_eq!(text, "再看看");
    assert_eq!(index.turn_began(path, logged).unwrap(), Some(logged));
    // The first prompt: its turn began at Enter, before the prompt was logged.
    let first = logged - 48;
    assert_eq!(index.turn_began(path, first).unwrap(), Some(first - 5));
    // A turn begun long before is one the prompt was typed into, not one it opened.
    assert_eq!(
        index.turn_began(path, logged + TURN_LEAD + 1).unwrap(),
        None
    );
}
#[test]
fn a_turn_longer_than_the_tail_still_reads_as_running() {
    use std::io::Write;
    let f = Fixture::new();
    let path = f.0.join("logs/long-turn.jsonl");
    let mut file = File::create(&path).unwrap();
    file.write_all(line("long").as_bytes()).unwrap();
    file.write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"\xe6\x8e\x92\xe6\x9f\xa5\"}}\n").unwrap();
    file.write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n")
        .unwrap();
    // Ten minutes of tool output: well past the 128 KiB tail, no turn boundary in it.
    let output = b"{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call_output\",\"call_id\":\"c\",\"output\":\"ok\"}}\n";
    for _ in 0..(400 * 1024 / output.len()) {
        file.write_all(output).unwrap();
    }
    drop(file);
    let mut index = f.index();
    index.update(Agent::Codex, &path, true).unwrap();
    let card = index.card(path.to_str().unwrap()).unwrap().unwrap();
    assert_eq!(card.turn, mooring_core::content::Turn::Running);
    assert!(index.counts.bytes < 4 * 1024 * 1024);
}
#[test]
fn bounded_tail_and_atomic_cursor_content_survive_restart() {
    use std::io::Write;
    let f = Fixture::new();
    let path = f.0.join("logs/content.jsonl");
    let mut file = File::create(&path).unwrap();
    file.write_all(line("content").as_bytes()).unwrap();
    file.write_all(&vec![b'x'; 2 * 1024 * 1024]).unwrap();
    file.write_all(b"\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"tail request\"}}\n").unwrap();
    let mut index = f.index();
    index.update(Agent::Codex, &path, true).unwrap();
    assert!(index.counts.bytes <= 512 * 1024 + 1);
    assert!(
        index
            .card(path.to_str().unwrap())
            .unwrap()
            .unwrap()
            .partial_history
    );
    drop(index);
    let mut index = f.index();
    let before = index.card(path.to_str().unwrap()).unwrap().unwrap();
    index.db.execute_batch("CREATE TRIGGER fail_record BEFORE INSERT ON session_records BEGIN SELECT RAISE(ABORT,'fixture atomic failure'); END;").unwrap();
    let extra=b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"additional requirement\"}}\n";
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(extra)
        .unwrap();
    assert!(index.update(Agent::Codex, &path, false).is_err());
    assert_eq!(index.card(path.to_str().unwrap()).unwrap().unwrap(), before);
    index.db.execute_batch("DROP TRIGGER fail_record").unwrap();
    index.update(Agent::Codex, &path, false).unwrap();
    assert_eq!(
        index.counts.bytes,
        extra.len() as u64 * 2,
        "failed transaction retries only the uncommitted delta"
    );
    let c = index.card(path.to_str().unwrap()).unwrap().unwrap();
    assert_eq!(c.asks.len(), 2);
    assert_eq!(c.asks[1].text, "additional requirement");
    let counts = index.counts.clone();
    index.update(Agent::Codex, &path, true).unwrap();
    assert_eq!(index.counts, counts);
}
#[test]
fn cursors_survive_restart_partial_lines_truncation_and_inode_replacement() {
    let f = Fixture::new();
    let path = f.0.join("logs/one.jsonl");
    let text = line("one");
    let cut = text.len() - 8;
    fs::write(&path, &text[..cut]).unwrap();
    let mut i = f.index();
    assert!(i.update(Agent::Codex, &path, false).unwrap().is_none());
    drop(i);
    let mut i = f.index();
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&text.as_bytes()[cut..])
        .unwrap();
    assert_eq!(
        i.update(Agent::Codex, &path, false)
            .unwrap()
            .unwrap()
            .cli_id,
        "one"
    );
    assert_eq!(i.counts.bytes, (text.len() - cut) as u64);
    let before = i.counts.clone();
    i.update(Agent::Codex, &path, false).unwrap();
    assert_eq!(
        i.counts, before,
        "duplicate event performs no file read/write"
    );
    // An index from the previous build has records but no generation table.
    i.db.execute_batch("DROP TABLE log_generations").unwrap();
    drop(i);
    let mut i = f.index();
    assert_eq!(i.cached().unwrap()[0].cli_id, "one");
    let original_generation = i.chat_generation(path.to_str().unwrap()).unwrap();
    assert_eq!(original_generation, "", "old records need no backfill");
    fs::write(&path, line("x")).unwrap();
    assert_eq!(
        i.update(Agent::Codex, &path, false)
            .unwrap()
            .unwrap()
            .cli_id,
        "x"
    );
    let truncated_generation = i.chat_generation(path.to_str().unwrap()).unwrap();
    assert_ne!(original_generation, truncated_generation);
    let replacement = f.0.join("replacement");
    fs::write(&replacement, line("replacement")).unwrap();
    fs::rename(replacement, &path).unwrap();
    assert_eq!(
        i.update(Agent::Codex, &path, false)
            .unwrap()
            .unwrap()
            .cli_id,
        "replacement"
    );
    assert_ne!(
        truncated_generation,
        i.chat_generation(path.to_str().unwrap()).unwrap()
    );
    fs::remove_file(&path).unwrap();
    assert!(i.update(Agent::Codex, &path, false).unwrap().is_none());
    assert!(i.cached().unwrap().is_empty());
}
#[test]
fn initial_index_skips_large_bodies_and_never_rewalks_completed_directories() {
    let f = Fixture::new();
    let path = f.0.join("logs/huge.jsonl");
    let mut file = File::create(&path).unwrap();
    file.write_all(line("huge").as_bytes()).unwrap();
    file.set_len(17 * 1024 * 1024 * 1024).unwrap();
    let mut i = f.index();
    let checkpoint = i.initialize().unwrap();
    i.bootstrap().unwrap();
    assert!(i.counts.bytes <= 512 * 1024 + 1);
    assert_eq!(i.cached().unwrap()[0].cli_id, "huge");
    assert_eq!(
        i.db.query_row("SELECT offset FROM log_files", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        17 * 1024 * 1024 * 1024
    );
    drop(i);
    let mut i = f.index();
    assert_eq!(i.initialize().unwrap(), checkpoint);
    i.bootstrap().unwrap();
    assert_eq!(
        i.counts,
        IoCounts::default(),
        "warm start has no directory walk or file reads"
    );
    assert_eq!(i.cached().unwrap().len(), 1);
}
#[test]
fn a_cli_supported_later_is_walked_once_and_only_where_transcripts_live() {
    let f = Fixture::new();
    fs::write(f.0.join("logs/a.jsonl"), line("codex-a")).unwrap();
    let mut i = f.index();
    i.initialize().unwrap();
    i.bootstrap().unwrap();
    drop(i);
    let session = f.0.join("grok/%2Frepo/01a1-grok");
    fs::create_dir_all(session.join("images/deep")).unwrap();
    fs::write(
        session.join("summary.json"),
        r#"{"info":{"id":"01a1-grok"}}"#,
    )
    .unwrap();
    fs::write(session.join("chat_history.jsonl"), "{}\n").unwrap();
    fs::write(
        session.join("updates.jsonl"),
        concat!(
            r#"{"timestamp":1800000000,"method":"session/update","params":{"sessionId":"01a1-grok","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"整理发布说明"}}}}"#,
            "\n",
            r#"{"timestamp":1800000001,"method":"_x.ai/session/update","params":{"sessionId":"01a1-grok","update":{"sessionUpdate":"turn_completed","stop_reason":"end_turn"}}}"#,
            "\n"
        ),
    )
    .unwrap();
    let open = || {
        Index::open(
            &f.0,
            vec![
                (Agent::Codex, f.0.join("logs")),
                (Agent::Grok, f.0.join("grok")),
            ],
        )
        .unwrap()
    };
    let mut i = open();
    i.initialize().unwrap();
    i.bootstrap().unwrap();
    // grok, its working directory and the session; not images/ or anything in logs/.
    assert_eq!(i.counts.directories, 3);
    let grok: Vec<_> = i
        .cached()
        .unwrap()
        .into_iter()
        .filter(|t| t.runtime == Agent::Grok)
        .collect();
    assert_eq!(grok.len(), 1);
    assert_eq!(
        (grok[0].cli_id.as_str(), grok[0].cwd.as_str()),
        ("01a1-grok", "/repo")
    );
    let card = i.card(&grok[0].path).unwrap().unwrap();
    assert_eq!(card.title, "整理发布说明");
    assert!(card.interactive);
    assert_eq!(card.hidden_reason(), None);
    drop(i);
    let mut i = open();
    i.initialize().unwrap();
    i.bootstrap().unwrap();
    assert_eq!(i.counts, IoCounts::default(), "the new CLI is walked once");
}
#[test]
fn a_change_while_the_host_was_away_is_replayed_or_scanned_for() {
    let f = Fixture::new();
    let mut i = f.index();
    let since = i.initialize().unwrap();
    let paths = vec![f.0.join("logs")];
    let watch = Watch::start(&paths, since).unwrap();
    i.bootstrap().unwrap();
    let first = paths[0].join("one.jsonl");
    fs::write(&first, line("one")).unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(10);
    let mut checkpoint = since;
    loop {
        let batch = watch
            .rx
            .recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
            .unwrap();
        let found = batch.events.iter().any(|e| e.path == first);
        for e in batch.events {
            assert_eq!(
                e.flags & (flag::SCAN | flag::IDS_WRAPPED) & !flag::ROOT_CHANGED,
                0,
                "event gap {e:?}"
            );
            checkpoint = checkpoint.max(e.id);
        }
        if found {
            break;
        }
    }
    i.update(Agent::Codex, &first, false).unwrap();
    i.checkpoint(checkpoint).unwrap();
    drop(watch);
    drop(i);
    let second = paths[0].join("two.jsonl");
    fs::write(&second, line("two")).unwrap();
    let mut i = f.index();
    let since = i.initialize().unwrap();
    i.bootstrap().unwrap();
    assert_eq!(i.counts.directories, 0);
    let watch = Watch::start(&paths, since).unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let batch = watch
            .rx
            .recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
            .unwrap();
        // FSEvents persists and replays it. Where the system keeps no journal, the watch
        // asks for a look through the root instead.
        if batch
            .events
            .iter()
            .any(|e| e.path == second || (e.path == paths[0] && e.flags & flag::SCAN != 0))
        {
            break;
        }
    }
    assert_eq!(
        i.update(Agent::Codex, &second, false)
            .unwrap()
            .unwrap()
            .cli_id,
        "two"
    );
    assert_eq!(i.cached().unwrap().len(), 2);
}

fn ask(text: &str) -> String {
    format!("{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"{text}\"}}}}\n")
}
/// An ask carrying a pasted image is one JSONL line well over 256 KiB (76 such
/// Claude asks in a week of real logs). It still counts as an ask.
#[test]
fn an_ask_line_over_256k_is_still_an_ask() {
    let f = Fixture::new();
    let path = f.0.join("logs/big.jsonl");
    let big = serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"看这张图","images":["x".repeat(300 * 1024)]}});
    fs::write(&path, format!("{}{}\n{}", line("big"), big, ask("然后呢"))).unwrap();
    let mut i = f.index();
    i.update(Agent::Codex, &path, false).unwrap();
    let card = i.card(path.to_str().unwrap()).unwrap().unwrap();
    let asks: Vec<_> = card.asks.iter().map(|a| a.text.as_str()).collect();
    assert_eq!(asks, ["看这张图", "然后呢"]);
}
/// Long strings are cut between whole characters, never inside an escape or a
/// surrogate pair, and a line stopped half-read resumes in the same place.
#[test]
fn eliding_long_strings_keeps_each_line_valid_json() {
    let tail = "é😀\\\"\\u00e9\\ud83d\\ude00x";
    for pad in 0..12 {
        let text = format!("{}{}", "a".repeat(STRING_KEEP - 6 + pad), tail.repeat(400));
        let raw = format!("{{\"k\":\"{text}\",\"next\":\"保留\"}}");
        assert!(serde_json::from_str::<serde_json::Value>(&raw).is_ok());
        for split in [0, raw.len() / 3, STRING_KEEP + pad, raw.len() - 3] {
            let mut kept = Vec::new();
            let mut e = Elide::default();
            for (n, &b) in raw.as_bytes().iter().enumerate() {
                if n == split {
                    e = Elide::over(&kept);
                }
                if e.keep(b) {
                    kept.push(b);
                }
            }
            let v: serde_json::Value = serde_json::from_slice(&kept)
                .unwrap_or_else(|e| panic!("pad {pad} split {split}: {e}"));
            assert_eq!(v["next"], "保留");
            assert!(kept.len() < STRING_KEEP + 64);
        }
    }
}

/// One unreadable log (e.g. written by `sudo claude`) does not stop every other
/// session from indexing.
#[test]
fn an_unreadable_log_does_not_block_the_index() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let bad = f.0.join("logs/a.jsonl");
    fs::write(&bad, line("a")).unwrap();
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
    let good = f.0.join("logs/b.jsonl");
    fs::write(&good, line("b")).unwrap();
    let mut i = f.index();
    let init = i.initialize();
    let boot = i.bootstrap();
    let card = i.card(good.to_str().unwrap()).ok().flatten();
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(init.is_ok(), "{init:?}");
    assert!(boot.is_ok(), "{boot:?}");
    assert!(card.is_some());
    // Readable again: it is picked up on its next change.
    fs::write(&bad, format!("{}{}", line("a"), line("a"))).unwrap();
    assert!(i.update(Agent::Codex, &bad, false).unwrap().is_some());
}

/// A log rewritten shorter in place keeps no words from before.
#[test]
fn a_rewritten_log_forgets_its_old_words() {
    let f = Fixture::new();
    let path = f.0.join("logs/re.jsonl");
    fs::write(
        &path,
        format!("{}{}", line("re"), ask("第一次的很长很长很长的要求")),
    )
    .unwrap();
    let mut i = f.index();
    i.update(Agent::Codex, &path, false).unwrap();
    fs::write(&path, format!("{}{}", line("re"), ask("新要求"))).unwrap();
    i.update(Agent::Codex, &path, false).unwrap();
    let words = i.original_words(path.to_str().unwrap()).unwrap();
    assert!(!words.contains("第一次"), "old words survive: {words}");
    assert!(words.contains("新要求"), "{words}");
}

#[test]
fn changing_a_runtime_root_imports_existing_files_once() {
    let f = Fixture::new();
    let second = f.0.join("second");
    fs::create_dir_all(&second).unwrap();
    // Both files predate the first checkpoint; B never emits a new event.
    fs::write(f.0.join("logs/a.jsonl"), line("root-a")).unwrap();
    fs::write(second.join("b.jsonl"), line("root-b")).unwrap();
    let mut first = f.index();
    let checkpoint = first.initialize().unwrap();
    first.bootstrap().unwrap();
    drop(first);
    let open = || Index::open(&f.0, vec![(Agent::Codex, second.clone())]).unwrap();
    let mut next = open();
    assert_eq!(next.initialize().unwrap(), checkpoint);
    next.bootstrap().unwrap();
    assert!(
        next.cached().unwrap().iter().any(|t| t.cli_id == "root-b"),
        "same runtime with a new root skipped existing files"
    );
    assert_eq!(next.counts.directories, 1);
    drop(next);
    let mut again = open();
    again.initialize().unwrap();
    again.bootstrap().unwrap();
    assert_eq!(
        again.counts,
        IoCounts::default(),
        "unchanged root was scanned again"
    );
}
