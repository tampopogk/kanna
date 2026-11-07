use crate::diffs::Diffs;
use crate::files::Files;
use crate::serial::Serial;
use crate::layout::DaemonLayout;
use crate::perf;
use crate::wire::Wire;
use crate::settings::Settings;
use crate::sizes::{Driver, Size, Sizes};
use crate::terminals::Terminals;
use crate::Event;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_mux::json::{encode, json, string, Map, Value};
use kanna_mux::{pane_record, pane_table, write_all, KmuxError, Pane, PaneState, PaneType, Responder, Takeover, WeakCore};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub const HELLO: &str = "app.hello";

const MAX_LINE: u64 = 16 << 20;

pub const QUEUE_BEHIND: usize = 16 << 20;

const QUEUE_CAUGHT_UP: usize = 1 << 20;

const QUEUE_LIMIT: usize = 64 << 20;

struct Line {
    bytes: Vec<u8>,
    typed: Option<Instant>,
    terminal: Option<String>,
}

#[derive(Default)]
struct Lanes {
    urgent: VecDeque<Line>,
    bulk: VecDeque<Line>,
    queued: HashMap<String, usize>,
}

#[derive(Default)]
struct Queue {
    lanes: Mutex<Lanes>,
    ready: Condvar,
    bytes: AtomicUsize,
    behind: AtomicBool,
}

#[derive(Clone, Default)]
pub struct Outbox {
    queue: Arc<Queue>,
}

impl Outbox {
    pub fn push(&self, message: &Value) -> usize {
        self.push_for(None, message, None)
    }

    pub fn behind(&self) -> bool {
        self.queue.behind.load(Ordering::SeqCst)
    }

    pub fn fell_behind(&self) -> bool {
        !self.queue.behind.swap(true, Ordering::SeqCst)
    }

    pub fn push_for(&self, terminal: Option<&str>, message: &Value, typed: Option<Instant>) -> usize {
        let mut bytes = encode(message).into_bytes();
        bytes.push(b'\n');
        let queued = self.queue.bytes.fetch_add(bytes.len(), Ordering::SeqCst) + bytes.len();
        let mut lanes = self.queue.lanes.lock().unwrap_or_else(PoisonError::into_inner);
        let alone = terminal.is_none_or(|terminal| !lanes.queued.contains_key(terminal));
        if let Some(terminal) = terminal {
            *lanes.queued.entry(terminal.to_string()).or_default() += 1;
        }
        let line = Line { bytes, typed, terminal: terminal.map(String::from) };
        if typed.is_some() && alone {
            lanes.urgent.push_back(line);
        } else {
            lanes.bulk.push_back(line);
        }
        self.queue.ready.notify_one();
        queued
    }

