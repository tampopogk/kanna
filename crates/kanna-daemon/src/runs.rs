use crate::db::{Db, RunRow, StageRow};
use crate::task_scripts;
use crate::tasks::{self, git, Entry, Tasks};
use kanna_detect::{Status, Turn, TURN_SETTLE};
use kanna_mux::json::{json, num, opt, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, PaneState, Responder, WeakCore};
use kanna_pipe::{Context, Kind, Merge, Pipeline, Sizes, Stage, Verdict, DONE};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TICK: Duration = Duration::from_secs(2);

const SCRATCH: &str = ".kanna/tmp";

const FEEDBACK_LINES: usize = 80;

const FEEDBACK_CHARS: usize = 8000;

const FAILURE_MARKS: &[&str] = &["FAILED", "error[", "panic", "✗", "failed"];

const ANSWER_CHARS: usize = 64 << 10;

pub const LIVE: &[&str] = &["running", "waiting", "paused"];

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
    pub landed: bool,
    pub note: Option<String>,
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
            "landed": self.landed,
            "note": opt(self.note.as_deref()),
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
            landed: value.get("landed") == Some(&Value::Bool(true)),
            note: string(value, "note").map(String::from),
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
            Kind::Ask { .. } => say("ask"),
            Kind::Merge(Merge::Stage(_)) => say("merge STAGE"),
            _ => {}
        }
        if stage.on.as_deref().is_some_and(|on| on != "here") {
            say("on (stages run here until machines are built)");
        }
    }
    out
}

