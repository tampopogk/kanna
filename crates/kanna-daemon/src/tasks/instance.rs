use super::*;
use crate::instance_move::{self as im, MOVING_DB};

const CHANGES: &[&str] = &["new", "edit", "ready", "name", "done", "start", "stop", "setup", "pick", "shelve", "move", "plan", "let-go", "send", "comment", "subscribe", "unsubscribe"];

const APP_QUIT_WAIT: Duration = Duration::from_secs(10);

type KeeperMove = Box<dyn Fn(&Path) -> Result<(), String>>;

pub struct MoveHooks {
    pub socket: PathBuf,
    pub terminals: Box<dyn Fn() -> usize>,
    pub close_endpoint: Box<dyn Fn()>,
    pub stop_terminals: Box<dyn Fn()>,
    pub move_keeper: KeeperMove,
    pub quit_app: Box<dyn Fn()>,
    pub app_attached: Box<dyn Fn() -> bool>,
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
        self.moving_instance.borrow().is_some() && string(args, "action").is_some_and(|action| CHANGES.contains(&action))
    }

    pub(super) fn holds_for_move(&self, args: &Value) -> bool {
        self.moving_instance.borrow().is_some() && matches!(string(args, "action"), Some("ask" | "grant" | "transfer"))
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
            if im::keeper_running(folder) {
                refusals.push(KmuxError::new("not_empty", format!("{shown}'s keeper still holds terminals: close them in {shown}'s Kanna first; two keepers don't merge")));
            }
        }
        let transfers: Vec<String> = [moving::OUT, "in"].iter().flat_map(|role| crate::transfer::under_way(&self.db, role)).map(|t| t.task).collect();
        if !transfers.is_empty() {
            refusals.push(KmuxError::new("transferring", format!("{} moving between machines: wait until it's over", transfers.join(", "))));
        }
        let entries = self.entries.borrow();
        let (started, waiting) = entries.iter().partition::<Vec<&Entry>, _>(|e| e.folder.is_some());
        let desk = self.running.borrow().len();
        let terminals = self.move_hooks.borrow().as_ref().map_or(0, |hooks| (hooks.terminals)());
        let plan = Map::from_iter([
            ("from".to_string(), json!(im::label(&self.state))),
            ("to".to_string(), json!(shown)),
            ("folder".to_string(), json!(folder)),
            ("tasks".to_string(), json!({ "waiting": waiting.len(), "started": started.len(), "desk": desk })),
            ("terminals".to_string(), json!(terminals)),
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
        let Some(folder) = string(args, "folder").filter(|f| !f.is_empty()).map(PathBuf::from) else { return responder.reply(Err(bad("say which instance to move to: scripts/move-instance.sh kanna-staging"))) };
        let Checked { lock, plan, mut refusals } = self.check_move(&folder);
        if args.get("dry") == Some(&Value::Bool(true)) {
            return responder.reply(Ok(plan));
        }
        if !refusals.is_empty() {
            return responder.reply(Err(refusals.remove(0)));
        }
        let Some(lock) = lock else { return responder.reply(Err(KmuxError::new("running", "the receiving folder isn't locked"))) };
        let Some(hooks) = self.move_hooks.borrow_mut().take() else { return responder.reply(Err(KmuxError::new("failed", "this daemon can't move its terminals; nothing moved"))) };
        *self.moving_instance.borrow_mut() = Some(im::label(&folder));
        log!("kanna-daemon: moving this Kanna to {}, its terminals and agents running on", folder.display());
        self.finish_move(folder, lock, hooks, plan, responder);
    }

    fn finish_move(self: &Rc<Self>, folder: PathBuf, lock: fs::File, hooks: MoveHooks, mut plan: Map, responder: Responder) {
        let id = format!("move-{}", replica::wall());
        if let Err(why) = im::leaving(&self.state, &id, &folder, &hooks.socket) {
            self.moving_instance.borrow_mut().take();
            *self.move_hooks.borrow_mut() = Some(hooks);
            return responder.reply(Err(KmuxError::new("failed", format!("{why}; nothing moved"))));
        }
        (hooks.close_endpoint)();
        let mark = json!({ "id": id, "from": im::label(&self.state), "from_dir": self.state, "to": im::label(&folder), "at": im::now(), "socket": hooks.socket });
        let copy = folder.join(MOVING_DB);
        (hooks.stop_terminals)();
        let done = self
            .db
            .backup_to(&copy)
            .map_err(|e| format!("can't copy the records to {}: {e}", copy.display()))
            .and_then(|_| im::prepare(&copy, &mark).map_err(|e| format!("can't mark the copy: {e}")))
            .and_then(|moved| im::stage_files(&self.state, &folder).map(|_| moved))
            .and_then(|moved| im::commit(&folder).map(|_| moved));
        let moved = match done {
            Ok(moved) => moved,
            Err(why) => {
                im::abandon(&folder);
                im::stay(&self.state);
                drop(lock);
                log!("kanna-daemon: the move to {} failed: {why}; this Kanna restarts as it was", folder.display());
                responder.reply(Err(KmuxError::new("failed", format!("{why}; nothing moved, and this Kanna restarts as it was, its terminals with it"))));
                return stop_soon();
            }
        };
        let mut warnings = vec![];
        if let Err(why) = (hooks.move_keeper)(&folder) {
            warnings.push(format!("{why}: whichever of the two Kannas starts first moves it"));
        }
        if let Err(why) = im::link_socket(&hooks.socket, &folder.join(kanna_build::SOCKET)) {
            warnings.push(format!("{why}: the receiving Kanna makes it when it starts"));
        }
        drop(lock);
        for warning in &warnings {
            log!("kanna-daemon: after the move: {warning}");
        }
        log!("kanna-daemon: moved to {} ({id}): {} waiting, {} started, {} on a Desk; this Kanna stops, and starts again empty", folder.display(), moved.waiting.len(), moved.started.len(), moved.desk.len());
        plan.insert("moved".into(), moved.to_value());
        plan.insert("id".into(), json!(id));
        plan.insert("warnings".into(), json!(warnings));
        responder.reply(Ok(plan));
        (hooks.quit_app)();
        crate::runtime::spawn("a moved Kanna's stop", async move {
            let by = Instant::now() + APP_QUIT_WAIT;
            tokio::time::sleep(Duration::from_millis(500)).await;
            while (hooks.app_attached)() && Instant::now() < by {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
        });
    }
}

fn stop_soon() {
    crate::runtime::spawn("a moved Kanna's stop", async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        unsafe { libc::kill(libc::getpid(), libc::SIGTERM) };
    });
}
