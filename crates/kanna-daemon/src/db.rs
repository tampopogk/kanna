use kanna_mux::json::Value;
use rusqlite::{params, Connection, OptionalExtension};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const FILE: &str = "kanna.db";

const BACKUP: &str = "kanna.db.bak";

const BACKLOG: &str = "backlog.json";

const IMPORTED: &str = "backlog.json.imported";

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
        text text not null
    );
    create index messages_waiting on messages (delivered, recipient);
    create index messages_by_recipient on messages (recipient, at);
")];

pub fn version() -> i64 {
    MIGRATIONS.len() as i64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Backlog,
    Shelf,
    Done,
}

impl Place {
    pub fn as_str(self) -> &'static str {
        match self {
            Place::Backlog => "backlog",
            Place::Shelf => "shelf",
            Place::Done => "done",
        }
    }

    pub fn parse(text: &str) -> Option<Place> {
        match text {
            "desk" => Some(Place::Shelf),
            text => [Place::Backlog, Place::Shelf, Place::Done].into_iter().find(|p| p.as_str() == text),
        }
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

pub struct Db {
    conn: Connection,
    layout: RefCell<Option<Box<dyn Fn() -> Option<Value>>>>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn stamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).to_string()
}

fn say(message: String) {
    eprintln!("kanna-daemon: {message}");
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
    let mut db = Db { conn, layout: RefCell::default() };
    match db.migrate(found) {
        Ok(()) => Opened::Good(db),
        Err(error) => Opened::Bad(format!("migrating from version {found}: {error}")),
    }
}

impl Db {
    pub fn open(state_dir: &Path) -> Db {
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
        } else if backup.exists() || state_dir.join(IMPORTED).exists() {
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
        let db = match db {
            Some(db) => db,
            None => Db::fresh(&path, state_dir, lost),
        };
        db.absorb(state_dir);
        if let Err(error) = db.conn.backup(rusqlite::MAIN_DB, &backup, None) {
            say(format!("couldn't copy {} to {}: {error}", path.display(), backup.display()));
        } else {
            let _ = fs::set_permissions(&backup, fs::Permissions::from_mode(0o600));
        }
        db
    }

