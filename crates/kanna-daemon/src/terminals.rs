use crate::pty::{self, Adopted, Spawned, Spawner, Spec};
use crate::reader::{Feed, Reader, Taken};
use crate::settings::Settings;
use crate::status::Detector;
use crate::store::{pane_number, Record, Saved, Store};
use crate::Event;
use kanna_mux::json::Map;
use kanna_mux::{terminal_number, KmuxError, Pane, PaneHost, PaneState, PaneType, Responder, WeakCore};
use kanna_vt::{Key, KeyError};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type OnStart = Box<dyn Fn(&str)>;

pub struct Attach {
    pub cols: u16,
    pub rows: u16,
    pub snapshot: Result<Vec<u8>, kanna_vt::Error>,
    pub exit_code: Option<i32>,
}

const COLS: u16 = 80;
const ROWS: u16 = 24;

const MAX_SIDE: u16 = 1000;

const KEY_GAP: Duration = Duration::from_millis(50);

const ENTER_GAP: Duration = Duration::from_millis(100);

struct Pty {
    master: OwnedFd,
    input: Sender<Vec<u8>>,
    reader: Reader,
}

struct Terminal {
    pane: String,
    serial: u64,
    pid: i32,
    keeper: String,
    pty: Option<Pty>,
    feed: Arc<Mutex<Feed>>,
    pixels: (u16, u16),
    exit_code: Option<i32>,
    exit_reported: bool,
    drained: bool,
}

impl Terminal {
    fn running(&self) -> Option<&Pty> {
        self.pty.as_ref().filter(|_| self.exit_code.is_none())
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Restored {
    pub adopted: usize,
    pub unknown: usize,
    pub exited: usize,
    pub dropped: usize,
}

pub struct Terminals {
    pub core: RefCell<Option<WeakCore>>,
    spawner: Rc<dyn Spawner>,
    store: Store,
    events: Sender<Event>,
    env: Vec<(String, String)>,
    live: RefCell<HashMap<String, Terminal>>,
    serial: Cell<u64>,
    sizes: RefCell<HashMap<String, (u16, u16)>>,
    changed: Cell<bool>,
    settings: RefCell<Settings>,
    pub on_start: RefCell<Option<OnStart>>,
    pub detector: RefCell<Option<Rc<Detector>>>,
}

impl Terminals {
    pub fn new(spawner: Rc<dyn Spawner>, store: Store, events: Sender<Event>, env: Vec<(String, String)>) -> Terminals {
        let settings = RefCell::new(store.load_settings());
        Terminals {
            settings,
            core: RefCell::new(None),
            spawner,
            store,
            events,
            env,
            live: RefCell::default(),
            serial: Cell::new(0),
            sizes: RefCell::default(),
            changed: Cell::new(false),
            on_start: RefCell::new(None),
            detector: RefCell::new(None),
        }
    }

    fn report(&self, id: &str, state: PaneState, exit_code: Option<i64>, error: Option<String>) {
        let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) else { return };
        let (id, reporter) = (id.to_string(), core.clone());
        core.later(move || reporter.update(&id, state, exit_code, error));
    }

    fn report_running(&self, id: &str, pid: i32) {
        let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) else { return };
        let (id, reporter) = (id.to_string(), core.clone());
        core.later(move || {
            reporter.set_pid(&id, Some(pid as i64));
            reporter.update(&id, PaneState::Running, None, None);
        });
    }

    fn spec(&self, pane: &Pane, terminal: &str) -> Spec {
        let mut env: Vec<(String, Option<String>)> = vec![("KANNA_INSTANCE".into(), None), ("KANNA_PANE".into(), None)];
        env.extend(self.env.iter().map(|(k, v)| (k.clone(), Some(v.clone()))));
        env.push(("KANNA_TERMINAL".into(), Some(terminal.to_string())));
        let (cols, rows) = self.sizes.borrow().get(&pane.id).copied().unwrap_or((COLS, ROWS));
        let command = pane.command.clone().filter(|c| !c.is_empty());
        Spec { id: terminal.to_string(), command, cwd: pane.cwd.clone(), env, cols, rows }
    }

