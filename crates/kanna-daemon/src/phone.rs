use crate::app_link::AppLink;
use crate::db::{self, Db, PhoneItem};
use crate::machines::Caller;
use crate::messages::{KANNA, PHONE, PLANNER};
use crate::runtime::Events;
use crate::tasks::{failed as refusal, ProjectRecord, Tasks};
use crate::Event;
use kanna_mux::json::{json, num, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Responder};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::rc::{Rc, Weak};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const COMMAND: &str = "phone";

const TICK: Duration = Duration::from_secs(1);

const START_LIMIT: f64 = 60.0;

const IDLE_LIMIT: f64 = 3600.0;

const SETTLE: f64 = 3.0;

const CODE_LINES: u64 = 200;

const PEER_WAIT: Duration = Duration::from_secs(3);

const SCAN_EVERY: u64 = 60;

const STATS_WAIT: Duration = Duration::from_secs(120);

const ACT_WAIT: Duration = Duration::from_secs(20);

const NUDGE: &str = "The phone is waiting: answer with kanna phone say";

static STREAMS: Mutex<Vec<UnixStream>> = Mutex::new(Vec::new());

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn bad(message: impl Into<String>) -> KmuxError {
    KmuxError::new("bad_request", message)
}

fn failed(what: &str, error: rusqlite::Error) -> KmuxError {
    log!("kanna-daemon: couldn't {what} in {}: {error}", db::FILE);
    KmuxError::new("failed", format!("couldn't {what}: {error}"))
}

