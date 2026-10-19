use crate::diffs::Diffs;
use crate::layout::DaemonLayout;
use crate::settings::Settings;
use crate::terminals::Terminals;
use crate::Event;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_mux::json::{encode, json, string, Map, Value};
use kanna_mux::{pane_record, pane_table, write_all, KmuxError, Pane, PaneState, PaneType, Responder, Takeover, WeakCore};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const HELLO: &str = "app.hello";

const MAX_LINE: u64 = 16 << 20;

const QUEUE_BEHIND: usize = 16 << 20;

const QUEUE_CAUGHT_UP: usize = 1 << 20;

const QUEUE_LIMIT: usize = 64 << 20;

pub struct Outbox {
    lines: Sender<Vec<u8>>,
    queue: Arc<Queue>,
}

#[derive(Default)]
struct Queue {
    bytes: AtomicUsize,
    behind: AtomicBool,
}

impl Outbox {
    fn push(&self, message: &Value) -> usize {
        let mut line = encode(message).into_bytes();
        line.push(b'\n');
        let queued = self.queue.bytes.fetch_add(line.len(), Ordering::SeqCst) + line.len();
        let _ = self.lines.send(line);
        queued
    }
}

const SNAPSHOT_RETRY: Duration = Duration::from_millis(100);

pub const APP_PREFIX: &str = "debug.";

pub const APP_ASK: &str = "app.";

pub fn takeover(events: Sender<Event>) -> Takeover {
    let counter = Arc::new(AtomicU64::new(0));
    Arc::new(move |hello: Value, mut reader: BufReader<UnixStream>, stream: UnixStream| {
        let conn = counter.fetch_add(1, Ordering::SeqCst) + 1;
        let (sender, lines) = mpsc::channel::<Vec<u8>>();
        let out = Outbox { lines: sender, queue: Arc::default() };
        let (queue, caught_up) = (out.queue.clone(), events.clone());
        let (Ok(writer), Ok(closer)) = (stream.try_clone(), stream.try_clone()) else { return };
        let spawned = std::thread::Builder::new().name("kanna.app.write".into()).spawn(move || {
            for line in lines {
                if !write_all(&writer, &line) {
                    break;
                }
                let left = queue.bytes.fetch_sub(line.len(), Ordering::SeqCst) - line.len();
                if left <= QUEUE_CAUGHT_UP && queue.behind.swap(false, Ordering::SeqCst) {
                    let _ = caught_up.send(Event::AppCaughtUp { conn });
                }
            }
            let _ = writer.shutdown(Shutdown::Both);
        });
        if spawned.is_err() || events.send(Event::AppHello { conn, hello, out, stream: closer }).is_err() {
            return;
        }
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.by_ref().take(MAX_LINE + 1).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.len() as u64 > MAX_LINE {
                log!("kanna-daemon: the app sent a line over {} MiB; dropping it", MAX_LINE >> 20);
                break;
            }
            match serde_json::from_slice::<Value>(&line) {
                Ok(message) => {
                    if events.send(Event::AppMessage { conn, message }).is_err() {
                        break;
                    }
                }
                Err(_) => log!("kanna-daemon: the app sent a line that isn't JSON"),
            }
        }
        let _ = events.send(Event::AppGone { conn });
    })
}

struct Connection {
    id: u64,
    out: Outbox,
    stream: UnixStream,
    behind: Cell<bool>,
    cut: Cell<bool>,
}

pub struct AppLink {
    core: WeakCore,
    terminals: Rc<Terminals>,
    diffs: Rc<Diffs>,
    layout: Rc<DaemonLayout>,
    app: RefCell<Option<Connection>>,
    pending: RefCell<HashMap<u64, (Instant, Responder)>>,
    timeout: Duration,
    next_id: Cell<u64>,
    attached: RefCell<HashSet<String>>,
    unsent: RefCell<HashMap<String, Instant>>,
    backlog: RefCell<Option<Weak<crate::backlog::Backlog>>>,
    build: Option<String>,
}