    pub fn renamed(&self, from: &str, to: &str) {
        let size = self.sizes.borrow_mut().remove(from);
        if let Some(size) = size {
            self.sizes.borrow_mut().insert(to.into(), size);
        }
    }

    pub fn unshown(&self, pane: &Pane) {
        self.report(&pane.id, PaneState::Running, None, None);
    }

    pub fn pane_of(&self, terminal: &str) -> Option<String> {
        self.live.borrow().get(terminal).map(|t| t.pane.clone())
    }

    fn of<'a>(pane: &'a Pane) -> &'a str {
        pane.terminal.as_deref().unwrap_or_default()
    }

    fn next_serial(&self) -> u64 {
        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        serial
    }

    pub fn set_status(&self, id: &str, status: Option<&str>) {
        let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) else { return };
        let (id, status, reporter) = (id.to_string(), status.map(String::from), core.clone());
        core.later(move || {
            reporter.with_pane_mut(&id, |pane| pane.status = status);
        });
    }

    pub fn current(&self, id: &str, serial: u64) -> bool {
        self.live.borrow().get(id).is_some_and(|t| t.serial == serial && t.exit_code.is_none())
    }

    pub fn output(&self, id: &str, serial: u64) -> (Option<Taken>, Option<String>) {
        let (taken, cwd) = {
            let live = self.live.borrow();
            let Some(terminal) = live.get(id).filter(|t| t.serial == serial) else { return (None, None) };
            let mut feed = Feed::lock(&terminal.feed);
            (feed.take(), feed.take_cwd())
        };
        if let Some(cwd) = &cwd {
            self.moved(id, cwd.clone());
        }
        (taken, cwd)
    }

    fn moved(&self, id: &str, cwd: String) {
        let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) else { return };
        let Some(pane) = self.pane_of(id) else { return };
        core.with_pane_mut(&pane, |pane| pane.cwd = Some(cwd));
        self.changed.set(true);
    }

    pub fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    pub fn set_settings(&self, settings: Settings) {
        if *self.settings.borrow() == settings {
            return;
        }
        if let Err(error) = self.store.save_settings(&settings) {
            eprintln!("kanna-daemon: couldn't save the terminal settings: {error}");
        }
        for terminal in self.live.borrow().values() {
            let replies = Feed::lock(&terminal.feed).apply(&settings);
            if let (false, Some(pty)) = (replies.is_empty(), terminal.running()) {
                let _ = pty.input.send(replies);
            }
        }
        *self.settings.borrow_mut() = settings;
    }

    pub fn exited(&self, keeper: &str, pid: i32, code: i32) -> Option<(String, i32)> {
        let pane = {
            let mut live = self.live.borrow_mut();
            let found = live.iter_mut().find(|(_, t)| t.keeper == keeper && t.pid == pid && t.exit_code.is_none() && t.pty.is_some());
            found.map(|(id, terminal)| {
                terminal.exit_code = Some(code);
                if let Some(detector) = self.detector.borrow().as_ref() {
                    detector.forget(id, terminal.serial);
                }
                if !terminal.drained {
                    if let Some(pty) = &terminal.pty {
                        pty.reader.drain();
                    }
                }
                (id.clone(), terminal.drained)
            })
        };
        if pane.is_some() {
            self.changed.set(true);
            self.save_if_changed();
        }
        self.spawner.acknowledge_exit(keeper);
        match pane {
            Some((id, true)) => self.report_exit(&id).map(|code| (id, code)),
            _ => None,
        }
    }

    pub fn drained(&self, id: &str, serial: u64) -> Option<i32> {
        self.live.borrow_mut().get_mut(id).filter(|t| t.serial == serial)?.drained = true;
        self.report_exit(id)
    }

    fn report_exit(&self, id: &str) -> Option<i32> {
        let (code, pane) = {
            let mut live = self.live.borrow_mut();
            let terminal = live.get_mut(id).filter(|t| t.drained && !t.exit_reported)?;
            let code = terminal.exit_code?;
            terminal.exit_reported = true;
            (code, terminal.pane.clone())
        };
        self.report(&pane, PaneState::Exited, Some(code as i64), None);
        Some(code)
    }

    pub fn write(&self, id: &str, bytes: Vec<u8>) {
        if let Some(pty) = self.live.borrow().get(id).and_then(Terminal::running) {
            let _ = pty.input.send(bytes);
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16, width: u16, height: u16) -> Option<(u16, u16)> {
        let mut live = self.live.borrow_mut();
        let terminal = live.get_mut(id)?;
        let mut feed = Feed::lock(&terminal.feed);
        if cols > MAX_SIDE || rows > MAX_SIDE {
            eprintln!("kanna-daemon: refused to make {id} {cols}×{rows}: at most {MAX_SIDE} cells a side");
            return Some((feed.cols, feed.rows));
        }
        let (cols, rows) = (cols.max(1), rows.max(1));
        if (cols, rows, (width, height)) != (feed.cols, feed.rows, terminal.pixels) {
            if let Some(pty) = &terminal.pty {
                pty::set_size(&pty.master, cols, rows, width, height);
            }
            terminal.pixels = (width, height);
            self.sizes.borrow_mut().insert(terminal.pane.clone(), (cols, rows));
            let replies = feed.resize(id, cols, rows, width, height);
            if let (false, Some(pty)) = (replies.is_empty(), terminal.running()) {
                let _ = pty.input.send(replies);
            }
        }
        Some((cols, rows))
    }

    pub fn attach(&self, id: &str) -> Option<Attach> {
        let live = self.live.borrow();
        let t = live.get(id)?;
        let mut feed = Feed::lock(&t.feed);
        let snapshot = feed.snapshot();
        Some(Attach { cols: feed.cols, rows: feed.rows, snapshot, exit_code: t.exit_code.filter(|_| t.exit_reported) })
    }

    pub fn mark_changed(&self) {
        self.changed.set(true);
    }

    pub fn save_if_changed(&self) {
        if !self.changed.replace(false) {
            return;
        }
        let Some(saved) = self.saved() else { return };
        if let Err(error) = self.store.save(&saved) {
            eprintln!("kanna-daemon: couldn't save the terminals to {}: {error}", self.store.dir().display());
        }
    }

    fn saved(&self) -> Option<Saved> {
        let core = self.core.borrow().as_ref().and_then(WeakCore::upgrade)?;
        let live = self.live.borrow();
        let sizes = self.sizes.borrow();
        Some(core.with_panes(|panes| Saved {
            counter: panes.counter(),
            terminals: panes.terminals(),
            records: panes
                .iter()
                .filter(|pane| pane.kind == PaneType::Term)
                .filter_map(|pane| {
                    let id = pane.terminal.clone()?;
                    let terminal = live.get(&id)?;
                    let (state, exit_code) = match terminal.exit_code {
                        Some(code) => (PaneState::Exited, Some(code as i64)),
                        None => (PaneState::Running, None),
                    };
                    Some(Record {
                        id,
                        pane: pane.id.clone(),
                        name: pane.name.clone(),
                        cmd: pane.command.clone(),
                        cwd: pane.cwd.clone(),
                        state,
                        exit_code,
                        pid: terminal.pid as i64,
                        keeper: terminal.keeper.clone(),
                        size: sizes.get(&pane.id).copied(),
                        hung_up: pane.hung_up,
                    })
                })
                .collect(),
        }))
    }

    pub fn shutdown(&self) {
        let mut live = self.live.borrow_mut();
        for pty in live.values().filter_map(|t| t.pty.as_ref()) {
            pty.reader.stop();
        }
        for pty in live.values_mut().filter_map(|t| t.pty.as_mut()) {
            pty.reader.join();
        }
        drop(live);
        self.changed.set(true);
        self.save_if_changed();
        self.store.flush();
        self.store.stop();
    }

    pub fn restore(&self, adopted: Vec<Adopted>) -> Restored {
        let saved = self.store.load();
        let (mut counter, mut terminals) = (saved.counter, saved.terminals);
        for id in saved.records.iter().flat_map(|r| [&r.id, &r.pane, &r.keeper]).chain(adopted.iter().map(|a| &a.id)) {
            counter = counter.max(pane_number(id).unwrap_or(0));
            terminals = terminals.max(terminal_number(id).unwrap_or(0));
        }
        let mut ptys: HashMap<String, Adopted> = adopted.into_iter().map(|pty| (pty.id.clone(), pty)).collect();
        let mut restored = Restored::default();
        let mut panes = Vec::new();
        let mut exits = Vec::new();
        for record in saved.records {
            let mut pane = Pane::new(&record.pane, record.name.clone(), PaneType::Term);
            (pane.command, pane.cwd, pane.pid, pane.hung_up) = (record.cmd.clone(), record.cwd.clone(), Some(record.pid), record.hung_up);
            pane.terminal = Some(record.id.clone());
            let terminal = match ptys.remove(&record.keeper) {
                Some(pty) => {
                    let snapshot = (i64::from(pty.pid) == record.pid).then(|| self.store.snapshot(&record.id)).flatten();
                    pane.pid = Some(pty.pid as i64);
                    pane.opened_in = Some(pty.cwd.clone()).filter(|cwd| !cwd.is_empty());
                    pane.state = PaneState::Running;
                    exits.extend(pty.exit.map(|code| (record.keeper.clone(), pty.pid, code)));
                    restored.adopted += 1;
                    self.adopt(&record.id, &record.pane, &record.keeper, pty, snapshot)
                }
                None if record.state == PaneState::Exited => {
                    pane.state = PaneState::Exited;
                    pane.exit_code = record.exit_code;
                    restored.exited += 1;
                    self.ended(&record, self.store.snapshot(&record.id))
                }
                None => {
                    eprintln!("kanna-daemon: {} ended while no daemon was running, and the keeper has no exit for it", pane.label());
                    restored.dropped += 1;
                    continue;
                }
            };
            match terminal {
                Ok(terminal) => {
                    self.live.borrow_mut().insert(record.id.clone(), terminal);
                    panes.push(pane);
                }
                Err(error) => eprintln!("kanna-daemon: couldn't take {} back: {error}", pane.label()),
            }
        }
        let mut unknown: Vec<Adopted> = ptys.into_values().collect();
        unknown.sort_by_key(|pty| (terminal_number(&pty.id).or(pane_number(&pty.id)).unwrap_or(usize::MAX), pty.id.clone()));
        for pty in unknown {
            let id = match terminal_number(&pty.id) {
                Some(_) if !self.live.borrow().contains_key(&pty.id) => pty.id.clone(),
                _ => {
                    terminals += 1;
                    format!("term-{terminals}")
                }
            };
            let pane_id = match pane_number(&pty.id) {
                Some(_) if !panes.iter().any(|p: &Pane| p.id == pty.id) => pty.id.clone(),
                _ => {
                    counter += 1;
                    format!("p{counter}")
                }
            };
            let mut pane = Pane::new(&pane_id, None, PaneType::Term);
            pane.terminal = Some(id.clone());
            pane.command = match pty.cmd.as_slice() {
                [_, flags @ .., c, command] if c == "-c" && flags.first().is_some_and(|l| l == "-l") && flags.iter().all(|f| ["-l", "-i", "+m"].contains(&f.as_str())) => Some(command.clone()),
                [sh, c, command] if sh == "/bin/sh" && c == "-c" => Some(command.clone()),
                _ => None,
            };
            pane.cwd = Some(pty.cwd.clone()).filter(|cwd| !cwd.is_empty());
            pane.opened_in = pane.cwd.clone();
            pane.pid = Some(pty.pid as i64);
            pane.state = PaneState::Running;
            let keeper = pty.id.clone();
            exits.extend(pty.exit.map(|code| (keeper.clone(), pty.pid, code)));
            match self.adopt(&id, &pane_id, &keeper, pty, None) {
                Ok(terminal) => {
                    eprintln!("kanna-daemon: took back {id} as {pane_id}, which the keeper had and the disk didn't know");
                    self.live.borrow_mut().insert(id, terminal);
                    panes.push(pane);
                    restored.unknown += 1;
                }
                Err(error) => {
                    eprintln!("kanna-daemon: couldn't take back the keeper's {keeper}: {error}; closing it");
                    self.spawner.close(&keeper);
                }
            }
        }
        if let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) {
            core.install_panes(panes, counter, terminals);
        }
        let keep: Vec<String> = self.live.borrow().keys().cloned().collect();
        self.store.prune(&keep);
        self.changed.set(true);
        self.save_if_changed();
        for (keeper, pid, code) in exits {
            if let Some((terminal, _)) = self.exited(&keeper, pid, code) {
                eprintln!("kanna-daemon: {terminal} exited ({code}) while no daemon was running");
            }
        }
        restored
    }

    fn adopt(&self, id: &str, pane: &str, keeper: &str, pty: Adopted, snapshot: Option<Vec<u8>>) -> std::io::Result<Terminal> {
        let settings = self.settings();
        let restored = snapshot.and_then(|snapshot| match Feed::restore(&snapshot, &settings) {
            Ok(feed) => Some(feed),
            Err(error) => {
                eprintln!("kanna-daemon: {id}'s saved screen can't be read ({error}); starting it blank");
                None
            }
        });
        let fresh = restored.is_none();
        let feed = match restored {
            Some(feed) => feed,
            None => Feed::new(COLS, ROWS, &settings)?,
        };
        let (cols, rows) = {
            let mut screen = Feed::lock(&feed);
            let (cols, rows) = pty::size(&pty.master).unwrap_or((screen.cols, screen.rows));
            let (cols, rows) = (cols.min(MAX_SIDE), rows.min(MAX_SIDE));
            if (cols, rows) != (screen.cols, screen.rows) {
                screen.resize(id, cols, rows, 0, 0);
            }
            screen.unsaved |= fresh;
            (cols, rows)
        };
        pty::set_size(&pty.master, cols, rows, 0, 0);
        self.sizes.borrow_mut().insert(pane.into(), (cols, rows));
        let serial = self.next_serial();
        let terminal = self.run(id, pane, keeper, serial, Spawned { pid: pty.pid, master: pty.master }, feed)?;
        if let (None, Some(running)) = (pty.exit, &terminal.pty) {
            pty::redraw(&running.master);
        }
        Ok(terminal)
    }

    fn ended(&self, record: &Record, snapshot: Option<Vec<u8>>) -> std::io::Result<Terminal> {
        let settings = self.settings();
        let feed = match snapshot.map(|snapshot| Feed::restore(&snapshot, &settings)) {
            Some(Ok(feed)) => feed,
            _ => {
                let (cols, rows) = record.size.unwrap_or((COLS, ROWS));
                Feed::new(cols.min(MAX_SIDE), rows.min(MAX_SIDE), &settings)?
            }
        };
        let (cols, rows) = {
            let screen = Feed::lock(&feed);
            (screen.cols, screen.rows)
        };
        self.sizes.borrow_mut().insert(record.pane.clone(), (cols, rows));
        self.store.track(&record.id, feed.clone());
        Ok(Terminal {
            pane: record.pane.clone(),
            serial: self.next_serial(),
            pid: record.pid as i32,
            keeper: record.keeper.clone(),
            pty: None,
            feed,
            pixels: (0, 0),
            exit_code: Some(record.exit_code.unwrap_or(-1) as i32),
            exit_reported: true,
            drained: true,
        })
    }
}