    fn next(&self) -> Option<Line> {
        let mut lanes = self.queue.lanes.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(line) = lanes.urgent.pop_front().or_else(|| lanes.bulk.pop_front()) {
                return Some(line);
            }
            if Arc::strong_count(&self.queue) == 1 {
                return None;
            }
            lanes = self.queue.ready.wait_timeout(lanes, Duration::from_millis(500)).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    fn written(&self, line: &Line) -> usize {
        if let Some(terminal) = &line.terminal {
            let mut lanes = self.queue.lanes.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(count) = lanes.queued.get_mut(terminal) {
                *count -= 1;
                if *count == 0 {
                    lanes.queued.remove(terminal);
                }
            }
        }
        self.queue.bytes.fetch_sub(line.bytes.len(), Ordering::SeqCst) - line.bytes.len()
    }
}

const SNAPSHOT_RETRY: Duration = Duration::from_millis(100);

pub const APP_PREFIX: &str = "debug.";

pub const APP_ASK: &str = "app.";

pub const RELAY_TOKEN: &str = "machine.relay_token";

pub const PROTOCOL: u64 = kanna_build::PROTOCOL as u64;

static CONNECTIONS: AtomicU64 = AtomicU64::new(0);

pub fn takeover(events: crate::runtime::Events, wire: Arc<Wire>) -> Takeover {
    Arc::new(move |hello: Value, reader: BufReader<UnixStream>, stream: UnixStream| serve(events.clone(), wire.clone(), hello, reader, stream, None))
}

fn on_the_wire(wire: &Wire, conn: u64, message: &Value, at: Instant) -> bool {
    if message.get("id").is_some() {
        return false;
    }
    let args = message.get("args").unwrap_or(&Value::Null);
    let terminal = string(args, "terminal").unwrap_or_default();
    match string(message, "cmd") {
        Some("input") => {
            if let Some(bytes) = string(args, "data").and_then(|d| BASE64.decode(d).ok()) {
                wire.input(conn, terminal, bytes, at);
            }
            true
        }
        Some("taken") => {
            wire.taken(conn, terminal);
            true
        }
        Some("used") => {
            wire.used(conn, terminal);
            true
        }
        _ => false,
    }
}

pub fn serve(events: crate::runtime::Events, wire: Arc<Wire>, hello: Value, mut reader: BufReader<UnixStream>, stream: UnixStream, remote: Option<String>) {
    let conn = CONNECTIONS.fetch_add(1, Ordering::SeqCst) + 1;
    let out = Outbox::default();
    let (lines, caught_up) = (Outbox { queue: out.queue.clone() }, events.clone());
    let (Ok(writer), Ok(closer)) = (stream.try_clone(), stream.try_clone()) else { return };
    let spawned = std::thread::Builder::new().name("kanna.app.write".into()).spawn(move || {
        while let Some(line) = lines.next() {
            if !write_all(&writer, &line.bytes) {
                break;
            }
            if let Some(typed) = line.typed {
                perf::since("echo.sent", typed);
            }
            let left = lines.written(&line);
            if left <= QUEUE_CAUGHT_UP && lines.queue.behind.swap(false, Ordering::SeqCst) {
                let _ = caught_up.send(Event::AppCaughtUp { conn });
            }
        }
        let _ = writer.shutdown(Shutdown::Both);
    });
    if spawned.is_err() || events.send(Event::AppHello { conn, hello, out, stream: closer, remote }).is_err() {
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
                let at = Instant::now();
                if on_the_wire(&wire, conn, &message, at) {
                    continue;
                }
                if events.send(Event::AppMessage { conn, message }).is_err() {
                    break;
                }
            }
            Err(_) => log!("kanna-daemon: the app sent a line that isn't JSON"),
        }
    }
    let _ = events.send(Event::AppGone { conn });
}

struct Connection {
    remote: Option<String>,
    client: String,
    out: Outbox,
    stream: UnixStream,
    behind: Cell<bool>,
    cut: Cell<bool>,
    keeps: bool,
    attached: RefCell<HashMap<String, Option<usize>>>,
    unsent: RefCell<HashMap<String, Instant>>,
}

impl Connection {
    fn name(&self) -> String {
        match &self.remote {
            Some(_) => format!("the app on {}", self.client),
            None => "the app".into(),
        }
    }
}

fn write_with(stream: &UnixStream, reply: &Value, listener: Option<&OwnedFd>) -> bool {
    let mut line = encode(reply).into_bytes();
    line.push(b'\n');
    let Some(listener) = listener else { return write_all(stream, &line) };
    match kanna_keeper::fdpass::send(stream.as_raw_fd(), &line, &[listener.as_raw_fd()], false) {
        Ok(sent) => write_all(stream, &line[sent..]),
        Err(_) => false,
    }
}

pub struct AppLink {
    unnamed_folders: RefCell<HashMap<String, String>>,
    core: WeakCore,
    terminals: Rc<Terminals>,
    diffs: Rc<Diffs>,
    files: Rc<Files>,
    threads: Rc<Files>,
    serial: Rc<Serial>,
    layout: Rc<DaemonLayout>,
    apps: RefCell<BTreeMap<u64, Connection>>,
    sizes: RefCell<Sizes>,
    last_used: Cell<u64>,
    seen_here: Cell<bool>,
    pending: RefCell<HashMap<u64, (Instant, u64, Responder)>>,
    timeout: Duration,
    next_id: Cell<u64>,
    stopping: Cell<bool>,
    tasks: RefCell<Option<Weak<crate::tasks::Tasks>>>,
    build: Option<String>,
    run: String,
    listener: Option<OwnedFd>,
    handed: Cell<bool>,
    pub machine: RefCell<Option<Box<dyn Fn() -> Value>>>,
    pub logs: RefCell<Option<Rc<crate::logs::Logs>>>,
}

impl AppLink {
    pub fn new(core: WeakCore, terminals: Rc<Terminals>, diffs: Rc<Diffs>, files: (Rc<Files>, Rc<Files>), serial: Rc<Serial>, layout: Rc<DaemonLayout>, timeout: Duration, build: Option<String>, listener: Option<OwnedFd>) -> AppLink {
        let (files, threads) = files;
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        AppLink {
            unnamed_folders: RefCell::default(),
            build,
            run: format!("{:x}-{:x}", std::process::id(), started.as_nanos()),
            listener,
            handed: Cell::new(false),
            core,
            terminals,
            diffs,
            files,
            threads,
            serial,
            layout,
            apps: RefCell::default(),
            sizes: RefCell::default(),
            last_used: Cell::new(0),
            seen_here: Cell::new(false),
            pending: RefCell::default(),
            tasks: RefCell::default(),
            timeout,
            next_id: Cell::new(0),
            stopping: Cell::new(false),
            machine: RefCell::new(None),
            logs: RefCell::new(None),
        }
    }