impl AppLink {
    pub fn new(core: WeakCore, terminals: Rc<Terminals>, diffs: Rc<Diffs>, layout: Rc<DaemonLayout>, timeout: Duration, build: Option<String>) -> AppLink {
        AppLink {
            build,
            core,
            terminals,
            diffs,
            layout,
            app: RefCell::new(None),
            pending: RefCell::default(),
            backlog: RefCell::default(),
            timeout,
            next_id: Cell::new(0),
            attached: RefCell::default(),
            unsent: RefCell::default(),
        }
    }

    fn send(&self, message: Value) {
        let app = self.app.borrow();
        let Some(app) = app.as_ref().filter(|app| !app.cut.get()) else { return };
        let queued = app.out.push(&message);
        if queued > QUEUE_LIMIT {
            log!("kanna-daemon: the app is {} MiB behind; dropping it", queued >> 20);
            app.cut.set(true);
            let _ = app.stream.shutdown(Shutdown::Both);
        } else if queued > QUEUE_BEHIND && !app.behind.replace(true) {
            log!("kanna-daemon: the app is {} MiB behind; its terminals start over when it catches up", queued >> 20);
            app.out.queue.behind.store(true, Ordering::SeqCst);
        }
    }

    fn behind(&self) -> bool {
        self.app.borrow().as_ref().is_some_and(|app| app.behind.get())
    }

    fn shows(&self, pane: &str) -> bool {
        !self.behind() && self.attached.borrow().contains(pane)
    }

    pub fn caught_up(&self, conn: u64) {
        match self.app.borrow().as_ref() {
            Some(app) if app.id == conn => app.behind.set(false),
            _ => return,
        }
        let mut panes: Vec<String> = self.attached.borrow().iter().cloned().collect();
        panes.sort();
        for pane in panes {
            self.attach(&pane);
        }
    }

    pub fn resync(&self, pane: &str) {
        if self.shows(pane) {
            self.attach(pane);
        }
    }

    fn notify(&self, cmd: &str, args: Value) {
        self.send(json!({ "cmd": cmd, "args": args }));
    }

    pub fn hello(&self, conn: u64, hello: Value, out: Outbox, stream: UnixStream) -> bool {
        let id = hello.get("id").cloned().unwrap_or(Value::Null);
        let answer = |reply: Value| {
            out.push(&reply);
        };
        if self.app.borrow().is_some() {
            answer(json!({ "id": id, "ok": false, "error": { "code": "app_attached", "message": "another Kanna.app is attached to this daemon" } }));
            let _ = stream.shutdown(Shutdown::Read);
            return false;
        }
        let app_build = hello.get("args").and_then(|a| a.get("build")).and_then(Value::as_str);
        if let (Some(app_build), Some(build)) = (app_build, self.build.as_deref()) {
            if app_build != build {
                log!("kanna-daemon: the app is from another build ({app_build}, this daemon is {build}); stopping so it starts its own");
                let reply = json!({ "id": id, "ok": false, "error": { "code": "other_build", "message": format!("this daemon is from another build ({build}); it is stopping so the app can start its own") } });
                let mut line = encode(&reply).into_bytes();
                line.push(b'\n');
                write_all(&stream, &line);
                let _ = stream.shutdown(Shutdown::Both);
                return true;
            }
        }
        let pid = hello.get("args").and_then(|a| a.get("pid")).cloned().unwrap_or(Value::Null);
        log!("kanna-daemon: the app attached (pid {pid})");
        self.layout.flush();
        let panes = self.core.upgrade().map_or(Value::Array(vec![]), |core| core.with_panes(pane_table));
        answer(json!({ "id": id, "ok": true, "daemon": { "pid": std::process::id(), "build": self.build }, "panes": panes, "layout": self.layout.view() }));
        *self.app.borrow_mut() = Some(Connection { id: conn, out, stream, behind: Cell::new(false), cut: Cell::new(false) });
        let statuses: Vec<(String, String)> = self.core.upgrade().map_or(vec![], |core| {
            core.with_panes(|panes| panes.iter().filter_map(|p| p.status().map(|s| (p.id.clone(), s.to_string()))).collect())
        });
        for (pane, status) in statuses {
            self.status(&pane, Some(&status));
        }
        let folders: Vec<(String, String)> = self.core.upgrade().map_or(vec![], |core| {
            core.with_panes(|panes| panes.iter().filter_map(|p| p.cwd.clone().map(|cwd| (p.id.clone(), cwd))).collect())
        });
        for (pane, cwd) in folders {
            self.folder(&pane, &cwd);
        }
        let tasks = self.backlog.borrow().as_ref().and_then(|b| b.upgrade()).map(|b| b.view());
        if let Some(tasks) = tasks {
            self.notify("backlog", tasks);
        }
        if let Some(core) = self.core.upgrade() {
            core.handle(json!({ "cmd": "list" }), |_| {});
        }
        false
    }