    fn fresh(path: &Path, state_dir: &Path, lost: bool) -> Db {
        let mut db = match connect(path) {
            Ok(conn) => Db { conn, layout: RefCell::default() },
            Err(error) => {
                say(format!("can't make {} ({error}); keeping tasks in memory only until the daemon restarts", path.display()));
                Db { conn: Connection::open_in_memory().expect("an in-memory database"), layout: RefCell::default() }
            }
        };
        db.migrate(0).expect("a new database takes the schema");
        let imported = state_dir.join(IMPORTED);
        if lost && !state_dir.join(BACKLOG).exists() {
            match imported.exists() {
                true => match read_backlog(&imported).and_then(|saved| db.import(&saved).map_err(|e| e.to_string())) {
                    Ok(count) => say(format!("started a new {} from {} ({count} tasks); tasks made or changed since it was imported are lost", path.display(), imported.display())),
                    Err(error) => say(format!("started a new, empty {}: {} can't be read ({error})", path.display(), imported.display())),
                },
                false => say(format!("started a new, empty {}: there's no copy of the tasks to restore", path.display())),
            }
        }
        db
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
        let to = state_dir.join(IMPORTED);
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
        let tasks = saved.get("tasks").and_then(Value::as_array).into_iter().flatten().map(|t| (t, Place::Backlog, t.get("held")));
        let running = saved.get("running").and_then(Value::as_array).into_iter().flatten().map(|t| (t, Place::Shelf, t.get("pane")));
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
                let made = Change { at: Some(row.made_at), by: row.doc.get("by").and_then(Value::as_str).map(String::from), to: Some(Place::Backlog), ..Change::new("made") };
                log(&tx, &row.id, &made)?;
                if place == Place::Shelf {
                    log(&tx, &row.id, &Change::new("imported").moved(Place::Backlog, Place::Shelf).by(Some("daemon")))?;
                }
                count += 1;
            }
        }
        renumber(&tx)?;
        tx.commit()?;
        Ok(count)
    }

    pub fn tasks(&self) -> rusqlite::Result<Vec<Row>> {
        let mut statement = self.conn.prepare("select id, repo, place, terminal, made_at, doc from tasks where place != 'done' order by made_at, rowid")?;
        let rows = statement.query_map([], |r| {
            let place: String = r.get(2)?;
            let doc: String = r.get(5)?;
            Ok(Row { id: r.get(0)?, repo: r.get(1)?, place: Place::parse(&place).unwrap_or(Place::Backlog), terminal: r.get(3)?, made_at: r.get(4)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) })
        })?;
        rows.collect()
    }

    pub fn task(&self, id: &str) -> rusqlite::Result<Option<Row>> {
        self.conn
            .query_row("select id, repo, place, terminal, made_at, doc from tasks where id = ?1", [id], |r| {
                let place: String = r.get(2)?;
                let doc: String = r.get(5)?;
                Ok(Row { id: r.get(0)?, repo: r.get(1)?, place: Place::parse(&place).unwrap_or(Place::Backlog), terminal: r.get(3)?, made_at: r.get(4)?, doc: serde_json::from_str(&doc).unwrap_or(Value::Null) })
            })
            .optional()
    }

    pub fn new_id(&self, free: impl Fn(&str) -> bool) -> rusqlite::Result<String> {
        for _ in 0..8 {
            let id = random_id()?;
            if self.conn.query_row("select 1 from tasks where id = ?1", [&id], |_| Ok(())).optional()?.is_none() && free(&id) {
                return Ok(id);
            }
        }
        Err(rusqlite::Error::InvalidParameterName("every task ID tried was taken".into()))
    }

    pub fn put(&self, row: &Row, change: Option<&Change>) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "insert into tasks (id, repo, place, terminal, made_at, doc) values (?1, ?2, ?3, ?4, ?5, json_patch('{}', ?6))
             on conflict (id) do update set repo = excluded.repo, place = excluded.place, terminal = excluded.terminal, made_at = excluded.made_at, doc = json_patch(tasks.doc, ?6)",
            params![row.id, row.repo, row.place.as_str(), row.terminal, row.made_at, row.doc.to_string()],
        )?;
        if let Some(change) = change {
            log(&tx, &row.id, change)?;
        }
        if row.place == Place::Done {
            if let Some(layout) = self.layout.borrow().as_ref().and_then(|now| now()) {
                put_layout(&tx, &layout)?;
            }
        }
        tx.commit()
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

    pub fn put_layout(&self, doc: &Value) -> rusqlite::Result<()> {
        put_layout(&self.conn, doc)
    }

    pub fn log(&self, task: &str, change: &Change) -> rusqlite::Result<()> {
        log(&self.conn, task, change)
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

    pub fn put_message(&self, recipient: &str, repo: &str, sender: &str, text: &str) -> rusqlite::Result<Message> {
        let at = now();
        self.conn.execute("insert into messages (recipient, repo, sender, at, text) values (?1, ?2, ?3, ?4, ?5)", params![recipient, repo, sender, at, text])?;
        Ok(Message { id: self.conn.last_insert_rowid(), recipient: recipient.into(), repo: repo.into(), sender: sender.into(), at, delivered: None, text: text.into() })
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
        let rows = statement.query_map(params, |r| Ok(Message { id: r.get(0)?, recipient: r.get(1)?, repo: r.get(2)?, sender: r.get(3)?, at: r.get(4)?, delivered: r.get(5)?, text: r.get(6)? }))?;
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
}

const MESSAGE: &str = "id, recipient, repo, sender, at, delivered, text";

const ANSWER: &str = "a.id, a.round, a.at, a.kind, a.file, a.right, a.again, a.doc";

fn doc_of(text: String) -> Value {
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

fn round_row(r: &rusqlite::Row) -> rusqlite::Result<Round> {
    Ok(Round { id: r.get(0)?, repo: r.get(1)?, at: r.get(2)?, head: r.get(3)?, doc: doc_of(r.get(4)?) })
}

fn answer_row(r: &rusqlite::Row) -> rusqlite::Result<Answer> {
    Ok(Answer { id: r.get(0)?, round: r.get(1)?, at: r.get(2)?, kind: r.get(3)?, file: r.get(4)?, right: r.get(5)?, again: r.get(6)?, doc: doc_of(r.get(7)?) })
}

fn log(conn: &Connection, task: &str, change: &Change) -> rusqlite::Result<()> {
    conn.execute(
        "insert into task_events (task, at, what, from_place, to_place, by, doc) values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![task, change.at.unwrap_or_else(now), change.what, change.from.map(Place::as_str), change.to.map(Place::as_str), change.by, change.doc.to_string()],
    )?;
    Ok(())
}

pub const TASK: &str = "task-";

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

fn random_id() -> rusqlite::Result<String> {
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
        let db = Db::open(&dir);
        assert!(!dir.join(BACKLOG).exists() && dir.join(IMPORTED).exists());
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
            assert_eq!((row.place, row.terminal.as_deref()), (Place::Shelf, task["pane"].as_str()));
        }
        assert!(rows.iter().filter(|r| r.place == Place::Backlog).all(|r| r.terminal.is_none()));
        drop(db);
        let again = Db::open(&dir);
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
        let rows = Db::open(&dir).tasks().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].place, rows[0].terminal.as_deref()), (Place::Backlog, Some("p9")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_change_and_its_event_are_written_together_and_unknown_doc_keys_are_kept() {
        let dir = folder("put");
        let db = Db::open(&dir);
        let id = db.new_id(|_| true).unwrap();
        assert!(is_task_id(&id), "{id}");
        let mut row = Row { id: id.clone(), repo: "/r".into(), place: Place::Backlog, terminal: None, made_at: 1.0, doc: json!({ "name": "A", "later": "kept" }) };
        db.put(&row, Some(&Change::new("made").by(Some("p1")))).unwrap();
        row.place = Place::Shelf;
        row.terminal = Some("term-7".into());
        row.doc = json!({ "name": "B", "session": null });
        db.put(&row, Some(&Change::new("shelved").moved(Place::Backlog, Place::Shelf).with("window", json!("w2")))).unwrap();
        let stored = db.tasks().unwrap().remove(0);
        assert_eq!(stored.doc, json!({ "name": "B", "later": "kept" }));
        assert_eq!((stored.place, stored.terminal.as_deref()), (Place::Shelf, Some("term-7")));
        let events = db.events(&id).unwrap();
        assert_eq!(events.iter().map(|e| e.what.as_str()).collect::<Vec<_>>(), ["made", "shelved"]);
        assert_eq!((events[1].from, events[1].to, events[1].doc["window"].as_str()), (Some(Place::Backlog), Some(Place::Shelf), Some("w2")));
        assert_eq!(events[0].by.as_deref(), Some("p1"));
        assert!(db.log("task-00000404", &Change::new("made")).is_err(), "an event needs its task");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_task_marked_done_writes_the_layout_in_the_same_transaction() {
        let dir = folder("done-layout");
        let db = Db::open(&dir);
        assert_eq!(db.layout().unwrap(), None);
        db.saves_layout_with_done(|| Some(json!({ "workspaces": [] })));
        let mut row = Row { id: db.new_id(|_| true).unwrap(), repo: "/r".into(), place: Place::Shelf, terminal: Some("term-1".into()), made_at: 1.0, doc: json!({}) };
        db.put(&row, None).unwrap();
        assert_eq!(db.layout().unwrap(), None, "only a done task writes the layout");
        row.place = Place::Done;
        db.put(&row, None).unwrap();
        assert_eq!(db.layout().unwrap(), Some(json!({ "workspaces": [] })));
        db.put_layout(&json!({ "workspaces": [{ "id": "ws-1" }] })).unwrap();
        assert_eq!(Db::open(&dir).layout().unwrap(), Some(json!({ "workspaces": [{ "id": "ws-1" }] })));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_db_is_moved_aside_and_the_last_good_copy_restored() {
        let dir = folder("corrupt");
        fs::write(dir.join(BACKLOG), real().to_string()).unwrap();
        let count = Db::open(&dir).tasks().unwrap().len();
        assert!(dir.join(BACKUP).exists());
        let _ = fs::remove_file(dir.join(format!("{FILE}-wal")));
        let _ = fs::remove_file(dir.join(format!("{FILE}-shm")));
        fs::write(dir.join(FILE), b"not a database, just garbage that isn't sqlite at all ".repeat(100)).unwrap();
        let db = Db::open(&dir);
        assert_eq!(db.tasks().unwrap().len(), count, "the backup's tasks are back");
        assert!(fs::read_dir(&dir).unwrap().flatten().any(|f| f.file_name().to_string_lossy().starts_with("kanna.db.bad-")));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_db_comes_back_from_its_copy_then_from_the_imported_backlog() {
        let dir = folder("missing");
        fs::write(dir.join(BACKLOG), real().to_string()).unwrap();
        let count = Db::open(&dir).tasks().unwrap().len();
        for file in [FILE.to_string(), format!("{FILE}-wal"), format!("{FILE}-shm")] {
            let _ = fs::remove_file(dir.join(file));
        }
        assert_eq!(Db::open(&dir).tasks().unwrap().len(), count, "from kanna.db.bak");
        for file in [FILE.to_string(), format!("{FILE}-wal"), format!("{FILE}-shm")] {
            let _ = fs::remove_file(dir.join(file));
        }
        fs::write(dir.join(BACKUP), b"garbage").unwrap();
        assert_eq!(Db::open(&dir).tasks().unwrap().len(), count, "from backlog.json.imported");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_db_from_a_newer_kanna_is_left_alone() {
        let dir = folder("newer");
        let conn = Connection::open(dir.join(FILE)).unwrap();
        conn.pragma_update(None, "user_version", version() + 1).unwrap();
        drop(conn);
        let db = Db::open(&dir);
        assert!(dir.join(format!("{FILE}.newer")).exists());
        assert!(db.tasks().unwrap().is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_backlog_json_written_beside_the_db_adds_only_the_tasks_it_lacks() {
        let dir = folder("beside");
        fs::write(dir.join(BACKLOG), json!({ "counter": 1, "tasks": [{ "id": "t1", "name": "In the db", "repo": "/r", "madeAt": 1.0 }] }).to_string()).unwrap();
        drop(Db::open(&dir));
        fs::write(dir.join(BACKLOG), json!({ "counter": 5, "tasks": [{ "id": "t1", "name": "Older", "repo": "/r" }, { "id": "t5", "name": "New", "repo": "/r", "madeAt": 2.0 }] }).to_string()).unwrap();
        let db = Db::open(&dir);
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
        let db = Db::open(&dir);
        let new = renumbered(&db);
        assert_eq!(new.len(), 5);
        assert_eq!((new["t1"].as_str(), new["t3"].as_str()), ("task-1dfb12c2", "task-00c0ffee"));
        assert!(new.values().all(|id| is_task_id(id)) && new.values().collect::<HashSet<_>>().len() == 5, "{new:?}");
        assert_eq!(db.task("t1").unwrap(), None, "old IDs are gone");
        let waits = db.task(&new["t2"]).unwrap().unwrap();
        assert_eq!(waits.doc["after"], json!([new["t1"], new["t3"]]));
        let events = |id: &str| db.events(id).unwrap().into_iter().map(|e| e.what).collect::<Vec<_>>();
        assert_eq!(events(&new["t1"]), ["made", "shelved", "renumbered"]);
        assert_eq!(db.events(&new["t2"]).unwrap()[1].doc["after"], json!([new["t1"], new["t3"]]));
        assert_eq!(events(&new["t3"]), ["made", "done", "renumbered"]);
        let shelf = db.task(&new["t1"]).unwrap().unwrap();
        assert_eq!((shelf.place, shelf.terminal.as_deref()), (Place::Shelf, Some("p4")));
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
        let db = Db::open(&dir);
        let new = renumbered(&db);
        assert_eq!(new.len(), old.len(), "every task got a new ID");
        let (mut folders, mut blockers) = (0, 0);
        for (id, repo, place, pane, made_at, doc, count) in &old {
            let row = db.task(&new[id]).unwrap().unwrap_or_else(|| panic!("{id} lost"));
            let doc: Value = serde_json::from_str(doc).unwrap();
            assert_eq!((&row.repo, row.place.as_str(), &row.terminal, row.made_at), (repo, place.as_str(), pane, *made_at), "{id}");
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
    fn a_db_from_before_the_quiz_takes_its_tables_and_keeps_its_tasks() {
        let dir = folder("quiz-migration");
        let conn = connect(&dir.join(FILE)).unwrap();
        let Step::Sql(first) = &MIGRATIONS[0] else { unreachable!() };
        conn.execute_batch(first).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute("insert into tasks (id, repo, place, made_at, doc) values ('t1', '/r', 'backlog', 1.0, '{\"name\": \"Old\"}')", []).unwrap();
        drop(conn);
        let db = Db::open(&dir);
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
        let db = Db::open(&dir);
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
    fn a_message_waits_until_it_is_delivered_and_stays_in_both_tasks_history() {
        let dir = folder("messages");
        let db = Db::open(&dir);
        let first = db.put_message("task-71af0000", "/r", "task-5e2c0000", "Is your spec final?").unwrap();
        let second = db.put_message("planner", "/r", "task-71af0000", "I'm done").unwrap();
        assert_eq!(db.waiting_messages().unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [first.id, second.id]);
        db.delivered(&[first.id]).unwrap();
        assert_eq!(db.waiting_messages().unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), [second.id]);
        let history = db.messages(Some("task-71af0000")).unwrap();
        assert_eq!(history.iter().map(|m| (m.id, m.delivered.is_some())).collect::<Vec<_>>(), [(first.id, true), (second.id, false)], "what it got and what it sent");
        assert_eq!(db.messages(Some("task-5e2c0000")).unwrap()[0].text, "Is your spec final?");
        drop(db);
        assert_eq!(Db::open(&dir).waiting_messages().unwrap().len(), 1, "a waiting message outlives the daemon");
        fs::remove_dir_all(&dir).unwrap();
    }
}
