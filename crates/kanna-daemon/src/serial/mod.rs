pub mod device;
pub mod lines;
pub mod usb;

use crate::Event;
use device::{Device, Lines, Read};
use kanna_mux::json::{string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Pane, PaneState, PaneType, Responder, WeakCore};
use lines::{Dir, Format, Row, Splitter};
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use usb::Found;

pub const KEEP_ROWS: usize = 10_000;
const LOG_LIMIT: u64 = 64 << 20;
const HISTORY: usize = 200;
const DEFAULT_BAUD: u32 = 115_200;
const RETRIES: u32 = 10;
const RETRY: Duration = Duration::from_millis(300);
const MAX_WAIT: Duration = Duration::from_secs(30);

pub enum SerialEvent {
    Read { key: String, generation: u64, read: Read },
    Hotplug,
    Retry { key: String },
    Timeout { token: u64 },
    ResetDone { token: u64, result: Result<(), String> },
    LeaseOver { lease: u64 },
}

#[derive(Clone, PartialEq, Debug)]
enum State {
    Connected,
    Unplugged(String),
    Lent { cmd: String, pid: i64 },
}

struct Port {
    key: String,
    path: String,
    found: Option<Found>,
    baud: u32,
    ending: String,
    lines: Lines,
    device: Option<Device>,
    generation: u64,
    state: State,
    since: i64,
    splitter: Splitter,
    rows: VecDeque<Row>,
    rx: u64,
    formats: [u64; 3],
    log: Log,
    panes: BTreeSet<String>,
    history: Vec<String>,
    lease: Option<u64>,
    lenders: VecDeque<(Value, Responder)>,
    retries: u32,
}

struct Waiter {
    token: u64,
    key: String,
    after: u64,
    responder: Responder,
}

pub struct Serial {
    events: Sender<Event>,
    core: RefCell<Option<WeakCore>>,
    folder: PathBuf,
    ports: RefCell<HashMap<String, Port>>,
    panes: RefCell<HashMap<String, String>>,
    views: RefCell<HashMap<String, String>>,
    waiters: RefCell<Vec<Waiter>>,
    resets: RefCell<HashMap<u64, Responder>>,
    sent: RefCell<HashMap<String, u64>>,
    seq: Cell<u64>,
    token: Cell<u64>,
    pub on_serial: RefCell<Option<Box<dyn Fn(&[String], &Value)>>>,
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub fn local_time(ms: i64) -> String {
    let secs = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, ms.rem_euclid(1000))
}

fn failed(code: &str, message: impl Into<String>) -> KmuxError {
    KmuxError::new(code, message.into())
}

struct Log {
    path: PathBuf,
    file: Option<std::fs::File>,
    size: u64,
}

impl Log {
    fn open(path: PathBuf) -> Log {
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
        let size = file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
        Log { path, file, size }
    }

    fn write(&mut self, row: &Row) {
        let line = format!("{} {} {}\n", local_time(row.t), row.dir.mark(), row.text);
        if self.size + line.len() as u64 > LOG_LIMIT {
            self.file = None;
            let _ = std::fs::rename(&self.path, self.path.with_extension("1.log"));
            *self = Log::open(self.path.clone());
        }
        if let Some(file) = self.file.as_mut() {
            if file.write_all(line.as_bytes()).is_ok() {
                self.size += line.len() as u64;
            }
        }
    }
}

impl Port {
    fn state_json(&self) -> Value {
        let (state, cmd, why) = match &self.state {
            State::Connected => ("connected", None, None),
            State::Unplugged(why) => ("unplugged", None, Some(why.clone())),
            State::Lent { cmd, .. } => ("lent", Some(cmd.clone()), None),
        };
        let format = Format::ALL.iter().zip(self.formats).filter(|(_, n)| *n > 0).max_by_key(|(_, n)| *n).map(|(f, _)| f.name());
        json!({
            "state": state,
            "cmd": cmd,
            "why": why,
            "since": self.since,
            "path": self.path,
            "name": self.key,
            "title": self.title(),
            "device": self.found.as_ref().and_then(Found::identity),
            "usb": self.found.as_ref().and_then(Found::usb),
            "product": self.found.as_ref().and_then(|f| f.product.clone()),
            "vendor": self.found.as_ref().and_then(|f| f.vendor.clone()),
            "baud": self.baud,
            "lineEnding": self.ending,
            "rx": self.rx,
            "format": format,
            "log": self.log.path.to_string_lossy(),
            "panes": self.panes.iter().collect::<Vec<_>>(),
        })
    }

    fn title(&self) -> String {
        match self.found.as_ref().and_then(|f| f.product.clone()) {
            Some(product) => format!("{product} · {}", self.key),
            None => self.key.clone(),
        }
    }

