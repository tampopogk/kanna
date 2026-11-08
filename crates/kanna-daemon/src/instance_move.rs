use kanna_mux::json::{json, Value};
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MOVING_DB: &str = "kanna.db.moving";
pub const MOVING_KEY: &str = "device.key.moving";
pub const LEAVING: &str = "moving.json";
pub const MOVED_FROM: &str = "moved_from";
pub const RESUMING: &str = "move_resume";
pub const SCHEMA_FLAG: &str = "--schema";
const LOCK: &str = "daemon.lock";
const DB: &str = crate::db::FILE;
const KEY: &str = crate::machines::KEY;
const LIVE: &str = "place in ('shelf', 'desk', 'backlog')";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Moved {
    pub waiting: Vec<String>,
    pub started: Vec<String>,
    pub desk: Vec<String>,
}

impl Moved {
    pub fn to_value(&self) -> Value {
        json!({ "waiting": self.waiting.len(), "started": self.started.len(), "desk": self.desk.len() })
    }
}

fn stamp() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).to_string()
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn say(message: String) {
    log!("kanna-daemon: {message}");
}

pub fn resolve(to: &str, support: &Path) -> PathBuf {
    match to.contains('/') {
        true => PathBuf::from(to),
        false => support.join(to),
    }
}

pub fn label(folder: &Path) -> String {
    folder.file_name().map_or_else(|| folder.display().to_string(), |n| n.to_string_lossy().into_owned())
}

pub fn lock(folder: &Path) -> Result<fs::File, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    fs::create_dir_all(folder).map_err(|e| format!("can't make {}: {e}", folder.display()))?;
    let _ = fs::set_permissions(folder, fs::Permissions::from_mode(0o700));
    let path = folder.join(LOCK);
    let file = fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&path).map_err(|e| format!("can't open {}: {e}", path.display()))?;
    match unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } {
        0 => Ok(file),
        _ => Err(format!("{}'s Kanna is running: quit it (or stop its served daemon) first; its terminals stay", label(folder))),
    }
}

pub fn receiving_daemon(folder: &Path) -> Option<PathBuf> {
    let record: Value = serde_json::from_str(&fs::read_to_string(folder.join(kanna_build::RECORD)).ok()?).ok()?;
    let daemon = record.get("daemon").and_then(Value::as_str).map(PathBuf::from);
    let beside_app = record.get("app").and_then(Value::as_str).map(|app| Path::new(app).join("Contents/Helpers/kanna-daemon"));
    daemon.filter(|d| d.is_file()).or(beside_app.filter(|d| d.is_file()))
}