    fn send_to(&self, conn: u64, message: &Value) {
        let apps = self.apps.borrow();
        let Some(app) = apps.get(&conn).filter(|app| !app.cut.get()) else { return };
        let queued = app.out.push(message);
        if queued > QUEUE_LIMIT {
            log!("kanna-daemon: {} is {} MiB behind; dropping it", app.name(), queued >> 20);
            app.cut.set(true);
            let _ = app.stream.shutdown(Shutdown::Both);
        } else if queued > QUEUE_BEHIND {
            app.out.queue.behind.store(true, Ordering::SeqCst);
            self.fell_behind(conn, queued);
        }
    }

    fn send(&self, message: Value) {
        for conn in self.conns() {
            self.send_to(conn, &message);
        }
    }

    fn conns(&self) -> Vec<u64> {
        self.apps.borrow().keys().copied().collect()
    }

    pub fn fell_behind(&self, conn: u64, queued: usize) {
        let apps = self.apps.borrow();
        let Some(app) = apps.get(&conn) else { return };
        if !app.behind.replace(true) {
            log!("kanna-daemon: {} is {} MiB behind; its terminals start over when it catches up", app.name(), queued >> 20);
            self.terminals.hide_all(conn);
        }
    }

    fn shows(&self, conn: u64, pane: &str) -> bool {
        self.apps.borrow().get(&conn).is_some_and(|app| !app.behind.get() && app.attached.borrow().contains_key(pane))
    }

    pub fn caught_up(&self, conn: u64) {
        let mut panes: Vec<String> = match self.apps.borrow().get(&conn) {
            Some(app) => {
                app.behind.set(false);
                app.attached.borrow().keys().cloned().collect()
            }
            None => return,
        };
        panes.sort();
        for pane in panes {
            self.attach(conn, &pane, None);
        }
    }

    pub fn resync(&self, conn: u64, pane: &str) {
        if self.shows(conn, pane) {
            self.attach(conn, pane, None);
        }
    }

    fn notify(&self, cmd: &str, args: Value) {
        self.send(json!({ "cmd": cmd, "args": args }));
    }

    fn notify_to(&self, conn: u64, cmd: &str, args: Value) {
        self.send_to(conn, &json!({ "cmd": cmd, "args": args }));
    }

