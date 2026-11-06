use crate::db::Db;
use crate::tasks::{config_files, failed as refusal, prompts, started_as, task_agent, Tasks, WINDOWLESS, WRITER_ROLE};
use kanna_mux::json::{json, num, Value};
use kanna_mux::{project, CoreHandle, KmuxError, PaneState, WeakCore};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TICK: Duration = Duration::from_secs(1);

const TIME_LIMIT: f64 = 1200.0;

const IDLE_LIMIT: f64 = 90.0;

const BLOCKED_LIMIT: f64 = 60.0;

const QUIT_WAIT: f64 = 60.0;

const COOLDOWN: f64 = 600.0;

pub type Panes = Box<dyn Fn() -> Vec<String>>;

pub type Changed = Box<dyn Fn(&str, Option<Written>)>;

#[derive(Clone)]
pub struct Written {
    pub round: i64,
    pub title: String,
    pub about: Option<String>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

struct Running {
    pane: Option<String>,
    agent: String,
    about: Option<String>,
    since: f64,
    written: Option<f64>,
    idle: Option<f64>,
    blocked: Option<f64>,
    quitting: bool,
    why: Option<String>,
}

#[derive(Default)]
struct Repo {
    running: Option<Running>,
    queue: VecDeque<Option<String>>,
    failed: Option<(f64, String)>,
}

pub struct Writers {
    db: Rc<Db>,
    tasks: Rc<Tasks>,
    core: WeakCore,
    panes: Panes,
    repos: RefCell<BTreeMap<String, Repo>>,
    pub on_change: RefCell<Option<Changed>>,
}

pub fn role(about: Option<&str>) -> String {
    match about {
        Some(about) => format!("{WRITER_ROLE} Its subject: {about}"),
        None => WRITER_ROLE.to_string(),
    }
}

impl Writers {
    pub fn install(db: Rc<Db>, tasks: Rc<Tasks>, core: &CoreHandle, panes: Panes) -> Rc<Writers> {
        let writers = Rc::new(Writers { db, tasks, core: core.weak(), panes, repos: RefCell::default(), on_change: RefCell::default() });
        Writers::ticking(Rc::downgrade(&writers));
        writers
    }

    fn ticking(this: Weak<Writers>) {
        crate::runtime::spawn("the quiz writers' watch", async move {
            loop {
                tokio::time::sleep(TICK).await;
                let Some(writers) = this.upgrade() else { return };
                writers.tick();
            }
        });
    }

    fn changed(&self, repo: &str, written: Option<Written>) {
        if let Some(changed) = self.on_change.borrow().as_ref() {
            changed(repo, written);
        }
    }

    pub fn writing(&self, repo: &str) -> Value {
        let repos = self.repos.borrow();
        let Some(state) = repos.get(repo) else { return json!({ "writing": null, "failed": null }) };
        let writing = state.running.as_ref().filter(|r| r.written.is_none()).map(|r| json!({ "since": num(r.since), "about": r.about }));
        let queued: Vec<Value> = state.queue.iter().flatten().map(|about| json!(about)).collect();
        json!({ "writing": writing, "queued": queued, "failed": state.failed.as_ref().map(|(_, why)| why.as_str()) })
    }

    pub fn subject(&self, repo: &str) -> Option<String> {
        self.repos.borrow().get(repo)?.running.as_ref()?.about.clone()
    }

    pub fn played(self: &Rc<Self>, repo: &str) {
        self.ahead(repo, None, false);
    }

