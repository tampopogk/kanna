use crate::pty::{self, Adopted, Heard, Spawned, Spawner, Spec};
use crate::reports::Kept;
use crate::reader::{Feed, Noticed, Reader, Replayed, Typing};
use crate::wire::Wire;
use crate::Event;
use crate::settings::Settings;
use crate::status::Detector;
use crate::store::{pane_number, Record, Saved, Store};
use kanna_mux::json::{json, Map, Value};
use kanna_mux::{pty_number, terminal_number, KmuxError, Pane, PaneHost, PaneState, PaneType, Responder, WeakCore};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_vt::{Key, KeyError};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type OnStart = Box<dyn Fn(&str)>;

pub struct Attach {
    pub snapshot: Result<(), kanna_vt::Error>,
    pub exit_code: Option<i32>,
}

const LEAVING: Duration = Duration::from_secs(6);

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

struct Leaving {
    session: i32,
    start: bool,
}

struct Starting {
    id: String,
    pane: String,
    serial: u64,
    command: Option<String>,
    size: (u16, u16),
    pixels: (u16, u16),
    hung_up: bool,
    stopped: bool,
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
    events: crate::runtime::Events,
    env: Vec<(String, String)>,
    live: RefCell<HashMap<String, Terminal>>,
    starting: RefCell<HashMap<u64, Starting>>,
    leaving: RefCell<HashMap<String, Leaving>>,
    serial: Cell<u64>,
    sizes: RefCell<HashMap<String, (u16, u16)>>,
    changed: Cell<bool>,
    settings: RefCell<Settings>,
    pub on_start: RefCell<Option<OnStart>>,
    pub detector: RefCell<Option<Rc<Detector>>>,
    wire: Arc<Wire>,
}

