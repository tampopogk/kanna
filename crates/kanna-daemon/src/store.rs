use crate::db::{Db, TerminalCounters, TerminalRow};
use crate::reader::Feed;
use crate::settings::Settings;
use kanna_mux::json::{json, num, opt, string, Value};
use kanna_mux::{pty_number, terminal_number, PaneState};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const VERSION: u32 = 3;

const RECORDS: &str = "terminals.json";

const IMPORTED: &str = "terminals.json.imported";

const ID_GAP: usize = 1000;

const SNAPSHOT: &str = "snap";

const SETTINGS: &str = "settings.json";

const MAX_SETTINGS: u64 = 1 << 20;

pub const SAVE_EVERY: Duration = Duration::from_secs(1);

const TICK: Duration = Duration::from_millis(250);

pub const MAX_SNAPSHOT: usize = 16 << 20;

const MAX_RECORDS: u64 = 4 << 20;

const FLUSH_LIMIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub id: String,
    pub pane: String,
    pub name: Option<String>,
    pub cmd: Option<String>,
    pub cwd: Option<String>,
    pub state: PaneState,
    pub exit_code: Option<i64>,
    pub pid: i64,
    pub keeper: String,
    pub size: Option<(u16, u16)>,
    pub hung_up: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Saved {
    pub counter: usize,
    pub terminals: usize,
    pub ptys: usize,
    pub records: Vec<Record>,
}

impl Record {
    fn to_value(&self) -> Value {
        let mut out = json!({
            "id": self.id,
            "pane": self.pane,
            "name": opt(self.name.as_deref()),
            "type": "term",
            "cmd": opt(self.cmd.as_deref()),
            "cwd": opt(self.cwd.as_deref()),
            "state": self.state.as_str(),
            "exitCode": self.exit_code.map_or(Value::Null, |c| num(c as f64)),
            "pid": num(self.pid as f64),
            "keeper": self.keeper,
            "cols": self.size.map_or(Value::Null, |(cols, _)| num(cols as f64)),
            "rows": self.size.map_or(Value::Null, |(_, rows)| num(rows as f64)),
        });
        if self.hung_up {
            out["hungUp"] = Value::Bool(true);
        }
        out
    }

    fn to_row(&self) -> TerminalRow {
        let mut doc = self.to_value();
        if let Some(fields) = doc.as_object_mut() {
            for key in ["id", "pane", "keeper", "state", "type"] {
                fields.remove(key);
            }
        }
        TerminalRow { id: self.id.clone(), pane: self.pane.clone(), keeper: self.keeper.clone(), state: self.state.as_str().into(), doc }
    }

    fn from_row(row: TerminalRow) -> Option<Record> {
        let mut value = row.doc;
        for (key, text) in [("id", row.id), ("pane", row.pane), ("keeper", row.keeper), ("state", row.state), ("type", "term".to_string())] {
            value[key] = Value::String(text);
        }
        Record::from_value(&value, VERSION)
    }

    fn from_value(value: &Value, version: u32) -> Option<Record> {
        let text = |key: &str| string(value, key).map(String::from);
        if string(value, "type") != Some("term") {
            return None;
        }
        let (id, pane) = match version {
            1 => {
                let pane = text("id")?;
                (format!("term-{}", pane_number(&pane)?), pane)
            }
            2 => (text("id").filter(|id| terminal_number(id).is_some())?, text("pane").filter(|pane| pane_number(pane).is_some())?),
            _ => {
                let id = text("id").filter(|id| terminal_number(id).is_some() || pty_number(id).is_some())?;
                let pane = text("pane").filter(|pane| terminal_number(pane).is_some()).or_else(|| Some(id.clone()).filter(|id| terminal_number(id).is_some()))?;
                (id, pane)
            }
        };
        Some(Record {
            keeper: text("keeper").unwrap_or_else(|| pane.clone()),
            id,
            pane,
            name: text("name").filter(|n| !n.is_empty()),
            cmd: text("cmd"),
            cwd: text("cwd"),
            state: string(value, "state").and_then(PaneState::parse)?,
            exit_code: value.get("exitCode").and_then(Value::as_f64).map(|c| c as i64),
            pid: value.get("pid").and_then(Value::as_f64)? as i64,
            size: side(value, "cols").zip(side(value, "rows")),
            hung_up: value.get("hungUp") == Some(&Value::Bool(true)),
        })
    }
}

