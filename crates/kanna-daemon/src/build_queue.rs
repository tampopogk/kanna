use kanna_mux::json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DETACHED_FOR: Duration = Duration::from_secs(600);

const RECENT: usize = 20;

const DEFAULT_JOBS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    Queued,
    Sending,
    Running,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Queued => "queued",
            Stage::Sending => "sending",
            Stage::Running => "running",
        }
    }

    fn parse(name: &str) -> Stage {
        match name {
            "sending" => Stage::Sending,
            "running" => Stage::Running,
            _ => Stage::Queued,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub machine: String,
    pub repo: String,
    pub task: String,
    pub label: String,
    pub command: String,
    pub ui: bool,
    pub needs: Value,
    pub submitted: u64,
    pub stage: Stage,
    pub guest: Option<String>,
    pub started: Option<u64>,
    pub cancel: bool,
    relay: u64,
    detached: Option<Instant>,
}

impl Job {
    pub fn new(id: &str, header: &Value) -> Job {
        let mirror = &header["mirror"];
        let text = |key: &str| mirror[key].as_str().unwrap_or_default().to_string();
        let argv: Vec<&str> = header["argv"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let command: String = argv.join(" ").chars().take(200).collect();
        Job {
            id: id.to_string(),
            machine: text("machine"),
            repo: text("repo"),
            task: text("task"),
            label: header["label"].as_str().filter(|l| !l.is_empty()).map_or_else(|| text("task"), |l| l.chars().take(120).collect()),
            command,
            ui: header["ui"].as_bool().unwrap_or(false),
            needs: header.get("needs").cloned().unwrap_or(Value::Null),
            submitted: now(),
            stage: Stage::Queued,
            guest: None,
            started: None,
            cancel: false,
            relay: 0,
            detached: None,
        }
    }

    pub fn mirror(&self) -> String {
        format!("{}/{}/{}", self.machine, self.repo, self.task)
    }

    fn to_json(&self) -> Value {
        json!({ "job": self.id, "machine": self.machine, "repo": self.repo, "task": self.task, "label": self.label, "command": self.command, "ui": self.ui, "needs": self.needs, "submitted": self.submitted, "stage": self.stage.name(), "guest": self.guest, "started": self.started, "cancel": self.cancel })
    }

    fn from_json(value: &Value) -> Option<Job> {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_string();
        let id = value["job"].as_str()?.to_string();
        let stage = Stage::parse(value["stage"].as_str().unwrap_or_default());
        Some(Job { id, machine: text("machine"), repo: text("repo"), task: text("task"), label: text("label"), command: text("command"), ui: value["ui"].as_bool().unwrap_or(false), needs: value["needs"].clone(), submitted: value["submitted"].as_u64().unwrap_or(0), stage, guest: value["guest"].as_str().map(String::from), started: value["started"].as_u64(), cancel: value["cancel"].as_bool().unwrap_or(false), relay: 0, detached: (stage == Stage::Queued).then(Instant::now) })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Guest {
    pub name: String,
    pub mirrors: usize,
    pub caps: Value,
}

#[derive(Debug, PartialEq)]
pub enum Poll {
    Waiting(usize),
    Granted(String),
    Gone(Option<Value>),
}

#[derive(Debug, PartialEq)]
pub enum Cancelled {
    Dequeued,
    Asked(Option<String>),
    Finished,
    Unknown,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn version_matches(wanted: &str, have: &str) -> bool {
    let wanted = wanted.trim_end_matches(".x");
    have == wanted || have.strip_prefix(wanted).is_some_and(|rest| rest.starts_with('.'))
}

pub fn satisfies(needs: &Value, caps: &Value) -> bool {
    let wants = |key: &str| needs[key].as_str().filter(|w| !w.is_empty());
    let exact = wants("arch").is_none_or(|arch| caps["arch"].as_str() == Some(arch));
    let versions = ["xcode", "macos"].iter().all(|key| wants(key).is_none_or(|w| caps[*key].as_str().is_some_and(|have| version_matches(w, have))));
    let runtime = wants("runtime").is_none_or(|w| caps["runtimes"].as_array().into_iter().flatten().filter_map(Value::as_str).any(|have| version_matches(w, have)));
    exact && versions && runtime
}

pub fn unmet(needs: &Value) -> String {
    needs.as_object().into_iter().flatten().filter_map(|(key, value)| value.as_str().map(|v| format!("{key} {v}"))).collect::<Vec<_>>().join(", ")
}

struct State {
    jobs: Vec<Job>,
    recent: VecDeque<Value>,
    relays: u64,
    limit: usize,
    detached_for: Duration,
}

pub struct Queue {
    file: PathBuf,
    state: Mutex<State>,
}

impl Queue {
    pub fn load(guests_dir: &Path) -> Queue {
        let saved: Value = std::fs::read(guests_dir.join("queue.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let jobs = saved["jobs"].as_array().into_iter().flatten().filter_map(Job::from_json).collect();
        let recent = saved["recent"].as_array().cloned().unwrap_or_default().into();
        let pool: Value = std::fs::read(guests_dir.join("pool.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let limit = pool["jobs"].as_u64().filter(|n| *n > 0).map_or(DEFAULT_JOBS, |n| n as usize);
        Queue { file: guests_dir.join("queue.json"), state: Mutex::new(State { jobs, recent, relays: 0, limit, detached_for: DETACHED_FOR }) }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn save(&self, state: &State) {
        let body = serde_json::to_vec(&json!({ "jobs": state.jobs.iter().map(Job::to_json).collect::<Vec<_>>(), "recent": state.recent })).unwrap_or_default();
        if let Some(parent) = self.file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let temp = self.file.with_extension("tmp");
        let _ = std::fs::write(&temp, body).and_then(|()| std::fs::rename(&temp, &self.file));
    }

    pub fn submit(&self, job: Job) -> u64 {
        let mut state = self.state();
        Self::expire(&mut state);
        state.relays += 1;
        let relay = state.relays;
        if state.recent.iter().any(|r| r["job"] == job.id.as_str()) {
            return relay;
        }
        match state.jobs.iter_mut().find(|j| j.id == job.id) {
            Some(known) => {
                known.relay = relay;
                known.detached = None;
            }
            None => {
                let mut job = job;
                job.relay = relay;
                state.jobs.push(job);
            }
        }
        self.save(&state);
        relay
    }

    pub fn has_guest(&self, id: &str) -> Option<String> {
        self.state().jobs.iter().find(|j| j.id == id).and_then(|j| j.guest.clone())
    }

    pub fn poll(&self, id: &str, relay: u64, guests: &[Guest], routes: &Value) -> Poll {
        let mut state = self.state();
        Self::expire(&mut state);
        let changed = Self::assign(&mut state, guests, routes);
        if changed {
            self.save(&state);
        }
        let Some(at) = state.jobs.iter().position(|j| j.id == id && j.relay == relay) else {
            let ended = state.recent.iter().find(|r| r["job"] == id).cloned();
            return Poll::Gone(ended);
        };
        match (&state.jobs[at].stage, &state.jobs[at].guest) {
            (Stage::Queued, _) | (_, None) => Poll::Waiting(state.jobs[..at].iter().filter(|j| j.stage == Stage::Queued).count()),
            (_, Some(guest)) => Poll::Granted(guest.clone()),
        }
    }

    fn expire(state: &mut State) {
        let life = state.detached_for;
        state.jobs.retain(|j| j.stage != Stage::Queued || j.detached.is_none_or(|at| at.elapsed() < life));
    }

    fn assign(state: &mut State, guests: &[Guest], routes: &Value) -> bool {
        let mut changed = false;
        for at in 0..state.jobs.len() {
            let active = state.jobs.iter().filter(|j| j.stage != Stage::Queued).count();
            if active >= state.limit {
                break;
            }
            let job = &state.jobs[at];
            if job.stage != Stage::Queued || job.detached.is_some() {
                continue;
            }
            let Some(guest) = Self::guest_for(&state.jobs, job, guests, routes) else { continue };
            let job = &mut state.jobs[at];
            job.stage = Stage::Sending;
            job.guest = Some(guest);
            changed = true;
        }
        changed
    }

    fn guest_for(jobs: &[Job], job: &Job, guests: &[Guest], routes: &Value) -> Option<String> {
        let busy_mirror = jobs.iter().any(|j| j.stage != Stage::Queued && j.mirror() == job.mirror());
        if busy_mirror {
            return None;
        }
        let compatible: Vec<&Guest> = guests.iter().filter(|g| job.needs.is_null() || satisfies(&job.needs, &g.caps)).collect();
        let free = |guest: &Guest| !jobs.iter().any(|j| j.stage != Stage::Queued && j.ui == job.ui && j.guest.as_deref() == Some(guest.name.as_str()));
        if let Some(home) = routes["mirrors"][job.mirror()].as_str().and_then(|name| compatible.iter().find(|g| g.name == name)) {
            return free(home).then(|| home.name.clone());
        }
        compatible.into_iter().filter(|g| free(g)).min_by_key(|g| g.mirrors).map(|g| g.name.clone())
    }

    pub fn detach(&self, id: &str, relay: u64) {
        let mut state = self.state();
        if let Some(job) = state.jobs.iter_mut().find(|j| j.id == id && j.relay == relay && j.stage != Stage::Running) {
            job.stage = Stage::Queued;
            job.guest = None;
            job.relay = 0;
            job.detached = Some(Instant::now());
            self.save(&state);
        }
    }

    pub fn running(&self, id: &str, started: Option<u64>) {
        let mut state = self.state();
        if let Some(job) = state.jobs.iter_mut().find(|j| j.id == id && j.stage != Stage::Running) {
            job.stage = Stage::Running;
            job.started = started.or(Some(now()));
            self.save(&state);
        }
    }

    pub fn finish(&self, id: &str, status: &Value) {
        let mut state = self.state();
        let Some(at) = state.jobs.iter().position(|j| j.id == id) else { return };
        let job = state.jobs.remove(at);
        Self::keep(&mut state, &job, status);
        self.save(&state);
    }

    fn keep(state: &mut State, job: &Job, status: &Value) {
        let mut record = job.to_json();
        record["status"] = status.clone();
        record["finished"] = json!(now());
        state.recent.push_front(record);
        state.recent.truncate(RECENT);
    }

    pub fn cancel(&self, id: &str) -> Cancelled {
        let mut state = self.state();
        Self::expire(&mut state);
        let Some(at) = state.jobs.iter().position(|j| j.id == id) else {
            return if state.recent.iter().any(|r| r["job"] == id) { Cancelled::Finished } else { Cancelled::Unknown };
        };
        if state.jobs[at].stage == Stage::Queued {
            let job = state.jobs.remove(at);
            Self::keep(&mut state, &job, &json!({ "phase": "cancelled", "job": id }));
            self.save(&state);
            return Cancelled::Dequeued;
        }
        state.jobs[at].cancel = true;
        let guest = state.jobs[at].guest.clone();
        self.save(&state);
        Cancelled::Asked(guest)
    }

    pub fn wants_cancel(&self, id: &str) -> bool {
        self.state().jobs.iter().any(|j| j.id == id && j.cancel)
    }

    pub fn resumed(&self) -> Vec<(String, String)> {
        self.state().jobs.iter().filter(|j| j.stage != Stage::Queued).filter_map(|j| Some((j.id.clone(), j.guest.clone()?))).collect()
    }

    pub fn list(&self) -> Value {
        let mut state = self.state();
        Self::expire(&mut state);
        let state = &*state;
        let mut jobs: Vec<&Job> = state.jobs.iter().filter(|j| j.stage != Stage::Queued).collect();
        jobs.extend(state.jobs.iter().filter(|j| j.stage == Stage::Queued));
        let mut ahead = 0;
        let list: Vec<Value> = jobs.into_iter().map(|job| {
            let mut record = job.to_json();
            if job.stage == Stage::Queued {
                record["ahead"] = json!(ahead);
                ahead += 1;
            }
            record
        }).collect();
        json!({ "jobs": list, "recent": state.recent, "limit": state.limit })
    }

    pub fn waiting(&self) -> usize {
        self.state().jobs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-queue-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn job(id: &str, task: &str, ui: bool) -> Job {
        Job::new(id, &json!({ "mirror": { "machine": "linux", "repo": "app", "task": task }, "argv": ["xcodebuild", "build"], "ui": ui, "label": task }))
    }

    fn guest(name: &str) -> Guest {
        Guest { name: name.into(), mirrors: 0, caps: json!({ "arch": "aarch64", "xcode": "26.1", "macos": "26.0", "runtimes": ["26.1"] }) }
    }

    fn no_routes() -> Value {
        json!({ "jobs": {}, "mirrors": {} })
    }

    #[test]
    fn jobs_run_in_the_order_they_came_and_the_next_starts_when_the_slot_frees() {
        let queue = Queue::load(&temp("order"));
        let guests = [guest("vm-a")];
        let (a, b, c) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", false)), queue.submit(job("c", "three", false)));
        assert_eq!(queue.poll("b", b, &guests, &no_routes()), Poll::Waiting(0));
        assert_eq!(queue.poll("c", c, &guests, &no_routes()), Poll::Waiting(1));
        assert_eq!(queue.poll("a", a, &guests, &no_routes()), Poll::Granted("vm-a".into()));
        let listed = queue.list();
        let order: Vec<&str> = listed["jobs"].as_array().unwrap().iter().map(|j| j["job"].as_str().unwrap()).collect();
        assert_eq!(order, ["a", "b", "c"]);
        queue.finish("a", &json!({ "phase": "exited", "code": 0 }));
        assert_eq!(queue.poll("b", b, &guests, &no_routes()), Poll::Granted("vm-a".into()));
        assert_eq!(queue.poll("c", c, &guests, &no_routes()), Poll::Waiting(0));
        assert_eq!(queue.list()["recent"][0]["job"], "a");
    }

    #[test]
    fn a_guest_has_one_build_slot_and_one_ui_slot_and_two_jobs_of_a_task_never_overlap() {
        let queue = Queue::load(&temp("slots"));
        let guests = [guest("vm-a")];
        let (build, ui, same) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", true)), queue.submit(job("c", "two", false)));
        assert_eq!(queue.poll("a", build, &guests, &no_routes()), Poll::Granted("vm-a".into()));
        assert_eq!(queue.poll("b", ui, &guests, &no_routes()), Poll::Granted("vm-a".into()));
        queue.finish("a", &Value::Null);
        assert!(matches!(queue.poll("c", same, &guests, &no_routes()), Poll::Waiting(_)), "task two's mirror is busy");
    }

    #[test]
    fn the_host_limit_holds_jobs_back_even_with_free_guests() {
        let dir = temp("limit");
        std::fs::write(dir.join("pool.json"), r#"{"jobs":1}"#).unwrap();
        let queue = Queue::load(&dir);
        let guests = [guest("vm-a"), guest("vm-b")];
        let (a, b) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", false)));
        assert_eq!(queue.poll("a", a, &guests, &no_routes()), Poll::Granted("vm-a".into()));
        assert_eq!(queue.poll("b", b, &guests, &no_routes()), Poll::Waiting(0));
    }

    #[test]
    fn a_mirror_goes_back_to_its_guest_and_a_new_one_to_the_emptiest() {
        let queue = Queue::load(&temp("mirror"));
        let guests = [Guest { mirrors: 3, ..guest("vm-a") }, guest("vm-b")];
        let routes = json!({ "jobs": {}, "mirrors": { "linux/app/one": "vm-a" } });
        let (a, b) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", false)));
        assert_eq!(queue.poll("a", a, &guests, &routes), Poll::Granted("vm-a".into()));
        assert_eq!(queue.poll("b", b, &guests, &routes), Poll::Granted("vm-b".into()));
    }

    #[test]
    fn needs_pick_the_guest_and_versions_match_by_prefix() {
        let caps = guest("x").caps;
        assert!(satisfies(&json!({ "xcode": "26.x", "arch": "aarch64" }), &caps));
        assert!(satisfies(&json!({ "xcode": "26.1" }), &caps));
        assert!(satisfies(&json!({ "runtime": "26" }), &caps));
        assert!(!satisfies(&json!({ "xcode": "2" }), &caps));
        assert!(!satisfies(&json!({ "xcode": "27" }), &caps));
        assert!(!satisfies(&json!({ "arch": "x86_64" }), &caps));
        assert!(!satisfies(&json!({ "runtime": "27" }), &caps));
        let queue = Queue::load(&temp("needs"));
        let mut picky = job("a", "one", false);
        picky.needs = json!({ "xcode": "27" });
        let relay = queue.submit(picky);
        let newer = Guest { caps: json!({ "xcode": "27.0" }), ..guest("vm-new") };
        assert_eq!(queue.poll("a", relay, &[guest("vm-a"), newer], &no_routes()), Poll::Granted("vm-new".into()));
    }

    #[test]
    fn cancelling_a_queued_job_frees_its_place_and_tells_its_waiter() {
        let queue = Queue::load(&temp("cancel"));
        let guests = [guest("vm-a")];
        let (a, b, c) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", false)), queue.submit(job("c", "three", false)));
        assert_eq!(queue.poll("a", a, &guests, &no_routes()), Poll::Granted("vm-a".into()));
        assert_eq!(queue.cancel("b"), Cancelled::Dequeued);
        assert_eq!(queue.poll("c", c, &guests, &no_routes()), Poll::Waiting(0));
        assert_eq!(queue.poll("b", b, &guests, &no_routes()), Poll::Gone(queue.list()["recent"].as_array().unwrap().first().cloned()));
        assert_eq!(queue.cancel("a"), Cancelled::Asked(Some("vm-a".into())));
        assert!(queue.wants_cancel("a"));
        assert_eq!(queue.cancel("b"), Cancelled::Finished);
        assert_eq!(queue.cancel("nope"), Cancelled::Unknown);
    }

    #[test]
    fn a_waiter_that_comes_back_keeps_its_place_and_a_detached_job_is_passed_over() {
        let queue = Queue::load(&temp("detach"));
        let guests = [guest("vm-a")];
        let (a, b) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", false)));
        queue.detach("a", a);
        assert_eq!(queue.poll("b", b, &guests, &no_routes()), Poll::Granted("vm-a".into()), "nobody's there to send a's snapshot");
        queue.finish("b", &Value::Null);
        let c = queue.submit(job("c", "three", false));
        let again = queue.submit(job("a", "one", false));
        assert_eq!(queue.poll("c", c, &guests, &no_routes()), Poll::Waiting(0), "a kept its place in front");
        assert_eq!(queue.poll("a", again, &guests, &no_routes()), Poll::Granted("vm-a".into()));
        assert_eq!(queue.poll("a", a, &guests, &no_routes()), Poll::Gone(None), "the old relay has lost it");
    }

    #[test]
    fn a_client_gone_after_another_waiter_granted_its_job_gives_the_slot_back() {
        let queue = Queue::load(&temp("grant"));
        let guests = [guest("vm-a")];
        let (a, b) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", false)));
        assert_eq!(queue.poll("b", b, &guests, &no_routes()), Poll::Waiting(0), "b's poll granted a");
        assert_eq!(queue.list()["jobs"][0]["stage"], "sending");
        queue.detach("a", a);
        assert_eq!(queue.poll("b", b, &guests, &no_routes()), Poll::Granted("vm-a".into()), "a's client never saw the grant");
        let c = queue.submit(job("a", "one", false));
        assert_eq!(queue.poll("a", c, &guests, &no_routes()), Poll::Waiting(0));
    }

    #[test]
    fn a_job_cancelled_while_its_client_was_away_stays_cancelled() {
        let queue = Queue::load(&temp("away"));
        let relay = queue.submit(job("a", "one", false));
        queue.detach("a", relay);
        assert_eq!(queue.cancel("a"), Cancelled::Dequeued);
        let again = queue.submit(job("a", "one", false));
        match queue.poll("a", again, &[guest("vm-a")], &no_routes()) {
            Poll::Gone(Some(ended)) => assert_eq!(ended["status"]["phase"], "cancelled"),
            other => panic!("{other:?}"),
        }
        assert_eq!(queue.list()["jobs"], json!([]));
    }

    #[test]
    fn a_detached_job_expires_without_a_waiter_and_a_late_return_goes_to_the_back() {
        let queue = Queue::load(&temp("expire"));
        queue.state().detached_for = Duration::ZERO;
        let (a, b) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", false)));
        queue.detach("a", a);
        assert_eq!(queue.list()["jobs"].as_array().unwrap().len(), 1, "gone from the list with nobody polling");
        let late = queue.submit(job("a", "one", false));
        assert_eq!(queue.poll("b", b, &[guest("vm-a")], &no_routes()), Poll::Granted("vm-a".into()));
        assert_eq!(queue.poll("a", late, &[guest("vm-a")], &no_routes()), Poll::Waiting(0));
        assert_eq!(queue.list()["jobs"][1]["job"], "a");
    }

    #[test]
    fn the_queue_survives_a_restart_with_running_jobs_to_watch() {
        let dir = temp("restart");
        let queue = Queue::load(&dir);
        let guests = [guest("vm-a")];
        let (a, _b) = (queue.submit(job("a", "one", false)), queue.submit(job("b", "two", false)));
        assert_eq!(queue.poll("a", a, &guests, &no_routes()), Poll::Granted("vm-a".into()));
        queue.running("a", None);
        let again = Queue::load(&dir);
        assert_eq!(again.resumed(), [("a".to_string(), "vm-a".to_string())]);
        let listed = again.list();
        assert_eq!(listed["jobs"][0]["stage"], "running");
        assert_eq!(listed["jobs"][1]["stage"], "queued");
        assert_eq!(listed["jobs"][1]["ahead"], 0);
    }
}