    pub fn hello(&self, conn: u64, hello: Value, out: Outbox, stream: UnixStream, remote: Option<String>) -> bool {
        let id = hello.get("id").cloned().unwrap_or(Value::Null);
        let answer = |reply: Value| {
            out.push(&reply);
        };
        if self.stopping.get() {
            answer(json!({ "id": id, "ok": false, "error": { "code": "stopping", "message": "this Kanna is stopping" } }));
            let _ = stream.shutdown(Shutdown::Read);
            return false;
        }
        if remote.is_none() && self.attached_here() {
            answer(json!({ "id": id, "ok": false, "error": { "code": "app_attached", "message": "this machine's Kanna.app is already attached to this daemon" } }));
            let _ = stream.shutdown(Shutdown::Read);
            return false;
        }
        let args = hello.get("args").cloned().unwrap_or(Value::Null);
        let app_build = args.get("build").and_then(Value::as_str).filter(|_| remote.is_none());
        let keeps = remote.is_none() && args.get("keepsSocket") == Some(&Value::Bool(true));
        let listener = self.listener.as_ref().filter(|_| keeps);
        if let (Some(app_build), Some(build)) = (app_build, self.build.as_deref()) {
            if app_build != build {
                log!("kanna-daemon: the app is from another build ({app_build}, this daemon is {build}); stopping so it starts its own");
                let reply = json!({ "id": id, "ok": false, "listener": listener.is_some(), "error": { "code": "other_build", "message": format!("this daemon is from another build ({build}); it is stopping so the app can start its own") } });
                self.handed.set(write_with(&stream, &reply, listener) && listener.is_some());
                let _ = stream.shutdown(Shutdown::Both);
                return true;
            }
        }
        let protocol = args.get("protocol").and_then(Value::as_u64);
        if protocol != Some(PROTOCOL) {
            let theirs = protocol.map_or("an older protocol".to_string(), |p| format!("protocol {p}"));
            log!("kanna-daemon: an app speaking {theirs} said hello; this daemon speaks {PROTOCOL}");
            answer(json!({ "id": id, "ok": false, "error": { "code": "other_protocol", "message": format!("this daemon speaks protocol {PROTOCOL} and the app {theirs}") }, "protocol": PROTOCOL, "commit": kanna_build::COMMIT, "version": kanna_build::VERSION }));
            let _ = stream.shutdown(Shutdown::Read);
            return false;
        }
        let pid = args.get("pid").cloned().unwrap_or(Value::Null);
        match (&remote, args.get("client").and_then(Value::as_str)) {
            (Some(from), client) => log!("kanna-daemon: the app on {} attached from another machine (pid {pid})", client.unwrap_or(from)),
            (None, _) => log!("kanna-daemon: the app attached (pid {pid})"),
        }
        self.layout.flush();
        let panes = self.core.upgrade().map_or(Value::Array(vec![]), |core| core.with_panes(pane_table));
        let mut daemon = json!({ "pid": std::process::id(), "build": self.build, "run": self.run, "commit": kanna_build::COMMIT, "version": kanna_build::VERSION, "protocol": PROTOCOL, "machine": null, "capabilities": null });
        if let Some(describe) = self.machine.borrow().as_ref() {
            let machine = describe();
            daemon["machine"] = machine["name"].clone();
            daemon["capabilities"] = machine["capabilities"].clone();
        }
        let reply = json!({ "id": id, "ok": true, "protocol": PROTOCOL, "listener": listener.is_some(), "daemon": daemon, "panes": panes, "layout": self.layout.view() });
        if listener.is_some() {
            if !write_with(&stream, &reply, listener) {
                let _ = stream.shutdown(Shutdown::Both);
                return false;
            }
        } else {
            answer(reply);
        }
        let client = remote.as_ref().map_or_else(|| kanna_mux::MAC_APP.to_string(), |from| args.get("client").and_then(Value::as_str).unwrap_or(from).to_string());
        self.seen_here.set(self.seen_here.get() || remote.is_none());
        self.terminals.wire().connect(conn, out.clone());
        self.apps.borrow_mut().insert(conn, Connection { remote, client, out, stream, behind: Cell::new(false), cut: Cell::new(false), keeps: listener.is_some(), attached: RefCell::default(), unsent: RefCell::default() });
        self.claim();
        let statuses: Vec<(String, String)> = self.core.upgrade().map_or(vec![], |core| {
            core.with_panes(|panes| panes.iter().filter_map(|p| p.status().map(|s| (p.id.clone(), s.to_string()))).collect())
        });
        for (pane, status) in statuses {
            self.notify_to(conn, "status", json!({ "pane": pane, "status": status }));
        }
        let folders: Vec<(String, String)> = self.core.upgrade().map_or(vec![], |core| {
            core.with_panes(|panes| panes.iter().filter_map(|p| p.cwd.clone().map(|cwd| (p.id.clone(), cwd))).collect())
        });
        for (pane, cwd) in folders {
            let notice = self.folder_notice(&pane, &cwd);
            self.notify_to(conn, "folder", notice);
        }
        let tasks = self.tasks.borrow().as_ref().and_then(|b| b.upgrade()).map(|b| b.view());
        if let Some(tasks) = tasks {
            self.notify_to(conn, "tasks", tasks);
        }
        if let Some(core) = self.core.upgrade() {
            core.handle(json!({ "cmd": "list" }), |_| {});
        }
        false
    }

    pub fn connect_tasks(self: &Rc<Self>, tasks: &Rc<crate::tasks::Tasks>) {
        *self.tasks.borrow_mut() = Some(Rc::downgrade(tasks));
        let link = Rc::downgrade(self);
        *tasks.on_change.borrow_mut() = Some(Box::new(move |tasks| {
            if let Some(link) = link.upgrade() {
                link.notify("tasks", tasks);
            }
        }));
    }

    fn folder_notice(&self, pane: &str, cwd: &str) -> Value {
        let place = crate::projects::place(cwd);
        let unnamed = !place["main"].is_null() && place["project"].is_null();
        match unnamed {
            true => self.unnamed_folders.borrow_mut().insert(pane.to_string(), cwd.to_string()),
            false => self.unnamed_folders.borrow_mut().remove(pane),
        };
        json!({ "pane": pane, "cwd": cwd, "place": place })
    }

