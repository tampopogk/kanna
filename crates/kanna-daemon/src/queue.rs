use crate::db::{Db, QueueRow};
use crate::runs::{self, Runs, Started};
use crate::task_events;
use crate::tasks::{self, git, Tasks};
use crate::test_results;
use kanna_mux::json::{json, num, opt, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Responder};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const PIPELINE: &str = "merge-queue";

const ATTEMPTS: u64 = 3;

const RETRY: Duration = Duration::from_secs(30);

const LIVE: &[&str] = &["queued", "testing", "landing"];

const FAILURE_LINES: usize = 40;

pub const WORKER: &str = "merge-queue";

const ALONE_MAX: usize = 10;

const RERUNNABLE_KEPT: usize = 50;

const ALONE_SCRIPT: &str = ".kanna/tmp/merge-queue-alone.sh";

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn failed(error: impl std::fmt::Display) -> KmuxError {
    KmuxError::new("failed", error.to_string())
}

fn text(doc: &Value, key: &str) -> String {
    string(doc, key).unwrap_or("").to_string()
}

pub fn main_branch(repo: &Path) -> Option<String> {
    git(repo, &["branch", "--show-current"]).ok().filter(|b| !b.is_empty())
}

pub fn test_command(repo: &Path) -> Option<String> {
    let file = repo.join(".kanna/config.json");
    let value: Value = serde_json::from_str(&fs::read_to_string(file).ok()?).ok()?;
    value.get("merge")?.get("test")?.as_str().map(str::trim).filter(|c| !c.is_empty()).map(String::from)
}

pub fn test_within(repo: &Path) -> Option<u64> {
    let file = repo.join(".kanna/config.json");
    let value: Value = serde_json::from_str(&fs::read_to_string(file).ok()?).ok()?;
    value.get("merge")?.get("within")?.as_u64().filter(|s| *s > 0)
}

pub fn pipeline_text(folder: &Path, repo: &Path) -> Option<(String, String)> {
    let own = folder.join(format!(".kanna/pipelines/{PIPELINE}.kojo"));
    if let Ok(text) = fs::read_to_string(&own) {
        return Some((text, own.display().to_string()));
    }
    let command = test_command(repo)?;
    Some((test_pipeline(&command, test_within(repo), "test"), format!("Kanna's {PIPELINE}.kojo (merge.test)")))
}

fn prompts(folder: &Path) -> BTreeMap<String, String> {
    fs::read_dir(folder.join(".kanna/prompts"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), fs::read_to_string(&p).ok()?)))
        .collect()
}

pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

pub struct Queue {
    db: Rc<Db>,
    tasks: Weak<Tasks>,
    runs: Weak<Runs>,
    live: RefCell<Vec<QueueRow>>,
    soon: Cell<bool>,
    retry: Cell<Option<Instant>>,
}

