use crate::db::{Db, RunRow, StageRow};
use crate::run_notice;
use crate::task_scripts;
use crate::tasks::{self, git, Entry, Tasks};
use kanna_mux::json::{json, num, opt, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, PaneState, PaneType, Responder, WeakCore};
use kanna_pipe::{Arrow, Context, Kind, Pipeline, Sizes, Stage, Verdict, DONE};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};


const SCRATCH: &str = ".kanna/tmp";

const FEEDBACK_LINES: usize = 80;

const FEEDBACK_CHARS: usize = 8000;

const FAILURE_MARKS: &[&str] = &["FAILED", "error[", "panic", "✗", "failed"];

const ANSWER_CHARS: usize = 64 << 10;

const SETTLE: f64 = 10.0;

const SCREEN_LINES: usize = 20;

const CARRY_ON: &str = "Carry on.";

const DRAFT: &str = "pipeline.kojo";

const ITEM_NAME: usize = 24;

pub const LIVE: &[&str] = &["running", "waiting", "paused"];

pub const QUEUE: &str = "queue";

pub type OnEnd = Box<dyn Fn(&str, Result<String, String>)>;

pub type OnTell = Box<dyn Fn(&str, &str)>;

pub struct Place {
    pub stage: Option<String>,
    pub state: String,
    pub why: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub k: u64,
    pub command: String,
    pub log: String,
    pub exit: String,
    pub terminal: Option<String>,
    pub started: f64,
    pub awaited: bool,
    pub code: Option<i32>,
}

impl Job {
    pub fn to_value(&self) -> Value {
        json!({ "k": self.k, "command": self.command, "log": self.log, "exit": self.exit, "terminal": opt(self.terminal.as_deref()), "startedAt": num(self.started), "awaited": self.awaited, "code": self.code })
    }

    pub fn from_value(value: &Value) -> Option<Job> {
        Some(Job {
            k: value.get("k")?.as_u64()?,
            command: string(value, "command")?.to_string(),
            log: string(value, "log")?.to_string(),
            exit: string(value, "exit")?.to_string(),
            terminal: string(value, "terminal").map(String::from),
            started: value.get("startedAt").and_then(Value::as_f64).unwrap_or(0.0),
            awaited: value.get("awaited").and_then(Value::as_bool).unwrap_or(false),
            code: value.get("code").and_then(Value::as_i64).map(|c| c as i32),
        })
    }
}

pub fn command_timeout(within: Option<Duration>) -> u64 {
    within.map_or(0, |within| within.as_secs().max(1))
}

pub fn ends_owned(owned: bool, note: Option<&str>) -> bool {
    owned && note == Some("timed out")
}

pub fn unreported(stage: &str, verdicts: bool) -> String {
    let done = if verdicts { "`\"$KANNA_CLI\" run done pass` or `\"$KANNA_CLI\" run done fail`" } else { "`\"$KANNA_CLI\" run done`" };
    format!("Your turn ended, but {stage} isn't over: only {done} ends it, and nobody reads what you wrote on screen. Finish what's left, write your answer to the file your prompt names, then run it. If you can't finish without the user, run `\"$KANNA_CLI\" run need \"WHAT YOU NEED\"`. Nothing you left running in your own shell is still running; ask Kanna for a long command with `\"$KANNA_CLI\" run command 'COMMAND'`.")
}

pub fn command_ended(job: &Job, code: i32) -> String {
    format!("The command you asked Kanna to run ended while your turn was over, so Kanna carries you on.\n\n    {}\n\nIt exited {code}, and all its output is in `{}`. Carry on from there, and report when you're done.", job.command, job.log)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Feedback {
    pub stage: String,
    pub text: String,
    pub full: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ended {
    pub verdict: Option<Verdict>,
    pub shown: &'static str,
    pub answer: String,
    pub feedback: String,
    pub full: String,
    pub failures: Option<Vec<String>>,
    pub note: Option<String>,
    pub by: Option<String>,
}

impl Ended {
    fn to_value(&self) -> Value {
        json!({ "verdict": self.shown, "answer": self.answer, "feedback": self.feedback, "full": self.full, "failures": self.failures, "note": opt(self.note.as_deref()), "by": opt(self.by.as_deref()) })
    }

    fn from_value(value: &Value) -> Ended {
        let mut ended = reported(value);
        ended.feedback = string(value, "feedback").unwrap_or(&ended.answer).to_string();
        ended.full = string(value, "full").unwrap_or("").to_string();
        ended.failures = value.get("failures").and_then(Value::as_array).map(|l| l.iter().filter_map(Value::as_str).map(String::from).collect());
        ended.note = string(value, "note").map(String::from);
        ended.by = string(value, "by").map(String::from);
        ended
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub name: String,
    pub text: String,
    pub worktree: String,
    pub branch: String,
    pub row: i64,
    pub head: String,
    pub ended: Option<Ended>,
    pub score: Option<f64>,
}

impl Item {
    fn to_value(&self) -> Value {
        json!({ "name": self.name, "text": self.text, "worktree": self.worktree, "branch": self.branch, "row": self.row, "head": self.head, "ended": self.ended.as_ref().map_or(Value::Null, Ended::to_value), "score": self.score })
    }

    fn from_value(value: &Value) -> Item {
        let text = |key: &str| string(value, key).unwrap_or("").to_string();
        Item {
            name: text("name"),
            text: text("text"),
            worktree: text("worktree"),
            branch: text("branch"),
            row: value.get("row").and_then(Value::as_i64).unwrap_or(0),
            head: text("head"),
            ended: value.get("ended").filter(|e| e.is_object()).map(Ended::from_value),
            score: value.get("score").and_then(Value::as_f64),
        }
    }

    fn label(&self, stage: &str) -> String {
        format!("{stage} · {}", self.name)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub stage: String,
    pub visit: i64,
    pub start: String,
    pub attempts: bool,
    pub items: Vec<Item>,
    pub before: Option<f64>,
    pub main: Option<Ended>,
    pub after: Option<f64>,
    pub row: i64,
}

impl Step {
    fn to_value(&self) -> Value {
        json!({
            "stage": self.stage, "visit": self.visit, "start": self.start, "attempts": self.attempts,
            "items": self.items.iter().map(Item::to_value).collect::<Vec<_>>(),
            "before": self.before, "main": self.main.as_ref().map_or(Value::Null, Ended::to_value), "after": self.after, "row": self.row,
        })
    }

    fn from_value(value: &Value) -> Step {
        Step {
            stage: string(value, "stage").unwrap_or("").to_string(),
            visit: value.get("visit").and_then(Value::as_i64).unwrap_or(1),
            start: string(value, "start").unwrap_or("").to_string(),
            attempts: value.get("attempts") == Some(&Value::Bool(true)),
            items: items_of(value.get("items")),
            before: value.get("before").and_then(Value::as_f64),
            main: value.get("main").filter(|m| m.is_object()).map(Ended::from_value),
            after: value.get("after").and_then(Value::as_f64),
            row: value.get("row").and_then(Value::as_i64).unwrap_or(0),
        }
    }
}

fn items_of(value: Option<&Value>) -> Vec<Item> {
    value.and_then(Value::as_array).map_or(vec![], |l| l.iter().map(Item::from_value).collect())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Doc {
    pub request: String,
    pub file: String,
    pub worker: String,
    pub base: String,
    pub revision: i64,
    pub taken: Vec<u32>,
    pub feedback: Option<Feedback>,
    pub failures: BTreeMap<String, Vec<String>>,
    pub starts: BTreeMap<String, String>,
    pub answers: BTreeMap<String, String>,
    pub sessions: u32,
    pub visits: u32,
    pub next: Option<String>,
    pub why: Option<String>,
    pub note: Option<String>,
    pub step: Option<Step>,
    pub parts: BTreeMap<String, Vec<Item>>,
    pub owner: Option<String>,
    pub pausing: bool,
    pub held: bool,
    pub interventions: BTreeMap<String, u32>,
}

impl Doc {
    fn to_value(&self) -> Value {
        json!({
            "request": self.request,
            "file": self.file,
            "worker": self.worker,
            "base": self.base,
            "revision": self.revision,
            "taken": self.taken,
            "feedback": self.feedback.as_ref().map_or(Value::Null, |f| json!({ "stage": f.stage, "text": f.text, "full": f.full })),
            "failures": self.failures,
            "starts": self.starts,
            "answers": self.answers,
            "sessions": self.sessions,
            "visits": self.visits,
            "next": opt(self.next.as_deref()),
            "why": opt(self.why.as_deref()),
            "note": opt(self.note.as_deref()),
            "step": self.step.as_ref().map_or(Value::Null, Step::to_value),
            "parts": self.parts.iter().map(|(k, v)| (k.clone(), Value::Array(v.iter().map(Item::to_value).collect()))).collect::<Map>(),
            "owner": opt(self.owner.as_deref()),
            "pausing": self.pausing,
            "held": self.held,
            "interventions": self.interventions,
        })
    }

    fn from_value(value: &Value) -> Doc {
        let text = |key: &str| string(value, key).unwrap_or("").to_string();
        let map = |key: &str| -> BTreeMap<String, String> { value.get(key).and_then(Value::as_object).map_or_else(BTreeMap::new, |m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()) };
        let lists = value.get("failures").and_then(Value::as_object).map_or_else(BTreeMap::new, |m| m.iter().map(|(k, v)| (k.clone(), v.as_array().map_or(vec![], |l| l.iter().filter_map(Value::as_str).map(String::from).collect()))).collect());
        let feedback = value.get("feedback").filter(|f| f.is_object()).map(|f| Feedback { stage: string(f, "stage").unwrap_or("").into(), text: string(f, "text").unwrap_or("").into(), full: string(f, "full").unwrap_or("").into() });
        Doc {
            request: text("request"),
            file: text("file"),
            worker: text("worker"),
            base: text("base"),
            revision: value.get("revision").and_then(Value::as_i64).unwrap_or(1),
            taken: value.get("taken").and_then(Value::as_array).map_or(vec![], |l| l.iter().map(|n| n.as_u64().unwrap_or(0) as u32).collect()),
            feedback,
            failures: lists,
            starts: map("starts"),
            answers: map("answers"),
            sessions: value.get("sessions").and_then(Value::as_u64).unwrap_or(0) as u32,
            visits: value.get("visits").and_then(Value::as_u64).unwrap_or(0) as u32,
            next: string(value, "next").map(String::from),
            why: string(value, "why").map(String::from),
            note: string(value, "note").map(String::from),
            step: value.get("step").filter(|s| s.is_object()).map(Step::from_value),
            parts: value.get("parts").and_then(Value::as_object).map_or_else(BTreeMap::new, |m| m.iter().map(|(k, v)| (k.clone(), items_of(Some(v)))).collect()),
            owner: string(value, "owner").map(String::from),
            pausing: value.get("pausing").and_then(Value::as_bool).unwrap_or(false),
            held: value.get("held").and_then(Value::as_bool).unwrap_or(false),
            interventions: value.get("interventions").and_then(Value::as_object).map_or_else(BTreeMap::new, |m| m.iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0) as u32)).collect()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Next {
    Stage { name: String, reset: bool },
    Done,
    Stuck,
}

pub fn decide(pipeline: &Pipeline, doc: &mut Doc, stage: &str, verdict: Option<Verdict>, failures: Option<Vec<String>>) -> Next {
    let stalled = matches!((&failures, verdict), (Some(now), Some(Verdict::Fail)) if doc.failures.get(stage) == Some(now));
    if let Some(now) = failures {
        doc.failures.insert(stage.to_string(), now);
    }
    if doc.taken.len() < pipeline.arrows.len() {
        doc.taken.resize(pipeline.arrows.len(), 0);
    }
    let Some((index, arrow)) = pipeline.pick(stage, verdict, &doc.taken, stalled) else { return Next::Stuck };
    doc.taken[index] += 1;
    if arrow.to == DONE {
        return Next::Done;
    }
    Next::Stage { name: arrow.to.clone(), reset: arrow.reset }
}

pub fn over_budget(pipeline: &Pipeline, doc: &Doc, stage: &Stage, sizes: &Sizes, elapsed: Duration) -> Option<String> {
    let budget = pipeline.budget.as_ref()?;
    let cost = stage.sessions(sizes);
    if let Some(max) = budget.sessions.filter(|max| cost > 0 && doc.sessions + cost > *max) {
        let what = if cost == 1 { format!("{} would be session {} of {max}", stage.name, doc.sessions + 1) } else { format!("{}'s {cost} sessions would make {} of {max}", stage.name, doc.sessions + cost) };
        return Some(format!("over budget: {what}"));
    }
    if let Some(time) = budget.time.filter(|time| elapsed >= *time) {
        return Some(format!("over budget: {} of {} spent before {}", clock(elapsed), clock(time), stage.name));
    }
    None
}

pub fn unbuilt(pipeline: &Pipeline, host: &str) -> Vec<String> {
    let mut out = vec![];
    for stage in &pipeline.stages {
        let line = stage.line;
        let mut say = |what: &str| out.push(format!("{line}: {} uses {what}, which runs can't do yet", stage.name));
        if let Kind::Agent(agent) = &stage.kind {
            if agent.each.is_some() && agent.attempts.is_some() {
                say("each with xN");
            }
            if agent.guard.is_some() && (agent.each.is_some() || agent.attempts.is_some()) {
                say("guard with each or xN");
            }
            if agent.resume.is_some() {
                say("resume");
            }
        }
        if let Some(on) = stage.on.as_deref().filter(|on| *on != "here" && *on != host) {
            out.push(format!("{line}: {} is on {on}: no such machine: stages run on this machine (here, or {host}) until machines are built", stage.name));
        }
    }
    out
}

pub fn draft_path(worker: &str) -> PathBuf {
    Path::new(worker).join(SCRATCH).join(DRAFT)
}

pub fn write_draft(worker: &str, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let draft = draft_path(worker);
    let folder = Path::new(worker).join(SCRATCH);
    let mut part = PathBuf::from(worker);
    for name in Path::new(SCRATCH).components() {
        part.push(name);
        match fs::symlink_metadata(&part) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(std::io::Error::other(format!("{} isn't a folder of the run's worktree", part.display()))),
            Err(_) => fs::create_dir(&part)?,
        }
    }
    if !fs::canonicalize(&folder)?.starts_with(fs::canonicalize(worker)?) {
        return Err(std::io::Error::other(format!("{} is outside the run's worktree", folder.display())));
    }
    let fresh = folder.join(format!(".{DRAFT}.new"));
    let _ = fs::remove_file(&fresh);
    fs::OpenOptions::new().write(true).create_new(true).open(&fresh)?.write_all(text.as_bytes())?;
    fs::rename(&fresh, &draft)
}

fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file())
}

pub fn wrapped(text: &str, width: usize) -> String {
    let mut lines: Vec<String> = vec![String::new()];
    for word in text.split_whitespace() {
        let last = lines.last_mut().expect("a line");
        if !last.is_empty() && last.chars().count() + 1 + word.chars().count() > width {
            lines.push(word.to_string());
        } else {
            if !last.is_empty() {
                last.push(' ');
            }
            last.push_str(word);
        }
    }
    lines.join("\n")
}

pub fn budget_value(pipeline: &Pipeline) -> Value {
    pipeline.budget.as_ref().map_or(Value::Null, |b| json!({ "sessions": b.sessions, "seconds": b.time.map(|t| t.as_secs()) }))
}

pub fn changes(before: &Pipeline, after: &Pipeline) -> String {
    let plain = |stage: &Stage| Stage { line: 0, ..stage.clone() };
    let added: Vec<&str> = after.stages.iter().filter(|s| before.stage(&s.name).is_none()).map(|s| s.name.as_str()).collect();
    let removed: Vec<&str> = before.stages.iter().filter(|s| after.stage(&s.name).is_none()).map(|s| s.name.as_str()).collect();
    let changed: Vec<&str> = after.stages.iter().filter(|s| before.stage(&s.name).is_some_and(|old| plain(old) != plain(s))).map(|s| s.name.as_str()).collect();
    let extra = |from: &Pipeline, than: &Pipeline| {
        let mut left: Vec<String> = than.arrows.iter().map(Arrow::render).collect();
        from.arrows.iter().map(Arrow::render).filter(|arrow| left.iter().position(|l| l == arrow).map(|i| left.swap_remove(i)).is_none()).count()
    };
    let count = |n: usize, what: &str| format!("{n} arrow{} {what}", if n == 1 { "" } else { "s" });
    let mut parts = vec![];
    for (what, names) in [("added", added), ("removed", removed), ("changed", changed)] {
        if !names.is_empty() {
            parts.push(format!("{what} {}", names.join(", ")));
        }
    }
    for (what, n) in [("added", extra(after, before)), ("removed", extra(before, after))] {
        if n > 0 {
            parts.push(count(n, what));
        }
    }
    if budget_value(before) != budget_value(after) {
        parts.push("changed the budget".into());
    }
    if parts.is_empty() {
        return "comments and spacing only".into();
    }
    parts.join(" · ")
}

pub fn edit_errors(current: &Pipeline, text: &str, context: &Context, at: Option<&str>) -> Result<kanna_pipe::Revision, Vec<(usize, String)>> {
    let revision = kanna_pipe::edited(current, text, context).map_err(|errors| errors.into_iter().map(|e| (e.line, e.message)).collect::<Vec<_>>())?;
    let mut errors: Vec<(usize, String)> = unbuilt(&revision.pipeline, context.machine.as_deref().unwrap_or("here")).iter().filter_map(|u| u.split_once(": ").and_then(|(line, message)| Some((line.parse().ok()?, message.to_string())))).collect();
    if let Some(at) = at.filter(|at| revision.pipeline.stage(at).is_none()) {
        errors.push((1, format!("the run is at {at}, so an edit can't remove it")));
    }
    if errors.is_empty() {
        Ok(revision)
    } else {
        Err(errors)
    }
}

pub fn emit_errors(current: &Pipeline, stage: &str, block: &str, context: &Context) -> Result<kanna_pipe::Revision, Vec<String>> {
    let revision = kanna_pipe::emitted(current, stage, block, context).map_err(|refused| refused.lines())?;
    let unbuilt = unbuilt(&revision.pipeline, context.machine.as_deref().unwrap_or("here"));
    if unbuilt.is_empty() {
        Ok(revision)
    } else {
        Err(unbuilt.iter().map(|u| format!("the merged pipeline:{u}")).collect())
    }
}

pub fn refusal(stage: &str, errors: &[String], feedback: Option<Feedback>) -> Feedback {
    let said = format!("Kanna refused the kojo block in {stage}'s answer, so the run's pipeline is as it was:\n{}", errors.join("\n"));
    match feedback {
        Some(feedback) => Feedback { text: format!("{said}\n\n{}", feedback.text), ..feedback },
        None => Feedback { stage: stage.to_string(), text: said, full: String::new() },
    }
}

pub fn prompt(stage: &Stage, file: &str, request: &str, part: Option<&str>, feedback: Option<&Feedback>, answers: &BTreeMap<String, String>, answer: &str) -> String {
    let named = regex::Regex::new(r"\{\{([a-z][a-z0-9_]*)\.([A-Za-z0-9_]+)\}\}").expect("pattern");
    let mut out = named
        .replace_all(file, |c: &regex::Captures| {
            let key = if &c[2] == "answer" { c[1].to_string() } else { format!("{}.{}", &c[1], &c[2]) };
            answers.get(&key).cloned().unwrap_or_default()
        })
        .trim_end()
        .to_string();
    out += &format!("\n\n# The task\n\n{}\n", request.trim());
    if let Some(part) = part {
        out += &format!("\n# Your part\n\n{}\n", part.trim());
    }
    if let Some(feedback) = feedback {
        out += &format!("\n# Feedback from {}\n\n{}\n", feedback.stage, feedback.text.trim_end());
        if !feedback.full.is_empty() {
            out += &format!("\nAll of its output is in `{}`.\n", feedback.full);
        }
    }
    let done = if stage.verdicts() { "`\"$KANNA_CLI\" run done pass` if it passes, or `\"$KANNA_CLI\" run done fail` if it doesn't" } else { "`\"$KANNA_CLI\" run done`" };
    out += &format!("\n# When you're done\n\nWrite your final answer to `{answer}`, then run {done}. Kanna ends this session once your turn ends. If you need something from the user before you can finish, run `\"$KANNA_CLI\" run need \"WHAT YOU NEED\"` and end your turn: Kanna stops this session and resumes it with their answer.\n");
    out
}

fn fences(answer: &str) -> Vec<(Option<String>, String)> {
    let mut found = vec![];
    let mut lines = answer.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(info) = trimmed.strip_prefix("```") else { continue };
        let mut words = info.split_whitespace();
        if words.next() != Some("json") {
            continue;
        }
        let key = words.find_map(|w| w.strip_prefix("name=")).map(String::from);
        let body: Vec<&str> = lines.by_ref().take_while(|l| !l.trim_start().starts_with("```")).collect();
        found.push((key, body.join("\n")));
    }
    found
}

pub fn outputs(answer: &str) -> Vec<(String, String)> {
    fences(answer).into_iter().filter_map(|(key, body)| Some((key?, body))).collect()
}

pub fn unnamed_output(answer: &str) -> Option<String> {
    fences(answer).into_iter().find_map(|(key, body)| key.is_none().then_some(body))
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= ITEM_NAME {
            break;
        }
    }
    out.trim_end_matches('-').to_string()
}

pub fn parts(output: &str) -> Result<Vec<Item>, String> {
    let value: Value = serde_json::from_str(output).map_err(|e| format!("isn't JSON ({e})"))?;
    let list = value.as_array().ok_or("isn't a JSON list")?;
    let mut items: Vec<Item> = vec![];
    for (index, entry) in list.iter().enumerate() {
        let (named, text) = match entry {
            Value::String(text) => (text.lines().next().unwrap_or("").to_string(), text.clone()),
            other => (string(other, "name").unwrap_or("").to_string(), serde_json::to_string_pretty(other).unwrap_or_default()),
        };
        let base = Some(slug(&named)).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("{}", index + 1));
        let mut name = base.clone();
        let mut k = 2;
        while items.iter().any(|i| i.name == name) {
            name = format!("{base}-{k}");
            k += 1;
        }
        items.push(Item { name, text, ..Item::default() });
    }
    Ok(items)
}