    fn partial(&self) -> Value {
        match self.splitter.partial() {
            Some(line) => json!({ "t": line.t, "text": lines::escape(&line.bytes) }),
            None => Value::Null,
        }
    }

    fn ending_bytes(&self) -> &'static [u8] {
        match self.ending.as_str() {
            "crlf" => b"\r\n",
            "cr" => b"\r",
            _ => b"\n",
        }
    }
}

impl Serial {
    pub fn new(events: Sender<Event>, state_dir: &Path) -> Serial {
        Serial {
            events,
            core: RefCell::new(None),
            folder: state_dir.join("serial"),
            ports: RefCell::new(HashMap::new()),
            panes: RefCell::new(HashMap::new()),
            views: RefCell::new(HashMap::new()),
            waiters: RefCell::new(Vec::new()),
            resets: RefCell::new(HashMap::new()),
            sent: RefCell::new(HashMap::new()),
            seq: Cell::new(0),
            token: Cell::new(0),
            on_serial: RefCell::new(None),
        }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        *self.core.borrow_mut() = Some(core.weak());
        let serial = Rc::downgrade(self);
        core.add_command(
            "serial",
            Rc::new(move |args, responder| match serial.upgrade() {
                Some(serial) => serial.command(args, responder),
                None => responder.reply(Err(failed("app_gone", "the daemon is stopping"))),
            }),
        );
        let events = self.events.clone();
        usb::watch(move || {
            let _ = events.send(Event::Serial(SerialEvent::Hotplug));
        });
    }

    fn core(&self) -> Option<CoreHandle> {
        self.core.borrow().as_ref().and_then(WeakCore::upgrade)
    }

    fn next_token(&self) -> u64 {
        self.token.set(self.token.get() + 1);
        self.token.get()
    }

    fn next_seq(&self) -> u64 {
        self.seq.set(self.seq.get() + 1);
        self.seq.get()
    }

    pub fn restore(&self, panes: Vec<Pane>) {
        for pane in panes.into_iter().filter(|p| p.kind == PaneType::Serial) {
            self.start(&pane);
        }
    }

    pub fn start(&self, pane: &Pane) {
        let wanted = pane.path.clone().unwrap_or_default();
        let found = usb::list().into_iter().find(|f| f.matches(&wanted) || pane.device.as_deref().is_some_and(|d| f.identity().as_deref() == Some(d)));
        let path = match &found {
            Some(found) => found.path.clone(),
            None if wanted.starts_with('/') && Path::new(&wanted).exists() => wanted.clone(),
            None if pane.device.is_some() && wanted.starts_with('/') => wanted.clone(),
            None => {
                return self.report(&pane.id, PaneState::Failed, Some(format!("no serial port \"{wanted}\" is plugged in: `kanna serial list` shows the ports")));
            }
        };
        let key = found.as_ref().map(Found::name).unwrap_or_else(|| usb::name_of(&path));
        if let Some(old) = self.panes.borrow_mut().insert(pane.id.clone(), key.clone()).filter(|old| *old != key) {
            self.leave(&pane.id, &old);
        }
        let baud = pane.baud.unwrap_or(DEFAULT_BAUD);
        let ending = pane.line_ending.clone().unwrap_or_else(|| "lf".into());
        let fresh = !self.ports.borrow().contains_key(&key);
        if fresh {
            let lines = if found.as_ref().is_some_and(|f| f.vid.is_some() && !f.uart_bridge()) { Lines::DtrOnly } else { Lines::BothClear };
            let port = Port {
                key: key.clone(),
                path: path.clone(),
                found: found.clone(),
                baud,
                ending,
                lines,
                device: None,
                generation: 0,
                state: State::Unplugged("not opened yet".into()),
                since: now_ms(),
                splitter: Splitter::default(),
                rows: VecDeque::new(),
                rx: 0,
                formats: [0; 3],
                log: Log::open(self.folder.join(format!("{key}.log"))),
                panes: BTreeSet::new(),
                history: self.read_history(&key),
                lease: None,
                lenders: VecDeque::new(),
                retries: 0,
            };
            self.ports.borrow_mut().insert(key.clone(), port);
        }
        if let Some(port) = self.ports.borrow_mut().get_mut(&key) {
            port.panes.insert(pane.id.clone());
            if found.is_some() {
                port.found = found.clone();
            }
        }
        if fresh {
            if let Err(error) = self.connect(&key) {
                if found.is_none() && pane.device.is_none() {
                    self.ports.borrow_mut().remove(&key);
                    self.panes.borrow_mut().remove(&pane.id);
                    return self.report(&pane.id, PaneState::Failed, Some(format!("can't open {path}: {error}")));
                }
                self.unplugged(&key, error);
            }
        }
        let (title, device, path) = {
            let ports = self.ports.borrow();
            let port = &ports[&key];
            (port.title(), port.found.as_ref().and_then(Found::identity), port.path.clone())
        };
        let (id, core) = (pane.id.clone(), self.core());
        if let Some(core) = core {
            let editor = core.clone();
            core.later(move || {
                editor.edit_panes(|p| {
                    if p.id != id {
                        return false;
                    }
                    let before = (p.title.clone(), p.device.clone(), p.path.clone());
                    p.title = Some(title.clone());
                    if device.is_some() {
                        p.device = device.clone();
                    }
                    p.path = Some(path.clone());
                    before != (p.title.clone(), p.device.clone(), p.path.clone())
                });
                editor.update(&id, PaneState::Running, None, None);
            });
        }
        self.push_state(&key);
    }