fn fields(value: Value) -> Map {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn notify(notice: &Value) {
    let line = format!("{notice}\n");
    let mut streams = STREAMS.lock().unwrap_or_else(|p| p.into_inner());
    streams.retain_mut(|stream| stream.write_all(line.as_bytes()).is_ok());
}

pub fn serve(events: Events, stream: UnixStream, from: String) {
    let Ok(reader) = stream.try_clone() else { return };
    let Ok(writer) = stream.try_clone() else { return };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    STREAMS.lock().unwrap_or_else(|p| p.into_inner()).push(writer);
    let replies = Mutex::new(stream);
    let replies = std::sync::Arc::new(replies);
    for line in BufReader::new(reader).lines().map_while(Result::ok) {
        let Ok(request) = serde_json::from_str::<Value>(&line) else { continue };
        let mut args = request.get("args").cloned().filter(Value::is_object).unwrap_or_else(|| json!({}));
        args["request"] = request.get("cmd").cloned().unwrap_or(Value::Null);
        args["phone"] = json!(from);
        let wrapped = json!({ "id": request.get("id").cloned().unwrap_or(Value::Null), "cmd": COMMAND, "args": args });
        let replies = replies.clone();
        let reply = Box::new(move |reply: Value| {
            let _ = writeln!(replies.lock().unwrap_or_else(|p| p.into_inner()), "{reply}");
        });
        if events.send(Event::Request(wrapped, reply)).is_err() {
            break;
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Seen {
    since: f64,
    last: f64,
    up: bool,
}

#[derive(Default)]
struct Watch {
    told: Option<(String, f64)>,
    starting: Option<f64>,
    answering: Option<f64>,
    nudged: bool,
    worked: bool,
    since: f64,
    idle: Option<f64>,
}

pub struct Phone {
    db: Rc<Db>,
    tasks: Rc<Tasks>,
    core: CoreHandle,
    caller: Option<Caller>,
    app: Rc<AppLink>,
    told: std::cell::Cell<u64>,
    repos: RefCell<BTreeMap<String, String>>,
    watches: RefCell<BTreeMap<String, Watch>>,
    records: RefCell<Vec<ProjectRecord>>,
    seen: RefCell<BTreeMap<String, Seen>>,
    ticks: std::cell::Cell<u64>,
    known: std::cell::Cell<usize>,
}

fn item_value(item: &PhoneItem, read: Option<f64>) -> Value {
    json!({ "seq": item.seq, "at": num(item.at), "kind": item.kind, "client": item.client, "read": read.map(num), "doc": item.doc })
}

impl Phone {
    pub fn install(core: &CoreHandle, db: Rc<Db>, tasks: Rc<Tasks>, caller: Option<Caller>, app: Rc<AppLink>) -> Rc<Phone> {
        let phone = Rc::new(Phone { db, tasks, core: core.clone(), caller, app, told: std::cell::Cell::new(0), repos: RefCell::default(), watches: RefCell::default(), records: RefCell::default(), seen: RefCell::default(), ticks: std::cell::Cell::new(0), known: std::cell::Cell::new(0) });
        let this = Rc::downgrade(&phone);
        core.add_command(
            COMMAND,
            Rc::new(move |args, responder| match this.upgrade() {
                Some(phone) => phone.handle(args, responder),
                None => responder.reply(Err(KmuxError::new("stopping", "the daemon is stopping"))),
            }),
        );
        Phone::ticking(Rc::downgrade(&phone));
        phone
    }

    fn ticking(this: Weak<Phone>) {
        crate::runtime::spawn("the phone's watch", async move {
            loop {
                tokio::time::sleep(TICK).await;
                let Some(phone) = this.upgrade() else { return };
                phone.tick();
            }
        });
    }

    fn handle(self: &Rc<Self>, args: Value, responder: Responder) {
        let request = string(&args, "request").or_else(|| string(&args, "action")).unwrap_or_default().to_string();
        match request.as_str() {
            "hello" => self.hello(responder),
            "offer" => responder.reply(self.offer()),
            "wait" => self.wait(responder),
            "chat.read" => responder.reply(self.read(&args)),
            "chat.send" => responder.reply(self.send(&args)),
            "chat.choose" => responder.reply(self.choose(&args)),
            "task.choose" => self.task_choose(&args, responder),
            "here.choose" => responder.reply(self.choose_here(&args)),
            "say" | "ask" | "show" => responder.reply(self.answer(&request, &args)),
            "history" => responder.reply(self.history(&args)),
            "machines" => self.machines(&args, responder),
            "planner.move" => self.planner_move(&args, responder),
            "stats" => self.stats(&args, responder),
            "here.machine" => responder.reply(Ok(self.here_machine())),
            "here.plan" => responder.reply(self.plan_here(&args)),
            "tasks.list" => self.tasks_list(&args, responder),
            "task.read" => self.task_read(&args, responder),
            "task.send" => self.task_send(&args, responder),
            "task.pick" => self.task_pick(&args, responder),
            "here.shelf" => responder.reply(self.here_shelf(&args)),
            "here.read" => responder.reply(string(&args, "task").ok_or_else(|| bad("say which task")).and_then(|task| self.read_here(task))),
            other => responder.reply(Err(bad(format!("no phone request \"{other}\"")))),
        }
    }

    fn offer(&self) -> Result<Map, KmuxError> {
        let caller = self.caller.as_ref().ok_or_else(|| KmuxError::new("unavailable", "this daemon has no endpoint"))?;
        caller.phone_offer().map(fields)
    }

    fn wait(&self, responder: Responder) {
        match self.caller.clone() {
            Some(caller) => caller.phone_wait(responder),
            None => responder.reply(Err(KmuxError::new("unavailable", "this daemon has no endpoint"))),
        }
    }

    fn scan(self: &Rc<Self>, then: impl FnOnce(&Rc<Phone>) + 'static) {
        let this = Rc::downgrade(self);
        self.core.handle(json!({ "cmd": "projects", "args": { "action": "list" } }), move |listed| {
            let paths: Vec<String> = listed["projects"].as_array().into_iter().flatten().filter_map(|p| p["path"].as_str().map(String::from)).collect();
            crate::runtime::spawn("the phone's look at the projects", async move {
                let found = crate::runtime::off_loop(move || paths.into_iter().filter_map(|path| Some((crate::projects::identity_off_loop(Path::new(&path))?, path))).collect::<Vec<_>>()).await.unwrap_or_default();
                let Some(this) = this.upgrade() else { return };
                this.found(found);
                then(&this);
            });
        });
    }

    fn found(&self, found: Vec<(String, String)>) {
        let mine: Vec<(String, String, bool)> = found.iter().map(|(repo, path)| (repo.clone(), Phone::label(path), self.tasks.planner_pane(path).is_some() || self.db.planner(path).ok().flatten().is_some())).collect();
        *self.repos.borrow_mut() = found.into_iter().collect();
        self.tasks.publish_projects(&mine);
    }

    fn hello(self: &Rc<Self>, responder: Responder) {
        self.scan(move |this| this.hello_with(responder));
    }

    fn roster(&self) -> (String, BTreeMap<String, String>, Vec<Value>) {
        let status = self.caller.as_ref().map(Caller::status).unwrap_or_default();
        let me = status["key"].as_str().unwrap_or_default().to_string();
        let machines: Vec<Value> = status["peers"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p["trusted"] == true && p["revoked"] != true).collect();
        let names = machines.iter().filter_map(|p| Some((p["key"].as_str()?.to_string(), p["name"].as_str()?.to_string()))).collect();
        (me, names, machines)
    }

    fn hello_with(&self, responder: Responder) {
        let machine = self.caller.as_ref().map(Caller::status).unwrap_or_default();
        let (me, names, machines) = self.roster();
        let mut projects = vec![];
        for record in self.tasks.project_records() {
            let Some(planner) = record.planner_machine(&me, &|key| names.contains_key(key)) else { continue };
            let mut project = json!({ "repo": record.repo, "name": record.name, "planner": names.get(&planner), "plannerKey": planner });
            if planner == me {
                if let Some(path) = self.repos.borrow().get(&record.repo).cloned() {
                    let (status, since) = self.status(&path);
                    project["status"] = json!(status);
                    project["since"] = num(since);
                }
            }
            projects.push(project);
        }
        let reachable: Vec<Value> = machines.iter().map(|m| json!({ "name": m["name"], "key": m["key"], "addrs": if m["key"] == me.as_str() { machine["addrs"].clone() } else { m["addrs"].clone() } })).collect();
        let log = self.caller.as_ref().and_then(Caller::log);
        responder.reply(Ok(Map::from_iter([("name".to_string(), machine["name"].clone()), ("key".to_string(), machine["key"].clone()), ("addrs".to_string(), machine["addrs"].clone()), ("projects".to_string(), Value::Array(projects)), ("machines".to_string(), Value::Array(reachable)), ("log".to_string(), log.unwrap_or(Value::Null))])));
    }

    fn here_machine(&self) -> Map {
        let release = crate::machines::release();
        Map::from_iter([("version".to_string(), release["version"].clone())])
    }

    fn machines(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(repo) = string(args, "repo").map(String::from) else { return responder.reply(Err(bad("say which repo"))) };
        let now_at = Instant::now();
        let peers = self.peers();
        let left = Rc::new(std::cell::Cell::new(peers.len()));
        let answer = Rc::new(RefCell::new(Some(responder)));
        let versions: Rc<RefCell<BTreeMap<String, Value>>> = Rc::default();
        let this = Rc::downgrade(self);
        let finish = {
            let versions = versions.clone();
            Rc::new(move || {
                let (Some(this), Some(responder)) = (this.upgrade(), answer.borrow_mut().take()) else { return };
                responder.reply(Ok(this.machines_with(&repo, &versions.borrow())));
            })
        };
        if peers.is_empty() {
            return finish();
        }
        for peer in peers {
            let key = peer["key"].as_str().unwrap_or_default().to_string();
            let (left, finish, versions, this) = (left.clone(), finish.clone(), versions.clone(), Rc::downgrade(self));
            let done = move |reply: Option<Value>| {
                if let Some(this) = this.upgrade() {
                    this.saw(&key, reply.is_some());
                }
                if let Some(reply) = reply {
                    versions.borrow_mut().insert(key, reply["version"].clone());
                }
                left.set(left.get() - 1);
                if left.get() == 0 {
                    finish();
                }
            };
            if self.tasks.is_away(peer["key"].as_str().unwrap_or_default(), now_at) {
                done(None);
                continue;
            }
            self.ask(peer["name"].as_str(), COMMAND, json!({ "request": "here.machine" }), PEER_WAIT, move |reply| done(reply.ok()));
        }
    }

    fn saw(&self, key: &str, up: bool) {
        let time = now();
        let mut seen = self.seen.borrow_mut();
        let entry = seen.entry(key.to_string()).or_default();
        if up {
            if !entry.up {
                entry.since = time;
            }
            entry.last = time;
        }
        entry.up = up;
    }

    fn machines_with(&self, repo: &str, versions: &BTreeMap<String, Value>) -> Map {
        let (me, names, machines) = self.roster();
        let record = self.tasks.project_record(repo).unwrap_or_default();
        let planner = record.planner_machine(&me, &|key| names.contains_key(key));
        let seen = self.seen.borrow();
        let version = crate::machines::release()["version"].clone();
        let list: Vec<Value> = machines
            .iter()
            .map(|m| {
                let key = m["key"].as_str().unwrap_or_default();
                let mine = key == me;
                let at = seen.get(key).copied().unwrap_or_default();
                let up = mine || at.up;
                let some = |value: f64| (value > 0.0).then(|| num(value));
                json!({
                    "name": m["name"],
                    "key": key,
                    "me": mine,
                    "os": m["capabilities"]["os"],
                    "arch": m["capabilities"]["arch"],
                    "cores": m["capabilities"]["cores"],
                    "version": if mine { version.clone() } else { versions.get(key).cloned().unwrap_or(Value::Null) },
                    "has": record.clones.iter().any(|c| c == key),
                    "planner": planner.as_deref() == Some(key),
                    "up": up,
                    "path": if up && !mine { m["path"].clone() } else { Value::Null },
                    "lan": up && !mine && m["lan"] == true,
                    "ms": if up && !mine { m["rtt_ms"].clone() } else { Value::Null },
                    "since": if up && !mine { some(at.since) } else { None },
                    "seen": if mine { None } else { some(at.last) },
                })
            })
            .collect();
        let from = names.get(&me).cloned().unwrap_or_else(|| self.machine());
        Map::from_iter([("from".to_string(), json!(from)), ("machines".to_string(), Value::Array(list))])
    }

    fn planner_move(self: &Rc<Self>, args: &Value, responder: Responder) {
        let (Some(repo), Some(machine)) = (string(args, "repo").map(String::from), string(args, "machine").map(String::from)) else { return responder.reply(Err(bad("say which repo and which machine"))) };
        let (me, names, _) = self.roster();
        let Some(key) = names.iter().find(|(key, name)| **name == machine || **key == machine).map(|(key, _)| key.clone()) else { return responder.reply(Err(KmuxError::new("not_found", format!("no machine {machine}")))) };
        if key == me {
            return responder.reply(self.plan_here(args));
        }
        let this = Rc::downgrade(self);
        self.ask(Some(&machine), COMMAND, json!({ "request": "here.plan", "repo": repo }), ACT_WAIT, move |reply| {
            if let (Ok(_), Some(this)) = (&reply, this.upgrade()) {
                this.tasks.set_planner(&repo, &key);
            }
            responder.reply(reply.map(fields));
        });
    }

    fn plan_here(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let repo = string(args, "repo").ok_or_else(|| bad("say which repo"))?;
        let Some(path) = self.repos.borrow().get(repo).cloned() else { return Err(KmuxError::new("no_clone", format!("{} has no clone of that repo", self.machine()))) };
        let (me, names, _) = self.roster();
        self.tasks.set_planner(repo, &me);
        self.watches.borrow_mut().entry(path.clone()).or_default();
        if self.tasks.planner_pane(&path).is_none() {
            self.start(&path);
        } else {
            self.tick_repo(&path);
        }
        Ok(Map::from_iter([("planner".to_string(), json!(names.get(&me))), ("plannerKey".to_string(), json!(me))]))
    }

    fn stats(self: &Rc<Self>, args: &Value, responder: Responder) {
        let path = match self.path_of(args) {
            Ok(path) => path,
            Err(error) => return responder.reply(Err(error)),
        };
        self.ask(None, "stats", json!({ "repo": path }), STATS_WAIT, move |reply| responder.reply(reply.map(fields)));
    }

    fn claim(&self, path: &str) -> Result<(), KmuxError> {
        let Some(repo) = self.identity_of(path) else { return Ok(()) };
        let (me, names, _) = self.roster();
        match self.tasks.project_record(&repo).and_then(|r| r.planner).filter(|key| names.contains_key(key)) {
            None => {
                self.tasks.set_planner(&repo, &me);
                Ok(())
            }
            Some(key) if key == me || me.is_empty() => Ok(()),
            Some(key) => Err(KmuxError::new("moved", format!("the planner for this repo is on {} now", names.get(&key).cloned().unwrap_or(key)))),
        }
    }

    fn path_of(&self, args: &Value) -> Result<String, KmuxError> {
        let repo = string(args, "repo").ok_or_else(|| bad("say which repo"))?;
        self.repos.borrow().get(repo).cloned().ok_or_else(|| KmuxError::new("unknown_repo", format!("{repo} isn't a project on this machine")))
    }

    fn identity_of(&self, path: &str) -> Option<String> {
        self.repos.borrow().iter().find(|(_, p)| p.as_str() == path).map(|(repo, _)| repo.clone())
    }

    fn value(&self, item: &PhoneItem) -> Value {
        let read = item.message.and_then(|id| self.db.message(id).ok().flatten()).and_then(|m| m.delivered);
        item_value(item, read)
    }

    fn tell(&self, path: &str, item: &PhoneItem) {
        if let Some(repo) = self.identity_of(path) {
            notify(&json!({ "notice": "chat", "repo": repo, "item": self.value(item) }));
        }
    }

    fn put(&self, path: &str, kind: &str, client: Option<&str>, message: Option<i64>, doc: Value) -> Result<PhoneItem, KmuxError> {
        let item = self.db.put_phone_item(path, kind, client, message, &doc).map_err(|e| failed("keep the phone's message", e))?;
        self.tell(path, &item);
        Ok(item)
    }

    fn read(&self, args: &Value) -> Result<Map, KmuxError> {
        let path = self.path_of(args)?;
        let after = args.get("after").and_then(Value::as_i64).unwrap_or(0);
        let items = self.db.phone_items(&path, after).map_err(|e| failed("read the phone's chat", e))?;
        let (status, since) = self.status(&path);
        Ok(Map::from_iter([("items".to_string(), Value::Array(items.iter().map(|i| self.value(i)).collect())), ("status".to_string(), json!(status)), ("since".to_string(), num(since))]))
    }

    fn send(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let path = self.path_of(args)?;
        let client = string(args, "id").ok_or_else(|| bad("a message from the phone carries its id"))?;
        if let Some(item) = self.db.phone_item_of(&path, client).map_err(|e| failed("look up the phone's message", e))? {
            return Ok(Map::from_iter([("item".to_string(), self.value(&item))]));
        }
        let text = string(args, "text").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("say what"))?;
        self.claim(&path)?;
        self.message(&path, client, text, json!({ "text": text }))
    }

    fn choose(self: &Rc<Self>, args: &Value) -> Result<Map, KmuxError> {
        let path = self.path_of(args)?;
        let client = string(args, "id").ok_or_else(|| bad("a choice from the phone carries its id"))?;
        if let Some(item) = self.db.phone_item_of(&path, client).map_err(|e| failed("look up the phone's choice", e))? {
            return Ok(Map::from_iter([("item".to_string(), self.value(&item))]));
        }
        self.claim(&path)?;
        let seq = args.get("seq").and_then(Value::as_i64).ok_or_else(|| bad("say which question"))?;
        let mut asked = self.db.phone_item(&path, seq).map_err(|e| failed("read the question", e))?.filter(|i| i.kind == "ask").ok_or_else(|| bad("no such question"))?;
        let choice = string(args, "choice").ok_or_else(|| bad("say which choice"))?.to_string();
        asked.doc["chosen"] = json!(choice);
        self.db.set_phone_doc(&path, seq, &asked.doc).map_err(|e| failed("keep the choice", e))?;
        self.tell(&path, &asked);
        self.message(&path, client, &format!("I chose: {choice}"), json!({ "text": choice, "choice": seq }))
    }

    fn message(self: &Rc<Self>, path: &str, client: &str, text: &str, doc: Value) -> Result<Map, KmuxError> {
        let message = self.tasks.queue(PLANNER, path, PHONE, text, None, None)?;
        let item = self.put(path, "me", Some(client), Some(message.id), doc)?;
        self.watches.borrow_mut().entry(path.to_string()).or_default();
        if self.tasks.planner_pane(path).is_none() {
            self.start(path);
        } else {
            self.tick_repo(path);
        }
        Ok(Map::from_iter([("item".to_string(), self.value(&item))]))
    }

    fn machine(&self) -> String {
        self.caller.as_ref().map(Caller::status).and_then(|s| s["name"].as_str().map(String::from)).unwrap_or_else(|| "this machine".into())
    }

    fn label(path: &str) -> String {
        Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
    }

    fn start(self: &Rc<Self>, path: &str) {
        if self.watches.borrow().get(path).is_some_and(|w| w.starting.is_some()) {
            return;
        }
        let (label, machine) = (Phone::label(path), self.machine());
        match self.tasks.start_windowless(path) {
            Ok(()) => {
                let mut watches = self.watches.borrow_mut();
                let watch = watches.entry(path.to_string()).or_default();
                watch.starting = Some(now());
                drop(watches);
                let _ = self.put(path, "sys", None, None, json!({ "text": format!("No planner was running for {label}, so one is starting on {machine}") }));
            }
            Err(error) => {
                let _ = self.put(path, "sys", None, None, json!({ "text": format!("The planner couldn't start on {machine}: {}", error.message) }));
            }
        }
        self.tick_repo(path);
    }

    fn answer(&self, kind: &str, args: &Value) -> Result<Map, KmuxError> {
        let task = string(args, "pane").and_then(|pane| self.tasks.task_of(pane));
        let thread = match &task {
            Some(task) => task.clone(),
            None => {
                let path = string(args, "repo").ok_or_else(|| bad("run it in the repo"))?;
                if !string(args, "pane").is_some_and(|pane| self.tasks.is_planner(path, pane)) {
                    return Err(KmuxError::new("not_an_agent", "only this repo's planner and its tasks' agents talk to the phone"));
                }
                path.to_string()
            }
        };
        let doc = match kind {
            "say" => json!({ "text": string(args, "text").filter(|t| !t.trim().is_empty()).ok_or_else(|| bad("say what"))? }),
            "ask" => {
                let choices = args.get("choices").and_then(Value::as_array).filter(|c| !c.is_empty()).ok_or_else(|| bad("a question has choices: --choice 'LABEL|WHY'"))?;
                json!({ "text": string(args, "text").filter(|t| !t.trim().is_empty()).ok_or_else(|| bad("ask what"))?, "choices": choices })
            }
            _ => {
                let lines = args.get("lines").and_then(Value::as_array).filter(|l| !l.is_empty()).ok_or_else(|| bad("a code card has lines"))?;
                if lines.len() as u64 > CODE_LINES {
                    return Err(bad(format!("a code card holds at most {CODE_LINES} lines")));
                }
                json!({ "file": args.get("file"), "line": args.get("line"), "commit": args.get("commit"), "hot": args.get("hot"), "lines": lines })
            }
        };
        let item = self.put(&thread, kind, None, None, doc)?;
        if let Some(task) = &task {
            self.tasks.spoke(task);
        } else if let Some(watch) = self.watches.borrow_mut().get_mut(&thread) {
            watch.answering = None;
            watch.nudged = false;
        }
        Ok(Map::from_iter([("seq".to_string(), json!(item.seq))]))
    }

    fn history(&self, args: &Value) -> Result<Map, KmuxError> {
        let task = string(args, "pane").and_then(|pane| self.tasks.task_of(pane));
        let path = match &task {
            Some(task) => task.as_str(),
            None => string(args, "repo").ok_or_else(|| bad("run it in the repo"))?,
        };
        let items = self.db.phone_items(path, 0).map_err(|e| failed("read the phone's chat", e))?;
        let last = args.get("last").and_then(Value::as_u64).unwrap_or(items.len() as u64) as usize;
        let from = items.len().saturating_sub(last);
        Ok(Map::from_iter([("items".to_string(), Value::Array(items[from..].iter().map(|i| self.value(i)).collect()))]))
    }

    fn peers(&self) -> Vec<Value> {
        let status = self.caller.as_ref().map(Caller::status).unwrap_or_default();
        status["peers"].as_array().cloned().unwrap_or_default().into_iter().filter(|p| p["me"] != true && p["trusted"] == true && p["revoked"] != true).collect()
    }

    fn here_shelf(&self, args: &Value) -> Result<Map, KmuxError> {
        let project = string(args, "project").ok_or_else(|| bad("say which project"))?;
        Ok(Map::from_iter([("tasks".to_string(), Value::Array(self.tasks.shelf_here(project))), ("open".to_string(), json!(self.app.attached_here()))]))
    }

    fn ask(&self, host: Option<&str>, cmd: &str, args: Value, wait: Duration, then: impl FnOnce(Result<Value, KmuxError>) + 'static) {
        let request = match host {
            Some(host) => json!({ "cmd": "machine", "args": { "action": "call", "host": host, "cmd": cmd, "args": args } }),
            None => json!({ "cmd": cmd, "args": args }),
        };
        let (sender, answer) = tokio::sync::oneshot::channel();
        self.core.handle(request, move |reply| {
            let _ = sender.send(reply);
        });
        let name = host.unwrap_or("this machine").to_string();
        crate::runtime::spawn("a phone request to a machine", async move {
            then(match tokio::time::timeout(wait, answer).await {
                Ok(Ok(reply)) => match refusal(&reply) {
                    Some(error) => Err(error),
                    None => Ok(reply),
                },
                _ => Err(KmuxError::new("unreachable", format!("{name} didn't answer"))),
            });
        });
    }

    fn tasks_list(self: &Rc<Self>, args: &Value, responder: Responder) {
        let project = match self.path_of(args) {
            Ok(_) => string(args, "repo").unwrap_or_default().to_string(),
            Err(error) => return responder.reply(Err(error)),
        };
        let mine = self.machine();
        let tasks = Rc::new(RefCell::new(self.tasks.shelf_here(&project)));
        let desks = Rc::new(RefCell::new(cfg!(target_os = "macos").then(|| json!({ "machine": mine, "open": self.app.attached_here() })).into_iter().collect::<Vec<_>>()));
        let now = Instant::now();
        let peers = self.peers();
        let left = Rc::new(std::cell::Cell::new(peers.len()));
        let answer = Rc::new(RefCell::new(Some(responder)));
        let this = Rc::downgrade(self);
        let finish = {
            let (tasks, desks, project) = (tasks.clone(), desks.clone(), project.clone());
            Rc::new(move || {
                let (Some(this), Some(responder)) = (this.upgrade(), answer.borrow_mut().take()) else { return };
                let mut tasks = tasks.take();
                let shown: HashSet<String> = tasks.iter().filter_map(|t| t["id"].as_str().map(String::from)).collect();
                tasks.extend(this.tasks.shelf_records(&project, &shown));
                responder.reply(Ok(Map::from_iter([("tasks".to_string(), Value::Array(tasks)), ("desks".to_string(), Value::Array(desks.take())), ("done".to_string(), Value::Array(this.tasks.done_lately(&project)))])));
            })
        };
        if peers.is_empty() {
            return finish();
        }
        for peer in peers {
            let name = peer["name"].as_str().unwrap_or_default().to_string();
            let mac = peer["capabilities"]["os"] == "macos";
            let (tasks, desks, left, finish) = (tasks.clone(), desks.clone(), left.clone(), finish.clone());
            let done = move |reply: Option<Value>| {
                if let Some(reply) = &reply {
                    tasks.borrow_mut().extend(reply["tasks"].as_array().cloned().unwrap_or_default());
                }
                if mac {
                    desks.borrow_mut().push(json!({ "machine": name, "open": reply.is_some_and(|r| r["open"] == true) }));
                }
                left.set(left.get() - 1);
                if left.get() == 0 {
                    finish();
                }
            };
            if self.tasks.is_away(peer["key"].as_str().unwrap_or_default(), now) {
                done(None);
                continue;
            }
            self.ask(peer["name"].as_str(), COMMAND, json!({ "request": "here.shelf", "project": project }), PEER_WAIT, move |reply| done(reply.ok()));
        }
    }

    fn read_here(self: &Rc<Self>, task: &str) -> Result<Map, KmuxError> {
        let read = self.tasks.task_here(task)?;
        if let Some(id) = read["task"]["id"].as_str() {
            self.tasks.fill_last_words(id);
        }
        Ok(read)
    }

    fn task_read(self: &Rc<Self>, args: &Value, responder: Responder) {
        let Some(task) = string(args, "task").map(String::from) else { return responder.reply(Err(bad("say which task"))) };
        match self.tasks.owner_of(&task) {
            None => responder.reply(Err(KmuxError::new("not_found", format!("no task {task}")))),
            Some(None) => responder.reply(self.read_here(&task)),
            Some(Some(owner)) => {
                let tasks = self.tasks.clone();
                self.ask(Some(&owner), COMMAND, json!({ "request": "here.read", "task": task }), PEER_WAIT, move |reply| match reply {
                    Ok(reply) => responder.reply(Ok(fields(reply))),
                    Err(error) => responder.reply(tasks.task_record(&task).ok_or(error)),
                });
            }
        }
    }

    fn task_send(self: &Rc<Self>, args: &Value, responder: Responder) {
        let (Some(task), Some(id)) = (string(args, "task"), string(args, "id")) else { return responder.reply(Err(bad("a message from the phone carries its task and id"))) };
        let Some(text) = string(args, "text").map(str::trim).filter(|t| !t.is_empty()) else { return responder.reply(Err(bad("say what"))) };
        let Some(owner) = self.tasks.owner_of(task) else { return responder.reply(Err(KmuxError::new("not_found", format!("no task {task}")))) };
        let request = json!({ "action": "send", "to": task, "text": text, "id": id, "from_phone": true });
        self.ask(owner.as_deref(), "tasks", request, ACT_WAIT, move |reply| responder.reply(reply.map(fields)));
    }

    fn choose_here(&self, args: &Value) -> Result<Map, KmuxError> {
        let task = string(args, "task").ok_or_else(|| bad("say which task"))?;
        let task = self.tasks.resolve(task)?.ok_or_else(|| KmuxError::new("not_found", format!("no task {task}")))?;
        let task = task.as_str();
        let seq = args.get("seq").and_then(Value::as_i64).ok_or_else(|| bad("say which question"))?;
        let choice = string(args, "choice").ok_or_else(|| bad("say which choice"))?;
        let mut asked = self.db.phone_item(task, seq).map_err(|e| failed("read the question", e))?.filter(|i| i.kind == "ask").ok_or_else(|| bad("no such question"))?;
        asked.doc["chosen"] = json!(choice);
        self.db.set_phone_doc(task, seq, &asked.doc).map_err(|e| failed("keep the choice", e))?;
        Ok(Map::new())
    }

    fn task_choose(self: &Rc<Self>, args: &Value, responder: Responder) {
        let (Some(task), Some(id), Some(seq), Some(choice)) = (string(args, "task").map(String::from), string(args, "id").map(String::from), args.get("seq").and_then(Value::as_i64), string(args, "choice").map(String::from)) else {
            return responder.reply(Err(bad("a choice from the phone carries its task, id, question and choice")));
        };
        let Some(owner) = self.tasks.owner_of(&task) else { return responder.reply(Err(KmuxError::new("not_found", format!("no task {task}")))) };
        let this = Rc::downgrade(self);
        let host = owner.clone();
        self.ask(owner.as_deref(), COMMAND, json!({ "request": "here.choose", "task": task, "seq": seq, "choice": choice }), ACT_WAIT, move |reply| {
            let (Ok(_), Some(this)) = (&reply, this.upgrade()) else { return responder.reply(reply.map(fields)) };
            let request = json!({ "action": "send", "to": task, "text": format!("I chose: {choice}"), "id": id, "from_phone": true });
            this.ask(host.as_deref(), "tasks", request, ACT_WAIT, move |reply| responder.reply(reply.map(fields)));
        });
    }

    fn task_pick(self: &Rc<Self>, args: &Value, responder: Responder) {
        let (Some(task), Some(desk)) = (string(args, "task").map(String::from), string(args, "desk").map(String::from)) else { return responder.reply(Err(bad("say which task and which Desk"))) };
        let Some(owner) = self.tasks.owner_of(&task) else { return responder.reply(Err(KmuxError::new("not_found", format!("no task {task}")))) };
        let this = Rc::downgrade(self);
        let room = string(args, "room").map(String::from);
        let freed = room.clone();
        let pick = move |responder: Responder| {
            let Some(this) = this.upgrade() else { return };
            let request = json!({ "action": "pick", "task": task, "desk": desk, "room": freed });
            this.ask(owner.as_deref(), "tasks", request, ACT_WAIT, move |reply| responder.reply(reply.map(fields)));
        };
        let Some(room) = room.as_deref() else { return pick(responder) };
        let Some(holder) = self.tasks.owner_of(room) else { return responder.reply(Err(KmuxError::new("not_found", format!("no task {room}")))) };
        self.ask(holder.as_deref(), "tasks", json!({ "action": "shelve", "task": room }), ACT_WAIT, move |reply| match reply {
            Ok(_) => pick(responder),
            Err(error) => responder.reply(Err(error)),
        });
    }

    fn status(&self, path: &str) -> (String, f64) {
        let watches = self.watches.borrow();
        let watch = watches.get(path);
        let waiting = self.db.phone_waiting(path).unwrap_or(false);
        let told = watch.and_then(|w| w.told.as_ref()).map(|(_, since)| *since);
        let last = self.db.phone_items(path, 0).ok().and_then(|items| items.last().cloned());
        let Some(pane) = self.tasks.planner_pane(path) else {
            let starting = watch.and_then(|w| w.starting);
            return match starting {
                Some(at) => ("starting".into(), at),
                None => ("not_running".into(), told.unwrap_or(0.0)),
            };
        };
        let state = self.tasks.pane_status(&pane);
        let (status, since) = match state.as_deref() {
            None if watch.is_some_and(|w| w.starting.is_some()) => ("starting", watch.and_then(|w| w.starting).unwrap_or(0.0)),
            Some("working") if waiting => ("elsewhere", watch.map_or(0.0, |w| w.since)),
            Some("working") => ("working", watch.map_or(0.0, |w| w.since)),
            Some("blocked" | "needs-you") => ("blocked", watch.map_or(0.0, |w| w.since)),
            _ if last.as_ref().is_some_and(|i| i.kind == "ask" && i.doc.get("chosen").is_none()) => ("asks", told.unwrap_or(0.0)),
            _ => ("idle", told.unwrap_or(0.0)),
        };
        (status.to_string(), since)
    }

    fn tick(self: &Rc<Self>) {
        let revision = self.tasks.revision();
        if revision != self.told.replace(revision) {
            for repo in self.repos.borrow().keys() {
                notify(&json!({ "notice": "tasks", "repo": repo }));
            }
            let records = self.tasks.project_records();
            if *self.records.borrow() != records {
                *self.records.borrow_mut() = records;
                notify(&json!({ "notice": "projects" }));
            }
        }
        let ticks = self.ticks.get();
        self.ticks.set(ticks + 1);
        let known = self.db.projects().map_or(0, |projects| projects.len());
        if ticks % SCAN_EVERY == 0 || known != self.known.replace(known) {
            self.scan(|_| {});
        }
        let repos: Vec<String> = self.watches.borrow().keys().cloned().collect();
        for path in repos {
            self.tick_repo(&path);
        }
    }

    fn tick_repo(self: &Rc<Self>, path: &str) {
        let time = now();
        let pane = self.tasks.planner_pane(path);
        let state = pane.as_deref().and_then(|p| self.tasks.pane_status(p));
        let working = state.as_deref() == Some("working");
        let mut retell = vec![];
        let mut sys = None;
        let mut nudge = false;
        let mut quit = None;
        {
            let mut watches = self.watches.borrow_mut();
            let watch = watches.entry(path.to_string()).or_default();
            if working && !watch.worked {
                watch.since = time;
            }
            watch.worked = working;
            if state.is_some() {
                watch.starting = None;
            }
            if watch.starting.is_some_and(|at| time - at > START_LIMIT) {
                watch.starting = None;
                sys = Some(format!("The planner couldn't start on {}; open the repo there once", self.machine()));
            }
            for item in self.db.phone_unread(path).unwrap_or_default() {
                if let Some(delivered) = item.message.and_then(|id| self.db.message(id).ok().flatten()).and_then(|m| m.delivered) {
                    let _ = self.db.set_phone_read(path, item.seq);
                    retell.push(item);
                    watch.answering = Some(delivered);
                    watch.nudged = false;
                }
            }
            if let Some(delivered) = watch.answering.filter(|at| state.as_deref() == Some("idle") && time - at > SETTLE) {
                let _ = delivered;
                match watch.nudged {
                    false => {
                        nudge = true;
                        watch.nudged = true;
                        watch.answering = None;
                    }
                    true => {
                        watch.answering = None;
                        watch.nudged = false;
                        sys = Some("The planner finished without answering".into());
                    }
                }
            }
            match state.as_deref() {
                Some("idle") if self.tasks.windowless_pane(path).is_some() => {
                    let since = *watch.idle.get_or_insert(time);
                    if time - since > IDLE_LIMIT {
                        quit = pane.clone();
                        watch.idle = None;
                    }
                }
                _ => watch.idle = None,
            }
        }
        for item in retell {
            self.tell(path, &item);
        }
        if nudge {
            if let Ok(message) = self.tasks.queue(PLANNER, path, KANNA, NUDGE, None, None) {
                log!("kanna-daemon: told the planner of {path} the phone waits (message {})", message.id);
                let mut watches = self.watches.borrow_mut();
                if let Some(watch) = watches.get_mut(path) {
                    watch.answering = Some(time + SETTLE);
                    watch.nudged = true;
                }
            }
        }
        if let Some(text) = sys {
            let _ = self.put(path, "sys", None, None, json!({ "text": text }));
        }
        if let Some(pane) = quit {
            self.tasks.quit_windowless(path, &pane);
        }
        let (status, since) = self.status(path);
        let changed = {
            let mut watches = self.watches.borrow_mut();
            let watch = watches.entry(path.to_string()).or_default();
            let changed = watch.told.as_ref().is_none_or(|(old, _)| *old != status);
            if changed {
                watch.told = Some((status.clone(), if since > 0.0 { since } else { time }));
            }
            changed
        };
        if changed {
            if let Some(repo) = self.identity_of(path) {
                notify(&json!({ "notice": "planner", "repo": repo, "status": status, "since": num(since) }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(planner: Option<&str>, clones: &[&str], ran: &[&str]) -> ProjectRecord {
        ProjectRecord { repo: "r".into(), name: "r".into(), planner: planner.map(String::from), clones: clones.iter().map(|k| k.to_string()).collect(), ran: ran.iter().map(|k| k.to_string()).collect() }
    }

    #[test]
    fn the_planner_machine_is_the_record_else_who_ran_one_else_a_clone() {
        let all = |_: &str| true;
        assert_eq!(record(Some("b"), &["a", "b"], &["a"]).planner_machine("a", &all).as_deref(), Some("b"));
        assert_eq!(record(None, &["a", "b"], &["b"]).planner_machine("a", &all).as_deref(), Some("b"));
        assert_eq!(record(None, &["a", "b"], &["a", "b"]).planner_machine("b", &all).as_deref(), Some("b"));
        assert_eq!(record(None, &["a", "b"], &[]).planner_machine("b", &all).as_deref(), Some("b"));
        assert_eq!(record(None, &["a", "b"], &[]).planner_machine("c", &all).as_deref(), Some("a"));
    }

    #[test]
    fn a_revoked_machine_is_no_planner() {
        let live = |key: &str| key != "gone";
        assert_eq!(record(Some("gone"), &["gone", "a"], &["gone"]).planner_machine("a", &live).as_deref(), Some("a"));
        assert_eq!(record(None, &["gone"], &[]).planner_machine("a", &live), None);
    }
}