fn side(value: &Value, key: &str) -> Option<u16> {
    value.get(key).and_then(Value::as_f64).filter(|n| n.fract() == 0.0 && (1.0..=f64::from(u16::MAX)).contains(n)).map(|n| n as u16)
}

pub fn pane_number(id: &str) -> Option<usize> {
    let n: usize = id.strip_prefix('p')?.parse().ok()?;
    (n > 0 && n < 1_000_000_000 && format!("p{n}") == id).then_some(n)
}

pub struct Store {
    dir: PathBuf,
    db: Rc<Db>,
    written: RefCell<Saved>,
    gap: usize,
    saver: Sender<Job>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

enum Job {
    Track(String, Arc<Mutex<Feed>>),
    Forget(String),
    Flush(Sender<()>),
    Stop,
}

impl Store {
    pub fn open(state_dir: &Path, db: Rc<Db>) -> Result<Store, String> {
        let dir = state_dir.join("terminals");
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(format!("{} isn't a folder", dir.display()));
            }
            Ok(_) => fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?,
            Err(_) => fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| e.to_string())?,
        }
        absorb(&dir, &db).map_err(|e| e.to_string())?;
        let written = read_rows(&db).map_err(|e| e.to_string())?;
        let gap = if db.lost() { ID_GAP } else { 0 };
        let (saver, jobs) = mpsc::channel();
        let folder = dir.clone();
        let thread = std::thread::Builder::new().name("kanna.save".into()).spawn(move || save_loop(&folder, jobs)).map_err(|e| e.to_string())?;
        Ok(Store { dir, db, written: RefCell::new(written), gap, saver, thread: Mutex::new(Some(thread)) })
    }

    #[cfg(test)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn content_ids(state_dir: &Path) -> HashMap<String, String> {
        let dir = state_dir.join("terminals");
        let path = dir.join(RECORDS);
        let Some(mut value) = fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) else { return HashMap::new() };
        let version = value.get("version").and_then(Value::as_f64).unwrap_or(0.0) as u32;
        if !(version == 1 || version == 2) || string(&value, "kanna") != Some("terminals") {
            return HashMap::new();
        }
        let mut renamed = HashMap::new();
        if let Some(Value::Array(items)) = value.get_mut("terminals") {
            for item in items.iter_mut() {
                let Some(record) = Record::from_value(item, version) else { continue };
                if version == 1 {
                    let _ = fs::rename(snapshot_path(&dir, &record.pane), snapshot_path(&dir, &record.id));
                }
                renamed.insert(record.pane.clone(), record.id.clone());
                item["id"] = Value::String(record.id.clone());
                item["pane"] = Value::String(record.id);
            }
        }
        value["version"] = num(VERSION as f64);
        let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
        text.push('\n');
        if let Err(error) = write_atomic(&path, text.as_bytes()) {
            log!("kanna-daemon: couldn't rewrite {} with terminal IDs: {error}", path.display());
        }
        renamed
    }

    pub fn load(&self) -> Saved {
        let written = self.written.borrow();
        Saved { counter: written.counter, terminals: written.terminals + self.gap, ptys: written.ptys + self.gap, records: written.records.clone() }
    }

    pub fn save(&self, saved: &Saved) -> rusqlite::Result<()> {
        let mut written = self.written.borrow_mut();
        if *written == *saved {
            return Ok(());
        }
        let gone: Vec<String> = written.records.iter().filter(|old| !saved.records.iter().any(|new| new.id == old.id)).map(|old| old.id.clone()).collect();
        let changed: Vec<TerminalRow> = saved.records.iter().filter(|new| !written.records.contains(new)).map(Record::to_row).collect();
        self.db.put_terminals(&gone, &changed, counters_of(saved))?;
        *written = Saved { counter: saved.counter, terminals: saved.terminals, ptys: saved.ptys, records: saved.records.clone() };
        Ok(())
    }

    pub fn load_settings(&self) -> Settings {
        let Ok(file) = fs::File::open(self.dir.join(SETTINGS)) else { return Settings::default() };
        let mut text = String::new();
        if io::Read::read_to_string(&mut io::Read::take(file, MAX_SETTINGS), &mut text).is_err() {
            return Settings::default();
        }
        serde_json::from_str::<Value>(&text)
            .ok()
            .filter(|value| string(value, "kanna") == Some("settings"))
            .map_or_else(Settings::default, |value| Settings::from_value(&value))
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        let mut value = settings.to_value();
        value["kanna"] = Value::String("settings".into());
        let mut text = serde_json::to_string_pretty(&value).map_err(io::Error::other)?;
        text.push('\n');
        write_atomic(&self.dir.join(SETTINGS), text.as_bytes())
    }

    pub fn snapshot(&self, id: &str) -> Option<Vec<u8>> {
        let path = snapshot_path(&self.dir, id);
        let size = fs::metadata(&path).ok()?.len();
        if size as usize > MAX_SNAPSHOT {
            return None;
        }
        fs::read(path).ok()
    }

    pub fn prune(&self, keep: &[String]) {
        let Ok(entries) = fs::read_dir(&self.dir) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stale = match name.strip_suffix(&format!(".{SNAPSHOT}")) {
                Some(id) => !keep.iter().any(|k| k == id),
                None => name.contains(".tmp-"),
            };
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    pub fn track(&self, id: &str, feed: Arc<Mutex<Feed>>) {
        let _ = self.saver.send(Job::Track(id.into(), feed));
    }

    pub fn forget(&self, id: &str) {
        let _ = self.saver.send(Job::Forget(id.into()));
    }

    pub fn flush(&self) {
        let (done, flushed) = mpsc::channel();
        if self.saver.send(Job::Flush(done)).is_ok() {
            let _ = flushed.recv_timeout(FLUSH_LIMIT);
        }
    }

    pub fn stop(&self) {
        let _ = self.saver.send(Job::Stop);
        if let Some(thread) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = thread.join();
        }
    }
}