    pub fn stop(&self, pane: &Pane) {
        let Some(key) = self.panes.borrow_mut().remove(&pane.id) else { return };
        self.leave(&pane.id, &key);
    }

    fn leave(&self, pane: &str, key: &str) {
        let empty = {
            let mut ports = self.ports.borrow_mut();
            let Some(port) = ports.get_mut(key) else { return };
            port.panes.remove(pane);
            port.panes.is_empty()
        };
        if !empty {
            return self.push_state(key);
        }
        let port = self.ports.borrow_mut().remove(key);
        if let Some(mut port) = port {
            if let Some(device) = port.device.take() {
                device.close();
            }
            self.flush_partial(&mut port);
            for (_, responder) in port.lenders.drain(..) {
                responder.reply(Ok(Map::from_iter([("lent".to_string(), json!(false))])));
            }
        }
        let gone: Vec<Waiter> = {
            let mut waiters = self.waiters.borrow_mut();
            let (gone, kept) = std::mem::take(&mut *waiters).into_iter().partition(|w| w.key == key);
            *waiters = kept;
            gone
        };
        for waiter in gone {
            waiter.responder.reply(Err(failed("not_found", "the serial pane closed")));
        }
    }

    fn report(&self, id: &str, state: PaneState, error: Option<String>) {
        let Some(core) = self.core() else { return };
        let (id, reporter) = (id.to_string(), core.clone());
        core.later(move || reporter.update(&id, state, None, error));
    }

    fn connect(&self, key: &str) -> Result<(), String> {
        let (path, baud, lines, generation) = {
            let mut ports = self.ports.borrow_mut();
            let port = ports.get_mut(key).ok_or("no such port")?;
            if let Some(device) = port.device.take() {
                device.close();
            }
            port.generation += 1;
            (port.path.clone(), port.baud, port.lines, port.generation)
        };
        let events = self.events.clone();
        let reader_key = key.to_string();
        let device = Device::open(&path, baud, lines, move |read| events.send(Event::Serial(SerialEvent::Read { key: reader_key.clone(), generation, read })).is_ok()).map_err(|e| e.to_string())?;
        let was = {
            let mut ports = self.ports.borrow_mut();
            let port = ports.get_mut(key).ok_or("no such port")?;
            port.device = Some(device);
            port.splitter.reset();
            port.retries = 0;
            let was = port.state.clone();
            port.state = State::Connected;
            port.since = now_ms();
            was
        };
        let text = match was {
            State::Unplugged(why) if why != "not opened yet" => format!("back {path}"),
            _ => format!("connected {path} at {baud}"),
        };
        self.event(key, text);
        Ok(())
    }

    fn unplugged(&self, key: &str, why: String) {
        {
            let mut ports = self.ports.borrow_mut();
            let Some(port) = ports.get_mut(key) else { return };
            if let Some(device) = port.device.take() {
                device.close();
            }
            if matches!(port.state, State::Unplugged(_)) {
                port.state = State::Unplugged(why);
                return;
            }
            port.state = State::Unplugged(why.clone());
            port.since = now_ms();
            let mut port_partial = std::mem::take(&mut port.splitter);
            if let Some(line) = port_partial.take_partial() {
                drop(ports);
                self.row(key, Dir::Rx, line.t, lines::escape(&line.bytes));
            }
        }
        self.event(key, format!("unplugged · {why}"));
    }

    pub fn event_in(&self, event: SerialEvent) {
        match event {
            SerialEvent::Read { key, generation, read } => self.read(&key, generation, read),
            SerialEvent::Hotplug => self.hotplug(),
            SerialEvent::Retry { key } => self.retry(&key),
            SerialEvent::Timeout { token } => self.timeout(token),
            SerialEvent::ResetDone { token, result } => {
                if let Some(responder) = self.resets.borrow_mut().remove(&token) {
                    responder.reply(result.map(|()| Map::from_iter([("at".to_string(), json!(now_ms()))])).map_err(|e| failed("failed", e)));
                }
            }
            SerialEvent::LeaseOver { lease } => self.give_back(lease),
        }
    }