impl Queue {
    pub fn open(db: Rc<Db>, tasks: Weak<Tasks>, runs: Weak<Runs>) -> Queue {
        let live = db.queued(true).unwrap_or_else(|error| {
            log!("kanna-daemon: can't read the merge queue in kanna.db: {error}");
            vec![]
        });
        let mut live = live;
        for row in live.iter_mut() {
            patch(&mut row.doc, json!({ "settled": true }));
        }
        for row in &live {
            log!("kanna-daemon: {} carries on in the merge queue ({}, {})", row.id, row.task, row.state);
        }
        Queue { db, tasks, runs, live: RefCell::new(live), soon: Cell::new(true), retry: Cell::new(None) }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle, runs: &Rc<Runs>) {
        let ending = Rc::downgrade(self);
        *runs.on_end.borrow_mut() = Some(Box::new(move |run, outcome| {
            if let Some(this) = ending.upgrade() {
                this.run_ended(run, outcome);
            }
        }));
        let this = Rc::downgrade(self);
        core.add_command(
            "queue",
            Rc::new(move |args, responder: Responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                let reply = this.handle(&args);
                responder.reply(reply);
            }),
        );
    }

    fn handle(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        match string(args, "action") {
            Some("merge") if args.get("cancel") == Some(&Value::Bool(true)) => self.cancel(args),
            Some("add" | "merge") => self.add(args),
            Some("cancel") => self.cancel(args),
            Some("list") => self.list(args),
            other => Err(bad(format!("unknown queue action {other:?} (add, cancel or list)"))),
        }
    }

    fn tasks(&self) -> Result<Rc<Tasks>, KmuxError> {
        self.tasks.upgrade().ok_or_else(|| KmuxError::new("app_gone", "the daemon is stopping"))
    }

    fn task_named(&self, args: &Value) -> Result<String, KmuxError> {
        let tasks = self.tasks()?;
        if let Some(reference) = string(args, "task").filter(|t| !t.is_empty()) {
            return Ok(reference.to_string());
        }
        let by = string(args, "by").and_then(|by| tasks.task_of(by));
        by.or_else(|| string(args, "cwd").and_then(|cwd| tasks.task_in_folder(Path::new(cwd))).map(|e| e.id))
            .ok_or_else(|| bad("which task? run it in the task's terminal, or name it: kanna task merge TASK"))
    }

    fn add(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let tasks = self.tasks()?;
        let reference = self.task_named(args)?;
        let candidate = tasks.merge_candidate(&reference)?;
        let entry = candidate.entry.clone();
        let short = tasks.short_of(&entry.id);
        if let Some(queued) = &entry.queued {
            return Err(KmuxError::new("queued", format!("{short} is in the merge queue already ({queued})")));
        }
        let folder = PathBuf::from(entry.folder.clone().ok_or_else(|| KmuxError::new("not_started", format!("{short} hasn't started: there's nothing to merge")))?);
        let repo = PathBuf::from(&entry.repo);
        if !entry.bases.is_empty() {
            let names: Vec<String> = entry.bases.iter().map(|b| tasks.label_for(&b.id)).collect();
            return Err(KmuxError::new("merges_after", format!("{short} merges after {}, which isn't in main yet: merge it once that lands (you're told when it does)", names.join(", "))));
        }
        let dirty = git(&folder, &["diff", "--name-only", "HEAD"]).map_err(failed)?;
        if !dirty.is_empty() {
            let files: Vec<&str> = dirty.lines().collect();
            return Err(KmuxError::new("uncommitted", format!("{short} has uncommitted changes to {} file{} ({}): commit them first", files.len(), if files.len() == 1 { "" } else { "s" }, files.join(", "))));
        }
        let main = main_branch(&repo).ok_or_else(|| failed(format!("{} has no branch checked out to merge into", repo.display())))?;
        let head = git(&folder, &["rev-parse", "--verify", "HEAD"]).map_err(failed)?;
        let main_head = git(&repo, &["rev-parse", "--verify", &main]).map_err(failed)?;
        let ahead: u64 = git(&repo, &["rev-list", "--count", &format!("{main_head}..{head}")]).ok().and_then(|n| n.parse().ok()).unwrap_or(0);
        if ahead == 0 {
            return Err(KmuxError::new("nothing", format!("{short} has no commits that aren't in {main}")));
        }
        let conflicts = task_events::conflicts(&repo, &main_head, &head).map_err(failed)?;
        if !conflicts.is_empty() {
            return Err(KmuxError::new("conflicts", format!("{short} conflicts with {main} in {}: merge {main} into your branch, fix them, rerun your tests and commit, then kanna task merge again", conflicts.join(", "))));
        }
        if pipeline_text(&folder, &repo).is_none() {
            return Err(KmuxError::new("no_tests", "this repo has no tests for the merge queue: set merge.test in .kanna/config.json (kanna config schema), or write .kanna/pipelines/merge-queue.kojo"));
        }
        let branch = git(&folder, &["branch", "--show-current"]).unwrap_or_default();
        let name = entry.name.clone().unwrap_or_else(|| short.clone());
        let message = string(args, "message").map(str::trim).filter(|m| !m.is_empty()).map_or_else(|| format!("Merge branch '{branch}': {name}"), |m| if m.starts_with("Merge ") { m.to_string() } else { format!("Merge branch '{branch}': {m}") });
        let by = string(args, "by").unwrap_or("user").to_string();
        let id = self.db.new_queue_id().map_err(failed)?;
        let row = QueueRow { id: id.clone(), repo: entry.repo.clone(), task: entry.id.clone(), head: head.clone(), state: "queued".into(), queued_at: now(), ended_at: None, run: None, doc: json!({ "message": message, "by": by, "branch": branch, "name": name, "attempt": 1 }) };
        self.db.put_queued(&row).map_err(failed)?;
        let ahead_of: Vec<QueueRow> = self.live.borrow().iter().filter(|q| q.repo == entry.repo).cloned().collect();
        self.live.borrow_mut().push(row);
        log!("kanna-daemon: {} goes into {}'s merge queue as {id}, at {head}", entry.id, entry.repo);
        let weak = Rc::downgrade(self);
        let settling = id.clone();
        tasks.enqueue(candidate, &id, Some(by), move || {
            if let Some(this) = weak.upgrade() {
                this.settle(&settling);
            }
        });
        self.soon.set(true);
        let position = ahead_of.len() + 1;
        let after = ahead_of.last().map(|q| text(&q.doc, "name"));
        let said = match &after {
            Some(after) => format!("{short} \"{name}\" is {} in the merge queue, after \"{after}\": it lands on {main} once the tests pass on top of it. Its agent quits now and its tabs close; if the tests fail it comes back to the Shelf and its agent gets the log.", ordinal(position)),
            None => format!("{short} \"{name}\" is next in the merge queue: it lands on {main} once the tests pass. Its agent quits now and its tabs close; if the tests fail it comes back to the Shelf and its agent gets the log."),
        };
        Ok(Map::from_iter([("queue".to_string(), json!(id)), ("task".to_string(), json!(entry.id)), ("position".to_string(), json!(position)), ("head".to_string(), json!(head)), ("said".to_string(), json!(said))]))
    }

    fn cancel(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let tasks = self.tasks()?;
        let reference = self.task_named(args)?;
        let id = tasks.resolve(&reference)?.ok_or_else(|| KmuxError::new("not_found", format!("no task {reference}")))?;
        let row = self.live.borrow().iter().find(|q| q.task == id).cloned().ok_or_else(|| KmuxError::new("not_queued", format!("{} isn't in the merge queue", tasks.short_of(&id))))?;
        let by = string(args, "by").unwrap_or("user").to_string();
        self.take_out(&row.id, &by);
        tasks.unqueue(&id, Some(&by));
        Ok(Map::from_iter([("queue".to_string(), json!(row.id)), ("task".to_string(), json!(id)), ("state".to_string(), json!("cancelled"))]))
    }

    fn settle(&self, queue: &str) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.id == queue).cloned() else { return };
        patch(&mut row.doc, json!({ "settled": true }));
        self.save(&row);
        self.soon.set(true);
    }

    fn take_out(self: &Rc<Self>, queue: &str, why: &str) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.id == queue).cloned() else { return };
        log!("kanna-daemon: {queue} leaves the merge queue: cancelled by {why}");
        row.state = "cancelled".into();
        row.ended_at = Some(now());
        self.save(&row);
        if let (Some(run), Some(runs)) = (row.run.clone(), self.runs.upgrade()) {
            runs.stop_owned(&run, "taken out of the merge queue");
        }
        self.release_worker(&row, true);
        self.soon.set(true);
    }

    pub fn task_closed(self: &Rc<Self>, task: &str) {
        let found = self.live.borrow().iter().find(|q| q.task == task && q.state != "landing").map(|q| q.id.clone());
        if let Some(queue) = found {
            self.take_out(&queue, "the task's Done");
        }
    }

    fn list(&self, args: &Value) -> Result<Map, KmuxError> {
        let repo = string(args, "repo");
        let rows: Vec<Value> = self.db.queued(args.get("all") != Some(&Value::Bool(true))).map_err(failed)?.iter().filter(|q| repo.is_none_or(|r| q.repo == r)).map(row_value).collect();
        Ok(Map::from_iter([("queue".to_string(), Value::Array(rows))]))
    }

    fn save(&self, row: &QueueRow) {
        if let Err(error) = self.db.put_queued(row) {
            log!("kanna-daemon: couldn't save {} in kanna.db: {error}", row.id);
        }
        let mut live = self.live.borrow_mut();
        match LIVE.contains(&row.state.as_str()) {
            true => {
                if let Some(at) = live.iter_mut().find(|q| q.id == row.id) {
                    *at = row.clone();
                }
            }
            false => live.retain(|q| q.id != row.id),
        }
        drop(live);
        if let Some(tasks) = self.tasks.upgrade() {
            tasks.queue_moved();
        }
    }

    pub fn tick(self: &Rc<Self>) -> Option<Instant> {
        let due = self.retry.get().is_some_and(|at| at <= Instant::now());
        if self.soon.replace(false) || due {
            self.retry.set(None);
            self.step();
        }
        if self.soon.get() {
            return Some(Instant::now());
        }
        self.retry.get()
    }

    fn step(self: &Rc<Self>) {
        let rows = self.live.borrow().clone();
        let mut repos: Vec<String> = vec![];
        for row in &rows {
            if !repos.contains(&row.repo) {
                repos.push(row.repo.clone());
            }
        }
        for repo in repos {
            let mine: Vec<&QueueRow> = rows.iter().filter(|q| q.repo == repo).collect();
            if let Some(landing) = mine.iter().find(|q| q.state == "landing") {
                self.land(&landing.id);
                continue;
            }
            if let Some(testing) = mine.iter().find(|q| q.state == "testing") {
                self.check_run(testing);
                continue;
            }
            if let Some(next) = mine.iter().find(|q| q.state == "queued") {
                if next.doc.get("settled") == Some(&Value::Bool(true)) {
                    self.begin(&next.id);
                }
            }
        }
    }

    fn check_run(self: &Rc<Self>, row: &QueueRow) {
        let Some(run) = row.run.clone() else { return };
        let found = self.db.runs(Some(&row.repo)).ok().and_then(|r| r.into_iter().find(|r| r.id == run));
        match found {
            Some(r) if runs::LIVE.contains(&r.state.as_str()) => {}
            Some(r) if r.state == "done" => {
                let head = git(Path::new(&row.repo), &["rev-parse", "--verify", &format!("refs/heads/{}", r.branch)]);
                self.run_ended(&run, head);
            }
            Some(r) => self.run_ended(&run, Err(string(&r.doc, "why").unwrap_or("the run stopped").to_string())),
            None => self.run_ended(&run, Err("its run is gone".into())),
        }
    }

    fn begin(self: &Rc<Self>, queue: &str) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.id == queue).cloned() else { return };
        let repo = PathBuf::from(&row.repo);
        let Some(main) = main_branch(&repo) else { return self.fail(&row.id, "the main checkout has no branch checked out", vec![]) };
        let base = match git(&repo, &["rev-parse", "--verify", &main]) {
            Ok(base) => base,
            Err(error) => return self.fail(&row.id, &format!("can't read {main}: {error}"), vec![]),
        };
        let attempt = row.doc.get("attempt").and_then(Value::as_u64).unwrap_or(1);
        let mut n = 1;
        while n < 1000 && git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/queue/{}-{n}", row.task)]).is_ok() {
            n += 1;
        }
        let branch = format!("queue/{}-{n}", row.task);
        let worker = match prepare_worker(&repo, &branch, &base) {
            Ok(worker) => worker,
            Err(error) => return self.fail(&row.id, &format!("can't get the queue's worktree ready: {error}"), vec![]),
        };
        let worktree = Path::new(&worker);
        let message = text(&row.doc, "message");
        patch(&mut row.doc, json!({ "worker": worker, "queueBranch": branch, "base": base, "main": main, "phase": null, "failing": null, "rerunnable": null, "new": null, "flaky": null, "known": null, "fixed": null, "result": null, "mainRun": null, "logs": null }));
        if let Err(error) = git(worktree, &["merge", "--no-ff", "-m", &message, &row.head]) {
            let files = git(worktree, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let _ = git(worktree, &["merge", "--abort"]);
            self.save(&row);
            let why = match files.is_empty() {
                true => format!("it doesn't merge onto {main}: {error}"),
                false => format!("it conflicts with {main} in {}", files.lines().collect::<Vec<_>>().join(", ")),
            };
            return self.fail(&row.id, &why, vec![]);
        }
        let merge = git(worktree, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let own = own_pipeline(worktree);
        patch(&mut row.doc, json!({ "merge": merge, "own": own }));
        let Some((pipeline, shown)) = pipeline_text(worktree, &repo) else {
            self.save(&row);
            return self.fail(&row.id, "this repo has no tests for the merge queue any more: set merge.test, or write .kanna/pipelines/merge-queue.kojo", vec![]);
        };
        patch(&mut row.doc, json!({ "since": now(), "attempt": attempt }));
        let request = message.clone();
        match self.start_phase(&mut row, "merge", pipeline, shown, &merge, request, prompts(worktree)) {
            Ok(run) => log!("kanna-daemon: {} tests {} on {main} {} as {run}", row.id, row.task, short(&base)),
            Err(error) => {
                self.save(&row);
                self.fail(&row.id, &format!("its pipeline can't run: {error}"), vec![]);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn start_phase(&self, row: &mut QueueRow, phase: &str, pipeline: String, shown: String, base: &str, request: String, prompts: BTreeMap<String, String>) -> Result<String, String> {
        let runs = self.runs.upgrade().ok_or("the daemon is stopping")?;
        let started = Started {
            task: row.task.clone(),
            repo: row.repo.clone(),
            worker: text(&row.doc, "worker"),
            branch: text(&row.doc, "queueBranch"),
            base: base.to_string(),
            text: pipeline,
            shown,
            name: PIPELINE.into(),
            request,
            prompts,
            owner: Some(runs::QUEUE.into()),
            by: row.id.clone(),
        };
        let run = runs.start_owned(started).map_err(|e| e.message)?;
        row.state = "testing".into();
        row.run = Some(run.clone());
        let stage = if phase == "merge" { "test" } else { phase };
        patch(&mut row.doc, json!({ "phase": phase, "stage": stage }));
        self.save(row);
        Ok(run)
    }

    fn run_ended(self: &Rc<Self>, run: &str, outcome: Result<String, String>) {
        let Some(row) = self.live.borrow().iter().find(|q| q.run.as_deref() == Some(run)).cloned() else { return };
        if row.state != "testing" {
            return;
        }
        if row.doc.get("own") != Some(&Value::Bool(false)) {
            return self.own_ended(row, run, outcome);
        }
        let phase = row.doc.get("phase").and_then(Value::as_str).unwrap_or("merge").to_string();
        let stage = self.db.stages(run).unwrap_or_default().into_iter().rev().find(|s| s.kind == "run" && matches!(s.verdict.as_deref(), Some("pass" | "fail")));
        let Some(stage) = stage else {
            let why = outcome.err().unwrap_or_else(|| "its run ended without testing".into());
            return self.fail(&row.id, &why, vec![]);
        };
        let path = string(&stage.doc, "log").unwrap_or("").to_string();
        let output = fs::read_to_string(&path).unwrap_or_default();
        let mut row = row;
        self.keep_log(&mut row, if phase == "merge" { "test" } else { &phase }, &path);
        if string(&stage.doc, "note") == Some("timed out") {
            let what = match phase.as_str() {
                "main" => "main's tests",
                "alone" => "running the new failures alone",
                _ => "the tests",
            };
            let lines = feedback_lines(&stage.doc);
            return self.fail(&row.id, &format!("{what} timed out"), lines);
        }
        let passed = stage.verdict.as_deref() == Some("pass");
        match phase.as_str() {
            "main" => self.main_tested(row, &output),
            "alone" => self.alone_tested(row, &output),
            _ => self.merge_tested(row, &output, passed, &stage.doc),
        }
    }

    fn own_ended(self: &Rc<Self>, mut row: QueueRow, run: &str, outcome: Result<String, String>) {
        let last = self.last_log(run);
        if let Some((stage, path, _)) = &last {
            self.keep_log(&mut row, stage, path);
        }
        let lines: Vec<String> = last.as_ref().map(|(_, _, feedback)| feedback.lines().map(String::from).collect()).unwrap_or_default();
        match outcome {
            Ok(_) if self.last_failed(run) => self.fail(&row.id, "its pipeline ended after a stage failed", lines),
            Ok(head) => self.pass(row, &head, None),
            Err(why) => {
                let why = match (self.last_failed(run), &last) {
                    (true, Some((stage, _, _))) => format!("{stage} failed"),
                    _ => why,
                };
                self.fail(&row.id, &why, lines)
            }
        }
    }

    fn merge_tested(self: &Rc<Self>, mut row: QueueRow, output: &str, passed: bool, stage: &Value) {
        let merge = text(&row.doc, "merge");
        let Some(results) = test_results::parse(output) else {
            return match passed {
                true => self.pass(row, &merge, None),
                false => self.fail(&row.id, "test failed", feedback_lines(stage)),
            };
        };
        let failing = results.failing();
        let worker = PathBuf::from(text(&row.doc, "worker"));
        let rerunnable: Vec<Value> = failing.iter().take(RERUNNABLE_KEPT).filter_map(|key| results.alone(key, &worker)).map(|a| json!({ "key": a.key, "name": a.name, "source": a.source, "binary": a.binary, "path": a.path.to_string_lossy(), "cwd": a.cwd.to_string_lossy() })).collect();
        self.put_results(&row, &merge, &results.passing(), &failing, &[]);
        patch(&mut row.doc, json!({ "failing": failing.iter().collect::<Vec<_>>(), "rerunnable": rerunnable, "tests": results.tests.len() }));
        if failing.is_empty() {
            return match passed {
                true => self.pass(row, &merge, Some(json!({ "tests": results.tests.len(), "new": [], "flaky": [], "known": 0, "fixed": [] }))),
                false => self.fail(&row.id, "the tests failed but named no failing test", feedback_lines(stage)),
            };
        }
        let base = text(&row.doc, "base");
        if self.results_at(&row, &base).is_some() {
            patch(&mut row.doc, json!({ "mainRun": "known" }));
            return self.compare(row);
        }
        let repo = PathBuf::from(&row.repo);
        let Some(command) = test_command(&repo) else { return self.fail(&row.id, "merge.test is gone, so main's tests can't run", vec![]) };
        let main = text(&row.doc, "main");
        if let Err(error) = checkout(&worker, &["--detach", &base]) {
            return self.fail(&row.id, &format!("can't check out {main} in the queue's worktree: {error}"), vec![]);
        }
        log!("kanna-daemon: {}'s merge has {} failing tests and {main} {} has no results yet, so main's tests run first", row.id, failing.len(), short(&base));
        patch(&mut row.doc, json!({ "mainRun": "ran" }));
        let pipeline = test_pipeline(&command, test_within(&repo), "main");
        let shown = format!("Kanna's {PIPELINE}.kojo: {main}'s tests at {}", short(&base));
        let request = format!("{main}'s results at {}, to judge {} against", short(&base), text(&row.doc, "name"));
        if let Err(error) = self.start_phase(&mut row, "main", pipeline, shown, &base, request, BTreeMap::new()) {
            self.fail(&row.id, &format!("main's tests can't run: {error}"), vec![]);
        }
    }

    fn main_tested(self: &Rc<Self>, mut row: QueueRow, output: &str) {
        let base = text(&row.doc, "base");
        let results = test_results::parse(output).unwrap_or_default();
        self.put_results(&row, &base, &results.passing(), &results.failing(), &[]);
        let worker = PathBuf::from(text(&row.doc, "worker"));
        if let Err(error) = checkout(&worker, &[&text(&row.doc, "queueBranch")]) {
            return self.fail(&row.id, &format!("can't check the merge out again after main's tests: {error}"), vec![]);
        }
        patch(&mut row.doc, json!({ "phase": "merge" }));
        self.save(&row);
        self.compare(row);
    }

    fn compare(self: &Rc<Self>, mut row: QueueRow) {
        let (base, merge) = (text(&row.doc, "base"), text(&row.doc, "merge"));
        let failing: BTreeSet<String> = strings(&row.doc, "failing");
        let main = self.results_at(&row, &base).unwrap_or_default();
        let on_main: BTreeSet<String> = strings(&main, "failed");
        let passed_here: BTreeSet<String> = self.results_at(&row, &merge).map(|r| strings(&r, "passed")).unwrap_or_default();
        let new: Vec<String> = failing.difference(&on_main).cloned().collect();
        let known = failing.intersection(&on_main).count();
        let fixed: Vec<String> = on_main.intersection(&passed_here).cloned().collect();
        patch(&mut row.doc, json!({ "new": new, "known": known, "fixed": fixed }));
        if new.is_empty() {
            let result = json!({ "tests": row.doc.get("tests").cloned().unwrap_or(Value::Null), "new": [], "flaky": [], "known": known, "fixed": fixed });
            return self.pass(row, &merge, Some(result));
        }
        if new.len() > ALONE_MAX {
            return self.refuse(row, &new, &[], &format!("{} new failures, too many to run again alone", new.len()));
        }
        let rerunnable: Vec<test_results::Alone> = row.doc.get("rerunnable").and_then(Value::as_array).into_iter().flatten().filter(|a| new.iter().any(|n| Some(n.as_str()) == a["key"].as_str())).map(|a| test_results::Alone {
            key: a["key"].as_str().unwrap_or("").into(),
            name: a["name"].as_str().unwrap_or("").into(),
            source: a["source"].as_str().unwrap_or("").into(),
            binary: a["binary"].as_str().unwrap_or("").into(),
            path: PathBuf::from(a["path"].as_str().unwrap_or("")),
            cwd: PathBuf::from(a["cwd"].as_str().unwrap_or("")),
        }).collect();
        if rerunnable.is_empty() {
            return self.refuse(row, &new, &[], "");
        }
        let worker = PathBuf::from(text(&row.doc, "worker"));
        let script = worker.join(ALONE_SCRIPT);
        if let Err(error) = fs::create_dir_all(script.parent().unwrap_or(&worker)).and_then(|_| fs::write(&script, test_results::alone_script(&rerunnable))) {
            return self.fail(&row.id, &format!("can't write {}: {error}", script.display()), vec![]);
        }
        log!("kanna-daemon: {} has {} new failures, so {} of them run again alone", row.id, new.len(), rerunnable.len());
        let within = test_within(Path::new(&row.repo)).map_or(String::new(), |s| format!(" within {s}s"));
        let pipeline = format!("alone = run sh {ALONE_SCRIPT}{within}\nalone -[pass]-> done\n");
        let shown = format!("Kanna's {PIPELINE}.kojo: the new failures, each alone");
        let request = format!("run {} again alone: {}", if rerunnable.len() == 1 { "the new failure" } else { "the new failures" }, rerunnable.iter().map(|a| a.key.as_str()).collect::<Vec<_>>().join(", "));
        if let Err(error) = self.start_phase(&mut row, "alone", pipeline, shown, &merge, request, BTreeMap::new()) {
            self.fail(&row.id, &format!("the new failures can't run alone: {error}"), vec![]);
        }
    }

    fn alone_tested(self: &Rc<Self>, mut row: QueueRow, output: &str) {
        let merge = text(&row.doc, "merge");
        let passed_alone = test_results::parse(output).map(|r| r.passing()).unwrap_or_default();
        let new: Vec<String> = strings(&row.doc, "new").into_iter().collect();
        let flaky: Vec<String> = new.iter().filter(|n| passed_alone.contains(*n)).cloned().collect();
        let still: Vec<String> = new.iter().filter(|n| !passed_alone.contains(*n)).cloned().collect();
        if let Some(results) = self.results_at(&row, &merge) {
            let mut passed = strings(&results, "passed");
            let mut failed = strings(&results, "failed");
            for f in &flaky {
                failed.remove(f);
                passed.insert(f.clone());
            }
            self.put_results(&row, &merge, &passed, &failed, &flaky);
        }
        patch(&mut row.doc, json!({ "flaky": flaky }));
        if still.is_empty() {
            log!("kanna-daemon: {}'s new failures all pass alone, so they're flaky: {}", row.id, flaky.join(", "));
            let result = json!({ "tests": row.doc.get("tests").cloned().unwrap_or(Value::Null), "new": [], "flaky": flaky, "known": row.doc.get("known").cloned().unwrap_or(json!(0)), "fixed": row.doc.get("fixed").cloned().unwrap_or(json!([])) });
            return self.pass(row, &merge, Some(result));
        }
        self.refuse(row, &still, &flaky, "");
    }

    fn refuse(self: &Rc<Self>, mut row: QueueRow, new: &[String], flaky: &[String], why: &str) {
        let known = row.doc.get("known").and_then(Value::as_u64).unwrap_or(0);
        let fixed = strings(&row.doc, "fixed");
        let result = json!({ "tests": row.doc.get("tests").cloned().unwrap_or(Value::Null), "new": new, "flaky": flaky, "known": known, "fixed": fixed });
        patch(&mut row.doc, json!({ "result": result }));
        self.save(&row);
        let why = match why.is_empty() {
            true => summary(new, flaky, known, &fixed),
            false => format!("{why}: {}", summary(new, flaky, known, &fixed)),
        };
        let log = row.doc.get("logs").and_then(|l| l.get("test")).and_then(Value::as_str).and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
        let mut lines = vec![];
        for key in new.iter().take(5) {
            let name = row.doc.get("rerunnable").and_then(Value::as_array).and_then(|a| a.iter().find(|a| a["key"].as_str() == Some(key.as_str()))).and_then(|a| a["name"].as_str().map(String::from)).unwrap_or_else(|| key.split(" (").next().unwrap_or(key).to_string());
            lines.extend(test_results::failure_text(&log, &name, FAILURE_LINES / 4));
        }
        self.fail(&row.id, &why, lines);
    }

    fn pass(self: &Rc<Self>, mut row: QueueRow, tested: &str, result: Option<Value>) {
        if let Some(result) = &result {
            let known = result["known"].as_u64().unwrap_or(0);
            let flaky: Vec<String> = result["flaky"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
            let fixed: BTreeSet<String> = result["fixed"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect();
            if known > 0 || !flaky.is_empty() || !fixed.is_empty() {
                log!("kanna-daemon: {}'s tests pass against main: {}", row.id, summary(&[], &flaky, known, &fixed));
            }
        }
        log!("kanna-daemon: {}'s tests passed at {tested}, so it lands", row.id);
        row.state = "landing".into();
        patch(&mut row.doc, json!({ "tested": tested, "result": result.unwrap_or(Value::Null) }));
        self.save(&row);
        self.soon.set(true);
    }

    fn results_at(&self, row: &QueueRow, commit: &str) -> Option<Value> {
        let command = test_command(Path::new(&row.repo))?;
        self.db.test_results(&row.repo, commit, &command).ok().flatten()
    }

    fn put_results(&self, row: &QueueRow, commit: &str, passed: &BTreeSet<String>, failed: &BTreeSet<String>, flaky: &[String]) {
        let Some(command) = test_command(Path::new(&row.repo)) else { return };
        let doc = json!({ "passed": passed.iter().collect::<Vec<_>>(), "failed": failed.iter().collect::<Vec<_>>(), "flaky": flaky, "run": opt(row.run.as_deref()), "queue": row.id });
        if let Err(error) = self.db.put_test_results(&row.repo, commit, &command, &doc) {
            log!("kanna-daemon: couldn't keep the test results at {commit}: {error}");
        }
    }

    fn keep_log(&self, row: &mut QueueRow, name: &str, path: &str) {
        if path.is_empty() {
            return;
        }
        let Some(folder) = self.tasks.upgrade().and_then(|t| t.entry(&row.task)).and_then(|e| e.folder).map(PathBuf::from) else { return };
        let dir = folder.join(format!(".kanna/tmp/{}", row.id));
        let to = dir.join(format!("{name}.log"));
        if fs::create_dir_all(&dir).is_ok() && fs::copy(path, &to).is_ok() {
            let mut logs = row.doc.get("logs").filter(|l| l.is_object()).cloned().unwrap_or_else(|| json!({}));
            logs[name] = json!(to.to_string_lossy());
            patch(&mut row.doc, json!({ "logs": logs }));
            self.save(row);
        }
    }

    fn last_failed(&self, run: &str) -> bool {
        let stages = self.db.stages(run).unwrap_or_default();
        stages.iter().rev().find(|s| s.verdict.as_deref() != Some("interrupted")).is_some_and(|s| s.verdict.as_deref() == Some("fail"))
    }

    fn last_log(&self, run: &str) -> Option<(String, String, String)> {
        let stages = self.db.stages(run).ok()?;
        let last = stages.iter().rev().find(|s| s.kind == "run" && s.verdict.as_deref() != Some("interrupted"))?;
        let log = string(&last.doc, "log").unwrap_or("").to_string();
        let feedback = string(&last.doc, "feedback").unwrap_or("").to_string();
        Some((last.stage.clone(), log, feedback))
    }

    fn land(self: &Rc<Self>, queue: &str) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.id == queue).cloned() else { return };
        let repo = PathBuf::from(&row.repo);
        let (base, merge, tested) = (text(&row.doc, "base"), text(&row.doc, "merge"), text(&row.doc, "tested"));
        let Some(main) = main_branch(&repo) else {
            return self.wait_landing(&mut row, "the main checkout has no branch checked out");
        };
        let now_main = git(&repo, &["rev-parse", "--verify", &main]).unwrap_or_default();
        let is_ancestor = |a: &str, b: &str| git(&repo, &["merge-base", "--is-ancestor", a, b]).is_ok();
        if !tested.is_empty() && now_main != base && is_ancestor(&tested, &now_main) {
            return self.landed(&mut row, &main, &now_main);
        }
        if merge.is_empty() || tested.is_empty() || !is_ancestor(&merge, &tested) {
            return self.fail(&row.id, "its pipeline ended on a commit that isn't the merge it tested", vec![]);
        }
        if now_main != base {
            let attempt = row.doc.get("attempt").and_then(Value::as_u64).unwrap_or(1);
            self.release_worker(&row, true);
            if attempt >= ATTEMPTS {
                return self.fail(&row.id, &format!("{main} kept moving under it ({attempt} tries)"), vec![]);
            }
            log!("kanna-daemon: {main} moved under {}, so it's tested again on the new {main}", row.id);
            row.state = "queued".into();
            row.run = None;
            patch(&mut row.doc, json!({ "attempt": attempt + 1, "tested": null, "stage": null, "phase": null }));
            self.save(&row);
            self.soon.set(true);
            return;
        }
        match git(&repo, &["merge", "--ff-only", &tested]) {
            Ok(_) => {
                let landed = git(&repo, &["rev-parse", "--verify", &main]).unwrap_or(tested);
                self.landed(&mut row, &main, &landed)
            }
            Err(error) => {
                let files: Vec<&str> = error.lines().filter(|l| l.starts_with('\t')).map(str::trim).collect();
                let why = match files.is_empty() {
                    true => format!("the main checkout can't fast-forward: {}", error.lines().next().unwrap_or("")),
                    false => format!("the main checkout has local changes to {}", files.join(", ")),
                };
                self.wait_landing(&mut row, &why)
            }
        }
    }

    fn wait_landing(&self, row: &mut QueueRow, why: &str) {
        if text(&row.doc, "why") != why {
            log!("kanna-daemon: {} waits to land: {why}", row.id);
        }
        patch(&mut row.doc, json!({ "why": why }));
        self.save(row);
        self.retry.set(Some(Instant::now() + RETRY));
    }

    pub fn refs_moved(&self) {
        if self.live.borrow().iter().any(|q| q.state == "landing") {
            self.soon.set(true);
        }
    }

    fn landed(self: &Rc<Self>, row: &mut QueueRow, main: &str, at: &str) {
        log!("kanna-daemon: {} landed: {main} is at {at}", row.id);
        row.state = "landed".into();
        row.ended_at = Some(now());
        patch(&mut row.doc, json!({ "landed": at, "why": null }));
        self.save(row);
        let tested = text(&row.doc, "tested");
        if at != tested {
            if let Some(results) = self.results_at(row, &tested) {
                if let Some(command) = test_command(Path::new(&row.repo)) {
                    let _ = self.db.put_test_results(&row.repo, at, &command, &results);
                }
            }
        }
        self.release_worker(row, true);
        if let Some(tasks) = self.tasks.upgrade() {
            tasks.landed(&row.task);
        }
        self.soon.set(true);
    }

    fn release_worker(&self, row: &QueueRow, drop_branch: bool) {
        let worker = text(&row.doc, "worker");
        if worker.is_empty() || !Path::new(&worker).is_dir() {
            return;
        }
        let worktree = Path::new(&worker);
        let _ = git(worktree, &["merge", "--abort"]);
        if let Err(error) = git(worktree, &["checkout", "-q", "-f", "--detach"]) {
            log!("kanna-daemon: {}'s worktree {worker} stays on its branch: {error}", row.id);
            return;
        }
        let branch = text(&row.doc, "queueBranch");
        if drop_branch && !branch.is_empty() {
            let _ = git(Path::new(&row.repo), &["branch", "-D", &branch]);
        }
    }

    fn fail(self: &Rc<Self>, queue: &str, why: &str, lines: Vec<String>) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.id == queue).cloned() else { return };
        log!("kanna-daemon: {queue} failed: {why}");
        self.release_worker(&row, false);
        let logs = row.doc.get("logs").cloned().unwrap_or(Value::Null);
        let kept = ["test", "alone", "main"].iter().find_map(|k| logs.get(*k).and_then(Value::as_str).map(String::from)).or_else(|| logs.as_object().and_then(|o| o.values().find_map(|v| v.as_str().map(String::from))));
        row.state = "failed".into();
        row.ended_at = Some(now());
        patch(&mut row.doc, json!({ "why": why, "log": opt(kept.as_deref()), "feedback": lines.join("\n") }));
        self.save(&row);
        let main = text(&row.doc, "main");
        let main = if main.is_empty() { "main".to_string() } else { main };
        let base = text(&row.doc, "base");
        let merge = text(&row.doc, "merge");
        let branch = text(&row.doc, "branch");
        let mut said = format!("\"{}\" didn't merge: {why}.", text(&row.doc, "name"));
        if !base.is_empty() {
            said.push_str(&format!(" It was tested on {main} {} + {branch}", short(&base)));
            if !merge.is_empty() {
                said.push_str(&format!(" (merge commit {})", short(&merge)));
            }
            said.push('.');
        }
        if let Some(result) = row.doc.get("result").filter(|r| r.is_object()) {
            said.push_str(&result_said(result, &main, &text(&row.doc, "mainRun")));
        }
        said.push_str(&format!(" {main} is unchanged and your branch is as you left it."));
        if let Some(kept) = &kept {
            said.push_str(&format!(" The log: {kept}."));
        }
        if let Some(main_log) = logs.get("main").and_then(Value::as_str).filter(|l| Some(*l) != kept.as_deref()) {
            said.push_str(&format!(" {main}'s own run: {main_log}."));
        }
        let shown: Vec<&String> = lines.iter().take(FAILURE_LINES).collect();
        if !shown.is_empty() {
            said.push_str(&format!(" The failures:\n\n{}\n\n", shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")));
        } else {
            said.push(' ');
        }
        said.push_str(&format!("Fix it on your branch (merge {main} first if it moved), then kanna task merge again."));
        let result = row.doc.get("result").filter(|r| r.is_object());
        let field = |key: &str| result.and_then(|r| r.get(key)).cloned().unwrap_or(Value::Null);
        let failure = json!({ "why": why, "queue": queue, "run": opt(row.run.as_deref()), "log": opt(kept.as_deref()), "at": num(now()), "new": field("new"), "flaky": field("flaky"), "onMain": field("known"), "fixed": field("fixed") });
        if let Some(tasks) = self.tasks.upgrade() {
            tasks.merge_failed(&row.task, queue, failure, &said);
        }
        self.soon.set(true);
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(8)]
}

fn strings(doc: &Value, key: &str) -> BTreeSet<String> {
    doc.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_str().map(String::from)).collect()
}

fn feedback_lines(stage: &Value) -> Vec<String> {
    string(stage, "feedback").unwrap_or("").lines().map(String::from).collect()
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn summary(new: &[String], flaky: &[String], known: u64, fixed: &BTreeSet<String>) -> String {
    let mut parts = vec![];
    if !new.is_empty() {
        parts.push(format!("{}: {}", plural(new.len(), "new failure", "new failures"), new.join(", ")));
    }
    if !flaky.is_empty() {
        parts.push(format!("{} (passes alone, fails under load): {}", plural(flaky.len(), "flaky test", "flaky tests"), flaky.join(", ")));
    }
    if known > 0 {
        parts.push(format!("{} already on main", plural(known as usize, "failure", "failures")));
    }
    if !fixed.is_empty() {
        parts.push(format!("fixes {} of main's", fixed.len()));
    }
    if parts.is_empty() {
        return "every test passes".into();
    }
    parts.join("; ")
}

fn result_said(result: &Value, main: &str, main_run: &str) -> String {
    let known = result["known"].as_u64().unwrap_or(0);
    let fixed: Vec<&str> = result["fixed"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut said = String::new();
    if known > 0 {
        said.push_str(&format!(" {} on the merge fail on {main} too, so they don't count against it.", plural(known as usize, "test that fails", "tests that fail")));
    }
    if !fixed.is_empty() {
        said.push_str(&format!(" It fixes {} that fail on {main}: {}.", fixed.len(), fixed.join(", ")));
    }
    if main_run == "ran" {
        said.push_str(&format!(" {main}'s results weren't known yet, so its tests ran first, in the same worktree."));
    }
    said
}

fn checkout(worker: &Path, args: &[&str]) -> Result<(), String> {
    let mut all = vec!["checkout", "-q", "-f"];
    all.extend_from_slice(args);
    git(worker, &all)?;
    git(worker, &["clean", "-fdq"]).map(|_| ())
}

pub fn worker_folder(repo: &Path) -> PathBuf {
    repo.join(tasks::WORKTREES).join(WORKER)
}

fn prepare_worker(repo: &Path, branch: &str, base: &str) -> Result<String, String> {
    tasks::exclude_worktrees(repo)?;
    let folder = worker_folder(repo);
    let real = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let listed = git(repo, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let registered = folder.join(".git").exists() && listed.lines().filter_map(|l| l.strip_prefix("worktree ")).any(|w| real(Path::new(w)) == real(&folder));
    let shown = folder.to_string_lossy().into_owned();
    if !registered {
        let _ = git(repo, &["worktree", "prune"]);
        if folder.exists() {
            return Err(format!("{shown} is there but isn't one of the repo's worktrees"));
        }
        git(repo, &["worktree", "add", "--detach", &shown, base])?;
        log!("kanna-daemon: {} has its merge queue's worktree now: {shown}", repo.display());
    }
    let _ = git(&folder, &["merge", "--abort"]);
    git(&folder, &["checkout", "-q", "-f", "-B", branch, base])?;
    git(&folder, &["clean", "-fdq"])?;
    Ok(shown)
}

pub fn own_pipeline(folder: &Path) -> bool {
    folder.join(format!(".kanna/pipelines/{PIPELINE}.kojo")).is_file()
}

pub fn test_pipeline(command: &str, within: Option<u64>, stage: &str) -> String {
    let within = within.map_or(String::new(), |s| format!(" within {s}s"));
    format!("{stage} = run {command}{within}\n{stage} -[pass]-> done\n")
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

fn row_value(row: &QueueRow) -> Value {
    json!({
        "id": row.id,
        "repo": row.repo,
        "task": row.task,
        "head": row.head,
        "state": row.state,
        "queuedAt": num(row.queued_at),
        "endedAt": row.ended_at.map_or(Value::Null, num),
        "run": opt(row.run.as_deref()),
        "doc": row.doc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kanna_s_pipeline_runs_merge_test_and_lands_only_on_a_pass() {
        let dir = std::env::temp_dir().join(format!("kanna-merge-queue-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".kanna")).unwrap();
        assert_eq!(pipeline_text(&dir, &dir), None, "no merge.test and no pipeline: nothing to test with");
        fs::write(dir.join(".kanna/config.json"), r#"{"version": 1, "merge": {"test": "sh test.sh", "within": 900}}"#).unwrap();
        let (text, shown) = pipeline_text(&dir, &dir).unwrap();
        assert_eq!(shown, "Kanna's merge-queue.kojo (merge.test)");
        let pipeline = kanna_pipe::parse(&text).unwrap();
        let mut doc = runs::Doc { taken: vec![0; pipeline.arrows.len()], ..runs::Doc::default() };
        assert_eq!(runs::decide(&pipeline, &mut doc, "test", Some(kanna_pipe::Verdict::Fail), None), runs::Next::Stuck);
        assert_eq!(runs::decide(&pipeline, &mut doc, "test", Some(kanna_pipe::Verdict::Pass), None), runs::Next::Done);
        assert_eq!(pipeline.stage("test").and_then(|s| s.within), Some(Duration::from_secs(900)));
        fs::create_dir_all(dir.join(".kanna/pipelines")).unwrap();
        fs::write(dir.join(".kanna/pipelines/merge-queue.kojo"), "check = run true\ncheck -[pass]-> done\n").unwrap();
        assert!(pipeline_text(&dir, &dir).unwrap().0.starts_with("check = run true"), "the repo's own pipeline wins");
        let _ = fs::remove_dir_all(&dir);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-queue-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_queue_tests_every_entry_in_one_worktree_that_keeps_its_build() {
        let dir = scratch("worker");
        let repo = dir.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let repo = fs::canonicalize(&repo).unwrap();
        let g = |args: &[&str]| git(&repo, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.email", "e@e"]);
        g(&["config", "user.name", "e"]);
        fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        g(&["add", "-A"]);
        g(&["commit", "-qm", "first"]);
        let first = g(&["rev-parse", "HEAD"]);
        let worker = prepare_worker(&repo, "queue/task-1-1", &first).unwrap();
        assert_eq!(PathBuf::from(&worker), worker_folder(&repo));
        fs::create_dir_all(Path::new(&worker).join("target")).unwrap();
        fs::write(Path::new(&worker).join("target/built"), "").unwrap();
        fs::write(Path::new(&worker).join("left-over.txt"), "").unwrap();
        fs::write(repo.join("a.txt"), "b\n").unwrap();
        g(&["commit", "-qam", "second"]);
        let second = g(&["rev-parse", "HEAD"]);
        assert_eq!(prepare_worker(&repo, "queue/task-2-1", &second).unwrap(), worker, "the next entry gets the same worktree");
        assert!(Path::new(&worker).join("target/built").exists(), "its build folder stays warm");
        assert!(!Path::new(&worker).join("left-over.txt").exists(), "what the last entry left untracked is gone");
        assert_eq!(git(Path::new(&worker), &["rev-parse", "HEAD"]).unwrap(), second);
        assert_eq!(git(Path::new(&worker), &["branch", "--show-current"]).unwrap(), "queue/task-2-1");
        let _ = fs::remove_dir_all(&dir);
    }

    fn judging(name: &str) -> (Rc<Queue>, PathBuf, QueueRow) {
        let dir = scratch(name);
        let repo = dir.join("repo");
        fs::create_dir_all(repo.join(".kanna")).unwrap();
        fs::write(repo.join(".kanna/config.json"), r#"{"version": 1, "merge": {"test": "cargo test"}}"#).unwrap();
        let db = Rc::new(Db::open(&dir.join("state")));
        let queue = Rc::new(Queue::open(db, Weak::new(), Weak::new()));
        let repo_text = repo.to_string_lossy().into_owned();
        queue.db.put_test_results(&repo_text, "base0", "cargo test", &json!({ "passed": ["steady", "fixed_here"], "failed": ["broken_on_main", "fixed_here_too"] })).unwrap();
        let row = QueueRow { id: "queue-1".into(), repo: repo_text, task: "task-1".into(), head: "h".into(), state: "testing".into(), queued_at: 1.0, ended_at: None, run: Some("run-1".into()), doc: json!({ "own": false, "base": "base0", "merge": "merge0", "name": "A", "main": "main", "phase": "merge" }) };
        queue.db.put_queued(&row).unwrap();
        queue.live.borrow_mut().push(row.clone());
        (queue, dir, row)
    }

    fn state(queue: &Queue) -> QueueRow {
        queue.db.queued(false).unwrap().into_iter().find(|q| q.id == "queue-1").unwrap()
    }

    const FAIL: &str = "running 4 tests\ntest steady ... ok\ntest broken_on_main ... FAILED\ntest fixed_here ... ok\ntest fixed_here_too ... ok\n\ntest result: FAILED. 3 passed; 1 failed\n";

    #[test]
    fn an_entry_lands_while_main_has_a_known_failing_test() {
        let (queue, dir, row) = judging("known");
        queue.merge_tested(row, FAIL, false, &json!({}));
        let landed = state(&queue);
        assert_eq!(landed.state, "landing", "{:?}", landed.doc);
        assert_eq!(landed.doc["tested"], "merge0");
        assert_eq!(landed.doc["result"]["known"], 1);
        assert_eq!(landed.doc["result"]["fixed"], json!(["fixed_here_too"]), "the merge fixes one of main's failures");
        let cached = queue.db.test_results(&landed.repo, "merge0", "cargo test").unwrap().unwrap();
        assert_eq!(cached["failed"], json!(["broken_on_main"]), "the merge's own results are kept for when it's main");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_that_adds_a_failure_is_refused_with_the_test_named() {
        let (queue, dir, row) = judging("new");
        let output = FAIL.replace("test steady ... ok", "test steady ... FAILED");
        queue.merge_tested(row, &output, false, &json!({}));
        let failed = state(&queue);
        assert_eq!(failed.state, "failed");
        let why = failed.doc["why"].as_str().unwrap();
        assert_eq!(why, "1 new failure: steady; 1 failure already on main; fixes 1 of main's");
        assert_eq!(failed.doc["result"]["new"], json!(["steady"]));
        assert_eq!(failed.doc["result"]["known"], 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_failure_that_passes_alone_is_flaky_and_lands() {
        let (queue, dir, mut row) = judging("flaky");
        patch(&mut row.doc, json!({ "phase": "alone", "new": ["steady", "fixed_here"], "known": 1, "fixed": [] }));
        queue.put_results(&row, "merge0", &BTreeSet::new(), &["steady".to_string(), "fixed_here".to_string(), "broken_on_main".to_string()].into(), &[]);
        queue.alone_tested(row.clone(), "running 1 test\ntest steady ... ok\n\ntest result: ok. 1 passed\nrunning 1 test\ntest fixed_here ... FAILED\n\ntest result: FAILED.\n");
        let failed = state(&queue);
        assert_eq!(failed.state, "failed", "fixed_here fails alone too");
        assert_eq!(failed.doc["why"], "1 new failure: fixed_here; 1 flaky test (passes alone, fails under load): steady; 1 failure already on main");
        assert_eq!(failed.doc["result"]["flaky"], json!(["steady"]));
        let (queue, dir2, mut row) = judging("flaky-lands");
        patch(&mut row.doc, json!({ "phase": "alone", "new": ["steady"], "known": 1, "fixed": [] }));
        queue.alone_tested(row, "test steady ... ok\ntest result: ok.\n");
        let landed = state(&queue);
        assert_eq!(landed.state, "landing");
        assert_eq!(landed.doc["result"]["flaky"], json!(["steady"]));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn output_that_isn_t_libtest_s_is_judged_by_its_exit_code() {
        let (queue, dir, row) = judging("exit");
        queue.merge_tested(row, "FAILED: broken is here\n", false, &json!({ "feedback": "FAILED: broken is here" }));
        let failed = state(&queue);
        assert_eq!((failed.state.as_str(), failed.doc["why"].as_str()), ("failed", Some("test failed")));
        assert_eq!(failed.doc["feedback"], "FAILED: broken is here");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn places_read_as_ordinals() {
        let said: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101].iter().map(|n| ordinal(*n)).collect();
        assert_eq!(said, ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st"]);
    }
}
