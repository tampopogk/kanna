use super::*;
use crate::instance_move::{self as im, Prepared, MOVED_FROM, MOVING_DB, RESUMING};

const TURN_WAIT: Duration = Duration::from_secs(600);

const STOP_WAIT: Duration = Duration::from_secs(60);

const CHANGES: &[&str] = &["new", "edit", "ready", "name", "done", "start", "stop", "setup", "pick", "shelve", "move", "plan", "let-go", "send", "comment", "subscribe", "unsubscribe"];

fn turn_wait() -> Duration {
    std::env::var("KANNA_MOVE_TURN_WAIT").ok().and_then(|s| s.parse().ok()).map_or(TURN_WAIT, Duration::from_secs)
}

struct Checked {
    lock: Option<fs::File>,
    plan: Map,
    refusals: Vec<KmuxError>,
}

impl Tasks {
    pub(super) fn moving_refusal(&self) -> KmuxError {
        let to = self.moving_instance.borrow().clone().unwrap_or_default();
        KmuxError::new("moving_instance", format!("this Kanna is moving to {to}: its tasks don't change until the move is over"))
    }

    pub(super) fn blocked_by_move(&self, args: &Value) -> bool {
        self.moving_instance.borrow().is_some() && args.get("for_move") != Some(&Value::Bool(true)) && string(args, "action").is_some_and(|action| CHANGES.contains(&action))
    }

    pub(super) fn holds_for_move(&self, args: &Value) -> bool {
        self.moving_instance.borrow().is_some() && matches!(string(args, "action"), Some("ask" | "grant" | "transfer"))
    }

    fn busy_tasks(&self) -> Vec<String> {
        let Some(core) = self.core.upgrade() else { return vec![] };
        let working = |pane: &str| alive(&core, pane) && (self.ready.borrow().get(pane).and_then(|r| r.status()) == Some(Status::Working) || self.quitting.borrow().contains_key(pane));
        let mut busy: Vec<String> = self.entries.borrow().iter().filter(|e| e.held.as_deref().is_some_and(working)).map(|e| e.id.clone()).collect();
        busy.extend(self.running.borrow().iter().filter(|r| working(&r.pane)).map(|r| r.entry.id.clone()));
        busy.extend(self.pending.borrow().values().map(|p| p.entry.id.clone()));
        busy.extend(self.launching.borrow().iter().cloned());
        busy.sort();
        busy.dedup();
        busy
    }

    fn live_agents(&self) -> Vec<String> {
        let Some(core) = self.core.upgrade() else { return vec![] };
        let mut live: Vec<String> = self.entries.borrow().iter().filter(|e| e.held.as_deref().is_some_and(|p| alive(&core, p))).map(|e| e.id.clone()).collect();
        live.extend(self.running.borrow().iter().filter(|r| alive(&core, &r.pane)).map(|r| r.entry.id.clone()));
        live
    }

    fn named(&self, ids: &[String]) -> Vec<Value> {
        ids.iter().map(|id| json!({ "id": id, "name": self.find_entry(id).and_then(|e| e.name) })).collect()
    }

    fn find_entry(&self, id: &str) -> Option<Entry> {
        self.entries.borrow().iter().find(|e| e.id == id).cloned().or_else(|| self.running.borrow().iter().find(|r| r.entry.id == id).map(|r| r.entry.clone()))
    }