    fn read(&self, key: &str, generation: u64, read: Read) {
        let current = self.ports.borrow().get(key).is_some_and(|p| p.generation == generation && p.state == State::Connected);
        if !current {
            return;
        }
        match read {
            Read::Bytes(data) => {
                let t = now_ms();
                let lines = {
                    let mut ports = self.ports.borrow_mut();
                    let port = ports.get_mut(key).unwrap();
                    port.rx += data.len() as u64;
                    port.splitter.feed(t, &data)
                };
                for line in lines {
                    self.row(key, Dir::Rx, line.t, lines::escape(&line.bytes));
                }
                self.push(key);
            }
            Read::Idle => {
                let stale = self.ports.borrow().get(key).and_then(|p| p.splitter.partial().map(|l| l.t)).is_some_and(|t| now_ms() - t >= device::IDLE.as_millis() as i64);
                if stale {
                    let line = self.ports.borrow_mut().get_mut(key).and_then(|p| p.splitter.take_partial());
                    if let Some(line) = line {
                        self.row(key, Dir::Rx, line.t, lines::escape(&line.bytes));
                        self.push(key);
                    }
                }
            }
            Read::Gone(why) => {
                self.unplugged(key, why);
                self.push_state(key);
            }
        }
    }

    fn flush_partial(&self, port: &mut Port) {
        if let Some(line) = port.splitter.take_partial() {
            let row = Row { seq: self.next_seq(), t: line.t, dir: Dir::Rx, text: lines::escape(&line.bytes), level: None };
            port.log.write(&row);
        }
    }

    fn row(&self, key: &str, dir: Dir, t: i64, text: String) {
        let seq = self.next_seq();
        let mut ports = self.ports.borrow_mut();
        let Some(port) = ports.get_mut(key) else { return };
        let (level, format) = if dir == Dir::Rx { lines::level(&text) } else { (None, None) };
        if let Some(format) = format {
            port.formats[Format::ALL.iter().position(|f| *f == format).unwrap_or(0)] += 1;
        }
        let row = Row { seq, t, dir, text, level };
        port.log.write(&row);
        port.rows.push_back(row);
        while port.rows.len() > KEEP_ROWS {
            port.rows.pop_front();
        }
    }

    fn event(&self, key: &str, text: String) {
        self.row(key, Dir::Event, now_ms(), text);
        self.push(key);
    }

    fn views_of(&self, key: &str) -> Vec<String> {
        self.views.borrow().iter().filter(|(_, k)| *k == key).map(|(v, _)| v.clone()).collect()
    }

    fn push(&self, key: &str) {
        self.wake(key);
        let views = self.views_of(key);
        if views.is_empty() {
            return;
        }
        let payload = {
            let ports = self.ports.borrow();
            let Some(port) = ports.get(key) else { return };
            json!({ "content": key, "partial": port.partial(), "rx": port.rx, "state": port.state_json() })
        };
        let mut payload = payload;
        payload["rows"] = Value::Array(self.pending_rows(key));
        if let Some(on_serial) = self.on_serial.borrow().as_ref() {
            on_serial(&views, &payload);
        }
    }

    fn pending_rows(&self, key: &str) -> Vec<Value> {
        let ports = self.ports.borrow();
        let Some(port) = ports.get(key) else { return vec![] };
        let mut sent = self.sent.borrow_mut();
        let after = sent.get(key).copied().unwrap_or(0);
        let rows: Vec<Value> = port.rows.iter().filter(|r| r.seq > after).map(Row::to_json).collect();
        if let Some(last) = port.rows.back() {
            sent.insert(key.to_string(), last.seq);
        }
        rows
    }

    fn push_state(&self, key: &str) {
        let views = self.views_of(key);
        if views.is_empty() {
            return;
        }
        let payload = match self.ports.borrow().get(key) {
            Some(port) => json!({ "content": key, "state": port.state_json(), "rows": [], "partial": port.partial(), "rx": port.rx }),
            None => return,
        };
        if let Some(on_serial) = self.on_serial.borrow().as_ref() {
            on_serial(&views, &payload);
        }
    }

    pub fn subscribe(&self, view: &str, args: &Value) -> Result<Map, KmuxError> {
        let pane = string(args, "pane").unwrap_or_default();
        let key = self.panes.borrow().get(pane).cloned().ok_or_else(|| failed("not_found", format!("{pane} isn't open on a serial port")))?;
        self.views.borrow_mut().insert(view.to_string(), key.clone());
        let ports = self.ports.borrow();
        let port = ports.get(&key).ok_or_else(|| failed("not_found", "the port closed"))?;
        let mut out = Map::new();
        out.insert("content".into(), json!(key));
        out.insert("rows".into(), Value::Array(port.rows.iter().map(Row::to_json).collect()));
        out.insert("partial".into(), port.partial());
        out.insert("state".into(), port.state_json());
        out.insert("history".into(), json!(port.history));
        out.insert("rx".into(), json!(port.rx));
        drop(ports);
        self.mark_sent(&key);
        Ok(out)
    }