    pub fn connect_schedules(self: &Rc<Self>, scheduler: &crate::schedule::Scheduler) {
        let link = Rc::downgrade(self);
        *scheduler.schedules.on_change.borrow_mut() = Some(Box::new(move || {
            if let Some(link) = link.upgrade() {
                link.notify("schedules", json!({}));
            }
        }));
    }

    pub fn stats_changed(&self, repo: &str) {
        self.notify("stats", json!({ "repo": repo }));
    }

    pub fn folder(&self, pane: &str, cwd: &str) {
        if self.attached() {
            let notice = self.folder_notice(pane, cwd);
            self.notify("folder", notice);
        }
    }

    pub fn project_found(&self) {
        let waiting = std::mem::take(&mut *self.unnamed_folders.borrow_mut());
        waiting.iter().for_each(|(pane, cwd)| self.folder(pane, cwd));
    }

    pub fn status(&self, pane: &str, status: Option<&str>) {
        self.notify("status", json!({ "pane": pane, "status": status }));
    }

    pub fn toast(&self, record: &Value) -> bool {
        let Some(conn) = self.target() else { return false };
        self.notify_to(conn, "toast", record.clone());
        true
    }

    pub fn quit(&self) {
        self.stopping.set(true);
        self.notify("quit", json!({}));
    }

    pub fn attached(&self) -> bool {
        !self.apps.borrow().is_empty()
    }

    pub fn is_current(&self, conn: u64) -> bool {
        self.apps.borrow().contains_key(&conn)
    }

    pub fn is_here(&self, conn: u64) -> bool {
        self.apps.borrow().get(&conn).is_some_and(|app| app.remote.is_none())
    }

    pub fn holds_socket(&self) -> bool {
        self.handed.get() || self.apps.borrow().values().any(|app| app.keeps)
    }

    pub fn attached_here(&self) -> bool {
        self.apps.borrow().values().any(|app| app.remote.is_none())
    }

    pub fn answers(&self) -> bool {
        self.target().is_some()
    }

    fn target(&self) -> Option<u64> {
        let apps = self.apps.borrow();
        if let Some((conn, _)) = apps.iter().find(|(_, app)| app.remote.is_none()) {
            return Some(*conn);
        }
        if self.seen_here.get() {
            return None;
        }
        Some(self.last_used.get()).filter(|conn| apps.contains_key(conn)).or_else(|| apps.keys().next_back().copied())
    }

    fn claim(&self) {
        let client = self.target().and_then(|conn| self.apps.borrow().get(&conn).map(|app| app.client.clone()));
        if let Some(tasks) = self.tasks.borrow().as_ref().and_then(|b| b.upgrade()) {
            *tasks.claimant.borrow_mut() = client.unwrap_or_else(|| kanna_mux::MAC_APP.into());
        }
    }

    pub fn used(&self, conn: u64, terminal: &str) {
        if !self.is_current(conn) {
            return;
        }
        self.last_used.set(conn);
        self.claim();
        let driver = self.sizes.borrow_mut().used(conn, self.is_here(conn), terminal);
        self.drive(terminal, driver);
    }

    fn drive(&self, terminal: &str, driver: Option<Driver>) {
        if let Some((conn, size)) = driver {
            self.terminals.resize(conn, terminal, size.cols, size.rows, size.width, size.height);
        }
    }

    pub fn gone(&self, conn: u64) {
        let Some(app) = self.apps.borrow_mut().remove(&conn) else { return };
        log!("kanna-daemon: {} went away", app.name());
        self.terminals.wire().disconnect(conn);
        self.terminals.hide_all(conn);
        self.claim();
        let prefix = view_prefix(conn);
        self.diffs.leave(|view| view.starts_with(&prefix));
        self.files.leave(|view| view.starts_with(&prefix));
        if let Some(logs) = self.logs.borrow().as_ref() {
            logs.leave(|view| view.starts_with(&prefix));
        }
        self.threads.leave(|view| view.starts_with(&prefix));
        self.serial.leave_views(|view| view.starts_with(&prefix));
        let driven = self.sizes.borrow_mut().gone(conn);
        for (terminal, driver) in driven {
            self.drive(&terminal, Some(driver));
        }
        let asked: Vec<u64> = self.pending.borrow().iter().filter(|(_, (_, by, _))| *by == conn).map(|(id, _)| *id).collect();
        for id in asked {
            if let Some((_, _, responder)) = self.pending.borrow_mut().remove(&id) {
                responder.reply(Err(KmuxError::new("app_gone", "Kanna.app went away before answering")));
            }
        }
        let Some(core) = self.core.upgrade() else { return };
        if app.remote.is_none() {
            core.handle(json!({ "id": 0, "cmd": "desks", "args": { "desks": [] } }), |_| {});
        }
        if self.attached() {
            return;
        }
        let starting: Vec<Pane> = core.with_panes(|panes| panes.iter().filter(|p| p.kind != PaneType::Term && p.state == PaneState::Starting).cloned().collect());
        for pane in starting {
            self.terminals.unshown(&pane);
        }
    }