    fn check_move(&self, folder: &Path) -> Checked {
        let mut refusals = vec![];
        let ours = self.db.records_version().unwrap_or(0);
        let same = match (fs::canonicalize(folder), fs::canonicalize(&self.state)) {
            (Ok(a), Ok(b)) => a == b,
            _ => folder == self.state,
        };
        let shown = im::label(folder);
        let mut lock = None;
        if same {
            refusals.push(KmuxError::new("same", format!("{} is this Kanna's own folder", folder.display())));
        } else if !folder.is_dir() {
            refusals.push(KmuxError::new("unknown", format!("{} isn't there: open the Kanna that uses it once, then quit it, so it says which build it is", folder.display())));
        } else {
            match im::lock(folder) {
                Ok(held) => lock = Some(held),
                Err(why) => refusals.push(KmuxError::new("running", why)),
            }
            match im::receiving_daemon(folder) {
                None => refusals.push(KmuxError::new("unknown", format!("{shown} doesn't say which build it is: open its Kanna once, then quit it"))),
                Some(daemon) => match im::reads_up_to(&daemon) {
                    Some(theirs) if theirs >= ours => {}
                    Some(theirs) => refusals.push(KmuxError::new("older", format!("{shown}'s Kanna ({}) reads records up to version {theirs}, and these are version {ours}: update it first", daemon.display()))),
                    None => refusals.push(KmuxError::new("older", format!("{shown}'s Kanna ({}) can't take moved records: update it first", daemon.display()))),
                },
            }
            match im::live_tasks(folder) {
                Ok(live) if live.is_empty() => {}
                Ok(live) => refusals.push(KmuxError::new("not_empty", format!("{shown} has {} task(s) on its Shelf or Desk ({}): a move doesn't merge two Shelves", live.len(), live.join(", ")))),
                Err(why) => refusals.push(KmuxError::new("not_empty", why)),
            }
        }
        let transfers: Vec<String> = [moving::OUT, "in"].iter().flat_map(|role| crate::transfer::under_way(&self.db, role)).map(|t| t.task).collect();
        if !transfers.is_empty() {
            refusals.push(KmuxError::new("transferring", format!("{} moving between machines: wait until it's over", transfers.join(", "))));
        }
        let entries = self.entries.borrow();
        let (started, waiting): (Vec<&Entry>, Vec<&Entry>) = entries.iter().partition(|e| e.folder.is_some());
        let desk = self.running.borrow().len();
        let live = self.live_agents();
        let busy = self.busy_tasks();
        let waiting_messages = self.db.waiting_messages().map(|m| m.len()).unwrap_or(0);
        let plan = Map::from_iter([
            ("from".to_string(), json!(im::label(&self.state))),
            ("to".to_string(), json!(shown)),
            ("folder".to_string(), json!(folder)),
            ("tasks".to_string(), json!({ "waiting": waiting.len(), "started": started.len(), "desk": desk })),
            ("quit".to_string(), json!(self.named(&live))),
            ("busy".to_string(), json!(self.named(&busy))),
            ("messages".to_string(), json!(waiting_messages)),
            ("accounts".to_string(), json!(im::own_accounts(&self.state))),
            ("records".to_string(), json!(ours)),
            ("refused".to_string(), Value::Array(refusals.iter().map(|e| json!({ "code": e.code, "message": e.message })).collect())),
        ]);
        Checked { lock, plan, refusals }
    }