    fn mark_sent(&self, key: &str) {
        let _ = self.pending_rows(key);
    }

    pub fn unsubscribe(&self, view: &str) {
        self.views.borrow_mut().remove(view);
    }

    pub fn leave_views(&self, gone: impl Fn(&str) -> bool) {
        self.views.borrow_mut().retain(|view, _| !gone(view));
    }

    fn hotplug(&self) {
        let found = usb::list();
        let waiting: Vec<String> = self.ports.borrow().values().filter(|p| matches!(p.state, State::Unplugged(_))).map(|p| p.key.clone()).collect();
        for key in waiting {
            let target = {
                let ports = self.ports.borrow();
                let port = &ports[&key];
                let identity = port.found.as_ref().and_then(Found::identity);
                found.iter().find(|f| match &identity {
                    Some(identity) => f.identity().as_deref() == Some(identity.as_str()),
                    None => f.path == port.path,
                }).cloned()
            };
            let Some(target) = target else { continue };
            if let Some(port) = self.ports.borrow_mut().get_mut(&key) {
                port.path = target.path.clone();
                port.found = Some(target);
            }
            self.try_connect(&key);
        }
        let mut gone = Vec::new();
        for port in self.ports.borrow().values().filter(|p| p.state == State::Connected) {
            if port.found.is_some() && !found.iter().any(|f| f.path == port.path) && usb::fake_file().is_none() {
                gone.push(port.key.clone());
            }
            if usb::fake_file().is_some() && !found.iter().any(|f| f.path == port.path) {
                gone.push(port.key.clone());
            }
        }
        for key in gone {
            self.unplugged(&key, "the device went away".into());
            self.push_state(&key);
        }
    }

    fn try_connect(&self, key: &str) {
        match self.connect(key) {
            Ok(()) => {
                self.push_state(key);
                self.retitle(key);
            }
            Err(error) => {
                let again = {
                    let mut ports = self.ports.borrow_mut();
                    let Some(port) = ports.get_mut(key) else { return };
                    port.state = State::Unplugged(error);
                    port.retries += 1;
                    port.retries <= RETRIES
                };
                if again {
                    let (events, key) = (self.events.clone(), key.to_string());
                    let _ = std::thread::Builder::new().name("kanna.serial.retry".into()).spawn(move || {
                        std::thread::sleep(RETRY);
                        let _ = events.send(Event::Serial(SerialEvent::Retry { key }));
                    });
                }
            }
        }
    }

    fn retry(&self, key: &str) {
        let waiting = self.ports.borrow().get(key).is_some_and(|p| matches!(p.state, State::Unplugged(_)));
        if waiting {
            self.try_connect(key);
        }
    }

    fn retitle(&self, key: &str) {
        let Some(core) = self.core() else { return };
        let (panes, title, path, device) = {
            let ports = self.ports.borrow();
            let Some(port) = ports.get(key) else { return };
            (port.panes.clone(), port.title(), port.path.clone(), port.found.as_ref().and_then(Found::identity))
        };
        let editor = core.clone();
        core.later(move || {
            editor.edit_panes(|p| {
                if !panes.contains(&p.id) {
                    return false;
                }
                let before = (p.title.clone(), p.path.clone(), p.device.clone());
                p.title = Some(title.clone());
                p.path = Some(path.clone());
                if device.is_some() {
                    p.device = device.clone();
                }
                before != (p.title.clone(), p.path.clone(), p.device.clone())
            });
        });
    }

    fn key_of(&self, args: &Value) -> Result<String, KmuxError> {
        let reference = string(args, "pane").ok_or_else(|| failed("bad_request", "say which serial pane"))?;
        let core = self.core().ok_or_else(|| failed("app_gone", "the daemon is stopping"))?;
        let pane = core.pane(reference).ok_or_else(|| failed("not_found", format!("no pane \"{reference}\"")))?;
        if pane.kind != PaneType::Serial {
            return Err(failed("wrong_type", format!("{} is a {} pane, not a serial pane", pane.label(), pane.kind.as_str())));
        }
        self.panes.borrow().get(&pane.id).cloned().ok_or_else(|| failed("failed", format!("{} has no port open", pane.label())))
    }

    fn command(&self, args: Value, responder: Responder) {
        let action = string(&args, "action").unwrap_or("list").to_string();
        let result = match action.as_str() {
            "list" => Ok(self.list()),
            "read" => return self.read_rows(args, responder),
            "send" => self.send(&args),
            "reset" => return self.reset(args, responder),
            "set" => self.set(&args),
            "state" => self.key_of(&args).map(|key| self.with_port(&key, |p| Map::from_iter([("port".to_string(), p.state_json())]))),
            "lend" => return self.lend(args, responder),
            "return" => self.return_lease(&args),
            other => Err(failed("bad_request", format!("unknown serial action \"{other}\" (list, read, send, reset, set, state, lend or return)"))),
        };
        responder.reply(result);
    }