pub fn score_of(output: &str, verdict: Option<Verdict>) -> f64 {
    let text = plain(output);
    let last = text.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or("");
    last.strip_prefix("score:").and_then(|s| s.trim().parse::<f64>().ok()).filter(|s| s.is_finite()).unwrap_or(if verdict == Some(Verdict::Pass) { 1.0 } else { 0.0 })
}

pub fn winner(items: &[Item]) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, item) in items.iter().enumerate() {
        let score = item.score.unwrap_or(0.0);
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((index, score));
        }
    }
    best.map(|(index, _)| index)
}

#[derive(Debug, PartialEq)]
pub struct Joined {
    pub joined: Vec<String>,
    pub conflict: Option<(String, Vec<String>)>,
}

pub fn join(worker: &Path, stage: &str, parts: &[Item]) -> Result<Joined, String> {
    if git(worker, &["rev-parse", "--quiet", "--verify", "MERGE_HEAD"]).is_ok() {
        let files = git(worker, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
        return Err(format!("a merge is still in progress in the run's worktree, so nothing more can join: commit it once its conflicts are resolved{}", if files.is_empty() { String::new() } else { format!(" (still conflicting: {})", files.lines().collect::<Vec<_>>().join(", ")) }));
    }
    let mut joined = vec![];
    for part in parts {
        if git(worker, &["merge-base", "--is-ancestor", &part.branch, "HEAD"]).is_ok() {
            joined.push(part.name.clone());
            continue;
        }
        let message = format!("Merge {}", part.label(stage));
        if let Err(error) = git(worker, &["merge", "--no-ff", "--no-edit", "-m", &message, &part.branch]) {
            let files: Vec<String> = git(worker, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default().lines().map(String::from).collect();
            if files.is_empty() {
                let _ = git(worker, &["merge", "--abort"]);
                return Err(format!("can't merge {}: {error}", part.label(stage)));
            }
            return Ok(Joined { joined, conflict: Some((part.name.clone(), files)) });
        }
        joined.push(part.name.clone());
    }
    Ok(Joined { joined, conflict: None })
}

pub fn plain(text: &str) -> String {
    let escapes = regex::Regex::new(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[@-Z\\-_])").expect("pattern");
    escapes.replace_all(text, "").replace('\r', "")
}

pub fn failure_lines(output: &str) -> Vec<String> {
    let text = plain(output);
    let lines: Vec<&str> = text.lines().collect();
    let tail = &lines[lines.len().saturating_sub(FEEDBACK_LINES)..];
    tail.iter().filter(|l| FAILURE_MARKS.iter().any(|m| l.contains(m))).map(|l| l.to_string()).collect()
}

pub fn command_feedback(output: &str) -> String {
    let text = plain(output);
    let lines: Vec<&str> = text.lines().collect();
    let tail = &lines[lines.len().saturating_sub(FEEDBACK_LINES)..];
    let failing = |l: &&&str| FAILURE_MARKS.iter().any(|m| l.contains(m));
    let ordered: Vec<&str> = tail.iter().filter(failing).chain(tail.iter().filter(|l| !failing(l))).copied().collect();
    let mut out = ordered.join("\n");
    if out.len() > FEEDBACK_CHARS {
        let mut cut = FEEDBACK_CHARS;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

pub fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for byte in text.bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

pub fn clock(time: Duration) -> String {
    let s = time.as_secs();
    match (s / 3600, s % 3600 / 60, s % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, s) => format!("{m}m {s:02}s"),
        (h, m, _) => format!("{h}h {m:02}m"),
    }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn uuid() -> Option<String> {
    let mut b = [0u8; 16];
    std::io::Read::read_exact(&mut fs::File::open("/dev/urandom").ok()?, &mut b).ok()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..]))
}

fn machine() -> String {
    let mut name = [0u8; 256];
    let ok = unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } == 0;
    let end = name.iter().position(|b| *b == 0).unwrap_or(0);
    let host = if ok { String::from_utf8_lossy(&name[..end]).to_string() } else { String::new() };
    host.split('.').next().filter(|h| !h.is_empty()).unwrap_or("here").to_string()
}