impl PaneHost for Terminals {
    fn start(&self, pane: &Pane) {
        let Some(id) = pane.terminal.clone() else {
            return self.report(&pane.id, PaneState::Failed, None, Some("no terminal ID to start".into()));
        };
        let serial = self.next_serial();
        let spec = self.spec(pane, &id);
        self.set_status(&pane.id, None);
        let started = self.spawner.spawn(&spec).and_then(|spawned| {
            let feed = Feed::new(spec.cols, spec.rows, &self.settings())?;
            self.run(&id, &pane.id, &id, serial, spawned, feed)
        });
        match started {
            Ok(terminal) => {
                let pid = terminal.pid;
                self.live.borrow_mut().insert(id.clone(), terminal);
                self.changed.set(true);
                if let Some(on_start) = self.on_start.borrow().as_ref() {
                    on_start(&id);
                }
                self.report_running(&pane.id, pid);
            }
            Err(error) => {
                let what = spec.command.as_deref().unwrap_or("the shell");
                self.report(&pane.id, PaneState::Failed, None, Some(format!("could not start {what}: {error}")))
            }
        }
    }

    fn stop(&self, pane: &Pane) {
        let id = Self::of(pane);
        let removed = self.live.borrow_mut().remove(id);
        if let (Some(terminal), Some(detector)) = (&removed, self.detector.borrow().as_ref()) {
            detector.forget(id, terminal.serial);
        }
        if let Some(terminal) = removed {
            if terminal.running().is_some() {
                self.spawner.close(&terminal.keeper);
            }
            self.store.forget(id);
            self.changed.set(true);
        }
        let restarting = self.core.borrow().as_ref().and_then(WeakCore::upgrade).is_some_and(|core| core.pane(&pane.id).is_some());
        if !restarting {
            self.sizes.borrow_mut().remove(&pane.id);
        }
    }