    fn with_port<R>(&self, key: &str, f: impl FnOnce(&Port) -> R) -> R {
        f(&self.ports.borrow()[key])
    }

    fn list(&self) -> Map {
        let ports = self.ports.borrow();
        let mut seen = BTreeSet::new();
        let mut out: Vec<Value> = usb::list()
            .into_iter()
            .map(|found| {
                let open = ports.values().find(|p| p.path == found.path);
                if let Some(port) = open {
                    seen.insert(port.key.clone());
                }
                let mut row = found.to_json();
                row["panes"] = json!(open.map(|p| p.panes.iter().collect::<Vec<_>>()).unwrap_or_default());
                row["state"] = json!(open.map(|p| p.state_json()["state"].clone()));
                row
            })
            .collect();
        for port in ports.values().filter(|p| !seen.contains(&p.key)) {
            let mut row = port.found.clone().unwrap_or(Found { path: port.path.clone(), ..Found::default() }).to_json();
            row["panes"] = json!(port.panes.iter().collect::<Vec<_>>());
            row["state"] = port.state_json()["state"].clone();
            out.push(row);
        }
        Map::from_iter([("ports".to_string(), Value::Array(out))])
    }

    fn select(port: &Port, args: &Value) -> Vec<Value> {
        let after = args.get("after").and_then(Value::as_u64);
        let since = args.get("since").and_then(Value::as_i64);
        let mut rows: Vec<&Row> = port.rows.iter().filter(|r| after.is_none_or(|a| r.seq > a) && since.is_none_or(|s| r.t >= s)).collect();
        if let Some(last) = args.get("last").and_then(Value::as_u64) {
            let skip = rows.len().saturating_sub(last as usize);
            rows.drain(..skip);
        }
        rows.into_iter().map(Row::to_json).collect()
    }

    fn read_reply(&self, key: &str, args: &Value) -> Map {
        let ports = self.ports.borrow();
        let port = &ports[key];
        let rows = Self::select(port, args);
        let next = port.rows.back().map(|r| r.seq).unwrap_or(0).max(args.get("after").and_then(Value::as_u64).unwrap_or(0));
        Map::from_iter([("rows".to_string(), Value::Array(rows)), ("next".to_string(), json!(next)), ("port".to_string(), port.state_json()), ("partial".to_string(), port.partial())])
    }

    fn read_rows(&self, args: Value, responder: Responder) {
        let key = match self.key_of(&args) {
            Ok(key) => key,
            Err(error) => return responder.reply(Err(error)),
        };
        let wait = args.get("wait").and_then(Value::as_u64).map(Duration::from_millis).unwrap_or_default().min(MAX_WAIT);
        let after = args.get("after").and_then(Value::as_u64);
        let has = self.ports.borrow().get(&key).is_some_and(|p| !Self::select(p, &args).is_empty());
        if has || wait.is_zero() || after.is_none() {
            return responder.reply(Ok(self.read_reply(&key, &args)));
        }
        let token = self.next_token();
        self.waiters.borrow_mut().push(Waiter { token, key, after: after.unwrap_or(0), responder });
        let events = self.events.clone();
        let _ = std::thread::Builder::new().name("kanna.serial.wait".into()).spawn(move || {
            std::thread::sleep(wait);
            let _ = events.send(Event::Serial(SerialEvent::Timeout { token }));
        });
    }

    fn wake(&self, key: &str) {
        let ready: Vec<Waiter> = {
            let mut waiters = self.waiters.borrow_mut();
            let (ready, kept) = std::mem::take(&mut *waiters).into_iter().partition(|w| w.key == key);
            *waiters = kept;
            ready
        };
        for waiter in ready {
            let args = json!({ "after": waiter.after });
            waiter.responder.reply(Ok(self.read_reply(&waiter.key, &args)));
        }
    }

    fn timeout(&self, token: u64) {
        let found = {
            let mut waiters = self.waiters.borrow_mut();
            waiters.iter().position(|w| w.token == token).map(|i| waiters.remove(i))
        };
        if let Some(waiter) = found {
            let args = json!({ "after": waiter.after });
            let reply = if self.ports.borrow().contains_key(&waiter.key) { Ok(self.read_reply(&waiter.key, &args)) } else { Err(failed("not_found", "the serial pane closed")) };
            waiter.responder.reply(reply);
        }
    }