fn reported(value: &Value) -> Ended {
    let shown = match string(value, "verdict") {
        Some("pass") => "pass",
        Some("fail") => "fail",
        _ => "done",
    };
    let verdict = match shown {
        "pass" => Some(Verdict::Pass),
        "fail" => Some(Verdict::Fail),
        _ => None,
    };
    let answer = string(value, "answer").unwrap_or("").to_string();
    Ended { verdict, shown, answer: answer.clone(), feedback: answer, full: String::new(), failures: None, note: None, by: None }
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn failed(error: impl std::fmt::Display) -> KmuxError {
    KmuxError::new("failed", error.to_string())
}

fn shown(list: &Value, pane: &str) -> bool {
    let has = |list: Option<&Value>| list.and_then(Value::as_array).is_some_and(|l| l.iter().any(|p| p == pane));
    list.get("workspaces").and_then(Value::as_array).is_some_and(|all| all.iter().any(|ws| !has(ws.get("background")) && ws.get("tabs").and_then(Value::as_array).is_some_and(|tabs| tabs.iter().any(|t| has(t.get("panes"))))))
}

fn alive(core: &CoreHandle, pane: &str) -> bool {
    core.pane(pane).is_some_and(|p| matches!(p.state, PaneState::Starting | PaneState::Running))
}

#[derive(Clone, Debug, PartialEq)]
enum Work {
    Agent { agent: String },
    Ask { question: String },
    Command { log: PathBuf, exit: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Role {
    Main(Option<usize>),
    Score(Option<usize>, bool),
}

impl Role {
    fn item(self) -> Option<usize> {
        match self {
            Role::Main(item) | Role::Score(item, _) => item,
        }
    }

    fn to_value(self) -> Value {
        match self {
            Role::Main(item) => json!({ "item": item }),
            Role::Score(item, before) => json!({ "item": item, "score": if before { "before" } else { "after" } }),
        }
    }

    fn from_value(value: &Value) -> Role {
        let item = value.get("item").and_then(Value::as_u64).map(|i| i as usize);
        match string(value, "score") {
            Some(when) => Role::Score(item, when == "before"),
            None => Role::Main(item),
        }
    }
}

#[derive(Clone, Debug)]
struct Visit {
    n: u32,
    role: Role,
    row: StageRow,
    work: Work,
    within: Option<Duration>,
    opening: bool,
    ending: bool,
    reported: Option<(Ended, f64)>,
    needs: Option<f64>,
    paused: bool,
}

impl Visit {
    fn waited(&self) -> f64 {
        self.row.doc.get("waited").and_then(Value::as_f64).unwrap_or(0.0)
    }

    fn due(&self) -> Option<f64> {
        self.within.map(|within| self.row.started_at + self.waited() + within.as_secs_f64())
    }

    fn job(&self) -> Option<Job> {
        self.row.doc.get("job").and_then(Job::from_value)
    }

    fn worktree(&self) -> String {
        self.row.worktree.clone().unwrap_or_default()
    }

    fn label(&self) -> String {
        label(&self.row)
    }
}

fn label(row: &StageRow) -> String {
    match (&row.instance, row.kind.as_str()) {
        (Some(instance), "score") if instance != "before" && instance != "after" => format!("{} · {instance} · score", row.stage),
        (Some(instance), _) => format!("{} · {instance}", row.stage),
        (None, _) => row.stage.clone(),
    }
}

fn patch(doc: &mut Value, with: Value) {
    if !doc.is_object() {
        *doc = json!({});
    }
    let (Some(doc), Value::Object(with)) = (doc.as_object_mut(), with) else { return };
    for (key, value) in with {
        if value.is_null() {
            doc.remove(&key);
        } else {
            doc.insert(key, value);
        }
    }
}

fn carried(next: &Next, ended: &Ended, stage: &str, ask: bool) -> Option<Feedback> {
    let noted = ask && !ended.feedback.trim().is_empty();
    match (next, ended.verdict) {
        (Next::Stage { reset: false, .. }, Some(Verdict::Fail)) => Some(Feedback { stage: stage.to_string(), text: ended.feedback.clone(), full: ended.full.clone() }),
        (Next::Stage { reset: false, .. }, _) if noted => Some(Feedback { stage: stage.to_string(), text: ended.feedback.clone(), full: String::new() }),
        _ => None,
    }
}

fn held(live: &mut Live, next: &str) {
    if live.doc.held {
        live.row.state = "paused".into();
        live.doc.why = Some("the task is stopped".into());
    } else if live.doc.pausing {
        live.row.state = "paused".into();
        live.doc.why = Some(format!("paused by you before {next}"));
    } else {
        live.row.state = "running".into();
        live.doc.why = None;
    }
}

fn intervene(live: &mut Live, kind: &str) {
    *live.doc.interventions.entry(kind.to_string()).or_default() += 1;
}

struct Live {
    row: RunRow,
    doc: Doc,
    pipeline: Pipeline,
    visits: Vec<Visit>,
    last: Option<String>,
}

impl Live {
    fn visit(&self, n: u32) -> Option<&Visit> {
        self.visits.iter().find(|v| v.n == n)
    }

    fn visit_mut(&mut self, n: u32) -> Option<&mut Visit> {
        self.visits.iter_mut().find(|v| v.n == n)
    }

    fn holding(&self) -> bool {
        self.visits.iter().any(|v| v.paused || v.needs.is_some())
    }

    fn scorer(&self, role: Role) -> Option<Stage> {
        let step = self.doc.step.as_ref()?;
        let agent = self.pipeline.stage(&step.stage)?.agent()?.clone();
        let name = match role {
            Role::Score(None, _) => agent.guard?,
            Role::Score(Some(_), _) => agent.attempts?.by,
            Role::Main(_) => return None,
        };
        self.pipeline.stage(&name).cloned()
    }
}

fn remove_worker(repo: &Path, worktree: &str, branch: Option<&str>) {
    if !worktree.is_empty() && Path::new(worktree).is_dir() {
        if let Err(error) = git(repo, &["worktree", "remove", "--force", worktree]) {
            log!("kanna-daemon: the worker {worktree} is kept: {error}");
        }
    }
    if let Some(branch) = branch.filter(|b| !b.is_empty()) {
        let _ = git(repo, &["branch", "-D", branch]);
    }
}

pub struct Started {
    pub task: String,
    pub repo: String,
    pub worker: String,
    pub branch: String,
    pub base: String,
    pub text: String,
    pub shown: String,
    pub name: String,
    pub request: String,
    pub prompts: BTreeMap<String, String>,
    pub owner: Option<String>,
    pub by: String,
    pub agent: bool,
}

pub struct Runs {
    db: Rc<Db>,
    core: WeakCore,
    tasks: Weak<Tasks>,
    live: RefCell<Vec<Live>>,
    soon: Cell<bool>,
    host: String,
    views: RefCell<BTreeMap<String, String>>,
    pub on_run: RefCell<Option<Box<dyn Fn(&[String], &Value)>>>,
    pub heed: RefCell<Option<Box<dyn Fn(&str)>>>,
    pub on_end: RefCell<Option<OnEnd>>,
    pub on_moved: RefCell<Option<Box<dyn Fn(&str)>>>,
    pub on_tell: RefCell<Option<OnTell>>,
    flagged: RefCell<HashSet<String>>,
    asks_changed: Cell<bool>,
    carrying: RefCell<Vec<String>>,
    pub on_status: RefCell<Option<Box<dyn Fn(&str)>>>,
}

impl Runs {
    pub fn open(db: Rc<Db>, core: WeakCore, tasks: Weak<Tasks>) -> Runs {
        let mut live = vec![];
        for row in db.live_runs().unwrap_or_else(|error| {
            log!("kanna-daemon: can't read the runs in kanna.db: {error}");
            vec![]
        }) {
            let doc = Doc::from_value(&row.doc);
            let text = db.revisions(&row.id).ok().and_then(|r| r.last().and_then(|r| string(&r.doc, "text").map(String::from))).unwrap_or_default();
            let Ok(pipeline) = kanna_pipe::parse(&text) else {
                log!("kanna-daemon: {}'s pipeline doesn't parse any more, so it's left as it is", row.id);
                continue;
            };
            let mut entry = Live { row, doc, pipeline, visits: vec![], last: None };
            let open: Vec<StageRow> = db.stages(&entry.row.id).unwrap_or_default().into_iter().filter(|s| s.ended_at.is_none()).collect();
            for row in open {
                let work = match row.kind.as_str() {
                    "agent" => Work::Agent { agent: row.agent.clone().unwrap_or_default() },
                    "ask" => Work::Ask { question: string(&row.doc, "question").unwrap_or("").to_string() },
                    _ => Work::Command { log: PathBuf::from(string(&row.doc, "log").unwrap_or("")), exit: PathBuf::from(string(&row.doc, "exit").unwrap_or("")) },
                };
                let role = row.doc.get("role").map_or(Role::Main(None), Role::from_value);
                let within = entry.scorer(role).or_else(|| entry.pipeline.stage(&row.stage).cloned()).and_then(|s| s.within);
                let reported = row.doc.get("reported").filter(|r| r.is_object()).map(|r| (reported(r), now()));
                let paused = row.doc.get("paused").is_some_and(Value::is_object);
                let n = row.doc.get("n").and_then(Value::as_u64).unwrap_or(0) as u32;
                let needs = (!paused && reported.is_none() && matches!(work, Work::Agent { .. }) && row.doc.get("needs").is_some_and(|n| !n.is_null())).then(now);
                entry.visits.push(Visit { n, role, row, work, within, opening: false, ending: false, reported, needs, paused });
            }
            if entry.row.state == "paused" && !entry.holding() {
                for visit in entry.visits.iter_mut().filter(|v| v.reported.is_none() && matches!(v.work, Work::Agent { .. })) {
                    visit.needs = Some(now());
                }
            }
            if let (None, Some(visit)) = (&entry.doc.step, entry.visits.first()) {
                entry.doc.step = Some(Step { stage: visit.row.stage.clone(), visit: visit.row.visit, start: string(&visit.row.doc, "start").unwrap_or("").to_string(), ..Step::default() });
            }
            let at = if entry.visits.is_empty() { "between stages".to_string() } else { entry.visits.iter().map(|v| format!("in {}", v.label())).collect::<Vec<_>>().join(", ") };
            log!("kanna-daemon: {} carries on ({}, {at})", entry.row.id, entry.row.state);
            live.push(entry);
        }
        Runs { db, core, tasks, live: RefCell::new(live), soon: Cell::new(true), host: machine(), flagged: RefCell::default(), asks_changed: Cell::new(true), carrying: RefCell::default(), on_status: RefCell::default(), views: RefCell::default(), on_run: RefCell::new(None), heed: RefCell::new(None), on_end: RefCell::default(), on_moved: RefCell::default(), on_tell: RefCell::default() }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let watching = Rc::downgrade(self);
        core.on_ended(Rc::new(move |pane| {
            let Some(this) = watching.upgrade() else { return };
            let ours = this.live.try_borrow().map_or(true, |live| live.iter().any(|l| l.visits.iter().any(|v| v.row.terminal.as_deref() == Some(pane) || v.job().is_some_and(|j| j.terminal.as_deref() == Some(pane)))));
            if ours {
                this.soon.set(true);
            }
        }));
        let preparing = Rc::downgrade(self);
        core.on_open(
            PaneType::Run,
            Rc::new(move |args| {
                let this = preparing.upgrade().ok_or_else(|| KmuxError::new("app_gone", "the daemon is stopping"))?;
                this.prepare_open(args)
            }),
        );
        let this = Rc::downgrade(self);
        core.add_command(
            "run",
            Rc::new(move |args, responder: Responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                match string(&args, "action") {
                    Some("watch") => return this.watch(&args, responder),
                    Some("edit") => return this.edit(&args, responder),
                    _ => {}
                }
                let reply = this.handle(&args);
                responder.reply(reply);
            }),
        );
    }

    fn handle(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        match string(args, "action") {
            Some("start") => self.start_run(args),
            Some("list") => self.list(args),
            Some("show") => self.show(args),
            Some("log") => self.log(args),
            Some("stop") => self.stop_run(args),
            Some("done") => self.done(args),
            Some("need") => self.need(args),
            Some("command") => self.command(args),
            Some("resume") => self.resume_run(args),
            Some("answer") => self.answer(args),
            Some("pause") => self.pause_run(args),
            Some("skip") => self.skip(args),
            Some("retry") => self.retry(args),
            other => Err(bad(format!("unknown run action {other:?} (start, list, show, log, stop, resume, watch, answer, pause, skip, retry, edit, done, need or command)"))),
        }
    }

    fn tasks(&self) -> Result<Rc<Tasks>, KmuxError> {
        self.tasks.upgrade().ok_or_else(|| KmuxError::new("app_gone", "the daemon is stopping"))
    }

    fn task_here(&self, args: &Value) -> Option<Entry> {
        let tasks = self.tasks.upgrade()?;
        let by = string(args, "by").and_then(|by| tasks.task_of(by)).and_then(|id| tasks.entry(&id));
        by.or_else(|| string(args, "cwd").and_then(|cwd| tasks.task_in_folder(Path::new(cwd))))
    }

    fn start_run(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let task = self.task_here(args).ok_or_else(|| KmuxError::new("not_a_task", "kanna run start runs in a task's terminal: the run builds that task's change"))?;
        let folder = PathBuf::from(task.folder.clone().ok_or_else(|| KmuxError::new("not_started", format!("{} has no worktree yet", task.id)))?);
        let named = string(args, "pipeline").filter(|p| !p.is_empty()).ok_or_else(|| bad("say which pipeline: kanna run start NAME|FILE"))?;
        let file = if named.ends_with(".kojo") || named.contains('/') { PathBuf::from(named) } else { folder.join(format!(".kanna/pipelines/{named}.kojo")) };
        let text = fs::read_to_string(&file).map_err(|e| KmuxError::new("not_found", format!("no pipeline {named}: can't read {} ({e})", file.display())))?;
        let name = file.file_stem().map_or_else(|| named.to_string(), |s| s.to_string_lossy().into_owned());
        let prompts: BTreeMap<String, String> = fs::read_dir(file.parent().and_then(Path::parent).map(|k| k.join("prompts")).unwrap_or_default())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), fs::read_to_string(&p).ok()?)))
            .collect();
        let shown = file.display().to_string();
        let pipeline = self.checked(&text, &shown, &prompts)?;
        let request = match string(args, "request").filter(|r| !r.trim().is_empty()) {
            Some(request) => request.to_string(),
            None => task.message.clone().unwrap_or_default(),
        };
        let repo = PathBuf::from(&task.repo);
        let base = git(&folder, &["rev-parse", "--verify", "HEAD"]).map_err(|e| failed(format!("the task's branch has no commit to start from: {e}")))?;
        let mut n = self.db.runs_of(&task.id).map_err(failed)?.len() + 1;
        let (worker, branch) = loop {
            match tasks::add_worktree(&repo, &format!("{}-run-{n}", task.id), &format!("run/{}-{n}", task.id), Some(&base)) {
                Ok(made) => break made,
                Err(error) if error.contains("is there already") && n < 1000 => n += 1,
                Err(error) => return Err(failed(format!("can't make the run's worktree: {error}"))),
            }
        };
        let by = string(args, "by").unwrap_or("user").to_string();
        let agent = string(args, "by").and_then(|by| self.tasks.upgrade()?.task_of(by)).is_some_and(|of| of == task.id);
        let started = Started { task: task.id.clone(), repo: task.repo.clone(), worker: worker.clone(), branch: branch.clone(), base, text, shown, name: name.clone(), request, prompts, owner: None, by, agent };
        let (id, worst) = self.open_run(started, pipeline)?;
        Ok(Map::from_iter([
            ("run".to_string(), json!(id)),
            ("task".to_string(), json!(task.id)),
            ("pipeline".to_string(), json!(name)),
            ("branch".to_string(), json!(branch)),
            ("worktree".to_string(), json!(worker)),
            ("worst".to_string(), json!(worst)),
        ]))
    }

    fn checked(&self, text: &str, shown: &str, prompts: &BTreeMap<String, String>) -> Result<Pipeline, KmuxError> {
        let context = Context { prompts: prompts.clone(), machine: Some(self.host.clone()), ..Context::default() };
        let pipeline = kanna_pipe::parse(text).map_err(|errors| KmuxError::new("invalid", errors.iter().map(|e| e.at(shown)).collect::<Vec<_>>().join("\n")))?;
        let unbuilt = unbuilt(&pipeline, &self.host);
        let placed: Vec<&String> = unbuilt.iter().filter(|u| u.contains("no such machine")).collect();
        if !placed.is_empty() {
            return Err(KmuxError::new("no_such_machine", placed.iter().map(|u| format!("{shown}:{u}")).collect::<Vec<_>>().join("\n")));
        }
        pipeline.check(&context).map_err(|errors| KmuxError::new("invalid", errors.iter().map(|e| e.at(shown)).collect::<Vec<_>>().join("\n")))?;
        if !unbuilt.is_empty() {
            return Err(KmuxError::new("unsupported", unbuilt.iter().map(|u| format!("{shown}:{u}")).collect::<Vec<_>>().join("\n")));
        }
        Ok(pipeline)
    }

    pub fn start_owned(&self, started: Started) -> Result<String, KmuxError> {
        let pipeline = self.checked(&started.text, &started.shown, &started.prompts)?;
        self.open_run(started, pipeline).map(|(id, _)| id)
    }

    fn open_run(&self, started: Started, pipeline: Pipeline) -> Result<(String, u32), KmuxError> {
        let Started { task, repo, worker, branch, base, text, shown, name, request, prompts, owner, by, agent } = started;
        let id = self.db.new_run_id().map_err(failed)?;
        let doc = Doc { request, file: shown.clone(), worker: worker.clone(), base: base.clone(), revision: 1, taken: vec![0; pipeline.arrows.len()], next: Some(pipeline.start.clone()), owner, ..Doc::default() };
        let mut value = doc.to_value();
        value["budget"] = budget_value(&pipeline);
        let row = RunRow { id: id.clone(), task: task.clone(), repo, pipeline: name.clone(), revision_hash: hash(&text), branch: branch.clone(), state: "running".into(), started_at: now(), ended_at: None, outcome: None, doc: value };
        self.db.put_run(&row, Some(("started", Some(&by), &json!({ "pipeline": name, "branch": branch, "base": base, "agent": agent })))).map_err(failed)?;
        self.db.add_revision(&id, "file", &row.revision_hash, &json!({ "text": text, "file": shown, "prompts": prompts, "worst": pipeline.max_sessions(&Sizes::new()) })).map_err(failed)?;
        log!("kanna-daemon: {} runs {name} for {task} on {branch} in {worker}", id);
        let worst = pipeline.max_sessions(&Sizes::new());
        self.live.borrow_mut().push(Live { row, doc, pipeline, visits: vec![], last: None });
        self.soon.set(true);
        Ok((id, worst))
    }

    fn find(&self, args: &Value) -> Result<RunRow, KmuxError> {
        self.find_run(args, false)
    }

    fn find_live(&self, args: &Value) -> Result<RunRow, KmuxError> {
        self.find_run(args, true)
    }

    fn find_run(&self, args: &Value, one_live: bool) -> Result<RunRow, KmuxError> {
        if let Some(reference) = string(args, "run").filter(|r| !r.is_empty()) {
            let all = self.db.runs(None).map_err(failed)?;
            let bare = reference.strip_prefix("run-").unwrap_or(reference);
            let found: Vec<RunRow> = all.into_iter().filter(|r| r.id == reference || r.id["run-".len()..].starts_with(bare)).collect();
            return match found.len() {
                1 => Ok(found.into_iter().next().unwrap()),
                0 => Err(KmuxError::new("not_found", format!("no run {reference}"))),
                _ => Err(bad(format!("{reference} could be {} runs: give more of its ID", found.len()))),
            };
        }
        let task = self.task_here(args).ok_or_else(|| bad("which run? give its ID, or run this in a task's terminal for the task's latest"))?;
        let mut runs = self.db.runs_of(&task.id).map_err(failed)?;
        let live: Vec<String> = runs.iter().filter(|r| LIVE.contains(&r.state.as_str())).map(|r| r.id.clone()).collect();
        if one_live && live.len() > 1 {
            return Err(KmuxError::new("several", format!("{} has {} live runs: say which ({})", task.id, live.len(), live.join(", "))));
        }
        if let Some(only) = live.first().filter(|_| one_live) {
            return runs.into_iter().find(|r| &r.id == only).ok_or_else(|| KmuxError::new("not_found", format!("no run {only}")));
        }
        runs.pop().ok_or_else(|| KmuxError::new("not_found", format!("{} has no runs", task.id)))
    }

    fn row_value(&self, row: &RunRow) -> Value {
        let stages = self.db.stages(&row.id).unwrap_or_default();
        let last = stages.last();
        let stage_count = self.live.borrow().iter().find(|l| l.row.id == row.id).map(|l| l.pipeline.stages.len()).or_else(|| {
            let text = self.db.revisions(&row.id).ok()?.last().and_then(|r| string(&r.doc, "text").map(String::from))?;
            kanna_pipe::parse(&text).ok().map(|p| p.stages.len())
        });
        let doc = self.live.borrow().iter().find(|l| l.row.id == row.id).map_or_else(|| Doc::from_value(&row.doc), |l| l.doc.clone());
        json!({
            "run": row.id,
            "task": row.task,
            "repo": row.repo,
            "pipeline": row.pipeline,
            "branch": row.branch,
            "state": row.state,
            "startedAt": num(row.started_at),
            "endedAt": row.ended_at.map_or(Value::Null, num),
            "outcome": opt(row.outcome.as_deref()),
            "stage": doc.step.as_ref().map(|s| json!(s.stage)).or_else(|| last.map(|s| json!(s.stage))).unwrap_or(Value::Null),
            "stageOpen": LIVE.contains(&row.state.as_str()) && doc.step.is_some() || last.is_some_and(|s| s.ended_at.is_none()),
            "next": opt(doc.next.as_deref()),
            "visits": stages.len(),
            "stages": stage_count,
            "sessions": doc.sessions,
            "budget": row.doc.get("budget").cloned().unwrap_or(Value::Null),
            "why": opt(doc.why.as_deref()),
            "note": opt(doc.note.as_deref()),
            "worktree": doc.worker,
            "request": doc.request,
            "pausing": doc.pausing,
            "held": doc.held,
            "interventions": doc.interventions,
        })
    }

    fn list(&self, args: &Value) -> Result<Map, KmuxError> {
        let repo = if args.get("all") == Some(&Value::Bool(true)) { None } else { string(args, "repo").map(String::from) };
        let runs = self.db.runs(repo.as_deref()).map_err(failed)?;
        Ok(Map::from_iter([("runs".to_string(), Value::Array(runs.iter().map(|r| self.row_value(r)).collect()))]))
    }

    fn show(&self, args: &Value) -> Result<Map, KmuxError> {
        let row = self.find(args)?;
        let revision = self.db.revisions(&row.id).map_err(failed)?.pop();
        let visits: Vec<Value> = self
            .db
            .stages(&row.id)
            .map_err(failed)?
            .iter()
            .map(|s| {
                json!({
                    "id": s.id, "stage": s.stage, "visit": s.visit, "instance": opt(s.instance.as_deref()), "kind": s.kind, "agent": opt(s.agent.as_deref()), "machine": s.machine,
                    "startedAt": num(s.started_at), "endedAt": s.ended_at.map_or(Value::Null, num), "verdict": opt(s.verdict.as_deref()),
                    "session": opt(s.session.as_deref()), "terminal": opt(s.terminal.as_deref()), "worktree": opt(s.worktree.as_deref()), "branch": opt(s.branch.as_deref()),
                    "doc": s.doc,
                })
            })
            .collect();
        let workers: Vec<Value> = self
            .live
            .borrow()
            .iter()
            .filter(|l| l.row.id == row.id)
            .flat_map(|l| l.visits.iter())
            .map(|v| json!({ "stage": v.row.stage, "instance": opt(v.row.instance.as_deref()), "kind": v.row.kind, "terminal": opt(v.row.terminal.as_deref()), "worktree": opt(v.row.worktree.as_deref()) }))
            .collect();
        Ok(Map::from_iter([
            ("run".to_string(), self.row_value(&row)),
            ("revision".to_string(), revision.map_or(Value::Null, |r| json!({ "n": r.n, "by": r.by, "hash": r.hash, "at": num(r.at), "text": r.doc.get("text").cloned().unwrap_or(Value::Null) }))),
            ("visits".to_string(), Value::Array(visits)),
            ("workers".to_string(), Value::Array(workers)),
        ]))
    }

    fn log(&self, args: &Value) -> Result<Map, KmuxError> {
        let row = self.find(args)?;
        let events = self.db.run_events(&row.id).map_err(failed)?;
        Ok(Map::from_iter([
            ("run".to_string(), json!(row.id)),
            ("events".to_string(), Value::Array(events.iter().map(|e| json!({ "at": num(e.at), "what": e.what, "by": opt(e.by.as_deref()), "doc": e.doc })).collect())),
        ]))
    }

    fn stop_run(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let row = self.find_live(args)?;
        if !LIVE.contains(&row.state.as_str()) {
            return Err(KmuxError::new("ended", format!("{} has ended already ({})", row.id, row.outcome.as_deref().unwrap_or(&row.state))));
        }
        let by = string(args, "by").unwrap_or("user").to_string();
        self.stop(&row.id, "stopped", "stopped", Some(&by), false);
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("state".to_string(), json!("stopped"))]))
    }

    fn reporting(&self, args: &Value) -> Result<(String, Visit, bool, String), KmuxError> {
        let not_a_stage = || KmuxError::new("not_a_stage", "kanna run done and need are for a stage's agent, in its worker's terminal");
        let by = string(args, "by").ok_or_else(not_a_stage)?;
        let live = self.live.borrow();
        let found = live.iter().find_map(|l| Some((l, l.visits.iter().find(|v| v.row.terminal.as_deref() == Some(by) && matches!(v.work, Work::Agent { .. }))?)));
        let (live, visit) = found.ok_or_else(not_a_stage)?;
        if visit.ending || visit.reported.is_some() || visit.paused {
            return Err(KmuxError::new("ended", format!("{} has reported already", visit.label())));
        }
        let verdicts = live.pipeline.stage(&visit.row.stage).is_some_and(Stage::verdicts);
        Ok((live.row.id.clone(), visit.clone(), verdicts, live.row.state.clone()))
    }

    fn done(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (id, visit, verdicts, state) = self.reporting(args)?;
        let stage = visit.label();
        let verdict = match (string(args, "verdict"), verdicts) {
            (Some("pass"), true) => Some(Verdict::Pass),
            (Some("fail"), true) => Some(Verdict::Fail),
            (None, false) => None,
            (_, true) => return Err(bad(format!("{stage} gives a verdict: kanna run done pass, or kanna run done fail"))),
            (Some(_), false) => return Err(bad(format!("{stage} gives no verdict: kanna run done"))),
        };
        let answer = string(args, "answer").unwrap_or("").to_string();
        let ended = Ended { verdict, shown: verdict.map_or("done", Verdict::name), answer: answer.clone(), feedback: answer.clone(), full: String::new(), failures: None, note: None, by: None };
        let mut row = visit.row.clone();
        row.doc = json!({ "reported": { "verdict": ended.shown, "answer": answer } });
        let _ = self.db.put_stage(&row);
        log!("kanna-daemon: {id}'s {stage} reports {}", ended.shown);
        let n = visit.n;
        self.update(&id, |live| {
            if let Some(visit) = live.visit_mut(n) {
                visit.reported = Some((ended.clone(), now()));
                visit.needs = None;
            }
            if (state == "waiting" || state == "paused") && !live.holding() && !live.doc.held && !live.doc.pausing {
                live.row.state = "running".into();
                live.doc.why = None;
                return Some(("resumed", None, json!({ "stage": stage })));
            }
            None
        });
        self.soon.set(true);
        Ok(Map::from_iter([("run".to_string(), json!(id)), ("stage".to_string(), json!(visit.row.stage)), ("verdict".to_string(), json!(ended.shown))]))
    }

    fn need(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (id, visit, _, _) = self.reporting(args)?;
        let text = string(args, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("say what you need: kanna run need TEXT"))?.to_string();
        let (stage, pane, n) = (visit.label(), visit.row.terminal.clone().unwrap_or_default(), visit.n);
        log!("kanna-daemon: {id}'s {stage} needs: {text}");
        self.flag_needs(&visit.row);
        self.update(&id, |live| {
            if let Some(visit) = live.visit_mut(n) {
                visit.needs = Some(now());
            }
            None
        });
        self.pause(&id, &stage, &format!("{stage} needs: {text}"), Some(&pane), &format!("{stage} needs you: {text}\nIts agent stops once its turn ends, and its session is kept: `kanna run resume {id} \"…\"` carries it on with your answer."));
        self.soon.set(true);
        Ok(Map::from_iter([("run".to_string(), json!(id)), ("stage".to_string(), json!(visit.row.stage)), ("state".to_string(), json!("paused"))]))
    }

    pub fn tick(self: &Rc<Self>) -> Option<Instant> {
        if self.asks_changed.replace(false) {
            self.sync_asks();
        }
        let carrying = std::mem::take(&mut *self.carrying.borrow_mut());
        for id in carrying {
            if let Err(error) = self.carry_on(&id, CARRY_ON, "task") {
                log!("kanna-daemon: {id} couldn't carry on once its task started: {}", error.message);
            }
        }
        if self.soon.replace(false) || self.deadline().is_some_and(|at| at <= now()) {
            let ids: Vec<String> = self.live.borrow().iter().filter(|l| l.row.state == "running" || l.visits.iter().any(|v| !v.paused)).map(|l| l.row.id.clone()).collect();
            for id in &ids {
                self.step(id);
            }
            self.live.borrow_mut().retain(|l| LIVE.contains(&l.row.state.as_str()));
        }
        if self.soon.get() {
            return Some(Instant::now());
        }
        self.deadline().map(|at| Instant::now() + Duration::from_secs_f64((at - now()).max(0.0)))
    }

    fn deadline(&self) -> Option<f64> {
        let live = self.live.borrow();
        let visits = live.iter().flat_map(|l| l.visits.iter()).filter(|v| !v.opening && !v.ending && !v.paused);
        visits.filter_map(|v| v.reported.as_ref().map(|(_, at)| at + SETTLE).or(v.needs.map(|at| at + SETTLE)).or_else(|| v.due())).min_by(f64::total_cmp)
    }

    fn update(&self, id: &str, edit: impl FnOnce(&mut Live) -> Option<(&'static str, Option<String>, Value)>) {
        let saved = {
            let mut live = self.live.borrow_mut();
            let Some(live) = live.iter_mut().find(|l| l.row.id == id) else { return };
            let event = edit(live);
            let mut doc = live.doc.to_value();
            if let Some(budget) = live.row.doc.get("budget") {
                doc["budget"] = budget.clone();
            }
            live.row.doc = doc;
            (live.row.clone(), event, live.doc.owner.is_some())
        };
        let (row, event, owned) = saved;
        let event = event.as_ref().map(|(what, by, doc)| (*what, by.as_deref(), doc));
        if let Err(error) = self.db.put_run(&row, event) {
            log!("kanna-daemon: couldn't save {id} in kanna.db: {error}");
        }
        self.publish(id);
        self.asks_changed.set(true);
        if owned {
            if let Some(on_moved) = self.on_moved.borrow().as_ref() {
                on_moved(id);
            }
        }
    }

    pub fn place_of(&self, run: &str) -> Option<Place> {
        let live = self.live.borrow();
        let live = live.iter().find(|l| l.row.id == run)?;
        let stage = live.visits.first().map(|v| v.row.stage.clone()).or_else(|| live.doc.step.as_ref().map(|s| s.stage.clone())).or_else(|| live.doc.next.clone());
        Some(Place { stage, state: live.row.state.clone(), why: live.doc.why.clone() })
    }

    fn job_code(&self, job: &Job) -> Option<i32> {
        if let Some(code) = job.code {
            return Some(code);
        }
        let exit = Path::new(&job.exit);
        if !exit.is_file() {
            let core = self.core.upgrade()?;
            let pane = job.terminal.as_deref()?;
            if alive(&core, pane) {
                return None;
            }
            let code = core.pane(pane).and_then(|p| p.exit_code).unwrap_or(1);
            let _ = fs::write(exit, format!("{code}\n"));
        }
        Some(fs::read_to_string(exit).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(1))
    }

    fn set_job(&self, id: &str, n: u32, job: &Job) {
        let mut saved = None;
        self.update(id, |live| {
            if let Some(visit) = live.visit_mut(n) {
                patch(&mut visit.row.doc, json!({ "job": job.to_value() }));
                saved = Some(visit.row.clone()).filter(|row| row.id != 0);
            }
            None
        });
        if let Some(row) = saved {
            let _ = self.db.put_stage(&row);
        }
    }

    fn settle_job(&self, id: &str, n: u32, job: &Job) -> Option<i32> {
        let code = self.job_code(job)?;
        if job.code.is_none() {
            if let Some(pane) = &job.terminal {
                self.close(pane);
            }
            log!("kanna-daemon: {id}'s command {} exited {code}", job.k);
            self.set_job(id, n, &Job { code: Some(code), ..job.clone() });
        }
        Some(code)
    }

    fn drop_job(&self, visit: &Visit) {
        let Some(job) = visit.job().filter(|j| j.code.is_none()) else { return };
        if let Some(pane) = &job.terminal {
            self.close(pane);
        }
    }

    fn command(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (id, visit, _, _) = self.reporting(args)?;
        let (stage, n) = (visit.label(), visit.n);
        let last = visit.job();
        let reply = |job: &Job, code: Option<i32>| {
            Ok(Map::from_iter([
                ("run".to_string(), json!(id)),
                ("stage".to_string(), json!(visit.row.stage)),
                ("job".to_string(), json!(job.k)),
                ("command".to_string(), json!(job.command)),
                ("log".to_string(), json!(job.log)),
                ("state".to_string(), json!(if code.is_some() { "ended" } else { "running" })),
                ("code".to_string(), code.map_or(Value::Null, |c| json!(c))),
                ("seconds".to_string(), num((now() - job.started).max(0.0))),
            ]))
        };
        let Some(command) = string(args, "command").map(str::trim).filter(|c| !c.is_empty()) else {
            let job = last.ok_or_else(|| KmuxError::new("no_command", format!("{stage} has asked for no command: kanna run command COMMAND")))?;
            let code = self.settle_job(&id, n, &job);
            return reply(&job, code);
        };
        if let Some(job) = last.as_ref().filter(|j| self.settle_job(&id, n, j).is_none()) {
            return Err(KmuxError::new("busy", format!("{stage}'s command is still running ({}): `kanna run command --wait` waits for it", job.command)));
        }
        let (run, doc) = {
            let live = self.live.borrow();
            let live = live.iter().find(|l| l.row.id == id).ok_or_else(|| KmuxError::new("ended", format!("{id} has ended")))?;
            (live.row.clone(), live.doc.clone())
        };
        let k = last.map_or(1, |j| j.k + 1);
        let worktree = visit.worktree();
        let scratch = PathBuf::from(&worktree).join(SCRATCH);
        let (log, exit) = (scratch.join(format!("command-{n}-{k}.log")), scratch.join(format!("command-{n}-{k}.exit")));
        let _ = fs::remove_file(&exit);
        let line = task_scripts::stage_command(&self.vars(&run, &doc, &visit.row, &visit.row.stage), command, 0, &log, &exit);
        let job = Job { k, command: command.to_string(), log: log.to_string_lossy().into_owned(), exit: exit.to_string_lossy().into_owned(), terminal: None, started: now(), awaited: false, code: None };
        self.set_job(&id, n, &job);
        log!("kanna-daemon: {id}'s {stage} asks for a command: {command}");
        let opening = job.clone();
        self.open_worker(&id, n, &worktree, &line, move |this, id, pane| {
            let current = this.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit(n)).and_then(Visit::job);
            match current.filter(|j| j.k == opening.k) {
                Some(job) => this.set_job(id, n, &Job { terminal: Some(pane.to_string()), ..job }),
                None => this.close(pane),
            }
        });
        reply(&job, None)
    }

    fn wait_for_job(self: &Rc<Self>, id: &str, n: u32, job: &Job) {
        let stage = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit(n)).map(Visit::label).unwrap_or_default();
        log!("kanna-daemon: {id}'s {stage} ended its turn while its command runs, so it's parked until the command ends");
        self.set_job(id, n, &Job { awaited: true, ..job.clone() });
        self.update(id, |live| {
            live.doc.why = Some(format!("{stage} waits for its command"));
            None
        });
        self.park(id, n);
    }

    fn remind(self: &Rc<Self>, id: &str, n: u32, stage: &str) {
        log!("kanna-daemon: {id}'s {stage} ended its turn without reporting, so it's carried on once to report");
        let mut saved = None;
        self.update(id, |live| {
            if let Some(visit) = live.visit_mut(n) {
                patch(&mut visit.row.doc, json!({ "reminded": "due" }));
                saved = Some(visit.row.clone()).filter(|row| row.id != 0);
            }
            Some(("reminded", Some("daemon".into()), json!({ "stage": stage })))
        });
        if let Some(row) = saved {
            let _ = self.db.put_stage(&row);
        }
        self.park(id, n);
    }

    fn reminding(self: &Rc<Self>, id: &str, visit: &Visit) {
        let (stage, n) = (visit.label(), visit.n);
        let verdicts = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.pipeline.stage(&visit.row.stage).map(Stage::verdicts)).unwrap_or(false);
        self.update(id, |live| {
            if let Some(visit) = live.visit_mut(n) {
                patch(&mut visit.row.doc, json!({ "reminded": "sent" }));
            }
            None
        });
        if let Err(error) = self.carry_on_visit(id, Some(n), &unreported(&stage, verdicts), "daemon") {
            self.pause(id, &stage, &format!("{stage} stopped without reporting"), None, &format!("{stage}'s turn ended without it reporting, and its agent couldn't be carried on to report: {}. `kanna run resume {id} \"…\"` tries again.", error.message));
        }
    }

    fn job_ended(self: &Rc<Self>, id: &str, n: u32, job: &Job, code: i32) {
        self.set_job(id, n, &Job { awaited: false, code: Some(code), ..job.clone() });
        if let Err(error) = self.carry_on_visit(id, Some(n), &command_ended(job, code), "daemon") {
            let stage = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit(n)).map(Visit::label).unwrap_or_default();
            self.pause(id, &stage, &format!("{stage} couldn't carry on after its command: {}", error.message), None, &format!("{stage}'s command ended (exit {code}, {}), but its agent couldn't be carried on: {}. `kanna run resume {id} \"…\"` tries again.", job.log, error.message));
        }
    }

    pub fn terminal_of(&self, run: &str) -> Option<String> {
        let live = self.live.borrow();
        let live = live.iter().find(|l| l.row.id == run)?;
        live.visits.iter().find(|v| !v.paused).and_then(|v| v.row.terminal.clone()).or_else(|| live.visits.iter().find_map(|v| v.job().filter(|j| j.code.is_none()).and_then(|j| j.terminal)))
    }

    fn sync_asks(&self) {
        let (Some(core), Some(tasks)) = (self.core.upgrade(), self.tasks.upgrade()) else { return };
        let asking: Vec<String> = self.live.borrow().iter().filter(|l| l.row.state == "waiting" && l.visits.iter().any(|v| matches!(v.work, Work::Ask { .. }) && !v.ending)).map(|l| l.row.task.clone()).collect();
        let want: HashSet<String> = asking.iter().filter_map(|task| tasks.agent_pane(task)).collect();
        let had = self.flagged.replace(want.clone());
        for pane in had.union(&want) {
            let waits = want.contains(pane);
            if core.pane(pane).is_some_and(|p| p.waits_on_you != waits) {
                core.with_pane_mut(pane, |p| p.waits_on_you = waits);
                if let Some(on_status) = self.on_status.borrow().as_ref() {
                    on_status(pane);
                }
            }
        }
    }

    pub fn task_stopped(self: &Rc<Self>, task: &str) {
        let ids: Vec<String> = self.live.borrow().iter().filter(|l| l.row.task == task && l.doc.owner.is_none() && (l.row.state == "running" || l.row.state == "waiting")).map(|l| l.row.id.clone()).collect();
        for id in ids {
            log!("kanna-daemon: {task} is stopped, so {id} stops its stages and starts no other until it starts");
            self.update(&id, |live| {
                live.doc.held = true;
                if live.row.state == "running" {
                    live.row.state = "paused".into();
                    live.doc.why = Some("the task is stopped".into());
                }
                Some(("paused", Some("task".into()), json!({ "why": "the task is stopped" })))
            });
        }
        self.soon.set(true);
    }

    pub fn task_started(self: &Rc<Self>, task: &str) {
        self.asks_changed.set(true);
        let found: Vec<(String, bool, Vec<Visit>)> = self.live.borrow().iter().filter(|l| l.row.task == task && l.doc.owner.is_none() && l.doc.held).map(|l| (l.row.id.clone(), l.doc.pausing, l.visits.clone())).collect();
        for (id, pausing, visits) in found {
            log!("kanna-daemon: {task} started again, so {id} carries on");
            let parked = visits.iter().filter(|v| v.paused && v.row.doc["paused"]["task"] == true).count();
            if parked > 0 && !pausing {
                self.carrying.borrow_mut().extend(std::iter::repeat_n(id.clone(), parked));
                continue;
            }
            self.update(&id, |live| {
                live.doc.held = false;
                if live.row.state == "paused" && !pausing && !live.holding() {
                    live.row.state = "running".into();
                    live.doc.why = None;
                }
                Some(("resumed", Some("task".into()), json!({ "why": "the task started" })))
            });
        }
        self.soon.set(true);
    }

    fn state(&self, id: &str) -> Option<String> {
        self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.state.clone())
    }

    fn step(self: &Rc<Self>, id: &str) {
        let Some((visits, next, state, stepping)) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| (l.visits.clone(), l.doc.next.clone(), l.row.state.clone(), l.doc.step.is_some())) else { return };
        for visit in &visits {
            if LIVE.contains(&self.state(id).unwrap_or_default().as_str()) {
                self.step_visit(id, visit);
            }
        }
        if stepping {
            return self.pump(id);
        }
        if let (true, Some(next), "running") = (visits.is_empty(), next, state.as_str()) {
            self.begin(id, &next);
        }
    }

    fn step_visit(self: &Rc<Self>, id: &str, visit: &Visit) {
        let Some(core) = self.core.upgrade() else { return };
        if visit.paused && !visit.ending && !visit.opening && self.state(id).as_deref() == Some("running") {
            if let Some(job) = visit.job().filter(|j| j.awaited) {
                if let Some(code) = self.settle_job(id, visit.n, &job) {
                    self.job_ended(id, visit.n, &job, code);
                }
            } else if string(&visit.row.doc, "reminded") == Some("due") {
                self.reminding(id, visit);
            }
            return;
        }
        if visit.opening || visit.ending || visit.paused {
            return;
        }
        let late = visit.due().is_some_and(|due| now() >= due);
        let n = visit.n;
        if let Work::Ask { .. } = &visit.work {
            if late && self.mark_ending(id, n) {
                self.finished(id, n, Ended { verdict: Some(Verdict::Fail), shown: "fail", answer: String::new(), feedback: "timed out".into(), full: String::new(), failures: None, note: Some("timed out".into()), by: None });
            }
            return;
        }
        let Some(pane) = visit.row.terminal.clone() else { return };
        let stopped = self.live.borrow().iter().any(|l| l.row.id == id && l.doc.held);
        if stopped && visit.reported.is_none() && visit.needs.is_none() {
            match &visit.work {
                Work::Agent { .. } => self.park(id, n),
                _ if self.mark_ending(id, n) => {
                    let (id, ended) = (id.to_string(), visit.clone());
                    self.end_visit(visit, move |this| this.rerun(&id, &ended, "interrupted"));
                }
                _ => {}
            }
            return;
        }
        match &visit.work {
            Work::Agent { agent } => {
                if let Some((ended, at)) = visit.reported.clone() {
                    if now() - at >= SETTLE && self.mark_ending(id, n) {
                        self.quit_then(id, n, &pane, agent, ended);
                    }
                    return;
                }
                if let Some(at) = visit.needs {
                    if now() - at >= SETTLE {
                        self.park(id, n);
                    }
                    return;
                }
                if core.pane(&pane).is_none_or(|p| p.lost()) {
                    return self.interrupted(id, visit);
                }
                if !alive(&core, &pane) {
                    self.close(&pane);
                    return self.stop(id, "stopped", &format!("{}'s agent exited without reporting it was done", visit.label()), Some("daemon"), true);
                }
                if late {
                    self.timed_out(id, n, &pane, agent);
                }
            }
            Work::Command { log, exit } => {
                if exit.is_file() {
                    return self.command_ended(id, n, &pane, log, exit);
                }
                if core.pane(&pane).is_none_or(|p| p.lost()) {
                    return self.interrupted(id, visit);
                }
                if !alive(&core, &pane) {
                    let code = core.pane(&pane).and_then(|p| p.exit_code).unwrap_or(1);
                    let _ = fs::write(exit, format!("{code}\n"));
                    self.command_ended(id, n, &pane, log, exit);
                }
            }
            Work::Ask { .. } => {}
        }
    }

    pub fn too_old(self: &Rc<Self>, pane: &str, why: &str) -> bool {
        let found = self.live.borrow().iter().find(|l| l.visits.iter().any(|v| v.row.terminal.as_deref() == Some(pane) && matches!(v.work, Work::Agent { .. }))).map(|l| l.row.id.clone());
        let Some(id) = found else { return false };
        self.stop(&id, "stopped", why, Some("daemon"), true);
        true
    }

    pub fn status(self: &Rc<Self>, pane: &str, ended: bool) {
        if !ended {
            return;
        }
        let found = self.live.borrow().iter().find_map(|l| {
            let visit = l.visits.iter().find(|v| v.row.terminal.as_deref() == Some(pane) && matches!(v.work, Work::Agent { .. }) && !v.opening && !v.ending && !v.paused)?;
            Some((l.row.id.clone(), visit.clone(), l.row.state.clone()))
        });
        let Some((id, visit, state)) = found else { return };
        let (n, stage) = (visit.n, visit.label());
        if let Some((ended, _)) = visit.reported {
            let Work::Agent { agent } = visit.work else { return };
            if self.mark_ending(&id, n) {
                self.quit_then(&id, n, pane, &agent, ended);
            }
            return;
        }
        if visit.needs.is_some() {
            return self.park(&id, n);
        }
        if state != "running" && !(state == "paused" && self.live.borrow().iter().any(|l| l.row.id == id && !l.doc.held)) {
            return;
        }
        let job = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit(n)).and_then(Visit::job);
        if let Some(job) = job.filter(|j| self.settle_job(&id, n, j).is_none()) {
            return self.wait_for_job(&id, n, &job);
        }
        let reminded = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit(n)).is_some_and(|v| v.row.doc.get("reminded").is_some());
        if self.owned(&id) && !reminded {
            return self.remind(&id, n, &stage);
        }
        let Some(core) = self.core.upgrade() else { return };
        let (this, closing) = (Rc::downgrade(self), pane.to_string());
        core.handle(json!({ "cmd": "read", "args": { "pane": pane, "lines": SCREEN_LINES } }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            let screen = reply.get("text").and_then(Value::as_str).unwrap_or("").lines().map(str::trim_end).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
            let still = this.live.borrow().iter().any(|l| l.row.id == id && l.visit(n).is_some_and(|v| v.row.terminal.as_deref() == Some(closing.as_str()) && v.reported.is_none() && v.needs.is_none() && !v.ending && !v.paused));
            if !still {
                return;
            }
            let mut flagged = None;
            this.update(&id, |live| {
                if let Some(visit) = live.visit_mut(n) {
                    visit.needs = Some(now());
                    flagged = Some(visit.row.clone());
                }
                None
            });
            if let Some(row) = flagged {
                this.flag_needs(&row);
            }
            let why = format!("{stage} stopped without reporting");
            this.pause(&id, &stage, &why, None, &format!("{stage}'s turn ended without it reporting it was done or what it needs{}, so the run is paused, its agent is quit and its session kept. Anything it left running in its own shell ended with it. Its screen ended:\n\n{screen}\n\n`kanna run resume {id} \"…\"` carries it on with your answer.", if reminded { ", again after Kanna carried it on once to report" } else { "" }));
            this.park(&id, n);
        });
    }

    fn flag_needs(&self, row: &StageRow) {
        if row.id == 0 {
            return;
        }
        let mut row = row.clone();
        row.doc = json!({ "needs": now() });
        let _ = self.db.put_stage(&row);
    }

    fn pause(&self, id: &str, stage: &str, why: &str, by: Option<&str>, tell: &str) {
        log!("kanna-daemon: {id} pauses: {why}");
        let (why, by, stage) = (why.to_string(), by.unwrap_or("daemon").to_string(), stage.to_string());
        self.update(id, |live| {
            live.row.state = "paused".into();
            live.doc.why = Some(why.clone());
            Some(("paused", Some(by), json!({ "stage": stage, "why": why })))
        });
        self.tell(id, tell);
    }

    fn park(self: &Rc<Self>, id: &str, n: u32) {
        let Some(visit) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit(n).cloned()) else { return };
        let (Work::Agent { agent }, Some(pane)) = (visit.work.clone(), visit.row.terminal.clone()) else { return };
        if visit.paused || visit.reported.is_some() || !self.mark_ending(id, n) {
            return;
        }
        let by_task = visit.needs.is_none() && self.live.borrow().iter().any(|l| l.row.id == id && l.doc.held);
        let (this, id) = (Rc::downgrade(self), id.to_string());
        let closing = pane.clone();
        let finish = move |said: Option<String>| {
            let Some(this) = this.upgrade() else { return };
            this.retire(&id, &closing);
            let mut parked = None;
            let mut started = false;
            this.update(&id, |live| {
                started = live.doc.held != by_task && !live.doc.pausing;
                if let Some(visit) = live.visit_mut(n) {
                    let session = visit.row.session.clone().or(said);
                    patch(&mut visit.row.doc, json!({ "paused": { "at": now(), "session": session, "task": by_task } }));
                    visit.paused = true;
                    visit.ending = false;
                    visit.needs = None;
                    parked = Some(visit.row.clone());
                }
                None
            });
            if let Some(row) = parked {
                log!("kanna-daemon: {id}'s {} is parked, its session {}", row.stage, row.doc["paused"]["session"].as_str().unwrap_or("the folder's latest"));
                let mut saved = row.clone();
                saved.doc = json!({ "paused": row.doc["paused"].clone(), "needs": null });
                let _ = this.db.put_stage(&saved);
                this.soon.set(true);
            }
            if by_task && started {
                this.carrying.borrow_mut().push(id.clone());
                this.soon.set(true);
            }
        };
        match self.tasks.upgrade() {
            Some(tasks) => tasks.end_agent(&pane, &agent, finish),
            None => finish(None),
        }
    }

    fn live_of(&self, row: &RunRow) -> Result<(String, Doc, Vec<Visit>, Pipeline), KmuxError> {
        let live = self.live.borrow();
        let live = live.iter().find(|l| l.row.id == row.id).ok_or_else(|| KmuxError::new("ended", format!("{} has ended ({})", row.id, row.outcome.as_deref().unwrap_or(&row.state))))?;
        Ok((live.row.state.clone(), live.doc.clone(), live.visits.clone(), live.pipeline.clone()))
    }

    fn who(&self, args: &Value, task: &str) -> String {
        let agent = string(args, "by").zip(self.tasks.upgrade()).and_then(|(by, tasks)| tasks.task_of(by)).is_some_and(|of| of == task);
        if agent { "agent" } else { "user" }.to_string()
    }

    fn resume_run(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let row = self.find_live(args)?;
        let text = string(args, "text").map(str::trim).filter(|t| !t.is_empty()).unwrap_or(CARRY_ON).to_string();
        let by = self.who(args, &row.task);
        let (state, doc, visits, _) = self.live_of(&row)?;
        let reply = |stage: &str, state: &str| Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), json!(stage)), ("state".to_string(), json!(state))]));
        if let Some(asking) = visits.iter().find(|v| matches!(v.work, Work::Ask { .. })) {
            return Err(KmuxError::new("asking", format!("{} asks: answer it with `kanna run answer {} yes|no [NOTE]`", asking.row.stage, row.id)));
        }
        if visits.iter().any(|v| v.paused) && state == "paused" {
            let stage = self.carry_on(&row.id, &text, &by)?;
            return reply(&stage, &self.state(&row.id).unwrap_or_default());
        }
        if let Some(visit) = visits.iter().find(|v| !v.paused && !v.ending).filter(|_| doc.pausing || doc.held) {
            let stage = visit.label();
            self.update(&row.id, |live| {
                live.doc.pausing = false;
                live.doc.held = false;
                if live.row.state == "paused" && !live.holding() {
                    live.row.state = "running".into();
                }
                if live.row.state == "running" {
                    live.doc.why = None;
                }
                intervene(live, "resumed");
                Some(("resumed", Some(by.clone()), json!({ "stage": stage })))
            });
            self.soon.set(true);
            return reply(&stage, &self.state(&row.id).unwrap_or_default());
        }
        if visits.iter().any(|v| v.ending || v.opening || v.needs.is_some()) {
            return Err(KmuxError::new("busy", "its stage's agent is still stopping: try again in a moment"));
        }
        if visits.is_empty() && state == "paused" {
            let next = doc.next.clone().or_else(|| doc.step.as_ref().map(|s| s.stage.clone())).unwrap_or_default();
            self.update(&row.id, |live| {
                live.row.state = "running".into();
                live.doc.why = None;
                live.doc.pausing = false;
                live.doc.held = false;
                intervene(live, "resumed");
                Some(("resumed", Some(by.clone()), json!({ "next": next })))
            });
            self.soon.set(true);
            return reply(&next, "running");
        }
        Err(KmuxError::new("not_paused", format!("{} isn't paused ({})", row.id, state)))
    }

    fn carry_on(self: &Rc<Self>, id: &str, text: &str, by: &str) -> Result<String, KmuxError> {
        self.carry_on_visit(id, None, text, by)
    }

    fn carry_on_visit(self: &Rc<Self>, id: &str, which: Option<u32>, text: &str, by: &str) -> Result<String, KmuxError> {
        let (run, doc, visit, stage) = {
            let live = self.live.borrow();
            let live = live.iter().find(|l| l.row.id == id).ok_or_else(|| KmuxError::new("ended", format!("{id} has ended")))?;
            let visit = live.visits.iter().find(|v| v.paused && which.is_none_or(|n| v.n == n)).cloned().ok_or_else(|| KmuxError::new("not_paused", format!("{id} has no stage to carry on")))?;
            let stage = live.pipeline.stage(&visit.row.stage).cloned().ok_or_else(|| failed(format!("{} isn't in the pipeline any more", visit.row.stage)))?;
            (live.row.clone(), live.doc.clone(), visit, stage)
        };
        let (text, by) = (text.to_string(), by.to_string());
        let Kind::Agent(agent) = &stage.kind else { return Err(failed(format!("{} isn't an agent stage", stage.name))) };
        let n = visit.n;
        let replies = visit.row.doc.get("replies").and_then(Value::as_u64).unwrap_or(0) + 1;
        let worktree = visit.worktree();
        let worker = PathBuf::from(&worktree);
        let reply = format!("{SCRATCH}/reply-{n}-{replies}.md");
        fs::write(worker.join(&reply), &text).map_err(|e| failed(format!("can't write {reply}: {e}")))?;
        let files = tasks::config_files(Path::new(&run.repo));
        let mut vars = self.vars(&run, &doc, &visit.row, &stage.name);
        vars.push(("KANNA_ANSWER", worker.join(format!("{SCRATCH}/answer-{n}.md")).to_string_lossy().into_owned()));
        vars.push(("KANNA_CLI", tasks::cli_path()));
        let exports: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect();
        let session = visit.row.doc["paused"]["session"].as_str().map(String::from);
        let command = tasks::pipeline_agent(&files, &agent.agent);
        let first = format!("\"$(cat {})\"", quote(&reply));
        let line = format!("export {}; {}", exports.join(" "), tasks::stage_agent(&command, agent.effort.as_deref(), agent.model.as_deref(), tasks::prompts(&files), session.as_deref(), true, &first));
        let paused_at = visit.row.doc["paused"]["at"].as_f64().unwrap_or_else(now);
        let label = visit.label();
        self.update(&run.id, |live| {
            live.doc.held = false;
            live.doc.pausing = false;
            if by != "task" && by != "daemon" {
                intervene(live, "resumed");
            }
            if let Some(visit) = live.visit_mut(n) {
                let waited = visit.waited() + (now() - paused_at).max(0.0);
                patch(&mut visit.row.doc, json!({ "paused": null, "waited": waited, "replies": replies }));
                visit.paused = false;
                visit.opening = true;
            }
            if !live.holding() {
                live.row.state = "running".into();
                live.doc.why = None;
            }
            Some(("resumed", Some(by.clone()), json!({ "stage": label, "reply": text })))
        });
        log!("kanna-daemon: {} resumes {label}", run.id);
        self.open_worker(&run.id, n, &worktree, &line, move |this, id, pane| {
            let mut saved = None;
            this.update(id, |live| {
                if let Some(visit) = live.visit_mut(n) {
                    visit.row.terminal = Some(pane.to_string());
                    visit.opening = false;
                    saved = Some(visit.row.clone());
                }
                None
            });
            if let Some(mut row) = saved {
                row.doc["paused"] = Value::Null;
                let _ = this.db.put_stage(&row);
            }
        });
        Ok(stage.name.clone())
    }

    fn answer(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let row = self.find_live(args)?;
        let said = string(args, "answer").ok_or_else(|| bad("say yes or no: kanna run answer [RUN] yes|no [NOTE]"))?.to_string();
        let verdict = match said.as_str() {
            "yes" => Verdict::Pass,
            "no" => Verdict::Fail,
            other => return Err(bad(format!("answer yes or no, not \"{other}\""))),
        };
        let note = string(args, "note").map(str::trim).unwrap_or("").to_string();
        let (state, _, visits, _) = self.live_of(&row)?;
        let visit = visits.into_iter().find(|v| matches!(v.work, Work::Ask { .. }) && !v.ending).ok_or_else(|| {
            let hint = if state == "paused" { format!(": a paused stage carries on with `kanna run resume {} \"…\"`", row.id) } else { String::new() };
            KmuxError::new("no_ask", format!("{} isn't waiting on an ask ({state}){hint}", row.id))
        })?;
        let (by, stage) = (self.who(args, &row.task), visit.row.stage.clone());
        self.mark_ending(&row.id, visit.n);
        self.update(&row.id, |live| {
            intervene(live, "answered");
            if live.row.state == "waiting" {
                live.row.state = "running".into();
                live.doc.why = None;
            }
            Some(("answered", Some(by.clone()), json!({ "stage": stage, "answer": said, "note": note })))
        });
        log!("kanna-daemon: {}'s {stage} is answered {said} by the {by}", row.id);
        self.finished(&row.id, visit.n, Ended { verdict: Some(verdict), shown: verdict.name(), answer: note.clone(), feedback: note.clone(), full: String::new(), failures: None, note: None, by: Some(by.clone()) });
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), json!(stage)), ("answer".to_string(), json!(said)), ("by".to_string(), json!(by))]))
    }

    fn pause_run(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let row = self.find_live(args)?;
        let by = self.who(args, &row.task);
        let (state, doc, visits, _) = self.live_of(&row)?;
        let running: Vec<&Visit> = visits.iter().filter(|v| !v.paused).collect();
        if doc.pausing || (state == "paused" && !doc.held) {
            return Err(KmuxError::new("paused", format!("{} is paused already{}: `kanna run resume {}` carries it on", row.id, running.first().map_or(String::new(), |v| format!(" once {} ends", v.label())), row.id)));
        }
        let stage = running.first().map(|v| v.label());
        let now_paused = visits.is_empty();
        self.update(&row.id, |live| {
            live.doc.pausing = true;
            if now_paused {
                live.row.state = "paused".into();
                live.doc.why = Some("paused by you".into());
            } else if live.row.state == "running" {
                live.doc.why = stage.as_ref().map(|s| format!("pauses once {s} ends"));
            }
            intervene(live, "paused");
            Some(("paused", Some(by.clone()), json!({ "stage": stage, "why": "paused by you" })))
        });
        log!("kanna-daemon: {} is paused by the {by}", row.id);
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), opt(stage.as_deref())), ("state".to_string(), json!(if now_paused { "paused" } else { "pausing" }))]))
    }

    fn steered(&self, args: &Value, verb: &str) -> Result<(RunRow, Visit, Stage, String), KmuxError> {
        let row = self.find_live(args)?;
        let (state, _, visits, pipeline) = self.live_of(&row)?;
        let visit = match visits.as_slice() {
            [] => return Err(KmuxError::new("no_stage", format!("{} has no stage to {verb} ({state}{})", row.id, if state == "paused" { ": `kanna run resume` starts the next" } else { "" }))),
            [visit] => visit.clone(),
            several => return Err(KmuxError::new("several", format!("{} has {} workers at once ({}): {verb} takes a run with one", row.id, several.len(), several.iter().map(Visit::label).collect::<Vec<_>>().join(", ")))),
        };
        if visit.opening || visit.ending {
            return Err(KmuxError::new("busy", format!("{} is starting or stopping: try again in a moment", visit.label())));
        }
        let stage = pipeline.stage(&visit.row.stage).cloned().ok_or_else(|| failed(format!("{} isn't in the pipeline any more", visit.row.stage)))?;
        let by = self.who(args, &row.task);
        Ok((row, visit, stage, by))
    }

    fn skip(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (row, visit, stage, by) = self.steered(args, "skip")?;
        let verdict = stage.verdicts().then_some(Verdict::Pass);
        let ended = Ended { verdict, shown: "skipped", answer: String::new(), feedback: String::new(), full: String::new(), failures: None, note: Some(format!("skipped by the {by}")), by: Some(by.clone()) };
        self.mark_ending(&row.id, visit.n);
        self.update(&row.id, |live| {
            intervene(live, "skipped");
            if live.row.state == "waiting" {
                live.row.state = "running".into();
                live.doc.why = None;
            }
            Some(("skipped", Some(by.clone()), json!({ "stage": stage.name, "visit": visit.row.visit })))
        });
        log!("kanna-daemon: {}'s {} is skipped by the {by}", row.id, visit.label());
        let (id, n) = (row.id.clone(), visit.n);
        self.end_visit(&visit, move |this| this.finished(&id, n, ended));
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), json!(stage.name))]))
    }

    fn retry(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (row, visit, stage, by) = self.steered(args, "retry")?;
        self.mark_ending(&row.id, visit.n);
        self.update(&row.id, |live| {
            intervene(live, "retried");
            Some(("retried", Some(by.clone()), json!({ "stage": stage.name, "visit": visit.row.visit })))
        });
        log!("kanna-daemon: {}'s {} runs again for the {by}", row.id, visit.label());
        let (id, ended) = (row.id.clone(), visit.clone());
        self.end_visit(&visit, move |this| this.rerun(&id, &ended, "retried"));
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), json!(stage.name))]))
    }

    fn end_visit(self: &Rc<Self>, visit: &Visit, then: impl FnOnce(&Rc<Self>) + 'static) {
        let this = Rc::downgrade(self);
        let pane = visit.row.terminal.clone().filter(|_| !visit.paused);
        let closing = pane.clone();
        let finish = move || {
            let Some(this) = this.upgrade() else { return };
            if let Some(pane) = &closing {
                this.close(pane);
            }
            then(&this);
        };
        match (&visit.work, pane) {
            (Work::Agent { agent }, Some(pane)) => match self.tasks.upgrade() {
                Some(tasks) => tasks.end_agent(&pane, agent, move |_| finish()),
                None => finish(),
            },
            (Work::Command { .. }, Some(pane)) => match self.core.upgrade().filter(|core| alive(core, &pane)) {
                Some(core) => core.end(&pane, move |_| finish()),
                None => finish(),
            },
            _ => finish(),
        }
    }

    fn open_worker(self: &Rc<Self>, id: &str, n: u32, cwd: &str, command: &str, opened: impl FnOnce(&Rc<Self>, &str, &str) + 'static) {
        let Some(core) = self.core.upgrade() else { return };
        let last = self.live.borrow_mut().iter_mut().find(|l| l.row.id == id).and_then(|l| l.last.take());
        let Some(old) = last else { return self.open_beside(id, n, cwd, command, None, opened) };
        let (this, id, cwd, command) = (Rc::downgrade(self), id.to_string(), cwd.to_string(), command.to_string());
        core.handle(json!({ "cmd": "list" }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            let shown = shown(&reply, &old);
            this.open_beside(&id, n, &cwd, &command, Some((old, shown)), opened);
        });
    }

    fn open_beside(self: &Rc<Self>, id: &str, n: u32, cwd: &str, command: &str, last: Option<(String, bool)>, opened: impl FnOnce(&Rc<Self>, &str, &str) + 'static) {
        let Some(core) = self.core.upgrade() else { return };
        let task = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.task.clone()).unwrap_or_default();
        let args = match &last {
            Some((old, true)) => json!({ "type": "term", "from": old, "cwd": cwd, "cmd": command, "wait": false }),
            _ => json!({ "type": "term", "background": true, "workspace": task, "cwd": cwd, "cmd": command, "wait": false }),
        };
        let (this, id) = (Rc::downgrade(self), id.to_string());
        core.handle(json!({ "cmd": "open", "args": args }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            if let Some((old, _)) = &last {
                this.close(old);
            }
            let pane = reply.get("pane").and_then(|p| string(p, "id")).map(String::from);
            let Some(pane) = pane.filter(|_| reply.get("ok") == Some(&Value::Bool(true))) else {
                let why = reply.get("error").and_then(|e| string(e, "message")).unwrap_or("it failed").to_string();
                let stage = this.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit(n)).map(|v| v.label()).unwrap_or_default();
                this.update(&id, |live| {
                    if let Some(visit) = live.visit_mut(n) {
                        visit.ending = true;
                    }
                    None
                });
                return this.stop(&id, "stopped", &format!("can't open a terminal for {stage}: {why}"), Some("daemon"), true);
            };
            if !LIVE.contains(&this.state(&id).unwrap_or_default().as_str()) {
                this.close(&pane);
                return;
            }
            if let Some((old, true)) = &last {
                log!("kanna-daemon: {id}'s next worker {pane} takes the tab {old} was shown in");
                this.name_tab(&id, &pane);
            }
            this.heed_typing(&pane);
            opened(&this, &id, &pane);
            this.soon.set(true);
        });
    }

    fn retire(&self, id: &str, pane: &str) {
        let before = {
            let mut live = self.live.borrow_mut();
            match live.iter_mut().find(|l| l.row.id == id) {
                Some(live) if live.visits.iter().filter(|v| v.row.terminal.is_some()).count() <= 1 => live.last.replace(pane.to_string()),
                _ => Some(pane.to_string()),
            }
        };
        if let Some(before) = before.filter(|b| b != pane) {
            self.close(&before);
        }
    }

    fn close_last(&self, id: &str) {
        let last = self.live.borrow_mut().iter_mut().find(|l| l.row.id == id).and_then(|l| l.last.take());
        if let Some(last) = last {
            self.close(&last);
        }
    }

    fn heed_typing(&self, pane: &str) {
        let terminal = self.core.upgrade().and_then(|core| core.pane(pane)).and_then(|p| p.terminal).unwrap_or_else(|| pane.to_string());
        if let Some(heed) = self.heed.borrow().as_ref() {
            heed(&terminal);
        }
    }

    pub fn typed(&self, pane: &str) {
        let found = self.live.borrow().iter().find_map(|l| {
            let visit = l.visits.iter().find(|v| v.row.terminal.as_deref() == Some(pane))?;
            Some((l.row.id.clone(), visit.row.stage.clone(), visit.row.visit, visit.row.instance.clone()))
        });
        let Some((id, stage, visit, instance)) = found else { return };
        log!("kanna-daemon: you typed into {id}'s {stage} ({pane})");
        if let Err(error) = self.db.run_event(&id, "typed", Some("user"), &json!({ "stage": stage, "visit": visit, "instance": instance, "terminal": pane })) {
            log!("kanna-daemon: couldn't record typing into {id}: {error}");
        }
        self.publish(&id);
    }

    fn vars(&self, row: &RunRow, doc: &Doc, visit: &StageRow, stage: &str) -> Vec<(&'static str, String)> {
        let task = self.tasks.upgrade().and_then(|t| t.entry(&row.task));
        let mut vars = vec![
            ("KANNA_TASK", row.task.clone()),
            ("KANNA_TASK_NAME", task.as_ref().and_then(|t| t.name.clone()).unwrap_or_default()),
            ("KANNA_REPO", row.repo.clone()),
            ("KANNA_WORKTREE", visit.worktree.clone().unwrap_or_else(|| doc.worker.clone())),
            ("KANNA_BRANCH", visit.branch.clone().unwrap_or_else(|| row.branch.clone())),
            ("KANNA_BASE", doc.base.clone()),
            ("KANNA_RUN", row.id.clone()),
            ("KANNA_STAGE", stage.to_string()),
        ];
        if let Some(instance) = &visit.instance {
            vars.push(("KANNA_INSTANCE", instance.clone()));
        }
        vars
    }

    fn mark_ending(&self, id: &str, n: u32) -> bool {
        let mut live = self.live.borrow_mut();
        let Some(visit) = live.iter_mut().find(|l| l.row.id == id).and_then(|l| l.visit_mut(n)) else { return false };
        if visit.ending {
            return false;
        }
        visit.ending = true;
        true
    }

    fn close(&self, pane: &str) {
        if let Some(core) = self.core.upgrade() {
            core.handle(json!({ "cmd": "close", "args": { "pane": pane } }), |_| {});
        }
    }

    fn quit_then(self: &Rc<Self>, id: &str, n: u32, pane: &str, agent: &str, ended: Ended) {
        let (this, id, closing) = (Rc::downgrade(self), id.to_string(), pane.to_string());
        let finish = move |_: Option<String>| {
            let Some(this) = this.upgrade() else { return };
            this.retire(&id, &closing);
            this.finished(&id, n, ended);
        };
        match self.tasks.upgrade() {
            Some(tasks) => tasks.end_agent(pane, agent, finish),
            None => finish(None),
        }
    }

    fn timed_out(self: &Rc<Self>, id: &str, n: u32, pane: &str, agent: &str) {
        if !self.mark_ending(id, n) {
            return;
        }
        let Some((worker, start, stage, verdicts)) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| {
            let visit = l.visit(n)?;
            Some((visit.worktree(), string(&visit.row.doc, "start").unwrap_or("").to_string(), visit.label(), l.pipeline.stage(&visit.row.stage).is_some_and(Stage::verdicts)))
        }) else {
            return;
        };
        if !start.is_empty() {
            let _ = git(Path::new(&worker), &["reset", "--hard", &start]);
        }
        if !verdicts {
            if let Some(tasks) = self.tasks.upgrade() {
                let (this, closing) = (Rc::downgrade(self), pane.to_string());
                tasks.end_agent(pane, agent, move |_| {
                    if let Some(this) = this.upgrade() {
                        this.close(&closing);
                    }
                });
            }
            let why = format!("{stage} timed out before its agent reported it was done");
            return self.stop(id, "stopped", &why, Some("daemon"), true);
        }
        let ended = Ended { verdict: Some(Verdict::Fail), shown: "fail", answer: String::new(), feedback: "timed out".into(), full: String::new(), failures: None, note: Some("timed out".into()), by: None };
        self.quit_then(id, n, pane, agent, ended);
    }

    fn command_ended(self: &Rc<Self>, id: &str, n: u32, pane: &str, log: &Path, exit: &Path) {
        if !self.mark_ending(id, n) {
            return;
        }
        let code: i32 = fs::read_to_string(exit).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(1);
        let output = fs::read_to_string(log).unwrap_or_default();
        let mut feedback = command_feedback(&output);
        let note = task_scripts::timed_out(code).then(|| "timed out".to_string());
        if note.is_some() {
            feedback = format!("timed out\n{feedback}");
        }
        let verdict = if code == 0 { Verdict::Pass } else { Verdict::Fail };
        let ended = Ended { verdict: Some(verdict), shown: verdict.name(), answer: String::new(), feedback, full: log.to_string_lossy().into_owned(), failures: Some(failure_lines(&output)), note, by: None };
        let (this, id, closing) = (Rc::downgrade(self), id.to_string(), pane.to_string());
        let finish = move || {
            let Some(this) = this.upgrade() else { return };
            this.retire(&id, &closing);
            this.finished(&id, n, ended);
        };
        match self.core.upgrade().filter(|core| alive(core, pane)) {
            Some(core) => core.end(pane, move |_| finish()),
            None => finish(),
        }
    }

    fn interrupted(self: &Rc<Self>, id: &str, visit: &Visit) {
        log!("kanna-daemon: {id}'s {} visit lost its terminal, so it's recorded interrupted and runs again", visit.label());
        if let Some(pane) = visit.row.terminal.as_deref() {
            self.close(pane);
        }
        self.rerun(id, visit, "interrupted");
    }

    fn rerun(self: &Rc<Self>, id: &str, visit: &Visit, verdict: &str) {
        self.drop_job(visit);
        let mut row = visit.row.clone();
        row.ended_at = Some(now());
        row.verdict = Some(verdict.into());
        if row.id != 0 {
            let _ = self.db.put_stage(&row);
        }
        let (stage, n) = (row.stage.clone(), visit.n);
        self.update(id, |live| {
            live.visits.retain(|v| v.n != n);
            if live.doc.step.is_none() {
                live.doc.next = Some(stage.clone());
            }
            if !live.holding() {
                held(live, &stage);
            }
            (verdict == "interrupted").then(|| ("verdict", Some("daemon".into()), json!({ "stage": stage, "visit": row.visit, "instance": opt(row.instance.as_deref()), "verdict": verdict })))
        });
        self.soon.set(true);
    }

    fn begin(self: &Rc<Self>, id: &str, name: &str) {
        let Some((stage, pipeline, doc, row)) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| Some((l.pipeline.stage(name).cloned()?, l.pipeline.clone(), l.doc.clone(), l.row.clone()))) else { return };
        let mut items = vec![];
        let mut sizes = Sizes::new();
        if let Some(each) = stage.agent().and_then(|a| a.each.clone()) {
            let key = each.to_string();
            let Some(output) = doc.answers.get(&key) else {
                return self.stop(id, "stopped", &format!("{name} fans out over {key}, and {} gave no `json name={}` block in its answer", each.stage, each.key), Some("daemon"), true);
            };
            items = match parts(output) {
                Ok(items) => items,
                Err(error) => return self.stop(id, "stopped", &format!("{name} fans out over {key}, which {error}"), Some("daemon"), true),
            };
            sizes.insert(key.clone(), items.len() as u32);
            if let Some(max) = pipeline.budget.as_ref().and_then(|b| b.sessions) {
                let worst = pipeline.max_sessions(&sizes);
                if worst > max {
                    return self.wait(id, &format!("over budget: {key} has {} items, which makes the worst case {worst} sessions of {max}", items.len()));
                }
            }
        }
        if let Some(attempts) = stage.agent().and_then(|a| a.attempts.clone()) {
            items = (1..=attempts.count).map(|k| Item { name: format!("attempt-{k}"), ..Item::default() }).collect();
        }
        let elapsed = Duration::from_secs_f64((now() - row.started_at).max(0.0));
        if let Some(why) = over_budget(&pipeline, &doc, &stage, &sizes, elapsed) {
            return self.wait(id, &why);
        }
        let visit = self.db.stages(id).map(|s| s.iter().filter(|s| s.stage == name).map(|s| s.visit).max().unwrap_or(0) + 1).unwrap_or(1);
        let head = git(Path::new(&doc.worker), &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        if let Kind::Merge(from) = &stage.kind {
            return self.join_stage(id, name, from, visit, &head);
        }
        let fans = stage.agent().is_some_and(|a| a.each.is_some());
        let stale: Vec<Item> = if fans { doc.parts.get(name).cloned().unwrap_or_default() } else { vec![] };
        for part in &stale {
            remove_worker(Path::new(&row.repo), &part.worktree, Some(&part.branch));
        }
        let step = Step { stage: name.to_string(), visit, start: head.clone(), attempts: stage.agent().is_some_and(|a| a.attempts.is_some()), items, ..Step::default() };
        self.update(id, |live| {
            live.doc.starts.entry(name.to_string()).or_insert(head.clone());
            if fans {
                live.doc.parts.remove(name);
            }
            live.doc.step = Some(step);
            live.doc.next = None;
            None
        });
        self.pump(id);
    }

    fn pump(self: &Rc<Self>, id: &str) {
        let Some((step, stage, state, visits)) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| {
            let step = l.doc.step.clone()?;
            Some((step.clone(), l.pipeline.stage(&step.stage).cloned()?, l.row.state.clone(), l.visits.clone()))
        }) else {
            return;
        };
        let running = state == "running";
        if !stage.agent().is_some_and(|a| a.each.is_some() || a.attempts.is_some()) {
            if !visits.is_empty() {
                return;
            }
            let guarded = stage.agent().is_some_and(|a| a.guard.is_some());
            let launch = match (guarded, step.before, &step.main, step.after) {
                (true, None, _, _) => Some(Role::Score(None, true)),
                (_, _, None, _) => Some(Role::Main(None)),
                (true, _, Some(_), None) => Some(Role::Score(None, false)),
                _ => None,
            };
            return match launch {
                Some(role) if running => self.launch(id, role),
                Some(_) => {}
                None => self.finish_single(id),
            };
        }
        let open: Vec<usize> = visits.iter().filter_map(|v| v.role.item()).collect();
        let ready = |i: usize, item: &Item| item.ended.is_some() && (!step.attempts || item.score.is_some()) && !open.contains(&i);
        if step.items.iter().enumerate().all(|(i, item)| ready(i, item)) {
            return self.finish_items(id);
        }
        if !running {
            return;
        }
        let limit = tasks::pipeline_workers(&tasks::config_files(Path::new(&self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.repo.clone()).unwrap_or_default())));
        let mut busy = open.len();
        let mut launches = vec![];
        for (i, item) in step.items.iter().enumerate() {
            if open.contains(&i) {
                continue;
            }
            if item.ended.is_some() && step.attempts && item.score.is_none() {
                launches.push(Role::Score(Some(i), false));
            } else if item.ended.is_none() && busy < limit {
                busy += 1;
                launches.push(Role::Main(Some(i)));
            }
        }
        for role in launches {
            if self.state(id).as_deref() != Some("running") {
                return;
            }
            self.launch(id, role);
        }
    }

    fn launch(self: &Rc<Self>, id: &str, role: Role) {
        let Some((step, stage, scorer, doc, row)) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| {
            let step = l.doc.step.clone()?;
            Some((step.clone(), l.pipeline.stage(&step.stage).cloned()?, l.scorer(role), l.doc.clone(), l.row.clone()))
        }) else {
            return;
        };
        let mut item = role.item().and_then(|i| step.items.get(i).cloned());
        if let Some(item) = item.as_mut().filter(|item| item.worktree.is_empty()) {
            let worker = Path::new(&doc.worker).file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let mut k = 1;
            let made = loop {
                let suffix = if k == 1 { item.name.clone() } else { format!("{}-{k}", item.name) };
                match tasks::add_worktree(Path::new(&row.repo), &format!("{worker}-{suffix}"), &format!("{}-{suffix}", row.branch), Some(&step.start)) {
                    Ok(made) => break made,
                    Err(error) if error.contains("is there already") && k < 100 => k += 1,
                    Err(error) => return self.stop(id, "stopped", &format!("can't make {}'s worktree: {error}", item.label(&step.stage)), Some("daemon"), true),
                }
            };
            (item.worktree, item.branch) = made;
            let (index, saved) = (role.item().unwrap_or(0), item.clone());
            self.update(id, |live| {
                if let Some(step) = live.doc.step.as_mut() {
                    step.items[index].worktree = saved.worktree.clone();
                    step.items[index].branch = saved.branch.clone();
                }
                None
            });
        }
        let (worktree, branch) = item.as_ref().map_or_else(|| (doc.worker.clone(), row.branch.clone()), |item| (item.worktree.clone(), item.branch.clone()));
        let instance = match role {
            Role::Main(Some(_)) | Role::Score(Some(_), _) => item.as_ref().map(|i| i.name.clone()),
            Role::Score(None, before) => Some(if before { "before" } else { "after" }.to_string()),
            Role::Main(None) => None,
        };
        let running = match role {
            Role::Main(_) => stage.clone(),
            Role::Score(..) => match scorer {
                Some(scorer) => scorer,
                None => return self.stop(id, "stopped", &format!("{} has no run stage to score it", step.stage), Some("daemon"), true),
            },
        };
        let worker = PathBuf::from(&worktree);
        let head = git(&worker, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let n = doc.visits + 1;
        let scratch = worker.join(SCRATCH);
        if let Err(error) = fs::create_dir_all(&scratch) {
            return self.stop(id, "stopped", &format!("can't make {}: {error}", scratch.display()), Some("daemon"), true);
        }
        let mut row_doc = json!({ "n": n, "start": head, "role": role.to_value() });
        let mut stage_row = StageRow {
            id: 0,
            run: id.to_string(),
            stage: step.stage.clone(),
            visit: step.visit,
            instance: instance.clone(),
            kind: String::new(),
            agent: None,
            machine: self.host.clone(),
            started_at: now(),
            ended_at: None,
            verdict: None,
            session: None,
            terminal: None,
            worktree: Some(worktree.clone()),
            branch: Some(branch.clone()),
            doc: Value::Null,
        };
        if let Kind::Ask { question } = &running.kind {
            return self.ask(id, n, role, stage_row, row_doc, question);
        }
        let files = tasks::config_files(Path::new(&row.repo));
        let mut vars = self.vars(&row, &doc, &stage_row, &step.stage);
        let (command, work, sessions) = match &running.kind {
            Kind::Agent(agent) => {
                let prompts = self.db.revisions(id).ok().and_then(|r| r.last().and_then(|r| r.doc.get("prompts").cloned())).unwrap_or(Value::Null);
                let file = running.prompt().and_then(|p| string(&prompts, p).map(String::from)).or_else(|| running.prompt().and_then(|p| fs::read_to_string(worker.join(format!(".kanna/prompts/{p}.md"))).ok())).unwrap_or_default();
                let answer = format!("{SCRATCH}/answer-{n}.md");
                let part = item.as_ref().filter(|_| !step.attempts).map(|i| i.text.as_str());
                let text = prompt(&running, &file, &doc.request, part, doc.feedback.as_ref(), &doc.answers, &answer);
                vars.push(("KANNA_ANSWER", worker.join(&answer).to_string_lossy().into_owned()));
                vars.push(("KANNA_CLI", tasks::cli_path()));
                let path = scratch.join(format!("stage-{n}.md"));
                if let Err(error) = fs::write(&path, &text) {
                    return self.stop(id, "stopped", &format!("can't write {}: {error}", path.display()), Some("daemon"), true);
                }
                let session = tasks::names_its_session(&agent.agent).then(uuid).flatten();
                let first = format!("\"$(cat {})\"", quote(&format!("{SCRATCH}/stage-{n}.md")));
                let exports: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect();
                let command = tasks::pipeline_agent(&files, &agent.agent);
                let line = format!("export {}; {}", exports.join(" "), tasks::stage_agent(&command, agent.effort.as_deref(), agent.model.as_deref(), tasks::prompts(&files), session.as_deref(), false, &first));
                stage_row.kind = "agent".into();
                stage_row.agent = Some(agent.agent.clone());
                stage_row.session = session;
                row_doc["prompt"] = json!(path.to_string_lossy());
                (line, Work::Agent { agent: agent.agent.clone() }, 1)
            }
            Kind::Run { command } => {
                let (log, exit, feedback) = (scratch.join(format!("stage-{n}.log")), scratch.join(format!("stage-{n}.exit")), scratch.join(format!("feedback-{n}.txt")));
                let _ = fs::remove_file(&exit);
                let previous = doc.feedback.as_ref().map(|f| fs::read_to_string(&f.full).unwrap_or_else(|_| f.text.clone())).unwrap_or_default();
                let _ = fs::write(&feedback, previous);
                vars.push(("KANNA_FEEDBACK", feedback.to_string_lossy().into_owned()));
                let timeout = command_timeout(running.within);
                stage_row.kind = if matches!(role, Role::Score(..)) { "score".into() } else { "run".into() };
                if running.name != step.stage {
                    row_doc["scorer"] = json!(running.name);
                }
                row_doc["log"] = json!(log.to_string_lossy());
                row_doc["exit"] = json!(exit.to_string_lossy());
                (task_scripts::stage_command(&vars, command, timeout, &log, &exit), Work::Command { log, exit }, 0)
            }
            _ => return self.stop(id, "stopped", &format!("{} is a kind of stage runs can't do yet", running.name), Some("daemon"), true),
        };
        stage_row.doc = row_doc;
        let within = running.within;
        let visit_row = stage_row.clone();
        self.update(id, |live| {
            live.doc.visits = n;
            live.doc.sessions += sessions;
            live.visits.push(Visit { n, role, row: visit_row, work: work.clone(), within, opening: true, ending: false, reported: None, needs: None, paused: false });
            None
        });
        self.open_worker(id, n, &worktree, &command, move |this, id, pane| {
            stage_row.terminal = Some(pane.to_string());
            if let Ok(row_id) = this.db.put_stage(&stage_row) {
                stage_row.id = row_id;
            }
            let label = label(&stage_row);
            log!("kanna-daemon: {id} starts {label} (visit {}) in {pane}", stage_row.visit);
            let event = json!({ "stage": stage_row.stage, "visit": stage_row.visit, "instance": opt(stage_row.instance.as_deref()), "kind": stage_row.kind, "terminal": pane });
            this.update(id, |live| {
                if let Some(visit) = live.visit_mut(n) {
                    visit.row = stage_row.clone();
                    visit.opening = false;
                }
                Some(("stage", Some("daemon".into()), event))
            });
        });
    }

    fn ask(self: &Rc<Self>, id: &str, n: u32, role: Role, mut stage_row: StageRow, mut row_doc: Value, question: &str) {
        let name = stage_row.stage.clone();
        row_doc["question"] = json!(question);
        stage_row.kind = "ask".into();
        stage_row.doc = row_doc;
        match self.db.put_stage(&stage_row) {
            Ok(row_id) => stage_row.id = row_id,
            Err(error) => return self.stop(id, "stopped", &format!("can't save {name}'s visit: {error}"), Some("daemon"), true),
        }
        let within = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.pipeline.stage(&name).and_then(|s| s.within));
        let why = format!("{name} asks: {question}");
        let event = json!({ "stage": name, "visit": stage_row.visit, "kind": "ask" });
        self.update(id, |live| {
            live.doc.visits = n;
            live.doc.next = None;
            live.visits.push(Visit { n, role, row: stage_row, work: Work::Ask { question: question.to_string() }, within, opening: false, ending: false, reported: None, needs: None, paused: false });
            Some(("stage", Some("daemon".into()), event))
        });
        self.update(id, |live| {
            live.row.state = "waiting".into();
            live.doc.why = Some(why.clone());
            Some(("waiting", Some("daemon".into()), json!({ "stage": name, "why": why })))
        });
        log!("kanna-daemon: {id} waits: {why}");
        self.tell(id, &format!("{name} asks: {question}\nAsk the user in your chat, and answer as they say: `kanna run answer {id} yes \"NOTE\"` or `kanna run answer {id} no \"NOTE\"` (the note goes to the next stage). The task shows needs you until one of you answers."));
    }

    fn finished(self: &Rc<Self>, id: &str, n: u32, ended: Ended) {
        let Some(visit) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit(n).cloned()) else { return };
        self.drop_job(&visit);
        let worktree = visit.worktree();
        let end = git(Path::new(&worktree), &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let start = string(&visit.row.doc, "start").unwrap_or("").to_string();
        let commits = if start.is_empty() || end.is_empty() { 0 } else { git(Path::new(&worktree), &["rev-list", "--count", &format!("{start}..{end}")]).ok().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0) };
        let mut row = visit.row.clone();
        row.ended_at = Some(now());
        row.verdict = Some(ended.shown.to_string());
        let mut answer = ended.answer.clone();
        if answer.len() > ANSWER_CHARS {
            let mut cut = ANSWER_CHARS;
            while !answer.is_char_boundary(cut) {
                cut -= 1;
            }
            answer.truncate(cut);
        }
        let score = matches!(visit.role, Role::Score(..)).then(|| score_of(&fs::read_to_string(&ended.full).unwrap_or_default(), ended.verdict));
        row.doc = json!({ "answer": answer, "feedback": ended.feedback, "failures": ended.failures, "note": opt(ended.note.as_deref()), "end": end, "commits": commits, "score": score, "by": opt(ended.by.as_deref()) });
        if let Err(error) = self.db.put_stage(&row) {
            log!("kanna-daemon: couldn't save {id}'s {} visit: {error}", visit.label());
        }
        log!("kanna-daemon: {id}: {}'s visit ended {}{}", visit.label(), ended.shown, score.map_or(String::new(), |s| format!(", score {s}")));
        if ends_owned(self.owned(id), ended.note.as_deref()) {
            let stage = visit.label();
            log!("kanna-daemon: {id}: {stage} timed out, which ends its owner's run");
            let event = json!({ "stage": visit.row.stage, "visit": row.visit, "instance": opt(row.instance.as_deref()), "verdict": ended.shown, "next": Value::Null, "commits": commits, "note": "timed out" });
            self.update(id, |live| {
                live.visits.retain(|v| v.n != n);
                Some(("verdict", Some("daemon".into()), event))
            });
            return self.stop_as(id, "stopped", &format!("{stage} timed out"), Some("daemon"), true, "failed");
        }
        self.update(id, |live| {
            live.visits.retain(|v| v.n != n);
            let step = live.doc.step.as_mut()?;
            match visit.role {
                Role::Main(None) => {
                    step.main = Some(ended.clone());
                    step.row = row.id;
                }
                Role::Score(None, true) => step.before = score,
                Role::Score(None, false) => step.after = score,
                Role::Main(Some(i)) => {
                    if let Some(item) = step.items.get_mut(i) {
                        item.ended = Some(ended.clone());
                        item.row = row.id;
                        item.head = end.clone();
                    }
                }
                Role::Score(Some(i), _) => {
                    if let Some(item) = step.items.get_mut(i) {
                        item.score = score;
                    }
                }
            }
            None
        });
        self.pump(id);
    }

    fn patch_row(&self, id: &str, row: i64, doc: Value) {
        if row == 0 {
            return;
        }
        let Some(mut found) = self.db.stages(id).ok().and_then(|s| s.into_iter().find(|s| s.id == row)) else { return };
        found.doc = doc;
        let _ = self.db.put_stage(&found);
    }

    fn finish_single(self: &Rc<Self>, id: &str) {
        let Some((step, stage, doc, row)) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| {
            let step = l.doc.step.clone()?;
            Some((step.clone(), l.pipeline.stage(&step.stage).cloned()?, l.doc.clone(), l.row.clone()))
        }) else {
            return;
        };
        let Some(mut ended) = step.main.clone() else { return };
        if let (Some(guard), Some(before), Some(after)) = (stage.agent().and_then(|a| a.guard.clone()), step.before, step.after) {
            let rolled = after < before;
            if rolled {
                let worker = Path::new(&doc.worker);
                let kept = format!("{}-guard-{}", row.branch, step.visit);
                let _ = git(worker, &["branch", "-f", &kept, "HEAD"]);
                let _ = git(worker, &["reset", "--hard", &step.start]);
                let note = format!("rolled back: {guard} scored {before} before {} and {after} after, so its work is set aside on {kept}", step.stage);
                log!("kanna-daemon: {id}: {note}");
                ended.note = Some(note);
            }
            self.patch_row(id, step.row, json!({ "guard": { "stage": guard, "before": before, "after": after, "rolledBack": rolled } }));
        }
        self.advance(id, &step, ended);
    }

    fn finish_items(self: &Rc<Self>, id: &str) {
        let Some((step, stage, doc, row)) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| {
            let step = l.doc.step.clone()?;
            Some((step.clone(), l.pipeline.stage(&step.stage).cloned()?, l.doc.clone(), l.row.clone()))
        }) else {
            return;
        };
        let repo = PathBuf::from(&row.repo);
        if step.attempts {
            let Some(won) = winner(&step.items) else { return self.advance(id, &step, Ended { verdict: None, shown: "done", answer: String::new(), feedback: String::new(), full: String::new(), failures: None, note: None, by: None }) };
            let best = &step.items[won];
            if let Err(error) = git(Path::new(&doc.worker), &["merge", "--ff-only", &best.branch]) {
                return self.stop(id, "stopped", &format!("can't take {}'s work onto {}: {error}", best.label(&step.stage), row.branch), Some("daemon"), true);
            }
            for (i, item) in step.items.iter().enumerate() {
                self.patch_row(id, item.row, json!({ "score": item.score, "won": i == won }));
                remove_worker(&repo, &item.worktree, Some(&item.branch));
            }
            let scores: Vec<String> = step.items.iter().map(|i| format!("{} {}", i.name, i.score.unwrap_or(0.0))).collect();
            let mut ended = best.ended.clone().unwrap_or(Ended { verdict: None, shown: "done", answer: String::new(), feedback: String::new(), full: String::new(), failures: None, note: None, by: None });
            ended.note = Some(format!("{} won: {}", best.name, scores.join(", ")));
            log!("kanna-daemon: {id}: {} kept {}", step.stage, ended.note.as_deref().unwrap_or(""));
            return self.advance(id, &step, ended);
        }
        let ended: Vec<(&Item, Ended)> = step.items.iter().filter_map(|i| Some((i, i.ended.clone()?))).collect();
        let failed: Vec<&(&Item, Ended)> = ended.iter().filter(|(_, e)| e.verdict == Some(Verdict::Fail)).collect();
        let verdict = stage.verdicts().then_some(if failed.is_empty() { Verdict::Pass } else { Verdict::Fail });
        let answer = ended.iter().map(|(i, e)| format!("## {}\n\n{}", i.label(&step.stage), e.answer.trim_end())).collect::<Vec<_>>().join("\n\n");
        let feedback = failed.iter().map(|(i, e)| format!("## {}\n\n{}", i.label(&step.stage), e.feedback.trim_end())).collect::<Vec<_>>().join("\n\n");
        let parts: Vec<Item> = step.items.iter().map(|i| Item { name: i.name.clone(), worktree: i.worktree.clone(), branch: i.branch.clone(), row: i.row, head: i.head.clone(), ..Item::default() }).collect();
        let name = step.stage.clone();
        self.update(id, |live| {
            live.doc.parts.insert(name, parts);
            None
        });
        let ended = Ended { verdict, shown: verdict.map_or("done", Verdict::name), answer, feedback, full: String::new(), failures: None, note: Some(format!("{} instances", step.items.len())), by: None };
        self.advance(id, &step, ended);
    }

    fn join_stage(self: &Rc<Self>, id: &str, name: &str, from: &str, visit: i64, head: &str) {
        let Some((doc, row)) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| (l.doc.clone(), l.row.clone())) else { return };
        let parts = doc.parts.get(from).cloned().unwrap_or_default();
        let worker = Path::new(&doc.worker);
        let started = now();
        let result = join(worker, from, &parts);
        let joined = result.as_ref().map(|j| j.joined.clone()).unwrap_or_default();
        let repo = PathBuf::from(&row.repo);
        for part in parts.iter().filter(|p| joined.contains(&p.name)) {
            remove_worker(&repo, &part.worktree, Some(&part.branch));
        }
        let left: Vec<Item> = parts.iter().filter(|p| !joined.contains(&p.name)).cloned().collect();
        let (verdict, feedback, conflicts) = match &result {
            Ok(Joined { conflict: Some((part, files)), .. }) => {
                let text = format!("Merging {from} · {part} into {} conflicts in:\n\n{}\n\nThe merge is left in progress in the run's worktree: resolve those files and commit the merge.", row.branch, files.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n"));
                (Verdict::Fail, text, files.clone())
            }
            Ok(_) => (Verdict::Pass, format!("joined {}", if joined.is_empty() { "nothing".to_string() } else { joined.join(", ") }), vec![]),
            Err(error) => (Verdict::Fail, error.clone(), vec![]),
        };
        let end = git(worker, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let stage_row = StageRow {
            id: 0,
            run: id.to_string(),
            stage: name.to_string(),
            visit,
            instance: None,
            kind: "merge".into(),
            agent: None,
            machine: self.host.clone(),
            started_at: started,
            ended_at: Some(now()),
            verdict: Some(verdict.name().into()),
            session: None,
            terminal: None,
            worktree: Some(doc.worker.clone()),
            branch: Some(row.branch.clone()),
            doc: json!({ "start": head, "end": end, "joined": joined, "conflicts": conflicts, "feedback": feedback }),
        };
        let _ = self.db.put_stage(&stage_row);
        log!("kanna-daemon: {id}: {name} joins {from}: {}", feedback.lines().next().unwrap_or(""));
        let step = Step { stage: name.to_string(), visit, start: head.to_string(), ..Step::default() };
        let (from, event) = (from.to_string(), json!({ "stage": name, "visit": visit, "kind": "merge", "joined": joined }));
        self.update(id, |live| {
            live.doc.starts.entry(step.stage.clone()).or_insert(step.start.clone());
            live.doc.parts.insert(from, left);
            live.doc.next = None;
            Some(("stage", Some("daemon".into()), event))
        });
        let ended = Ended { verdict: Some(verdict), shown: verdict.name(), answer: String::new(), feedback, full: String::new(), failures: Some(conflicts), note: None, by: None };
        self.advance(id, &step, ended);
    }

    fn advance(self: &Rc<Self>, id: &str, step: &Step, ended: Ended) {
        let Some((mut pipeline, mut doc, branch)) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| (l.pipeline.clone(), l.doc.clone(), l.row.branch.clone())) else { return };
        let stage = step.stage.clone();
        let agent = pipeline.stage(&stage).is_some_and(|s| s.agent().is_some());
        if agent {
            doc.answers.insert(stage.clone(), ended.answer.clone());
            let found = outputs(&ended.answer);
            for (key, value) in &found {
                doc.answers.insert(format!("{stage}.{key}"), value.clone());
            }
            let consumers: std::collections::BTreeSet<String> = pipeline.fanouts().iter().filter_map(|s| s.agent()?.each.clone()).filter(|e| e.stage == stage).map(|e| e.key).collect();
            if let Some(key) = consumers.first().filter(|key| consumers.len() == 1 && !found.iter().any(|(k, _)| k == *key)) {
                if let Some(body) = unnamed_output(&ended.answer) {
                    doc.answers.insert(format!("{stage}.{key}"), body);
                }
            }
        }
        let block = Some(&ended.answer).filter(|_| agent).and_then(|answer| kanna_pipe::kojo_block(answer));
        let emitted = block.map(|block| self.emit(id, &pipeline, &doc.worker, &stage, &block));
        match &emitted {
            Some(Ok((n, what))) => {
                if let Some(live) = self.live.borrow().iter().find(|l| l.row.id == id) {
                    (pipeline, doc.taken, doc.revision) = (live.pipeline.clone(), live.doc.taken.clone(), live.doc.revision);
                }
                self.patch_row(id, step.row, json!({ "revision": { "n": n, "summary": what } }));
            }
            Some(Err(errors)) => self.patch_row(id, step.row, json!({ "refused": errors })),
            None => {}
        }
        let worker = PathBuf::from(&doc.worker);
        let worker = worker.as_path();
        let end = git(worker, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let commits = if step.start.is_empty() || end.is_empty() { 0 } else { git(worker, &["rev-list", "--count", &format!("{}..{end}", step.start)]).ok().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0) };
        let next = decide(&pipeline, &mut doc, &stage, ended.verdict, ended.failures.clone());
        let arrow = match &next {
            Next::Stage { name, .. } => json!(name),
            Next::Done => json!(DONE),
            Next::Stuck => Value::Null,
        };
        log!("kanna-daemon: {id}: {stage} ended {} → {}", ended.shown, arrow.as_str().unwrap_or("no arrow"));
        let asked = pipeline.stage(&stage).is_some_and(|s| matches!(s.kind, Kind::Ask { .. }));
        doc.feedback = carried(&next, &ended, &stage, asked);
        if let (Some(Err(errors)), Next::Stage { .. }) = (&emitted, &next) {
            doc.feedback = Some(refusal(&stage, errors, doc.feedback.take()));
        }
        let event = json!({ "stage": stage, "visit": step.visit, "verdict": ended.shown, "next": arrow, "commits": commits, "note": opt(ended.note.as_deref()) });
        if let Next::Stage { name, reset: true } = &next {
            if let Some(start) = doc.starts.get(name).cloned() {
                let abandoned = format!("{branch}-reset-{}", doc.visits);
                let _ = git(worker, &["branch", &abandoned, "HEAD"]);
                let _ = git(worker, &["reset", "--hard", &start]);
            }
        }
        doc.next = match &next {
            Next::Stage { name, .. } => Some(name.clone()),
            _ => None,
        };
        doc.step = None;
        self.update(id, |live| {
            let (pausing, held_now, interventions) = (live.doc.pausing, live.doc.held, live.doc.interventions.clone());
            live.doc = doc.clone();
            live.doc.pausing = pausing;
            live.doc.held = held_now;
            live.doc.interventions = interventions;
            if let Next::Stage { name, .. } = &next {
                held(live, name);
            }
            Some(("verdict", Some("daemon".into()), event))
        });
        match next {
            Next::Stage { .. } => self.soon.set(true),
            Next::Done => self.hand_back(id),
            Next::Stuck if doc.owner.is_some() && ended.verdict == Some(Verdict::Fail) => {
                self.stop_as(id, "stopped", &format!("{stage} failed"), Some("daemon"), true, "failed");
            }
            Next::Stuck => {
                let why = format!("stuck: no arrow leaves {stage} after {}", ended.shown);
                self.stop(id, "stopped", &why, Some("daemon"), true);
            }
        }
    }

    fn wait(self: &Rc<Self>, id: &str, why: &str) {
        log!("kanna-daemon: {id} waits: {why}");
        self.update(id, |live| {
            live.row.state = "waiting".into();
            live.doc.why = Some(why.to_string());
            Some(("waiting", Some("daemon".into()), json!({ "why": why })))
        });
        self.tell(id, &format!("waiting: {why}. `kanna run show {id}` says where it is; `kanna run stop {id}` ends it."));
    }

    fn stop(self: &Rc<Self>, id: &str, state: &str, why: &str, by: Option<&str>, tell: bool) {
        let outcome = if why.starts_with("stuck") { "stuck" } else { "stopped" };
        self.stop_as(id, state, why, by, tell, outcome);
    }

    fn stop_as(self: &Rc<Self>, id: &str, state: &str, why: &str, by: Option<&str>, tell: bool, outcome: &str) {
        let visits = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.visits.clone()).unwrap_or_default();
        for visit in &visits {
            self.drop_job(visit);
            let mut row = visit.row.clone();
            if row.id != 0 {
                row.ended_at = Some(now());
                row.verdict = Some("stopped".into());
                let _ = self.db.put_stage(&row);
            }
            if let (Some(pane), false) = (row.terminal.clone(), visit.ending || visit.paused) {
                match (&visit.work, self.tasks.upgrade()) {
                    (Work::Agent { agent }, Some(tasks)) => {
                        let (this, closing) = (Rc::downgrade(self), pane.clone());
                        tasks.end_agent(&pane, agent, move |_| {
                            if let Some(this) = this.upgrade() {
                                this.close(&closing);
                            }
                        });
                    }
                    _ => self.close(&pane),
                }
            }
        }
        self.close_last(id);
        log!("kanna-daemon: {id} {state}: {why}");
        let (by, why_owned, state_owned, outcome) = (by.map(String::from), why.to_string(), state.to_string(), outcome.to_string());
        self.update(id, |live| {
            live.visits.clear();
            live.row.state = state_owned.clone();
            live.row.ended_at = Some(now());
            live.row.outcome = Some(outcome.clone());
            live.doc.why = Some(why_owned.clone());
            live.doc.next = None;
            Some(("stopped", by.clone(), json!({ "why": why_owned })))
        });
        if self.owned(id) {
            return self.ended(id, Err(why.to_string()));
        }
        if tell {
            let branch = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.branch.clone()).unwrap_or_default();
            self.tell(id, &format!("{why}. The run's branch {branch} is kept for you to look at; `kanna run show {id}` has its visits."));
        }
    }

    fn workers(&self, id: &str) -> Vec<String> {
        let live = self.live.borrow();
        let Some(live) = live.iter().find(|l| l.row.id == id) else { return vec![] };
        let items = live.doc.step.iter().flat_map(|s| s.items.iter());
        let parts = live.doc.parts.values().flatten();
        items.chain(parts).map(|i| i.worktree.clone()).filter(|w| !w.is_empty()).collect()
    }

    fn owned(&self, id: &str) -> bool {
        self.live.borrow().iter().any(|l| l.row.id == id && l.doc.owner.is_some())
    }

    fn ended(&self, id: &str, outcome: Result<String, String>) {
        if let Some(on_end) = self.on_end.borrow().as_ref() {
            on_end(id, outcome);
        }
    }

    pub fn stop_owned(self: &Rc<Self>, id: &str, why: &str) {
        let found = self.live.borrow().iter().find(|l| l.row.id == id && LIVE.contains(&l.row.state.as_str())).map(|l| (l.row.repo.clone(), l.doc.worker.clone(), l.visits.iter().filter_map(|v| v.row.terminal.clone()).collect::<Vec<_>>()));
        let Some((repo, worker, terminals)) = found else { return };
        self.close_run(id, &repo, &worker, terminals, why);
    }

    pub fn task_closed(self: &Rc<Self>, task: &str) {
        let found: Vec<(String, String, String, Vec<String>)> = self
            .live
            .borrow()
            .iter()
            .filter(|l| l.row.task == task && l.doc.owner.is_none() && LIVE.contains(&l.row.state.as_str()))
            .map(|l| (l.row.id.clone(), l.row.repo.clone(), l.doc.worker.clone(), l.visits.iter().filter_map(|v| v.row.terminal.clone()).collect()))
            .collect();
        for (id, repo, worker, terminals) in found {
            self.close_run(&id, &repo, &worker, terminals, "the task was closed");
        }
    }

    fn close_run(self: &Rc<Self>, id: &str, repo: &str, worker: &str, terminals: Vec<String>, why: &str) {
        let workers = self.workers(id);
        self.update(id, |live| {
            for visit in live.visits.iter_mut() {
                visit.ending = true;
            }
            None
        });
        for pane in terminals {
            self.close(&pane);
        }
        self.stop(id, "stopped", why, Some("daemon"), false);
        let owned = self.owned(id);
        for worktree in workers.iter().map(String::as_str).chain([worker].into_iter().filter(|_| !owned)) {
            remove_worker(Path::new(repo), worktree, None);
        }
    }

    fn hand_back(self: &Rc<Self>, id: &str) {
        let Some((row, doc)) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| (l.row.clone(), l.doc.clone())) else { return };
        let branch = &row.branch;
        let note = format!("{branch} is ready: take it with `git merge --ff-only {branch}` in your worktree, or look first with `git log HEAD..{branch}` and `git diff HEAD...{branch}`");
        for worktree in self.workers(id) {
            remove_worker(Path::new(&row.repo), &worktree, None);
        }
        let worker = Path::new(&doc.worker);
        if worker.is_dir() && doc.owner.is_none() {
            if let Err(error) = git(Path::new(&row.repo), &["worktree", "remove", &doc.worker]) {
                log!("kanna-daemon: {id}'s worktree {} is kept: {error}", doc.worker);
            }
        }
        self.close_last(id);
        let stages = self.db.stages(id).unwrap_or_default();
        let distinct: std::collections::BTreeSet<&str> = stages.iter().map(|s| s.stage.as_str()).collect();
        let took = clock(Duration::from_secs_f64((now() - row.started_at).max(0.0)));
        let summary = format!("{} visits of {} stages, {took}, {} sessions", stages.len(), distinct.len(), doc.sessions);
        log!("kanna-daemon: {id} handed back: {summary}; {note}");
        let note_saved = note.clone();
        self.update(id, |live| {
            live.row.state = "done".into();
            live.row.ended_at = Some(now());
            live.row.outcome = Some("handed back".into());
            live.doc.note = Some(note_saved.clone());
            None
        });
        let _ = self.db.run_event(id, "handed_back", Some("daemon"), &json!({ "note": note }));
        if doc.owner.is_some() {
            let head = git(Path::new(&row.repo), &["rev-parse", "--verify", &format!("refs/heads/{branch}")]).map_err(|e| format!("{branch} is gone: {e}"));
            return self.ended(id, head);
        }
        self.tell(id, &format!("handed back: {summary}. {note}. `kanna run show {id}` has its visits."));
    }

    fn context(&self, id: &str, worker: &str) -> Context {
        let kept = self.db.revisions(id).ok().and_then(|r| r.last().and_then(|r| r.doc.get("prompts").cloned())).unwrap_or(Value::Null);
        let mut prompts: BTreeMap<String, String> = kept.as_object().map_or_else(BTreeMap::new, |m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect());
        for file in fs::read_dir(Path::new(worker).join(".kanna/prompts")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "md")) {
            if let (Some(name), Ok(text)) = (file.file_stem().map(|s| s.to_string_lossy().into_owned()), fs::read_to_string(&file)) {
                prompts.entry(name).or_insert(text);
            }
        }
        Context { prompts, machine: Some(self.host.clone()), ..Context::default() }
    }

    fn adopt(&self, id: &str, by: &str, revision: kanna_pipe::Revision, context: &Context, more: Value) -> Result<(i64, String), String> {
        let Some((before, file, worker)) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| (l.pipeline.clone(), l.doc.file.clone(), l.doc.worker.clone())) else { return Err(format!("{id} has ended")) };
        let after = revision.pipeline.clone();
        let (hash, what) = (hash(&after.text), changes(&before, &after));
        let lines = kanna_pipe::diff(&before.text, &after.text);
        let count = |sign: char| lines.iter().filter(|c| c.sign == sign).count();
        let diff: Vec<Value> = lines.iter().map(|c| json!([c.sign.to_string(), c.line])).collect();
        let mut doc = json!({ "text": after.text, "file": file, "prompts": context.prompts, "diff": diff, "summary": what, "added": count('+'), "removed": count('-'), "worst": after.max_sessions(&Sizes::new()) });
        patch(&mut doc, more);
        let n = self.db.add_revision(id, by, &hash, &doc).map_err(|e| format!("can't save the revision: {e}"))?;
        let yours = !by.starts_with("stage:");
        let event = json!({ "n": n, "hash": hash, "summary": what });
        self.update(id, |live| {
            live.doc.taken = revision.taken(&live.doc.taken);
            live.doc.revision = n;
            live.row.revision_hash = hash.clone();
            live.row.doc["budget"] = budget_value(&after);
            live.pipeline = after.clone();
            for visit in live.visits.iter_mut() {
                visit.within = after.stage(&visit.row.stage).and_then(|s| s.within);
            }
            if yours {
                intervene(live, "revision");
            }
            Some(("revision", Some(by.to_string()), event))
        });
        let draft = draft_path(&worker);
        if is_plain_file(&draft) && fs::read_to_string(&draft).is_ok_and(|text| text == before.text) {
            let _ = write_draft(&worker, &after.text);
        }
        log!("kanna-daemon: {id} takes revision {n} by {by}: {what}");
        Ok((n, what))
    }

    fn refuse(&self, id: &str, by: &str, errors: &[String], more: Value) {
        log!("kanna-daemon: {id} refuses a revision by {by}: {}", errors.join("; "));
        let mut event = json!({ "errors": errors });
        patch(&mut event, more);
        self.update(id, |_| Some(("refused", Some(by.to_string()), event)));
    }

    fn emit(&self, id: &str, pipeline: &Pipeline, worker: &str, stage: &str, block: &str) -> Result<(i64, String), Vec<String>> {
        let by = format!("stage:{stage}");
        let context = self.context(id, worker);
        let adopted = emit_errors(pipeline, stage, block, &context).and_then(|revision| self.adopt(id, &by, revision, &context, json!({ "block": block })).map_err(|error| vec![error]));
        if let Err(errors) = &adopted {
            self.refuse(id, &by, errors, json!({ "stage": stage, "block": block }));
        }
        adopted
    }

    fn draft_pane(&self, draft: &str) -> Option<String> {
        let core = self.core.upgrade()?;
        core.with_panes(|panes| panes.iter().find(|p| p.kind == PaneType::Code && p.path.as_deref() == Some(draft) && !matches!(p.state, PaneState::Exited | PaneState::Failed)).map(|p| p.id.clone()))
    }

    fn edit(self: &Rc<Self>, args: &Value, responder: Responder) {
        let row = match self.find_live(args).and_then(|row| self.live_of(&row).map(|_| row)) {
            Ok(row) => row,
            Err(error) => return responder.reply(Err(error)),
        };
        if string(args, "text").is_some() {
            return responder.reply(self.apply_edit(&row, args));
        }
        let Some((text, worker, owned, n)) = self.live.borrow().iter().find(|l| l.row.id == row.id).map(|l| (l.pipeline.text.clone(), l.doc.worker.clone(), l.doc.owner.is_some(), l.doc.revision)) else { return responder.reply(Err(KmuxError::new("ended", format!("{} has ended", row.id)))) };
        let draft = draft_path(&worker);
        if !is_plain_file(&draft) {
            if let Err(error) = write_draft(&worker, &text) {
                return responder.reply(Err(failed(format!("can't write {}: {error}", draft.display()))));
            }
        }
        let file = draft.to_string_lossy().into_owned();
        let out = Map::from_iter([("run".to_string(), json!(row.id)), ("revision".to_string(), json!(n)), ("file".to_string(), json!(file))]);
        let Some(core) = self.core.upgrade() else { return responder.reply(Ok(out)) };
        let request = match (self.draft_pane(&file), owned) {
            (Some(pane), _) => json!({ "cmd": "focus", "args": { "pane": pane } }),
            (None, false) => json!({ "cmd": "open", "args": { "type": "code", "path": file, "workspace": row.task, "tab": true, "wait": false } }),
            (None, true) => return responder.reply(Ok(out)),
        };
        let shown = self.draft_pane(&file);
        core.handle(request, move |reply| {
            let mut out = out;
            let pane = shown.or_else(|| reply.get("pane").and_then(|p| string(p, "id")).map(String::from));
            match (reply.get("ok") == Some(&Value::Bool(true)), pane) {
                (true, Some(pane)) => out.insert("pane".to_string(), json!(pane)),
                _ => out.insert("unshown".to_string(), json!(reply.get("error").and_then(|e| string(e, "message")).unwrap_or("no pane opened"))),
            };
            responder.reply(Ok(out));
        });
    }

    fn apply_edit(self: &Rc<Self>, row: &RunRow, args: &Value) -> Result<Map, KmuxError> {
        let text = string(args, "text").unwrap_or("");
        let shown = string(args, "file").unwrap_or("the edit");
        let by = self.who(args, &row.task);
        let (state, doc, visit, _) = self.live_of(row)?;
        let current = self.pipeline_of(row).ok_or_else(|| failed(format!("{}'s pipeline doesn't parse any more", row.id)))?;
        if text == current.text {
            return Err(KmuxError::new("unchanged", format!("{shown} is {}'s revision {} as it is: nothing to apply", row.id, doc.revision)));
        }
        let at = visit.first().map(|v| v.row.stage.clone()).or_else(|| doc.step.as_ref().map(|s| s.stage.clone())).or_else(|| doc.next.clone());
        let context = self.context(&row.id, &doc.worker);
        let draft = draft_path(&doc.worker).to_string_lossy().into_owned();
        let pane = Some(shown).filter(|shown| *shown == draft).and_then(|_| self.draft_pane(&draft));
        let checked = edit_errors(&current, text, &context, at.as_deref());
        let adopted = checked.and_then(|revision| self.adopt(&row.id, &by, revision, &context, Value::Null).map_err(|error| vec![(1, error)]));
        let (n, what) = match adopted {
            Ok(adopted) => adopted,
            Err(errors) => {
                let lines: Vec<String> = errors.iter().map(|(line, message)| format!("{shown}:{line}: {message}")).collect();
                let name = Path::new(shown).file_name().map_or_else(|| shown.to_string(), |n| n.to_string_lossy().into_owned());
                let brief: Vec<String> = errors.iter().map(|(line, message)| format!("{name}:{line}: {message}")).collect();
                self.refuse(&row.id, &by, &brief, json!({ "text": text, "file": shown }));
                if let (Some(pane), Some(core)) = (pane, self.core.upgrade()) {
                    let line = errors.iter().map(|(line, _)| *line).min().unwrap_or(1).max(1);
                    let said = format!("Not applied: {} is still at revision {}.\n{}", row.id, doc.revision, errors.iter().map(|(line, message)| wrapped(&format!("line {line}: {message}"), 72)).collect::<Vec<_>>().join("\n"));
                    core.handle(json!({ "cmd": "comment", "args": { "path": draft, "line": line, "last": line, "text": said, "pane": pane } }), |reply| {
                        if let Some(error) = reply.get("error").and_then(|e| string(e, "message")) {
                            log!("kanna-daemon: couldn't show the edit's errors in its pane: {error}");
                        }
                    });
                }
                return Err(KmuxError::new("invalid", lines.join("\n")));
            }
        };
        if let (Some(pane), Some(core)) = (pane, self.core.upgrade()) {
            core.handle(json!({ "cmd": "close", "args": { "pane": pane } }), |_| {});
        }
        if state == "waiting" && visit.is_empty() && doc.step.is_none() {
            self.update(&row.id, |live| {
                live.row.state = "running".into();
                live.doc.why = None;
                Some(("resumed", Some(by.clone()), json!({ "next": live.doc.next, "why": format!("revision {n}") })))
            });
            self.soon.set(true);
        }
        let worst = self.pipeline_of(row).map(|p| p.max_sessions(&Sizes::new()));
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("revision".to_string(), json!(n)), ("by".to_string(), json!(by)), ("summary".to_string(), json!(what)), ("worst".to_string(), json!(worst))]))
    }

    fn pipeline_of(&self, row: &RunRow) -> Option<Pipeline> {
        if let Some(live) = self.live.borrow().iter().find(|l| l.row.id == row.id) {
            return Some(live.pipeline.clone());
        }
        let text = self.db.revisions(&row.id).ok()?.last().and_then(|r| string(&r.doc, "text").map(String::from))?;
        kanna_pipe::parse(&text).ok()
    }

    fn lines(&self, row: &RunRow) -> Option<(u64, u64, u64)> {
        let base = string(&row.doc, "base").filter(|b| !b.is_empty())?;
        let stat = git(Path::new(&row.repo), &["diff", "--shortstat", base, &row.branch]).ok()?;
        let number = |word: &str| stat.split(',').find(|part| part.contains(word)).and_then(|part| part.split_whitespace().next()?.parse::<u64>().ok()).unwrap_or(0);
        Some((number("insertion"), number("deletion"), number("file")))
    }

    pub fn notice(&self, id: &str) -> Result<Value, KmuxError> {
        let row = match self.live.borrow().iter().find(|l| l.row.id == id) {
            Some(live) => live.row.clone(),
            None => self.db.runs(None).map_err(failed)?.into_iter().find(|r| r.id == id).ok_or_else(|| KmuxError::new("not_found", format!("no run {id}")))?,
        };
        let pipeline = self.pipeline_of(&row).ok_or_else(|| failed(format!("{id}'s pipeline doesn't parse any more")))?;
        let stages = self.db.stages(id).map_err(failed)?;
        let events = self.db.run_events(id).map_err(failed)?;
        let revisions = self.db.revisions(id).map_err(failed)?;
        let task = self.tasks.upgrade().and_then(|t| t.entry(&row.task));
        let lines = self.lines(&row);
        Ok(run_notice::notice(&run_notice::Inputs {
            row: &row,
            pipeline: &pipeline,
            stages: &stages,
            events: &events,
            revisions: &revisions,
            task_name: task.as_ref().and_then(|t| t.name.as_deref()),
            lines,
            now: now(),
        }))
    }

    fn publish(&self, id: &str) {
        let views: Vec<String> = self.views.borrow().iter().filter(|(_, run)| run.as_str() == id).map(|(view, _)| view.clone()).collect();
        if views.is_empty() {
            return;
        }
        match self.notice(id) {
            Ok(notice) => {
                if let Some(on_run) = self.on_run.borrow().as_ref() {
                    on_run(&views, &notice);
                }
            }
            Err(error) => log!("kanna-daemon: can't tell the Run tab about {id}: {}", error.message),
        }
    }

    pub fn subscribe(&self, view: &str, args: &Value) -> Result<Map, KmuxError> {
        let run = string(args, "run").filter(|r| !r.is_empty()).ok_or_else(|| bad("say which run"))?;
        let notice = self.notice(run)?;
        self.views.borrow_mut().insert(view.to_string(), run.to_string());
        match notice {
            Value::Object(map) => Ok(map),
            _ => Ok(Map::new()),
        }
    }

    pub fn unsubscribe(&self, view: &str) {
        self.views.borrow_mut().remove(view);
    }

    pub fn leave_views(&self, gone: impl Fn(&str) -> bool) {
        self.views.borrow_mut().retain(|view, _| !gone(view));
    }

    pub fn stage_of(&self, task: &str) -> Option<String> {
        let live = self.live.borrow();
        let live = live.iter().rfind(|l| l.row.task == task && LIVE.contains(&l.row.state.as_str()))?;
        let stages = self.db.stages(&live.row.id).ok()?;
        let (stage, at, of) = run_notice::place(&live.pipeline, &live.row, &stages);
        Some(format!("{stage} {at}/{of}"))
    }

    fn prepare_open(&self, args: &mut Value) -> Result<(), KmuxError> {
        let row = self.find(args)?;
        args["run"] = json!(row.id);
        for key in ["by", "cwd"] {
            if let Some(map) = args.as_object_mut() {
                map.remove(key);
            }
        }
        let placed = ["window", "workspace", "from", "dock", "lead", "planner"].iter().any(|key| args.get(*key).is_some_and(|v| !v.is_null()));
        if !placed {
            args["workspace"] = json!(row.task);
        }
        if args.get("split").is_none() {
            args["tab"] = json!(true);
        }
        Ok(())
    }

    fn watch(self: &Rc<Self>, args: &Value, responder: Responder) {
        let terminal = match self.watched(args) {
            Ok(found) => found,
            Err(error) => return responder.reply(Err(error)),
        };
        let Some(core) = self.core.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
        let (run, terminal) = terminal;
        let by = string(args, "by").unwrap_or("user").to_string();
        let (this, focusing) = (Rc::downgrade(self), core.weak());
        core.handle(json!({ "cmd": "move", "args": { "pane": terminal, "tab": "new" } }), move |moved| {
            if moved.get("ok") != Some(&Value::Bool(true)) {
                let message = moved.get("error").and_then(|e| string(e, "message")).unwrap_or("it couldn't be moved").to_string();
                return responder.reply(Err(failed(format!("can't show {terminal} in a tab: {message}"))));
            }
            if let Some(this) = this.upgrade() {
                let _ = this.db.run_event(&run, "watched", Some(&by), &json!({ "terminal": terminal }));
                log!("kanna-daemon: {run} is watched in {terminal}");
                this.name_tab(&run, &terminal);
            }
            let mut out = Map::from_iter([("run".to_string(), json!(run)), ("terminal".to_string(), json!(terminal))]);
            for key in ["tab", "workspace", "window"] {
                if let Some(value) = moved.get(key) {
                    out.insert(key.to_string(), value.clone());
                }
            }
            let Some(core) = focusing.upgrade() else { return responder.reply(Ok(out)) };
            core.handle(json!({ "cmd": "focus", "args": { "pane": terminal } }), move |_| responder.reply(Ok(out)));
        });
    }

    fn name_tab(&self, id: &str, pane: &str) {
        let title = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visits.iter().find(|v| v.row.terminal.as_deref() == Some(pane)).or(l.visits.first()).map(|v| format!("{} · {}", v.label(), run_notice::who(&l.pipeline, &v.row))));
        let (Some(title), Some(core)) = (title, self.core.upgrade()) else { return };
        let (renaming, pane) = (core.weak(), pane.to_string());
        core.handle(json!({ "cmd": "list" }), move |list| {
            let tab = list.get("workspaces").and_then(Value::as_array).into_iter().flatten().flat_map(|ws| ws.get("tabs").and_then(Value::as_array).into_iter().flatten()).find(|t| t.get("panes").and_then(Value::as_array).is_some_and(|p| p.iter().any(|p| p == pane.as_str()))).and_then(|t| string(t, "id").map(String::from));
            if let (Some(tab), Some(core)) = (tab, renaming.upgrade()) {
                core.handle(json!({ "cmd": "rename-tab", "args": { "tab": tab, "title": title } }), |_| {});
            }
        });
    }

    fn watched(&self, args: &Value) -> Result<(String, String), KmuxError> {
        let row = self.find_live(args)?;
        let current: Vec<String> = self.live.borrow().iter().find(|l| l.row.id == row.id).map_or(vec![], |l| l.visits.iter().filter(|v| !v.paused && !v.ending).filter_map(|v| v.row.terminal.clone()).collect());
        let terminal = match string(args, "worker").filter(|w| !w.is_empty()) {
            Some(worker) if current.iter().any(|c| c == worker) => worker.to_string(),
            Some(worker) => return Err(KmuxError::new("not_a_worker", format!("{worker} isn't {}'s worker now{}", row.id, if current.is_empty() { String::new() } else { format!(": its workers are {}", current.join(", ")) }))),
            None => current.into_iter().next().ok_or_else(|| KmuxError::new("no_worker", format!("{} has no stage running now ({}), so there's no terminal to watch", row.id, row.state)))?,
        };
        Ok((row.id, terminal))
    }

    fn tell(&self, id: &str, text: &str) {
        if self.owned(id) {
            if let Some(on_tell) = self.on_tell.borrow().as_ref() {
                on_tell(id, text);
            }
            return;
        }
        let Some(row) = self.live.borrow().iter().find(|l| l.row.id == id && l.doc.owner.is_none()).map(|l| l.row.clone()) else { return };
        let Ok(tasks) = self.tasks() else { return };
        if let Err(error) = tasks.post(&row.task, Some(&row.repo), id, text) {
            log!("kanna-daemon: couldn't tell {} about {id}: {}", row.task, error.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipeline(text: &str) -> Pipeline {
        kanna_pipe::parse(text).unwrap_or_else(|e| panic!("{e:?}"))
    }

    #[test]
    fn a_stage_is_told_to_report_done_with_its_verdict_or_that_it_needs_you() {
        let p = pipeline("build = claude\nreview = claude verdict\nbuild -> review -> done\n");
        let build = prompt(p.stage("build").unwrap(), "Build it.", "Add a button", None, None, &BTreeMap::new(), ".kanna/tmp/answer-1.md");
        assert!(build.ends_with("# When you're done\n\nWrite your final answer to `.kanna/tmp/answer-1.md`, then run `\"$KANNA_CLI\" run done`. Kanna ends this session once your turn ends. If you need something from the user before you can finish, run `\"$KANNA_CLI\" run need \"WHAT YOU NEED\"` and end your turn: Kanna stops this session and resumes it with their answer.\n"), "{build}");
        let review = prompt(p.stage("review").unwrap(), "Review it.", "Add a button", None, None, &BTreeMap::new(), ".kanna/tmp/answer-2.md");
        assert!(review.contains("then run `\"$KANNA_CLI\" run done pass` if it passes, or `\"$KANNA_CLI\" run done fail` if it doesn't."), "{review}");
    }

    #[test]
    fn a_report_kept_on_the_visit_comes_back_after_a_restart() {
        let back = reported(&json!({ "verdict": "fail", "answer": "two tests fail" }));
        assert_eq!((back.verdict, back.shown, back.feedback.as_str()), (Some(Verdict::Fail), "fail", "two tests fail"));
        let back = reported(&json!({ "verdict": "done", "answer": "built" }));
        assert_eq!((back.verdict, back.shown), (None, "done"));
    }

    #[test]
    fn the_arrow_taken_is_the_first_that_holds_and_feedback_goes_to_a_fix_after_a_fail() {
        let p = pipeline("build = claude\ncheck = run make test\nfix = claude\nbuild -> check\ncheck -[fail, max 2]-> fix -> check\ncheck -> done\n");
        let mut doc = Doc::default();
        assert_eq!(decide(&p, &mut doc, "build", None, None), Next::Stage { name: "check".into(), reset: false });
        assert_eq!(decide(&p, &mut doc, "check", Some(Verdict::Fail), Some(vec!["FAILED a".into()])), Next::Stage { name: "fix".into(), reset: false });
        assert_eq!(decide(&p, &mut doc, "fix", None, None), Next::Stage { name: "check".into(), reset: false });
        assert_eq!(decide(&p, &mut doc, "check", Some(Verdict::Pass), Some(vec![])), Next::Done);
        let stage = p.stage("fix").unwrap();
        let text = prompt(stage, "Fix what failed.\n{{build.answer}}", "Add a button", None, Some(&Feedback { stage: "check".into(), text: "FAILED a".into(), full: String::new() }), &BTreeMap::from([("build".into(), "built it".into())]), "a.md");
        assert!(text.starts_with("Fix what failed.\nbuilt it\n\n# The task\n\nAdd a button\n\n# Feedback from check\n\nFAILED a\n\n# When you're done\n"), "{text}");
    }

    #[test]
    fn a_loop_ends_at_its_max_or_when_a_fix_changes_nothing() {
        let p = pipeline("check = run make test\nfix = claude\ncheck -[fail, max 3]-> fix -> check\ncheck -[pass]-> done\n");
        let mut doc = Doc::default();
        let mut fixes = 0;
        loop {
            match decide(&p, &mut doc, "check", Some(Verdict::Fail), Some(vec![format!("FAILED {fixes}")])) {
                Next::Stage { name, .. } if name == "fix" => {
                    fixes += 1;
                    assert_eq!(decide(&p, &mut doc, "fix", None, None), Next::Stage { name: "check".into(), reset: false });
                }
                Next::Stuck => break,
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(fixes, 3, "max 3 bounds the loop, and the run is stuck after it");
        let p = pipeline("check = run make test\nfix = claude\ncheck -[fail, max 5, progress]-> fix -> check\ncheck -[pass]-> done\n");
        let mut doc = Doc::default();
        let same = || Some(vec!["FAILED same".to_string()]);
        assert_eq!(decide(&p, &mut doc, "check", Some(Verdict::Fail), same()), Next::Stage { name: "fix".into(), reset: false });
        decide(&p, &mut doc, "fix", None, None);
        assert_eq!(decide(&p, &mut doc, "check", Some(Verdict::Fail), same()), Next::Stuck, "a fix that changed nothing ends the loop");
    }

    #[test]
    fn the_budget_stops_a_run_before_a_session_it_has_no_room_for() {
        let p = pipeline("budget = 4 sessions, 1h\nbuild = claude\nreview = claude verdict\nbuild -> review\nreview -[fail, max 1]-> build\nreview -> done\n");
        assert_eq!(p.max_sessions(&Sizes::new()), 4);
        let mut doc = Doc { sessions: 3, ..Doc::default() };
        let build = p.stage("build").unwrap();
        assert_eq!(over_budget(&p, &doc, build, &Sizes::new(), Duration::from_secs(60)), None);
        doc.sessions = 4;
        assert_eq!(over_budget(&p, &doc, build, &Sizes::new(), Duration::from_secs(60)).as_deref(), Some("over budget: build would be session 5 of 4"));
        doc.sessions = 0;
        assert_eq!(over_budget(&p, &doc, build, &Sizes::new(), Duration::from_secs(3600)).as_deref(), Some("over budget: 1h 00m of 1h 00m spent before build"));
    }

    #[test]
    fn a_command_s_feedback_is_its_tail_with_failure_lines_first() {
        let output = "\x1b[32mcompiling\x1b[0m\r\nok 1\nFAILED test_two\nok 3\ntest result: 1 failed\n";
        assert_eq!(command_feedback(output), "FAILED test_two\ntest result: 1 failed\ncompiling\nok 1\nok 3");
        assert_eq!(failure_lines(output), vec!["FAILED test_two", "test result: 1 failed"]);
        let long: String = (0..200).map(|i| format!("line {i}\n")).collect();
        assert!(command_feedback(&long).starts_with("line 120\n"), "the last 80 lines");
    }

    #[test]
    fn time_paused_doesn_t_count_against_within() {
        let row = StageRow { id: 1, run: "run-1".into(), stage: "s".into(), visit: 1, instance: None, kind: "agent".into(), agent: Some("claude".into()), machine: "here".into(), started_at: 100.0, ended_at: None, verdict: None, session: None, terminal: None, worktree: None, branch: None, doc: json!({ "n": 1 }) };
        let mut visit = Visit { n: 1, role: Role::Main(None), row, work: Work::Agent { agent: "claude".into() }, within: Some(Duration::from_secs(60)), opening: false, ending: false, reported: None, needs: None, paused: false };
        assert_eq!(visit.due(), Some(160.0));
        patch(&mut visit.row.doc, json!({ "paused": { "at": 120.0, "session": "s-1" } }));
        assert_eq!(visit.row.doc["paused"]["session"], "s-1");
        patch(&mut visit.row.doc, json!({ "paused": null, "waited": 30.0 }));
        assert_eq!(visit.row.doc, json!({ "n": 1, "waited": 30.0 }));
        assert_eq!(visit.due(), Some(190.0));
    }

    #[test]
    fn what_runs_can_t_do_yet_is_refused_by_line_and_on_takes_only_this_machine() {
        let p = pipeline("plan = claude\nbuild = claude each plan.parts on mbp\nok = ask \"Is the plan right?\"\njoin = merge build on here\nboth = claude each plan.parts x2 by check\ncheck = run make\nfix = claude guard check on mini\nplan -> ok -> build -> join -> both -> check -> fix -> done\n");
        assert_eq!(
            unbuilt(&p, "mbp"),
            vec![
                "5: both uses each with xN, which runs can't do yet",
                "7: fix is on mini: no such machine: stages run on this machine (here, or mbp) until machines are built",
            ]
        );
    }

    #[test]
    fn a_fan_out_s_items_are_named_by_their_name_or_first_line() {
        let items = parts(r#"["Part a\nmock: run true", {"name": "Part B", "file": "b.txt"}, "Part a", 7]"#).unwrap();
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["part-a", "part-b", "part-a-2", "4"]);
        assert_eq!(items[0].text, "Part a\nmock: run true");
        assert!(items[1].text.contains("\"file\": \"b.txt\""), "{}", items[1].text);
        assert_eq!(parts("{\"a\": 1}").unwrap_err(), "isn't a JSON list");
        assert!(parts("nope").unwrap_err().starts_with("isn't JSON"));
        let p = pipeline("build = claude\nbuild -> done\n");
        let text = prompt(p.stage("build").unwrap(), "Build your part.", "Add a button", Some("part a"), None, &BTreeMap::new(), "a.md");
        assert!(text.starts_with("Build your part.\n\n# The task\n\nAdd a button\n\n# Your part\n\npart a\n\n# When you're done"), "{text}");
    }

    #[test]
    fn a_scorer_s_last_line_is_its_score_else_its_verdict_and_ties_go_to_the_first() {
        assert_eq!(score_of("ran\nscore: 0.83\n\n", Some(Verdict::Fail)), 0.83);
        assert_eq!(score_of("\x1b[32mscore: 2\x1b[0m\r\n", None), 2.0);
        assert_eq!(score_of("all good", Some(Verdict::Pass)), 1.0);
        assert_eq!(score_of("score: high", Some(Verdict::Fail)), 0.0);
        let scored = |scores: &[f64]| scores.iter().map(|s| Item { score: Some(*s), ..Item::default() }).collect::<Vec<_>>();
        assert_eq!(winner(&scored(&[0.5, 0.9, 0.9])), Some(1));
        assert_eq!(winner(&scored(&[1.0, 1.0])), Some(0));
    }

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-runs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "e@e"], &["config", "user.name", "e"]] {
            git(&dir, args).unwrap();
        }
        fs::write(dir.join("shared.txt"), "base\n").unwrap();
        git(&dir, &["add", "-A"]).unwrap();
        git(&dir, &["commit", "-qm", "base"]).unwrap();
        dir
    }

    fn part(dir: &Path, name: &str, file: &str, text: &str) -> Item {
        git(dir, &["checkout", "-q", "-b", name, "main"]).unwrap();
        fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", "-A"]).unwrap();
        git(dir, &["commit", "-qm", name]).unwrap();
        git(dir, &["checkout", "-q", "main"]).unwrap();
        Item { name: name.into(), branch: name.into(), ..Item::default() }
    }

    #[test]
    fn a_join_merges_every_part_in_order_and_stops_at_a_conflict_with_its_files() {
        let dir = repo("join");
        let parts = [part(&dir, "a", "a.txt", "a\n"), part(&dir, "b", "b.txt", "b\n")];
        assert_eq!(join(&dir, "build", &parts).unwrap(), Joined { joined: vec!["a".into(), "b".into()], conflict: None });
        assert_eq!(git(&dir, &["log", "--format=%s", "-2"]).unwrap(), "Merge build · b\nMerge build · a");
        assert_eq!(join(&dir, "build", &parts).unwrap().joined, ["a", "b"], "parts already in are joined");
        let clash = [part(&dir, "c", "shared.txt", "c\n"), part(&dir, "d", "shared.txt", "d\n"), part(&dir, "e", "e.txt", "e\n")];
        let joined = join(&dir, "build", &clash).unwrap();
        assert_eq!(joined, Joined { joined: vec!["c".into()], conflict: Some(("d".into(), vec!["shared.txt".into()])) });
        assert!(join(&dir, "build", &clash).unwrap_err().starts_with("a merge is still in progress in the run's worktree"));
        fs::write(dir.join("shared.txt"), "c and d\n").unwrap();
        git(&dir, &["commit", "-qam", "resolved"]).unwrap();
        assert_eq!(join(&dir, "build", &clash).unwrap().joined, ["c", "d", "e"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stage_with_one_consumer_may_give_its_list_as_any_json_block() {
        assert_eq!(unnamed_output("Plan:\n```json\n[1]\n```\n```json name=x\n[2]\n```"), Some("[1]".to_string()));
        assert_eq!(outputs("```json\n[1]\n```"), vec![]);
    }

    #[test]
    fn a_step_round_trips_with_its_items_and_scores() {
        let ended = Ended { verdict: Some(Verdict::Fail), shown: "fail", answer: "a".into(), feedback: "f".into(), full: "/log".into(), failures: Some(vec!["x".into()]), note: Some("n".into()), by: Some("agent".into()) };
        let step = Step { stage: "build".into(), visit: 2, start: "abc".into(), attempts: true, items: vec![Item { name: "attempt-1".into(), worktree: "/w".into(), branch: "b".into(), row: 4, head: "h".into(), ended: Some(ended.clone()), score: Some(0.5), ..Item::default() }], before: Some(1.0), main: Some(ended), after: None, row: 3 };
        let doc = Doc { step: Some(step.clone()), parts: BTreeMap::from([("build".into(), step.items.clone())]), ..Doc::default() };
        assert_eq!(Doc::from_value(&doc.to_value()), doc);
        assert_eq!(Role::from_value(&Role::Score(Some(1), false).to_value()), Role::Score(Some(1), false));
        assert_eq!(Role::from_value(&Role::Main(None).to_value()), Role::Main(None));
    }

    #[test]
    fn a_stage_s_block_adds_to_the_pipeline_or_is_refused_with_why() {
        let p = pipeline("budget = 4 sessions\nplan = claude emits\ncheck = run true\nplan -> check\ncheck -> done\n");
        let context = Context { prompts: BTreeMap::from([("plan".to_string(), String::new())]), ..Context::default() };
        let revision = emit_errors(&p, "plan", "lint = run true\nplan -> lint -> check", &context).unwrap();
        assert_eq!(revision.pipeline.text, "budget = 4 sessions\nplan = claude emits\ncheck = run true\nlint = run true\nplan -> lint\nplan -> check\ncheck -> done\nlint -> check\n");
        assert_eq!(changes(&p, &revision.pipeline), "added lint · 2 arrows added");
        let mut doc = Doc { taken: vec![0, 0], ..Doc::default() };
        doc.taken = revision.taken(&doc.taken);
        assert_eq!(decide(&revision.pipeline, &mut doc, "plan", None, None), Next::Stage { name: "lint".into(), reset: false });
        assert_eq!(emit_errors(&p, "plan", "check = run false", &context).unwrap_err(), ["the kojo block:1: stage 'check' exists already; a kojo block can only add stages"]);
        assert_eq!(emit_errors(&p, "plan", "more = claude prompt plan resume plan\nplan -> more -> check", &context).unwrap_err(), ["the merged pipeline:4: more uses resume, which runs can't do yet"]);
        assert_eq!(emit_errors(&p, "check", "lint = run true", &context).unwrap_err(), ["the kojo block:1: stage 'check' has no emits, so its answer can't change the pipeline"]);
        let said = refusal("plan", &["the kojo block:1: no".to_string()], None);
        assert_eq!((said.stage.as_str(), said.text.as_str()), ("plan", "Kanna refused the kojo block in plan's answer, so the run's pipeline is as it was:\nthe kojo block:1: no"));
        let kept = refusal("plan", &["e".to_string()], Some(Feedback { stage: "plan".into(), text: "The plan.".into(), full: "/f".into() }));
        assert!(kept.text.ends_with("\n\nThe plan.") && kept.full == "/f");
    }

    #[test]
    fn an_edit_is_checked_whole_and_can_t_remove_the_stage_the_run_is_at() {
        let p = pipeline("budget = 4 sessions\nbuild = claude\ncheck = run true\nbuild -> check\ncheck -[fail, max 2]-> build\ncheck -> done\n");
        let context = Context { prompts: BTreeMap::from([("build".to_string(), String::new())]), ..Context::default() };
        let text = p.text.replace("max 2", "max 1").replace("budget = 4", "budget = 3");
        let revision = edit_errors(&p, &text, &context, Some("check")).unwrap();
        assert_eq!(revision.taken(&[2, 1, 1]), [2, 0, 1]);
        assert_eq!(changes(&p, &revision.pipeline), "1 arrow added · 1 arrow removed · changed the budget");
        assert_eq!(changes(&p, &pipeline(&format!("# why\n{}", p.text))), "comments and spacing only");
        assert_eq!(wrapped("line 8: this loop has no max", 16), "line 8: this\nloop has no max");
        assert_eq!(edit_errors(&p, "build = claude\nbuild -> done\n", &context, Some("check")).unwrap_err(), [(1, "the run is at check, so an edit can't remove it".to_string())]);
        assert_eq!(edit_errors(&p, &p.text.replace("max 2", "max 9"), &context, None).unwrap_err().len(), 1);
        assert_eq!(edit_errors(&p, "build = claude\nbuild -> nowhere\n", &context, None).unwrap_err()[0].0, 2);
        assert_eq!(edit_errors(&p, &p.text.replace("build = claude", "build = claude resume build"), &context, None).unwrap_err(), [(2, "build uses resume, which runs can't do yet".to_string())]);
    }

    #[test]
    fn a_draft_is_never_written_through_a_link() {
        let folder = std::env::temp_dir().join(format!("kanna-draft-{}-{}", std::process::id(), now()));
        let (worker, elsewhere) = (folder.join("worker"), folder.join("elsewhere"));
        fs::create_dir_all(worker.join(SCRATCH)).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("kept.txt"), "kept").unwrap();
        let (worker, draft) = (worker.to_string_lossy().into_owned(), draft_path(&worker.to_string_lossy()));
        std::os::unix::fs::symlink(elsewhere.join("kept.txt"), &draft).unwrap();
        assert!(!is_plain_file(&draft));
        write_draft(&worker, "a = run true\n").unwrap();
        assert_eq!(fs::read_to_string(elsewhere.join("kept.txt")).unwrap(), "kept");
        assert!(is_plain_file(&draft) && fs::read_to_string(&draft).unwrap() == "a = run true\n");
        fs::remove_dir_all(Path::new(&worker).join(".kanna")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, Path::new(&worker).join(".kanna")).unwrap();
        assert!(write_draft(&worker, "x").is_err());
        assert!(!elsewhere.join("tmp").exists());
        let _ = fs::remove_dir_all(&folder);
    }

    #[test]
    fn named_outputs_are_kept_by_their_key() {
        assert_eq!(outputs("Plan:\n```json name=parts\n[\"a\", \"b\"]\n```\nok"), vec![("parts".to_string(), "[\"a\", \"b\"]".to_string())]);
    }

    fn answered(verdict: Verdict, note: &str) -> Ended {
        Ended { verdict: Some(verdict), shown: verdict.name(), answer: note.into(), feedback: note.into(), full: String::new(), failures: None, note: None, by: Some("user".into()) }
    }

    #[test]
    fn an_ask_s_note_goes_to_the_next_stage_whichever_the_answer() {
        let p = pipeline("plan = claude\nok = ask \"Is the plan right?\"\nbuild = claude\nplan -> ok\nok -[fail, max 2]-> plan\nok -> build -> done\n");
        let mut doc = Doc::default();
        decide(&p, &mut doc, "plan", None, None);
        let next = decide(&p, &mut doc, "ok", Some(Verdict::Fail), None);
        assert_eq!(next, Next::Stage { name: "plan".into(), reset: false });
        assert_eq!(carried(&next, &answered(Verdict::Fail, "Split it in two."), "ok", true).map(|f| f.text).as_deref(), Some("Split it in two."));
        let next = decide(&p, &mut doc, "ok", Some(Verdict::Pass), None);
        assert_eq!(next, Next::Stage { name: "build".into(), reset: false });
        assert_eq!(carried(&next, &answered(Verdict::Pass, "Yes, but keep the old flag."), "ok", true).map(|f| f.stage).as_deref(), Some("ok"));
        assert_eq!(carried(&next, &answered(Verdict::Pass, ""), "ok", true), None, "a bare yes hands nothing on");
        assert_eq!(carried(&next, &answered(Verdict::Pass, "fine"), "check", false), None, "a pass from any other stage hands nothing on");
    }

    fn live(doc: Doc) -> Live {
        let row = RunRow { id: "run-1".into(), task: "task-1".into(), repo: "/r".into(), pipeline: "p".into(), revision_hash: String::new(), branch: "run/task-1-1".into(), state: "waiting".into(), started_at: 0.0, ended_at: None, outcome: None, doc: Value::Null };
        Live { row, doc, pipeline: pipeline("a = claude\n"), visits: vec![], last: None }
    }

    #[test]
    fn after_a_visit_the_run_runs_on_unless_you_paused_it_or_its_task_is_stopped() {
        let mut run = live(Doc::default());
        held(&mut run, "build");
        assert_eq!((run.row.state.as_str(), run.doc.why.as_deref()), ("running", None));
        let mut run = live(Doc { pausing: true, ..Doc::default() });
        held(&mut run, "build");
        assert_eq!((run.row.state.as_str(), run.doc.why.as_deref()), ("paused", Some("paused by you before build")));
        let mut run = live(Doc { pausing: true, held: true, ..Doc::default() });
        held(&mut run, "build");
        assert_eq!((run.row.state.as_str(), run.doc.why.as_deref()), ("paused", Some("the task is stopped")));
        intervene(&mut run, "paused");
        intervene(&mut run, "paused");
        intervene(&mut run, "skipped");
        assert_eq!(run.doc.interventions, BTreeMap::from([("paused".to_string(), 2), ("skipped".to_string(), 1)]));
    }

    #[test]
    fn a_stage_s_command_is_kept_on_its_visit_and_its_end_carries_the_agent_on() {
        let job = Job { k: 2, command: "cargo test --workspace".into(), log: "/w/.kanna/tmp/command-3-2.log".into(), exit: "/w/.kanna/tmp/command-3-2.exit".into(), terminal: Some("term-9".into()), started: 5.0, awaited: true, code: None };
        assert_eq!(Job::from_value(&job.to_value()), Some(job.clone()));
        let ended = Job { code: Some(101), awaited: false, ..job.clone() };
        assert_eq!(Job::from_value(&ended.to_value()), Some(ended));
        assert_eq!(Job::from_value(&json!({})), None);
        let row = StageRow { id: 1, run: "run-1".into(), stage: "judge".into(), visit: 1, instance: None, kind: "agent".into(), agent: Some("claude".into()), machine: "here".into(), started_at: 1.0, ended_at: None, verdict: None, session: None, terminal: None, worktree: None, branch: None, doc: json!({ "n": 3, "job": job.to_value() }) };
        let visit = Visit { n: 3, role: Role::Main(None), row, work: Work::Agent { agent: "claude".into() }, within: None, opening: false, ending: false, reported: None, needs: None, paused: true };
        assert_eq!(visit.job(), Some(job.clone()));
        assert_eq!(command_ended(&job, 101), "The command you asked Kanna to run ended while your turn was over, so Kanna carries you on.\n\n    cargo test --workspace\n\nIt exited 101, and all its output is in `/w/.kanna/tmp/command-3-2.log`. Carry on from there, and report when you're done.");
    }

    #[test]
    fn a_stage_whose_turn_ends_unreported_is_told_that_only_the_call_ends_it() {
        let said = unreported("judge", true);
        assert!(said.starts_with("Your turn ended, but judge isn't over: only `\"$KANNA_CLI\" run done pass` or `\"$KANNA_CLI\" run done fail` ends it"), "{said}");
        assert!(said.contains("run need \"WHAT YOU NEED\"") && said.contains("run command 'COMMAND'"), "{said}");
        assert!(unreported("build", false).contains("only `\"$KANNA_CLI\" run done` ends it"));
    }

    #[test]
    fn a_run_stage_has_no_time_limit_unless_it_says_within() {
        let p = pipeline("check = run make test\ne2e = run make e2e within 30m\ncheck -> e2e -> done\n");
        assert_eq!(command_timeout(p.stage("check").unwrap().within), 0, "0 is no alarm: tasks.timeout is for setup and teardown");
        assert_eq!(command_timeout(p.stage("e2e").unwrap().within), 1800);
        assert_eq!(command_timeout(Some(Duration::from_millis(200))), 1);
    }

    #[test]
    fn feedback_from_a_command_names_the_file_with_all_its_output() {
        let p = pipeline("check = run make test\nfix = claude\ncheck -[fail, max 2]-> fix -> check\ncheck -> done\n");
        let feedback = Feedback { stage: "check".into(), text: "FAILED a".into(), full: "/w/.kanna/tmp/stage-1.log".into() };
        let text = prompt(p.stage("fix").unwrap(), "Fix it.", "Add a button", None, Some(&feedback), &BTreeMap::new(), "a.md");
        assert!(text.contains("# Feedback from check\n\nFAILED a\n\nAll of its output is in `/w/.kanna/tmp/stage-1.log`.\n\n# When you're done\n"), "{text}");
    }

    #[test]
    fn a_run_s_doc_round_trips() {
        let doc = Doc { request: "r".into(), taken: vec![1, 0], feedback: Some(Feedback { stage: "check".into(), text: "t".into(), full: "/f".into() }), failures: BTreeMap::from([("check".into(), vec!["x".into()])]), sessions: 2, next: Some("fix".into()), revision: 1, pausing: true, held: true, interventions: BTreeMap::from([("answered".into(), 1)]), ..Doc::default() };
        assert_eq!(Doc::from_value(&doc.to_value()), doc);
    }
}