    fn end(&self, pane: &Pane) {
        let pid = self.live.borrow().get(Self::of(pane)).filter(|t| t.exit_code.is_none()).map(|t| t.pid);
        if let Some(pid) = pid.filter(|pid| *pid > 0) {
            unsafe {
                libc::kill(-pid, libc::SIGHUP);
                libc::kill(pid, libc::SIGHUP);
            }
        }
    }

    fn send(&self, pane: &Pane, text: &str, done: Responder) {
        let line = {
            let live = self.live.borrow();
            live.get(Self::of(pane)).and_then(|terminal| {
                let pty = terminal.running()?;
                let feed = Feed::lock(&terminal.feed);
                Some(Line { label: pane.label(), text: feed.paste(text), enter: feed.enter(), input: pty.input.clone() })
            })
        };
        let Some(line) = line else { return done.reply(Err(KmuxError::new("bad_request", format!("{} has no running terminal", pane.label())))) };
        let sent = std::thread::Builder::new().name(format!("kanna.send.{}", pane.id)).spawn(move || line.type_in(done));
        if let Err(error) = sent {
            eprintln!("kanna-daemon: couldn't start sending text: {error}");
        }
    }

    fn send_keys(&self, pane: &Pane, names: Vec<String>, done: Responder) {
        match self.presses(pane, &names) {
            Ok(presses) => {
                let sent = std::thread::Builder::new().name(format!("kanna.keys.{}", pane.id)).spawn(move || presses.press(done));
                if let Err(error) = sent {
                    eprintln!("kanna-daemon: couldn't start sending keys: {error}");
                }
            }
            Err(error) => done.reply(Err(error)),
        }
    }