fn counters_of(saved: &Saved) -> TerminalCounters {
    TerminalCounters { pane: saved.counter, terminal: saved.terminals, pty: saved.ptys }
}

fn read_rows(db: &Db) -> rusqlite::Result<Saved> {
    let (rows, counters) = db.terminals()?;
    let (mut records, mut unreadable) = (Vec::with_capacity(rows.len()), Vec::new());
    for row in rows {
        let id = row.id.clone();
        match Record::from_row(row) {
            Some(record) => records.push(record),
            None => {
                log!("kanna-daemon: {id}'s record in kanna.db can't be read; dropped it");
                unreadable.push(id);
            }
        }
    }
    if !unreadable.is_empty() {
        db.put_terminals(&unreadable, &[], counters)?;
    }
    records.sort_by_key(|record| (terminal_number(&record.pane).unwrap_or(usize::MAX), record.pane.clone()));
    Ok(Saved { counter: counters.pane, terminals: counters.terminal, ptys: counters.pty, records })
}

fn absorb(dir: &Path, db: &Db) -> rusqlite::Result<()> {
    let (file, imported) = (dir.join(RECORDS), dir.join(IMPORTED));
    if db.terminals_imported()? {
        if file.exists() {
            log!("kanna-daemon: {} is beside a db that has its terminals; not reading it, and renaming it {IMPORTED}", file.display());
            if let Err(error) = fs::rename(&file, &imported) {
                log!("kanna-daemon: couldn't rename {}: {error}", file.display());
            }
        }
        return Ok(());
    }
    let Some(source) = [&file, &imported].into_iter().find(|path| path.exists()) else { return db.mark_terminals_imported() };
    match read_file(dir, source) {
        Some(saved) => {
            db.import_terminals(&saved.records.iter().map(Record::to_row).collect::<Vec<_>>(), counters_of(&saved))?;
            log!("kanna-daemon: imported {} terminals from {} into kanna.db", saved.records.len(), source.display());
            if *source == file {
                if let Err(error) = fs::rename(&file, &imported) {
                    log!("kanna-daemon: couldn't rename {}: {error}", file.display());
                }
            }
        }
        None => {
            log!("kanna-daemon: {} can't be read; moved aside as {RECORDS}.bad", source.display());
            let _ = fs::rename(source, dir.join(format!("{RECORDS}.bad")));
            db.mark_terminals_imported()?;
        }
    }
    Ok(())
}