pub fn prompt(stage: &Stage, file: &str, request: &str, feedback: Option<&Feedback>, answers: &BTreeMap<String, String>) -> String {
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
    if stage.agent().is_some_and(|a| a.verdict) {
        out += "\n# Your verdict\n\nEnd your answer with a line of its own: `VERDICT: PASS` if it passes, or `VERDICT: FAIL` if it doesn't.\n";
    }
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

pub fn claude_answer(config: &Path, session: &str) -> Option<String> {
    let name = format!("{session}.jsonl");
    let file = fs::read_dir(config.join("projects")).ok()?.flatten().map(|d| d.path().join(&name)).find(|f| f.is_file())?;
    Some(claude_text(&fs::read_to_string(file).ok()?))
}

pub fn claude_text(jsonl: &str) -> String {
    let mut texts: Vec<String> = vec![];
    for line in jsonl.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        match string(&entry, "type") {
            Some("user") if !entry.get("isMeta").and_then(Value::as_bool).unwrap_or(false) => texts.clear(),
            Some("assistant") => {
                let content = entry.get("message").and_then(|m| m.get("content"));
                match content {
                    Some(Value::String(text)) => texts.push(text.clone()),
                    Some(Value::Array(blocks)) => texts.extend(blocks.iter().filter(|b| string(b, "type") == Some("text")).filter_map(|b| string(b, "text")).map(String::from)),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    texts.join("\n\n")
}

pub fn codex_answer(sessions: &Path, folder: &Path, since: SystemTime) -> Option<String> {
    let listed = |dir: &Path| fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect::<Vec<_>>();
    let touched = |path: &Path| fs::metadata(path).and_then(|m| m.modified()).is_ok_and(|at| at >= since);
    let mut found: Vec<(SystemTime, PathBuf)> = listed(sessions)
        .into_iter()
        .flat_map(|year| listed(&year))
        .flat_map(|month| listed(&month))
        .filter(|day| day.is_dir() && touched(day))
        .flat_map(|day| listed(&day))
        .filter(|f| f.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl")) && touched(f))
        .filter_map(|f| {
            let first = fs::read_to_string(&f).ok()?.lines().next()?.to_string();
            let meta = serde_json::from_str::<Value>(&first).ok()?;
            let cwd = meta.get("payload").and_then(|p| string(p, "cwd"))?;
            let same = Path::new(cwd) == folder || fs::canonicalize(cwd).ok() == fs::canonicalize(folder).ok();
            same.then(|| (fs::metadata(&f).and_then(|m| m.created().or_else(|_| m.modified())).unwrap_or(since), f))
        })
        .collect();
    found.sort();
    let (_, file) = found.into_iter().next()?;
    Some(codex_text(&fs::read_to_string(file).ok()?))
}

pub fn codex_text(jsonl: &str) -> String {
    let mut last = String::new();
    for line in jsonl.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else { continue };
        let Some(payload) = entry.get("payload") else { continue };
        if string(&entry, "type") == Some("response_item") && string(payload, "type") == Some("message") && string(payload, "role") == Some("assistant") {
            let text: Vec<&str> = payload.get("content").and_then(Value::as_array).into_iter().flatten().filter(|b| string(b, "type") == Some("output_text")).filter_map(|b| string(b, "text")).collect();
            if !text.is_empty() {
                last = text.join("\n\n");
            }
        }
    }
    last
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

pub fn agent_command(agent: &str, overrides: Option<&str>) -> String {
    let found = overrides.into_iter().flat_map(str::split_whitespace).find_map(|pair| pair.split_once('=').filter(|(name, _)| *name == agent).map(|(_, command)| command.to_string()));
    found.unwrap_or_else(|| agent.to_string())
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
    Command { log: PathBuf, exit: PathBuf },
}

#[derive(Clone, Debug)]
struct Visit {
    row: StageRow,
    work: Work,
    within: Option<Duration>,
    opening: bool,
    ending: bool,
}

struct Live {
    row: RunRow,
    doc: Doc,
    pipeline: Pipeline,
    visit: Option<Visit>,
}

struct Ended {
    verdict: Option<Verdict>,
    shown: &'static str,
    answer: String,
    feedback: String,
    full: String,
    failures: Option<Vec<String>>,
    note: Option<String>,
}

pub struct Runs {
    db: Rc<Db>,
    core: WeakCore,
    tasks: Weak<Tasks>,
    live: RefCell<Vec<Live>>,
    turns: RefCell<HashMap<String, Turn>>,
    soon: Cell<bool>,
    host: String,
    home: PathBuf,
}

impl Runs {
    pub fn open(db: Rc<Db>, core: WeakCore, tasks: Weak<Tasks>, home: PathBuf) -> Runs {
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
                    _ => Work::Command { log: PathBuf::from(string(&row.doc, "log").unwrap_or("")), exit: PathBuf::from(string(&row.doc, "exit").unwrap_or("")) },
                };
                let within = pipeline.stage(&row.stage).and_then(|s| s.within);
                Visit { row, work, within, opening: false, ending: false }
            });
            log!("kanna-daemon: {} carries on ({}, {})", row.id, row.state, visit.as_ref().map_or("between stages".to_string(), |v| format!("in {}", v.row.stage)));
            live.push(Live { row, doc, pipeline, visit });
        }
        Runs { db, core, tasks, live: RefCell::new(live), turns: RefCell::default(), soon: Cell::new(true), host: machine(), home }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
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
            other => Err(bad(format!("unknown run action {other:?} (start, list, show, log or stop)"))),
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
        if let Some(running) = self.db.runs_of(&task.id).map_err(failed)?.into_iter().find(|r| LIVE.contains(&r.state.as_str())) {
            return Err(KmuxError::new("running", format!("{} already has a run, {}: one run at a time per task (kanna run stop {})", task.id, running.id, running.id)));
        }
        let folder = PathBuf::from(task.folder.clone().ok_or_else(|| KmuxError::new("not_started", format!("{} has no worktree yet", task.id)))?);
        let named = string(args, "pipeline").filter(|p| !p.is_empty()).ok_or_else(|| bad("say which pipeline: kanna run start NAME|FILE"))?;
        let file = if named.ends_with(".pipe") || named.contains('/') { PathBuf::from(named) } else { folder.join(format!(".kanna/pipelines/{named}.pipe")) };
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
        let context = Context { prompts: prompts.clone(), machine: Some(self.host.clone()), ..Context::default() };
        let pipeline = kanna_pipe::load(&text, &context).map_err(|errors| KmuxError::new("invalid", errors.iter().map(|e| e.at(&shown)).collect::<Vec<_>>().join("\n")))?;
        let unbuilt = unbuilt(&pipeline);
        if !unbuilt.is_empty() {
            return Err(KmuxError::new("unsupported", unbuilt.iter().map(|u| format!("{shown}:{u}")).collect::<Vec<_>>().join("\n")));
        }
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
        let id = self.db.new_run_id().map_err(failed)?;
        let doc = Doc { request, file: shown.clone(), worker: worker.clone(), base: base.clone(), revision: 1, taken: vec![0; pipeline.arrows.len()], next: Some(pipeline.start.clone()), ..Doc::default() };
        let budget = pipeline.budget.as_ref().map_or(Value::Null, |b| json!({ "sessions": b.sessions, "seconds": b.time.map(|t| t.as_secs()) }));
        let mut value = doc.to_value();
        value["budget"] = budget;
        let row = RunRow { id: id.clone(), task: task.id.clone(), repo: task.repo.clone(), pipeline: name.clone(), revision_hash: hash(&text), branch: branch.clone(), state: "running".into(), started_at: now(), ended_at: None, outcome: None, doc: value };
        let by = string(args, "by").unwrap_or("user").to_string();
        self.db.put_run(&row, Some(("started", Some(&by), &json!({ "pipeline": name, "branch": branch, "base": base })))).map_err(failed)?;
        self.db.add_revision(&id, "file", &row.revision_hash, &json!({ "text": text, "file": shown, "prompts": prompts })).map_err(failed)?;
        log!("kanna-daemon: {} runs {name} for {} on {branch} in {worker}", id, task.id);
        let worst = pipeline.max_sessions(&Sizes::new());
        self.live.borrow_mut().push(Live { row, doc, pipeline, visit: None });
        self.soon.set(true);
        Ok(Map::from_iter([
            ("run".to_string(), json!(id)),
            ("task".to_string(), json!(task.id)),
            ("pipeline".to_string(), json!(name)),
            ("branch".to_string(), json!(branch)),
            ("worktree".to_string(), json!(worker)),
            ("worst".to_string(), json!(worst)),
        ]))
    }

    fn find(&self, args: &Value) -> Result<RunRow, KmuxError> {
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
        self.db.runs_of(&task.id).map_err(failed)?.pop().ok_or_else(|| KmuxError::new("not_found", format!("{} has no runs", task.id)))
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
        let row = self.find(args)?;
        if !LIVE.contains(&row.state.as_str()) {
            return Err(KmuxError::new("ended", format!("{} has ended already ({})", row.id, row.outcome.as_deref().unwrap_or(&row.state))));
        }
        let by = string(args, "by").unwrap_or("user").to_string();
        self.stop(&row.id, "stopped", "stopped", Some(&by), false);
        Ok(Map::from_iter([("run".to_string(), json!(row.id)), ("state".to_string(), json!("stopped"))]))
    }

    pub fn status(self: &Rc<Self>, pane: &str, status: Option<&str>, ended: bool) {
        let status = status.and_then(Status::parse);
        let now = Instant::now();
        let mut turns = self.turns.borrow_mut();
        let Some(turn) = turns.get_mut(pane) else { return };
        if ended {
            turn.reported_end(status, now);
        } else {
            turn.see(status, now);
        }
        drop(turns);
        if status == Some(Status::Working) {
            let waiting = self.live.borrow().iter().find(|l| l.row.state == "waiting" && l.visit.as_ref().and_then(|v| v.row.terminal.as_deref()) == Some(pane)).map(|l| l.row.id.clone());
            if let Some(id) = waiting {
                self.update(&id, |live| {
                    live.row.state = "running".into();
                    live.doc.why = None;
                    Some(("resumed", None, json!({ "stage": live.visit.as_ref().map(|v| v.row.stage.clone()) })))
                });
            }
        }
        self.soon.set(true);
    }

    pub fn tick(self: &Rc<Self>) -> Option<Instant> {
        let now = Instant::now();
        self.soon.set(false);
        let ids: Vec<String> = self.live.borrow().iter().filter(|l| l.row.state == "running" || (l.row.state == "waiting" && l.visit.is_some())).map(|l| l.row.id.clone()).collect();
        for id in &ids {
            self.step(id);
        }
        self.live.borrow_mut().retain(|l| LIVE.contains(&l.row.state.as_str()));
        let busy = self.live.borrow().iter().any(|l| l.row.state == "running" || l.visit.is_some());
        let turns = self.turns.borrow().values().filter_map(|t| t.due(TURN_SETTLE)).min();
        let soon = self.soon.get().then_some(now);
        [busy.then(|| now + TICK), turns, soon].into_iter().flatten().min()
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
    }

    fn step(self: &Rc<Self>, id: &str) {
        let Some(core) = self.core.upgrade() else { return };
        let (visit, next, state) = {
            let live = self.live.borrow();
            let Some(live) = live.iter().find(|l| l.row.id == id) else { return };
            (live.visit.clone(), live.doc.next.clone(), live.row.state.clone())
        };
        let Some(visit) = visit else {
            if let (Some(next), "running") = (next, state.as_str()) {
                self.begin(id, &next);
            }
            return;
        };
        if visit.opening || visit.ending {
            return;
        }
        let Some(pane) = visit.row.terminal.clone() else { return };
        let late = visit.within.is_some_and(|within| now() - visit.row.started_at >= within.as_secs_f64());
        match &visit.work {
            Work::Agent { agent } => {
                if core.pane(&pane).is_none() {
                    return self.interrupted(id, &visit);
                }
                if !alive(&core, &pane) {
                    return self.agent_ended(id, &pane, agent);
                }
                if late {
                    return self.timed_out(id, &pane, agent);
                }
                let ended = {
                    let mut turns = self.turns.borrow_mut();
                    let turn = turns.entry(pane.clone()).or_insert_with(|| {
                        let mut turn = Turn::underway();
                        turn.see(core.pane(&pane).and_then(|p| p.status).as_deref().and_then(Status::parse), Instant::now());
                        turn
                    });
                    turn.ended(Instant::now(), TURN_SETTLE)
                };
                match ended {
                    Some(Status::NeedsYou) if state != "waiting" => {
                        self.turns.borrow_mut().insert(pane.clone(), Turn::default());
                        let why = format!("{}'s agent needs you", visit.row.stage);
                        self.wait(id, &why);
                    }
                    Some(Status::NeedsYou) => {}
                    Some(_) => self.agent_ended(id, &pane, agent),
                    None => {}
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
        }
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
        self.turns.borrow_mut().remove(pane);
        if let Some(core) = self.core.upgrade() {
            core.handle(json!({ "cmd": "close", "args": { "pane": pane } }), |_| {});
        }
    }

    fn agent_ended(self: &Rc<Self>, id: &str, pane: &str, agent: &str) {
        if !self.mark_ending(id) {
            return;
        }
        let (session, worker, started) = {
            let live = self.live.borrow();
            let Some(visit) = live.iter().find(|l| l.row.id == id).and_then(|l| l.visit.as_ref()) else { return };
            (visit.row.session.clone(), visit.row.worktree.clone().unwrap_or_default(), visit.row.started_at)
        };
        let since = UNIX_EPOCH + Duration::from_secs_f64((started - 1.0).max(0.0));
        let read = match (tasks::program_name(agent), &session) {
            ("claude", Some(session)) => claude_answer(&self.home.join(".claude"), session),
            ("codex", _) => {
                let codex = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()).map_or_else(|| self.home.join(".codex"), PathBuf::from);
                codex_answer(&codex.join("sessions"), Path::new(&worker), since)
            }
            _ => None,
        };
        let note = read.is_none().then(|| format!("{agent}'s session file couldn't be read, so its answer is empty"));
        if let Some(note) = &note {
            log!("kanna-daemon: {id}: {note}");
        }
        let answer = read.unwrap_or_default();
        let ended = Ended { verdict: None, shown: "done", answer: answer.clone(), feedback: answer, full: String::new(), failures: None, note };
        self.quit_then(id, pane, agent, ended);
    }

    fn quit_then(self: &Rc<Self>, id: &str, pane: &str, agent: &str, mut ended: Ended) {
        let verdicts = {
            let live = self.live.borrow();
            live.iter().find(|l| l.row.id == id).and_then(|l| l.visit.as_ref().and_then(|v| l.pipeline.stage(&v.row.stage)).map(Stage::verdicts)).unwrap_or(false)
        };
        if verdicts && ended.verdict.is_none() {
            let verdict = kanna_pipe::verdict_of(&ended.answer);
            ended.verdict = Some(verdict);
            ended.shown = verdict.name();
        }
        let (this, id, closing) = (Rc::downgrade(self), id.to_string(), pane.to_string());
        let finish = move |_: Option<String>| {
            let Some(this) = this.upgrade() else { return };
            this.close(&closing);
            this.conclude(&id, ended);
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
        let (worker, start) = {
            let live = self.live.borrow();
            let Some(visit) = live.iter().find(|l| l.row.id == id).and_then(|l| l.visit.as_ref()) else { return };
            (visit.row.worktree.clone().unwrap_or_default(), string(&visit.row.doc, "start").unwrap_or("").to_string())
        };
        if !start.is_empty() {
            let _ = git(Path::new(&worker), &["reset", "--hard", &start]);
        }
        let ended = Ended { verdict: Some(Verdict::Fail), shown: "fail", answer: String::new(), feedback: "timed out".into(), full: String::new(), failures: None, note: Some("timed out".into()) };
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
        let ended = Ended { verdict: Some(verdict), shown: verdict.name(), answer: String::new(), feedback, full: log.to_string_lossy().into_owned(), failures: Some(failure_lines(&output)), note };
        let (this, id, closing) = (Rc::downgrade(self), id.to_string(), pane.to_string());
        let finish = move || {
            let Some(this) = this.upgrade() else { return };
            this.close(&closing);
            this.conclude(&id, ended);
        };
        match self.core.upgrade().filter(|core| alive(core, pane)) {
            Some(core) => core.end(pane, move |_| finish()),
            None => finish(),
        }
    }

    fn interrupted(self: &Rc<Self>, id: &str, visit: &Visit) {
        log!("kanna-daemon: {id}'s {} visit lost its terminal, so it's recorded interrupted and runs again", visit.row.stage);
        let mut row = visit.row.clone();
        row.ended_at = Some(now());
        row.verdict = Some("interrupted".into());
        let _ = self.db.put_stage(&row);
        let stage = row.stage.clone();
        self.update(id, |live| {
            live.visit = None;
            live.doc.next = Some(stage.clone());
            Some(("verdict", Some("daemon".into()), json!({ "stage": stage, "visit": row.visit, "verdict": "interrupted" })))
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
        let files = tasks::config_files(Path::new(&row.repo));
        let task = self.tasks.upgrade().and_then(|t| t.entry(&row.task));
        let mut vars: Vec<(&str, String)> = vec![
            ("KANNA_TASK", row.task.clone()),
            ("KANNA_TASK_NAME", task.as_ref().and_then(|t| t.name.clone()).unwrap_or_default()),
            ("KANNA_REPO", row.repo.clone()),
            ("KANNA_WORKTREE", doc.worker.clone()),
            ("KANNA_BRANCH", row.branch.clone()),
            ("KANNA_BASE", doc.base.clone()),
            ("KANNA_RUN", id.to_string()),
            ("KANNA_STAGE", name.to_string()),
        ];
        let (command, work, sessions) = match &stage.kind {
            Kind::Agent(agent) => {
                let prompts = self.db.revisions(id).ok().and_then(|r| r.last().and_then(|r| r.doc.get("prompts").cloned())).unwrap_or(Value::Null);
                let file = stage.prompt().and_then(|p| string(&prompts, p).map(String::from)).or_else(|| stage.prompt().and_then(|p| fs::read_to_string(worker.join(format!(".kanna/prompts/{p}.md"))).ok())).unwrap_or_default();
                let text = prompt(&stage, &file, &doc.request, doc.feedback.as_ref(), &doc.answers);
                let path = scratch.join(format!("stage-{n}.md"));
                if let Err(error) = fs::write(&path, &text) {
                    return self.stop(id, "stopped", &format!("can't write {}: {error}", path.display()), Some("daemon"), true);
                }
                let session = tasks::names_its_session(&agent.agent).then(uuid).flatten();
                let first = format!("\"$(cat {})\"", quote(&format!("{SCRATCH}/stage-{n}.md")));
                let exports: Vec<String> = vars.iter().map(|(k, v)| format!("{k}={}", quote(v))).collect();
                let command = agent_command(&agent.agent, std::env::var("KANNA_PIPELINE_AGENTS").ok().as_deref());
                let line = format!("export {}; {}", exports.join(" "), tasks::stage_agent(&command, agent.effort.as_deref(), agent.model.as_deref(), tasks::prompts(&files), session.as_deref(), &first));
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
            Kind::Merge(Merge::Main) => match tasks::land(&files) {
                Some(land) => {
                    let (log, exit) = (scratch.join(format!("stage-{n}.log")), scratch.join(format!("stage-{n}.exit")));
                    let _ = fs::remove_file(&exit);
                    let timeout = stage.within.map_or_else(|| tasks::scripts(&files).timeout, |w| w.as_secs().max(1));
                    stage_row.kind = "merge".into();
                    row_doc["log"] = json!(log.to_string_lossy());
                    row_doc["exit"] = json!(exit.to_string_lossy());
                    row_doc["land"] = json!(land);
                    (task_scripts::stage_command(&vars, &land, timeout, &log, &exit), Work::Command { log, exit }, 0)
                }
                None => {
                    stage_row.kind = "merge".into();
                    stage_row.doc = row_doc;
                    return self.merge_main(id, stage_row, &row);
                }
            },
            _ => return self.stop(id, "stopped", &format!("{name} is a kind of stage runs can't do yet"), Some("daemon"), true),
        };
        stage_row.doc = row_doc;
        let within = stage.within;
        let visit_row = stage_row.clone();
        self.update(id, |live| {
            live.doc.starts.entry(name.to_string()).or_insert(head.clone());
            live.doc.visits = n;
            live.doc.sessions += sessions;
            live.visit = Some(Visit { row: visit_row, work: work.clone(), within, opening: true, ending: false });
            None
        });
        let Some(core) = self.core.upgrade() else { return };
        let (this, id) = (Rc::downgrade(self), id.to_string());
        core.handle(json!({ "cmd": "open", "args": { "type": "term", "hidden": true, "cwd": doc.worker, "cmd": command, "wait": false } }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            let pane = reply.get("pane").and_then(|p| string(p, "id")).map(String::from);
            let Some(pane) = pane.filter(|_| reply.get("ok") == Some(&Value::Bool(true))) else {
                let why = reply.get("error").and_then(|e| string(e, "message")).unwrap_or("it failed").to_string();
                this.update(&id, |live| {
                    live.visit = None;
                    None
                });
                return this.stop(&id, "stopped", &format!("can't open a terminal for {}: {why}", stage_row.stage), Some("daemon"), true);
            };
            stage_row.terminal = Some(pane.clone());
            let saved = this.db.put_stage(&stage_row);
            if let Ok(row_id) = saved {
                stage_row.id = row_id;
            }
            if matches!(work, Work::Agent { .. }) {
                this.turns.borrow_mut().insert(pane.clone(), Turn::default());
            }
            log!("kanna-daemon: {id} starts {} (visit {}) in {pane}", stage_row.stage, stage_row.visit);
            let event = json!({ "stage": stage_row.stage, "visit": stage_row.visit, "kind": stage_row.kind, "terminal": pane });
            this.update(&id, |live| {
                if let Some(visit) = live.visit.as_mut() {
                    visit.row = stage_row.clone();
                    visit.opening = false;
                }
                live.doc.next = None;
                Some(("stage", Some("daemon".into()), event))
            });
            this.soon.set(true);
        });
    }

    fn merge_main(self: &Rc<Self>, id: &str, mut stage_row: StageRow, row: &RunRow) {
        let repo = Path::new(&row.repo);
        let message = format!("Merge {} (run {id}, pipeline {})", row.branch, row.pipeline);
        let merged = git(repo, &["merge", "--no-ff", "-m", &message, &row.branch]);
        let ended = match merged {
            Ok(_) => Ended { verdict: Some(Verdict::Pass), shown: "pass", answer: String::new(), feedback: String::new(), full: String::new(), failures: None, note: None },
            Err(error) => {
                let conflicts = git(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
                let _ = git(repo, &["merge", "--abort"]);
                let feedback = if conflicts.is_empty() { format!("merging {} failed: {error}", row.branch) } else { format!("merging {} conflicts in:\n{conflicts}", row.branch) };
                Ended { verdict: Some(Verdict::Fail), shown: "fail", answer: String::new(), feedback, full: String::new(), failures: None, note: None }
            }
        };
        if let Ok(row_id) = self.db.put_stage(&stage_row) {
            stage_row.id = row_id;
        }
        let event = json!({ "stage": stage_row.stage, "visit": stage_row.visit, "kind": "merge" });
        self.update(id, |live| {
            live.doc.next = None;
            live.doc.visits += 1;
            live.visit = Some(Visit { row: stage_row.clone(), work: Work::Command { log: PathBuf::new(), exit: PathBuf::new() }, within: None, opening: false, ending: true });
            Some(("stage", Some("daemon".into()), event))
        });
        self.conclude(id, ended);
    }

    fn conclude(self: &Rc<Self>, id: &str, ended: Ended) {
        let worker_head = |worker: &str| git(Path::new(worker), &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let Some((mut row, kind, pipeline, mut doc)) = ({
            let live = self.live.borrow();
            live.iter().find(|l| l.row.id == id).and_then(|l| l.visit.as_ref().map(|v| (v.row.clone(), v.row.kind.clone(), l.pipeline.clone(), l.doc.clone())))
        }) else {
            return;
        };
        let end = worker_head(&doc.worker);
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
        row.doc = json!({ "answer": answer, "feedback": ended.feedback, "failures": ended.failures, "note": opt(ended.note.as_deref()), "end": end, "commits": commits });
        if let Err(error) = self.db.put_stage(&row) {
            log!("kanna-daemon: couldn't save {id}'s {} visit: {error}", row.stage);
        }
        let stage = row.stage.clone();
        if row.kind == "agent" {
            doc.answers.insert(stage.clone(), ended.answer.clone());
            for (key, value) in outputs(&ended.answer) {
                doc.answers.insert(format!("{stage}.{key}"), value);
            }
        }
        if kind == "merge" && ended.verdict == Some(Verdict::Pass) {
            doc.landed = true;
        }
        let next = decide(&pipeline, &mut doc, &stage, ended.verdict, ended.failures.clone());
        let arrow = match &next {
            Next::Stage { name, .. } => json!(name),
            Next::Done => json!(DONE),
            Next::Stuck => Value::Null,
        };
        log!("kanna-daemon: {id}: {stage} ended {} → {}", ended.shown, arrow.as_str().unwrap_or("no arrow"));
        doc.feedback = match (&next, ended.verdict) {
            (Next::Stage { reset: false, .. }, Some(Verdict::Fail)) => Some(Feedback { stage: stage.clone(), text: ended.feedback.clone(), full: ended.full.clone() }),
            _ => None,
        };
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
            live.doc = doc.clone();
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
            }
            if let (Some(pane), false) = (row.terminal.clone(), visit.ending) {
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
        if tell {
            let branch = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.branch.clone()).unwrap_or_default();
            self.tell(id, &format!("{why}. The run's branch {branch} is kept for you to look at; `kanna run show {id}` has its visits."));
        }
    }

    fn hand_back(self: &Rc<Self>, id: &str) {
        let Some((row, doc)) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| (l.row.clone(), l.doc.clone())) else { return };
        let folder = self.tasks.upgrade().and_then(|t| t.entry(&row.task)).and_then(|e| e.folder);
        let note = match folder.as_deref().map(Path::new) {
            None => format!("the task has no worktree, so {} is there for you to merge", row.branch),
            Some(folder) => match git(folder, &["status", "--porcelain", "--untracked-files=no"]) {
                Ok(changes) if !changes.is_empty() => format!("your worktree has changes, so it's left as it is: `git merge --ff-only {}` when you're ready", row.branch),
                Err(error) => format!("git status failed in your worktree ({error}), so it's left as it is: `git merge --ff-only {}`", row.branch),
                Ok(_) => match git(folder, &["merge", "--ff-only", &row.branch]) {
                    Ok(_) => format!("your branch is fast-forwarded to {}", row.branch),
                    Err(_) => format!("your branch has moved since the run started, so it's left as it is: merge {} yourself", row.branch),
                },
            },
        };
        let worker = Path::new(&doc.worker);
        if worker.is_dir() {
            if let Err(error) = git(Path::new(&row.repo), &["worktree", "remove", &doc.worker]) {
                log!("kanna-daemon: {id}'s worktree {} is kept: {error}", doc.worker);
            }
        }
        let stages = self.db.stages(id).unwrap_or_default();
        let distinct: std::collections::BTreeSet<&str> = stages.iter().map(|s| s.stage.as_str()).collect();
        let took = clock(Duration::from_secs_f64((now() - row.started_at).max(0.0)));
        let (state, outcome, word) = if doc.landed { ("landed", "landed", "landed") } else { ("done", "handed back", "handed back") };
        let summary = format!("{} visits of {} stages, {took}, {} sessions", stages.len(), distinct.len(), doc.sessions);
        let landed_line = if doc.landed { format!("{} is merged on main; ", row.branch) } else { String::new() };
        log!("kanna-daemon: {id} {word}: {summary}; {note}");
        let note_saved = note.clone();
        self.update(id, |live| {
            live.row.state = state.into();
            live.row.ended_at = Some(now());
            live.row.outcome = Some(outcome.into());
            live.doc.note = Some(note_saved.clone());
            None
        });
        if doc.landed {
            let _ = self.db.run_event(id, "landed", Some("daemon"), &json!({ "branch": row.branch }));
        }
        let _ = self.db.run_event(id, "handed_back", Some("daemon"), &json!({ "note": note }));
        self.tell(id, &format!("{word}: {summary}. {landed_line}{note}. `kanna run show {id}` has its visits."));
    }

    fn tell(&self, id: &str, text: &str) {
        let Some(row) = self.live.borrow().iter().find(|l| l.row.id == id).map(|l| l.row.clone()) else { return };
        let Ok(tasks) = self.tasks() else { return };
        if let Err(error) = tasks.post(&row.task, Some(&row.repo), id, text) {
            log!("kanna-daemon: couldn't tell {} about {id}: {}", row.task, error.message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn pipeline(text: &str) -> Pipeline {
        kanna_pipe::parse(text).unwrap_or_else(|e| panic!("{e:?}"))
    }

    fn mock() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/mock-agent/mock-agent").canonicalize().unwrap()
    }

    #[test]
    fn a_visit_s_answer_is_read_from_the_mock_agent_s_session_file() {
        let dir = std::env::temp_dir().join(format!("kanna-runs-answer-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (home, bin, work) = (dir.join("home"), dir.join("bin"), dir.join("work"));
        for folder in [&home, &bin, &work] {
            fs::create_dir_all(folder).unwrap();
        }
        std::os::unix::fs::symlink(mock(), bin.join("claude")).unwrap();
        let session = "11111111-2222-4333-8444-555555555555";
        let stage = "Review it.\nmock: work 0.1\nsay: Looks right.\nsay: VERDICT: PASS\n";
        let out = Command::new(bin.join("claude"))
            .args(["--session-id", session, stage])
            .current_dir(&work)
            .env("HOME", &home)
            .env_remove("KANNA_AGENTS_HOME")
            .env("KANNA_STAGE", "review")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(b"exit\n")?;
                child.wait()
            })
            .unwrap();
        assert!(out.success());
        let answer = claude_answer(&home.join(".claude"), session).expect("the session file");
        assert_eq!(answer, "Looks right.\nVERDICT: PASS");
        assert_eq!(kanna_pipe::verdict_of(&answer), Verdict::Pass);
        assert_eq!(claude_answer(&home.join(".claude"), "00000000-0000-4000-8000-000000000000"), None, "an unknown session gives no answer");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_answer_is_the_last_turn_s_text_after_its_last_tool_result() {
        let jsonl = [
            r#"{"type":"user","message":{"role":"user","content":"Build it."}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done: it builds."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"VERDICT: PASS"}]}}"#,
        ]
        .join("\n");
        assert_eq!(claude_text(&jsonl), "Done: it builds.\n\nVERDICT: PASS");
        let rollout = [
            r#"{"type":"session_meta","payload":{"id":"x","cwd":"/w"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"first"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"VERDICT: FAIL"}]}}"#,
        ]
        .join("\n");
        assert_eq!(codex_text(&rollout), "VERDICT: FAIL");
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
        let text = prompt(stage, "Fix what failed.\n{{build.answer}}", "Add a button", Some(&Feedback { stage: "check".into(), text: "FAILED a".into(), full: String::new() }), &BTreeMap::from([("build".into(), "built it".into())]));
        assert_eq!(text, "Fix what failed.\nbuilt it\n\n# The task\n\nAdd a button\n\n# Feedback from check\n\nFAILED a\n");
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
    fn a_pipeline_with_parts_runs_can_t_do_yet_is_refused_by_line() {
        let p = pipeline("plan = claude\nbuild = claude each plan.parts\nship = ask \"Land?\"\njoin = merge build\nplan -> build -> join -> ship -> done\n");
        assert_eq!(unbuilt(&p), vec!["2: build uses each, which runs can't do yet", "3: ship uses ask, which runs can't do yet", "4: join uses merge STAGE, which runs can't do yet"]);
    }

    #[test]
    fn a_stage_s_agent_is_its_name_unless_a_test_points_it_elsewhere() {
        assert_eq!(agent_command("claude", None), "claude");
        assert_eq!(agent_command("claude", Some("codex=/x/codex claude=/fake/claude")), "/fake/claude");
        assert_eq!(agent_command("copilot", Some("claude=/fake/claude")), "copilot");
    }

    #[test]
    fn named_outputs_are_kept_by_their_key() {
        assert_eq!(outputs("Plan:\n```json name=parts\n[\"a\", \"b\"]\n```\nok"), vec![("parts".to_string(), "[\"a\", \"b\"]".to_string())]);
    }

    #[test]
    fn a_run_s_doc_round_trips() {
        let doc = Doc { request: "r".into(), taken: vec![1, 0], feedback: Some(Feedback { stage: "check".into(), text: "t".into(), full: "/f".into() }), failures: BTreeMap::from([("check".into(), vec!["x".into()])]), sessions: 2, next: Some("fix".into()), revision: 1, ..Doc::default() };
        assert_eq!(Doc::from_value(&doc.to_value()), doc);
    }
}