    pub fn move_instance(self: &Rc<Self>, args: &Value, responder: Responder) {
        if self.moving_instance.borrow().is_some() {
            return responder.reply(Err(self.moving_refusal()));
        }
        let Some(folder) = string(args, "folder").filter(|f| !f.is_empty()).map(PathBuf::from) else { return responder.reply(Err(bad("say which instance to move to: kanna instances move kanna-staging"))) };
        let Checked { lock, plan, mut refusals } = self.check_move(&folder);
        if args.get("dry") == Some(&Value::Bool(true)) {
            return responder.reply(Ok(plan));
        }
        if !refusals.is_empty() {
            return responder.reply(Err(refusals.remove(0)));
        }
        let Some(lock) = lock else { return responder.reply(Err(KmuxError::new("running", "the receiving folder isn't locked"))) };
        let to = im::label(&folder);
        log!("kanna-daemon: moving this Kanna to {}: no task starts, and each agent quits at its turn's end", folder.display());
        *self.moving_instance.borrow_mut() = Some(to);
        let this = Rc::downgrade(self);
        crate::runtime::spawn("an instance move", async move {
            let deadline = Instant::now() + turn_wait();
            loop {
                let Some(tasks) = this.upgrade() else { return };
                let busy = tasks.busy_tasks();
                if busy.is_empty() {
                    break;
                }
                if Instant::now() >= deadline {
                    tasks.moving_instance.borrow_mut().take();
                    let names: Vec<String> = busy.iter().map(|id| tasks.find_entry(id).and_then(|e| e.name).map_or_else(|| id.clone(), |name| format!("\"{name}\""))).collect();
                    return responder.reply(Err(KmuxError::new("busy", format!("still in the middle of a turn after {}s: {}; nothing moved", turn_wait().as_secs(), names.join(", ")))));
                }
                drop(tasks);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let Some(tasks) = this.upgrade() else { return };
            let resume = tasks.live_agents();
            if let Err(error) = tasks.db.put_meta(RESUMING, &json!(resume).to_string()) {
                tasks.moving_instance.borrow_mut().take();
                return responder.reply(Err(KmuxError::new("failed", format!("can't note which agents to start again: {error}; nothing moved"))));
            }
            let Some(core) = tasks.core.upgrade() else { return responder.reply(Err(gone())) };
            let left = Rc::new(Cell::new(resume.len()));
            let failures: Rc<RefCell<Vec<String>>> = Rc::default();
            for id in &resume {
                let (left, failures, id) = (left.clone(), failures.clone(), id.clone());
                core.handle(json!({ "cmd": "tasks", "args": { "action": "stop", "task": id, "by": "daemon", "for_move": true } }), move |reply| {
                    if let Some(error) = failed(&reply) {
                        failures.borrow_mut().push(format!("{id}: {}", error.message));
                    }
                    left.set(left.get() - 1);
                });
            }
            drop((tasks, core));
            let stopping = Instant::now() + STOP_WAIT;
            while left.get() > 0 && Instant::now() < stopping {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let Some(tasks) = this.upgrade() else { return };
            let failures = failures.borrow().clone();
            if left.get() > 0 || !failures.is_empty() {
                let why = if failures.is_empty() { format!("{} agent(s) didn't quit in {}s", left.get(), STOP_WAIT.as_secs()) } else { failures.join("; ") };
                tasks.call_off_move(&resume);
                return responder.reply(Err(KmuxError::new("failed", format!("couldn't quit every agent ({why}); nothing moved, and the agents that quit start again"))));
            }
            tasks.finish_move(folder, lock, resume, plan, responder);
        });
    }

    fn call_off_move(self: &Rc<Self>, resume: &[String]) {
        self.moving_instance.borrow_mut().take();
        im::stay(&self.state);
        if let Err(error) = self.db.put_meta(RESUMING, "[]") {
            log!("kanna-daemon: can't clear the agents a called-off move would start again: {error}");
        }
        self.start_again(resume.to_vec(), "after the move was called off");
        self.refill();
    }

    fn finish_move(self: &Rc<Self>, folder: PathBuf, lock: fs::File, resume: Vec<String>, mut plan: Map, responder: Responder) {
        let id = format!("move-{}", replica::wall());
        let prepared = Prepared { id: id.clone(), from: im::label(&self.state), to: im::label(&folder), resume: resume.clone() };
        if let Err(why) = im::leaving(&self.state, &id, &folder) {
            self.call_off_move(&resume);
            return responder.reply(Err(KmuxError::new("failed", format!("{why}; nothing moved"))));
        }
        if let Some(close) = self.close_endpoint.borrow().as_ref() {
            close();
        }
        let copy = folder.join(MOVING_DB);
        let done = self.db.backup_to(&copy).map_err(|e| format!("can't copy the records to {}: {e}", copy.display())).and_then(|_| im::prepare(&copy, &folder, &prepared).map_err(|e| format!("can't make the copy Shelf-only: {e}"))).and_then(|moved| im::stage_key(&self.state, &folder).map(|_| moved)).and_then(|moved| im::commit(&folder).map(|_| moved));
        drop(lock);
        let moved = match done {
            Ok(moved) => moved,
            Err(why) => {
                im::abandon(&folder);
                im::stay(&self.state);
                log!("kanna-daemon: the move to {} failed: {why}; this Kanna restarts as it was", folder.display());
                responder.reply(Err(KmuxError::new("failed", format!("{why}; nothing moved, and this Kanna restarts as it was, its agents with it"))));
                return restart_soon();
            }
        };
        log!("kanna-daemon: moved to {} ({id}): {} waiting, {} started, {} from a Desk; this Kanna stops, and starts again empty", folder.display(), moved.waiting.len(), moved.started.len(), moved.desk.len());
        plan.insert("moved".into(), moved.to_value());
        plan.insert("resume".into(), json!(self.named(&resume)));
        plan.insert("id".into(), json!(id));
        responder.reply(Ok(plan));
        restart_soon();
    }

    pub fn resume_moved(self: &Rc<Self>) {
        let unfinished: Vec<String> = self.db.meta(RESUMING).ok().flatten().and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
        if !unfinished.is_empty() {
            match self.db.put_meta(RESUMING, "[]") {
                Ok(()) => {
                    log!("kanna-daemon: a move didn't finish: {} task(s) whose agents quit for it start again", unfinished.len());
                    self.start_again(unfinished, "after a move that didn't finish");
                }
                Err(error) => log!("kanna-daemon: can't clear the agents a move that didn't finish quit, so they wait on the Shelf: {error}"),
            }
        }
        let Some(mut mark) = self.db.meta(MOVED_FROM).ok().flatten().and_then(|v| serde_json::from_str::<Value>(&v).ok()) else { return };
        let resume = strings(&mark, "resume");
        if resume.is_empty() {
            return;
        }
        mark["resume"] = json!([]);
        mark["resumed"] = json!(resume);
        if let Err(error) = self.db.put_meta(MOVED_FROM, &mark.to_string()) {
            return log!("kanna-daemon: can't note that the moved tasks start again, so they wait on the Shelf: {error}");
        }
        log!("kanna-daemon: moved here from {}: {} task(s) whose agents ran there start again", string(&mark, "from").unwrap_or("another Kanna"), resume.len());
        self.start_again(resume, "after the move");
    }

    fn start_again(&self, tasks: Vec<String>, when: &'static str) {
        let Some(core) = self.core.upgrade() else { return };
        for id in tasks {
            core.handle(json!({ "cmd": "tasks", "args": { "action": "start", "task": id, "by": "daemon" } }), move |reply| {
                if let Some(error) = failed(&reply) {
                    log!("kanna-daemon: {id} didn't start again {when}: {}", error.message);
                }
            });
        }
    }
}

fn restart_soon() {
    crate::runtime::spawn("a moved Kanna's stop", async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
    });
}