fn read_file(dir: &Path, path: &Path) -> Option<Saved> {
    let file = fs::File::open(path).ok()?;
    let mut text = String::new();
    let read = io::Read::read_to_string(&mut io::Read::take(file, MAX_RECORDS + 1), &mut text).ok()?;
    if read as u64 > MAX_RECORDS {
        return None;
    }
    let value = serde_json::from_str::<Value>(&text).ok()?;
    let version = value.get("version").and_then(Value::as_f64).filter(|v| [1.0, 2.0, VERSION as f64].contains(v))? as u32;
    if string(&value, "kanna") != Some("terminals") {
        return None;
    }
    let count = |key: &str| value.get("counters").and_then(|c| c.get(key)).and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as usize;
    let mut records: Vec<Record> = Vec::new();
    for item in value.get("terminals")?.as_array()? {
        let record = Record::from_value(item, version)?;
        if records.iter().any(|r| r.id == record.id || r.pane == record.pane || r.keeper == record.keeper || (r.name.is_some() && r.name == record.name)) {
            return None;
        }
        records.push(record);
    }
    if version == 1 {
        for record in &records {
            let _ = fs::rename(snapshot_path(dir, &record.pane), snapshot_path(dir, &record.id));
        }
    }
    Some(Saved { counter: count("pane"), terminals: count("terminal"), ptys: count("pty"), records })
}

fn snapshot_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.{SNAPSHOT}"))
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = path.with_file_name(format!("{name}.tmp-{}", std::process::id()));
    let _ = fs::remove_file(&temporary);
    let written = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .and_then(|mut file| file.write_all(bytes))
        .and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

struct Tracked {
    feed: Arc<Mutex<Feed>>,
    next: Instant,
    failing: bool,
    too_big: bool,
}