    pub fn connect_backlog(self: &Rc<Self>, backlog: &Rc<crate::backlog::Backlog>) {
        *self.backlog.borrow_mut() = Some(Rc::downgrade(backlog));
        let link = Rc::downgrade(self);
        *backlog.on_change.borrow_mut() = Some(Box::new(move |tasks| {
            if let Some(link) = link.upgrade() {
                link.notify("backlog", tasks);
            }
        }));
    }

    pub fn folder(&self, pane: &str, cwd: &str) {
        self.notify("folder", json!({ "pane": pane, "cwd": cwd }));
    }

    pub fn status(&self, pane: &str, status: Option<&str>) {
        self.notify("status", json!({ "pane": pane, "status": status }));
    }

    pub fn attached(&self) -> bool {
        self.app.borrow().is_some()
    }

    pub fn gone(&self, conn: u64) {
        if self.app.borrow().as_ref().map(|a| a.id) != Some(conn) {
            return;
        }
        log!("kanna-daemon: the app went away");
        self.app.borrow_mut().take();
        self.attached.borrow_mut().clear();
        self.unsent.borrow_mut().clear();
        let prefix = view_prefix(conn);
        self.diffs.leave(|view| view.starts_with(&prefix));
        let pending: Vec<Responder> = self.pending.borrow_mut().drain().map(|(_, (_, r))| r).collect();
        for responder in pending {
            responder.reply(Err(KmuxError::new("app_gone", "Kanna.app went away before answering")));
        }
        let Some(core) = self.core.upgrade() else { return };
        let starting: Vec<Pane> = core.with_panes(|panes| panes.iter().filter(|p| p.kind != PaneType::Term && p.state == PaneState::Starting).cloned().collect());
        for pane in starting {
            self.terminals.unshown(&pane);
        }
    }

    pub fn close(&self) {
        if let Some(app) = self.app.borrow_mut().take() {
            let _ = app.stream.shutdown(Shutdown::Both);
        }
    }

