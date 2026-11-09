use crate::db::{Db, QueueRow};
use crate::runs::{self, Runs, Started};
use crate::task_events;
use crate::tasks::{self, git, Tasks};
use kanna_mux::json::{json, num, opt, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Responder};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const PIPELINE: &str = "merge-queue";

const ATTEMPTS: u64 = 3;

const RETRY: Duration = Duration::from_secs(30);

const LIVE: &[&str] = &["queued", "testing", "landing"];

const FAILURE_LINES: usize = 40;

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
    let within = test_within(repo).map_or(String::new(), |s| format!(" within {s}s"));
    Some((format!("test = run {command}{within}\ntest -[pass]-> done\n"), format!("Kanna's {PIPELINE}.kojo (merge.test)")))
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
        self.remove_worker(&row);
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
        let Some(main) = main_branch(&repo) else { return self.fail(&row.id, "the main checkout has no branch checked out", None) };
        let base = match git(&repo, &["rev-parse", "--verify", &main]) {
            Ok(base) => base,
            Err(error) => return self.fail(&row.id, &format!("can't read {main}: {error}"), None),
        };
        let attempt = row.doc.get("attempt").and_then(Value::as_u64).unwrap_or(1);
        let mut n = 1;
        let (worker, branch) = loop {
            match tasks::add_worktree(&repo, &format!("{}-queue-{n}", row.task), &format!("queue/{}-{n}", row.task), Some(&base)) {
                Ok(made) => break made,
                Err(error) if error.contains("is there already") && n < 1000 => n += 1,
                Err(error) => return self.fail(&row.id, &format!("can't make the merge's worktree: {error}"), None),
            }
        };
        let worktree = Path::new(&worker);
        let message = text(&row.doc, "message");
        if let Err(error) = git(worktree, &["merge", "--no-ff", "-m", &message, &row.head]) {
            let files = git(worktree, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let _ = git(worktree, &["merge", "--abort"]);
            patch(&mut row.doc, json!({ "worker": worker, "queueBranch": branch, "base": base }));
            self.save(&row);
            let why = match files.is_empty() {
                true => format!("it doesn't merge onto {main}: {error}"),
                false => format!("it conflicts with {main} in {}", files.lines().collect::<Vec<_>>().join(", ")),
            };
            return self.fail(&row.id, &why, None);
        }
        let merge = git(worktree, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let Some((pipeline, shown)) = pipeline_text(worktree, &repo) else {
            patch(&mut row.doc, json!({ "worker": worker, "queueBranch": branch, "base": base, "merge": merge }));
            self.save(&row);
            return self.fail(&row.id, "this repo has no tests for the merge queue any more: set merge.test, or write .kanna/pipelines/merge-queue.kojo", None);
        };
        let Some(runs) = self.runs.upgrade() else { return };
        let started = Started {
            task: row.task.clone(),
            repo: row.repo.clone(),
            worker: worker.clone(),
            branch: branch.clone(),
            base: merge.clone(),
            text: pipeline,
            shown,
            name: PIPELINE.into(),
            request: message.clone(),
            prompts: prompts(worktree),
            owner: Some(runs::QUEUE.into()),
            by: row.id.clone(),
            agent: false,
        };
        patch(&mut row.doc, json!({ "worker": worker, "queueBranch": branch, "base": base, "merge": merge, "main": main, "since": now(), "attempt": attempt }));
        match runs.start_owned(started) {
            Ok(run) => {
                log!("kanna-daemon: {} tests {} on {main} {} as {run}", row.id, row.task, &base[..base.len().min(8)]);
                row.state = "testing".into();
                row.run = Some(run);
                patch(&mut row.doc, json!({ "stage": "test" }));
                self.save(&row);
            }
            Err(error) => {
                self.save(&row);
                self.fail(&row.id, &format!("its pipeline can't run: {}", error.message), None);
            }
        }
    }

    fn run_ended(self: &Rc<Self>, run: &str, outcome: Result<String, String>) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.run.as_deref() == Some(run)).cloned() else { return };
        if row.state != "testing" {
            return;
        }
        match outcome {
            Ok(_) if self.last_failed(run) => {
                let log = self.last_log(run);
                self.fail(&row.id, "its pipeline ended after a stage failed", log);
            }
            Ok(head) => {
                log!("kanna-daemon: {}'s tests passed at {head}, so it lands", row.id);
                row.state = "landing".into();
                patch(&mut row.doc, json!({ "tested": head }));
                self.save(&row);
                self.soon.set(true);
            }
            Err(why) => {
                let log = self.last_log(run);
                let why = match (self.last_failed(run), &log) {
                    (true, Some((stage_log, _))) => format!("{} failed", stage_log.split('\n').next().unwrap_or("test")),
                    _ => why,
                };
                self.fail(&row.id, &why, log);
            }
        }
    }

    fn last_failed(&self, run: &str) -> bool {
        let stages = self.db.stages(run).unwrap_or_default();
        stages.iter().rev().find(|s| s.verdict.as_deref() != Some("interrupted")).is_some_and(|s| s.verdict.as_deref() == Some("fail"))
    }

    fn last_log(&self, run: &str) -> Option<(String, String)> {
        let stages = self.db.stages(run).ok()?;
        let last = stages.iter().rev().find(|s| s.kind == "run" && s.verdict.as_deref() != Some("interrupted"))?;
        let log = string(&last.doc, "log").unwrap_or("");
        let feedback = string(&last.doc, "feedback").unwrap_or("").to_string();
        Some((format!("{}\n{log}", last.stage), feedback))
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
            return self.fail(&row.id, "its pipeline ended on a commit that isn't the merge it tested", None);
        }
        if now_main != base {
            let attempt = row.doc.get("attempt").and_then(Value::as_u64).unwrap_or(1);
            self.remove_worker(&row);
            if attempt >= ATTEMPTS {
                return self.fail(&row.id, &format!("{main} kept moving under it ({attempt} tries)"), None);
            }
            log!("kanna-daemon: {main} moved under {}, so it's tested again on the new {main}", row.id);
            row.state = "queued".into();
            row.run = None;
            patch(&mut row.doc, json!({ "attempt": attempt + 1, "tested": null, "stage": null }));
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
        self.remove_worker(row);
        let branch = text(&row.doc, "queueBranch");
        if !branch.is_empty() {
            let _ = git(Path::new(&row.repo), &["branch", "-D", &branch]);
        }
        if let Some(tasks) = self.tasks.upgrade() {
            tasks.landed(&row.task);
        }
        self.soon.set(true);
    }

    fn remove_worker(&self, row: &QueueRow) {
        let worker = text(&row.doc, "worker");
        if !worker.is_empty() && Path::new(&worker).is_dir() {
            if let Err(error) = git(Path::new(&row.repo), &["worktree", "remove", "--force", &worker]) {
                log!("kanna-daemon: {}'s worktree {worker} is kept: {error}", row.id);
            }
        }
    }

    fn fail(self: &Rc<Self>, queue: &str, why: &str, log: Option<(String, String)>) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.id == queue).cloned() else { return };
        log!("kanna-daemon: {queue} failed: {why}");
        let task_folder = self.tasks.upgrade().and_then(|t| t.entry(&row.task)).and_then(|e| e.folder).map(PathBuf::from);
        let mut kept = None;
        let mut feedback = String::new();
        if let Some((stage_log, said)) = log {
            feedback = said;
            let (stage, path) = stage_log.split_once('\n').unwrap_or(("test", ""));
            if let (Some(folder), false) = (&task_folder, path.is_empty()) {
                let dir = folder.join(format!(".kanna/tmp/{queue}"));
                let to = dir.join(format!("{stage}.log"));
                if fs::create_dir_all(&dir).is_ok() && fs::copy(path, &to).is_ok() {
                    kept = Some(to.display().to_string());
                }
            }
        }
        self.remove_worker(&row);
        row.state = "failed".into();
        row.ended_at = Some(now());
        patch(&mut row.doc, json!({ "why": why, "log": opt(kept.as_deref()), "feedback": feedback }));
        self.save(&row);
        let main = text(&row.doc, "main");
        let main = if main.is_empty() { "main".to_string() } else { main };
        let base = text(&row.doc, "base");
        let merge = text(&row.doc, "merge");
        let branch = text(&row.doc, "branch");
        let mut said = format!("\"{}\" didn't merge: {why}.", text(&row.doc, "name"));
        if !base.is_empty() {
            said.push_str(&format!(" It was tested on {main} {} + {branch}", &base[..base.len().min(8)]));
            if !merge.is_empty() {
                said.push_str(&format!(" (merge commit {})", &merge[..merge.len().min(8)]));
            }
            said.push('.');
        }
        said.push_str(&format!(" {main} is unchanged and your branch is as you left it."));
        if let Some(kept) = &kept {
            said.push_str(&format!(" The log: {kept}."));
        }
        let lines: Vec<&str> = feedback.lines().take(FAILURE_LINES).collect();
        if !lines.is_empty() {
            said.push_str(&format!(" The failures:\n\n{}\n\n", lines.join("\n")));
        } else {
            said.push(' ');
        }
        said.push_str(&format!("Fix it on your branch (merge {main} first if it moved), then kanna task merge again."));
        let failure = json!({ "why": why, "queue": queue, "run": opt(row.run.as_deref()), "log": opt(kept.as_deref()), "at": num(now()) });
        if let Some(tasks) = self.tasks.upgrade() {
            tasks.merge_failed(&row.task, queue, failure, &said);
        }
        self.soon.set(true);
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

    #[test]
    fn places_read_as_ordinals() {
        let said: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101].iter().map(|n| ordinal(*n)).collect();
        assert_eq!(said, ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st"]);
    }
}