    pub fn ahead(self: &Rc<Self>, repo: &str, about: Option<String>, again: bool) {
        let now = now();
        {
            let mut repos = self.repos.borrow_mut();
            let state = repos.entry(repo.to_string()).or_default();
            if again {
                state.failed = None;
            }
            match &about {
                Some(subject) => {
                    state.failed = None;
                    if state.running.as_ref().is_some_and(|r| r.written.is_none() && r.about.as_ref() == Some(subject)) || state.queue.iter().flatten().any(|q| q == subject) {
                        return;
                    }
                }
                None => {
                    if state.running.as_ref().is_some_and(|r| r.written.is_none() && r.about.is_none()) || state.queue.iter().any(Option::is_none) {
                        return;
                    }
                    if state.failed.as_ref().is_some_and(|(at, _)| now - at < COOLDOWN) {
                        return;
                    }
                }
            }
        }
        if about.is_none() && self.db.round_waits(repo).unwrap_or(false) {
            return;
        }
        let mut repos = self.repos.borrow_mut();
        let state = repos.entry(repo.to_string()).or_default();
        state.queue.push_back(about);
        let idle = state.running.is_none();
        drop(repos);
        if idle {
            self.next(repo);
        } else {
            self.changed(repo, None);
        }
    }

    pub fn written(&self, repo: &str, written: Written) {
        if let Some(running) = self.repos.borrow_mut().get_mut(repo).and_then(|s| s.running.as_mut()) {
            running.written = Some(now());
        }
        self.changed(repo, Some(written));
    }

    fn next(self: &Rc<Self>, repo: &str) {
        let about = {
            let mut repos = self.repos.borrow_mut();
            let Some(state) = repos.get_mut(repo).filter(|s| s.running.is_none()) else { return };
            match state.queue.pop_front() {
                Some(about) => about,
                None => return,
            }
        };
        if about.is_none() && self.db.round_waits(repo).unwrap_or(false) {
            return self.next(repo);
        }
        if let Err(error) = self.start(repo, about) {
            log!("kanna-daemon: couldn't start a quiz writer in {repo}: {}", error.message);
            self.fail(repo, format!("The quiz writer couldn't start: {}", error.message));
        }
    }

    fn start(self: &Rc<Self>, repo: &str, about: Option<String>) -> Result<(), KmuxError> {
        let core = self.core.upgrade().ok_or_else(|| KmuxError::new("app_gone", "the daemon is stopping"))?;
        let folder = Path::new(repo);
        if !folder.is_dir() {
            return Err(KmuxError::new("not_found", format!("{repo} isn't on this machine")));
        }
        let files = config_files(folder);
        let agent = task_agent(&files);
        let cmd = started_as(&agent, &role(about.as_deref()), None, prompts(&files));
        let open = json!({ "type": "term", "quiz": true, "claim": WINDOWLESS, "cwd": repo, "cmd": cmd, "wait": false });
        let running = Running { pane: None, agent, about, since: now(), written: None, idle: None, blocked: None, quitting: false, why: None };
        self.repos.borrow_mut().entry(repo.to_string()).or_default().running = Some(running);
        log!("kanna-daemon: a quiz writer starts in {repo}");
        let (this, path) = (Rc::downgrade(self), repo.to_string());
        core.handle(json!({ "cmd": "open", "args": open }), move |reply| {
            let Some(this) = this.upgrade() else { return };
            match refusal(&reply) {
                Some(error) => {
                    if let Some(state) = this.repos.borrow_mut().get_mut(&path) {
                        state.running = None;
                    }
                    this.fail(&path, format!("The quiz writer couldn't start: {}", error.message));
                }
                None => {
                    let pane = reply.get("pane").and_then(|p| p.get("id")).and_then(Value::as_str).map(String::from);
                    if let Some(running) = this.repos.borrow_mut().get_mut(&path).and_then(|s| s.running.as_mut()) {
                        running.pane = pane;
                    }
                }
            }
        });
        self.changed(repo, None);
        Ok(())
    }

    fn fail(self: &Rc<Self>, repo: &str, why: String) {
        log!("kanna-daemon: quiz writer in {repo}: {why}");
        self.repos.borrow_mut().entry(repo.to_string()).or_default().failed = Some((now(), why));
        self.changed(repo, None);
    }

