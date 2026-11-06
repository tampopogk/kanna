use kanna_mux::json::Value;
use rusqlite::{params, Connection, OptionalExtension};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::cmp::Ordering;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const FILE: &str = "kanna.db";

const BACKUP: &str = "kanna.db.bak";

const BACKLOG: &str = "backlog.json";

pub const IMPORTED_BACKLOG: &str = "backlog.json.imported";

const MAX_BACKLOG: u64 = 4 << 20;

enum Step {
    Sql(&'static str),
    Run(fn(&Connection) -> rusqlite::Result<()>),
}

const MIGRATIONS: &[Step] = &[Step::Sql("
    create table meta (key text primary key, value text not null);
    create table tasks (
        id text primary key,
        repo text not null,
        place text not null check (place in ('backlog', 'shelf', 'desk', 'done')),
        pane text,
        made_at real not null,
        doc text not null default '{}' check (json_valid(doc))
    );
    create index tasks_by_place on tasks (place, repo);
    create table task_events (
        id integer primary key,
        task text not null references tasks (id),
        at real not null,
        what text not null,
        from_place text,
        to_place text,
        by text,
        doc text not null default '{}' check (json_valid(doc))
    );
    create index task_events_by_task on task_events (task, at);
    create index task_events_by_time on task_events (at);
"), Step::Sql("
    create table quiz_rounds (
        id integer primary key,
        repo text not null,
        at real not null,
        head text not null,
        doc text not null default '{}' check (json_valid(doc))
    );
    create index quiz_rounds_by_repo on quiz_rounds (repo, at);
    create table quiz_answers (
        id integer primary key,
        round integer not null references quiz_rounds (id),
        at real not null,
        kind text not null,
        file text not null,
        right integer not null check (right in (0, 1)),
        again integer references quiz_answers (id),
        doc text not null default '{}' check (json_valid(doc))
    );
    create index quiz_answers_by_round on quiz_answers (round);
    create index quiz_answers_by_again on quiz_answers (again);
"), Step::Run(renumber), Step::Sql("
    create table layout (
        id integer primary key check (id = 1),
        doc text not null check (json_valid(doc))
    );
"), Step::Sql("
    alter table tasks rename column pane to terminal;
"), Step::Sql("
    update tasks set place = 'shelf' where place = 'desk';
"), Step::Sql("
    create table messages (
        id integer primary key,
        recipient text not null,
        repo text not null,
        sender text not null,
        at real not null,
        delivered real,
        text text not null,
        mode text
    );
    create index messages_waiting on messages (delivered, recipient);
    create index messages_by_recipient on messages (recipient, at);
"), Step::Sql("
    create table projects (
        path text primary key,
        at real not null,
        seq integer not null
    );
"), Step::Sql("
    create table fleet (
        id integer primary key check (id = 1),
        doc text not null check (json_valid(doc))
    );
"), Step::Sql("
    alter table fleet rename to machines;
    update machines set doc = json_remove(doc, '$.anchor', '$.entries');
"), Step::Sql("
    create table planners (
        path text primary key,
        doc text not null check (json_valid(doc))
    );
"), Step::Sql("
    create table never_import (
        path text primary key,
        at real not null
    );
"), Step::Sql("
    update tasks set place = case place when 'shelf' then 'desk' when 'backlog' then 'shelf' else place end;
    update task_events set
        from_place = case from_place when 'shelf' then 'desk' when 'backlog' then 'shelf' else from_place end,
        to_place = case to_place when 'shelf' then 'desk' when 'backlog' then 'shelf' else to_place end,
        what = case what when 'shelved' then 'picked' when 'deferred' then 'shelved' when 'pulled' then 'activated' when 'parked' then 'left' else what end;
"), Step::Sql("
    create table crashes (
        file text primary key,
        signature text not null,
        at real not null,
        dismissed real,
        reported real
    );
"), Step::Sql("
    create table schedules (
        id integer primary key,
        repo text,
        name text,
        target text not null,
        rule text not null,
        prompt text not null,
        next real not null,
        message integer references messages (id),
        made_at real not null,
        doc text not null default '{}' check (json_valid(doc))
    );
    create unique index schedules_by_name on schedules (coalesce(repo, ''), name) where name is not null;
"), Step::Sql("
    create table runs (
        id text primary key,
        task text not null,
        repo text not null,
        pipeline text not null,
        revision_hash text not null,
        branch text not null,
        state text not null check (state in ('running', 'waiting', 'paused', 'done', 'stopped')),
        started_at real not null,
        ended_at real,
        outcome text,
        doc text not null default '{}' check (json_valid(doc))
    );
    create index runs_by_task on runs (task, started_at);
    create index runs_by_repo on runs (repo, started_at);
    create index runs_by_state on runs (state);
    create table run_revisions (
        run text not null references runs (id),
        n integer not null,
        at real not null,
        by text not null,
        hash text not null,
        doc text not null default '{}' check (json_valid(doc)),
        primary key (run, n)
    );
    create table run_stages (
        id integer primary key,
        run text not null references runs (id),
        stage text not null,
        visit integer not null,
        instance text,
        kind text not null,
        agent text,
        machine text not null,
        started_at real not null,
        ended_at real,
        verdict text,
        session text,
        terminal text,
        worktree text,
        branch text,
        doc text not null default '{}' check (json_valid(doc))
    );
    create index run_stages_by_run on run_stages (run, id);
    create index run_stages_open on run_stages (ended_at, run);
    create table run_events (
        id integer primary key,
        run text not null references runs (id),
        at real not null,
        what text not null,
        by text,
        doc text not null default '{}' check (json_valid(doc))
    );
    create index run_events_by_run on run_events (run, at);
"), Step::Sql("
    create table record_ops (
        device text not null,
        ms integer not null,
        n integer not null,
        task text not null,
        birth text not null,
        field text not null,
        value text not null check (json_valid(value)),
        primary key (device, ms, n)
    );
    create table authority (
        task text not null,
        birth text not null,
        epoch integer not null,
        permit text not null check (permit in ('ready', 'running', 'sent', 'acked')),
        recipient text,
        primary key (task, birth)
    );
"), Step::Sql("
    create table transfers (
        id text primary key,
        task text not null,
        role text not null check (role in ('out', 'in')),
        peer text not null,
        state text not null,
        doc text not null default '{}' check (json_valid(doc)),
        at real not null
    );
"), Step::Sql("
    create table samples (
        repo text not null,
        commit_id text not null,
        metric text not null,
        at real not null,
        value real not null,
        doc text not null default '{}' check (json_valid(doc)),
        primary key (repo, commit_id, metric)
    );
    create index samples_by_time on samples (repo, at);
"), Step::Sql("
    alter table samples add column revision integer not null default 0;
"), Step::Sql("
    alter table messages add column comment text check (comment is null or json_valid(comment));
"), Step::Sql("
    alter table schedules add column paused real;
    create table schedule_firings (
        id integer primary key,
        schedule integer not null references schedules (id) on delete cascade,
        at real not null,
        how text not null check (how in ('rule', 'run')),
        state text not null check (state in ('left', 'skipped', 'failed')),
        message integer references messages (id),
        detail text,
        reply text,
        replied_at real
    );
    create index schedule_firings_by_schedule on schedule_firings (schedule, id);
"), Step::Sql("
    create table phone_chat (
        repo text not null,
        seq integer not null,
        at real not null,
        kind text not null check (kind in ('me', 'say', 'ask', 'show', 'round', 'sys')),
        client text,
        message integer references messages (id),
        read integer not null default 0,
        doc text not null default '{}' check (json_valid(doc)),
        primary key (repo, seq)
    );
    create unique index phone_chat_once on phone_chat (repo, client) where client is not null;
"), Step::Sql("
    alter table messages add column client text;
    create unique index messages_once on messages (recipient, client) where client is not null;
    create table last_words (
        who text primary key,
        at real not null,
        asking integer not null default 0,
        text text not null
    );
"), Step::Sql("
    create table said (
        id integer primary key,
        who text not null,
        at real not null,
        asking integer not null default 0,
        text text not null
    );
    create index said_by_who on said (who, id);
    insert into said (who, at, asking, text) select who, at, asking, text from last_words;
    drop table last_words;
"), Step::Sql("
    create table status_reports (
        terminal text primary key,
        at real not null,
        doc text not null check (json_valid(doc))
    );
"), Step::Sql("
    alter table quiz_answers add column n integer;
    create unique index quiz_answers_by_question on quiz_answers (round, n) where n is not null;
    create table quiz_ready (
        round integer primary key references quiz_rounds (id),
        repo text not null,
        written real not null,
        doc text not null default '{}' check (json_valid(doc)),
        taken real
    );
    create index quiz_ready_by_repo on quiz_ready (repo, taken);
    create table quiz_map (
        repo text primary key,
        at real not null,
        doc text not null default '{}' check (json_valid(doc))
    );
    create table quiz_notes (
        repo text not null,
        subsystem text not null,
        at real not null,
        text text not null,
        primary key (repo, subsystem)
    );
"), Step::Sql("
    create table terminals (
        id text primary key,
        pane text not null unique,
        keeper text not null unique,
        state text not null,
        doc text not null default '{}' check (json_valid(doc))
    );
")];

const TERMINAL_COUNTERS: &str = "terminal_counters";

const TERMINALS_IMPORTED: &str = "terminals_imported";

pub const RECENT: usize = 10;
pub const SAID_KEPT: i64 = 100;

#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub at: f64,
    pub asking: bool,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhoneItem {
    pub seq: i64,
    pub at: f64,
    pub kind: String,
    pub client: Option<String>,
    pub message: Option<i64>,
    pub doc: Value,
}

pub const FIRINGS_KEPT: i64 = 20;

pub fn version() -> i64 {
    MIGRATIONS.len() as i64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Shelf,
    Desk,
    Done,
}

impl Place {
    pub fn as_str(self) -> &'static str {
        match self {
            Place::Shelf => "shelf",
            Place::Desk => "desk",
            Place::Done => "done",
        }
    }

    pub fn parse(text: &str) -> Option<Place> {
        [Place::Shelf, Place::Desk, Place::Done].into_iter().find(|p| p.as_str() == text)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub repo: String,
    pub place: Place,
    pub terminal: Option<String>,
    pub made_at: f64,
    pub doc: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub what: &'static str,
    pub from: Option<Place>,
    pub to: Option<Place>,
    pub by: Option<String>,
    pub at: Option<f64>,
    pub doc: Value,
}

impl Change {
    pub fn new(what: &'static str) -> Change {
        Change { what, from: None, to: None, by: None, at: None, doc: Value::Object(Default::default()) }
    }

    pub fn moved(mut self, from: Place, to: Place) -> Change {
        (self.from, self.to) = (Some(from), Some(to));
        self
    }

    pub fn by(mut self, by: Option<&str>) -> Change {
        self.by = by.map(String::from);
        self
    }

    pub fn with(mut self, key: &str, value: Value) -> Change {
        self.doc[key] = value;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Logged {
    pub task: String,
    pub at: f64,
    pub what: String,
    pub from: Option<Place>,
    pub to: Option<Place>,
    pub by: Option<String>,
    pub doc: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: i64,
    pub recipient: String,
    pub repo: String,
    pub sender: String,
    pub at: f64,
    pub delivered: Option<f64>,
    pub text: String,
    pub mode: Option<String>,
    pub comment: Option<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub file: String,
    pub thread: String,
}

impl Comment {
    fn json(&self) -> String {
        serde_json::json!({ "file": self.file, "thread": self.thread }).to_string()
    }

    fn parse(text: Option<String>) -> Option<Comment> {
        let doc: Value = serde_json::from_str(&text?).ok()?;
        Some(Comment { file: doc["file"].as_str()?.into(), thread: doc["thread"].as_str()?.into() })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Round {
    pub id: i64,
    pub repo: String,
    pub at: f64,
    pub head: String,
    pub doc: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ready {
    pub round: i64,
    pub repo: String,
    pub head: String,
    pub written: f64,
    pub doc: Value,
    pub taken: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub id: i64,
    pub round: i64,
    pub at: f64,
    pub kind: String,
    pub file: String,
    pub right: bool,
    pub again: Option<i64>,
    pub doc: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub id: i64,
    pub repo: Option<String>,
    pub name: Option<String>,
    pub target: String,
    pub rule: String,
    pub prompt: String,
    pub next: f64,
    pub message: Option<i64>,
    pub made_at: f64,
    pub paused: Option<f64>,
    pub doc: Value,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum How {
    Rule,
    Run,
}

impl How {
    pub fn as_str(self) -> &'static str {
        match self {
            How::Rule => "rule",
            How::Run => "run",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Left {
    Message(i64),
    Skipped,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Firing {
    pub id: i64,
    pub schedule: i64,
    pub at: f64,
    pub how: String,
    pub state: String,
    pub message: Option<i64>,
    pub delivered: Option<f64>,
    pub detail: Option<String>,
    pub reply: Option<String>,
    pub replied_at: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub commit: String,
    pub metric: String,
    pub at: f64,
    pub value: f64,
    pub doc: Value,
    pub revision: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Life {
    pub task: String,
    pub name: String,
    pub made: f64,
    pub started: Option<f64>,
    pub done: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerminalRow {
    pub id: String,
    pub pane: String,
    pub keeper: String,
    pub state: String,
    pub doc: Value,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TerminalCounters {
    pub pane: usize,
    pub terminal: usize,
    pub pty: usize,
}

impl TerminalCounters {
    fn json(self) -> String {
        serde_json::json!({ "pane": self.pane, "terminal": self.terminal, "pty": self.pty }).to_string()
    }

    fn parse(text: &str) -> TerminalCounters {
        let doc: Value = serde_json::from_str(text).unwrap_or(Value::Null);
        let count = |key: &str| doc[key].as_u64().unwrap_or(0) as usize;
        TerminalCounters { pane: count("pane"), terminal: count("terminal"), pty: count("pty") }
    }
}

pub struct Db {
    conn: Connection,
    layout: RefCell<Option<Box<dyn Fn() -> Option<Value>>>>,
    checkpoints: Option<Checkpoints>,
    lost: bool,
}

struct Checkpoints {
    stop: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Checkpoints {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

const CHECKPOINT_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

fn checkpointing(path: PathBuf) -> Option<Checkpoints> {
    let (stop, stopping) = std::sync::mpsc::channel::<()>();
    let started = std::thread::Builder::new().name("kanna.checkpoint".into()).spawn(move || {
        let Ok(conn) = Connection::open(&path) else { return };
        let _ = conn.busy_timeout(std::time::Duration::from_secs(1));
        while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopping.recv_timeout(CHECKPOINT_EVERY) {
            let started = std::time::Instant::now();
            let _ = conn.query_row("pragma wal_checkpoint(passive)", [], |_| Ok(()));
            crate::perf::since("db.checkpoint", started);
        }
    });
    started.ok().map(|thread| Checkpoints { stop: Some(stop), thread: Some(thread) })
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn stamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).to_string()
}

fn say(message: String) {
    log!("kanna-daemon: {message}");
}

fn aside(path: &Path, suffix: &str) -> PathBuf {
    let to = path.with_file_name(format!("{}.{suffix}", path.file_name().and_then(|n| n.to_str()).unwrap_or(FILE)));
    for extra in ["-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{extra}", path.display()));
        if from.exists() {
            let _ = fs::rename(&from, PathBuf::from(format!("{}{extra}", to.display())));
        }
    }
    let _ = fs::rename(path, &to);
    to
}

fn connect(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "wal")?;
    conn.pragma_update(None, "synchronous", "normal")?;
    conn.pragma_update(None, "foreign_keys", "on")?;
    Ok(conn)
}

enum Opened {
    Good(Db),
    Newer(i64),
    Bad(String),
}

fn open_checked(path: &Path) -> Opened {
    let conn = match connect(path) {
        Ok(conn) => conn,
        Err(error) => return Opened::Bad(error.to_string()),
    };
    let check: rusqlite::Result<String> = conn.query_row("pragma quick_check", [], |row| row.get(0));
    match check {
        Ok(ok) if ok == "ok" => {}
        Ok(problem) => return Opened::Bad(problem),
        Err(error) => return Opened::Bad(error.to_string()),
    }
    let found: i64 = match conn.query_row("pragma user_version", [], |row| row.get(0)) {
        Ok(found) => found,
        Err(error) => return Opened::Bad(error.to_string()),
    };
    if found > version() {
        return Opened::Newer(found);
    }
    let mut db = Db { conn, layout: RefCell::default(), checkpoints: None, lost: false };
    match db.migrate(found) {
        Ok(()) => Opened::Good(db),
        Err(error) => Opened::Bad(format!("migrating from version {found}: {error}")),
    }
}

impl Db {
    pub fn open(state_dir: &Path) -> Result<Db, String> {
        let path = state_dir.join(FILE);
        let backup = state_dir.join(BACKUP);
        let mut lost = false;
        let mut db = None;
        if path.exists() {
            match open_checked(&path) {
                Opened::Good(good) => db = Some(good),
                Opened::Newer(found) => {
                    let to = aside(&path, "newer");
                    say(format!("{} is from a newer Kanna (version {found}, this one reads {}); moved it to {} and left it alone", path.display(), version(), to.display()));
                    lost = true;
                }
                Opened::Bad(why) => {
                    let to = aside(&path, &format!("bad-{}", stamp()));
                    say(format!("{} can't be used ({why}); moved it to {}", path.display(), to.display()));
                    lost = true;
                }
            }
        } else if backup.exists() || state_dir.join(IMPORTED_BACKLOG).exists() {
            say(format!("{} is missing", path.display()));
            lost = true;
        }
        if db.is_none() && backup.exists() {
            let restored = fs::copy(&backup, &path).map_err(|e| e.to_string()).map(|_| open_checked(&path));
            match restored {
                Ok(Opened::Good(good)) => {
                    say(format!("restored {} from {}, the copy made when the daemon last started; changes since then are lost", path.display(), backup.display()));
                    db = Some(good);
                }
                Ok(Opened::Newer(_)) | Ok(Opened::Bad(_)) | Err(_) => {
                    aside(&path, &format!("bad-{}", stamp()));
                    say(format!("{} can't be used either", backup.display()));
                }
            }
        }
        let mut db = match db {
            Some(db) => db,
            None => Db::fresh(&path, state_dir, lost)?,
        };
        db.lost = lost;
        db.absorb(state_dir);
        if let Err(error) = db.conn.backup(rusqlite::MAIN_DB, &backup, None) {
            say(format!("couldn't copy {} to {}: {error}", path.display(), backup.display()));
        } else {
            let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o600));
        }
        if db.conn.path().is_some_and(|file| !file.is_empty()) && db.conn.pragma_update(None, "wal_autocheckpoint", 0).is_ok() {
            db.checkpoints = checkpointing(path);
        }
        Ok(db)
    }

    pub fn lost(&self) -> bool {
        self.lost
    }

    fn fresh(path: &Path, state_dir: &Path, lost: bool) -> Result<Db, String> {
        let conn = connect(path).map_err(|error| format!("can't make {}: {error}", path.display()))?;
        let mut db = Db { conn, layout: RefCell::default(), checkpoints: None, lost: false };
        db.migrate(0).map_err(|error| format!("can't make {}: {error}", path.display()))?;
        let imported = state_dir.join(IMPORTED_BACKLOG);
        if lost && !state_dir.join(BACKLOG).exists() {
            match imported.exists() {
                true => match read_backlog(&imported).and_then(|saved| db.import(&saved).map_err(|e| e.to_string())) {
                    Ok(count) => say(format!("started a new {} from {} ({count} tasks); tasks made or changed since it was imported are lost", path.display(), imported.display())),
                    Err(error) => say(format!("started a new, empty {}: {} can't be read ({error})", path.display(), imported.display())),
                },
                false => say(format!("started a new, empty {}: there's no copy of the tasks to restore", path.display())),
            }
        }
        Ok(db)
    }

    fn absorb(&self, state_dir: &Path) {
        let file = state_dir.join(BACKLOG);
        if !file.exists() {
            return;
        }
        let imported = match read_backlog(&file).and_then(|saved| self.import(&saved).map_err(|e| e.to_string())) {
            Ok(count) => count,
            Err(error) => return say(format!("can't import {} ({error}); left it where it is", file.display())),
        };
        let to = state_dir.join(IMPORTED_BACKLOG);
        match fs::rename(&file, &to) {
            Ok(()) => say(format!("imported {imported} tasks from {} into {FILE}; the file is now {}", file.display(), to.display())),
            Err(error) => say(format!("imported {imported} tasks from {} into {FILE}, but can't rename it ({error})", file.display())),
        }
    }

    fn migrate(&mut self, from: i64) -> rusqlite::Result<()> {
        for (number, step) in MIGRATIONS.iter().enumerate().skip(from as usize) {
            let tx = self.conn.transaction()?;
            match step {
                Step::Sql(sql) => tx.execute_batch(sql)?,
                Step::Run(run) => run(&tx)?,
            }
            tx.pragma_update(None, "user_version", number as i64 + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn import(&self, saved: &Value) -> rusqlite::Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0;
        let tasks = saved.get("tasks").and_then(Value::as_array).into_iter().flatten().map(|t| (t, Place::Shelf, t.get("held")));
        let running = saved.get("running").and_then(Value::as_array).into_iter().flatten().map(|t| (t, Place::Desk, t.get("pane")));
        for (task, place, pane) in tasks.chain(running) {
            let Some(row) = row_from_saved(task, place, pane.and_then(Value::as_str)) else { continue };
            if tx.query_row("select 1 from task_events where what = 'renumbered' and json_extract(doc, '$.from') = ?1", [&row.id], |_| Ok(())).optional()?.is_some() {
                continue;
            }
            let added = tx.execute(
                "insert or ignore into tasks (id, repo, place, terminal, made_at, doc) values (?1, ?2, ?3, ?4, ?5, ?6)",
                params![row.id, row.repo, row.place.as_str(), row.terminal, row.made_at, row.doc.to_string()],
            )?;
            if added == 1 {
                let made = Change { at: Some(row.made_at), by: row.doc.get("by").and_then(Value::as_str).map(String::from), to: Some(Place::Shelf), ..Change::new("made") };
                log(&tx, &row.id, &made)?;
                if place == Place::Desk {
                    log(&tx, &row.id, &Change::new("imported").moved(Place::Shelf, Place::Desk).by(Some("daemon")))?;
                }
                count += 1;
            }
        }
        renumber(&tx)?;
        tx.commit()?;
        Ok(count)
    }

    pub fn tasks(&self) -> rusqlite::Result<Vec<Row>> {
        let mut statement = self.conn.prepare("select id, repo, place, terminal, made_at, doc from tasks where place != 'done'")?;
        let rows = statement.query_map([], |r| {
            let place: String = r.get(2)?;
            let doc: String = r.get(5)?;
            Ok(Row { id: r.get(0)?, repo: r.get(1)?, place: Place::parse(&place).unwrap_or(Place::Shelf), terminal: r.get(3)?, made_at: r.get(4)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) })
        })?;
        let mut rows = rows.collect::<rusqlite::Result<Vec<Row>>>()?;
        rows.sort_by(|a, b| oldest_first((a.made_at, &a.id), (b.made_at, &b.id)));
        Ok(rows)
    }

    pub fn done_tasks(&self) -> rusqlite::Result<Vec<(Row, f64)>> {
        let mut statement = self.conn.prepare(
            "select t.id, t.repo, t.terminal, t.made_at, t.doc, coalesce((select max(at) from task_events e where e.task = t.id and e.what = 'done'), t.made_at) as done_at \
             from tasks t where t.place = 'done' order by done_at desc, t.rowid desc",
        )?;
        let rows = statement.query_map([], |r| {
            let doc: String = r.get(4)?;
            Ok((Row { id: r.get(0)?, repo: r.get(1)?, place: Place::Done, terminal: r.get(2)?, made_at: r.get(3)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) }, r.get(5)?))
        })?;
        rows.collect()
    }

    pub fn made_order(&self) -> rusqlite::Result<Vec<String>> {
        let mut statement = self.conn.prepare("select id from tasks where place != 'done' order by made_at, rowid")?;
        let ids = statement.query_map([], |r| r.get(0))?;
        ids.collect()
    }

    pub fn ids_starting(&self, start: &str) -> rusqlite::Result<Vec<String>> {
        let mut statement = self.conn.prepare("select id from tasks where substr(id, 1, length(?1)) = ?1 order by made_at, rowid")?;
        let ids = statement.query_map([start], |r| r.get(0))?;
        ids.collect()
    }

    pub fn task(&self, id: &str) -> rusqlite::Result<Option<Row>> {
        self.conn
            .query_row("select id, repo, place, terminal, made_at, doc from tasks where id = ?1", [id], |r| {
                let place: String = r.get(2)?;
                let doc: String = r.get(5)?;
                Ok(Row { id: r.get(0)?, repo: r.get(1)?, place: Place::parse(&place).unwrap_or(Place::Shelf), terminal: r.get(3)?, made_at: r.get(4)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) })
            })
            .optional()
    }

    pub fn new_id(&self, candidates: Vec<String>, free: impl Fn(&str) -> bool) -> rusqlite::Result<String> {
        for id in candidates {
            if self.conn.query_row("select 1 from tasks where id = ?1", [&id], |_| Ok(())).optional()?.is_none() && free(&id) {
                return Ok(id);
            }
        }
        Err(rusqlite::Error::InvalidParameterName("every task ID tried was taken".into()))
    }

    pub fn put(&self, row: &Row, change: Option<&Change>) -> rusqlite::Result<()> {
        self.put_with(row, change, |_| Ok(()))
    }

    pub fn put_with(&self, row: &Row, change: Option<&Change>, also: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        put_row(&tx, row, change)?;
        also(&tx)?;
        if row.place == Place::Done {
            if let Some(layout) = self.layout.borrow().as_ref().and_then(|now| now()) {
                put_layout(&tx, &layout)?;
            }
        }
        tx.commit()
    }

    pub fn atomically<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let tx = self.conn.unchecked_transaction()?;
        let done = work(&tx)?;
        tx.commit()?;
        Ok(done)
    }

    pub fn saves_layout_with_done(&self, now: impl Fn() -> Option<Value> + 'static) {
        *self.layout.borrow_mut() = Some(Box::new(now));
    }

    pub fn layout(&self) -> rusqlite::Result<Option<Value>> {
        let doc: Option<String> = self.conn.query_row("select doc from layout where id = 1", [], |row| row.get(0)).optional()?;
        Ok(doc.and_then(|doc| serde_json::from_str(&doc).ok()))
    }

    pub fn rename_terminals(&self, renamed: &HashMap<String, String>) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (from, to) in renamed {
            tx.execute("update tasks set terminal = ?2 where terminal = ?1", params![from, to])?;
        }
        tx.commit()
    }

    pub fn machines(&self) -> rusqlite::Result<Value> {
        let doc: Option<String> = self.conn.query_row("select doc from machines where id = 1", [], |row| row.get(0)).optional()?;
        Ok(doc.and_then(|doc| serde_json::from_str(&doc).ok()).unwrap_or_else(|| Value::Object(Default::default())))
    }

    pub fn put_machines(&self, doc: &Value) -> rusqlite::Result<()> {
        self.conn.execute("insert into machines (id, doc) values (1, ?1) on conflict (id) do update set doc = excluded.doc", params![doc.to_string()]).map(|_| ())
    }

    pub fn put_layout(&self, doc: &Value) -> rusqlite::Result<()> {
        put_layout(&self.conn, doc)
    }

    pub fn planner(&self, path: &str) -> rusqlite::Result<Option<Value>> {
        let doc: Option<String> = self.conn.query_row("select doc from planners where path = ?1", [path], |row| row.get(0)).optional()?;
        Ok(doc.and_then(|doc| serde_json::from_str(&doc).ok()))
    }

    pub fn put_planner(&self, path: &str, doc: &Value) -> rusqlite::Result<()> {
        self.conn.execute("insert into planners (path, doc) values (?1, ?2) on conflict (path) do update set doc = excluded.doc", params![path, doc.to_string()]).map(|_| ())
    }

    pub fn reports(&self) -> rusqlite::Result<Vec<(String, Value)>> {
        let mut statement = self.conn.prepare("select terminal, doc from status_reports")?;
        let rows = statement.query_map([], |r| Ok((r.get(0)?, doc_of(r.get(1)?))))?;
        rows.collect()
    }

    pub fn put_reports(&self, rows: &[(String, Option<Value>)]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (terminal, doc) in rows {
            match doc {
                Some(doc) => tx.execute("insert into status_reports (terminal, at, doc) values (?1, ?2, ?3) on conflict (terminal) do update set at = excluded.at, doc = excluded.doc", params![terminal, now(), doc.to_string()])?,
                None => tx.execute("delete from status_reports where terminal = ?1", [terminal])?,
            };
        }
        tx.commit()
    }

    pub fn keep_reports(&self, terminals: &[String]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let kept: Vec<String> = {
            let mut statement = tx.prepare("select terminal from status_reports")?;
            let rows = statement.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for terminal in kept.iter().filter(|t| !terminals.contains(t)) {
            tx.execute("delete from status_reports where terminal = ?1", [terminal])?;
        }
        tx.commit()
    }

    pub fn log(&self, task: &str, change: &Change) -> rusqlite::Result<()> {
        log(&self.conn, task, change)
    }

    pub fn ended_tips(&self) -> rusqlite::Result<Vec<(String, String, Option<String>, String, String)>> {
        let mut statement = self.conn.prepare(
            "select t.id, t.repo, json_extract(t.doc, '$.name'), json_extract(e.doc, '$.branch'), json_extract(e.doc, '$.tip')
             from task_events e join tasks t on t.id = e.task
             where t.place = 'done' and json_extract(e.doc, '$.tip') is not null and json_extract(e.doc, '$.branch') is not null
             order by e.at",
        )?;
        let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        rows.collect()
    }

    pub fn events(&self, task: &str) -> rusqlite::Result<Vec<Logged>> {
        let mut statement = self.conn.prepare("select task, at, what, from_place, to_place, by, doc from task_events where task = ?1 order by at, id")?;
        let rows = statement.query_map([task], |r| {
            let place = |i: usize| r.get::<_, Option<String>>(i).map(|p| p.as_deref().and_then(Place::parse));
            let doc: String = r.get(6)?;
            Ok(Logged { task: r.get(0)?, at: r.get(1)?, what: r.get(2)?, from: place(3)?, to: place(4)?, by: r.get(5)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) })
        })?;
        rows.collect()
    }

    pub fn put_message(&self, recipient: &str, repo: &str, sender: &str, text: &str, mode: Option<&str>, comment: Option<&Comment>) -> rusqlite::Result<Message> {
        let at = now();
        self.conn.execute(
            "insert into messages (recipient, repo, sender, at, text, mode, comment) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![recipient, repo, sender, at, text, mode, comment.map(Comment::json)],
        )?;
        Ok(Message { id: self.conn.last_insert_rowid(), recipient: recipient.into(), repo: repo.into(), sender: sender.into(), at, delivered: None, text: text.into(), mode: mode.map(String::from), comment: comment.cloned() })
    }

    pub fn waiting_messages(&self) -> rusqlite::Result<Vec<Message>> {
        self.select_messages(&format!("select {MESSAGE} from messages where delivered is null order by at, id"), [])
    }

    pub fn messages(&self, recipient: Option<&str>) -> rusqlite::Result<Vec<Message>> {
        match recipient {
            Some(to) => self.select_messages(&format!("select {MESSAGE} from messages where recipient = ?1 or sender = ?1 order by at, id"), [to]),
            None => self.select_messages(&format!("select {MESSAGE} from messages order by at, id"), []),
        }
    }

    pub fn set_message_client(&self, id: i64, client: &str) -> rusqlite::Result<()> {
        self.conn.execute("update messages set client = ?2 where id = ?1", params![id, client]).map(|_| ())
    }

    pub fn message_of(&self, client: &str) -> rusqlite::Result<Option<Message>> {
        Ok(self.select_messages(&format!("select {MESSAGE} from messages where client = ?1"), [client])?.into_iter().next())
    }

    pub fn put_said(&self, who: &str, text: &str, asking: bool) -> rusqlite::Result<()> {
        let newest = self.conn.query_row("select id, asking, text from said where who = ?1 order by id desc limit 1", [who], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?, r.get::<_, String>(2)?))).optional()?;
        match newest {
            Some((_, was, same)) if same == text && was == asking => Ok(()),
            Some((id, _, same)) if same == text => self.conn.execute("update said set at = ?2, asking = ?3 where id = ?1", params![id, now(), asking]).map(|_| ()),
            _ => {
                self.conn.execute("insert into said (who, at, asking, text) values (?1, ?2, ?3, ?4)", params![who, now(), asking, text])?;
                self.conn.execute("delete from said where who = ?1 and id not in (select id from said where who = ?1 order by id desc limit ?2)", params![who, SAID_KEPT]).map(|_| ())
            }
        }
    }

    pub fn said(&self, who: &str) -> rusqlite::Result<Vec<Said>> {
        self.conn.prepare("select at, asking, text from said where who = ?1 order by id")?.query_map([who], |r| Ok(Said { at: r.get(0)?, asking: r.get(1)?, text: r.get(2)? }))?.collect()
    }

    pub fn message(&self, id: i64) -> rusqlite::Result<Option<Message>> {
        Ok(self.select_messages(&format!("select {MESSAGE} from messages where id = ?1"), [id])?.into_iter().next())
    }

    fn phone_item_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PhoneItem> {
        let doc: String = r.get(5)?;
        Ok(PhoneItem { seq: r.get(0)?, at: r.get(1)?, kind: r.get(2)?, client: r.get(3)?, message: r.get(4)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) })
    }

    pub fn put_phone_item(&self, repo: &str, kind: &str, client: Option<&str>, message: Option<i64>, doc: &Value) -> rusqlite::Result<PhoneItem> {
        let at = now();
        let seq: i64 = self.conn.query_row("insert into phone_chat (repo, seq, at, kind, client, message, doc) values (?1, (select coalesce(max(seq), 0) + 1 from phone_chat where repo = ?1), ?2, ?3, ?4, ?5, ?6) returning seq", params![repo, at, kind, client, message, doc.to_string()], |r| r.get(0))?;
        Ok(PhoneItem { seq, at, kind: kind.to_string(), client: client.map(String::from), message, doc: doc.clone() })
    }

    pub fn phone_items(&self, repo: &str, after: i64) -> rusqlite::Result<Vec<PhoneItem>> {
        let mut statement = self.conn.prepare("select seq, at, kind, client, message, doc from phone_chat where repo = ?1 and seq > ?2 order by seq")?;
        let rows = statement.query_map(params![repo, after], Self::phone_item_row)?;
        rows.collect()
    }

    pub fn phone_asked(&self, thread: &str) -> rusqlite::Result<Option<f64>> {
        self.conn.query_row("select at from phone_chat where repo = ?1 and seq = (select max(seq) from phone_chat where repo = ?1) and kind = 'ask' and json_extract(doc, '$.chosen') is null", [thread], |r| r.get(0)).optional()
    }

    pub fn phone_item(&self, repo: &str, seq: i64) -> rusqlite::Result<Option<PhoneItem>> {
        self.conn.query_row("select seq, at, kind, client, message, doc from phone_chat where repo = ?1 and seq = ?2", params![repo, seq], Self::phone_item_row).optional()
    }

    pub fn phone_item_of(&self, repo: &str, client: &str) -> rusqlite::Result<Option<PhoneItem>> {
        self.conn.query_row("select seq, at, kind, client, message, doc from phone_chat where repo = ?1 and client = ?2", params![repo, client], Self::phone_item_row).optional()
    }

    pub fn set_phone_doc(&self, repo: &str, seq: i64, doc: &Value) -> rusqlite::Result<()> {
        self.conn.execute("update phone_chat set doc = ?3 where repo = ?1 and seq = ?2", params![repo, seq, doc.to_string()]).map(|_| ())
    }

    pub fn phone_unread(&self, repo: &str) -> rusqlite::Result<Vec<PhoneItem>> {
        let mut statement = self.conn.prepare("select seq, at, kind, client, message, doc from phone_chat where repo = ?1 and kind = 'me' and read = 0 order by seq")?;
        let rows = statement.query_map([repo], Self::phone_item_row)?;
        rows.collect()
    }

    pub fn phone_waiting(&self, repo: &str) -> rusqlite::Result<bool> {
        self.conn.query_row("select exists (select 1 from phone_chat p join messages m on m.id = p.message where p.repo = ?1 and m.delivered is null)", [repo], |r| r.get(0))
    }

    pub fn set_phone_read(&self, repo: &str, seq: i64) -> rusqlite::Result<()> {
        self.conn.execute("update phone_chat set read = 1 where repo = ?1 and seq = ?2", params![repo, seq]).map(|_| ())
    }

    pub fn delivered(&self, ids: &[i64]) -> rusqlite::Result<()> {
        let at = now();
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            tx.execute("update messages set delivered = ?2 where id = ?1", params![id, at])?;
        }
        tx.commit()
    }

    fn select_messages<P: rusqlite::Params>(&self, sql: &str, params: P) -> rusqlite::Result<Vec<Message>> {
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map(params, |r| Ok(Message { id: r.get(0)?, recipient: r.get(1)?, repo: r.get(2)?, sender: r.get(3)?, at: r.get(4)?, delivered: r.get(5)?, text: r.get(6)?, mode: r.get(7)?, comment: Comment::parse(r.get(8)?) }))?;
        rows.collect()
    }

    pub fn start_round(&self, repo: &str, head: &str, doc: &Value) -> rusqlite::Result<i64> {
        self.conn.execute("insert into quiz_rounds (repo, at, head, doc) values (?1, ?2, ?3, json_patch('{}', ?4))", params![repo, now(), head, doc.to_string()])?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn round(&self, id: i64) -> rusqlite::Result<Option<Round>> {
        self.conn.query_row("select id, repo, at, head, doc from quiz_rounds where id = ?1", [id], round_row).optional()
    }

    pub fn answer(&self, round: i64, kind: &str, file: &str, right: bool, again: Option<i64>, doc: &Value) -> rusqlite::Result<i64> {
        self.conn.execute(
            "insert into quiz_answers (round, at, kind, file, right, again, doc) values (?1, ?2, ?3, ?4, ?5, ?6, json_patch('{}', ?7))",
            params![round, now(), kind, file, right, again, doc.to_string()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn answer_question(&self, round: i64, n: i64, kind: &str, file: &str, right: bool, again: Option<i64>, doc: &Value) -> rusqlite::Result<(i64, bool)> {
        let added = self.conn.execute(
            "insert into quiz_answers (round, at, kind, file, right, again, doc, n) values (?1, ?2, ?3, ?4, ?5, ?6, json_patch('{}', ?7), ?8) on conflict (round, n) where n is not null do nothing",
            params![round, now(), kind, file, right, again, doc.to_string(), n],
        )?;
        let id = self.conn.query_row("select id from quiz_answers where round = ?1 and n = ?2", params![round, n], |r| r.get(0))?;
        Ok((id, added > 0))
    }

    pub fn put_ready(&self, repo: &str, head: &str, round_doc: &Value, doc: &Value) -> rusqlite::Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        let written = now();
        tx.execute("insert into quiz_rounds (repo, at, head, doc) values (?1, ?2, ?3, json_patch('{}', ?4))", params![repo, written, head, round_doc.to_string()])?;
        let round = tx.last_insert_rowid();
        tx.execute("insert into quiz_ready (round, repo, written, doc) values (?1, ?2, ?3, ?4)", params![round, repo, written, doc.to_string()])?;
        tx.commit()?;
        Ok(round)
    }

    pub fn ready(&self, round: i64) -> rusqlite::Result<Option<Ready>> {
        self.conn.query_row(&format!("{READY} where q.round = ?1"), [round], ready_row).optional()
    }

    pub fn open_rounds(&self, repo: &str) -> rusqlite::Result<Vec<Ready>> {
        let mut statement = self.conn.prepare(&format!("{READY} where q.repo = ?1 and (q.taken is null or (select count(*) from quiz_answers a where a.round = q.round and a.n is not null) < json_array_length(q.doc, '$.questions')) order by q.round"))?;
        let rows = statement.query_map([repo], ready_row)?;
        rows.collect()
    }

    pub fn round_waits(&self, repo: &str) -> rusqlite::Result<bool> {
        self.conn.prepare("select 1 from quiz_ready where repo = ?1 and taken is null and json_type(doc, '$.about') is not 'text'")?.exists([repo])
    }

    pub fn take_ready(&self, round: i64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("update quiz_ready set taken = ?2 where round = ?1 and taken is null", params![round, now()])? > 0)
    }

    pub fn picks(&self, round: i64) -> rusqlite::Result<Vec<(i64, i64)>> {
        let mut statement = self.conn.prepare("select n, coalesce(json_extract(doc, '$.pick'), 0) from quiz_answers where round = ?1 and n is not null order by n")?;
        let rows = statement.query_map([round], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn quiz_map(&self, repo: &str) -> rusqlite::Result<Option<Value>> {
        self.conn.query_row("select doc from quiz_map where repo = ?1", [repo], |r| Ok(doc_of(r.get(0)?))).optional()
    }

    pub fn set_quiz_map(&self, repo: &str, doc: &Value) -> rusqlite::Result<()> {
        self.conn.execute("insert into quiz_map (repo, at, doc) values (?1, ?2, ?3) on conflict (repo) do update set at = excluded.at, doc = excluded.doc", params![repo, now(), doc.to_string()])?;
        Ok(())
    }

    pub fn quiz_notes(&self, repo: &str) -> rusqlite::Result<Vec<(String, String)>> {
        let mut statement = self.conn.prepare("select subsystem, text from quiz_notes where repo = ?1 order by subsystem")?;
        let rows = statement.query_map([repo], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn set_quiz_note(&self, repo: &str, subsystem: &str, text: &str) -> rusqlite::Result<()> {
        self.conn.execute("insert into quiz_notes (repo, subsystem, at, text) values (?1, ?2, ?3, ?4) on conflict (repo, subsystem) do update set at = excluded.at, text = excluded.text", params![repo, subsystem, now(), text])?;
        Ok(())
    }

    pub fn find_answer(&self, id: i64) -> rusqlite::Result<Option<(Answer, String)>> {
        self.conn
            .query_row(&format!("select {ANSWER}, r.repo from quiz_answers a join quiz_rounds r on r.id = a.round where a.id = ?1"), [id], |r| Ok((answer_row(r)?, r.get(8)?)))
            .optional()
    }

    pub fn rounds(&self, repo: &str) -> rusqlite::Result<Vec<Round>> {
        let mut statement = self.conn.prepare("select id, repo, at, head, doc from quiz_rounds where repo = ?1 order by id desc")?;
        let rows = statement.query_map([repo], round_row)?;
        rows.collect()
    }

    pub fn answers(&self, repo: &str, misses: bool) -> rusqlite::Result<Vec<Answer>> {
        let only = if misses { "and a.right = 0 and not exists (select 1 from quiz_answers later where later.again = a.id)" } else { "" };
        let mut statement = self.conn.prepare(&format!("select {ANSWER} from quiz_answers a join quiz_rounds r on r.id = a.round where r.repo = ?1 {only} order by a.round desc, a.id"))?;
        let rows = statement.query_map([repo], answer_row)?;
        rows.collect()
    }

    pub fn sampled(&self, repo: &str, since: f64) -> rusqlite::Result<HashMap<(String, String), i64>> {
        let mut statement = self.conn.prepare("select commit_id, metric, revision from samples where repo = ?1 and at >= ?2")?;
        let rows = statement.query_map(params![repo, since], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?;
        rows.collect()
    }

    pub fn samples(&self, repo: &str, since: f64) -> rusqlite::Result<Vec<Sample>> {
        let mut statement = self.conn.prepare("select commit_id, metric, at, value, doc, revision from samples where repo = ?1 and at >= ?2 order by at, commit_id, metric")?;
        let rows = statement.query_map(params![repo, since], |r| Ok(Sample { commit: r.get(0)?, metric: r.get(1)?, at: r.get(2)?, value: r.get(3)?, doc: doc_of(r.get(4)?), revision: r.get(5)? }))?;
        rows.collect()
    }

    pub fn put_samples(&self, repo: &str, samples: &[Sample]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for sample in samples {
            tx.execute(
                "insert or replace into samples (repo, commit_id, metric, at, value, doc, revision) values (?1, ?2, ?3, ?4, ?5, json_patch('{}', ?6), ?7)",
                params![repo, sample.commit, sample.metric, sample.at, sample.value, sample.doc.to_string(), sample.revision],
            )?;
        }
        tx.commit()
    }

    pub fn latest_sample(&self, repo: &str, metric: &str) -> rusqlite::Result<Option<Sample>> {
        self.conn
            .query_row("select commit_id, metric, at, value, doc, revision from samples where repo = ?1 and metric = ?2 order by at desc, rowid desc limit 1", params![repo, metric], |r| Ok(Sample { commit: r.get(0)?, metric: r.get(1)?, at: r.get(2)?, value: r.get(3)?, doc: doc_of(r.get(4)?), revision: r.get(5)? }))
            .optional()
    }

    pub fn branches(&self, repo: &str) -> rusqlite::Result<HashMap<String, String>> {
        let mut statement = self.conn.prepare(
            "select json_extract(e.doc, '$.branch'), coalesce(json_extract(t.doc, '$.name'), ''), coalesce(json_extract(t.doc, '$.message'), '')
             from task_events e join tasks t on t.id = e.task
             where t.repo = ?1 and e.what = 'done' and json_extract(e.doc, '$.branch') is not null",
        )?;
        let rows = statement.query_map([repo], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        rows.map(|row| row.map(|(branch, name, message)| (branch, format!("{name}. {}", kanna_quality::szz::first_sentence(&message))))).collect()
    }

    pub fn lives(&self, repo: &str, since: f64) -> rusqlite::Result<Vec<Life>> {
        let mut statement = self.conn.prepare(
            "select id, name, made, started, done from (
                 select t.id, coalesce(json_extract(t.doc, '$.name'), t.id) as name, t.made_at,
                        coalesce((select min(at) from task_events where task = t.id and what = 'made'), t.made_at) as made,
                        (select min(at) from task_events where task = t.id and what = 'started') as started,
                        coalesce((select min(at) from task_events where task = t.id and what = 'done'),
                                 case when t.place = 'done' then coalesce((select max(at) from task_events where task = t.id), t.made_at) end) as done
                 from tasks t where t.repo = ?1)
             where made >= ?2 or started >= ?2 or done is null or done >= ?2
             order by made_at, id",
        )?;
        let rows = statement.query_map(params![repo, since], |r| Ok(Life { task: r.get(0)?, name: r.get(1)?, made: r.get(2)?, started: r.get(3)?, done: r.get(4)? }))?;
        rows.collect()
    }

    #[cfg(test)]
    pub fn run(&self, sql: &str) {
        self.conn.execute_batch(sql).unwrap();
    }

    pub fn touch_project(&self, path: &str) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "insert into projects (path, at, seq) values (?1, ?2, (select coalesce(max(seq), 0) + 1 from projects))
             on conflict (path) do update set at = excluded.at, seq = excluded.seq",
            params![path, now()],
        )?;
        tx.execute("delete from projects where path not in (select path from projects order by seq desc limit ?1)", [RECENT as i64])?;
        tx.commit()
    }

    pub fn projects(&self) -> rusqlite::Result<Vec<(String, f64)>> {
        let mut statement = self.conn.prepare("select path, at from projects order by seq desc")?;
        let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    pub fn forget_project(&self, path: &str) -> rusqlite::Result<()> {
        self.conn.execute("delete from projects where path = ?1", [path]).map(|_| ())
    }

    pub fn clear_projects(&self) -> rusqlite::Result<()> {
        self.conn.execute("delete from projects", []).map(|_| ())
    }

    pub fn crashes(&self) -> rusqlite::Result<Vec<(String, String, bool, bool)>> {
        let mut statement = self.conn.prepare("select file, signature, dismissed is not null, reported is not null from crashes order by file")?;
        let rows = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect()
    }

    pub fn put_crash(&self, file: &str, signature: &str) -> rusqlite::Result<()> {
        self.conn.execute("insert or ignore into crashes (file, signature, at) values (?1, ?2, ?3)", params![file, signature, now()]).map(|_| ())
    }

    pub fn crash_reported(&self, signature: &str) -> rusqlite::Result<()> {
        self.conn.execute("update crashes set reported = ?2 where signature = ?1 and reported is null", params![signature, now()]).map(|_| ())
    }

    pub fn dismiss_crashes(&self, through: &str) -> rusqlite::Result<usize> {
        self.conn.execute("update crashes set dismissed = ?2 where file <= ?1 and dismissed is null", params![through, now()])
    }

    pub fn never_import(&self, path: &str) -> rusqlite::Result<()> {
        self.conn.execute("insert or replace into never_import (path, at) values (?1, ?2)", params![path, now()]).map(|_| ())
    }

    pub fn never_imports(&self, path: &str) -> rusqlite::Result<bool> {
        self.conn.prepare("select 1 from never_import where path = ?1")?.exists([path])
    }

    pub fn relocate(&self, from: &str, to: &str) -> rusqlite::Result<usize> {
        let under = |column: &str| format!("({column} = ?1 or substr({column}, 1, length(?1) + 1) = ?1 || '/')");
        let moved = |column: &str| format!("?2 || substr({column}, length(?1) + 1)");
        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0;
        for (table, column, verb) in [("tasks", "repo", "update"), ("messages", "repo", "update"), ("quiz_rounds", "repo", "update"), ("quiz_ready", "repo", "update"), ("quiz_map", "repo", "update or replace"), ("quiz_notes", "repo", "update or replace"), ("phone_chat", "repo", "update"), ("samples", "repo", "update or replace"), ("projects", "path", "update or replace"), ("planners", "path", "update or replace")] {
            count += tx.execute(&format!("{verb} {table} set {column} = {} where {}", moved(column), under(column)), [from, to])?;
        }
        let folder = "json_extract(doc, '$.folder')";
        count += tx.execute(&format!("update tasks set doc = json_set(doc, '$.folder', {}) where {}", moved(folder), under(folder)), [from, to])?;
        tx.execute(&format!("delete from never_import where {}", under("path")), [from])?;
        tx.commit()?;
        Ok(count)
    }
}

const MESSAGE: &str = "id, recipient, repo, sender, at, delivered, text, mode, comment";

impl Db {
    pub fn add_schedule(&self, schedule: &Schedule) -> rusqlite::Result<i64> {
        insert_schedule(&self.conn, schedule)
    }

    pub fn schedules(&self) -> rusqlite::Result<Vec<Schedule>> {
        let mut statement = self.conn.prepare("select id, repo, name, target, rule, prompt, next, message, made_at, paused, doc from schedules order by id")?;
        let rows = statement.query_map([], |r| {
            Ok(Schedule { id: r.get(0)?, repo: r.get(1)?, name: r.get(2)?, target: r.get(3)?, rule: r.get(4)?, prompt: r.get(5)?, next: r.get(6)?, message: r.get(7)?, made_at: r.get(8)?, paused: r.get(9)?, doc: doc_of(r.get(10)?) })
        })?;
        rows.collect()
    }

    pub fn remove_schedule(&self, id: i64) -> rusqlite::Result<bool> {
        Ok(self.conn.execute("delete from schedules where id = ?1", [id])? == 1)
    }

    pub fn schedule_fired(&self, id: i64, at: f64, how: How, left: &Left, next: f64) -> rusqlite::Result<()> {
        let (state, message, detail) = match left {
            Left::Message(message) => ("left", Some(*message), None),
            Left::Skipped => ("skipped", None, None),
            Left::Failed(why) => ("failed", None, Some(why.as_str())),
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("update schedules set next = ?2, message = coalesce(?3, message) where id = ?1", params![id, next, message])?;
        tx.execute("insert into schedule_firings (schedule, at, how, state, message, detail) values (?1, ?2, ?3, ?4, ?5, ?6)", params![id, at, how.as_str(), state, message, detail])?;
        tx.execute("delete from schedule_firings where schedule = ?1 and id not in (select id from schedule_firings where schedule = ?1 order by id desc limit ?2)", params![id, FIRINGS_KEPT])?;
        tx.commit()
    }

    pub fn schedule_paused(&self, id: i64, paused: Option<f64>) -> rusqlite::Result<()> {
        self.conn.execute("update schedules set paused = ?2 where id = ?1", params![id, paused])?;
        Ok(())
    }

    pub fn firings(&self, schedule: i64, limit: i64) -> rusqlite::Result<Vec<Firing>> {
        let mut statement = self.conn.prepare(
            "select f.id, f.schedule, f.at, f.how, f.state, f.message, m.delivered, f.detail, f.reply, f.replied_at
             from schedule_firings f left join messages m on m.id = f.message where f.schedule = ?1 order by f.id desc limit ?2",
        )?;
        let rows = statement.query_map(params![schedule, limit], |r| {
            Ok(Firing { id: r.get(0)?, schedule: r.get(1)?, at: r.get(2)?, how: r.get(3)?, state: r.get(4)?, message: r.get(5)?, delivered: r.get(6)?, detail: r.get(7)?, reply: r.get(8)?, replied_at: r.get(9)? })
        })?;
        rows.collect()
    }

    pub fn firing_reply(&self, firing: i64, text: &str, at: f64) -> rusqlite::Result<()> {
        self.conn.execute("update schedule_firings set reply = ?2, replied_at = ?3 where id = ?1", params![firing, text, at])?;
        Ok(())
    }

    pub fn meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn.query_row("select value from meta where key = ?1", [key], |r| r.get(0)).optional()
    }

    pub fn terminals(&self) -> rusqlite::Result<(Vec<TerminalRow>, TerminalCounters)> {
        let mut statement = self.conn.prepare("select id, pane, keeper, state, doc from terminals")?;
        let rows = statement.query_map([], |r| {
            let doc: String = r.get(4)?;
            Ok(TerminalRow { id: r.get(0)?, pane: r.get(1)?, keeper: r.get(2)?, state: r.get(3)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) })
        })?;
        let rows = rows.collect::<rusqlite::Result<Vec<TerminalRow>>>()?;
        let counters = self.meta(TERMINAL_COUNTERS)?.map_or_else(TerminalCounters::default, |text| TerminalCounters::parse(&text));
        Ok((rows, counters))
    }

    pub fn put_terminals(&self, gone: &[String], rows: &[TerminalRow], counters: TerminalCounters) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for id in gone {
            tx.execute("delete from terminals where id = ?1", [id])?;
        }
        put_terminal_rows(&tx, rows, counters)?;
        tx.commit()
    }

    pub fn terminals_imported(&self) -> rusqlite::Result<bool> {
        Ok(self.meta(TERMINALS_IMPORTED)?.is_some())
    }

    pub fn mark_terminals_imported(&self) -> rusqlite::Result<()> {
        self.put_meta(TERMINALS_IMPORTED, "1")
    }

    pub fn import_terminals(&self, rows: &[TerminalRow], counters: TerminalCounters) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("delete from terminals", [])?;
        put_terminal_rows(&tx, rows, counters)?;
        tx.execute("insert into meta (key, value) values (?1, '1') on conflict (key) do update set value = excluded.value", [TERMINALS_IMPORTED])?;
        tx.commit()
    }

    pub fn put_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute("insert into meta (key, value) values (?1, ?2) on conflict (key) do update set value = excluded.value", [key, value]).map(|_| ())
    }

    pub fn seed_once(&self, key: &str, schedule: &Schedule) -> rusqlite::Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        if tx.query_row("select 1 from meta where key = ?1", [key], |_| Ok(())).optional()?.is_some() {
            return Ok(false);
        }
        tx.execute("insert into meta (key, value) values (?1, '1')", [key])?;
        let taken = tx.query_row("select 1 from schedules where coalesce(repo, '') = coalesce(?1, '') and name = ?2", params![schedule.repo, schedule.name], |_| Ok(())).optional()?.is_some();
        if !taken {
            insert_schedule(&tx, schedule)?;
        }
        tx.commit()?;
        Ok(!taken)
    }

    pub fn repos(&self) -> rusqlite::Result<Vec<String>> {
        let mut statement = self.conn.prepare("select repo from tasks union select path from projects order by 1")?;
        let rows = statement.query_map([], |r| r.get(0))?;
        rows.collect()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunRow {
    pub id: String,
    pub task: String,
    pub repo: String,
    pub pipeline: String,
    pub revision_hash: String,
    pub branch: String,
    pub state: String,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub outcome: Option<String>,
    pub doc: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageRow {
    pub id: i64,
    pub run: String,
    pub stage: String,
    pub visit: i64,
    pub instance: Option<String>,
    pub kind: String,
    pub agent: Option<String>,
    pub machine: String,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub verdict: Option<String>,
    pub session: Option<String>,
    pub terminal: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub doc: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunEvent {
    pub at: f64,
    pub what: String,
    pub by: Option<String>,
    pub doc: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Revision {
    pub n: i64,
    pub at: f64,
    pub by: String,
    pub hash: String,
    pub doc: Value,
}

const RUN: &str = "id, task, repo, pipeline, revision_hash, branch, state, started_at, ended_at, outcome, doc";

const STAGE: &str = "id, run, stage, visit, instance, kind, agent, machine, started_at, ended_at, verdict, session, terminal, worktree, branch, doc";

fn run_row(r: &rusqlite::Row) -> rusqlite::Result<RunRow> {
    Ok(RunRow { id: r.get(0)?, task: r.get(1)?, repo: r.get(2)?, pipeline: r.get(3)?, revision_hash: r.get(4)?, branch: r.get(5)?, state: r.get(6)?, started_at: r.get(7)?, ended_at: r.get(8)?, outcome: r.get(9)?, doc: doc_of(r.get(10)?) })
}

fn stage_row(r: &rusqlite::Row) -> rusqlite::Result<StageRow> {
    Ok(StageRow {
        id: r.get(0)?,
        run: r.get(1)?,
        stage: r.get(2)?,
        visit: r.get(3)?,
        instance: r.get(4)?,
        kind: r.get(5)?,
        agent: r.get(6)?,
        machine: r.get(7)?,
        started_at: r.get(8)?,
        ended_at: r.get(9)?,
        verdict: r.get(10)?,
        session: r.get(11)?,
        terminal: r.get(12)?,
        worktree: r.get(13)?,
        branch: r.get(14)?,
        doc: doc_of(r.get(15)?),
    })
}

impl Db {
    pub fn new_run_id(&self) -> rusqlite::Result<String> {
        loop {
            let id = format!("run-{}", &random_id()?["task-".len()..]);
            if self.conn.query_row("select 1 from runs where id = ?1", [&id], |_| Ok(())).optional()?.is_none() {
                return Ok(id);
            }
        }
    }

    pub fn put_run(&self, run: &RunRow, event: Option<(&str, Option<&str>, &Value)>) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "insert into runs (id, task, repo, pipeline, revision_hash, branch, state, started_at, ended_at, outcome, doc) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             on conflict (id) do update set revision_hash = excluded.revision_hash, state = excluded.state, ended_at = excluded.ended_at, outcome = excluded.outcome, doc = json_patch(runs.doc, excluded.doc)",
            params![run.id, run.task, run.repo, run.pipeline, run.revision_hash, run.branch, run.state, run.started_at, run.ended_at, run.outcome, run.doc.to_string()],
        )?;
        if let Some((what, by, doc)) = event {
            tx.execute("insert into run_events (run, at, what, by, doc) values (?1, ?2, ?3, ?4, ?5)", params![run.id, now(), what, by, doc.to_string()])?;
        }
        tx.commit()
    }

    pub fn run_event(&self, run: &str, what: &str, by: Option<&str>, doc: &Value) -> rusqlite::Result<()> {
        self.conn.execute("insert into run_events (run, at, what, by, doc) values (?1, ?2, ?3, ?4, ?5)", params![run, now(), what, by, doc.to_string()])?;
        Ok(())
    }

    pub fn runs(&self, repo: Option<&str>) -> rusqlite::Result<Vec<RunRow>> {
        let mut statement = self.conn.prepare(&format!("select {RUN} from runs where ?1 is null or repo = ?1 order by started_at, id"))?;
        let rows = statement.query_map([repo], run_row)?;
        rows.collect()
    }

    pub fn live_runs(&self) -> rusqlite::Result<Vec<RunRow>> {
        let mut statement = self.conn.prepare(&format!("select {RUN} from runs where state in ('running', 'waiting', 'paused') order by started_at, id"))?;
        let rows = statement.query_map([], run_row)?;
        rows.collect()
    }

    pub fn runs_of(&self, task: &str) -> rusqlite::Result<Vec<RunRow>> {
        let mut statement = self.conn.prepare(&format!("select {RUN} from runs where task = ?1 order by started_at, id"))?;
        let rows = statement.query_map([task], run_row)?;
        rows.collect()
    }

    pub fn add_revision(&self, run: &str, by: &str, hash: &str, doc: &Value) -> rusqlite::Result<i64> {
        let n: i64 = self.conn.query_row("select coalesce(max(n), 0) + 1 from run_revisions where run = ?1", [run], |r| r.get(0))?;
        self.conn.execute("insert into run_revisions (run, n, at, by, hash, doc) values (?1, ?2, ?3, ?4, ?5, ?6)", params![run, n, now(), by, hash, doc.to_string()])?;
        Ok(n)
    }

    pub fn revisions(&self, run: &str) -> rusqlite::Result<Vec<Revision>> {
        let mut statement = self.conn.prepare("select n, at, by, hash, doc from run_revisions where run = ?1 order by n")?;
        let rows = statement.query_map([run], |r| Ok(Revision { n: r.get(0)?, at: r.get(1)?, by: r.get(2)?, hash: r.get(3)?, doc: doc_of(r.get(4)?) }))?;
        rows.collect()
    }

    pub fn put_stage(&self, stage: &StageRow) -> rusqlite::Result<i64> {
        if stage.id == 0 {
            self.conn.execute(
                "insert into run_stages (run, stage, visit, instance, kind, agent, machine, started_at, ended_at, verdict, session, terminal, worktree, branch, doc) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![stage.run, stage.stage, stage.visit, stage.instance, stage.kind, stage.agent, stage.machine, stage.started_at, stage.ended_at, stage.verdict, stage.session, stage.terminal, stage.worktree, stage.branch, stage.doc.to_string()],
            )?;
            return Ok(self.conn.last_insert_rowid());
        }
        self.conn.execute(
            "update run_stages set ended_at = ?2, verdict = ?3, session = ?4, terminal = ?5, worktree = ?6, branch = ?7, doc = json_patch(doc, ?8) where id = ?1",
            params![stage.id, stage.ended_at, stage.verdict, stage.session, stage.terminal, stage.worktree, stage.branch, stage.doc.to_string()],
        )?;
        Ok(stage.id)
    }

    pub fn stages(&self, run: &str) -> rusqlite::Result<Vec<StageRow>> {
        let mut statement = self.conn.prepare(&format!("select {STAGE} from run_stages where run = ?1 order by id"))?;
        let rows = statement.query_map([run], stage_row)?;
        rows.collect()
    }

    pub fn run_events(&self, run: &str) -> rusqlite::Result<Vec<RunEvent>> {
        let mut statement = self.conn.prepare("select at, what, by, doc from run_events where run = ?1 order by at, id")?;
        let rows = statement.query_map([run], |r| Ok(RunEvent { at: r.get(0)?, what: r.get(1)?, by: r.get(2)?, doc: doc_of(r.get(3)?) }))?;
        rows.collect()
    }
}

const READY: &str = "select q.round, q.repo, r.head, q.written, q.doc, q.taken from quiz_ready q join quiz_rounds r on r.id = q.round";

const ANSWER: &str = "a.id, a.round, a.at, a.kind, a.file, a.right, a.again, a.doc";

fn doc_of(text: String) -> Value {
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

fn round_row(r: &rusqlite::Row) -> rusqlite::Result<Round> {
    Ok(Round { id: r.get(0)?, repo: r.get(1)?, at: r.get(2)?, head: r.get(3)?, doc: doc_of(r.get(4)?) })
}

fn ready_row(r: &rusqlite::Row) -> rusqlite::Result<Ready> {
    Ok(Ready { round: r.get(0)?, repo: r.get(1)?, head: r.get(2)?, written: r.get(3)?, doc: doc_of(r.get(4)?), taken: r.get(5)? })
}

fn answer_row(r: &rusqlite::Row) -> rusqlite::Result<Answer> {
    Ok(Answer { id: r.get(0)?, round: r.get(1)?, at: r.get(2)?, kind: r.get(3)?, file: r.get(4)?, right: r.get(5)?, again: r.get(6)?, doc: doc_of(r.get(7)?) })
}

pub fn put_row(conn: &Connection, row: &Row, change: Option<&Change>) -> rusqlite::Result<()> {
    conn.execute(
        "insert into tasks (id, repo, place, terminal, made_at, doc) values (?1, ?2, ?3, ?4, ?5, json_patch('{}', ?6))
         on conflict (id) do update set repo = excluded.repo, place = excluded.place, terminal = excluded.terminal, made_at = excluded.made_at, doc = json_patch(tasks.doc, ?6)",
        params![row.id, row.repo, row.place.as_str(), row.terminal, row.made_at, row.doc.to_string()],
    )?;
    if let Some(change) = change {
        log(conn, &row.id, change)?;
    }
    Ok(())
}

fn insert_schedule(conn: &Connection, schedule: &Schedule) -> rusqlite::Result<i64> {
    conn.execute(
        "insert into schedules (repo, name, target, rule, prompt, next, message, made_at, paused, doc) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, json_patch('{}', ?10))",
        params![schedule.repo, schedule.name, schedule.target, schedule.rule, schedule.prompt, schedule.next, schedule.message, schedule.made_at, schedule.paused, schedule.doc.to_string()],
    )?;
    Ok(conn.last_insert_rowid())
}

fn log(conn: &Connection, task: &str, change: &Change) -> rusqlite::Result<()> {
    conn.execute(
        "insert into task_events (task, at, what, from_place, to_place, by, doc) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![task, change.at.unwrap_or_else(now), change.what, change.from.map(Place::as_str), change.to.map(Place::as_str), change.by, change.doc.to_string()],
    )?;
    Ok(())
}

pub const TASK: &str = "task-";

fn put_terminal_rows(conn: &Connection, rows: &[TerminalRow], counters: TerminalCounters) -> rusqlite::Result<()> {
    for row in rows {
        conn.execute(
            "insert into terminals (id, pane, keeper, state, doc) values (?1, ?2, ?3, ?4, ?5) \
             on conflict (id) do update set pane = excluded.pane, keeper = excluded.keeper, state = excluded.state, doc = json_patch(doc, excluded.doc)",
            params![row.id, row.pane, row.keeper, row.state, row.doc.to_string()],
        )?;
    }
    conn.execute("insert into meta (key, value) values (?1, ?2) on conflict (key) do update set value = excluded.value", [TERMINAL_COUNTERS, &counters.json()])?;
    Ok(())
}

fn put_layout(conn: &Connection, doc: &Value) -> rusqlite::Result<()> {
    conn.execute("insert into layout (id, doc) values (1, ?1) on conflict (id) do update set doc = excluded.doc", params![doc.to_string()])?;
    Ok(())
}

pub fn is_task_id(id: &str) -> bool {
    id.strip_prefix(TASK).is_some_and(|hex| hex.len() == 8 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

pub fn short_id<'a>(id: &str, open: impl Iterator<Item = &'a str>) -> String {
    let Some(hex) = id.strip_prefix(TASK).filter(|_| is_task_id(id)) else { return id.to_string() };
    let others: Vec<&str> = open.filter(|other| *other != id).filter_map(|other| other.strip_prefix(TASK)).collect();
    let len = (4..8).find(|&n| !others.iter().any(|other| other.get(..n) == Some(&hex[..n]))).unwrap_or(8);
    format!("{TASK}{}", &hex[..len])
}

pub fn random_id() -> rusqlite::Result<String> {
    let mut bytes = [0u8; 4];
    fs::File::open("/dev/urandom").and_then(|mut random| std::io::Read::read_exact(&mut random, &mut bytes)).map_err(|e| rusqlite::Error::InvalidParameterName(format!("can't read /dev/urandom: {e}")))?;
    Ok(format!("{TASK}{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))
}

fn legacy(id: &str) -> bool {
    id.strip_prefix('t').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn renumber(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "defer_foreign_keys", true)?;
    let mut statement = conn.prepare("select id, json_extract(doc, '$.folder') from tasks order by made_at, rowid")?;
    let rows: Vec<(String, Option<String>)> = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut taken: HashSet<String> = rows.iter().map(|(id, _)| id.clone()).filter(|id| is_task_id(id)).collect();
    let (mut renamed, mut order) = (HashMap::new(), vec![]);
    for (id, folder) in rows.into_iter().filter(|(id, _)| !is_task_id(id)) {
        let named = folder.as_deref().and_then(|f| Path::new(f).file_name()?.to_str().map(String::from)).filter(|name| is_task_id(name) && !taken.contains(name));
        let new = match named {
            Some(name) => name,
            None => loop {
                let fresh = random_id()?;
                if !taken.contains(&fresh) {
                    break fresh;
                }
            },
        };
        taken.insert(new.clone());
        conn.execute("update task_events set task = ?2 where task = ?1", [&id, &new])?;
        conn.execute("update tasks set id = ?2 where id = ?1", [&id, &new])?;
        log(conn, &new, &Change::new("renumbered").by(Some("daemon")).with("from", Value::String(id.clone())))?;
        order.push(format!("{id} is {new}"));
        renamed.insert(id, new);
    }
    if renamed.is_empty() {
        return Ok(());
    }
    let follow = |doc: &str| -> Option<String> {
        let mut doc: Value = serde_json::from_str(doc).ok()?;
        let after = doc.get("after")?.as_array()?.clone();
        let mapped: Vec<Value> = after.iter().filter_map(Value::as_str).filter_map(|id| renamed.get(id).cloned().or_else(|| is_task_id(id).then(|| id.to_string()))).map(Value::String).collect();
        (mapped != after).then(|| {
            doc["after"] = Value::Array(mapped);
            doc.to_string()
        })
    };
    for table in ["tasks", "task_events"] {
        let mut statement = conn.prepare(&format!("select rowid, doc from {table} where json_type(doc, '$.after') = 'array'"))?;
        let docs: Vec<(i64, String)> = statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        for (rowid, doc) in docs {
            if let Some(doc) = follow(&doc) {
                conn.execute(&format!("update {table} set doc = ?2 where rowid = ?1"), params![rowid, doc])?;
            }
        }
    }
    conn.execute("delete from meta where key = 'next_task'", [])?;
    say(format!("gave {} tasks their new IDs: {}", renamed.len(), order.join(", ")));
    Ok(())
}

pub fn oldest_first(a: (f64, &str), b: (f64, &str)) -> Ordering {
    a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1))
}

fn read_backlog(file: &Path) -> Result<Value, String> {
    let size = fs::metadata(file).map_err(|e| e.to_string())?.len();
    if size > MAX_BACKLOG {
        return Err(format!("it's {size} bytes, more than a backlog holds"));
    }
    let text = fs::read_to_string(file).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn row_from_saved(task: &Value, place: Place, pane: Option<&str>) -> Option<Row> {
    let text = |key: &str| task.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    let id = text("id").filter(|id| legacy(id))?;
    let repo = text("repo")?;
    let mut doc = serde_json::Map::new();
    for key in ["name", "message", "by", "agent", "folder", "session"] {
        if let Some(value) = text(key) {
            doc.insert(key.into(), Value::String(value));
        }
    }
    Some(Row { id, repo, place, terminal: pane.filter(|p| !p.is_empty()).map(String::from), made_at: task.get("madeAt").and_then(Value::as_f64).unwrap_or(0.0), doc: Value::Object(doc) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_db_that_cannot_be_made_is_refused() {
        let dir = folder("refused");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let opened = Db::open(&dir);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(opened.is_err_and(|error| error.contains("can't make")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_wal_is_checkpointed_beside_the_loop() {
        let dir = std::env::temp_dir().join(format!("kanna-checkpoint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir).unwrap();
        let auto: i64 = db.conn.query_row("pragma wal_autocheckpoint", [], |row| row.get(0)).unwrap();
        assert_eq!(auto, 0, "the loop's connection never checkpoints");
        let size = || fs::metadata(dir.join(FILE)).unwrap().len();
        let before = size();
        db.put_layout(&json!({ "filler": "x".repeat(1 << 20) })).unwrap();
        let by = std::time::Instant::now() + CHECKPOINT_EVERY * 4;
        while size() == before {
            assert!(std::time::Instant::now() < by, "nothing checkpointed the WAL");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(db);
        fs::remove_dir_all(&dir).unwrap();
    }
    use kanna_mux::json::json;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-db-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn real() -> Value {
        serde_json::from_str(include_str!("../tests/fixtures/backlog.json")).unwrap()
    }

    fn renumbered(db: &Db) -> HashMap<String, String> {
        let mut statement = db.conn.prepare("select json_extract(doc, '$.from'), task from task_events where what = 'renumbered'").unwrap();
        statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }

    #[test]
    fn a_real_backlog_json_moves_into_the_db_with_no_task_lost() {
        let dir = folder("import");
        let saved = real();
        fs::write(dir.join(BACKLOG), saved.to_string()).unwrap();
        let db = Db::open(&dir).unwrap();
        assert!(!dir.join(BACKLOG).exists() && dir.join(IMPORTED_BACKLOG).exists());
        let rows = db.tasks().unwrap();
        let waiting = saved["tasks"].as_array().unwrap();
        let running = saved["running"].as_array().unwrap();
        assert_eq!(rows.len(), waiting.len() + running.len());
        assert!(rows.iter().all(|r| is_task_id(&r.id)), "every task has a new ID");
        let new = renumbered(&db);
        for task in waiting.iter().chain(running) {
            let id = &new[task["id"].as_str().unwrap()];
            let row = rows.iter().find(|r| &r.id == id).unwrap_or_else(|| panic!("{} lost", task["id"]));
            assert_eq!(row.repo, task["repo"]);
            assert_eq!(row.made_at, task["madeAt"].as_f64().unwrap());
            for key in ["name", "message", "by", "agent", "folder", "session"] {
                assert_eq!(row.doc.get(key).unwrap_or(&Value::Null), task.get(key).unwrap_or(&Value::Null), "{} {key}", task["id"]);
            }
            if let Some(folder) = task["folder"].as_str() {
                assert!(folder.ends_with(id.as_str()), "{} keeps its worktree's name: {folder} is {id}", task["id"]);
            }
            let made = &db.events(&row.id).unwrap()[0];
            assert_eq!((made.what.as_str(), made.at), ("made", row.made_at));
        }
        for task in running {
            let row = rows.iter().find(|r| r.id == new[task["id"].as_str().unwrap()]).unwrap();
            assert_eq!((row.place, row.terminal.as_deref()), (Place::Desk, task["pane"].as_str()));
        }
        assert!(rows.iter().filter(|r| r.place == Place::Shelf).all(|r| r.terminal.is_none()));
        drop(db);
        let again = Db::open(&dir).unwrap();
        assert_eq!(again.tasks().unwrap().len(), rows.len(), "nothing is imported twice");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_task_s_short_id_is_4_hex_digits_growing_only_to_stay_unique() {
        let open = ["task-1dfb12c2", "task-1dfb9a00", "task-1dfb9b11", "task-c0ffee00"];
        let short = |id: &str| short_id(id, open.iter().copied());
        assert_eq!(short("task-c0ffee00"), "task-c0ff");
        assert_eq!(short("task-1dfb12c2"), "task-1dfb1");
        assert_eq!((short("task-1dfb9a00"), short("task-1dfb9b11")), ("task-1dfb9a".to_string(), "task-1dfb9b".to_string()));
        assert_eq!(short("task-0decaf00"), "task-0dec", "a done task is shortened against the open ones");
        assert_eq!(short("p12"), "p12");
    }

    #[test]
    fn a_held_task_keeps_its_pane_and_a_bad_entry_is_skipped() {
        let dir = folder("held");
        let saved = json!({ "counter": 3, "tasks": [
            { "id": "t2", "name": "Held", "repo": "/r", "madeAt": 4.0, "agent": "claude", "held": "p9", "folder": "/r/w" },
            { "id": "p5", "repo": "/r" },
            { "id": "t3", "name": "No repo" },
        ]});
        fs::write(dir.join(BACKLOG), saved.to_string()).unwrap();
        let rows = Db::open(&dir).unwrap().tasks().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].place, rows[0].terminal.as_deref()), (Place::Shelf, Some("p9")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tasks_load_oldest_first_with_the_id_breaking_a_tie() {
        let dir = folder("order");
        let db = Db::open(&dir).unwrap();
        for (id, made_at) in [("task-0000000c", 2.0), ("task-0000000b", 1.0), ("task-0000000d", 3.0), ("task-0000000a", 1.0)] {
            db.put(&Row { id: id.into(), repo: "/r".into(), place: Place::Shelf, terminal: None, made_at, doc: json!({}) }, None).unwrap();
        }
        let ids = Db::open(&dir).unwrap().tasks().unwrap().into_iter().map(|row| row.id).collect::<Vec<_>>();
        assert_eq!(ids, ["task-0000000a", "task-0000000b", "task-0000000c", "task-0000000d"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_change_and_its_event_are_written_together_and_unknown_doc_keys_are_kept() {
        let dir = folder("put");
        let db = Db::open(&dir).unwrap();
        let id = db.new_id(vec![random_id().unwrap()], |_| true).unwrap();
        assert!(is_task_id(&id), "{id}");
        let mut row = Row { id: id.clone(), repo: "/r".into(), place: Place::Shelf, terminal: None, made_at: 1.0, doc: json!({ "name": "A", "later": "kept" }) };
        db.put(&row, Some(&Change::new("made").by(Some("p1")))).unwrap();
        row.place = Place::Desk;
        row.terminal = Some("term-7".into());
        row.doc = json!({ "name": "B", "session": null });
        db.put(&row, Some(&Change::new("picked").moved(Place::Shelf, Place::Desk).with("window", json!("w2")))).unwrap();
        let stored = db.tasks().unwrap().remove(0);
        assert_eq!(stored.doc, json!({ "name": "B", "later": "kept" }));
        assert_eq!((stored.place, stored.terminal.as_deref()), (Place::Desk, Some("term-7")));
        let events = db.events(&id).unwrap();
        assert_eq!(events.iter().map(|e| e.what.as_str()).collect::<Vec<_>>(), ["made", "picked"]);
        assert_eq!((events[1].from, events[1].to, events[1].doc["window"].as_str()), (Some(Place::Shelf), Some(Place::Desk), Some("w2")));
        assert_eq!(events[0].by.as_deref(), Some("p1"));
        assert!(db.log("task-00000404", &Change::new("made")).is_err(), "an event needs its task");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_task_marked_done_writes_the_layout_in_the_same_transaction() {
        let dir = folder("done-layout");
        let db = Db::open(&dir).unwrap();
        assert_eq!(db.layout().unwrap(), None);
        db.saves_layout_with_done(|| Some(json!({ "workspaces": [] })));
        let mut row = Row { id: db.new_id(vec![random_id().unwrap()], |_| true).unwrap(), repo: "/r".into(), place: Place::Desk, terminal: Some("term-1".into()), made_at: 1.0, doc: json!({}) };
        db.put(&row, None).unwrap();
        assert_eq!(db.layout().unwrap(), None, "only a done task writes the layout");
        row.place = Place::Done;
        db.put(&row, None).unwrap();
        assert_eq!(db.layout().unwrap(), Some(json!({ "workspaces": [] })));
        db.put_layout(&json!({ "workspaces": [{ "id": "ws-1" }] })).unwrap();
        assert_eq!(Db::open(&dir).unwrap().layout().unwrap(), Some(json!({ "workspaces": [{ "id": "ws-1" }] })));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn done_tasks_come_newest_done_first_with_when_they_were_done() {
        let dir = folder("done");
        let db = Db::open(&dir).unwrap();
        for (id, made, done) in [("t1", 1.0, Some(30.0)), ("t2", 2.0, Some(20.0)), ("t3", 3.0, None), ("t4", 4.0, Some(40.0))] {
            let row = Row { id: id.into(), repo: "/r".into(), place: Place::Shelf, terminal: None, made_at: made, doc: json!({ "name": id }) };
            db.put(&row, Some(&Change::new("made"))).unwrap();
            if let Some(at) = done {
                let mut change = Change::new("done").moved(Place::Shelf, Place::Done);
                change.at = Some(at);
                db.put(&Row { place: Place::Done, ..row }, Some(&change)).unwrap();
            }
        }
        let done = db.done_tasks().unwrap();
        assert_eq!(done.iter().map(|(r, at)| (r.id.as_str(), *at)).collect::<Vec<_>>(), [("t4", 40.0), ("t1", 30.0), ("t2", 20.0)]);
        assert_eq!(done[0].0.doc["name"], "t4");
        assert_eq!(db.tasks().unwrap().len(), 1, "the open tasks leave out the done ones");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_db_is_moved_aside_and_the_last_good_copy_restored() {
        let dir = folder("corrupt");
        fs::write(dir.join(BACKLOG), real().to_string()).unwrap();
        let count = Db::open(&dir).unwrap().tasks().unwrap().len();
        assert!(dir.join(BACKUP).exists());
        let _ = fs::remove_file(dir.join(format!("{FILE}-wal")));
        let _ = fs::remove_file(dir.join(format!("{FILE}-shm")));
        fs::write(dir.join(FILE), b"not a database, just garbage that isn't sqlite at all ".repeat(100)).unwrap();
        let db = Db::open(&dir).unwrap();
        assert_eq!(db.tasks().unwrap().len(), count, "the backup's tasks are back");
        assert!(fs::read_dir(&dir).unwrap().flatten().any(|f| f.file_name().to_string_lossy().starts_with("kanna.db.bad-")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_db_comes_back_from_its_copy_then_from_the_imported_backlog() {
        let dir = folder("missing");
        fs::write(dir.join(BACKLOG), real().to_string()).unwrap();
        let count = Db::open(&dir).unwrap().tasks().unwrap().len();
        for file in [FILE.to_string(), format!("{FILE}-wal"), format!("{FILE}-shm")] {
            let _ = fs::remove_file(dir.join(file));
        }
        assert_eq!(Db::open(&dir).unwrap().tasks().unwrap().len(), count, "from kanna.db.bak");
        for file in [FILE.to_string(), format!("{FILE}-wal"), format!("{FILE}-shm")] {
            let _ = fs::remove_file(dir.join(file));
        }
        fs::write(dir.join(BACKUP), b"garbage").unwrap();
        assert_eq!(Db::open(&dir).unwrap().tasks().unwrap().len(), count, "from backlog.json.imported");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_db_from_a_newer_kanna_is_left_alone() {
        let dir = folder("newer");
        let conn = Connection::open(dir.join(FILE)).unwrap();
        conn.pragma_update(None, "user_version", version() + 1).unwrap();
        drop(conn);
        let db = Db::open(&dir).unwrap();
        assert!(dir.join(format!("{FILE}.newer")).exists());
        assert!(db.tasks().unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_backlog_json_written_beside_the_db_adds_only_the_tasks_it_lacks() {
        let dir = folder("beside");
        fs::write(dir.join(BACKLOG), json!({ "counter": 1, "tasks": [{ "id": "t1", "name": "In the db", "repo": "/r", "madeAt": 1.0 }] }).to_string()).unwrap();
        drop(Db::open(&dir).unwrap());
        fs::write(dir.join(BACKLOG), json!({ "counter": 5, "tasks": [{ "id": "t1", "name": "Older", "repo": "/r" }, { "id": "t5", "name": "New", "repo": "/r", "madeAt": 2.0 }] }).to_string()).unwrap();
        let db = Db::open(&dir).unwrap();
        let rows = db.tasks().unwrap();
        assert_eq!(rows.iter().map(|r| r.doc["name"].as_str().unwrap()).collect::<Vec<_>>(), ["In the db", "New"]);
        assert!(rows.iter().all(|r| is_task_id(&r.id)));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_db_with_t_ids_gives_each_task_its_worktree_s_name_and_carries_over_its_events_and_blockers() {
        let dir = folder("renumber");
        let conn = connect(&dir.join(FILE)).unwrap();
        for step in &MIGRATIONS[..2] {
            let Step::Sql(sql) = step else { unreachable!() };
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute_batch(r#"
            insert into meta (key, value) values ('next_task', '9');
            insert into tasks (id, repo, place, pane, made_at, doc) values
                ('t1', '/r', 'shelf', 'p4', 1.0, '{"name": "Started", "folder": "/r/.kanna-worktrees/task-1dfb12c2"}'),
                ('t2', '/r', 'backlog', null, 2.0, '{"name": "Waits", "after": ["t1", "t3", "t404"]}'),
                ('t3', '/r', 'done', null, 3.0, '{"name": "Done", "folder": "/r/.kanna-worktrees/task-00c0ffee"}'),
                ('t4', '/r', 'backlog', null, 4.0, '{"name": "Same folder", "folder": "/r/.kanna-worktrees/task-1dfb12c2"}'),
                ('t5', '/r', 'desk', 'p9', 5.0, '{"name": "By hand", "folder": "/r"}');
            insert into task_events (task, at, what, doc) values ('t1', 1.0, 'made', '{}'), ('t1', 2.0, 'shelved', '{}'), ('t2', 2.0, 'made', '{}'), ('t2', 3.0, 'edited', '{"after": ["t1", "t3"]}'), ('t3', 3.0, 'made', '{}'), ('t3', 4.0, 'done', '{}');
        "#).unwrap();
        drop(conn);
        let db = Db::open(&dir).unwrap();
        let new = renumbered(&db);
        assert_eq!(new.len(), 5);
        assert_eq!((new["t1"].as_str(), new["t3"].as_str()), ("task-1dfb12c2", "task-00c0ffee"));
        assert!(new.values().all(|id| is_task_id(id)) && new.values().collect::<HashSet<_>>().len() == 5, "{new:?}");
        assert_eq!(db.task("t1").unwrap(), None, "old IDs are gone");
        let waits = db.task(&new["t2"]).unwrap().unwrap();
        assert_eq!(waits.doc["after"], json!([new["t1"], new["t3"]]));
        let events = |id: &str| db.events(id).unwrap().into_iter().map(|e| e.what).collect::<Vec<_>>();
        assert_eq!(events(&new["t1"]), ["made", "picked", "renumbered"]);
        assert_eq!(db.events(&new["t2"]).unwrap()[1].doc["after"], json!([new["t1"], new["t3"]]));
        assert_eq!(events(&new["t3"]), ["made", "done", "renumbered"]);
        let desk = db.task(&new["t1"]).unwrap().unwrap();
        assert_eq!((desk.place, desk.terminal.as_deref()), (Place::Desk, Some("p4")));
        assert_eq!([db.task(&new["t2"]).unwrap().unwrap().place, db.task(&new["t5"]).unwrap().unwrap().place], [Place::Shelf, Place::Desk]);
        let next: Option<String> = db.conn.query_row("select value from meta where key = 'next_task'", [], |r| r.get(0)).optional().unwrap();
        assert_eq!(next, None);
        let dangling: i64 = db.conn.query_row("select count(*) from task_events where task not in (select id from tasks)", [], |r| r.get(0)).unwrap();
        assert_eq!(dangling, 0);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[ignore = "needs KANNA_REAL_DB, a copy of a real kanna.db at version 2"]
    fn a_real_db_s_tasks_events_and_blockers_all_carry_over() {
        let source = PathBuf::from(std::env::var("KANNA_REAL_DB").expect("KANNA_REAL_DB"));
        let dir = folder("real-db");
        fs::copy(&source, dir.join(FILE)).unwrap();
        let before = Connection::open(dir.join(FILE)).unwrap();
        let mut statement = before.prepare("select id, repo, place, pane, made_at, doc, (select count(*) from task_events e where e.task = t.id) from tasks t").unwrap();
        let old: Vec<(String, String, String, Option<String>, f64, String, i64)> =
            statement.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        let events: i64 = before.query_row("select count(*) from task_events", [], |r| r.get(0)).unwrap();
        drop(statement);
        drop(before);
        let db = Db::open(&dir).unwrap();
        let new = renumbered(&db);
        assert_eq!(new.len(), old.len(), "every task got a new ID");
        let (mut folders, mut blockers) = (0, 0);
        for (id, repo, place, pane, made_at, doc, count) in &old {
            let row = db.task(&new[id]).unwrap().unwrap_or_else(|| panic!("{id} lost"));
            let doc: Value = serde_json::from_str(doc).unwrap();
            assert_eq!((&row.repo, row.place.as_str(), &row.terminal, row.made_at), (repo, match place.as_str() { "shelf" | "desk" => "desk", "backlog" => "shelf", place => place }, pane, *made_at), "{id}");
            for key in ["name", "message", "by", "agent", "folder", "session", "stopped"] {
                assert_eq!(row.doc.get(key), doc.get(key), "{id} {key}");
            }
            let hex = doc["folder"].as_str().and_then(|f| Path::new(f).file_name()?.to_str()).filter(|n| is_task_id(n));
            if hex == Some(new[id].as_str()) {
                folders += 1;
            }
            if let Some(after) = doc["after"].as_array().filter(|a| !a.is_empty()) {
                let mapped: Vec<Value> = after.iter().filter_map(|a| new.get(a.as_str()?)).map(|n| json!(n)).collect();
                assert_eq!(row.doc["after"], Value::Array(mapped), "{id}'s blockers");
                blockers += 1;
            }
            assert_eq!(db.events(&row.id).unwrap().len() as i64, count + 1, "{id}'s events");
        }
        let now: i64 = db.conn.query_row("select count(*) from task_events", [], |r| r.get(0)).unwrap();
        assert_eq!(now, events + old.len() as i64);
        println!("{} tasks ({folders} named after their worktree), {events} events and {blockers} tasks' blockers carried over", old.len());
        drop(db);
        if let Ok(out) = std::env::var("KANNA_REAL_DB_OUT") {
            fs::copy(dir.join(FILE), out).unwrap();
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_db_from_before_the_desk_and_shelf_names_moves_each_task_and_event_to_the_place_it_was_in() {
        let dir = folder("place-names");
        let conn = connect(&dir.join(FILE)).unwrap();
        let renaming = MIGRATIONS.iter().position(|step| matches!(step, Step::Sql(sql) if sql.contains("when 'shelved' then 'picked'"))).unwrap();
        for step in &MIGRATIONS[..renaming] {
            match step {
                Step::Sql(sql) => conn.execute_batch(sql).unwrap(),
                Step::Run(run) => run(&conn).unwrap(),
            }
        }
        conn.pragma_update(None, "user_version", renaming as i64).unwrap();
        conn.execute_batch(r#"
            insert into tasks (id, repo, place, terminal, made_at, doc) values
                ('task-00000001', '/r', 'shelf', 'term-4', 1.0, '{"name": "On the Desk"}'),
                ('task-00000002', '/r', 'backlog', null, 2.0, '{"name": "On the Shelf"}'),
                ('task-00000003', '/r', 'done', null, 3.0, '{"name": "Done"}');
            insert into task_events (task, at, what, from_place, to_place, by) values
                ('task-00000001', 1.0, 'made', null, 'backlog', 'user'),
                ('task-00000001', 2.0, 'shelved', 'backlog', 'shelf', 'user'),
                ('task-00000001', 3.0, 'parked', 'shelf', 'shelf', 'user'),
                ('task-00000001', 4.0, 'pulled', 'shelf', 'shelf', 'user'),
                ('task-00000002', 2.0, 'made', null, 'backlog', 'user'),
                ('task-00000002', 3.0, 'shelved', 'backlog', 'shelf', 'user'),
                ('task-00000002', 4.0, 'deferred', 'shelf', 'backlog', 'user'),
                ('task-00000003', 3.0, 'done', 'desk', 'done', 'user');
        "#).unwrap();
        drop(conn);
        let db = Db::open(&dir).unwrap();
        let place = |id: &str| db.task(id).unwrap().unwrap().place;
        assert_eq!([place("task-00000001"), place("task-00000002"), place("task-00000003")], [Place::Desk, Place::Shelf, Place::Done]);
        let events = |id: &str| db.events(id).unwrap().into_iter().map(|e| (e.what, e.from, e.to)).collect::<Vec<_>>();
        assert_eq!(events("task-00000001").into_iter().map(|e| e.0).collect::<Vec<_>>(), ["made", "picked", "left", "activated"]);
        assert_eq!(events("task-00000001")[..2], [("made".into(), None, Some(Place::Shelf)), ("picked".into(), Some(Place::Shelf), Some(Place::Desk))]);
        assert_eq!(events("task-00000002")[1..], [("picked".into(), Some(Place::Shelf), Some(Place::Desk)), ("shelved".into(), Some(Place::Desk), Some(Place::Shelf))]);
        assert_eq!(events("task-00000003"), [("done".into(), Some(Place::Desk), Some(Place::Done))]);
        let stored: Vec<String> = db.conn.prepare("select distinct place from tasks order by place").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(stored, ["desk", "done", "shelf"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_db_from_before_the_quiz_takes_its_tables_and_keeps_its_tasks() {
        let dir = folder("quiz-migration");
        let conn = connect(&dir.join(FILE)).unwrap();
        let Step::Sql(first) = &MIGRATIONS[0] else { unreachable!() };
        conn.execute_batch(first).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute("insert into tasks (id, repo, place, made_at, doc) values ('t1', '/r', 'backlog', 1.0, '{\"name\": \"Old\"}')", []).unwrap();
        drop(conn);
        let db = Db::open(&dir).unwrap();
        let found: i64 = db.conn.query_row("pragma user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(found, version());
        assert_eq!(db.tasks().unwrap()[0].doc["name"], "Old");
        let round = db.start_round("/r", "abc", &json!({ "by": "p3" })).unwrap();
        db.answer(round, "why", "src/a.rs", true, None, &json!({})).unwrap();
        assert_eq!(db.answers("/r", false).unwrap().len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_miss_stays_a_miss_until_it_is_asked_again() {
        let dir = folder("quiz-misses");
        let db = Db::open(&dir).unwrap();
        let first = db.start_round("/r", "abc", &json!({ "commits": ["abc"], "gone": null })).unwrap();
        let missed = db.answer(first, "where", "src/a.rs", false, None, &json!({ "question": "Where?", "line": 4 })).unwrap();
        let kept = db.answer(first, "why", "docs/b.md", false, None, &json!({})).unwrap();
        db.answer(first, "bug", "src/c.rs", true, None, &json!({})).unwrap();
        let elsewhere = db.start_round("/other", "def", &json!({})).unwrap();
        db.answer(elsewhere, "why", "x.rs", false, None, &json!({})).unwrap();
        let round = db.round(first).unwrap().unwrap();
        assert_eq!((round.repo.as_str(), round.head.as_str(), &round.doc), ("/r", "abc", &json!({ "commits": ["abc"] })));
        assert_eq!(db.answers("/r", true).unwrap().iter().map(|a| a.id).collect::<Vec<_>>(), [missed, kept]);
        let second = db.start_round("/r", "def", &json!({})).unwrap();
        let again = db.answer(second, "where", "src/a.rs", false, Some(missed), &json!({})).unwrap();
        assert_eq!(db.answers("/r", true).unwrap().iter().map(|a| a.id).collect::<Vec<_>>(), [again, kept], "the newer miss stands for it");
        db.answer(second, "where", "src/a.rs", true, Some(again), &json!({})).unwrap();
        assert_eq!(db.answers("/r", true).unwrap().iter().map(|a| a.id).collect::<Vec<_>>(), [kept]);
        assert_eq!(db.rounds("/r").unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), [second, first]);
        assert_eq!(db.answers("/r", false).unwrap().len(), 5);
        assert_eq!(db.find_answer(missed).unwrap().map(|(a, repo)| (a.file, repo)), Some(("src/a.rs".into(), "/r".into())));
        assert!(db.answer(404, "why", "x", true, None, &json!({})).is_err(), "an answer needs its round");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_machines_doc_starts_empty_and_keeps_what_was_put() {
        let dir = folder("machines");
        let db = Db::open(&dir).unwrap();
        assert_eq!(db.machines().unwrap(), serde_json::json!({}));
        let doc = serde_json::json!({ "name": "mbp", "entries": [{ "body": {}, "signature": "00" }] });
        db.put_machines(&doc).unwrap();
        db.put_machines(&doc).unwrap();
        assert_eq!(Db::open(&dir).unwrap().machines().unwrap(), doc);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_s_planner_is_kept_across_a_restart_and_replaced_by_the_next() {
        let dir = folder("planners");
        let db = Db::open(&dir).unwrap();
        assert_eq!(db.planner("/w/app").unwrap(), None);
        db.put_planner("/w/app", &serde_json::json!({ "agent": "claude", "session": "s1" })).unwrap();
        db.put_planner("/w/tool", &serde_json::json!({ "agent": "codex" })).unwrap();
        db.put_planner("/w/app", &serde_json::json!({ "agent": "claude", "session": "s2" })).unwrap();
        let db = Db::open(&dir).unwrap();
        assert_eq!(db.planner("/w/app").unwrap(), Some(serde_json::json!({ "agent": "claude", "session": "s2" })));
        assert_eq!(db.planner("/w/tool").unwrap(), Some(serde_json::json!({ "agent": "codex" })));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_db_with_a_fleet_table_keeps_its_name_and_port_as_machines_and_drops_the_old_log() {
        let dir = folder("fleet-migration");
        let conn = connect(&dir.join(FILE)).unwrap();
        let rename = MIGRATIONS.iter().position(|s| matches!(s, Step::Sql(sql) if sql.contains("rename to machines"))).unwrap();
        for step in &MIGRATIONS[..rename] {
            match step {
                Step::Sql(sql) => conn.execute_batch(sql).unwrap(),
                Step::Run(run) => run(&conn).unwrap(),
            }
        }
        conn.pragma_update(None, "user_version", rename as i64).unwrap();
        conn.execute("insert into fleet (id, doc) values (1, '{\"name\": \"mbp\", \"port\": 7470, \"addrs\": {\"k\": [\"10.0.0.5:7470\"]}, \"anchor\": {\"key\": \"k\"}, \"entries\": [{\"body\": {}}]}')", []).unwrap();
        drop(conn);
        let db = Db::open(&dir).unwrap();
        assert_eq!(db.machines().unwrap(), serde_json::json!({ "name": "mbp", "port": 7470, "addrs": { "k": ["10.0.0.5:7470"] } }));
        let fleet: i64 = db.conn.query_row("select count(*) from sqlite_master where name = 'fleet'", [], |r| r.get(0)).unwrap();
        assert_eq!(fleet, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_reply_is_kept_and_a_second_read_of_one_adds_nothing() {
        let dir = std::env::temp_dir().join(format!("kanna-said-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db = Db::open(&dir).unwrap();
        db.put_said("task-a", "one", false).unwrap();
        db.put_said("task-a", "one", false).unwrap();
        db.put_said("task-a", "two", false).unwrap();
        db.put_said("task-a", "two", true).unwrap();
        db.put_said("task-b", "other", false).unwrap();
        let said = db.said("task-a").unwrap();
        assert_eq!(said.iter().map(|w| (w.text.as_str(), w.asking)).collect::<Vec<_>>(), [("one", false), ("two", true)]);
        for reply in 0..SAID_KEPT {
            db.put_said("task-a", &format!("reply {reply}"), false).unwrap();
        }
        let said = db.said("task-a").unwrap();
        assert_eq!(said.len() as i64, SAID_KEPT);
        assert_eq!(said[0].text, "reply 0");
        assert_eq!(db.said("task-b").unwrap().len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_message_waits_until_it_is_delivered_and_stays_in_both_tasks_history() {
        let dir = folder("messages");
        let db = Db::open(&dir).unwrap();
        let first = db.put_message("task-71af0000", "/r", "task-5e2c0000", "Is your spec final?", Some("bypass"), None).unwrap();
        let second = db.put_message("planner", "/r", "task-71af0000", "I'm done", None, None).unwrap();
        assert_eq!(db.waiting_messages().unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [first.id, second.id]);
        db.delivered(&[first.id]).unwrap();
        assert_eq!(db.waiting_messages().unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [second.id]);
        let history = db.messages(Some("task-71af0000")).unwrap();
        assert_eq!(history.iter().map(|m| (m.id, m.delivered.is_some())).collect::<Vec<_>>(), [(first.id, true), (second.id, false)], "what it got and what it sent");
        assert_eq!(db.messages(Some("task-5e2c0000")).unwrap()[0].text, "Is your spec final?");
        assert_eq!(db.messages(Some("task-5e2c0000")).unwrap()[0].mode.as_deref(), Some("bypass"), "the sender's permissions");
        drop(db);
        assert_eq!(Db::open(&dir).unwrap().waiting_messages().unwrap().len(), 1, "a waiting message outlives the daemon");
        let db = Db::open(&dir).unwrap();
        let comment = Comment { file: "/r/docs/a.md".into(), thread: "c2".into() };
        let third = db.put_message("task-71af0000", "/r", "user", "[kanna comment c2 · docs/a.md:3]\nFix it.", None, Some(&comment)).unwrap();
        assert_eq!(third.comment.as_ref(), Some(&comment));
        assert_eq!(Db::open(&dir).unwrap().waiting_messages().unwrap().iter().find(|m| m.id == third.id).and_then(|m| m.comment.clone()), Some(comment), "a comment's thread outlives the daemon");
        assert_eq!(db.messages(Some("task-5e2c0000")).unwrap()[0].comment.clone(), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