    fn read(&self, pane: &Pane, lines: Option<usize>) -> Result<String, KmuxError> {
        let live = self.live.borrow();
        let terminal = live.get(Self::of(pane)).ok_or_else(|| KmuxError::new("bad_request", format!("{} has no terminal", pane.label())))?;
        let text = Feed::lock(&terminal.feed).text(lines);
        Ok(text)
    }
}

struct Presses {
    label: String,
    keys: Vec<Key>,
    feed: Arc<Mutex<Feed>>,
    input: Sender<Vec<u8>>,
}

struct Line {
    label: String,
    text: Vec<u8>,
    enter: Vec<u8>,
    input: Sender<Vec<u8>>,
}

impl Line {
    fn type_in(self, done: Responder) {
        if !self.text.is_empty() {
            if self.input.send(self.text).is_err() {
                return done.reply(Err(KmuxError::new("bad_request", format!("{} ended before the text was sent", self.label))));
            }
            std::thread::sleep(ENTER_GAP);
        }
        if self.input.send(self.enter).is_err() {
            return done.reply(Err(KmuxError::new("bad_request", format!("{} ended before Return was pressed", self.label))));
        }
        done.reply(Ok(Map::new()));
    }
}

impl Presses {
    fn press(self, done: Responder) {
        for key in &self.keys {
            std::thread::sleep(KEY_GAP);
            let bytes = Feed::lock(&self.feed).encode_key(key).unwrap_or_default();
            if !bytes.is_empty() && self.input.send(bytes).is_err() {
                return done.reply(Err(KmuxError::new("bad_request", format!("{} ended before {key} was sent", self.label))));
            }
        }
        done.reply(Ok(Map::new()));
    }
}