    fn connected(&self, key: &str) -> Result<(), KmuxError> {
        match &self.ports.borrow()[key].state {
            State::Connected => Ok(()),
            State::Unplugged(_) => Err(failed("unplugged", format!("{key} is unplugged"))),
            State::Lent { cmd, .. } => Err(failed("lent", format!("{key} is lent to {cmd}"))),
        }
    }

    fn send(&self, args: &Value) -> Result<Map, KmuxError> {
        let key = self.key_of(args)?;
        self.connected(&key)?;
        let ending = args.get("ending").and_then(Value::as_bool).unwrap_or(true);
        let (data, shown) = match (string(args, "hex"), string(args, "text")) {
            (Some(hex), _) => {
                let digits: String = hex.chars().filter(|c| !c.is_whitespace() && *c != ',' && *c != ':').collect::<String>().trim_start_matches("0x").to_string();
                if !digits.len().is_multiple_of(2) || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(failed("bad_request", "hex must be pairs of hex digits, like 1b 5b 41"));
                }
                let bytes: Vec<u8> = (0..digits.len()).step_by(2).map(|i| u8::from_str_radix(&digits[i..i + 2], 16).unwrap()).collect();
                (bytes.clone(), lines::escape(&bytes))
            }
            (None, Some(text)) => (text.as_bytes().to_vec(), text.to_string()),
            (None, None) => return Err(failed("bad_request", "send needs text or hex")),
        };
        let mut data = data;
        {
            let ports = self.ports.borrow();
            let port = &ports[&key];
            if ending && string(args, "hex").is_none() {
                data.extend_from_slice(port.ending_bytes());
            }
            port.device.as_ref().ok_or_else(|| failed("unplugged", format!("{key} is unplugged")))?.write(&data).map_err(|e| failed("failed", format!("can't write to {key}: {e}")))?;
        }
        let t = now_ms();
        self.row(&key, Dir::Tx, t, shown.clone());
        if string(args, "text").is_some() && string(args, "hex").is_none() && !shown.is_empty() {
            self.remember(&key, &shown);
        }
        self.push(&key);
        Ok(Map::from_iter([("at".to_string(), json!(t))]))
    }

    fn remember(&self, key: &str, line: &str) {
        let history = {
            let mut ports = self.ports.borrow_mut();
            let Some(port) = ports.get_mut(key) else { return };
            port.history.retain(|h| h != line);
            port.history.push(line.to_string());
            let skip = port.history.len().saturating_sub(HISTORY);
            port.history.drain(..skip);
            port.history.clone()
        };
        let _ = std::fs::create_dir_all(&self.folder);
        let _ = std::fs::write(self.folder.join(format!("{key}.history")), history.join("\n") + "\n");
    }

    fn read_history(&self, key: &str) -> Vec<String> {
        std::fs::read_to_string(self.folder.join(format!("{key}.history"))).map(|t| t.lines().filter(|l| !l.is_empty()).map(String::from).collect()).unwrap_or_default()
    }

    fn reset(&self, args: Value, responder: Responder) {
        let key = match self.key_of(&args).and_then(|key| self.connected(&key).map(|()| key)) {
            Ok(key) => key,
            Err(error) => return responder.reply(Err(error)),
        };
        let hold = Duration::from_millis(args.get("hold").and_then(Value::as_u64).unwrap_or(100).clamp(1, 5000));
        let (fd, lines) = {
            let ports = self.ports.borrow();
            let port = &ports[&key];
            (port.device.as_ref().map(Device::share), port.lines)
        };
        let Some(fd) = fd else { return responder.reply(Err(failed("unplugged", format!("{key} is unplugged")))) };
        self.event(&key, "reset · DTR/RTS".into());
        let token = self.next_token();
        self.resets.borrow_mut().insert(token, responder);
        let events = self.events.clone();
        let _ = std::thread::Builder::new().name("kanna.serial.reset".into()).spawn(move || {
            use std::os::fd::AsRawFd;
            let result = device::pulse_reset(fd.as_raw_fd(), hold, lines).map_err(|e| e.to_string());
            let _ = events.send(Event::Serial(SerialEvent::ResetDone { token, result }));
        });
    }

    fn set(&self, args: &Value) -> Result<Map, KmuxError> {
        let key = self.key_of(args)?;
        let baud = kanna_mux::read_baud(args.get("baud"))?;
        let ending = kanna_mux::read_line_ending(args.get("lineEnding"))?;
        if let Some(ending) = &ending {
            self.ports.borrow_mut().get_mut(&key).unwrap().ending = ending.clone();
        }
        if let Some(baud) = baud {
            let changed = {
                let mut ports = self.ports.borrow_mut();
                let port = ports.get_mut(&key).unwrap();
                let changed = port.baud != baud;
                port.baud = baud;
                if let (true, Some(device)) = (changed, port.device.as_ref()) {
                    device.set_baud(baud).map_err(|e| failed("failed", format!("can't set {key} to {baud}: {e}")))?;
                }
                changed
            };
            if changed {
                self.event(&key, format!("baud {baud}"));
            }
        }
        let panes = self.ports.borrow()[&key].panes.clone();
        if let Some(core) = self.core() {
            let editor = core.clone();
            core.later(move || {
                editor.edit_panes(|p| {
                    if !panes.contains(&p.id) {
                        return false;
                    }
                    let before = (p.baud, p.line_ending.clone());
                    if baud.is_some() {
                        p.baud = baud;
                    }
                    if ending.is_some() {
                        p.line_ending = ending.clone();
                    }
                    before != (p.baud, p.line_ending.clone())
                });
            });
        }
        self.push_state(&key);
        Ok(Map::from_iter([("port".to_string(), self.ports.borrow()[&key].state_json())]))
    }

    fn lend_key(&self, args: &Value) -> Result<Option<String>, KmuxError> {
        if string(args, "pane").is_some() {
            return self.key_of(args).map(Some);
        }
        let wanted = string(args, "port").ok_or_else(|| failed("bad_request", "say which pane or port to lend"))?;
        let ports = self.ports.borrow();
        let found = ports.values().find(|p| p.path == wanted || p.key == wanted || p.found.as_ref().is_some_and(|f| f.matches(wanted)) || usb::name_of(wanted) == p.key);
        Ok(found.map(|p| p.key.clone()))
    }

    fn lend(&self, args: Value, responder: Responder) {
        let key = match self.lend_key(&args) {
            Ok(Some(key)) => key,
            Ok(None) => return responder.reply(Ok(Map::from_iter([("lent".to_string(), json!(false))]))),
            Err(error) => return responder.reply(Err(error)),
        };
        let busy = self.ports.borrow().get(&key).is_some_and(|p| p.lease.is_some());
        if busy {
            self.ports.borrow_mut().get_mut(&key).unwrap().lenders.push_back((args, responder));
            return;
        }
        self.start_lease(&key, &args, responder);
    }

    fn start_lease(&self, key: &str, args: &Value, responder: Responder) {
        let lease = self.next_token();
        let cmd = string(args, "cmd").unwrap_or("a command").to_string();
        let pid = args.get("pid").and_then(Value::as_i64).unwrap_or(0);
        let path = {
            let mut ports = self.ports.borrow_mut();
            let Some(port) = ports.get_mut(key) else { return responder.reply(Err(failed("not_found", "the port closed"))) };
            if let Some(device) = port.device.take() {
                device.close();
            }
            let mut partial = std::mem::take(&mut port.splitter);
            if let Some(line) = partial.take_partial() {
                drop(ports);
                self.row(key, Dir::Rx, line.t, lines::escape(&line.bytes));
                ports = self.ports.borrow_mut();
            }
            let port = ports.get_mut(key).unwrap();
            port.generation += 1;
            port.lease = Some(lease);
            port.state = State::Lent { cmd: cmd.clone(), pid };
            port.since = now_ms();
            port.path.clone()
        };
        self.event(key, if pid > 0 { format!("lent · {cmd} (pid {pid})") } else { format!("lent · {cmd}") });
        if pid > 0 {
            let events = self.events.clone();
            let _ = std::thread::Builder::new().name("kanna.serial.lease".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_millis(250));
                if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    let _ = events.send(Event::Serial(SerialEvent::LeaseOver { lease }));
                    return;
                }
            });
        }
        responder.reply(Ok(Map::from_iter([("lent".to_string(), json!(true)), ("lease".to_string(), json!(lease)), ("path".to_string(), json!(path)), ("name".to_string(), json!(key))])));
    }

    fn return_lease(&self, args: &Value) -> Result<Map, KmuxError> {
        let lease = args.get("lease").and_then(Value::as_u64).ok_or_else(|| failed("bad_request", "return needs the lease"))?;
        self.give_back(lease);
        Ok(Map::new())
    }

    fn give_back(&self, lease: u64) {
        let key = self.ports.borrow().values().find(|p| p.lease == Some(lease)).map(|p| p.key.clone());
        let Some(key) = key else { return };
        {
            let mut ports = self.ports.borrow_mut();
            let port = ports.get_mut(&key).unwrap();
            port.lease = None;
            port.state = State::Unplugged("returned".into());
            port.retries = 0;
        }
        let next = self.ports.borrow_mut().get_mut(&key).and_then(|p| p.lenders.pop_front());
        if let Some((args, responder)) = next {
            return self.start_lease(&key, &args, responder);
        }
        match self.connect(&key) {
            Ok(()) => self.push_state(&key),
            Err(error) => {
                self.ports.borrow_mut().get_mut(&key).unwrap().state = State::Unplugged(error.clone());
                self.event(&key, format!("unplugged · {error}"));
                self.try_connect(&key);
            }
        }
    }
}
