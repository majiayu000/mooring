//! Record-local normalization, sourced receipts and durable pending tool correlation.
use mooring_core::{
    content::{short, Content, Evidence, Quote, Source, Turn},
    model::Agent,
    protocol::HookFacts,
};
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    io,
    sync::LazyLock,
};
mod children;
mod db;
mod result;
mod rules;
pub use children::{children, link_child, Children};
pub use db::{card, load, save, save_records, save_state, schema};
use rules::*;
pub(crate) use rules::{claims_done, re, user_text};
// A session_meta line may exceed the 16 KiB head budget because of base_instructions.
// Decode only complete top-level metadata values in its prefix, never scan its body.
pub fn header_value(runtime: Agent, bytes: &[u8]) -> Option<Value> {
    crate::agents::adapter(runtime).header_value(bytes)
}
pub(crate) fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b.as_str().or_else(|| b.get("text").and_then(Value::as_str)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        _ => v.to_string(),
    }
}
pub(crate) fn input(v: &Value) -> Value {
    v.as_str()
        .map(|s| serde_json::from_str(s).unwrap_or_else(|_| json!({"input":s})))
        .unwrap_or_else(|| v.clone())
}
/// Lines an Edit, MultiEdit, Write or NotebookEdit call added and removed, by comparing
/// its old and new text as multisets of lines: what both sides share is unchanged.
pub(crate) fn edit_lines(input: &Value) -> (u32, u32) {
    fn delta(old: &str, new: &str) -> (u32, u32) {
        let mut left: HashMap<&str, i64> = HashMap::new();
        for line in old.lines() {
            *left.entry(line).or_default() += 1;
        }
        let mut added = 0;
        for line in new.lines() {
            match left.get_mut(line) {
                Some(n) if *n > 0 => *n -= 1,
                _ => added += 1,
            }
        }
        let removed = left.values().filter(|n| **n > 0).sum::<i64>() as u32;
        (added, removed)
    }
    let text = |key: &str| input[key].as_str().unwrap_or("");
    if let Some(edits) = input["edits"].as_array() {
        return edits.iter().fold((0, 0), |(a, r), e| {
            let (da, dr) = delta(
                e["old_string"].as_str().unwrap_or(""),
                e["new_string"].as_str().unwrap_or(""),
            );
            (a + da, r + dr)
        });
    }
    if input.get("old_string").is_some() || input.get("new_string").is_some() {
        return delta(text("old_string"), text("new_string"));
    }
    // A whole file written: what it held before is not in the call.
    for key in ["content", "new_source"] {
        if let Some(body) = input[key].as_str() {
            return (body.lines().count() as u32, 0);
        }
    }
    (0, 0)
}
/// Each file an apply_patch names, with the `+` and `-` lines of its section.
/// Where a kept call input holds its edit's line counts and its patch's files, worked out
/// when the call arrived.
pub(crate) const LINES: &str = "_lines";
pub(crate) const PATCHED: &str = "_patch";
/// Fields of a call's input that its result is read against. The rest (a Write's whole
/// file, an Edit's old and new text) is reduced to line counts up front, so the pending
/// calls saved after every log line stay small.
fn kept_input(args: &Value) -> Value {
    let mut kept = serde_json::Map::new();
    for key in [
        "session_id",
        "file_path",
        "path",
        "notebook_path",
        "cmd",
        "command",
    ] {
        if let Some(v) = args.get(key) {
            kept.insert(key.into(), v.clone());
        }
    }
    let lines = edit_lines(args);
    if lines != (0, 0) {
        kept.insert(LINES.into(), json!(lines));
    }
    let patch = patch_lines(patch_text(args));
    if !patch.is_empty() {
        kept.insert(PATCHED.into(), json!(patch));
    }
    Value::Object(kept)
}
pub(crate) fn patch_text(input: &Value) -> &str {
    input["input"]
        .as_str()
        .or_else(|| input["patch"].as_str())
        .unwrap_or("")
}
pub(crate) fn patch_lines(patch: &str) -> Vec<(String, u32, u32)> {
    let mut out: Vec<(String, u32, u32)> = Vec::new();
    for line in patch.lines() {
        if let Some(c) = PATCH.captures(line) {
            out.push((c[1].to_owned(), 0, 0));
        } else if let Some(last) = out.last_mut() {
            if line.starts_with("*** ") {
                continue;
            }
            if line.starts_with('+') {
                last.1 += 1;
            } else if line.starts_with('-') {
                last.2 += 1;
            }
        }
    }
    out
}
/// The first question's option labels, in order, for a question tool's arguments.
pub(crate) fn question_choices(v: &Value) -> Vec<String> {
    v.get("questions")
        .and_then(Value::as_array)
        .and_then(|q| q.first())
        .and_then(|q| q["options"].as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|o| o.as_str().or_else(|| o["label"].as_str()))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
pub(crate) fn question_summary(v: &Value) -> String {
    let questions = v.get("questions").and_then(Value::as_array);
    if let Some(questions) = questions {
        return short(
            &questions
                .iter()
                .filter_map(|q| {
                    let prompt = q["question"].as_str().or_else(|| q["title"].as_str())?;
                    let options = q["options"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|o| {
                                    o.as_str()
                                        .or_else(|| o["label"].as_str())
                                        .map(str::to_owned)
                                })
                                .collect::<Vec<_>>()
                                .join(" / ")
                        })
                        .unwrap_or_default();
                    // The question's short title leads, 【范围】, for the status chip's phrase
                    // (mooring-core reasons rule `question-header`).
                    let title = q["header"]
                        .as_str()
                        .map(str::trim)
                        .filter(|h| !h.is_empty() && !h.contains(['【', '】', '\n']))
                        .map(|h| format!("【{h}】"))
                        .unwrap_or_default();
                    Some(if options.is_empty() {
                        format!("{title}{prompt}")
                    } else {
                        format!("{title}{prompt}（{options}）")
                    })
                })
                .collect::<Vec<_>>()
                .join("\n"),
            600,
        );
    }
    short(
        v["question"]
            .as_str()
            .or_else(|| v["message"].as_str())
            .or_else(|| v["reason"].as_str())
            .unwrap_or("需要在原终端回答问题或审批"),
        600,
    )
}
pub fn seconds(v: &Value) -> u64 {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp().max(0) as u64)
        .or_else(|| v.as_u64())
        .unwrap_or(0)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Call {
    name: String,
    input: Value,
    source: Source,
    at: u64,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Parser {
    pub card: Content,
    /// The paths this turn pushed off the card's list of files, hashed.
    #[serde(default)]
    evicted_files: std::collections::HashSet<u64>,
    turn_id: String,
    last_user: String,
    last_user_turn: String,
    pub(crate) calls: BTreeMap<String, Call>,
    /// Reply text a CLI streams in chunks, put on the card once its segment ends.
    #[serde(default)]
    pub(crate) reply: String,
    executions: BTreeMap<String, String>,
    /// A turn end arrived while a dialog was still shown; settles the turn once the dialog
    /// is known to be gone.
    #[serde(default)]
    completed_while_waiting: bool,
    /// The turn a subagent's dialog interrupted, restored once that dialog is gone.
    #[serde(default)]
    pub(crate) child_wait_from: Option<Turn>,
    /// The card's name came from a title the user or the desktop app set, which a
    /// generated title does not replace.
    #[serde(default)]
    pub(crate) custom_name: bool,
    /// The record being read was copied from another conversation (a fork's history), so
    /// the changes it shows were made by that one.
    #[serde(skip)]
    pub(crate) inherited: bool,
    #[serde(skip)]
    pub records: Vec<Record>,
    /// "Running→Waiting Hook" lines for the status trace, one per change of `card.turn`.
    #[serde(skip)]
    pub transitions: Vec<String>,
    /// The content rules this state was built under, `RULES` when saved. State from older
    /// rules is rebuilt from the log instead of continued.
    #[serde(rename = "_rules", default, serialize_with = "current_rules")]
    pub(crate) rules: u32,
}
/// Bumped when parsing rules change what a saved state means; such states are reseeded.
pub const RULES: u32 = 14;
fn current_rules<S: serde::Serializer>(_: &u32, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u32(RULES)
}
/// What `claude agents --json` says about a live session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Official {
    Waiting(String),
    Busy,
    Idle,
}
#[derive(Clone, Debug)]
pub struct Record {
    pub source: Source,
    pub at: u64,
    pub kind: String,
    pub text: String,
    pub call_id: String,
    pub code: Option<i32>,
}
impl Parser {
    pub fn observe_header(&mut self, runtime: Agent, line: &[u8]) {
        if let Some(v) = header_value(runtime, line) {
            self.metadata(runtime, &v);
        }
    }
    pub(crate) fn metadata(&mut self, runtime: Agent, v: &Value) {
        crate::agents::adapter(runtime).metadata(self, v);
    }
    /// The first reason a session is not the user's own conversation wins.
    pub(crate) fn hide(&mut self, reason: &str) {
        if self.card.hidden_reason.is_empty() {
            self.card.hidden_reason = reason.into();
        }
    }
    /// Some launchers inherit the desktop entrypoint. Require an explicit machine task marker.
    pub(crate) fn machine_prompt(&mut self, prompt: &str) {
        if (prompt.starts_with("Use the you-are-here skill")
            && prompt.contains("This is an unattended background refresh:"))
            || prompt.starts_with("This is an unattended background refresh:")
            || prompt.starts_with("You are a Claude-Mem")
            || prompt.starts_with("You are a memory extraction agent")
            || prompt.starts_with("<remem-hook")
        {
            self.card.hidden_reason = "Hook / 无人值守后台任务".into();
        }
    }
    fn record(
        &mut self,
        kind: &str,
        text: String,
        call_id: &str,
        code: Option<i32>,
        at: u64,
        source: &Source,
    ) {
        self.records.push(Record {
            source: source.clone(),
            at,
            kind: kind.into(),
            text,
            call_id: call_id.into(),
            code,
        });
    }
    pub(crate) fn message(
        &mut self,
        user: bool,
        s: String,
        at: u64,
        source: &Source,
        final_answer: bool,
    ) {
        let s = if user { user_text(&s) } else { s };
        if s.is_empty() {
            return;
        }
        if user {
            if self
                .card
                .asks
                .iter()
                .any(|q| q.source == *source && q.text == s)
            {
                return;
            }
            if self.last_user == s && self.last_user_turn == self.turn_id {
                return;
            }
            self.last_user = s.clone();
            self.last_user_turn = self.turn_id.clone();
            if self.card.asks.is_empty() && !self.card.partial_history {
                self.card.first_ask_known = true;
                self.card.title = short(&s.replace('\n', " "), 70);
            }
            self.record("user", s.clone(), "", None, at, source);
            self.card.asks.push(Quote {
                text: short(&s, 1200),
                at,
                source: source.clone(),
            });
            if self.card.asks.len() > 8 {
                self.card.asks.remove(1);
            }
            self.card.turn = Turn::Running;
            self.card.waiting.clear();
            self.card.choices.clear();
            self.card.error.clear();
            self.card.completed_claim = false;
        } else {
            self.record("assistant", s.clone(), "", None, at, source);
            self.card.last_reply = short(&s, 1400);
            if final_answer {
                self.card.completed_claim = claims_done(&s);
            }
        }
        self.card.updated = self.card.updated.max(at);
    }
    pub(crate) fn phase(&mut self, turn: Turn, id: &str, at: u64, source: &Source) {
        if turn == Turn::Running {
            self.turn_id = if id.is_empty() {
                format!("{at}:{}", source.offset)
            } else {
                id.into()
            };
            self.card.turn_started = at;
            self.card.turn_key = self.turn_id.clone();
            self.card.completed_claim = false;
            self.card.waiting.clear();
            self.card.choices.clear();
            self.card.error.clear();
            self.card.background = None;
            self.card.shells.clear();
            self.completed_while_waiting = false;
            self.child_wait_from = None;
        }
        let still_waiting = self.card.turn == Turn::Waiting
            && !self.card.waiting.is_empty()
            && turn == Turn::Completed;
        self.completed_while_waiting |= still_waiting;
        self.card.turn = if still_waiting { Turn::Waiting } else { turn };
        self.card.updated = self.card.updated.max(at);
        self.record("turn", format!("{turn:?}"), id, None, at, source);
    }
    pub(crate) fn tool(&mut self, id: &str, name: &str, args: Value, at: u64, source: &Source) {
        if id.is_empty() || name.is_empty() {
            return;
        }
        self.record(
            "tool_call",
            format!("{name} {}", db::logged_args(name, &args)),
            id,
            None,
            at,
            source,
        );
        if name == "AskUserQuestion"
            || name.ends_with("request_user_input")
            || name.ends_with("request_user_input_async")
        {
            self.card.turn = Turn::Waiting;
            self.child_wait_from = None;
            self.card.waiting = question_summary(&args);
            self.card.choices = question_choices(&args);
        }
        self.calls.insert(
            id.into(),
            Call {
                name: name.into(),
                input: kept_input(&args),
                source: source.clone(),
                at,
            },
        );
        if self.calls.len() > 32 {
            let oldest = self
                .calls
                .iter()
                .min_by_key(|(_, c)| c.source.offset)
                .map(|(k, _)| k.clone())
                .unwrap();
            self.calls.remove(&oldest);
            self.card.unknown_records += 1;
        }
        self.card.updated = self.card.updated.max(at);
    }
    fn fact(&mut self, kind: &str, summary: String, code: Option<i32>, at: u64, source: &Source) {
        self.card.evidence.push(Evidence {
            kind: kind.into(),
            summary: short(&summary, 400),
            exit_code: code,
            at,
            source: source.clone(),
        });
        if self.card.evidence.len() > 16 {
            self.card.evidence.remove(0);
        }
    }
    pub fn add(&mut self, runtime: Agent, line: &[u8], source: Source) {
        let before = self.card.turn;
        let from = self.records.len();
        let record = self.add_line(runtime, line, source);
        let detail = match record {
            Some(v) if self.card.turn != before => {
                log_detail(&v, self.records.get(from..).unwrap_or_default())
            }
            _ => String::new(),
        };
        self.settle(before, "日志", self.card.updated, detail);
    }
    fn add_line(&mut self, runtime: Agent, line: &[u8], source: Source) -> Option<Value> {
        let Ok(v) = serde_json::from_slice::<Value>(line) else {
            self.card.unknown_records += 1;
            return None;
        };
        self.metadata(runtime, &v);
        let at = seconds(&v["timestamp"]);
        crate::agents::adapter(runtime).add(self, &v, at, &source);
        Some(v)
    }
    pub fn hook(&mut self, event: &str, facts: &HookFacts, at: u64, source: Source) {
        let before = self.card.turn;
        self.hook_event(event, facts, at, &source);
        let detail = match &facts.notification {
            _ if self.card.turn == before => String::new(),
            Some(n) if event == "Notification" => format!("事件：{event}（{n}）"),
            _ => format!("事件：{event}"),
        };
        self.settle(before, "Hook", at, detail);
    }
    fn hook_event(&mut self, event: &str, facts: &HookFacts, at: u64, source: &Source) {
        // SubagentStop lists the parent's tasks; other subagent events say nothing about
        // the parent turn.
        if event == "SubagentStop" {
            if facts.background.is_some() {
                self.card.background = facts.background;
                self.card.shells = shell_tasks(facts);
            }
            return;
        }
        if facts.subagent {
            return;
        }
        // Work that will resume the session: a tool after the stop is that resumed turn.
        let keeps_working = self
            .card
            .background
            .is_some_and(|b| b.keeps_working() || b.wakes());
        match event {
            "PermissionRequest" => self.wait("CLI 请求工具审批（Hook）", at, source),
            "Notification" => match facts.notification.as_deref() {
                Some("permission_prompt") => self.wait("CLI 请求工具审批（Hook）", at, source),
                Some("elicitation_dialog") => self.wait("MCP 表单等待填写（Hook）", at, source),
                Some("elicitation_url_dialog") => {
                    self.wait("MCP 请求在浏览器中完成（Hook）", at, source)
                }
                Some("agent_needs_input") => self.wait("Agent 等待你的输入（Hook）", at, source),
                _ => (),
            },
            "Stop" => {
                // Absent arrays are unknown; an older turn's tasks never carry over.
                self.card.background = facts.background;
                self.card.shells = shell_tasks(facts);
                self.card.completed_claim = claims_done(&self.card.last_reply);
                self.phase(Turn::Completed, "", at, source);
            }
            "StopFailure" => {
                if self.card.error.is_empty() {
                    self.card.error = "API 错误中止了回合（Hook）".into();
                }
                self.phase(Turn::Error, "", at, source);
            }
            // A tool finishing after a hook-reported dialog means the dialog was answered.
            "PostToolUse" | "PostToolUseFailure"
                if self.card.turn == Turn::Waiting && self.card.signal == "Hook" =>
            {
                self.card.turn = Turn::Running;
                self.card.waiting.clear();
                self.card.choices.clear();
            }
            // The prompt is in: the turn runs, whether or not its log record is read yet.
            "UserPromptSubmit" if !matches!(self.card.turn, Turn::Running | Turn::Waiting) => {
                self.phase(Turn::Running, "", at, source);
            }
            // Turns woken by background work send no prompt; a tool after a stop is the only
            // sign of one. Without running work it is a straggler from the finished turn.
            // A tool in a turn whose start was never read is that turn still running.
            "PreToolUse" | "PostToolUse" | "PostToolUseFailure"
                if self.card.turn == Turn::Completed && keeps_working
                    || self.card.turn == Turn::Unknown =>
            {
                self.phase(Turn::Running, "", at, source);
            }
            _ => (),
        }
    }
    fn wait(&mut self, reason: &str, at: u64, source: &Source) {
        // A dialog already known from the transcript or screen keeps its richer text.
        if self.card.turn != Turn::Waiting {
            self.card.waiting = reason.into();
            self.phase(Turn::Waiting, "", at, source);
        }
    }
    /// Applies `claude agents --json`. It only settles a dialog: it never starts or ends a
    /// turn the transcript and hooks have not reported, because it can lag either of them.
    /// A parent status cannot dismiss a dialog projected from a child's own log.
    pub fn official(&mut self, status: &Official, at: u64, source: Source) {
        if self.card.turn == Turn::Waiting && self.child_wait_from.is_some() {
            return;
        }
        let before = self.card.turn;
        match status {
            Official::Waiting(reason) if self.card.turn != Turn::Waiting => {
                self.card.waiting = format!("{reason}（Claude 官方状态）");
                self.phase(Turn::Waiting, "", at, &source);
            }
            Official::Busy if self.card.turn == Turn::Waiting => {
                self.card.turn = Turn::Running;
                self.card.waiting.clear();
                self.card.choices.clear();
            }
            Official::Idle if self.card.turn == Turn::Waiting => {
                self.card.waiting.clear();
                self.card.choices.clear();
                let settled = if self.completed_while_waiting {
                    Turn::Completed
                } else {
                    Turn::Interrupted
                };
                self.phase(settled, "", at, &source);
            }
            _ => (),
        }
        let detail = match status {
            _ if self.card.turn == before => String::new(),
            Official::Waiting(reason) => format!("报告：在等（{reason}）"),
            Official::Busy => "报告：在忙".into(),
            Official::Idle => "报告：空闲".into(),
        };
        self.settle(before, "官方状态", at, detail);
    }
    /// Applies the state of a session's subagents, read from their own logs: running
    /// ones keep a stopped turn working, and a subagent's dialog is the parent's dialog.
    pub fn children(&mut self, kids: &Children, at: u64, source: Source) {
        let before = self.card.turn;
        if kids.running > 0 || self.card.background.is_some() {
            let mut b = self.card.background.unwrap_or_default();
            b.agents = kids.running;
            self.card.background = Some(b);
        }
        match &kids.waiting {
            Some(w) if self.card.turn != Turn::Waiting => {
                self.child_wait_from = Some(self.card.turn);
                self.card.waiting = w.clone();
                self.phase(Turn::Waiting, "", at, &source);
            }
            None if self.card.turn == Turn::Waiting => {
                if let Some(turn) = self.child_wait_from.take() {
                    self.card.waiting.clear();
                    self.card.choices.clear();
                    self.card.turn = if self.completed_while_waiting {
                        Turn::Completed
                    } else {
                        turn
                    };
                }
            }
            _ => (),
        }
        self.card.keep_subagents(kids.subagents.clone());
        self.settle(before, "子 Agent", at, String::new());
    }
    /// `detail` is the card's `signal_detail` when the turn changed, built only then.
    fn settle(&mut self, before: Turn, signal: &str, at: u64, detail: String) {
        if self.card.turn == before {
            return;
        }
        if self.card.turn == Turn::Waiting {
            self.card.waiting_episode += 1;
        } else {
            self.completed_while_waiting = false;
            self.child_wait_from = None;
        }
        self.card.signal = signal.into();
        self.card.signal_at = at;
        self.card.signal_detail = detail;
        self.transitions
            .push(format!("{before:?}→{:?} {signal}", self.card.turn));
    }
}
/// The log record that changed the turn: its type (with the payload type or subtype that
/// refines it) and a snippet of what it said, from the newest record it produced.
fn log_detail(v: &Value, records: &[Record]) -> String {
    let ty = v["type"].as_str().unwrap_or("?");
    let kind = match v["payload"]["type"].as_str().or(v["subtype"].as_str()) {
        Some(sub) => format!("{ty}·{sub}"),
        None => ty.to_owned(),
    };
    let said = records
        .iter()
        .rev()
        .find(|r| r.kind != "turn" && !r.text.trim().is_empty());
    match said {
        Some(r) => format!(
            "记录：{kind}「{}」",
            short(&mooring_core::content::plain_line(&r.text), 60)
        ),
        None => format!("记录：{kind}"),
    }
}
/// Claude Code lists the watch it keeps on a published Artifact (new versions and
/// comments) as a running "monitor" for as long as the session lives. It starts no turn
/// on its own, so it is not background work: Claude Code 2.1.316 describes it as
/// "live updates for artifact <slug> (...)". A Monitor the agent armed itself (a shell is
/// listed as "shell", a WebSocket as "monitor") carries the agent's own description.
const ARTIFACT_WATCH: &str = "live updates for artifact ";
/// The card's record of the shells a hook listed: descriptions, never commands.
fn shell_tasks(facts: &HookFacts) -> Vec<mooring_core::content::ShellTask> {
    facts
        .shells
        .iter()
        .map(|t| mooring_core::content::ShellTask {
            id: t.id.clone(),
            description: t.description.clone(),
            ..Default::default()
        })
        .collect()
}
/// Reads the status facts from a hook payload. Stop and SubagentStop list the parent
/// session's in-flight tasks; an entry counts unless its status says it has ended or it
/// is the watch on a published Artifact.
pub fn hook_facts(v: &Value) -> HookFacts {
    const ENDED: [&str; 9] = [
        "completed",
        "complete",
        "done",
        "failed",
        "error",
        "killed",
        "stopped",
        "cancelled",
        "canceled",
    ];
    let tasks = v["background_tasks"].as_array();
    let crons = v["session_crons"].as_array();
    let mut shells = Vec::new();
    let background = (tasks.is_some() || crons.is_some()).then(|| {
        let mut b = mooring_core::content::Background {
            crons: crons.map_or(0, |c| c.len() as u32),
            ..Default::default()
        };
        for task in tasks.into_iter().flatten() {
            let status = task["status"].as_str().unwrap_or("").to_lowercase();
            if ENDED.contains(&status.as_str()) {
                continue;
            }
            match task["type"].as_str().unwrap_or("") {
                "shell" => {
                    b.shells += 1;
                    let text = |k: &str| task[k].as_str().unwrap_or("").to_owned();
                    shells.push(mooring_core::protocol::HookShell {
                        id: text("id"),
                        description: text("description"),
                        command: text("command"),
                    });
                }
                "monitor"
                    if task["description"]
                        .as_str()
                        .is_some_and(|d| d.starts_with(ARTIFACT_WATCH)) => {}
                "monitor" => b.monitors += 1,
                _ => b.agents += 1,
            }
        }
        b
    });
    HookFacts {
        background,
        notification: v["notification_type"].as_str().map(str::to_owned),
        subagent: v["agent_id"].as_str().is_some_and(|id| !id.is_empty()),
        shells,
    }
}
#[cfg(test)]
mod tests;