impl Terminals {
    fn presses(&self, pane: &Pane, names: &[String]) -> Result<Presses, KmuxError> {
        let keys = names.iter().map(|name| name.parse::<Key>()).collect::<Result<Vec<Key>, KeyError>>().map_err(|error| {
            let message = match error {
                KeyError::AppOnly(key) => format!("{key} only means something to Kanna.app, not to the program in {}", pane.label()),
                KeyError::UnknownKey(key) if key.chars().count() > 1 => format!("\"{key}\" isn't a key; to type text, use kanna send"),
                error => error.to_string(),
            };
            KmuxError::new("bad_request", message)
        })?;
        let live = self.live.borrow();
        let terminal = live.get(Self::of(pane)).ok_or_else(|| KmuxError::new("bad_request", format!("{} has no terminal", pane.label())))?;
        let pty = terminal.running().ok_or_else(|| KmuxError::new("bad_request", format!("{} has exited", pane.label())))?;
        {
            let feed = Feed::lock(&terminal.feed);
            for key in &keys {
                let bytes = feed.encode_key(key).map_err(|error| KmuxError::new("bad_request", format!("couldn't encode {key}: {error}")))?;
                if bytes.is_empty() {
                    return Err(KmuxError::new("bad_request", format!("{key} means nothing to the program in {} in the mode it's in", pane.label())));
                }
            }
        }
        Ok(Presses { label: pane.label(), keys, feed: terminal.feed.clone(), input: pty.input.clone() })
    }

    fn run(&self, id: &str, pane: &str, keeper: &str, serial: u64, spawned: Spawned, feed: Arc<Mutex<Feed>>) -> std::io::Result<Terminal> {
        let Spawned { pid, master } = spawned;
        let (input, inputs) = mpsc::channel::<Vec<u8>>();
        let reader = Reader::start(id, serial, master.try_clone()?, feed.clone(), input.clone(), self.events.clone())?;
        let mut writer = File::from(master.try_clone()?);
        std::thread::Builder::new().name(format!("kanna.write.{id}")).spawn(move || {
            for bytes in inputs {
                if writer.write_all(&bytes).is_err() {
                    return;
                }
            }
        })?;
        self.store.track(id, feed.clone());
        if let Some(detector) = self.detector.borrow().as_ref() {
            detector.watch(id, serial, feed.clone(), master.try_clone()?);
        }
        Ok(Terminal {
            pane: pane.into(),
            serial,
            pid,
            keeper: keeper.into(),
            pty: Some(Pty { master, input, reader }),
            feed,
            pixels: (0, 0),
            exit_code: None,
            exit_reported: false,
            drained: false,
        })
    }
}