    pub fn message(&self, conn: u64, message: Value) -> Option<Value> {
        if self.app.borrow().as_ref().map(|a| a.id) != Some(conn) {
            return None;
        }
        if message.get("ok").is_some() {
            let responder = message.get("id").and_then(Value::as_u64).and_then(|id| self.pending.borrow_mut().remove(&id));
            if let Some((_, responder)) = responder {
                responder.reply(answer(message));
            }
            return None;
        }
        let args = message.get("args").cloned().unwrap_or_else(|| json!({}));
        let view = string(&args, "view").map(|view| format!("{}{view}", view_prefix(conn)));
        match (string(&message, "cmd").unwrap_or_default(), view) {
            ("diff.subscribe", Some(view)) => {
                let reply = match self.diffs.subscribe(&view, &args) {
                    Ok(mut fields) => {
                        fields.insert("id".into(), message.get("id").cloned().unwrap_or(Value::Null));
                        fields.insert("ok".into(), Value::Bool(true));
                        Value::Object(fields)
                    }
                    Err(error) => json!({ "id": message.get("id").cloned().unwrap_or(Value::Null), "ok": false, "error": { "code": error.code, "message": error.message } }),
                };
                self.send(reply);
                return None;
            }
            ("diff.show", Some(view)) => {
                self.diffs.show(&view, args.get("show").unwrap_or(&Value::Null));
                return None;
            }
            ("diff.unsubscribe", Some(view)) => {
                self.diffs.unsubscribe(&view);
                return None;
            }
            _ => {}
        }
        if message.get("id").is_some() {
            return Some(message);
        }
        let pane = string(&args, "pane").unwrap_or_default().to_string();
        let terminal = string(&args, "terminal").unwrap_or_default().to_string();
        match string(&message, "cmd").unwrap_or_default() {
            "attach" => {
                if !self.attached.borrow().contains(&terminal) {
                    self.attach(&terminal);
                }
            }
            "detach" => {
                self.attached.borrow_mut().remove(&terminal);
                self.unsent.borrow_mut().remove(&terminal);
            }
            "input" => {
                if let Some(bytes) = string(&args, "data").and_then(|d| BASE64.decode(d).ok()) {
                    self.terminals.write(&terminal, bytes);
                }
            }
            "resize" => {
                let field = |key: &str| args.get(key).and_then(Value::as_u64).unwrap_or(0).min(u16::MAX as u64) as u16;
                if let Some((cols, rows)) = self.terminals.resize(&terminal, field("cols"), field("rows"), field("width"), field("height")) {
                    self.notify("grid", json!({ "terminal": terminal, "cols": cols, "rows": rows }));
                }
            }
            "update" => self.updated(&pane, &args),
            "settings" => self.terminals.set_settings(Settings::from_value(&args)),
            other => log!("kanna-daemon: the app sent an unknown message \"{other}\""),
        }
        None
    }

    pub fn answer_app(&self, conn: u64, reply: Value) {
        if self.app.borrow().as_ref().map(|a| a.id) == Some(conn) {
            self.send(reply);
        }
    }

    fn updated(&self, id: &str, args: &Value) {
        let Some(core) = self.core.upgrade() else { return };
        if core.pane(id).is_none_or(|p| p.kind == PaneType::Term) {
            return;
        }
        if let Some(url) = string(args, "url") {
            core.page_moved(id, url);
        }
        core.with_pane_mut(id, |pane| {
            if let Some(title) = string(args, "title").filter(|_| pane.kind == PaneType::Web) {
                let title: String = title.chars().filter(|c| !c.is_control()).collect();
                pane.title = Some(title.trim().to_string()).filter(|t| !t.is_empty());
            }
            if let Some(device) = string(args, "device") {
                pane.device = Some(device.into());
            }
            if let Some(view) = string(args, "view").filter(|v| *v == "unified" || *v == "split") {
                pane.view = Some(view.into());
            }
        });
        if let Some(state) = string(args, "state").and_then(PaneState::parse) {
            core.update(id, state, None, string(args, "error").map(String::from));
        }
        if let Some(pane) = core.pane(id) {
            self.notify("pane", json!({ "pane": Value::Object(pane_record(&pane)) }));
        }
    }

    pub fn show(&self, pane: &Pane) {
        self.notify("start", json!({ "pane": Value::Object(pane_record(pane)) }));
    }

    pub fn diff(&self, views: &[String], payload: &Value) {
        let Some(conn) = self.app.borrow().as_ref().map(|a| a.id) else { return };
        let prefix = view_prefix(conn);
        if views.iter().any(|view| view.starts_with(&prefix)) {
            self.notify("diff", payload.clone());
        }
    }

    pub fn layout(&self, view: &Value) {
        self.notify("layout", view.clone());
    }

    pub fn hide(&self, pane: &str) {
        self.notify("stop", json!({ "pane": pane }));
    }

