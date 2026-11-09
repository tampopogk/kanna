use crate::db::{Db, RunRow, StageRow};
use crate::pipelines;
use crate::task_scripts;
use crate::tasks::{self, git, Entry, Tasks};
use crate::usage;
use crate::runtime;
use kanna_mux::json::{json, num, opt, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, PaneState, Responder, WeakCore};
use kanna_pipe::{Context, Kind, Pipeline, Sizes, Stage, Verdict, DONE};
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

pub const LIVE: &[&str] = &["running", "waiting", "paused"];

pub const QUEUE: &str = "queue";

pub type OnEnd = Box<dyn Fn(&str, Result<String, String>)>;

#[derive(Clone, Debug, PartialEq)]
pub struct Feedback {
    pub stage: String,
    pub text: String,
    pub full: String,
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

pub fn over_budget(pipeline: &Pipeline, doc: &Doc, stage: &Stage, elapsed: Duration) -> Option<String> {
    let budget = pipeline.budget.as_ref()?;
    let cost = stage.sessions(&Sizes::new());
    if let Some(max) = budget.sessions.filter(|max| cost > 0 && doc.sessions + cost > *max) {
        return Some(format!("over budget: {} would be session {} of {max}", stage.name, doc.sessions + cost));
    }
    if let Some(time) = budget.time.filter(|time| elapsed >= *time) {
        return Some(format!("over budget: {} of {} spent before {}", clock(elapsed), clock(time), stage.name));
    }
    None
}

pub fn unbuilt(pipeline: &Pipeline) -> Vec<String> {
    let mut out = vec![];
    for stage in &pipeline.stages {
        let line = stage.line;
        let mut say = |what: &str| out.push(format!("{line}: {} uses {what}, which runs can't do yet", stage.name));
        match &stage.kind {
            Kind::Agent(agent) => {
                if agent.each.is_some() {
                    say("each");
                }
                if agent.attempts.is_some() {
                    say("xN by");
                }
                if agent.guard.is_some() {
                    say("guard");
                }
                if agent.emits {
                    say("emits");
                }
                if agent.resume.is_some() {
                    say("resume");
                }
            }
            Kind::Merge(_) => say("merge STAGE"),
            _ => {}
        }
        if stage.on.as_deref().is_some_and(|on| on != "here") {
            say("on (stages run here until machines are built)");
        }
    }
    out
}

pub fn prompt(stage: &Stage, file: &str, request: &str, feedback: Option<&Feedback>, answers: &BTreeMap<String, String>, answer: &str) -> String {
    let named = regex::Regex::new(r"\{\{([a-z][a-z0-9_]*)\.([A-Za-z0-9_]+)\}\}").expect("pattern");
    let mut out = named
        .replace_all(file, |c: &regex::Captures| {
            let key = if &c[2] == "answer" { c[1].to_string() } else { format!("{}.{}", &c[1], &c[2]) };
            answers.get(&key).cloned().unwrap_or_default()
        })
        .trim_end()
        .to_string();
    out += &format!("\n\n# The task\n\n{}\n", request.trim());
    if let Some(feedback) = feedback {
        out += &format!("\n# Feedback from {}\n\n{}\n", feedback.stage, feedback.text.trim_end());
    }
    let done = if stage.verdicts() { "`\"$KANNA_CLI\" run done pass` if it passes, or `\"$KANNA_CLI\" run done fail` if it doesn't" } else { "`\"$KANNA_CLI\" run done`" };
    out += &format!("\n# When you're done\n\nWrite your final answer to `{answer}`, then run {done}. Kanna ends this session once your turn ends. If you need something from the user before you can finish, run `\"$KANNA_CLI\" run need \"WHAT YOU NEED\"` and end your turn: Kanna stops this session and resumes it with their answer.\n");
    out
}

pub fn outputs(answer: &str) -> Vec<(String, String)> {
    let mut found = vec![];
    let mut lines = answer.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(info) = trimmed.strip_prefix("```") else { continue };
        let mut words = info.split_whitespace();
        if words.next() != Some("json") {
            continue;
        }
        let Some(key) = words.find_map(|w| w.strip_prefix("name=")) else { continue };
        let body: Vec<&str> = lines.by_ref().take_while(|l| !l.trim_start().starts_with("```")).collect();
        found.push((key.to_string(), body.join("\n")));
    }
    found
}

fn plain(text: &str) -> String {
    let escapes = regex::Regex::new(r"\x1b(\[[0-9;?]*[ -/]*[@-~]|\][^\x07\x1b]*(\x07|\x1b\\)|[@-Z\\-_])").expect("pattern");
    escapes.replace_all(text, "").replace('\r', "")
}

pub fn failure_lines(output: &str) -> Vec<String> {
    let text = plain(output);
    let lines: Vec<&str> = text.lines().collect();
    let tail = &lines[lines.len().saturating_sub(FEEDBACK_LINES)..];
    tail.iter().filter(|l| FAILURE_MARKS.iter().any(|m| l.contains(m))).map(|l| l.to_string()).collect()
}

pub fn failing_tests(lines: &[String]) -> usize {
    let summary = regex::Regex::new(r"^\s*(test result:|failures:|error: test failed|\d+ (passed|failed)|=+ .* =+$)|\b\d+ (passed|failed)\b").expect("pattern");
    lines.iter().filter(|l| !summary.is_match(l)).count()
}