fn save_loop(dir: &Path, jobs: mpsc::Receiver<Job>) {
    let mut tracked: HashMap<String, Tracked> = HashMap::new();
    loop {
        let job = jobs.recv_timeout(TICK);
        let mut flushed = None;
        match job {
            Ok(Job::Track(id, feed)) => {
                tracked.insert(id, Tracked { feed, next: Instant::now(), failing: false, too_big: false });
            }
            Ok(Job::Forget(id)) => {
                tracked.remove(&id);
                let _ = fs::remove_file(snapshot_path(dir, &id));
            }
            Ok(Job::Flush(done)) => flushed = Some(done),
            Ok(Job::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        for (id, terminal) in tracked.iter_mut() {
            if flushed.is_some() || terminal.next <= now {
                save_one(dir, id, terminal);
            }
        }
        if let Some(done) = flushed {
            let _ = done.send(());
        }
    }
}

fn save_one(dir: &Path, id: &str, terminal: &mut Tracked) {
    let snapshot = {
        let mut feed = Feed::lock(&terminal.feed);
        if !feed.unsaved {
            return;
        }
        let snapshot = feed.save();
        if snapshot.is_ok() {
            feed.unsaved = false;
        }
        snapshot
    };
    terminal.next = Instant::now() + SAVE_EVERY;
    let snapshot = match snapshot {
        Ok(snapshot) => snapshot,
        Err(error) => {
            if !std::mem::replace(&mut terminal.failing, true) {
                log!("kanna-daemon: can't snapshot {id} for the disk yet ({error}); keeping the last one and trying again");
            }
            return;
        }
    };
    terminal.failing = false;
    if snapshot.len() > MAX_SNAPSHOT {
        if !std::mem::replace(&mut terminal.too_big, true) {
            log!("kanna-daemon: {id}'s screen is {} MiB, over {} MiB; keeping the last one saved", snapshot.len() >> 20, MAX_SNAPSHOT >> 20);
        }
        return;
    }
    terminal.too_big = false;
    if let Err(error) = write_atomic(&snapshot_path(dir, id), &snapshot) {
        log!("kanna-daemon: couldn't save {id}'s screen: {error}");
        Feed::lock(&terminal.feed).unsaved = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kds{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(pane: &str, name: Option<&str>) -> Record {
        Record {
            id: format!("term-{}", pane_number(pane).unwrap()),
            pane: format!("term-{}", pane_number(pane).unwrap()),
            name: name.map(String::from),
            cmd: Some("top".into()),
            cwd: Some("/tmp".into()),
            state: PaneState::Running,
            exit_code: None,
            pid: 42,
            keeper: pane.into(),
            size: Some((120, 40)),
            hung_up: false,
        }
    }

    fn open(dir: &Path) -> Store {
        Store::open(dir, Rc::new(Db::open(dir).unwrap())).unwrap()
    }

    fn file(id: &str, pane: &str, keeper: &str, pid: i64) -> Value {
        json!({ "id": id, "pane": pane, "name": null, "type": "term", "cmd": null, "cwd": "/tmp", "state": "running", "exitCode": null, "pid": num(pid as f64), "keeper": keeper, "cols": 120, "rows": 40 })
    }

    fn version_3(terminals: Vec<Value>) -> String {
        json!({ "kanna": "terminals", "version": 3, "counters": { "pane": 0, "terminal": 7, "pty": 12 }, "terminals": Value::Array(terminals) }).to_string()
    }

    fn write_records(dir: &Path, text: &str) {
        fs::create_dir_all(dir.join("terminals")).unwrap();
        fs::write(dir.join("terminals").join(RECORDS), text).unwrap();
    }

    #[test]
    fn records_round_trip_in_the_db() {
        let dir = folder("records");
        let store = open(&dir);
        let mut exited = record("p3", None);
        (exited.state, exited.exit_code, exited.hung_up, exited.size) = (PaneState::Exited, Some(3), true, None);
        let saved = Saved { counter: 7, terminals: 9, ptys: 12, records: vec![record("p1", Some("logs")), exited] };
        store.save(&saved).unwrap();
        store.stop();
        drop(store);
        assert_eq!(open(&dir).load(), saved);
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir.join("terminals")), 0o700);
        assert_eq!(mode(&dir.join("kanna.db")), 0o600);
        assert!(!dir.join("terminals").join(RECORDS).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_writes_only_what_changed_and_keeps_keys_it_does_not_know() {
        let dir = folder("changed");
        let db = Rc::new(Db::open(&dir).unwrap());
        let store = Store::open(&dir, db.clone()).unwrap();
        let first = record("p1", None);
        store.save(&Saved { counter: 0, terminals: 1, ptys: 0, records: vec![first.clone()] }).unwrap();
        let (mut rows, counters) = db.terminals().unwrap();
        rows[0].doc["fromNewerKanna"] = json!(1);
        db.put_terminals(&[], &rows, counters).unwrap();
        let later = Record { id: "pty-2".into(), keeper: "pty-2".into(), ..first.clone() };
        store.save(&Saved { counter: 0, terminals: 1, ptys: 2, records: vec![later.clone()] }).unwrap();
        let (rows, counters) = db.terminals().unwrap();
        assert_eq!(rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["pty-2"]);
        assert_eq!(counters, TerminalCounters { pane: 0, terminal: 1, pty: 2 });
        let renamed = Record { name: Some("logs".into()), ..later };
        let (mut rows, _) = db.terminals().unwrap();
        rows[0].doc["fromNewerKanna"] = json!(1);
        db.put_terminals(&[], &rows, counters).unwrap();
        store.save(&Saved { counter: 0, terminals: 1, ptys: 2, records: vec![renamed] }).unwrap();
        let (rows, _) = db.terminals().unwrap();
        assert_eq!((rows[0].doc["fromNewerKanna"].clone(), rows[0].doc["name"].clone()), (json!(1), json!("logs")));
        store.stop();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_3_file_is_imported_once_with_its_ids_and_counters() {
        let dir = folder("import3");
        write_records(&dir, &version_3(vec![file("pty-12", "term-7", "pty-12", 42), file("term-3", "term-3", "term-3", 43)]));
        let store = open(&dir);
        let loaded = store.load();
        assert_eq!(loaded.records.iter().map(|r| (r.id.as_str(), r.pane.as_str(), r.keeper.as_str())).collect::<Vec<_>>(), [("term-3", "term-3", "term-3"), ("pty-12", "term-7", "pty-12")]);
        assert_eq!((loaded.terminals, loaded.ptys), (7, 12));
        assert!(!dir.join("terminals").join(RECORDS).exists());
        assert!(dir.join("terminals").join(IMPORTED).exists());
        store.save(&Saved { counter: 0, terminals: 7, ptys: 13, records: vec![] }).unwrap();
        store.stop();
        drop(store);
        write_records(&dir, &version_3(vec![file("pty-12", "term-7", "pty-12", 42)]));
        let again = open(&dir).load();
        assert_eq!((again.records.len(), again.ptys), (0, 13), "a second file doesn't overwrite the db");
        assert!(!dir.join("terminals").join(RECORDS).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_2_file_is_imported_with_its_panes_named_by_their_terminals() {
        let dir = folder("import2");
        write_records(&dir, &json!({ "kanna": "terminals", "version": 2, "counters": { "pane": 4, "terminal": 4 }, "terminals": [
            { "id": "term-4", "pane": "p4", "name": "logs", "type": "term", "cmd": "top", "cwd": "/tmp", "state": "running", "exitCode": null, "pid": 42, "keeper": "p4", "cols": 120, "rows": 40 }] }).to_string());
        assert_eq!(Store::content_ids(&dir), HashMap::from([("p4".to_string(), "term-4".to_string())]));
        let loaded = open(&dir).load();
        assert_eq!(loaded.records, vec![record("p4", Some("logs"))]);
        assert_eq!((loaded.counter, loaded.terminals), (4, 4));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_1_file_becomes_terminal_ids_and_its_screen_moves() {
        let dir = folder("version1");
        let old = json!({ "kanna": "terminals", "version": 1, "counters": { "pane": 4 }, "terminals": [
            { "id": "p4", "name": "logs", "type": "term", "cmd": "top", "cwd": "/tmp", "state": "running", "exitCode": null, "pid": 42, "keeper": "p4", "cols": 120, "rows": 40 }] });
        write_records(&dir, &old.to_string());
        fs::write(dir.join("terminals/p4.snap"), b"screen").unwrap();
        assert_eq!(Store::content_ids(&dir), HashMap::from([("p4".to_string(), "term-4".to_string())]));
        let store = open(&dir);
        assert_eq!(store.load().records, vec![record("p4", Some("logs"))]);
        assert_eq!(store.snapshot("term-4"), Some(b"screen".to_vec()));
        assert!(!store.dir().join("p4.snap").exists());
        store.stop();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pane_comes_back_with_its_own_id_behind_a_later_run() {
        let dir = folder("runs");
        let store = open(&dir);
        let record = Record { id: "pty-9".into(), pane: "term-4".into(), name: None, cmd: None, cwd: None, state: PaneState::Running, exit_code: None, pid: 7, keeper: "pty-9".into(), size: None, hung_up: false };
        store.save(&Saved { counter: 0, terminals: 4, ptys: 9, records: vec![record.clone()] }).unwrap();
        drop(store);
        assert_eq!(open(&dir).load().records, [record]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_db_lost_with_no_copy_takes_the_imported_file_and_skips_ids() {
        let dir = folder("lost");
        write_records(&dir, &version_3(vec![file("pty-12", "term-7", "pty-12", 42)]));
        drop(open(&dir));
        fs::remove_file(dir.join("kanna.db.bak")).unwrap();
        for file in ["kanna.db", "kanna.db-wal", "kanna.db-shm"] {
            let _ = fs::remove_file(dir.join(file));
        }
        fs::write(dir.join(crate::db::IMPORTED_BACKLOG), "{}").unwrap();
        let loaded = open(&dir).load();
        assert_eq!(loaded.records.iter().map(|r| r.pane.as_str()).collect::<Vec<_>>(), ["term-7"]);
        assert_eq!((loaded.terminals, loaded.ptys), (7 + ID_GAP, 12 + ID_GAP));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_db_restored_from_its_copy_skips_ids_and_keeps_only_what_the_copy_has() {
        let dir = folder("restored");
        write_records(&dir, &version_3(vec![file("pty-12", "term-7", "pty-12", 42)]));
        let store = open(&dir);
        drop(store);
        let store = open(&dir);
        let mut records = store.load().records;
        records.push(Record { id: "pty-13".into(), pane: "term-8".into(), keeper: "pty-13".into(), ..records[0].clone() });
        store.save(&Saved { counter: 0, terminals: 8, ptys: 13, records }).unwrap();
        drop(store);
        for file in ["kanna.db-wal", "kanna.db-shm"] {
            let _ = fs::remove_file(dir.join(file));
        }
        fs::write(dir.join("kanna.db"), "this is not a database".repeat(200)).unwrap();
        let loaded = open(&dir).load();
        assert_eq!(loaded.records.iter().map(|r| r.pane.as_str()).collect::<Vec<_>>(), ["term-7"]);
        assert_eq!((loaded.terminals, loaded.ptys), (7 + ID_GAP, 12 + ID_GAP));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_are_kept_privately() {
        let dir = folder("settings");
        let store = open(&dir);
        assert_eq!(store.load_settings(), Settings::default());
        let settings = Settings { graphemes: false, scrollback_lines: Some(9), ..Settings::default() };
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings(), settings);
        let mode = fs::metadata(store.dir().join(SETTINGS)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        store.prune(&[]);
        assert_eq!(store.load_settings(), settings);
        fs::write(store.dir().join(SETTINGS), "{").unwrap();
        assert_eq!(store.load_settings(), Settings::default());
        store.stop();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_file_is_moved_aside_and_not_read_again() {
        let dir = folder("bad");
        write_records(&dir, &version_3(vec![file("pty-1", "term-1", "pty-1", 1), file("pty-2", "term-1", "pty-2", 2)]));
        let store = open(&dir);
        assert_eq!(store.load(), Saved::default());
        assert!(store.dir().join(format!("{RECORDS}.bad")).exists());
        store.stop();
        drop(store);
        for broken in ["{", &json!({ "kanna": "terminals", "version": 9, "terminals": [] }).to_string()] {
            let (dir, name) = (folder(&format!("bad{}", broken.len())), RECORDS);
            write_records(&dir, broken);
            assert_eq!(open(&dir).load(), Saved::default());
            assert!(dir.join("terminals").join(format!("{name}.bad")).exists());
            let _ = fs::remove_dir_all(&dir);
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn screens_are_saved_when_they_change_and_kept_when_they_cannot_be() {
        let dir = folder("screens");
        let store = open(&dir);
        let feed = Feed::new(20, 4, &Settings::default()).unwrap();
        Feed::lock(&feed).push(b"first");
        store.track("p1", feed.clone());
        store.flush();
        let saved = || kanna_vt::Terminal::from_snapshot(&store.snapshot("p1").unwrap()).unwrap().text(kanna_vt::Lines::Screen);
        assert_eq!(saved(), "first");
        Feed::lock(&feed).push(&[b" second".as_slice(), &crate::reader::tests::overlong_osc()].concat());
        store.flush();
        assert_eq!(saved(), "first");
        assert!(Feed::lock(&feed).unsaved);
        Feed::lock(&feed).push(b"\x07 third");
        store.flush();
        assert_eq!(saved(), "first second third");
        let mode = fs::metadata(store.dir().join("p1.snap")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        store.forget("p1");
        store.flush();
        assert!(store.snapshot("p1").is_none());
        store.stop();
        let _ = fs::remove_dir_all(&dir);
    }
}