    pub fn close(&self) {
        self.terminals.wire().disconnect_all();
        for (_, app) in std::mem::take(&mut *self.apps.borrow_mut()) {
            let _ = app.stream.shutdown(Shutdown::Both);
        }
    }

    pub fn message(&self, conn: u64, message: Value) -> Option<Value> {
        if !self.is_current(conn) {
            return None;
        }
        if message.get("ok").is_some() {
            let id = message.get("id").and_then(Value::as_u64).filter(|id| self.pending.borrow().get(id).is_some_and(|(_, by, _)| *by == conn));
            if let Some((_, _, responder)) = id.and_then(|id| self.pending.borrow_mut().remove(&id)) {
                responder.reply(answer(message));
            }
            return None;
        }
        let args = message.get("args").cloned().unwrap_or_else(|| json!({}));
        let view = string(&args, "view").map(|view| format!("{}{view}", view_prefix(conn)));
        match (string(&message, "cmd").unwrap_or_default(), view) {
            ("diff.subscribe", Some(view)) => {
                let reply = self.diffs.subscribe(&view, &args);
                self.answer_subscribe(conn, &message, reply);
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
            ("log.subscribe", Some(view)) => {
                let reply = match self.logs.borrow().as_ref() {
                    Some(logs) => logs.subscribe(&view, &args),
                    None => Err(KmuxError::new("unsupported", "this daemon keeps no logs")),
                };
                self.answer_subscribe(conn, &message, reply);
                return None;
            }
            ("log.unsubscribe", Some(view)) => {
                if let Some(logs) = self.logs.borrow().as_ref() {
                    logs.unsubscribe(&view);
                }
                return None;
            }
            ("file.subscribe", Some(view)) => {
                let reply = self.files.subscribe(&view, &args);
                self.answer_subscribe(conn, &message, reply);
                return None;
            }
            ("serial.subscribe", Some(view)) => {
                let reply = self.serial.subscribe(&view, &args);
                self.answer_subscribe(conn, &message, reply);
                return None;
            }
            ("serial.unsubscribe", Some(view)) => {
                self.serial.unsubscribe(&view);
                return None;
            }
            ("file.unsubscribe", Some(view)) => {
                self.files.unsubscribe(&view);
                return None;
            }
            ("markup.subscribe", Some(view)) => {
                let mut args = args.clone();
                args["kind"] = json!("threads");
                let reply = self.threads.subscribe(&view, &args);
                self.answer_subscribe(conn, &message, reply);
                return None;
            }
            ("markup.unsubscribe", Some(view)) => {
                self.threads.unsubscribe(&view);
                return None;
            }
            _ => {}
        }
        if string(&message, "cmd") == Some(RELAY_TOKEN) {
            let id = message.get("id").cloned().unwrap_or(Value::Null);
            let token = self.machine.borrow().as_ref().map(|describe| describe()["relay_token"].clone()).unwrap_or(Value::Null);
            self.send_to(conn, &if self.is_here(conn) {
                json!({ "id": id, "ok": true, "token": token })
            } else {
                json!({ "id": id, "ok": false, "error": { "code": "local_only", "message": "a relay's token goes only to this machine's own app" } })
            });
            return None;
        }
        if message.get("id").is_some() {
            return Some(message);
        }
        let pane = string(&args, "pane").unwrap_or_default().to_string();
        let terminal = string(&args, "terminal").unwrap_or_default().to_string();
        match string(&message, "cmd").unwrap_or_default() {
            "attach" => {
                let fresh = !self.with_app(conn, |app| app.attached.borrow().contains_key(&terminal));
                if fresh {
                    let history = args.get("scrollback").and_then(Value::as_u64).map(|rows| rows as usize);
                    self.with_app(conn, |app| app.attached.borrow_mut().insert(terminal.clone(), history).is_some());
                    let from = args.get("from").and_then(Value::as_u64).filter(|_| string(&args, "run") == Some(self.run.as_str()));
                    self.attach(conn, &terminal, from);
                }
            }
            "scrollback" => {
                self.with_app(conn, |app| app.attached.borrow_mut().get_mut(&terminal).map(|history| *history = None).is_some());
                if self.shows(conn, &terminal) {
                    self.attach(conn, &terminal, None);
                }
            }
            "detach" => {
                self.with_app(conn, |app| {
                    app.unsent.borrow_mut().remove(&terminal);
                    app.attached.borrow_mut().remove(&terminal).is_some()
                });
                self.terminals.hide(conn, &terminal);
                self.terminals.wire().left(conn, &terminal);
                let driver = self.sizes.borrow_mut().left(conn, &terminal);
                self.drive(&terminal, driver);
            }
            "resize" => {
                let field = |key: &str| args.get(key).and_then(Value::as_u64).unwrap_or(0).min(u16::MAX as u64) as u16;
                let size = Size { cols: field("cols"), rows: field("rows"), width: field("width"), height: field("height") };
                let sets = self.sizes.borrow_mut().asked(conn, self.is_here(conn), &terminal, size);
                match sets {
                    true => self.drive(&terminal, Some((conn, size))),
                    false => self.terminals.tell_grid(conn, &terminal),
                }
            }
            "update" => self.updated(&pane, &args),
            "settings" => {
                if self.target() == Some(conn) {
                    self.terminals.set_settings(Settings::from_value(&args));
                }
            }
            other => log!("kanna-daemon: the app sent an unknown message \"{other}\""),
        }
        None
    }

    fn with_app(&self, conn: u64, change: impl FnOnce(&Connection) -> bool) -> bool {
        self.apps.borrow().get(&conn).is_some_and(change)
    }

    fn answer_subscribe(&self, conn: u64, message: &Value, reply: Result<Map, KmuxError>) {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        self.send_to(conn, &match reply {
            Ok(mut fields) => {
                fields.insert("id".into(), id);
                fields.insert("ok".into(), Value::Bool(true));
                Value::Object(fields)
            }
            Err(error) => json!({ "id": id, "ok": false, "error": { "code": error.code, "message": error.message } }),
        });
    }

    pub fn answer_app(&self, conn: u64, reply: Value) {
        self.send_to(conn, &reply);
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
        self.to_viewers("diff", views, payload);
    }

    pub fn log(&self, views: &[String], payload: &Value) {
        self.to_viewers("log", views, payload);
    }

    pub fn file(&self, cmd: &str, views: &[String], payload: &Value) {
        self.to_viewers(cmd, views, payload);
    }

    fn to_viewers(&self, cmd: &str, views: &[String], payload: &Value) {
        for conn in self.conns() {
            let prefix = view_prefix(conn);
            if views.iter().any(|view| view.starts_with(&prefix)) {
                self.notify_to(conn, cmd, payload.clone());
            }
        }
    }

    pub fn serial(&self, views: &[String], payload: &Value) {
        self.to_viewers("serial", views, payload);
    }

    pub fn layout(&self, view: &Value) {
        self.notify("layout", view.clone());
    }

    pub fn changed(&self, pane: &Pane) {
        self.notify("pane", json!({ "pane": Value::Object(pane_record(pane)) }));
    }

    pub fn hide(&self, pane: &str) {
        self.notify("stop", json!({ "pane": pane }));
    }

    fn attach(&self, conn: u64, pane: &str, from: Option<u64>) {
        let Some(history) = self.apps.borrow().get(&conn).map(|app| *app.attached.borrow_mut().entry(pane.into()).or_default()) else { return };
        let shows = self.shows(conn, pane) && self.apps.borrow().get(&conn).is_some_and(|app| !app.cut.get());
        let Some(attach) = self.terminals.attach(conn, pane, shows, history, from) else { return };
        if let Err(error) = attach.snapshot {
            let retry = Instant::now() + SNAPSHOT_RETRY;
            if self.with_app(conn, |app| app.unsent.borrow_mut().insert(pane.into(), retry).is_none()) {
                log!("kanna-daemon: can't snapshot {pane} yet ({error}); trying again shortly");
            }
            return;
        }
        self.with_app(conn, |app| app.unsent.borrow_mut().remove(pane).is_some());
        if let Some(code) = attach.exit_code {
            self.exited_to(conn, pane, code);
        }
    }

    fn exited_to(&self, conn: u64, pane: &str, code: i32) {
        if self.shows(conn, pane) && !self.with_app(conn, |app| app.unsent.borrow().contains_key(pane)) {
            self.terminals.after_output(conn, pane, &json!({ "cmd": "exited", "args": { "terminal": pane, "code": code } }));
        }
    }

    pub fn exited(&self, pane: &str, code: i32) {
        for conn in self.conns() {
            self.exited_to(conn, pane, code);
        }
    }

    pub fn started(&self, terminal: &str) {
        let Some(pane) = self.terminals.pane_of(terminal).and_then(|pane| self.core.upgrade()?.pane(&pane)) else { return };
        self.notify("pane", json!({ "pane": Value::Object(pane_record(&pane)) }));
        if let Some(cwd) = &pane.cwd {
            self.folder(&pane.id, cwd);
        }
        for conn in self.conns() {
            if self.shows(conn, terminal) {
                self.attach(conn, terminal, None);
            }
        }
    }

    pub fn forward(&self, cmd: &str, mut args: Value, responder: Responder) {
        if !self.answers() {
            return responder.reply(Err(no_app(cmd)));
        }
        if let (Some(reference), Some(core)) = (string(&args, "pane").filter(|p| !p.contains('@')).map(String::from), self.core.upgrade()) {
            match core.pane(&reference) {
                Some(pane) => args["pane"] = Value::String(pane.id),
                None => return responder.reply(Err(KmuxError::new("not_found", format!("no pane \"{reference}\"")))),
            }
        }
        self.ask(cmd, args, responder);
    }

    fn ask(&self, cmd: &str, args: Value, responder: Responder) {
        self.ask_within(cmd, args, self.timeout, responder);
    }

    pub fn ask_within(&self, cmd: &str, args: Value, within: Duration, responder: Responder) {
        let Some(conn) = self.target() else { return responder.reply(Err(no_app(cmd))) };
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        self.pending.borrow_mut().insert(id, (Instant::now() + within, conn, responder));
        self.send_to(conn, &json!({ "id": id, "cmd": cmd, "args": args }));
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        let replies = self.pending.borrow().values().map(|(deadline, _, _)| *deadline).min();
        let snapshots = self.apps.borrow().values().filter_map(|app| app.unsent.borrow().values().copied().min()).min();
        [replies, snapshots].into_iter().flatten().min()
    }

    pub fn expire(&self, now: Instant) {
        for conn in self.conns() {
            let due = self.apps.borrow().get(&conn).map_or(vec![], |app| {
                let mut unsent = app.unsent.borrow_mut();
                let due: Vec<String> = unsent.iter().filter(|(_, retry)| **retry <= now).map(|(pane, _)| pane.clone()).collect();
                due.iter().for_each(|pane| {
                    unsent.remove(pane);
                });
                due
            });
            for pane in due {
                if self.with_app(conn, |app| app.attached.borrow().contains_key(&pane)) {
                    self.attach(conn, &pane, None);
                }
            }
        }
        let late: Vec<u64> = self.pending.borrow().iter().filter(|(_, (deadline, _, _))| *deadline <= now).map(|(id, _)| *id).collect();
        for id in late {
            let Some((_, _, responder)) = self.pending.borrow_mut().remove(&id) else { continue };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn order(out: &Outbox) -> Vec<String> {
        let mut seen = vec![];
        loop {
            let waiting = {
                let lanes = out.queue.lanes.lock().unwrap();
                lanes.urgent.len() + lanes.bulk.len()
            };
            if waiting == 0 {
                return seen;
            }
            let line = out.next().unwrap();
            out.written(&line);
            let message: Value = serde_json::from_slice(&line.bytes).unwrap();
            seen.push(format!("{} {}", message["cmd"].as_str().unwrap(), message["args"]["terminal"].as_str().unwrap_or("-")));
        }
    }

    #[test]
    fn an_echo_goes_ahead_of_other_terminals_output_but_never_its_own() {
        let out = Outbox::default();
        let typed = Some(Instant::now());
        out.push_for(Some("term-1"), &json!({ "cmd": "output", "args": { "terminal": "term-1" } }), None);
        out.push(&json!({ "cmd": "layout", "args": {} }));
        out.push_for(Some("term-2"), &json!({ "cmd": "output", "args": { "terminal": "term-2" } }), typed);
        out.push_for(Some("term-3"), &json!({ "cmd": "snapshot", "args": { "terminal": "term-3" } }), None);
        out.push_for(Some("term-3"), &json!({ "cmd": "output", "args": { "terminal": "term-3" } }), typed);
        assert_eq!(order(&out), ["output term-2", "output term-1", "layout -", "snapshot term-3", "output term-3"]);
        assert_eq!(out.queue.bytes.load(Ordering::SeqCst), 0);
        assert!(out.queue.lanes.lock().unwrap().queued.is_empty());
    }
}