    fn adopt(&self) {
        let Some(core) = self.core.upgrade() else { return };
        for pane in (self.panes)() {
            let Some(found) = core.pane(&pane).filter(|p| p.state != PaneState::Exited && p.state != PaneState::Failed) else { continue };
            let Some(repo) = project(found.cwd.as_deref()).map(|r| r.to_string_lossy().to_string()) else { continue };
            let mut repos = self.repos.borrow_mut();
            let state = repos.entry(repo).or_default();
            if state.running.is_none() {
                let files = config_files(Path::new(found.cwd.as_deref().unwrap_or("/")));
                state.running = Some(Running { pane: Some(pane), agent: task_agent(&files), about: None, since: now(), written: None, idle: None, blocked: None, quitting: false, why: None });
            }
        }
    }

    fn tick(self: &Rc<Self>) {
        self.adopt();
        let repos: Vec<String> = self.repos.borrow().iter().filter(|(_, s)| s.running.is_some()).map(|(repo, _)| repo.clone()).collect();
        for repo in repos {
            self.tick_repo(&repo);
        }
    }

    fn close(&self, pane: &str) {
        if let Some(core) = self.core.upgrade() {
            core.handle(json!({ "cmd": "close", "args": { "pane": pane } }), |_| {});
        }
    }

    fn tick_repo(self: &Rc<Self>, repo: &str) {
        let Some(core) = self.core.upgrade() else { return };
        let time = now();
        let mut quit = None;
        let mut gone = None;
        {
            let mut repos = self.repos.borrow_mut();
            let Some(running) = repos.get_mut(repo).and_then(|s| s.running.as_mut()) else { return };
            let Some(pane) = running.pane.clone() else { return };
            let alive = core.pane(&pane).is_some_and(|p| p.state != PaneState::Exited && p.state != PaneState::Failed);
            if !alive {
                gone = Some(pane);
            } else if !running.quitting {
                let status = self.tasks.pane_status(&pane);
                running.idle = (status.as_deref() == Some("idle")).then(|| running.idle.unwrap_or(time));
                running.blocked = (status.as_deref() == Some("blocked")).then(|| running.blocked.unwrap_or(time));
                let why = match running.written {
                    Some(at) if status.as_deref() == Some("idle") || time - at > QUIT_WAIT => Some(None),
                    Some(_) => None,
                    None if time - running.since > TIME_LIMIT => Some(Some("The quiz writer ran out of time without writing a round")),
                    None if running.blocked.is_some_and(|at| time - at > BLOCKED_LIMIT) => Some(Some("The quiz writer stopped to ask permission, and nobody can answer it there: set agents.prompts to skip in ~/.kanna-v4/config.json")),
                    None if running.idle.is_some_and(|at| time - at > IDLE_LIMIT) => Some(Some("The quiz writer finished without writing a round")),
                    None => None,
                };
                if let Some(why) = why {
                    running.quitting = true;
                    running.why = why.map(String::from);
                    quit = Some((pane, running.agent.clone()));
                }
            }
        }
        if let Some((pane, agent)) = quit {
            let (this, path) = (Rc::downgrade(self), repo.to_string());
            self.tasks.end_agent(&pane, &agent, move |_| {
                if let Some(this) = this.upgrade() {
                    this.ended(&path);
                }
            });
        }
        if gone.is_some() {
            self.ended(repo);
        }
    }

    fn ended(self: &Rc<Self>, repo: &str) {
        let Some(running) = self.repos.borrow_mut().get_mut(repo).and_then(|s| s.running.take()) else { return };
        if let Some(pane) = &running.pane {
            self.close(pane);
        }
        match (running.written, running.why) {
            (Some(_), _) => {
                if let Some(state) = self.repos.borrow_mut().get_mut(repo) {
                    state.failed = None;
                }
            }
            (None, Some(why)) => self.fail(repo, why),
            (None, None) => self.fail(repo, "The quiz writer quit without writing a round".into()),
        }
        self.changed(repo, None);
        self.next(repo);
    }
}