impl Terminals {
    pub fn new(spawner: Rc<dyn Spawner>, store: Store, events: crate::runtime::Events, env: Vec<(String, String)>, wire: Arc<Wire>) -> Terminals {
        let settings = RefCell::new(store.load_settings());
        Terminals {
            settings,
            core: RefCell::new(None),
            spawner,
            store,
            events,
            env,
            live: RefCell::default(),
            starting: RefCell::default(),
            leaving: RefCell::default(),
            serial: Cell::new(0),
            sizes: RefCell::default(),
            changed: Cell::new(false),
            on_start: RefCell::new(None),
            detector: RefCell::new(None),
            wire,
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
        env.push(("KANNA_TERMINAL".into(), Some(pane.id.clone())));
        let (cols, rows) = self.sizes.borrow().get(&pane.id).copied().unwrap_or((COLS, ROWS));
        let command = pane.command.clone().filter(|c| !c.is_empty());
        Spec { id: terminal.to_string(), command, cwd: pane.cwd.clone(), env, cols, rows }
    }

    fn watch_leaving(&self, pane: &str, session: i32) {
        self.leaving.borrow_mut().insert(pane.into(), Leaving { session, start: false });
        let events = self.events.clone();
        let watching = std::thread::Builder::new().name("kanna.session-leaving".into()).spawn(move || {
            let by = Instant::now() + LEAVING;
            while kanna_keeper::session::alive(session) && Instant::now() < by {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = events.send(Event::SessionLeft(session));
        });
        if let Err(error) = watching {
            log!("kanna-daemon: couldn't watch session {session} leave: {error}");
            self.leaving.borrow_mut().remove(pane);
        }
    }

    pub fn session_left(&self, session: i32) {
        let pane = self.leaving.borrow().iter().find(|(_, leaving)| leaving.session == session).map(|(pane, _)| pane.clone());
        let Some(pane) = pane else { return };
        let start = self.leaving.borrow_mut().remove(&pane).is_some_and(|leaving| leaving.start);
        let core = self.core.borrow().as_ref().and_then(WeakCore::upgrade);
        if let (true, Some(pane)) = (start, core.and_then(|core| core.pane(&pane))) {
            self.start(&pane);
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

    pub fn noticed(&self, id: &str, serial: u64) -> Option<Noticed> {
        let noticed = {
            let live = self.live.borrow();
            let terminal = live.get(id).filter(|t| t.serial == serial)?;
            let noticed = Feed::lock(&terminal.feed).noticed();
            noticed
        };
        if let Some(cwd) = &noticed.cwd {
            self.moved(id, cwd.clone());
        }
        Some(noticed)
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
            log!("kanna-daemon: couldn't save the terminal settings: {error}");
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
                self.wire.remove(id);
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
        let saved = pane.is_none() || {
            self.changed.set(true);
            self.save_if_changed()
        };
        if saved {
            self.spawner.acknowledge_exit(keeper);
        }
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

    pub fn resize(&self, conn: u64, id: &str, cols: u16, rows: u16, width: u16, height: u16) -> Option<(u16, u16)> {
        let mut live = self.live.borrow_mut();
        let Some(terminal) = live.get_mut(id) else { return self.resize_starting(id, cols, rows, width, height) };
        let mut feed = Feed::lock(&terminal.feed);
        if cols > MAX_SIDE || rows > MAX_SIDE {
            log!("kanna-daemon: refused to make {id} {cols}×{rows}: at most {MAX_SIDE} cells a side");
            let grid = grid(id, feed.cols, feed.rows);
            self.wire.after_output(conn, id, &mut feed, &grid);
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
        let grid = grid(id, cols, rows);
        let viewers: Vec<u64> = feed.viewers.keys().copied().filter(|viewer| *viewer != conn).collect();
        self.wire.after_output(conn, id, &mut feed, &grid);
        for viewer in viewers {
            self.wire.after_output(viewer, id, &mut feed, &grid);
        }
        Some((cols, rows))
    }

    pub fn tell_grid(&self, conn: u64, id: &str) {
        if let Some(terminal) = self.live.borrow().get(id) {
            let mut feed = Feed::lock(&terminal.feed);
            let grid = grid(id, feed.cols, feed.rows);
            self.wire.after_output(conn, id, &mut feed, &grid);
        }
    }

    fn resize_starting(&self, id: &str, cols: u16, rows: u16, width: u16, height: u16) -> Option<(u16, u16)> {
        let mut starting = self.starting.borrow_mut();
        let start = starting.values_mut().find(|s| s.id == id && !s.stopped)?;
        if cols <= MAX_SIDE && rows <= MAX_SIDE {
            (start.size, start.pixels) = ((cols.max(1), rows.max(1)), (width, height));
            self.sizes.borrow_mut().insert(start.pane.clone(), start.size);
        }
        Some(start.size)
    }

    pub fn heard(&self, heard: Vec<Heard>) -> Vec<(String, i32)> {
        let mut ended = Vec::new();
        for heard in heard {
            match heard {
                Heard::Exited { keeper, pid, code } => ended.extend(self.exited(&keeper, pid, code)),
                Heard::Spawned(tag, spawned) => self.spawned(tag, spawned),
            }
        }
        ended
    }

    fn spawned(&self, tag: u64, spawned: std::io::Result<Spawned>) {
        let Some(start) = self.starting.borrow_mut().remove(&tag) else { return };
        if start.stopped {
            if spawned.is_ok() {
                self.spawner.close(&start.id);
            }
            return;
        }
        let (cols, rows) = start.size;
        let started = spawned.and_then(|spawned| {
            pty::set_size(&spawned.master, cols, rows, start.pixels.0, start.pixels.1);
            let feed = Feed::new(cols, rows, &self.settings())?;
            Feed::lock(&feed).resize(&start.id, cols, rows, start.pixels.0, start.pixels.1);
            self.run(&start.id, &start.pane, &start.id, start.serial, spawned, feed)
        });
        match started {
            Ok(mut terminal) => {
                terminal.pixels = start.pixels;
                let pid = terminal.pid;
                self.live.borrow_mut().insert(start.id.clone(), terminal);
                self.changed.set(true);
                if let Some(on_start) = self.on_start.borrow().as_ref() {
                    on_start(&start.id);
                }
                self.report_running(&start.pane, pid);
                if start.hung_up {
                    unsafe {
                        libc::kill(-pid, libc::SIGHUP);
                        libc::kill(pid, libc::SIGHUP);
                    }
                }
            }
            Err(error) => {
                let what = start.command.as_deref().unwrap_or("the shell");
                self.report(&start.pane, PaneState::Failed, None, Some(format!("could not start {what}: {error}")))
            }
        }
    }

    pub fn attach(&self, conn: u64, id: &str, shown: bool, history: Option<usize>, from: Option<u64>) -> Option<Attach> {
        let live = self.live.borrow();
        let t = live.get(id)?;
        let mut feed = Feed::lock(&t.feed);
        let messages = match from.and_then(|from| feed.replay(from)) {
            Some(replayed) => replayed
                .into_iter()
                .map(|item| match item {
                    Replayed::Output(bytes, offset) => crate::wire::output(id, &bytes, offset),
                    Replayed::Grid(cols, rows) => grid(id, cols, rows),
                })
                .collect(),
            None => match feed.snapshot(history) {
                Ok(snapshot) => vec![
                    json!({ "cmd": "snapshot", "args": { "terminal": id, "data": BASE64.encode(&snapshot.data), "offset": snapshot.offset, "more": snapshot.more } }),
                    grid(id, feed.cols, feed.rows),
                ],
                Err(error) => {
                    feed.hide(conn);
                    return Some(Attach { snapshot: Err(error), exit_code: None });
                }
            },
        };
        if shown {
            self.wire.shown(conn, id, &mut feed, &messages);
        }
        Some(Attach { snapshot: Ok(()), exit_code: t.exit_code.filter(|_| t.exit_reported) })
    }

    pub fn picture(&self, id: &str) -> Result<Map, KmuxError> {
        let live = self.live.borrow();
        let Some(t) = live.get(id) else { return Err(KmuxError::new("stopped", format!("{id} isn't running"))) };
        if t.running().is_none() {
            return Err(KmuxError::new("stopped", format!("{id} isn't running")));
        }
        let feed = Feed::lock(&t.feed);
        let snapshot = feed.save().map_err(|error| KmuxError::new("unavailable", format!("can't read {id}'s screen: {error}")))?;
        Ok(Map::from_iter([
            ("data".to_string(), json!(BASE64.encode(&snapshot))),
            ("cols".to_string(), json!(feed.cols)),
            ("rows".to_string(), json!(feed.rows)),
            ("width".to_string(), json!(t.pixels.0)),
            ("height".to_string(), json!(t.pixels.1)),
        ]))
    }

    pub fn hide(&self, conn: u64, id: &str) {
        if let Some(terminal) = self.live.borrow().get(id) {
            Feed::lock(&terminal.feed).hide(conn);
        }
    }

    pub fn hide_all(&self, conn: u64) {
        for terminal in self.live.borrow().values() {
            Feed::lock(&terminal.feed).hide(conn);
        }
    }

    pub fn after_output(&self, conn: u64, id: &str, message: &Value) {
        if let Some(terminal) = self.live.borrow().get(id) {
            self.wire.after_output(conn, id, &mut Feed::lock(&terminal.feed), message);
        }
    }

    pub fn wire(&self) -> &Arc<Wire> {
        &self.wire
    }

    pub fn mark_changed(&self) {
        self.changed.set(true);
    }

    pub fn save_if_changed(&self) -> bool {
        if !self.changed.replace(false) {
            return true;
        }
        let Some(saved) = self.saved() else { return true };
        match self.store.save(&saved) {
            Ok(()) => true,
            Err(error) => {
                log!("kanna-daemon: couldn't save the terminals to kanna.db: {error}");
                false
            }
        }
    }

    fn saved(&self) -> Option<Saved> {
        let core = self.core.borrow().as_ref().and_then(WeakCore::upgrade)?;
        let live = self.live.borrow();
        let sizes = self.sizes.borrow();
        Some(core.with_panes(|panes| Saved {
            counter: panes.counter(),
            terminals: panes.terminals(),
            ptys: panes.ptys(),
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

    pub fn hang_up_all(&self) -> usize {
        let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) else { return 0 };
        let code = 128 + libc::SIGHUP;
        let mut hung_up = 0;
        for (id, terminal) in self.live.borrow_mut().iter_mut().filter(|(_, t)| t.exit_code.is_none() && t.pty.is_some()) {
            terminal.exit_code = Some(code);
            terminal.exit_reported = true;
            self.wire.remove(id);
            if let Some(detector) = self.detector.borrow().as_ref() {
                detector.forget(id, terminal.serial);
            }
            core.with_pane_mut(&terminal.pane, |pane| {
                pane.hung_up = true;
                pane.state = PaneState::Exited;
                pane.exit_code = Some(code as i64);
            });
            hung_up += 1;
        }
        self.changed.set(true);
        self.save_if_changed();
        hung_up
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

    pub fn running(&self) -> Vec<String> {
        self.live.borrow().iter().filter(|(_, t)| t.running().is_some()).map(|(id, _)| id.clone()).collect()
    }

    pub fn restore(&self, adopted: Vec<Adopted>, mut reports: HashMap<String, Kept>, placed: &[String]) -> Restored {
        let saved = self.store.load();
        let (mut counter, mut terminals, mut numbered) = (saved.counter, saved.terminals, saved.ptys);
        for id in saved.records.iter().flat_map(|r| [&r.id, &r.pane, &r.keeper]).chain(adopted.iter().map(|a| &a.id)) {
            counter = counter.max(pane_number(id).unwrap_or(0));
            terminals = terminals.max(terminal_number(id).unwrap_or(0));
            numbered = numbered.max(pty_number(id).unwrap_or(0));
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
                    let kept = reports.remove(&record.id).filter(|_| i64::from(pty.pid) == record.pid);
                    self.adopt(&record.id, &record.pane, &record.keeper, pty, snapshot, kept)
                }
                None if record.state == PaneState::Exited => {
                    pane.state = PaneState::Exited;
                    pane.exit_code = record.exit_code;
                    restored.exited += 1;
                    self.ended(&record, self.store.snapshot(&record.id))
                }
                None if placed.contains(&record.pane) => {
                    log!("kanna-daemon: {} ended while no daemon was running, and the keeper has no exit for it", pane.label());
                    pane.state = PaneState::Exited;
                    restored.exited += 1;
                    self.ended(&record, self.store.snapshot(&record.id))
                }
                None => {
                    restored.dropped += 1;
                    continue;
                }
            };
            match terminal {
                Ok(terminal) => {
                    self.live.borrow_mut().insert(record.id.clone(), terminal);
                    panes.push(pane);
                }
                Err(error) => log!("kanna-daemon: couldn't take {} back: {error}", pane.label()),
            }
        }
        let mut unknown: Vec<Adopted> = ptys.into_values().collect();
        unknown.sort_by_key(|pty| (terminal_number(&pty.id).or(pty_number(&pty.id)).or(pane_number(&pty.id)).unwrap_or(usize::MAX), pty.id.clone()));
        for pty in unknown {
            let free = !self.live.borrow().contains_key(&pty.id);
            let id = match (terminal_number(&pty.id), pty_number(&pty.id)) {
                (Some(_), _) | (_, Some(_)) if free => pty.id.clone(),
                _ => {
                    numbered += 1;
                    format!("pty-{numbered}")
                }
            };
            let pane_id = match terminal_number(&id) {
                Some(_) => id.clone(),
                None => {
                    terminals += 1;
                    format!("term-{terminals}")
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
            match self.adopt(&id, &pane_id, &keeper, pty, None, None) {
                Ok(terminal) => {
                    log!("kanna-daemon: took back the keeper's {keeper} as {id}, which the disk didn't know");
                    self.live.borrow_mut().insert(id, terminal);
                    panes.push(pane);
                    restored.unknown += 1;
                }
                Err(error) => {
                    log!("kanna-daemon: couldn't take back the keeper's {keeper}: {error}; closing it");
                    self.spawner.close(&keeper);
                }
            }
        }
        if let Some(core) = self.core.borrow().as_ref().and_then(WeakCore::upgrade) {
            core.install_panes(panes, counter, terminals, numbered);
        }
        let keep: Vec<String> = self.live.borrow().keys().cloned().collect();
        self.store.prune(&keep);
        self.changed.set(true);
        self.save_if_changed();
        for (keeper, pid, code) in exits {
            if let Some((terminal, _)) = self.exited(&keeper, pid, code) {
                log!("kanna-daemon: {terminal} exited ({code}) while no daemon was running");
            }
        }
        restored
    }

    fn adopt(&self, id: &str, pane: &str, keeper: &str, pty: Adopted, snapshot: Option<Vec<u8>>, kept: Option<Kept>) -> std::io::Result<Terminal> {
        let settings = self.settings();
        let restored = snapshot.and_then(|snapshot| match Feed::restore(&snapshot, &settings) {
            Ok(feed) => Some(feed),
            Err(error) => {
                log!("kanna-daemon: {id}'s saved screen can't be read ({error}); starting it blank");
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
            if let Some(kept) = kept {
                screen.restore_statuses(kept.owner, kept.records);
            }
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
        self.set_status(&pane.id, None);
        if let Some(leaving) = self.leaving.borrow_mut().get_mut(&pane.id) {
            leaving.start = true;
            return;
        }
        let Some(id) = pane.terminal.clone() else {
            return self.report(&pane.id, PaneState::Failed, None, Some("no terminal ID to start".into()));
        };
        let serial = self.next_serial();
        let spec = self.spec(pane, &id);
        match self.spawner.spawn(&spec) {
            Ok(tag) => {
                let start = Starting { id, pane: pane.id.clone(), serial, command: spec.command, size: (spec.cols, spec.rows), pixels: (0, 0), hung_up: false, stopped: false };
                self.starting.borrow_mut().insert(tag, start);
            }
            Err(error) => {
                let what = spec.command.as_deref().unwrap_or("the shell");
                self.report(&pane.id, PaneState::Failed, None, Some(format!("could not start {what}: {error}")))
            }
        }
    }

    fn stop(&self, pane: &Pane) {
        let id = Self::of(pane);
        if let Some(leaving) = self.leaving.borrow_mut().get_mut(&pane.id) {
            leaving.start = false;
        }
        for start in self.starting.borrow_mut().values_mut().filter(|s| s.id == id) {
            start.stopped = true;
        }
        let removed = self.live.borrow_mut().remove(id);
        self.wire.remove(id);
        if let (Some(terminal), Some(detector)) = (&removed, self.detector.borrow().as_ref()) {
            detector.forget(id, terminal.serial);
        }
        if let Some(terminal) = removed {
            if terminal.running().is_some() {
                self.spawner.close(&terminal.keeper);
            }
            self.store.forget(id);
            self.changed.set(true);
            if terminal.pid > 0 {
                self.watch_leaving(&pane.id, terminal.pid);
            }
        }
        let restarting = self.core.borrow().as_ref().and_then(WeakCore::upgrade).is_some_and(|core| core.pane(&pane.id).is_some());
        if !restarting {
            self.sizes.borrow_mut().remove(&pane.id);
        }
    }

    fn end(&self, pane: &Pane) {
        for start in self.starting.borrow_mut().values_mut().filter(|s| s.id == Self::of(pane)) {
            start.hung_up = true;
        }
        let pid = self.live.borrow().get(Self::of(pane)).filter(|t| t.exit_code.is_none()).map(|t| t.pid);
        if let Some(pid) = pid.filter(|pid| *pid > 0) {
            unsafe {
                libc::kill(-pid, libc::SIGHUP);
                libc::kill(pid, libc::SIGHUP);
            }
        }
    }

    fn send(&self, pane: &Pane, text: &str, done: Responder) {
        self.line(pane, text, true, done);
    }

    fn paste(&self, pane: &Pane, text: &str, done: Responder) {
        self.line(pane, text, false, done);
    }

    fn send_keys(&self, pane: &Pane, names: Vec<String>, done: Responder) {
        match self.presses(pane, &names) {
            Ok(presses) => {
                let sent = std::thread::Builder::new().name(format!("kanna.keys.{}", pane.id)).spawn(move || presses.press(done));
                if let Err(error) = sent {
                    log!("kanna-daemon: couldn't start sending keys: {error}");
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
    enter: Option<Vec<u8>>,
    input: Sender<Vec<u8>>,
}

impl Line {
    fn type_in(self, done: Responder) {
        if !self.text.is_empty() {
            if self.input.send(self.text).is_err() {
                return done.reply(Err(KmuxError::new("bad_request", format!("{} ended before the text was sent", self.label))));
            }
            if self.enter.is_some() {
                std::thread::sleep(ENTER_GAP);
            }
        }
        let Some(enter) = self.enter else { return done.reply(Ok(Map::new())) };
        if self.input.send(enter).is_err() {
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
    fn line(&self, pane: &Pane, text: &str, enter: bool, done: Responder) {
        let line = {
            let live = self.live.borrow();
            live.get(Self::of(pane)).and_then(|terminal| {
                let pty = terminal.running()?;
                let feed = Feed::lock(&terminal.feed);
                Some(Line { label: pane.label(), text: feed.paste(text), enter: enter.then(|| feed.enter()), input: pty.input.clone() })
            })
        };
        let Some(line) = line else { return done.reply(Err(KmuxError::new("bad_request", format!("{} has no running terminal", pane.label())))) };
        let sent = std::thread::Builder::new().name(format!("kanna.send.{}", pane.id)).spawn(move || line.type_in(done));
        if let Err(error) = sent {
            log!("kanna-daemon: couldn't start sending text: {error}");
        }
    }

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
        let typing = Typing::default();
        self.store.track(id, feed.clone());
        if let Some(detector) = self.detector.borrow().as_ref() {
            detector.watch(id, serial, feed.clone(), master.try_clone()?);
        }
        let reader = Reader::start(id, serial, master.try_clone()?, feed.clone(), typing.clone(), self.wire.clone(), input.clone(), self.events.clone()).inspect_err(|_| {
            if let Some(detector) = self.detector.borrow().as_ref() {
                detector.forget(id, serial);
            }
        })?;
        let mut writer = File::from(master.try_clone()?);
        let written = typing.clone();
        std::thread::Builder::new().name(format!("kanna.write.{id}")).spawn(move || {
            for bytes in inputs {
                if writer.write_all(&bytes).is_err() {
                    return;
                }
                crate::reader::written(&written);
            }
        })?;
        self.wire.add(id, serial, reader.resumer()?, input.clone(), typing, feed.clone());
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

fn grid(id: &str, cols: u16, rows: u16) -> Value {
    json!({ "cmd": "grid", "args": { "terminal": id, "cols": cols, "rows": rows } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Host;
    use crate::keeper::Keeper;
    use kanna_mux::json::{json, Value};
    use std::io::{BufRead, BufReader};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::net::UnixListener;
    use std::path::Path;
    use std::time::Instant;

    fn fake_keeper(listener: UnixListener, pid: i32, spawns: Sender<u64>, release: mpsc::Receiver<()>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut lines = BufReader::new(stream.try_clone().unwrap());
            let send = |reply: Value, fds: &[i32]| {
                let bytes = format!("{reply}\n");
                kanna_keeper::fdpass::send(stream.as_raw_fd(), bytes.as_bytes(), fds, false).unwrap();
            };
            let mut line = String::new();
            while lines.read_line(&mut line).is_ok_and(|n| n > 0) {
                let req: Value = serde_json::from_str(&line).unwrap();
                line.clear();
                let tag = req["req"].clone();
                match req["op"].as_str() {
                    Some("hello") => send(json!({"ok": true, "version": kanna_keeper::PROTOCOL_VERSION, "pid": 4242, "req": tag}), &[]),
                    Some("spawn") => {
                        spawns.send(tag.as_u64().unwrap()).unwrap();
                        release.recv().unwrap();
                        let (mut master, mut child) = (0, 0);
                        assert_eq!(unsafe { libc::openpty(&mut master, &mut child, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) }, 0);
                        send(json!({"ok": true, "id": req["id"], "pid": pid, "cmd": ["sleep"], "fds": 1, "req": tag}), &[master]);
                        unsafe { libc::close(master) };
                        std::mem::forget(unsafe { OwnedFd::from_raw_fd(child) });
                    }
                    _ => send(json!({"ok": true, "req": tag}), &[]),
                }
            }
        })
    }

    struct NoSpawner;

    impl Spawner for NoSpawner {
        fn spawn(&self, _: &Spec) -> std::io::Result<u64> {
            Ok(0)
        }
        fn close(&self, _: &str) {}
        fn acknowledge_exit(&self, _: &str) {}
    }

    #[test]
    fn a_started_terminal_is_told_its_daemons_kanna() {
        let dir = std::env::temp_dir().join(format!("kdt-cli{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Helpers")).unwrap();
        for binary in ["kanna", "kanna-daemon"] {
            std::fs::write(dir.join("Helpers").join(binary), "").unwrap();
        }
        let cli = crate::cli::beside(&dir.join("Helpers/kanna-daemon"));
        let mut env = vec![("KANNA_SOCKET".to_string(), "/s/kanna.sock".to_string())];
        env.extend(crate::cli::env(cli.as_deref()));
        let (events, _inbox) = crate::runtime::channel();
        let terminals = Terminals::new(Rc::new(NoSpawner), Store::open(&dir, Rc::new(crate::db::Db::open(&dir).unwrap())).unwrap(), events.clone(), env, Arc::new(Wire::new(events)));
        let spec = terminals.spec(&Pane::new("term-7", None, PaneType::Term), "t1");
        let var = |name: &str| spec.env.iter().rev().find(|(key, _)| key == name).and_then(|(_, value)| value.clone());
        let named = var(crate::cli::VAR).expect("KANNA_CLI is set");
        assert_eq!(Path::new(&named), dir.join("Helpers/kanna"));
        assert!(Path::new(&named).is_file(), "it names a kanna that exists");
        assert_eq!(Path::new(&named).parent(), dir.join("Helpers/kanna-daemon").parent(), "beside the daemon");
        assert_eq!((var("KANNA_SOCKET").as_deref(), var("KANNA_TERMINAL").as_deref()), (Some("/s/kanna.sock"), Some("term-7")));
        drop(terminals);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_spawn_whose_reply_is_held_doesnt_hold_up_other_commands() {
        let dir = std::env::temp_dir().join(format!("kdt{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (spawns, spawned) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let fake = fake_keeper(UnixListener::bind(dir.join("keeper.sock")).unwrap(), 4243, spawns, released);
        let (events, _inbox) = crate::runtime::channel();
        let keeper = Rc::new(Keeper::connect(&dir, Path::new("/nonexistent/kanna-keeper"), events.clone()).unwrap());
        let serial = Rc::new(crate::serial::Serial::new(events.clone(), &dir));
        let terminals = Rc::new(Terminals::new(keeper.clone(), Store::open(&dir, Rc::new(crate::db::Db::open(&dir).unwrap())).unwrap(), events.clone(), vec![], Arc::new(Wire::new(events))));
        let core = kanna_mux::Core::default();
        *terminals.core.borrow_mut() = Some(core.weak());
        core.set_host(Some(Rc::new(Host::new(terminals.clone(), serial))));
        let opened = Rc::new(RefCell::new(None));
        let answer = opened.clone();
        core.handle(json!({"cmd": "open", "args": {"type": "term", "cmd": "sleep 30"}}), move |reply| *answer.borrow_mut() = Some(reply));
        core.turn();
        spawned.recv_timeout(Duration::from_secs(5)).expect("the keeper was asked to spawn");
        let listed = core.handle_blocking(json!({"cmd": "list"}));
        assert_eq!(listed["ok"], true, "{listed}");
        let pane = listed["panes"][0]["id"].as_str().unwrap().to_string();
        assert_eq!(core.pane(&pane).map(|p| p.state), Some(PaneState::Starting));
        assert!(opened.borrow().is_none(), "open waits for the terminal to start");
        release.send(()).unwrap();
        let by = Instant::now() + Duration::from_secs(5);
        while opened.borrow().is_none() {
            assert!(Instant::now() < by, "the spawn's reply never finished the start");
            assert!(terminals.heard(keeper.take().unwrap()).is_empty());
            core.turn();
            std::thread::sleep(Duration::from_millis(5));
        }
        let reply = opened.borrow_mut().take().unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(core.pane(&pane).map(|p| (p.state, p.pid)), Some((PaneState::Running, Some(4243))));
        assert!(terminals.attach(1, core.pane(&pane).unwrap().terminal.as_deref().unwrap(), false, None, None).is_some());
        drop((terminals, keeper));
        drop(fake);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn held_restart(name: &str, then: impl FnOnce(&kanna_mux::Core, &str), comes_back: bool) {
        use std::os::unix::process::CommandExt;
        let dir = std::env::temp_dir().join(format!("kdh{name}{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut session = std::process::Command::new("sleep");
        session.arg("30");
        unsafe {
            session.pre_exec(|| {
                libc::setsid();
                Ok(())
            })
        };
        let mut session = session.spawn().unwrap();
        let pid = session.id() as i32;
        let by = Instant::now() + Duration::from_secs(5);
        while unsafe { libc::getsid(pid) } != pid {
            assert!(Instant::now() < by, "the session never started");
            std::thread::sleep(Duration::from_millis(5));
        }
        let (spawns, spawned) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let fake = fake_keeper(UnixListener::bind(dir.join("keeper.sock")).unwrap(), pid, spawns, released);
        let (events, mut inbox) = crate::runtime::channel();
        let keeper = Rc::new(Keeper::connect(&dir, Path::new("/nonexistent/kanna-keeper"), events.clone()).unwrap());
        let serial = Rc::new(crate::serial::Serial::new(events.clone(), &dir));
        let terminals = Rc::new(Terminals::new(keeper.clone(), Store::open(&dir, Rc::new(crate::db::Db::open(&dir).unwrap())).unwrap(), events.clone(), vec![], Arc::new(Wire::new(events))));
        let core = kanna_mux::Core::default();
        *terminals.core.borrow_mut() = Some(core.weak());
        core.set_host(Some(Rc::new(Host::new(terminals.clone(), serial))));
        core.handle(json!({"cmd": "open", "args": {"type": "term", "cmd": "sleep 30", "wait": false}}), |_| {});
        core.turn();
        spawned.recv_timeout(Duration::from_secs(5)).expect("the first spawn");
        release.send(()).unwrap();
        let by = Instant::now() + Duration::from_secs(5);
        let id = loop {
            assert!(Instant::now() < by, "the terminal never ran");
            terminals.heard(keeper.take().unwrap());
            core.turn();
            let listed = core.handle_blocking(json!({"cmd": "list"}));
            let pane = &listed["panes"][0];
            if pane["state"] == "running" {
                break pane["id"].as_str().unwrap().to_string();
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        core.with_pane_mut(&id, |pane| pane.status = Some("idle".into()));
        core.handle(json!({"cmd": "restart", "args": {"pane": id, "wait": false}}), |_| {});
        core.turn();
        assert!(spawned.recv_timeout(Duration::from_millis(200)).is_err(), "the old session is still there, so the new one waits");
        core.turn();
        assert_eq!(core.pane(&id).map(|p| (p.state, p.status)), Some((PaneState::Starting, None)), "the old run's status goes as it waits, so nothing reads it as the new run's");
        then(&core, &id);
        core.turn();
        session.kill().unwrap();
        session.wait().unwrap();
        let by = Instant::now() + Duration::from_secs(5);
        let left = loop {
            assert!(Instant::now() < by, "never heard the session leave");
            match inbox.try_recv() {
                Ok(Event::SessionLeft(left)) => break left,
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        assert_eq!(left, pid);
        terminals.session_left(left);
        core.turn();
        assert_eq!(spawned.recv_timeout(Duration::from_millis(comes_back.then_some(5000).unwrap_or(200))).is_ok(), comes_back);
        drop(release);
        drop((terminals, keeper));
        drop(fake);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restart_starts_once_the_old_session_is_gone() {
        held_restart("a", |_, _| {}, true);
    }

    #[test]
    fn a_pane_closed_while_it_waits_for_its_old_session_doesnt_start() {
        held_restart("b", |core, id| core.handle(json!({"cmd": "close", "args": {"pane": id}}), |_| {}), false);
    }
}
