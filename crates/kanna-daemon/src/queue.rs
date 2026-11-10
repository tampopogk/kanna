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

const KEPT_LOGS: usize = 30;

const LOG_BYTES: u64 = 256 * 1024;

pub const WORKER: &str = "merge-queue";

pub const PROMPT: &str = "merge-judge";

pub const TEST: &str = "test";

pub const JUDGE: &str = "judge";

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

pub fn pipeline_text(folder: &Path, repo: &Path) -> Option<(String, String)> {
    let own = folder.join(format!(".kanna/pipelines/{PIPELINE}.kojo"));
    if let Ok(text) = fs::read_to_string(&own) {
        return Some((text, own.display().to_string()));
    }
    let command = test_command(repo)?;
    let files = tasks::config_files(repo);
    let agent = tasks::task_agent(&files);
    let agent = if kanna_pipe::AGENTS.contains(&agent.as_str()) { agent } else { tasks::DEFAULT_AGENT.to_string() };
    Some((test_pipeline(&command, &agent), format!("Kanna's {PIPELINE}.kojo (merge.test, a failure judged by {agent})")))
}

pub fn fits(command: &str) -> bool {
    let text = test_pipeline(command, tasks::DEFAULT_AGENT);
    kanna_pipe::parse(&text).ok().and_then(|p| p.stage(TEST).map(|s| matches!(&s.kind, kanna_pipe::Kind::Run { command: read } if read == command.trim()))).unwrap_or(false)
}

const UNFIT: &str = "merge.test can't be written as a pipeline's command (it has a #, a line break, or ends in `within N` or `on NAME`): put it in a script and name the script";

pub struct Tested<'a> {
    pub command: &'a str,
    pub name: &'a str,
    pub branch: &'a str,
    pub main: &'a str,
    pub base: &'a str,
    pub merge: &'a str,
    pub queue_branch: &'a str,
}