pub fn reads_up_to(daemon: &Path) -> Option<i64> {
    let out = std::process::Command::new(daemon).arg(SCHEMA_FLAG).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn open_quiet(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

fn has_table(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    conn.query_row("select 1 from sqlite_master where type = 'table' and name = ?1", [table], |_| Ok(())).optional().map(|found| found.is_some())
}

pub fn live_tasks(folder: &Path) -> Result<Vec<String>, String> {
    let path = folder.join(DB);
    if !path.exists() {
        return Ok(vec![]);
    }
    let conn = open_quiet(&path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    if !has_table(&conn, "tasks").map_err(|e| e.to_string())? {
        return Ok(vec![]);
    }
    let mut query = conn.prepare(&format!("select id from tasks where {LIVE} order by made_at")).map_err(|e| e.to_string())?;
    let ids = query.query_map([], |row| row.get(0)).and_then(|rows| rows.collect::<rusqlite::Result<Vec<String>>>()).map_err(|e| e.to_string())?;
    Ok(ids)
}

pub fn own_accounts(state: &Path) -> Vec<String> {
    let book: Value = fs::read_to_string(state.join(crate::accounts::BOOK)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null);
    book.get("own").and_then(Value::as_array).into_iter().flatten().map(|a| format!("{} ({})", a.get("agent").and_then(Value::as_str).unwrap_or("?"), a.get("label").and_then(Value::as_str).unwrap_or("?"))).collect()
}

pub fn moved_from(folder: &Path) -> Option<Value> {
    let conn = open_quiet(&folder.join(DB)).ok()?;
    let value: String = conn.query_row("select value from meta where key = ?1", [MOVED_FROM], |r| r.get(0)).optional().ok()??;
    serde_json::from_str(&value).ok()
}

fn receiver_layout(folder: &Path) -> Option<String> {
    let path = folder.join(DB);
    if !path.exists() {
        return None;
    }
    let conn = open_quiet(&path).ok()?;
    if !has_table(&conn, "layout").ok()? {
        return None;
    }
    conn.query_row("select doc from layout where id = 1", [], |r| r.get(0)).optional().ok()?
}

pub struct Prepared {
    pub id: String,
    pub from: String,
    pub to: String,
    pub resume: Vec<String>,
}

pub fn prepare(copy: &Path, receiver: &Path, move_: &Prepared) -> rusqlite::Result<Moved> {
    let layout = receiver_layout(receiver);
    let mut conn = open_quiet(copy)?;
    conn.pragma_update(None, "journal_mode", "delete")?;
    let tx = conn.transaction()?;
    let live: Vec<(String, String, String)> = {
        let mut query = tx.prepare(&format!("select id, place, doc from tasks where {LIVE} order by made_at"))?;
        let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut moved = Moved::default();
    let at = now();
    for (id, place, doc) in live {
        let mut doc: Value = serde_json::from_str(&doc).unwrap_or_else(|_| json!({}));
        let started = doc.get("folder").and_then(Value::as_str).is_some_and(|f| !f.is_empty());
        if let Some(fields) = doc.as_object_mut() {
            fields.remove("workspace");
            fields.remove("claim");
            if started && fields.get("stopped").is_none_or(Value::is_null) {
                fields.insert("stopped".into(), json!(format!("moved from {}", move_.from)));
            }
        }
        tx.execute("update tasks set place = 'shelf', terminal = null, doc = ?2 where id = ?1", params![id, doc.to_string()])?;
        let event = json!({ "from": move_.from, "to": move_.to, "move": move_.id });
        tx.execute("insert into task_events (task, at, what, from_place, to_place, by, doc) values (?1, ?2, 'moved', ?3, 'shelf', 'daemon', ?4)", params![id, at, place, event.to_string()])?;
        match (place.as_str(), started) {
            ("desk", _) => moved.desk.push(id),
            (_, true) => moved.started.push(id),
            _ => moved.waiting.push(id),
        }
    }
    tx.execute("update planners set doc = json_set(json_remove(doc, '$.pane'), '$.running', json('false'))", [])?;
    tx.execute("delete from layout", [])?;
    if let Some(layout) = layout {
        tx.execute("insert into layout (id, doc) values (1, ?1)", [layout])?;
    }
    tx.execute("delete from crashes", [])?;
    tx.execute("delete from meta where key = ?1", [RESUMING])?;
    let mark = json!({ "id": move_.id, "from": move_.from, "to": move_.to, "at": at, "resume": move_.resume });
    tx.execute("insert into meta (key, value) values (?1, ?2) on conflict (key) do update set value = excluded.value", params![MOVED_FROM, mark.to_string()])?;
    tx.commit()?;
    conn.close().map_err(|(_, e)| e)?;
    let _ = fs::set_permissions(copy, fs::Permissions::from_mode(0o600));
    Ok(moved)
}

fn copy_key(from: &Path, to: &Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let key = fs::read(from).map_err(|e| format!("can't read {}: {e}", from.display()))?;
    let _ = fs::remove_file(to);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(to).map_err(|e| format!("can't write {}: {e}", to.display()))?;
    file.write_all(&key).and_then(|_| file.sync_all()).map_err(|e| format!("can't write {}: {e}", to.display()))
}

fn quiet_receiver_db(folder: &Path) -> Result<(), String> {
    let path = folder.join(DB);
    if !path.exists() {
        return Ok(());
    }
    let conn = open_quiet(&path).map_err(|e| format!("can't open {}: {e}", path.display()))?;
    let _ = conn.query_row("pragma wal_checkpoint(truncate)", [], |_| Ok(()));
    let mode: String = conn.query_row("pragma journal_mode = delete", [], |r| r.get(0)).map_err(|e| format!("can't take {} out of WAL: {e}", path.display()))?;
    if mode != "delete" {
        return Err(format!("{} stays in {mode} mode", path.display()));
    }
    drop(conn);
    let kept = folder.join(format!("{DB}.before-move-{}", stamp()));
    fs::copy(&path, &kept).map_err(|e| format!("can't keep {} as {}: {e}", path.display(), kept.display()))?;
    let _ = fs::set_permissions(&kept, fs::Permissions::from_mode(0o600));
    for extra in ["-wal", "-shm"] {
        let _ = fs::remove_file(PathBuf::from(format!("{}{extra}", path.display())));
    }
    Ok(())
}

pub fn stage_key(state: &Path, receiver: &Path) -> Result<(), String> {
    let key = state.join(KEY);
    match key.exists() {
        true => copy_key(&key, &receiver.join(MOVING_KEY)),
        false => Ok(()),
    }
}

pub fn commit(receiver: &Path) -> Result<(), String> {
    quiet_receiver_db(receiver)?;
    fs::rename(receiver.join(MOVING_DB), receiver.join(DB)).map_err(|e| format!("can't put the records in place in {}: {e}", receiver.display()))?;
    install_key(receiver);
    Ok(())
}

fn install_key(folder: &Path) {
    let moving = folder.join(MOVING_KEY);
    if !moving.exists() {
        return;
    }
    let key = folder.join(KEY);
    if key.exists() {
        let _ = fs::rename(&key, folder.join(format!("{KEY}.before-move-{}", stamp())));
    }
    if let Err(error) = fs::rename(&moving, &key) {
        say(format!("can't put the moved machine key in place in {}: {error}", folder.display()));
    }
}

pub fn abandon(receiver: &Path) {
    let _ = fs::remove_file(receiver.join(MOVING_DB));
    let _ = fs::remove_file(receiver.join(format!("{MOVING_DB}-journal")));
    let _ = fs::remove_file(receiver.join(MOVING_KEY));
}

pub fn leaving(state: &Path, id: &str, to: &Path) -> Result<(), String> {
    let path = state.join(LEAVING);
    fs::write(&path, json!({ "id": id, "to": to }).to_string()).map_err(|e| format!("can't write {}: {e}", path.display()))
}

pub fn stay(state: &Path) {
    let _ = fs::remove_file(state.join(LEAVING));
}

fn aside(state: &Path, name: &str, to: &str) {
    let from = state.join(name);
    if from.exists() {
        let _ = fs::rename(&from, state.join(to));
    }
}

fn retire(state: &Path, to: &str) -> PathBuf {
    let stamp = stamp();
    let kept = format!("{DB}.moved-{stamp}");
    aside(state, &format!("{DB}-wal"), &format!("{kept}-wal"));
    aside(state, &format!("{DB}-shm"), &format!("{kept}-shm"));
    aside(state, DB, &kept);
    aside(state, &format!("{DB}.bak"), &format!("{kept}.bak"));
    aside(state, "backlog.json.imported", &format!("backlog.json.imported.moved-{stamp}"));
    aside(state, KEY, &format!("{KEY}.moved-{stamp}"));
    let _ = fs::remove_file(state.join(LEAVING));
    say(format!("this Kanna moved to {to}: it starts again empty, with a machine key of its own; its records are in {}", state.join(&kept).display()));
    state.join(kept)
}

pub fn settle(state: &Path) -> Option<PathBuf> {
    if state.join(MOVING_DB).exists() {
        abandon(state);
        say(format!("a move into {} didn't finish, so it's as it was", state.display()));
    } else {
        install_key(state);
    }
    let leaving: Value = serde_json::from_str(&fs::read_to_string(state.join(LEAVING)).ok()?).ok()?;
    let (id, to) = (leaving.get("id").and_then(Value::as_str)?, PathBuf::from(leaving.get("to").and_then(Value::as_str)?));
    if moved_from(&to).is_some_and(|m| m.get("id").and_then(Value::as_str) == Some(id)) {
        return Some(retire(state, &to.display().to_string()));
    }
    stay(state);
    say(format!("the move to {} didn't finish, so this Kanna is as it was", to.display()));
    None
}

pub fn keep_from(conn: &Connection, old: &Path) -> rusqlite::Result<()> {
    conn.execute("attach database ?1 as old", [old.to_string_lossy()])?;
    let copied = (|| {
        conn.execute("insert or replace into layout (id, doc) select id, doc from old.layout", [])?;
        conn.execute("insert or replace into projects (path, at, seq) select path, at, seq from old.projects", [])?;
        conn.execute("insert or replace into never_import (path, at) select path, at from old.never_import", [])
    })();
    conn.execute("detach database old", [])?;
    copied.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-instance-move-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn records(dir: &Path) -> crate::db::Db {
        let db = crate::db::Db::open(dir);
        let conn = Connection::open(dir.join(DB)).unwrap();
        conn.execute_batch(
            "insert into tasks (id, repo, place, terminal, made_at, doc) values
               ('task-00000001', '/r', 'shelf', null, 1, '{\"name\":\"Waiting\",\"message\":\"do it\",\"after\":[\"task-00000002\"],\"on\":[\"mbp\"]}'),
               ('task-00000002', '/r', 'shelf', 'term-4', 2, '{\"name\":\"Started\",\"folder\":\"/r/.kanna-worktrees/task-00000002\",\"session\":\"s-2\",\"workspace\":\"ws-3\"}'),
               ('task-00000003', '/r', 'desk', 'term-9', 3, '{\"name\":\"Desk\",\"folder\":\"/r/.kanna-worktrees/task-00000003\",\"session\":\"s-3\",\"workspace\":\"ws-4\",\"claim\":\"mbp\"}'),
               ('task-00000004', '/r', 'done', null, 4, '{\"name\":\"Done\"}');
             insert into messages (recipient, repo, sender, at, delivered, text) values ('task-00000001', '/r', 'user', 5, null, 'later');
             insert into planners (path, doc) values ('/r', '{\"agent\":\"claude\",\"session\":\"p-1\",\"pane\":\"term-6\",\"running\":true}');
             insert into layout (id, doc) values (1, '{\"workspaces\":[\"source\"]}');",
        )
        .unwrap();
        db
    }

    fn moving(dir: &Path) -> Prepared {
        Prepared { id: "m-1".into(), from: "kanna-dev".into(), to: label(dir), resume: vec!["task-00000002".into()] }
    }

    #[test]
    fn the_copy_puts_every_live_task_on_the_shelf_without_the_old_keepers_terminals() {
        let (source, receiver) = (scratch("src"), scratch("dst"));
        let db = records(&source);
        db.backup_to(&receiver.join(MOVING_DB)).unwrap();
        let moved = prepare(&receiver.join(MOVING_DB), &receiver, &moving(&receiver)).unwrap();
        assert_eq!(moved, Moved { waiting: vec!["task-00000001".into()], started: vec!["task-00000002".into()], desk: vec!["task-00000003".into()] });
        commit(&receiver).unwrap();
        let conn = Connection::open(receiver.join(DB)).unwrap();
        let rows: Vec<(String, String, Option<String>, String)> = conn.prepare("select id, place, terminal, doc from tasks order by id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect();
        let doc = |n: usize| serde_json::from_str::<Value>(&rows[n].3).unwrap();
        assert_eq!(rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["shelf", "shelf", "shelf", "done"]);
        assert!(rows.iter().all(|r| r.2.is_none()), "no terminal: they were the old keeper's");
        assert_eq!((doc(0)["message"].clone(), doc(0)["after"].clone(), doc(0)["on"].clone(), doc(0)["stopped"].clone()), (json!("do it"), json!(["task-00000002"]), json!(["mbp"]), Value::Null), "a waiting task as it was");
        assert_eq!((doc(1)["session"].clone(), doc(1)["workspace"].clone(), doc(1)["stopped"].clone()), (json!("s-2"), Value::Null, json!("moved from kanna-dev")));
        assert_eq!((doc(2)["session"].clone(), doc(2)["claim"].clone()), (json!("s-3"), Value::Null), "the Desk task is on the Shelf, its session kept");
        let events: i64 = conn.query_row("select count(*) from task_events where what = 'moved' and to_place = 'shelf'", [], |r| r.get(0)).unwrap();
        assert_eq!(events, 3);
        let desk_from: String = conn.query_row("select from_place from task_events where what = 'moved' and task = 'task-00000003'", [], |r| r.get(0)).unwrap();
        assert_eq!(desk_from, "desk");
        let waiting: i64 = conn.query_row("select count(*) from messages where delivered is null", [], |r| r.get(0)).unwrap();
        assert_eq!(waiting, 1, "a message still waiting goes with it");
        let planner: String = conn.query_row("select doc from planners", [], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&planner).unwrap(), json!({ "agent": "claude", "session": "p-1", "running": false }));
        let layout: i64 = conn.query_row("select count(*) from layout", [], |r| r.get(0)).unwrap();
        assert_eq!(layout, 0, "the old keeper's layout stays behind");
        let mark = moved_from(&receiver).unwrap();
        assert_eq!((mark["id"].clone(), mark["resume"].clone()), (json!("m-1"), json!(["task-00000002"])));
        drop(conn);
        assert_eq!(crate::db::Db::open(&receiver).tasks().unwrap().len(), 3, "the receiving daemon opens the live ones");
    }

    #[test]
    fn a_receiver_keeps_its_own_layout_and_a_copy_of_its_own_records() {
        let (source, receiver) = (scratch("src2"), scratch("dst2"));
        let db = records(&source);
        let own = crate::db::Db::open(&receiver);
        Connection::open(receiver.join(DB)).unwrap().execute("insert into layout (id, doc) values (1, '{\"workspaces\":[\"receiver\"]}')", []).unwrap();
        drop(own);
        assert!(live_tasks(&receiver).unwrap().is_empty());
        db.backup_to(&receiver.join(MOVING_DB)).unwrap();
        prepare(&receiver.join(MOVING_DB), &receiver, &moving(&receiver)).unwrap();
        commit(&receiver).unwrap();
        let conn = Connection::open(receiver.join(DB)).unwrap();
        let layout: String = conn.query_row("select doc from layout", [], |r| r.get(0)).unwrap();
        assert_eq!(layout, "{\"workspaces\":[\"receiver\"]}");
        let kept: Vec<_> = fs::read_dir(&receiver).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("kanna.db.before-move-")).collect();
        assert_eq!(kept.len(), 1, "its own records are kept beside");
        assert!(!receiver.join("kanna.db-wal").exists(), "no WAL of the old records beside the new ones");
    }

    #[test]
    fn a_move_stopped_before_its_commit_leaves_both_sides_as_they_were() {
        let (source, receiver) = (scratch("src3"), scratch("dst3"));
        let db = records(&source);
        fs::write(source.join(KEY), "key").unwrap();
        leaving(&source, "m-1", &receiver).unwrap();
        db.backup_to(&receiver.join(MOVING_DB)).unwrap();
        stage_key(&source, &receiver).unwrap();
        drop(db);
        assert_eq!(settle(&receiver), None);
        assert!(!receiver.join(MOVING_DB).exists() && !receiver.join(MOVING_KEY).exists() && !receiver.join(KEY).exists(), "the receiver throws the half-made copy away");
        assert_eq!(settle(&source), None);
        assert!(source.join(KEY).exists() && source.join(DB).exists() && !source.join(LEAVING).exists(), "the source is as it was");
    }

    #[test]
    fn a_source_stopped_after_the_commit_finishes_its_move_when_it_starts() {
        let (source, receiver) = (scratch("src4"), scratch("dst4"));
        let db = records(&source);
        fs::write(source.join(KEY), "key").unwrap();
        leaving(&source, "m-1", &receiver).unwrap();
        db.backup_to(&receiver.join(MOVING_DB)).unwrap();
        prepare(&receiver.join(MOVING_DB), &receiver, &moving(&receiver)).unwrap();
        stage_key(&source, &receiver).unwrap();
        fs::rename(receiver.join(MOVING_DB), receiver.join(DB)).unwrap();
        drop(db);
        assert_eq!(settle(&receiver), None);
        assert_eq!(fs::read_to_string(receiver.join(KEY)).unwrap(), "key", "the receiver takes the machine key when it starts");
        let old = settle(&source).expect("the source retires");
        assert!(!source.join(KEY).exists() && !source.join(DB).exists() && !source.join(LEAVING).exists());
        let fresh = crate::db::Db::open(&source);
        fresh.keep_from_moved(&old).unwrap();
        assert!(fresh.tasks().unwrap().is_empty(), "it starts again empty");
        let conn = Connection::open(source.join(DB)).unwrap();
        let layout: String = conn.query_row("select doc from layout", [], |r| r.get(0)).unwrap();
        assert_eq!(layout, "{\"workspaces\":[\"source\"]}", "its windows stay, for its terminals");
    }

    #[test]
    fn a_folder_resolves_by_name_in_the_support_folder_or_by_path() {
        assert_eq!(resolve("kanna-staging", Path::new("/S")), PathBuf::from("/S/kanna-staging"));
        assert_eq!(resolve("/x/state", Path::new("/S")), PathBuf::from("/x/state"));
    }

    #[test]
    fn a_running_receiver_refuses_the_lock() {
        let dir = scratch("lock");
        let held = lock(&dir).unwrap();
        assert!(lock(&dir).unwrap_err().contains("is running"));
        drop(held);
        assert!(lock(&dir).is_ok());
    }
}