    fn attach(&self, pane: &str) {
        self.attached.borrow_mut().insert(pane.into());
        let Some(attach) = self.terminals.attach(pane) else { return };
        let snapshot = match attach.snapshot {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let retry = Instant::now() + SNAPSHOT_RETRY;
                if self.unsent.borrow_mut().insert(pane.into(), retry).is_none() {
                    log!("kanna-daemon: can't snapshot {pane} yet ({error}); trying again shortly");
                }
                return;
            }
        };
        self.unsent.borrow_mut().remove(pane);
        if self.shows(pane) {
            self.notify("snapshot", json!({ "terminal": pane, "data": BASE64.encode(&snapshot) }));
            self.notify("grid", json!({ "terminal": pane, "cols": attach.cols, "rows": attach.rows }));
        }
        if let Some(code) = attach.exit_code {
            self.exited(pane, code);
        }
    }

    pub fn output(&self, pane: &str, bytes: &[u8]) {
        if !self.shows(pane) {
            return;
        }
        if self.unsent.borrow().contains_key(pane) {
            return;
        }
        self.notify("output", json!({ "terminal": pane, "data": BASE64.encode(bytes) }));
    }

    pub fn exited(&self, pane: &str, code: i32) {
        if !self.shows(pane) {
            return;
        }
        if self.unsent.borrow().contains_key(pane) {
            return;
        }
        self.notify("exited", json!({ "terminal": pane, "code": code }));
    }

    pub fn started(&self, terminal: &str) {
        let Some(pane) = self.core.upgrade().and_then(|core| core.pane(terminal)) else { return };
        self.notify("pane", json!({ "pane": Value::Object(pane_record(&pane)) }));
        if let Some(cwd) = &pane.cwd {
            self.folder(&pane.id, cwd);
        }
    }

    pub fn forward(&self, cmd: &str, mut args: Value, responder: Responder) {
        if self.app.borrow().is_none() {
            return responder.reply(Err(no_app(cmd)));
        }
        if let (Some(reference), Some(core)) = (string(&args, "pane").map(String::from), self.core.upgrade()) {
            match core.pane(&reference) {
                Some(pane) => args["pane"] = Value::String(pane.id),
                None => return responder.reply(Err(KmuxError::new("not_found", format!("no pane \"{reference}\"")))),
            }
        }
        self.ask(cmd, args, responder);
    }

    fn ask(&self, cmd: &str, args: Value, responder: Responder) {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        self.pending.borrow_mut().insert(id, (Instant::now() + self.timeout, responder));
        self.send(json!({ "id": id, "cmd": cmd, "args": args }));
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let replies = self.pending.borrow().values().map(|(deadline, _)| *deadline).min();
        let snapshots = self.unsent.borrow().values().copied().min();
        [replies, snapshots].into_iter().flatten().min()
    }

    pub fn expire(&self, now: Instant) {
        let due: Vec<String> = self.unsent.borrow().iter().filter(|(_, retry)| **retry <= now).map(|(pane, _)| pane.clone()).collect();
        for pane in due {
            self.unsent.borrow_mut().remove(&pane);
            if self.attached.borrow().contains(&pane) {
                self.attach(&pane);
            }
        }
        let late: Vec<u64> = self.pending.borrow().iter().filter(|(_, (deadline, _))| *deadline <= now).map(|(id, _)| *id).collect();
        for id in late {
            let Some((_, responder)) = self.pending.borrow_mut().remove(&id) else { continue };
            let seconds = self.timeout.as_secs_f64();
            responder.reply(Err(KmuxError::new("timeout", format!("Kanna.app didn't answer within {seconds}s"))));
        }
    }
}

fn answer(message: Value) -> Result<Map, KmuxError> {
    let Value::Object(mut fields) = message else { return Err(KmuxError::new("bad_request", "the app's answer isn't an object")) };
    if fields.get("ok") != Some(&Value::Bool(true)) {
        let error = fields.get("error").cloned().unwrap_or(Value::Null);
        let text = |key: &str, default: &str| string(&error, key).unwrap_or(default).to_string();
        return Err(KmuxError::new(text("code", "bad_request"), text("message", "the app failed")));
    }
    fields.remove("id");
    fields.remove("ok");
    Ok(fields)
}

fn view_prefix(conn: u64) -> String {
    format!("app{conn}:")
}

fn no_app(what: &str) -> KmuxError {
    KmuxError::new("no_app", format!("{what} needs Kanna.app, and none is attached to this daemon"))
}