pub fn judge_prompt(t: &Tested) -> String {
    let (base, merge) = (short(t.base), short(t.merge));
    format!(
        "You judge a merge for {main}'s merge queue: \"{name}\" (the branch {branch}) merged onto {main} at {base}, as commit {merge}. The queue ran the repo's tests on it (`{command}`) and they failed. The end of their output is under \"Feedback from {test}\" below, with the path of the file that has all of it. This worktree is the queue's own, on {queue}. Nothing in it is yours to change: don't edit, commit, push or merge.

## Whose failure is it

Decide, for each failure, whether this branch caused it, however you like: read the branch's change (`git diff {base} HEAD`, `git log --oneline {base}..HEAD`), run a failing test again on its own, or run it on {main} (`git checkout -q --detach {base}`, and `git checkout -q -f {queue}` to come back). A failure that's on {main} too, or that passes when it runs again alone (it fails only under load), isn't the branch's. A build that the branch breaks is.

## A command that takes minutes

Your shell stops a command after about ten minutes, and whatever you leave running in the background ends with your turn. So ask Kanna to run anything that could take more than a few minutes, such as a build on {main} or the whole suite again. Kanna runs it in this worktree and waits with you:

    \"$KANNA_CLI\" run command 'COMMAND'

It prints the command's exit code and the path of its output, or after 8 minutes that it's still running; then run `\"$KANNA_CLI\" run command --wait`, as often as it takes. Don't end your turn to wait for it.

## Your verdict

Report pass if no failure is the branch's, and fail if any is: a pass lands the merge on {main}, a fail sends the task back to its agent.

## Your answer

Its first line is one line on the outcome, which the Merge queue tab shows: `2 tests fail, neither is the branch's: …`, or `broken_thing fails: the branch removed its fixture`. Then each failure: its name, whether it's the branch's, and how you know. The task's agent reads it when the entry fails, and the queue keeps it when the entry lands.
",
        main = t.main,
        name = t.name,
        branch = t.branch,
        queue = t.queue_branch,
        command = t.command,
        test = TEST,
    )
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
    pub on_needs: RefCell<Option<Box<dyn Fn(&str, &str, &str)>>>,
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
        Queue { db, tasks, runs, live: RefCell::new(live), soon: Cell::new(true), retry: Cell::new(None), on_needs: RefCell::default() }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle, runs: &Rc<Runs>) {
        let ending = Rc::downgrade(self);
        *runs.on_end.borrow_mut() = Some(Box::new(move |run, outcome| {
            if let Some(this) = ending.upgrade() {
                this.run_ended(run, outcome);
            }
        }));
        let moving = Rc::downgrade(self);
        *runs.on_moved.borrow_mut() = Some(Box::new(move |run| {
            if let Some(this) = moving.upgrade() {
                this.run_moved(run);
            }
        }));
        let telling = Rc::downgrade(self);
        *runs.on_tell.borrow_mut() = Some(Box::new(move |run, text| {
            if let Some(this) = telling.upgrade() {
                this.run_tells(run, text);
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
            Some("log") => self.log(args),
            other => Err(bad(format!("unknown queue action {other:?} (add, cancel, list or log)"))),
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
        if !own_pipeline(&folder) && !test_command(&repo).is_some_and(|c| fits(&c)) {
            return Err(KmuxError::new("no_tests", UNFIT));
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
        let repo = string(args, "repo").map(same_folder);
        let recent = args.get("recent").and_then(Value::as_u64).map(|n| n as usize);
        let live_only = recent.is_none() && args.get("all") != Some(&Value::Bool(true));
        let rows: Vec<QueueRow> = self.db.queued(live_only).map_err(failed)?.into_iter().filter(|q| repo.as_ref().is_none_or(|r| same_folder(&q.repo) == *r)).collect();
        let rows = match recent {
            Some(most) => {
                let (live, ended): (Vec<QueueRow>, Vec<QueueRow>) = rows.into_iter().partition(|q| LIVE.contains(&q.state.as_str()));
                let mut ended: Vec<QueueRow> = ended.into_iter().filter(|q| matches!(q.state.as_str(), "landed" | "failed")).collect();
                ended.sort_by(|a, b| b.ended_at.unwrap_or(0.0).total_cmp(&a.ended_at.unwrap_or(0.0)));
                ended.truncate(most);
                live.into_iter().chain(ended).collect()
            }
            None => rows,
        };
        let runs = self.runs.upgrade();
        let rows: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut value = row_value(row);
                let terminal = row.run.as_deref().filter(|_| row.state == "testing").and_then(|run| runs.as_ref().and_then(|r| r.terminal_of(run)));
                value["terminal"] = opt(terminal.as_deref());
                value
            })
            .collect();
        Ok(Map::from_iter([("queue".to_string(), Value::Array(rows))]))
    }

    fn log(&self, args: &Value) -> Result<Map, KmuxError> {
        let id = string(args, "queue").ok_or_else(|| bad("which entry? give its queue ID"))?;
        let row = self.db.queued(false).map_err(failed)?.into_iter().find(|q| q.id == id).ok_or_else(|| KmuxError::new("not_found", format!("no merge queue entry {id}")))?;
        let path = ["kept", "log"].iter().filter_map(|key| string(&row.doc, key)).find(|p| Path::new(p).is_file()).map(String::from);
        let text = path.as_deref().and_then(|p| tail(Path::new(p), LOG_BYTES));
        let note = row.doc.get("result").and_then(|r| r["note"].as_str()).or_else(|| string(&row.doc, "note")).filter(|n| !n.is_empty());
        Ok(Map::from_iter([
            ("queue".to_string(), json!(row.id)),
            ("state".to_string(), json!(row.state)),
            ("note".to_string(), opt(note)),
            ("path".to_string(), opt(path.as_deref())),
            ("text".to_string(), opt(text.as_deref())),
        ]))
    }

    fn keep_copy(&self, row: &mut QueueRow, path: &str) {
        let Ok(common) = git(Path::new(&row.repo), &["rev-parse", "--path-format=absolute", "--git-common-dir"]) else { return };
        let dir = Path::new(&common).join("kanna/merge-queue");
        let to = dir.join(format!("{}.log", row.id));
        if fs::create_dir_all(&dir).is_err() || copy_readable(path, &to).is_err() {
            return;
        }
        patch(&mut row.doc, json!({ "kept": to.to_string_lossy() }));
        self.save(row);
        let mut logs: Vec<(SystemTime, PathBuf)> = fs::read_dir(&dir).into_iter().flatten().flatten().filter_map(|f| Some((f.metadata().ok()?.modified().ok()?, f.path()))).collect();
        logs.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, old) in logs.into_iter().skip(KEPT_LOGS) {
            let _ = fs::remove_file(old);
        }
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
        patch(&mut row.doc, json!({ "worker": worker, "queueBranch": branch, "base": base, "main": main, "stage": null, "result": null, "note": null, "logs": null, "needs": null, "told": null }));
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
        let mut prompts = prompts(worktree);
        if !own {
            let command = test_command(&repo).unwrap_or_default();
            if !fits(&command) {
                self.save(&row);
                return self.fail(&row.id, UNFIT, vec![]);
            }
            let tested = Tested { command: &command, name: &text(&row.doc, "name"), branch: &text(&row.doc, "branch"), main: &main, base: &base, merge: &merge, queue_branch: &branch };
            prompts.entry(PROMPT.to_string()).or_insert_with(|| judge_prompt(&tested));
        }
        match self.start_run(&mut row, pipeline, shown, &merge, message.clone(), prompts) {
            Ok(run) => log!("kanna-daemon: {} tests {} on {main} {} as {run}", row.id, row.task, short(&base)),
            Err(error) => {
                self.save(&row);
                self.fail(&row.id, &format!("its pipeline can't run: {error}"), vec![]);
            }
        }
    }

    fn start_run(&self, row: &mut QueueRow, pipeline: String, shown: String, base: &str, request: String, prompts: BTreeMap<String, String>) -> Result<String, String> {
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
            agent: false,
        };
        let run = runs.start_owned(started).map_err(|e| e.message)?;
        row.state = "testing".into();
        row.run = Some(run.clone());
        self.save(row);
        Ok(run)
    }

    fn run_moved(&self, run: &str) {
        let Some(mut row) = self.live.try_borrow().ok().and_then(|live| live.iter().find(|q| q.run.as_deref() == Some(run) && q.state == "testing").cloned()) else { return };
        let Some(place) = self.runs.upgrade().and_then(|runs| runs.place_of(run)) else { return };
        let needs = place.why.filter(|_| matches!(place.state.as_str(), "paused" | "waiting"));
        let stage = place.stage.or_else(|| string(&row.doc, "stage").map(String::from));
        if string(&row.doc, "stage") == stage.as_deref() && string(&row.doc, "needs") == needs.as_deref() {
            return;
        }
        let told = needs.as_ref().and_then(|_| string(&row.doc, "told").map(String::from));
        patch(&mut row.doc, json!({ "stage": opt(stage.as_deref()), "needs": opt(needs.as_deref()), "told": opt(told.as_deref()) }));
        self.save(&row);
    }

    fn run_tells(&self, run: &str, said: &str) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.run.as_deref() == Some(run) && q.state == "testing").cloned() else { return };
        let why = self.runs.upgrade().and_then(|runs| runs.place_of(run)).and_then(|p| p.why).unwrap_or_else(|| said.lines().next().unwrap_or("").to_string());
        log!("kanna-daemon: {} waits for you: {why}", row.id);
        patch(&mut row.doc, json!({ "needs": why, "told": said }));
        self.save(&row);
        let name = text(&row.doc, "name");
        if let Some(tasks) = self.tasks.upgrade() {
            tasks.queue_needs(&row.task, &row.id, &needs_you(&name, &tasks.short_of(&row.task), said));
        }
        if let Some(on_needs) = self.on_needs.borrow().as_ref() {
            on_needs(&row.id, &format!("\u{201C}{name}\u{201D} waits in the merge queue"), &why);
        }
    }

    fn run_ended(self: &Rc<Self>, run: &str, outcome: Result<String, String>) {
        let Some(mut row) = self.live.borrow().iter().find(|q| q.run.as_deref() == Some(run)).cloned() else { return };
        if row.state != "testing" {
            return;
        }
        let judged = row.doc.get("own") == Some(&Value::Bool(false));
        let stages = self.db.stages(run).unwrap_or_default();
        let ended = |s: &&crate::db::StageRow| s.verdict.as_deref() != Some("interrupted");
        let last = stages.iter().rev().find(|s| matches!(s.kind.as_str(), "run" | "agent") && ended(s)).cloned();
        let log = stages.iter().rev().find(|s| s.kind == "run" && ended(s)).and_then(|s| string(&s.doc, "log").map(String::from));
        if let Some(log) = log.filter(|l| Path::new(l).is_file()) {
            self.keep_log(&mut row, "test", &log);
            self.keep_copy(&mut row, &log);
        }
        let failed = last.as_ref().is_some_and(|s| s.verdict.as_deref() == Some("fail"));
        let stage = last.as_ref().map_or(TEST.to_string(), |s| s.stage.clone());
        let doc = last.as_ref().map_or(Value::Null, |s| s.doc.clone());
        let judge = stages.iter().rev().find(|s| judged && s.kind == "agent" && ended(s) && matches!(s.verdict.as_deref(), Some("pass" | "fail")));
        let answer = judge.and_then(|s| string(&s.doc, "answer")).unwrap_or("").trim().to_string();
        patch(&mut row.doc, json!({ "needs": null, "told": null }));
        if !answer.is_empty() {
            patch(&mut row.doc, json!({ "note": answer }));
        }
        self.save(&row);
        let lines: Vec<String> = string(&doc, "feedback").unwrap_or("").lines().map(String::from).collect();
        let timed_out = string(&doc, "note") == Some("timed out");
        match outcome {
            Ok(head) if !failed => {
                let result = judge.map(|_| json!({ "verdict": "pass", "note": answer }));
                self.pass(row, &head, result)
            }
            _ if timed_out => self.fail(&row.id, &format!("{stage} timed out"), lines.into_iter().filter(|l| l != "timed out").collect()),
            _ if judge.is_some() && failed => {
                let first = answer.lines().next().map(str::trim).filter(|l| !l.is_empty()).unwrap_or("the tests failed, and the queue's judge says it's the branch's doing").to_string();
                patch(&mut row.doc, json!({ "result": { "verdict": "fail", "note": answer } }));
                self.save(&row);
                self.fail(&row.id, &first, vec![])
            }
            Ok(_) => self.fail(&row.id, "its pipeline ended after a stage failed", lines),
            Err(_) if failed => self.fail(&row.id, &format!("{stage} failed"), lines),
            Err(why) => self.fail(&row.id, &why, lines),
        }
    }

    fn pass(self: &Rc<Self>, mut row: QueueRow, tested: &str, result: Option<Value>) {
        match result.as_ref().and_then(|r| r["note"].as_str()).filter(|n| !n.is_empty()) {
            Some(note) => log!("kanna-daemon: {}'s judge passes it at {tested}, so it lands: {}", row.id, note.lines().next().unwrap_or("")),
            None => log!("kanna-daemon: {}'s tests passed at {tested}, so it lands", row.id),
        }
        row.state = "landing".into();
        patch(&mut row.doc, json!({ "tested": tested, "result": result.unwrap_or(Value::Null) }));
        self.save(&row);
        self.soon.set(true);
    }

    fn keep_log(&self, row: &mut QueueRow, name: &str, path: &str) {
        let Some(folder) = self.tasks.upgrade().and_then(|t| t.entry(&row.task)).and_then(|e| e.folder).map(PathBuf::from) else { return };
        let dir = folder.join(format!(".kanna/tmp/{}", row.id));
        let to = dir.join(format!("{name}.log"));
        if fs::create_dir_all(&dir).is_ok() && copy_readable(path, &to).is_ok() {
            let mut logs = row.doc.get("logs").filter(|l| l.is_object()).cloned().unwrap_or_else(|| json!({}));
            logs[name] = json!(to.to_string_lossy());
            patch(&mut row.doc, json!({ "logs": logs }));
            self.save(row);
        }
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
        let kept = logs.get("test").and_then(Value::as_str).map(String::from);
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
        said.push_str(&format!(" {main} is unchanged and your branch is as you left it."));
        if let Some(kept) = &kept {
            said.push_str(&format!(" The tests' log: {kept}."));
        }
        let result = row.doc.get("result").filter(|r| r.is_object());
        let note = result.and_then(|r| r["note"].as_str()).filter(|n| !n.is_empty());
        let shown: Vec<&String> = lines.iter().take(FAILURE_LINES).collect();
        if let Some(note) = note {
            said.push_str(&format!(" The queue's judge says it's the branch's doing:\n\n{note}\n\n"));
        } else if !shown.is_empty() {
            said.push_str(&format!(" The failures:\n\n{}\n\n", shown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n")));
        } else {
            said.push(' ');
        }
        said.push_str(&format!("Fix it on your branch (merge {main} first if it moved), then kanna task merge again."));
        let verdict = result.and_then(|r| r["verdict"].as_str());
        let failure = json!({ "why": why, "queue": queue, "run": opt(row.run.as_deref()), "log": opt(kept.as_deref()), "at": num(now()), "verdict": opt(verdict), "note": opt(note) });
        if let Some(tasks) = self.tasks.upgrade() {
            tasks.merge_failed(&row.task, queue, failure, &said);
        }
        self.soon.set(true);
    }
}

fn copy_readable(from: &str, to: &Path) -> std::io::Result<()> {
    fs::write(to, readable(&String::from_utf8_lossy(&fs::read(from)?)))
}

pub fn needs_you(name: &str, task: &str, said: &str) -> String {
    format!("\"{name}\" waits in the merge queue for you, and the entries behind it wait too. {said}\n`kanna task merge --cancel {task}` takes it out of the queue instead.")
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(8)]
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

pub fn test_pipeline(command: &str, agent: &str) -> String {
    format!("{TEST} = run {}\n{JUDGE} = {agent} prompt {PROMPT} verdict\n{TEST} -[pass]-> done\n{TEST} -[fail]-> {JUDGE}\n{JUDGE} -[pass]-> done\n", command.trim())
}

pub fn readable(output: &str) -> String {
    let text = runs::plain(&output.replace("\r\n", "\n").replace('\r', "\u{1}"));
    let lines: Vec<&str> = text.lines().map(|line| line.rsplit('\u{1}').find(|part| !part.is_empty()).unwrap_or("")).collect();
    lines.join("\n") + "\n"
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

fn same_folder(path: &str) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

fn tail(path: &Path, most: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(most))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Some(if size > most { text.split_once('\n').map_or(text.clone(), |(_, rest)| rest.to_string()) } else { text })
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
    fn kanna_s_pipeline_runs_merge_test_as_a_command_and_starts_the_judge_only_when_it_fails() {
        let dir = std::env::temp_dir().join(format!("kanna-merge-queue-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".kanna")).unwrap();
        assert_eq!(pipeline_text(&dir, &dir), None, "no merge.test and no pipeline: nothing to test with");
        fs::write(dir.join(".kanna/config.json"), r#"{"version": 1, "merge": {"test": "sh test.sh"}, "tasks": {"timeout": 60}}"#).unwrap();
        let (text, shown) = pipeline_text(&dir, &dir).unwrap();
        assert!(shown.starts_with("Kanna's merge-queue.kojo (merge.test, a failure judged by "), "{shown}");
        let pipeline = kanna_pipe::parse(&text).unwrap();
        assert_eq!(pipeline.start, TEST);
        let test = pipeline.stage(TEST).unwrap();
        assert!(matches!(&test.kind, kanna_pipe::Kind::Run { command } if command == "sh test.sh"), "{text}");
        assert_eq!(test.within, None, "the tests have no time limit");
        assert_eq!(runs::command_timeout(test.within), 0, "whatever tasks.timeout is");
        assert_eq!(runs::command_timeout(Some(Duration::from_secs(900))), 900, "a repo's own pipeline can still set within");
        let judge = pipeline.stage(JUDGE).unwrap();
        assert!(judge.agent().is_some_and(|a| a.verdict && kanna_pipe::AGENTS.contains(&a.agent.as_str())), "{text}");
        assert_eq!((judge.prompt(), judge.within), (Some(PROMPT), None));
        assert_eq!(pipeline.max_sessions(&kanna_pipe::Sizes::new()), 1, "at most one agent session an entry");
        let fresh = || runs::Doc { taken: vec![0; pipeline.arrows.len()], ..runs::Doc::default() };
        assert_eq!(runs::decide(&pipeline, &mut fresh(), TEST, Some(kanna_pipe::Verdict::Pass), Some(vec![])), runs::Next::Done, "a pass lands with no agent");
        let mut doc = fresh();
        assert_eq!(runs::decide(&pipeline, &mut doc, TEST, Some(kanna_pipe::Verdict::Fail), Some(vec!["FAILED a".into()])), runs::Next::Stage { name: JUDGE.into(), reset: false });
        assert_eq!(runs::decide(&pipeline, &mut doc, JUDGE, Some(kanna_pipe::Verdict::Pass), None), runs::Next::Done);
        let mut doc = fresh();
        runs::decide(&pipeline, &mut doc, TEST, Some(kanna_pipe::Verdict::Fail), None);
        assert_eq!(runs::decide(&pipeline, &mut doc, JUDGE, Some(kanna_pipe::Verdict::Fail), None), runs::Next::Stuck);
        assert!(runs::ends_owned(true, Some("timed out")) && !runs::ends_owned(false, Some("timed out")) && !runs::ends_owned(true, None), "a queue run's stage that times out ends the entry, with no judge");
        fs::create_dir_all(dir.join(".kanna/pipelines")).unwrap();
        fs::write(dir.join(".kanna/pipelines/merge-queue.kojo"), "check = run true\ncheck -[pass]-> done\n").unwrap();
        assert!(pipeline_text(&dir, &dir).unwrap().0.starts_with("check = run true"), "the repo's own pipeline wins");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_merge_test_that_a_pipeline_would_cut_short_is_refused() {
        for fit in ["scripts/merge-test.sh", "cargo test --workspace -- --test-threads 4", "sh -c 'make check && make e2e'"] {
            assert!(fits(fit), "{fit}");
        }
        for unfit in ["make test # the lot", "make test within 5m", "make test on mini", "make a\nmake b"] {
            assert!(!fits(unfit), "{unfit}");
        }
    }

    #[test]
    fn the_judge_s_prompt_names_main_the_merge_and_how_to_run_a_long_command() {
        let tested = Tested { command: "scripts/merge-test.sh", name: "Add a", branch: "add-a", main: "main", base: "0123456789abcdef", merge: "fedcba9876543210", queue_branch: "queue/task-1-1" };
        let said = judge_prompt(&tested);
        assert!(said.contains("merged onto main at 01234567, as commit fedcba98. The queue ran the repo's tests on it (`scripts/merge-test.sh`) and they failed."), "{said}");
        assert!(said.contains("under \"Feedback from test\" below"), "{said}");
        assert!(said.contains("`git checkout -q --detach 01234567`, and `git checkout -q -f queue/task-1-1` to come back"), "{said}");
        assert!(said.contains("    \"$KANNA_CLI\" run command 'COMMAND'\n") && said.contains("Don't end your turn to wait for it."), "{said}");
        assert!(!said.contains("in the background and wait"), "{said}");
        let pipeline = kanna_pipe::parse(&test_pipeline("sh test.sh", "claude")).unwrap();
        let feedback = runs::Feedback { stage: TEST.into(), text: "FAILED a".into(), full: "/w/.kanna/tmp/stage-1.log".into() };
        let whole = runs::prompt(pipeline.stage(JUDGE).unwrap(), &said, "Merge branch 'add-a': Add a", None, Some(&feedback), &BTreeMap::new(), ".kanna/tmp/answer-2.md");
        assert!(whole.contains("# Feedback from test\n\nFAILED a\n\nAll of its output is in `/w/.kanna/tmp/stage-1.log`.\n"), "{whole}");
    }

    #[test]
    fn a_kept_log_reads_as_the_terminal_showed_it() {
        let output = "Running: sh test.sh\r\n\r\n\x1b[32m   Compiling\x1b[0m a\r\n    Building [=>  ] 1/3\r    Building [==> ] 2/3\r\x1b[K    Finished\r\ntest a ... ok\r\n";
        assert_eq!(readable(output), "Running: sh test.sh\n\n   Compiling a\n    Finished\ntest a ... ok\n");
    }

    #[test]
    fn a_paused_entry_s_news_says_what_happened_and_both_ways_out() {
        let said = needs_you("Add a", "task-1", "judge's turn ended without it reporting it was done or what it needs. `kanna run resume run-1 \"…\"` carries it on with your answer.");
        assert_eq!(said, "\"Add a\" waits in the merge queue for you, and the entries behind it wait too. judge's turn ended without it reporting it was done or what it needs. `kanna run resume run-1 \"…\"` carries it on with your answer.\n`kanna task merge --cancel task-1` takes it out of the queue instead.");
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

    fn judging(name: &str, own: bool) -> (Rc<Queue>, PathBuf) {
        let dir = scratch(name);
        let repo = dir.join("repo");
        fs::create_dir_all(repo.join(".kanna")).unwrap();
        fs::write(repo.join(".kanna/config.json"), r#"{"version": 1, "merge": {"test": "cargo test"}}"#).unwrap();
        let db = Rc::new(Db::open(&dir.join("state")));
        let queue = Rc::new(Queue::open(db, Weak::new(), Weak::new()));
        let row = QueueRow { id: "queue-1".into(), repo: repo.to_string_lossy().into_owned(), task: "task-1".into(), head: "h".into(), state: "testing".into(), queued_at: 1.0, ended_at: None, run: Some("run-1".into()), doc: json!({ "own": own, "base": "base0", "merge": "merge0", "name": "A", "main": "main", "branch": "a", "stage": "test", "needs": "judge stopped without reporting", "told": "x" }) };
        queue.db.put_queued(&row).unwrap();
        let run = crate::db::RunRow { id: "run-1".into(), task: "task-1".into(), repo: row.repo.clone(), pipeline: PIPELINE.into(), revision_hash: "r".into(), branch: "queue/task-1-1".into(), state: "stopped".into(), started_at: 1.0, ended_at: Some(2.0), outcome: Some("failed".into()), doc: json!({}) };
        queue.db.put_run(&run, None).unwrap();
        queue.live.borrow_mut().push(row);
        (queue, dir)
    }

    fn visited(queue: &Queue, stage: &str, kind: &str, verdict: &str, doc: Value) {
        let row = crate::db::StageRow { id: 0, run: "run-1".into(), stage: stage.into(), visit: 1, instance: None, kind: kind.into(), agent: (kind == "agent").then(|| "claude".into()), machine: "here".into(), started_at: 1.0, ended_at: Some(2.0), verdict: Some(verdict.into()), session: None, terminal: None, worktree: None, branch: None, doc };
        queue.db.put_stage(&row).unwrap();
    }

    fn state(queue: &Queue) -> QueueRow {
        queue.db.queued(false).unwrap().into_iter().find(|q| q.id == "queue-1").unwrap()
    }

    #[test]
    fn tests_that_pass_land_it_with_no_judge_and_no_note() {
        let (queue, dir) = judging("passes", false);
        visited(&queue, TEST, "run", "pass", json!({ "feedback": "test result: ok." }));
        queue.run_ended("run-1", Ok("merge0".into()));
        let landed = state(&queue);
        assert_eq!(landed.state, "landing", "{:?}", landed.doc);
        assert_eq!(landed.doc["tested"], "merge0");
        assert_eq!((landed.doc.get("result"), landed.doc.get("note")), (None, None));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_the_judge_says_isn_t_the_branch_s_lands_it_with_the_note() {
        let (queue, dir) = judging("not-the-branch", false);
        let note = "1 test fails, not the branch's: known_broken\nknown_broken fails on main at base0 too.";
        visited(&queue, TEST, "run", "fail", json!({ "feedback": "test known_broken ... FAILED" }));
        visited(&queue, JUDGE, "agent", "pass", json!({ "answer": note }));
        queue.run_ended("run-1", Ok("merge0".into()));
        let landed = state(&queue);
        assert_eq!(landed.state, "landing", "{:?}", landed.doc);
        assert_eq!(landed.doc["tested"], "merge0");
        assert_eq!(landed.doc["result"], json!({ "verdict": "pass", "note": note }));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_the_judge_blames_on_the_branch_refuses_it_with_the_reason() {
        let (queue, dir) = judging("the-branch", false);
        let note = "broken_thing fails: the branch removed its fixture\nbroken_thing passes on main and fails here, alone too.";
        visited(&queue, TEST, "run", "fail", json!({ "feedback": "test broken_thing ... FAILED" }));
        visited(&queue, JUDGE, "agent", "fail", json!({ "answer": note, "feedback": note }));
        queue.run_ended("run-1", Err("judge failed".into()));
        let failed = state(&queue);
        assert_eq!(failed.state, "failed");
        assert_eq!(failed.doc["why"], "broken_thing fails: the branch removed its fixture");
        assert_eq!(failed.doc["result"], json!({ "verdict": "fail", "note": note }));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stage_of_a_repo_s_own_pipeline_that_runs_out_of_time_fails_the_entry_as_timed_out() {
        let (queue, dir) = judging("timed-out", true);
        visited(&queue, "check", "run", "fail", json!({ "feedback": "timed out\ntest slow_thing has been running for over 60 seconds", "note": "timed out" }));
        queue.run_ended("run-1", Err("check timed out".into()));
        let failed = state(&queue);
        assert_eq!(failed.doc["why"], "check timed out");
        assert_eq!(failed.doc["feedback"], "test slow_thing has been running for over 60 seconds");
        assert_eq!(failed.doc.get("result"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_judge_that_exits_without_a_verdict_fails_the_entry_with_why() {
        let (queue, dir) = judging("no-verdict", false);
        visited(&queue, TEST, "run", "fail", json!({ "feedback": "test broken_thing ... FAILED" }));
        visited(&queue, JUDGE, "agent", "stopped", json!({}));
        queue.run_ended("run-1", Err("judge's agent exited without reporting it was done".into()));
        let failed = state(&queue);
        assert_eq!((failed.state.as_str(), failed.doc["why"].as_str()), ("failed", Some("judge's agent exited without reporting it was done")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_repo_s_own_pipeline_fails_on_its_stage_with_the_stage_s_lines() {
        let (queue, dir) = judging("own", true);
        visited(&queue, "check", "run", "fail", json!({ "feedback": "FAILED: broken is here" }));
        queue.run_ended("run-1", Err("check failed".into()));
        let failed = state(&queue);
        assert_eq!((failed.state.as_str(), failed.doc["why"].as_str()), ("failed", Some("check failed")));
        assert_eq!(failed.doc["feedback"], "FAILED: broken is here");
        assert_eq!(failed.doc.get("result"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn places_read_as_ordinals() {
        let said: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101].iter().map(|n| ordinal(*n)).collect();
        assert_eq!(said, ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st"]);
    }
}