pub fn findings(answer: &str) -> Option<BTreeMap<String, u64>> {
    let (_, body) = outputs(answer).into_iter().find(|(key, _)| key == "findings")?;
    let list: Value = serde_json::from_str(&body).ok()?;
    let mut by = BTreeMap::new();
    for finding in list.as_array()? {
        *by.entry(string(finding, "severity").unwrap_or("unrated").to_string()).or_insert(0) += 1;
    }
    Some(by)
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

fn alive(core: &CoreHandle, pane: &str) -> bool {
    core.pane(pane).is_some_and(|p| matches!(p.state, PaneState::Starting | PaneState::Running))
}

#[derive(Clone, Debug, PartialEq)]
enum Work {
    Agent { agent: String },
    Ask { question: String },
    Command { log: PathBuf, exit: PathBuf },
}

#[derive(Clone, Debug)]
struct Visit {
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

struct Measured {
    row: i64,
    session: Option<String>,
    doc: Value,
}

struct Measuring {
    row: i64,
    agent: Option<String>,
    session: Option<String>,
    folder: Option<String>,
    repo: String,
    branch: Option<String>,
    start: Option<String>,
    end: Option<String>,
    since: f64,
}

impl Measuring {
    fn of(row: &StageRow, repo: &str, session: Option<String>, end: Option<String>) -> Measuring {
        let paused = row.doc.get("paused").and_then(|p| string(p, "session")).map(String::from);
        Measuring {
            row: row.id,
            agent: row.agent.clone(),
            session: row.session.clone().or(session).or(paused),
            folder: row.worktree.clone(),
            repo: repo.to_string(),
            branch: row.branch.clone(),
            start: string(&row.doc, "start").map(String::from),
            end: end.or_else(|| string(&row.doc, "end").map(String::from)).filter(|e| !e.is_empty()),
            since: row.started_at,
        }
    }

    fn session(&self) -> Option<String> {
        let started = UNIX_EPOCH + Duration::from_secs_f64(self.since.max(0.0));
        self.session.clone().or_else(|| tasks::first_session_since(self.agent.as_deref()?, Path::new(self.folder.as_deref()?), started))
    }

    fn take(&self) -> Measured {
        let repo = Path::new(&self.repo);
        let session = self.session();
        let usage = self.agent.as_deref().zip(session.as_deref()).and_then(|(agent, session)| usage::of(agent, session, self.since));
        let mut doc = json!({ "measured": true, "usage": usage.map_or(Value::Null, |u| u.doc(&tasks::pricing(&tasks::config_files(repo)))) });
        let end = self.end.clone().or_else(|| git(repo, &["rev-parse", "--verify", self.branch.as_deref()?]).ok());
        if let Some((start, end)) = self.start.as_deref().filter(|s| !s.is_empty()).zip(end) {
            let span = format!("{start}..{end}");
            if let Ok(numstat) = git(repo, &["diff", "--numstat", "--no-renames", &span]) {
                let rows: Vec<Vec<&str>> = numstat.lines().map(|l| l.splitn(3, '\t').collect()).collect();
                let sum = |column: usize| rows.iter().filter_map(|r| r.get(column)?.parse::<u64>().ok()).sum::<u64>();
                doc["files"] = json!(rows.len());
                doc["added"] = json!(sum(0));
                doc["deleted"] = json!(sum(1));
            }
            if let Some(commits) = git(repo, &["rev-list", "--count", &span]).ok().and_then(|n| n.trim().parse::<u64>().ok()) {
                doc["commits"] = json!(commits);
            }
        }
        Measured { row: self.row, session, doc }
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
    visit: Option<Visit>,
}

#[derive(Clone, Debug)]
struct Ended {
    verdict: Option<Verdict>,
    shown: &'static str,
    answer: String,
    feedback: String,
    full: String,
    failures: Option<Vec<String>>,
    note: Option<String>,
    by: Option<String>,
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
}

pub struct Runs {
    db: Rc<Db>,
    core: WeakCore,
    tasks: Weak<Tasks>,
    live: RefCell<Vec<Live>>,
    soon: Cell<bool>,
    host: String,
    pub on_end: RefCell<Option<OnEnd>>,
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
            let open = db.stages(&row.id).ok().and_then(|s| s.into_iter().rfind(|s| s.ended_at.is_none()));
            let visit = open.map(|row| {
                let work = match row.kind.as_str() {
                    "agent" => Work::Agent { agent: row.agent.clone().unwrap_or_default() },
                    "ask" => Work::Ask { question: string(&row.doc, "question").unwrap_or("").to_string() },
                    _ => Work::Command { log: PathBuf::from(string(&row.doc, "log").unwrap_or("")), exit: PathBuf::from(string(&row.doc, "exit").unwrap_or("")) },
                };
                let within = pipeline.stage(&row.stage).and_then(|s| s.within);
                let reported = row.doc.get("reported").filter(|r| r.is_object()).map(|r| (reported(r), now()));
                let paused = row.doc.get("paused").is_some_and(Value::is_object);
                Visit { row, work, within, opening: false, ending: false, reported, needs: None, paused }
            });
            let visit = visit.map(|mut v| {
                if row.state == "paused" && !v.paused && v.reported.is_none() {
                    v.needs = Some(now());
                }
                v
            });
            log!("kanna-daemon: {} carries on ({}, {})", row.id, row.state, visit.as_ref().map_or("between stages".to_string(), |v| format!("in {}", v.row.stage)));
            live.push(Live { row, doc, pipeline, visit });
        }
        let this = Runs { db, core, tasks, live: RefCell::new(live), soon: Cell::new(true), host: machine(), flagged: RefCell::default(), asks_changed: Cell::new(true), carrying: RefCell::default(), on_status: RefCell::default(), on_end: RefCell::default() };
        for (row, repo) in this.db.unmeasured().unwrap_or_default() {
            this.measure(Measuring::of(&row, &repo, None, None));
        }
        this
    }

    fn measure(&self, measuring: Measuring) {
        if measuring.row == 0 {
            return;
        }
        let db = self.db.clone();
        runtime::spawn("runs.measure", async move {
            let Some(measured) = runtime::off_loop(move || measuring.take()).await else { return };
            if let Err(error) = db.patch_stage(measured.row, measured.session.as_deref(), &measured.doc) {
                log!("kanna-daemon: couldn't save visit {}'s numbers: {error}", measured.row);
            }
        });
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let watching = Rc::downgrade(self);
        core.on_ended(Rc::new(move |pane| {
            let Some(this) = watching.upgrade() else { return };
            let ours = this.live.try_borrow().map_or(true, |live| live.iter().any(|l| l.visit.as_ref().is_some_and(|v| v.row.terminal.as_deref() == Some(pane))));
            if ours {
                this.soon.set(true);
            }
        }));
        let this = Rc::downgrade(self);
        core.add_command(
            "run",
            Rc::new(move |args, responder: Responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
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
            Some("resume") => self.resume_run(args),
            Some("answer") => self.answer(args),
            Some("pause") => self.pause_run(args),
            Some("skip") => self.skip(args),
            Some("retry") => self.retry(args),
            other => Err(bad(format!("unknown run action {other:?} (start, list, show, log, stop, resume, answer, pause, skip, retry, done or need)"))),
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
        let started = Started { task: task.id.clone(), repo: task.repo.clone(), worker: worker.clone(), branch: branch.clone(), base, text, shown, name: name.clone(), request, prompts, owner: None, by };
        let (id, worst) = self.launch(started, pipeline)?;
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
        let pipeline = kanna_pipe::load(text, &context).map_err(|errors| KmuxError::new("invalid", errors.iter().map(|e| e.at(shown)).collect::<Vec<_>>().join("\n")))?;
        let unbuilt = unbuilt(&pipeline);
        if !unbuilt.is_empty() {
            return Err(KmuxError::new("unsupported", unbuilt.iter().map(|u| format!("{shown}:{u}")).collect::<Vec<_>>().join("\n")));
        }
        Ok(pipeline)
    }

    pub fn start_owned(&self, started: Started) -> Result<String, KmuxError> {
        let pipeline = self.checked(&started.text, &started.shown, &started.prompts)?;
        self.launch(started, pipeline).map(|(id, _)| id)
    }

    fn launch(&self, started: Started, pipeline: Pipeline) -> Result<(String, u32), KmuxError> {
        let Started { task, repo, worker, branch, base, text, shown, name, request, prompts, owner, by } = started;
        let id = self.db.new_run_id().map_err(failed)?;
        let doc = Doc { request, file: shown.clone(), worker: worker.clone(), base: base.clone(), revision: 1, taken: vec![0; pipeline.arrows.len()], next: Some(pipeline.start.clone()), owner, ..Doc::default() };
        let budget = pipeline.budget.as_ref().map_or(Value::Null, |b| json!({ "sessions": b.sessions, "seconds": b.time.map(|t| t.as_secs()) }));
        let mut value = doc.to_value();
        value["budget"] = budget;
        let row = RunRow { id: id.clone(), task: task.clone(), repo, pipeline: name.clone(), revision_hash: hash(&text), branch: branch.clone(), state: "running".into(), started_at: now(), ended_at: None, outcome: None, doc: value };
        self.db.put_run(&row, Some(("started", Some(&by), &json!({ "pipeline": name, "branch": branch, "base": base })))).map_err(failed)?;
        self.db.add_revision(&id, "file", &row.revision_hash, &json!({ "text": text, "file": shown, "prompts": prompts })).map_err(failed)?;
        log!("kanna-daemon: {} runs {name} for {task} on {branch} in {worker}", id);
        let worst = pipeline.max_sessions(&Sizes::new());
        self.live.borrow_mut().push(Live { row, doc, pipeline, visit: None });
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
            "stage": last.map_or(Value::Null, |s| json!(s.stage)),
            "stageOpen": last.is_some_and(|s| s.ended_at.is_none()),
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
                    "id": s.id, "stage": s.stage, "visit": s.visit, "kind": s.kind, "agent": opt(s.agent.as_deref()), "machine": s.machine,
                    "startedAt": num(s.started_at), "endedAt": s.ended_at.map_or(Value::Null, num), "verdict": opt(s.verdict.as_deref()),
                    "session": opt(s.session.as_deref()), "terminal": opt(s.terminal.as_deref()), "worktree": opt(s.worktree.as_deref()), "branch": opt(s.branch.as_deref()),
                    "doc": s.doc,
                })
            })
            .collect();
        let workers: Vec<Value> = self.live.borrow().iter().filter(|l| l.row.id == row.id).filter_map(|l| l.visit.as_ref()).map(|v| json!({ "stage": v.row.stage, "terminal": opt(v.row.terminal.as_deref()), "worktree": opt(v.row.worktree.as_deref()) })).collect();
        let events = self.db.run_events(&row.id).map_err(failed)?;
        let stages = self.db.stages(&row.id).map_err(failed)?;
        let visits = visits.into_iter().zip(&stages).map(|(mut visit, stage)| { visit["interventions"] = json!(pipelines::visit_interventions(stage, &stages, &events)); visit }).collect();
        Ok(Map::from_iter([
            ("run".to_string(), self.row_value(&row)),
            ("metrics".to_string(), pipelines::numbers(&row, &stages, &events, 0, now()).value()),
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
        let found = live.iter().find(|l| l.visit.as_ref().is_some_and(|v| v.row.terminal.as_deref() == Some(by) && matches!(v.work, Work::Agent { .. })));
        let live = found.ok_or_else(not_a_stage)?;
        let visit = live.visit.clone().ok_or_else(not_a_stage)?;
        if visit.ending || visit.reported.is_some() || visit.paused {
            return Err(KmuxError::new("ended", format!("{} has reported already", visit.row.stage)));
        }
        let verdicts = live.pipeline.stage(&visit.row.stage).is_some_and(Stage::verdicts);
        Ok((live.row.id.clone(), visit, verdicts, live.row.state.clone()))
    }

    fn done(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (id, visit, verdicts, state) = self.reporting(args)?;
        let stage = visit.row.stage.clone();
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
        self.update(&id, |live| {
            if let Some(visit) = live.visit.as_mut() {
                visit.reported = Some((ended.clone(), now()));
                visit.needs = None;
            }
            if (state == "waiting" || state == "paused") && !live.doc.held {
                live.row.state = "running".into();
                live.doc.why = None;
                return Some(("resumed", None, json!({ "stage": stage })));
            }
            None
        });
        self.soon.set(true);
        Ok(Map::from_iter([("run".to_string(), json!(id)), ("stage".to_string(), json!(stage)), ("verdict".to_string(), json!(ended.shown))]))
    }

    fn need(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (id, visit, _, _) = self.reporting(args)?;
        let text = string(args, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("say what you need: kanna run need TEXT"))?.to_string();
        let (stage, pane) = (visit.row.stage.clone(), visit.row.terminal.clone().unwrap_or_default());
        log!("kanna-daemon: {id}'s {stage} needs: {text}");
        self.update(&id, |live| {
            if let Some(visit) = live.visit.as_mut() {
                visit.needs = Some(now());
            }
            None
        });
        self.pause(&id, &format!("{stage} needs: {text}"), Some(&pane), &format!("{stage} needs you: {text}\nIts agent stops once its turn ends, and its session is kept: `kanna run resume {id} \"…\"` carries it on with your answer."));
        self.soon.set(true);
        Ok(Map::from_iter([("run".to_string(), json!(id)), ("stage".to_string(), json!(stage)), ("state".to_string(), json!("paused"))]))
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
            let ids: Vec<String> = self.live.borrow().iter().filter(|l| l.row.state == "running" || l.visit.as_ref().is_some_and(|v| !v.paused)).map(|l| l.row.id.clone()).collect();
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
        let visits = live.iter().filter_map(|l| l.visit.as_ref()).filter(|v| !v.opening && !v.ending && !v.paused);
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
            (live.row.clone(), event)
        };
        let (row, event) = saved;
        let event = event.as_ref().map(|(what, by, doc)| (*what, by.as_deref(), doc));
        if let Err(error) = self.db.put_run(&row, event) {
            log!("kanna-daemon: couldn't save {id} in kanna.db: {error}");
        }
        self.asks_changed.set(true);
    }

    fn sync_asks(&self) {
        let (Some(core), Some(tasks)) = (self.core.upgrade(), self.tasks.upgrade()) else { return };
        let asking: Vec<String> = self.live.borrow().iter().filter(|l| l.row.state == "waiting" && l.visit.as_ref().is_some_and(|v| matches!(v.work, Work::Ask { .. }) && !v.ending)).map(|l| l.row.task.clone()).collect();
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
            log!("kanna-daemon: {task} is stopped, so {id} stops its stage and starts no other until it starts");
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
        let found: Vec<(String, bool, Option<Visit>)> = self.live.borrow().iter().filter(|l| l.row.task == task && l.doc.owner.is_none() && l.doc.held).map(|l| (l.row.id.clone(), l.doc.pausing, l.visit.clone())).collect();
        for (id, pausing, visit) in found {
            log!("kanna-daemon: {task} started again, so {id} carries on");
            let parked = visit.as_ref().is_some_and(|v| v.paused && v.row.doc["paused"]["task"] == true);
            if parked && !pausing {
                self.carrying.borrow_mut().push(id);
                continue;
            }
            self.update(&id, |live| {
                live.doc.held = false;
                if live.row.state == "paused" && !pausing && !live.visit.as_ref().is_some_and(|v| v.paused) {
                    live.row.state = "running".into();
                    live.doc.why = None;
                }
                Some(("resumed", Some("task".into()), json!({ "why": "the task started" })))
            });
        }
        self.soon.set(true);
    }

    fn step(self: &Rc<Self>, id: &str) {
        let Some(core) = self.core.upgrade() else { return };
        let (visit, next, state, stopped) = {
            let live = self.live.borrow();
            let Some(live) = live.iter().find(|l| l.row.id == id) else { return };
            (live.visit.clone(), live.doc.next.clone(), live.row.state.clone(), live.doc.held)
        };
        let Some(visit) = visit else {
            if let (Some(next), "running") = (next, state.as_str()) {
                self.begin(id, &next);
            }
            return;
        };
        if visit.opening || visit.ending || visit.paused {
            return;
        }
        let late = visit.due().is_some_and(|due| now() >= due);
        if let Work::Ask { .. } = &visit.work {
            if late && self.mark_ending(id) {
                self.conclude(id, Ended { verdict: Some(Verdict::Fail), shown: "fail", answer: String::new(), feedback: "timed out".into(), full: String::new(), failures: None, note: Some("timed out".into()), by: None }, None);
            }
            return;
        }
        let Some(pane) = visit.row.terminal.clone() else { return };
        if stopped && visit.reported.is_none() && visit.needs.is_none() {
            match &visit.work {
                Work::Agent { .. } => self.park(id),
                _ if self.mark_ending(id) => {
                    let (id, ended) = (id.to_string(), visit.clone());
                    self.end_visit(&visit, move |this, _| this.rerun(&id, &ended, "interrupted"));
                }
                _ => {}
            }
            return;
        }
        match &visit.work {
            Work::Agent { agent } => {
                if let Some((ended, at)) = visit.reported.clone() {
                    if now() - at >= SETTLE && self.mark_ending(id) {
                        self.quit_then(id, &pane, agent, ended);
                    }
                    return;
                }
                if let Some(at) = visit.needs {
                    if now() - at >= SETTLE {
                        self.park(id);
                    }
                    return;
                }
                if core.pane(&pane).is_none() {
                    return self.interrupted(id, &visit);
                }
                if !alive(&core, &pane) {
                    self.close(&pane);
                    return self.stop(id, "stopped", &format!("{}'s agent exited without reporting it was done", visit.row.stage), Some("daemon"), true);
                }
                if late {
                    self.timed_out(id, &pane, agent);
                }
            }
            Work::Command { log, exit } => {
                if exit.is_file() {
                    return self.command_ended(id, &pane, log, exit);
                }
                if core.pane(&pane).is_none() {
                    return self.interrupted(id, &visit);
                }
                if !alive(&core, &pane) {
                    let code = core.pane(&pane).and_then(|p| p.exit_code).unwrap_or(1);
                    let _ = fs::write(exit, format!("{code}\n"));
                    return self.command_ended(id, &pane, log, exit);
                }
            }
            Work::Ask { .. } => {}
        }
    }

    pub fn too_old(self: &Rc<Self>, pane: &str, why: &str) -> bool {
        let found = self.live.borrow().iter().find(|l| l.visit.as_ref().is_some_and(|v| v.row.terminal.as_deref() == Some(pane) && matches!(v.work, Work::Agent { .. }))).map(|l| l.row.id.clone());
        let Some(id) = found else { return false };
        self.stop(&id, "stopped", why, Some("daemon"), true);
        true
    }

    pub fn status(self: &Rc<Self>, pane: &str, ended: bool) {
        if !ended {
            return;
        }
        let found = self.live.borrow().iter().find_map(|l| {
            let visit = l.visit.as_ref().filter(|v| v.row.terminal.as_deref() == Some(pane) && matches!(v.work, Work::Agent { .. }) && !v.opening && !v.ending && !v.paused)?;
            Some((l.row.id.clone(), visit.row.stage.clone(), visit.reported.clone(), visit.needs.is_some(), l.row.state.clone()))
        });
        let Some((id, stage, reported, needs, state)) = found else { return };
        if let Some((ended, _)) = reported {
            let agent = match self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit.as_ref()).map(|v| v.work.clone()) {
                Some(Work::Agent { agent }) => agent,
                _ => return,
            };
            if self.mark_ending(&id) {
                self.quit_then(&id, pane, &agent, ended);
            }
            return;
        }
        if needs {
            return self.park(&id);
        }
        if state != "running" {
            return;
        }
        let Some(core) = self.core.upgrade() else { return };
        let (this, closing) = (Rc::downgrade(self), pane.to_string());
        core.handle(json!({ "cmd": "read", "args": { "pane": pane, "lines": SCREEN_LINES } }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            let screen = reply.get("text").and_then(Value::as_str).unwrap_or("").lines().map(str::trim_end).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
            let still = this.live.borrow().iter().any(|l| l.row.id == id && l.visit.as_ref().is_some_and(|v| v.row.terminal.as_deref() == Some(closing.as_str()) && v.reported.is_none() && v.needs.is_none() && !v.ending && !v.paused));
            if !still {
                return;
            }
            let why = format!("{stage} stopped without reporting");
            this.pause(&id, &why, None, &format!("{stage}'s turn ended without it reporting it was done or what it needs, so the run is paused and its session kept. Its screen ended:\n\n{screen}\n\n`kanna run resume {id} \"…\"` carries it on with your answer."));
            this.park(&id);
        });
    }

    fn pause(&self, id: &str, why: &str, by: Option<&str>, tell: &str) {
        log!("kanna-daemon: {id} pauses: {why}");
        let (why, by) = (why.to_string(), by.unwrap_or("daemon").to_string());
        self.update(id, |live| {
            let stage = live.visit.as_ref().map(|v| v.row.stage.clone());
            live.row.state = "paused".into();
            live.doc.why = Some(why.clone());
            Some(("paused", Some(by), json!({ "stage": stage, "why": why })))
        });
        self.tell(id, tell);
    }

    fn park(self: &Rc<Self>, id: &str) {
        let Some(visit) = self.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit.clone()) else { return };
        let (Work::Agent { agent }, Some(pane)) = (visit.work.clone(), visit.row.terminal.clone()) else { return };
        if visit.paused || visit.reported.is_some() || !self.mark_ending(id) {
            return;
        }
        let by_task = visit.needs.is_none() && self.live.borrow().iter().any(|l| l.row.id == id && l.doc.held);
        let (this, id) = (Rc::downgrade(self), id.to_string());
        let closing = pane.clone();
        let finish = move |said: Option<String>| {
            let Some(this) = this.upgrade() else { return };
            this.close(&closing);
            let mut parked = None;
            let mut started = false;
            this.update(&id, |live| {
                started = live.doc.held != by_task && !live.doc.pausing;
                if let Some(visit) = live.visit.as_mut() {
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
                let _ = this.db.put_stage(&row);
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

    fn live_of(&self, row: &RunRow) -> Result<(String, Doc, Option<Visit>, Option<Stage>), KmuxError> {
        let live = self.live.borrow();
        let live = live.iter().find(|l| l.row.id == row.id).ok_or_else(|| KmuxError::new("ended", format!("{} has ended ({})", row.id, row.outcome.as_deref().unwrap_or(&row.state))))?;
        let stage = live.visit.as_ref().and_then(|v| live.pipeline.stage(&v.row.stage).cloned());
        Ok((live.row.state.clone(), live.doc.clone(), live.visit.clone(), stage))
    }

    fn who(&self, args: &Value, task: &str) -> String {
        let agent = string(args, "by").zip(self.tasks.upgrade()).and_then(|(by, tasks)| tasks.task_of(by)).is_some_and(|of| of == task);
        if agent { "agent" } else { "user" }.to_string()
    }

    fn resume_run(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let row = self.find_live(args)?;
        let text = string(args, "text").map(str::trim).filter(|t| !t.is_empty()).unwrap_or(CARRY_ON).to_string();
        let by = self.who(args, &row.task);
        let (state, doc, visit, _) = self.live_of(&row)?;
        let reply = |stage: &str, state: &str| Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), json!(stage)), ("state".to_string(), json!(state))]));
        match &visit {
            Some(visit) if matches!(visit.work, Work::Ask { .. }) => Err(KmuxError::new("asking", format!("{} asks: answer it with `kanna run answer {} yes|no [NOTE]`", visit.row.stage, row.id))),
            Some(visit) if visit.paused && state == "paused" => {
                let stage = self.carry_on(&row.id, &text, &by)?;
                reply(&stage, "running")
            }
            Some(visit) if !visit.paused && (doc.pausing || doc.held) && !visit.ending => {
                let stage = visit.row.stage.clone();
                self.update(&row.id, |live| {
                    live.doc.pausing = false;
                    live.doc.held = false;
                    if live.row.state == "paused" {
                        live.row.state = "running".into();
                    }
                    if live.row.state == "running" {
                        live.doc.why = None;
                    }
                    intervene(live, "resumed");
                    Some(("resumed", Some(by.clone()), json!({ "stage": stage })))
                });
                reply(&stage, &state)
            }
            Some(visit) if visit.ending || visit.opening => Err(KmuxError::new("busy", format!("{}'s agent is still stopping: try again in a moment", visit.row.stage))),
            None if state == "paused" => {
                let next = doc.next.clone().unwrap_or_default();
                self.update(&row.id, |live| {
                    live.row.state = "running".into();
                    live.doc.why = None;
                    live.doc.pausing = false;
                    live.doc.held = false;
                    intervene(live, "resumed");
                    Some(("resumed", Some(by.clone()), json!({ "next": next })))
                });
                self.soon.set(true);
                reply(&next, "running")
            }
            _ => Err(KmuxError::new("not_paused", format!("{} isn't paused ({})", row.id, state))),
        }
    }

    fn carry_on(self: &Rc<Self>, id: &str, text: &str, by: &str) -> Result<String, KmuxError> {
        let (run, doc, visit, stage) = {
            let live = self.live.borrow();
            let live = live.iter().find(|l| l.row.id == id).ok_or_else(|| KmuxError::new("ended", format!("{id} has ended")))?;
            let visit = live.visit.clone().filter(|v| v.paused).ok_or_else(|| KmuxError::new("not_paused", format!("{id} has no stage to carry on")))?;
            let stage = live.pipeline.stage(&visit.row.stage).cloned().ok_or_else(|| failed(format!("{} isn't in the pipeline any more", visit.row.stage)))?;
            (live.row.clone(), live.doc.clone(), visit, stage)
        };
        let (text, by) = (text.to_string(), by.to_string());
        let Kind::Agent(agent) = &stage.kind else { return Err(failed(format!("{} isn't an agent stage", stage.name))) };
        let n = visit.row.doc.get("n").and_then(Value::as_u64).unwrap_or(u64::from(doc.visits));
        let replies = visit.row.doc.get("replies").and_then(Value::as_u64).unwrap_or(0) + 1;
        let worker = PathBuf::from(&doc.worker);
        let reply = format!("{SCRATCH}/reply-{n}-{replies}.md");
        fs::write(worker.join(&reply), &text).map_err(|e| failed(format!("can't write {reply}: {e}")))?;
        let files = tasks::config_files(Path::new(&run.repo));
        let mut vars = self.vars(&run, &doc, &stage.name);
        vars.push(("KANNA_ANSWER", worker.join(format!("{SCRATCH}/answer-{n}.md")).to_string_lossy().into_owned()));
        vars.push(("KANNA_CLI", tasks::cli_path()));
        let exports: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect();
        let session = visit.row.doc["paused"]["session"].as_str().map(String::from);
        let command = tasks::pipeline_agent(&files, &agent.agent);
        let first = format!("\"$(cat {})\"", quote(&reply));
        let line = format!("export {}; {}", exports.join(" "), tasks::stage_agent(&command, agent.effort.as_deref(), agent.model.as_deref(), tasks::prompts(&files), session.as_deref(), true, &first));
        let paused_at = visit.row.doc["paused"]["at"].as_f64().unwrap_or_else(now);
        self.update(&run.id, |live| {
            live.row.state = "running".into();
            live.doc.why = None;
            live.doc.held = false;
            live.doc.pausing = false;
            if by != "task" {
                intervene(live, "resumed");
            }
            if let Some(visit) = live.visit.as_mut() {
                let waited = visit.waited() + (now() - paused_at).max(0.0);
                patch(&mut visit.row.doc, json!({ "paused": null, "waited": waited, "replies": replies }));
                visit.paused = false;
                visit.opening = true;
            }
            Some(("resumed", Some(by.clone()), json!({ "stage": stage.name, "reply": text })))
        });
        log!("kanna-daemon: {} resumes {}", run.id, stage.name);
        self.open_worker(&run.id, &doc.worker, &line, |this, id, pane| {
            let mut saved = None;
            this.update(id, |live| {
                if let Some(visit) = live.visit.as_mut() {
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
        let (state, _, visit, _) = self.live_of(&row)?;
        let visit = visit.filter(|v| matches!(v.work, Work::Ask { .. }) && !v.ending).ok_or_else(|| {
            let hint = if state == "paused" { format!(": a paused stage carries on with `kanna run resume {} \"…\"`", row.id) } else { String::new() };
            KmuxError::new("no_ask", format!("{} isn't waiting on an ask ({state}){hint}", row.id))
        })?;
        let (by, stage) = (self.who(args, &row.task), visit.row.stage.clone());
        self.mark_ending(&row.id);
        self.update(&row.id, |live| {
            intervene(live, "answered");
            Some(("answered", Some(by.clone()), json!({ "stage": stage, "answer": said, "note": note })))
        });
        log!("kanna-daemon: {}'s {stage} is answered {said} by the {by}", row.id);
        self.conclude(&row.id, Ended { verdict: Some(verdict), shown: verdict.name(), answer: note.clone(), feedback: note.clone(), full: String::new(), failures: None, note: None, by: Some(by.clone()) }, None);
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), json!(stage)), ("answer".to_string(), json!(said)), ("by".to_string(), json!(by))]))
    }

    fn pause_run(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let row = self.find_live(args)?;
        let by = self.who(args, &row.task);
        let (state, doc, visit, _) = self.live_of(&row)?;
        if doc.pausing || (state == "paused" && !doc.held) {
            return Err(KmuxError::new("paused", format!("{} is paused already{}: `kanna run resume {}` carries it on", row.id, visit.as_ref().filter(|v| !v.paused).map_or(String::new(), |v| format!(" once {} ends", v.row.stage)), row.id)));
        }
        let stage = visit.as_ref().map(|v| v.row.stage.clone());
        let now_paused = visit.is_none();
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
        let (state, _, visit, stage) = self.live_of(&row)?;
        let visit = visit.ok_or_else(|| KmuxError::new("no_stage", format!("{} has no stage to {verb} ({state}{})", row.id, if state == "paused" { ": `kanna run resume` starts the next" } else { "" })))?;
        if visit.opening || visit.ending {
            return Err(KmuxError::new("busy", format!("{} is starting or stopping: try again in a moment", visit.row.stage)));
        }
        let stage = stage.ok_or_else(|| failed(format!("{} isn't in the pipeline any more", visit.row.stage)))?;
        let by = self.who(args, &row.task);
        Ok((row, visit, stage, by))
    }

    fn skip(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (row, visit, stage, by) = self.steered(args, "skip")?;
        let verdict = stage.verdicts().then_some(Verdict::Pass);
        let ended = Ended { verdict, shown: "skipped", answer: String::new(), feedback: String::new(), full: String::new(), failures: None, note: Some(format!("skipped by the {by}")), by: Some(by.clone()) };
        self.mark_ending(&row.id);
        self.update(&row.id, |live| {
            intervene(live, "skipped");
            Some(("skipped", Some(by.clone()), json!({ "stage": stage.name, "visit": visit.row.visit })))
        });
        log!("kanna-daemon: {}'s {} is skipped by the {by}", row.id, stage.name);
        let id = row.id.clone();
        self.end_visit(&visit, move |this, said| this.conclude(&id, ended, said));
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), json!(stage.name))]))
    }

    fn retry(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let (row, visit, stage, by) = self.steered(args, "retry")?;
        self.mark_ending(&row.id);
        self.update(&row.id, |live| {
            intervene(live, "retried");
            Some(("retried", Some(by.clone()), json!({ "stage": stage.name, "visit": visit.row.visit })))
        });
        log!("kanna-daemon: {}'s {} runs again for the {by}", row.id, stage.name);
        let (id, ended) = (row.id.clone(), visit.clone());
        self.end_visit(&visit, move |this, _| this.rerun(&id, &ended, "retried"));
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("stage".to_string(), json!(stage.name))]))
    }

    fn end_visit(self: &Rc<Self>, visit: &Visit, then: impl FnOnce(&Rc<Self>, Option<String>) + 'static) {
        let this = Rc::downgrade(self);
        let pane = visit.row.terminal.clone().filter(|_| !visit.paused);
        let closing = pane.clone();
        let finish = move |said: Option<String>| {
            let Some(this) = this.upgrade() else { return };
            if let Some(pane) = &closing {
                this.close(pane);
            }
            then(&this, said);
        };
        match (&visit.work, pane) {
            (Work::Agent { agent }, Some(pane)) => match self.tasks.upgrade() {
                Some(tasks) => tasks.end_agent(&pane, agent, finish),
                None => finish(None),
            },
            (Work::Command { .. }, Some(pane)) => match self.core.upgrade().filter(|core| alive(core, &pane)) {
                Some(core) => core.end(&pane, move |_| finish(None)),
                None => finish(None),
            },
            _ => finish(None),
        }
    }

    fn open_worker(self: &Rc<Self>, id: &str, cwd: &str, command: &str, opened: impl FnOnce(&Rc<Self>, &str, &str) + 'static) {
        let Some(core) = self.core.upgrade() else { return };
        let task = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.task.clone()).unwrap_or_default();
        let (this, id) = (Rc::downgrade(self), id.to_string());
        core.handle(json!({ "cmd": "open", "args": { "type": "term", "background": true, "workspace": task, "cwd": cwd, "cmd": command, "wait": false } }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            let pane = reply.get("pane").and_then(|p| string(p, "id")).map(String::from);
            let Some(pane) = pane.filter(|_| reply.get("ok") == Some(&Value::Bool(true))) else {
                let why = reply.get("error").and_then(|e| string(e, "message")).unwrap_or("it failed").to_string();
                let stage = this.live.borrow().iter().find(|l| l.row.id == id).and_then(|l| l.visit.as_ref()).map(|v| v.row.stage.clone()).unwrap_or_default();
                this.update(&id, |live| {
                    if let Some(visit) = live.visit.as_mut() {
                        visit.ending = true;
                    }
                    None
                });
                return this.stop(&id, "stopped", &format!("can't open a terminal for {stage}: {why}"), Some("daemon"), true);
            };
            opened(&this, &id, &pane);
            this.soon.set(true);
        });
    }

    fn vars(&self, row: &RunRow, doc: &Doc, stage: &str) -> Vec<(&'static str, String)> {
        let task = self.tasks.upgrade().and_then(|t| t.entry(&row.task));
        vec![
            ("KANNA_TASK", row.task.clone()),
            ("KANNA_TASK_NAME", task.as_ref().and_then(|t| t.name.clone()).unwrap_or_default()),
            ("KANNA_REPO", row.repo.clone()),
            ("KANNA_WORKTREE", doc.worker.clone()),
            ("KANNA_BRANCH", row.branch.clone()),
            ("KANNA_BASE", doc.base.clone()),
            ("KANNA_RUN", row.id.clone()),
            ("KANNA_STAGE", stage.to_string()),
        ]
    }

    fn mark_ending(&self, id: &str) -> bool {
        let mut live = self.live.borrow_mut();
        let Some(visit) = live.iter_mut().find(|l| l.row.id == id).and_then(|l| l.visit.as_mut()) else { return false };
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

    fn quit_then(self: &Rc<Self>, id: &str, pane: &str, agent: &str, ended: Ended) {
        let (this, id, closing) = (Rc::downgrade(self), id.to_string(), pane.to_string());
        let finish = move |said: Option<String>| {
            let Some(this) = this.upgrade() else { return };
            this.close(&closing);
            this.conclude(&id, ended, said);
        };
        match self.tasks.upgrade() {
            Some(tasks) => tasks.end_agent(pane, agent, finish),
            None => finish(None),
        }
    }

    fn timed_out(self: &Rc<Self>, id: &str, pane: &str, agent: &str) {
        if !self.mark_ending(id) {
            return;
        }
        let (worker, start, stage, verdicts) = {
            let live = self.live.borrow();
            let Some(l) = live.iter().find(|l| l.row.id == id) else { return };
            let Some(visit) = l.visit.as_ref() else { return };
            let verdicts = l.pipeline.stage(&visit.row.stage).is_some_and(Stage::verdicts);
            (visit.row.worktree.clone().unwrap_or_default(), string(&visit.row.doc, "start").unwrap_or("").to_string(), visit.row.stage.clone(), verdicts)
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
            self.update(id, |live| {
                if let Some(visit) = live.visit.as_mut() {
                    visit.ending = true;
                }
                None
            });
            return self.stop(id, "stopped", &why, Some("daemon"), true);
        }
        let ended = Ended { verdict: Some(Verdict::Fail), shown: "fail", answer: String::new(), feedback: "timed out".into(), full: String::new(), failures: None, note: Some("timed out".into()), by: None };
        self.quit_then(id, pane, agent, ended);
    }

    fn command_ended(self: &Rc<Self>, id: &str, pane: &str, log: &Path, exit: &Path) {
        if !self.mark_ending(id) {
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
            this.close(&closing);
            this.conclude(&id, ended, None);
        };
        match self.core.upgrade().filter(|core| alive(core, pane)) {
            Some(core) => core.end(pane, move |_| finish()),
            None => finish(),
        }
    }

    fn interrupted(self: &Rc<Self>, id: &str, visit: &Visit) {
        log!("kanna-daemon: {id}'s {} visit lost its terminal, so it's recorded interrupted and runs again", visit.row.stage);
        self.rerun(id, visit, "interrupted");
    }

    fn rerun(self: &Rc<Self>, id: &str, visit: &Visit, verdict: &str) {
        let mut row = visit.row.clone();
        row.ended_at = Some(now());
        row.verdict = Some(verdict.into());
        if row.id != 0 {
            let _ = self.db.put_stage(&row);
        }
        if let Some(repo) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.repo.clone()) {
            self.measure(Measuring::of(&row, &repo, None, None));
        }
        let stage = row.stage.clone();
        self.update(id, |live| {
            live.visit = None;
            live.doc.next = Some(stage.clone());
            held(live, &stage);
            (verdict == "interrupted").then(|| ("verdict", Some("daemon".into()), json!({ "stage": stage, "visit": row.visit, "verdict": verdict })))
        });
        self.soon.set(true);
    }

    fn begin(self: &Rc<Self>, id: &str, name: &str) {
        let (stage, pipeline, doc, row) = {
            let live = self.live.borrow();
            let Some(live) = live.iter().find(|l| l.row.id == id) else { return };
            let Some(stage) = live.pipeline.stage(name).cloned() else { return };
            (stage, live.pipeline.clone(), live.doc.clone(), live.row.clone())
        };
        let elapsed = Duration::from_secs_f64((now() - row.started_at).max(0.0));
        if let Some(why) = over_budget(&pipeline, &doc, &stage, elapsed) {
            return self.wait(id, &why);
        }
        let worker = PathBuf::from(&doc.worker);
        let head = git(&worker, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let n = doc.visits + 1;
        let visit = self.db.stages(id).map(|s| s.iter().filter(|s| s.stage == name).count() as i64 + 1).unwrap_or(1);
        let scratch = worker.join(SCRATCH);
        if let Err(error) = fs::create_dir_all(&scratch) {
            return self.stop(id, "stopped", &format!("can't make {}: {error}", scratch.display()), Some("daemon"), true);
        }
        let mut row_doc = json!({ "n": n, "start": head });
        let mut stage_row = StageRow {
            id: 0,
            run: id.to_string(),
            stage: name.to_string(),
            visit,
            instance: None,
            kind: String::new(),
            agent: None,
            machine: self.host.clone(),
            started_at: now(),
            ended_at: None,
            verdict: None,
            session: None,
            terminal: None,
            worktree: Some(doc.worker.clone()),
            branch: Some(row.branch.clone()),
            doc: Value::Null,
        };
        if let Kind::Ask { question } = &stage.kind {
            return self.ask(id, stage_row, row_doc, &head, question);
        }
        let files = tasks::config_files(Path::new(&row.repo));
        let mut vars = self.vars(&row, &doc, name);
        let (command, work, sessions) = match &stage.kind {
            Kind::Agent(agent) => {
                let prompts = self.db.revisions(id).ok().and_then(|r| r.last().and_then(|r| r.doc.get("prompts").cloned())).unwrap_or(Value::Null);
                let file = stage.prompt().and_then(|p| string(&prompts, p).map(String::from)).or_else(|| stage.prompt().and_then(|p| fs::read_to_string(worker.join(format!(".kanna/prompts/{p}.md"))).ok())).unwrap_or_default();
                let answer = format!("{SCRATCH}/answer-{n}.md");
                let text = prompt(&stage, &file, &doc.request, doc.feedback.as_ref(), &doc.answers, &answer);
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
                (line, Work::Agent { agent: agent.agent.clone() }, stage.sessions(&Sizes::new()))
            }
            Kind::Run { command } => {
                let (log, exit, feedback) = (scratch.join(format!("stage-{n}.log")), scratch.join(format!("stage-{n}.exit")), scratch.join(format!("feedback-{n}.txt")));
                let _ = fs::remove_file(&exit);
                let previous = doc.feedback.as_ref().map(|f| fs::read_to_string(&f.full).unwrap_or_else(|_| f.text.clone())).unwrap_or_default();
                let _ = fs::write(&feedback, previous);
                vars.push(("KANNA_FEEDBACK", feedback.to_string_lossy().into_owned()));
                let timeout = stage.within.map_or_else(|| tasks::scripts(&files).timeout, |w| w.as_secs().max(1));
                stage_row.kind = "run".into();
                row_doc["log"] = json!(log.to_string_lossy());
                row_doc["exit"] = json!(exit.to_string_lossy());
                (task_scripts::stage_command(&vars, command, timeout, &log, &exit), Work::Command { log, exit }, 0)
            }
            _ => return self.stop(id, "stopped", &format!("{name} is a kind of stage runs can't do yet"), Some("daemon"), true),
        };
        stage_row.doc = row_doc;
        let within = stage.within;
        let visit_row = stage_row.clone();
        self.update(id, |live| {
            live.doc.starts.entry(name.to_string()).or_insert(head.clone());
            live.doc.visits = n;
            live.doc.sessions += sessions;
            live.visit = Some(Visit { row: visit_row, work: work.clone(), within, opening: true, ending: false, reported: None, needs: None, paused: false });
            None
        });
        self.open_worker(id, &doc.worker, &command, move |this, id, pane| {
            stage_row.terminal = Some(pane.to_string());
            if let Ok(row_id) = this.db.put_stage(&stage_row) {
                stage_row.id = row_id;
            }
            log!("kanna-daemon: {id} starts {} (visit {}) in {pane}", stage_row.stage, stage_row.visit);
            let event = json!({ "stage": stage_row.stage, "visit": stage_row.visit, "kind": stage_row.kind, "terminal": pane });
            this.update(id, |live| {
                if let Some(visit) = live.visit.as_mut() {
                    visit.row = stage_row.clone();
                    visit.opening = false;
                }
                live.doc.next = None;
                Some(("stage", Some("daemon".into()), event))
            });
        });
    }

    fn ask(self: &Rc<Self>, id: &str, mut stage_row: StageRow, mut row_doc: Value, head: &str, question: &str) {
        let (name, n) = (stage_row.stage.clone(), row_doc["n"].as_u64().unwrap_or(1) as u32);
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
            live.doc.starts.entry(name.clone()).or_insert(head.to_string());
            live.doc.visits = n;
            live.doc.next = None;
            live.visit = Some(Visit { row: stage_row, work: Work::Ask { question: question.to_string() }, within, opening: false, ending: false, reported: None, needs: None, paused: false });
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

    fn conclude(self: &Rc<Self>, id: &str, ended: Ended, said: Option<String>) {
        let worker_head = |worker: &str| git(Path::new(worker), &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let Some((mut row, pipeline, mut doc, repo)) = ({
            let live = self.live.borrow();
            live.iter().find(|l| l.row.id == id).and_then(|l| l.visit.as_ref().map(|v| (v.row.clone(), l.pipeline.clone(), l.doc.clone(), l.row.repo.clone())))
        }) else {
            return;
        };
        let end = worker_head(&doc.worker);
        let measuring = Measuring::of(&row, &repo, said, Some(end.clone()));
        let start = string(&row.doc, "start").unwrap_or("").to_string();
        let commits = if start.is_empty() || end.is_empty() { 0 } else { git(Path::new(&doc.worker), &["rev-list", "--count", &format!("{start}..{end}")]).ok().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0) };
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
        let failed_verdict = ended.verdict == Some(Verdict::Fail);
        let failing = ended.failures.as_ref().filter(|_| failed_verdict).map(|lines| failing_tests(lines));
        let mut kept = json!({ "answer": answer, "feedback": ended.feedback, "failures": ended.failures, "note": opt(ended.note.as_deref()), "end": end, "commits": commits, "by": opt(ended.by.as_deref()), "feedbackSize": ended.feedback.len(), "failing": failing, "findings": findings(&ended.answer) });
        for key in ["question", "replies", "waited", "log"] {
            if let Some(value) = row.doc.get(key) {
                kept[key] = value.clone();
            }
        }
        row.doc = kept;
        if let Err(error) = self.db.put_stage(&row) {
            log!("kanna-daemon: couldn't save {id}'s {} visit: {error}", row.stage);
        }
        self.measure(measuring);
        let stage = row.stage.clone();
        if row.kind == "agent" {
            doc.answers.insert(stage.clone(), ended.answer.clone());
            for (key, value) in outputs(&ended.answer) {
                doc.answers.insert(format!("{stage}.{key}"), value);
            }
        }
        let next = decide(&pipeline, &mut doc, &stage, ended.verdict, ended.failures.clone());
        let arrow = match &next {
            Next::Stage { name, .. } => json!(name),
            Next::Done => json!(DONE),
            Next::Stuck => Value::Null,
        };
        log!("kanna-daemon: {id}: {stage} ended {} → {}", ended.shown, arrow.as_str().unwrap_or("no arrow"));
        doc.feedback = carried(&next, &ended, &stage, row.kind == "ask");
        let event = json!({ "stage": stage, "visit": row.visit, "verdict": ended.shown, "next": arrow, "commits": commits, "note": opt(ended.note.as_deref()) });
        if let Next::Stage { name, reset: true } = &next {
            if let Some(start) = doc.starts.get(name).cloned() {
                let worker = Path::new(&doc.worker);
                let abandoned = format!("{}-reset-{}", row.branch.clone().unwrap_or_default(), doc.visits);
                let _ = git(worker, &["branch", &abandoned, "HEAD"]);
                let _ = git(worker, &["reset", "--hard", &start]);
            }
        }
        doc.next = match &next {
            Next::Stage { name, .. } => Some(name.clone()),
            _ => None,
        };
        self.update(id, |live| {
            live.visit = None;
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
        let visit = {
            let live = self.live.borrow();
            live.iter().find(|l| l.row.id == id).and_then(|l| l.visit.clone())
        };
        if let Some(visit) = &visit {
            let mut row = visit.row.clone();
            if row.id != 0 {
                row.ended_at = Some(now());
                row.verdict = Some("stopped".into());
                let _ = self.db.put_stage(&row);
                if let Some(repo) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.repo.clone()) {
                    self.measure(Measuring::of(&row, &repo, None, None));
                }
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
        let outcome = if why.starts_with("stuck") { "stuck" } else { "stopped" };
        log!("kanna-daemon: {id} {state}: {why}");
        let (by, why_owned, state_owned) = (by.map(String::from), why.to_string(), state.to_string());
        self.update(id, |live| {
            live.visit = None;
            live.row.state = state_owned.clone();
            live.row.ended_at = Some(now());
            live.row.outcome = Some(outcome.into());
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

    fn owned(&self, id: &str) -> bool {
        self.live.borrow().iter().any(|l| l.row.id == id && l.doc.owner.is_some())
    }

    fn ended(&self, id: &str, outcome: Result<String, String>) {
        if let Some(on_end) = self.on_end.borrow().as_ref() {
            on_end(id, outcome);
        }
    }

    pub fn stop_owned(self: &Rc<Self>, id: &str, why: &str) {
        let found = self.live.borrow().iter().find(|l| l.row.id == id && LIVE.contains(&l.row.state.as_str())).map(|l| (l.row.repo.clone(), l.doc.worker.clone(), l.visit.as_ref().and_then(|v| v.row.terminal.clone())));
        let Some((repo, worker, terminal)) = found else { return };
        self.close_run(id, &repo, &worker, terminal, why);
    }

    pub fn task_closed(self: &Rc<Self>, task: &str) {
        let found: Vec<(String, String, String, Option<String>)> = self.live.borrow().iter().filter(|l| l.row.task == task && l.doc.owner.is_none() && LIVE.contains(&l.row.state.as_str())).map(|l| (l.row.id.clone(), l.row.repo.clone(), l.doc.worker.clone(), l.visit.as_ref().and_then(|v| v.row.terminal.clone()))).collect();
        for (id, repo, worker, terminal) in found {
            self.close_run(&id, &repo, &worker, terminal, "the task was closed");
        }
    }

    fn close_run(self: &Rc<Self>, id: &str, repo: &str, worker: &str, terminal: Option<String>, why: &str) {
        self.update(id, |live| {
            if let Some(visit) = live.visit.as_mut() {
                visit.ending = true;
            }
            None
        });
        if let Some(pane) = terminal {
            self.close(&pane);
        }
        self.stop(id, "stopped", why, Some("daemon"), false);
        if Path::new(worker).is_dir() {
            if let Err(error) = git(Path::new(repo), &["worktree", "remove", "--force", worker]) {
                log!("kanna-daemon: {id}'s worktree {worker} is kept: {error}");
            }
        }
    }

    fn hand_back(self: &Rc<Self>, id: &str) {
        let Some((row, doc)) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| (l.row.clone(), l.doc.clone())) else { return };
        let branch = &row.branch;
        let note = format!("{branch} is ready: take it with `git merge --ff-only {branch}` in your worktree, or look first with `git log HEAD..{branch}` and `git diff HEAD...{branch}`");
        let worker = Path::new(&doc.worker);
        if worker.is_dir() {
            if let Err(error) = git(Path::new(&row.repo), &["worktree", "remove", &doc.worker]) {
                log!("kanna-daemon: {id}'s worktree {} is kept: {error}", doc.worker);
            }
        }
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

    fn tell(&self, id: &str, text: &str) {
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
        let build = prompt(p.stage("build").unwrap(), "Build it.", "Add a button", None, &BTreeMap::new(), ".kanna/tmp/answer-1.md");
        assert!(build.ends_with("# When you're done\n\nWrite your final answer to `.kanna/tmp/answer-1.md`, then run `\"$KANNA_CLI\" run done`. Kanna ends this session once your turn ends. If you need something from the user before you can finish, run `\"$KANNA_CLI\" run need \"WHAT YOU NEED\"` and end your turn: Kanna stops this session and resumes it with their answer.\n"), "{build}");
        let review = prompt(p.stage("review").unwrap(), "Review it.", "Add a button", None, &BTreeMap::new(), ".kanna/tmp/answer-2.md");
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
        let text = prompt(stage, "Fix what failed.\n{{build.answer}}", "Add a button", Some(&Feedback { stage: "check".into(), text: "FAILED a".into(), full: String::new() }), &BTreeMap::from([("build".into(), "built it".into())]), "a.md");
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
        assert_eq!(over_budget(&p, &doc, build, Duration::from_secs(60)), None);
        doc.sessions = 4;
        assert_eq!(over_budget(&p, &doc, build, Duration::from_secs(60)).as_deref(), Some("over budget: build would be session 5 of 4"));
        doc.sessions = 0;
        assert_eq!(over_budget(&p, &doc, build, Duration::from_secs(3600)).as_deref(), Some("over budget: 1h 00m of 1h 00m spent before build"));
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
        let mut visit = Visit { row, work: Work::Agent { agent: "claude".into() }, within: Some(Duration::from_secs(60)), opening: false, ending: false, reported: None, needs: None, paused: false };
        assert_eq!(visit.due(), Some(160.0));
        patch(&mut visit.row.doc, json!({ "paused": { "at": 120.0, "session": "s-1" } }));
        assert_eq!(visit.row.doc["paused"]["session"], "s-1");
        patch(&mut visit.row.doc, json!({ "paused": null, "waited": 30.0 }));
        assert_eq!(visit.row.doc, json!({ "n": 1, "waited": 30.0 }));
        assert_eq!(visit.due(), Some(190.0));
    }

    #[test]
    fn a_pipeline_with_parts_runs_can_t_do_yet_is_refused_by_line() {
        let p = pipeline("plan = claude\nbuild = claude each plan.parts\nok = ask \"Is the plan right?\"\njoin = merge build\nplan -> ok -> build -> join -> done\n");
        assert_eq!(unbuilt(&p), vec!["2: build uses each, which runs can't do yet", "4: join uses merge STAGE, which runs can't do yet"]);
    }

    #[test]
    fn findings_come_from_the_block_named_findings_by_severity() {
        let answer = "All in all:\n```json name=findings\n[{\"severity\": \"bug\", \"file\": \"a.rs\", \"line\": 1, \"what\": \"x\"}, {\"severity\": \"nit\"}, {\"severity\": \"nit\"}, {\"what\": \"y\"}]\n```\nfail";
        assert_eq!(findings(answer), Some(BTreeMap::from([("bug".to_string(), 1), ("nit".to_string(), 2), ("unrated".to_string(), 1)])));
        assert_eq!(findings("```json name=findings\n[]\n```"), Some(BTreeMap::new()), "a review that found nothing still counts as a review");
        assert_eq!(findings("no block"), None);
        assert_eq!(findings("```json name=findings\nnot json\n```"), None);
    }

    #[test]
    fn failing_tests_are_the_failure_lines_without_their_summaries() {
        let output = "test a ... ok\ntest gutter_colors ... FAILED\ntest diff_theme ... FAILED\n\nfailures:\ntest result: FAILED. 3 passed; 2 failed; 0 ignored\nerror: test failed, to rerun pass `-p x`\n";
        let lines = failure_lines(output);
        assert_eq!(failing_tests(&lines), 2, "{lines:?}");
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
        Live { row, doc, pipeline: pipeline("a = claude\n"), visit: None }
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
    fn a_run_s_doc_round_trips() {
        let doc = Doc { request: "r".into(), taken: vec![1, 0], feedback: Some(Feedback { stage: "check".into(), text: "t".into(), full: "/f".into() }), failures: BTreeMap::from([("check".into(), vec!["x".into()])]), sessions: 2, next: Some("fix".into()), revision: 1, pausing: true, held: true, interventions: BTreeMap::from([("answered".into(), 1)]), ..Doc::default() };
        assert_eq!(Doc::from_value(&doc.to_value()), doc);
    }
}
