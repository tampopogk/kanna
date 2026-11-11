use kanna_keeper::paths as keeper_paths;
use kanna_mux::json::{json, Value};
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MOVING_DB: &str = "kanna.db.moving";
pub const MOVING_KEY: &str = "device.key.moving";
pub const LEAVING: &str = "moving.json";
pub const LEFT: &str = "moved.json";
pub const MOVED_FROM: &str = "moved_from";
pub const SCHEMA_FLAG: &str = "--schema";
pub const TERMINALS: &str = "terminals";
pub const APP_FILES: [&str; 2] = ["app-state.json", "app.key"];
const MOVING: &str = ".moving";
const LOCK: &str = "daemon.lock";
const DB: &str = crate::db::FILE;
const KEY: &str = crate::machines::KEY;
const LIVE: &str = "place in ('shelf', 'desk', 'backlog')";
const KEEPER_WAIT: Duration = Duration::from_secs(10);

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

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn say(message: String) {
    log!("kanna-daemon: {message}");
}

fn moving(name: &str) -> String {
    format!("{name}{MOVING}")
}

fn staged() -> Vec<(String, String)> {
    let mut staged = vec![(moving(TERMINALS), TERMINALS.to_string())];
    staged.extend(APP_FILES.iter().map(|f| (moving(f), f.to_string())));
    staged.push((MOVING_KEY.to_string(), KEY.to_string()));
    staged
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

fn flock(path: &Path, create: bool) -> Result<Option<fs::File>, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let file = match fs::OpenOptions::new().read(true).write(true).create(create).truncate(false).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(path) {
        Ok(file) => file,
        Err(e) if !create && e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("can't open {}: {e}", path.display())),
    };
    match unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } {
        0 => Ok(Some(file)),
        _ => Err(format!("{} is held", path.display())),
    }
}

pub fn lock(folder: &Path) -> Result<fs::File, String> {
    fs::create_dir_all(folder).map_err(|e| format!("can't make {}: {e}", folder.display()))?;
    let _ = fs::set_permissions(folder, fs::Permissions::from_mode(0o700));
    match flock(&folder.join(LOCK), true) {
        Ok(Some(file)) => Ok(file),
        Ok(None) => Err(format!("can't lock {}", folder.display())),
        Err(_) => Err(format!("{}'s Kanna is running: quit it (or stop its served daemon) first", label(folder))),
    }
}

pub fn keeper_running(folder: &Path) -> bool {
    flock(&keeper_paths::lock(folder), false).is_err()
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
    conn.busy_timeout(Duration::from_secs(5))?;
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
    let path = folder.join(DB);
    if !path.exists() {
        return None;
    }
    let conn = open_quiet(&path).ok()?;
    let value: String = conn.query_row("select value from meta where key = ?1", [MOVED_FROM], |r| r.get(0)).optional().ok()??;
    serde_json::from_str(&value).ok()
}

pub fn prepare(copy: &Path, mark: &Value) -> rusqlite::Result<Moved> {
    let conn = open_quiet(copy)?;
    conn.pragma_update(None, "journal_mode", "delete")?;
    let live: Vec<(String, String, String)> = {
        let mut query = conn.prepare(&format!("select id, place, doc from tasks where {LIVE} order by made_at"))?;
        let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut moved = Moved::default();
    for (id, place, doc) in live {
        let doc: Value = serde_json::from_str(&doc).unwrap_or_else(|_| json!({}));
        match (place.as_str(), doc.get("folder").and_then(Value::as_str).is_some_and(|f| !f.is_empty())) {
            ("desk", _) => moved.desk.push(id),
            (_, true) => moved.started.push(id),
            _ => moved.waiting.push(id),
        }
    }
    conn.execute("insert into meta (key, value) values (?1, ?2) on conflict (key) do update set value = excluded.value", params![MOVED_FROM, mark.to_string()])?;
    conn.close().map_err(|(_, e)| e)?;
    let _ = fs::set_permissions(copy, fs::Permissions::from_mode(0o600));
    Ok(moved)
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let bytes = fs::read(from).map_err(|e| format!("can't read {}: {e}", from.display()))?;
    let _ = fs::remove_file(to);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(to).map_err(|e| format!("can't write {}: {e}", to.display()))?;
    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| format!("can't write {}: {e}", to.display()))
}

fn copy_folder(from: &Path, to: &Path) -> Result<(), String> {
    let _ = fs::remove_dir_all(to);
    fs::create_dir_all(to).map_err(|e| format!("can't make {}: {e}", to.display()))?;
    let _ = fs::set_permissions(to, fs::Permissions::from_mode(0o700));
    for entry in fs::read_dir(from).map_err(|e| format!("can't read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("can't read {}: {e}", from.display()))?;
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            copy_file(&entry.path(), &to.join(entry.file_name()))?;
        }
    }
    Ok(())
}

pub fn stage_files(state: &Path, receiver: &Path) -> Result<(), String> {
    let terminals = state.join(TERMINALS);
    if terminals.is_dir() {
        copy_folder(&terminals, &receiver.join(moving(TERMINALS)))?;
    }
    for name in APP_FILES.iter().copied().chain([KEY]) {
        let from = state.join(name);
        if from.is_file() {
            let to = if name == KEY { receiver.join(MOVING_KEY) } else { receiver.join(moving(name)) };
            copy_file(&from, &to)?;
        }
    }
    Ok(())
}

fn quiet_receiver_db(folder: &Path, stamp: &str) -> Result<(), String> {
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
    let kept = folder.join(format!("{DB}.before-move-{stamp}"));
    fs::copy(&path, &kept).map_err(|e| format!("can't keep {} as {}: {e}", path.display(), kept.display()))?;
    let _ = fs::set_permissions(&kept, fs::Permissions::from_mode(0o600));
    for extra in ["-wal", "-shm"] {
        let _ = fs::remove_file(PathBuf::from(format!("{}{extra}", path.display())));
    }
    Ok(())
}

pub fn commit(receiver: &Path) -> Result<(), String> {
    let stamp = stamp();
    quiet_receiver_db(receiver, &stamp)?;
    fs::rename(receiver.join(MOVING_DB), receiver.join(DB)).map_err(|e| format!("can't put the records in place in {}: {e}", receiver.display()))?;
    install_staged(receiver, &stamp);
    Ok(())
}

fn install_staged(folder: &Path, stamp: &str) {
    for (staged, live) in staged() {
        let from = folder.join(&staged);
        if !from.exists() {
            continue;
        }
        let to = folder.join(&live);
        if to.exists() {
            let _ = fs::rename(&to, folder.join(format!("{live}.before-move-{stamp}")));
        }
        if let Err(error) = fs::rename(&from, &to) {
            say(format!("can't put the moved {live} in place in {}: {error}", folder.display()));
        }
    }
}

pub fn abandon(receiver: &Path) {
    let _ = fs::remove_file(receiver.join(MOVING_DB));
    let _ = fs::remove_file(receiver.join(format!("{MOVING_DB}-journal")));
    for (staged, _) in staged() {
        let path = receiver.join(staged);
        let _ = fs::remove_dir_all(&path);
        let _ = fs::remove_file(&path);
    }
}

pub fn leaving(state: &Path, id: &str, to: &Path, socket: &Path) -> Result<(), String> {
    let path = state.join(LEAVING);
    fs::write(&path, json!({ "id": id, "to": to, "socket": socket }).to_string()).map_err(|e| format!("can't write {}: {e}", path.display()))
}

fn marker(state: &Path, name: &str) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(state.join(name)).ok()?).ok()
}

fn id_of(value: &Value) -> Option<&str> {
    value.get("id").and_then(Value::as_str)
}

pub fn stay(state: &Path) {
    let _ = fs::remove_file(state.join(LEAVING));
}

pub fn move_keeper(from: &Path, to: &Path) -> Result<bool, String> {
    let mut client = match kanna_keeper::client::Client::connect(&keeper_paths::socket(from)) {
        Ok(client) => client,
        Err(_) => return Ok(false),
    };
    client.set_timeout(Some(KEEPER_WAIT)).map_err(|e| e.to_string())?;
    client.move_to(to).map(|_| true).map_err(|e| format!("the keeper of {} didn't move to {}: {e}", from.display(), to.display()))
}

pub fn link_socket(at: &Path, to: &Path) -> Result<(), String> {
    if fs::read_link(at).is_ok_and(|now| now == to) {
        return Ok(());
    }
    let temporary = at.with_file_name(format!(".{}.link-{}", label(at), std::process::id()));
    let _ = fs::remove_file(&temporary);
    std::os::unix::fs::symlink(to, &temporary).map_err(|e| format!("can't make a link to {} at {}: {e}", to.display(), temporary.display()))?;
    fs::rename(&temporary, at).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        format!("can't link {} to {}: {e}", at.display(), to.display())
    })
}

fn aside(state: &Path, name: &str, to: &str) {
    let from = state.join(name);
    if from.exists() {
        let _ = fs::rename(&from, state.join(to));
    }
}

fn retire(state: &Path, to: &str, left: bool) -> PathBuf {
    let stamp = stamp();
    let kept = format!("{DB}.moved-{stamp}");
    aside(state, &format!("{DB}-wal"), &format!("{kept}-wal"));
    aside(state, &format!("{DB}-shm"), &format!("{kept}-shm"));
    aside(state, DB, &kept);
    aside(state, &format!("{DB}.bak"), &format!("{kept}.bak"));
    aside(state, "backlog.json.imported", &format!("backlog.json.imported.moved-{stamp}"));
    aside(state, KEY, &format!("{KEY}.moved-{stamp}"));
    aside(state, TERMINALS, &format!("{TERMINALS}.moved-{stamp}"));
    aside(state, APP_FILES[0], &format!("{}.moved-{stamp}", APP_FILES[0]));
    let kept = state.join(kept);
    if left {
        let mut marker = marker(state, LEAVING).unwrap_or_else(|| json!({}));
        marker["kept"] = json!(kept);
        if let Err(error) = fs::write(state.join(LEFT), marker.to_string()) {
            say(format!("can't write {}: {error}", state.join(LEFT).display()));
        }
    }
    let _ = fs::remove_file(state.join(LEAVING));
    say(format!("this Kanna moved to {to}: it starts again empty, with a machine key of its own; its records are in {}", kept.display()));
    kept
}

fn left_behind(state: &Path, to: &Path) -> String {
    format!(
        "this Kanna moved to {}, but its keeper, with the moved terminals, is still in {}: open {}'s Kanna, which takes it, or end those terminals; this Kanna doesn't start until then",
        label(to),
        state.display(),
        label(to)
    )
}

fn hand_over_left(state: &Path, left: &Value) -> Result<Option<PathBuf>, String> {
    let kept = left.get("kept").and_then(Value::as_str).map(PathBuf::from);
    let done = || {
        let _ = fs::remove_file(state.join(LEFT));
        Ok(kept.clone())
    };
    let Some(to) = left.get("to").and_then(Value::as_str).map(PathBuf::from) else { return done() };
    if !keeper_running(state) {
        return done();
    }
    if moved_from(&to).as_ref().and_then(id_of) != id_of(left) {
        say(format!("{} no longer holds the move that left the keeper here, so this Kanna keeps it", to.display()));
        return done();
    }
    if !keeper_running(&to) {
        match move_keeper(state, &to) {
            Ok(true) => say(format!("its keeper moved to {}, after the move", to.display())),
            Ok(false) => {}
            Err(error) => say(error),
        }
    }
    match keeper_running(state) {
        false => done(),
        true => Err(left_behind(state, &to)),
    }
}

fn finish_leaving(state: &Path, leaving: &Value, to: &Path) -> bool {
    match move_keeper(state, to) {
        Ok(true) => say(format!("its keeper moved to {}, after the move", to.display())),
        Ok(false) => {}
        Err(error) => say(error),
    }
    if let Some(socket) = leaving.get("socket").and_then(Value::as_str).map(Path::new).filter(|s| fs::read_link(s).is_err()) {
        if let Err(error) = link_socket(socket, &to.join(kanna_build::SOCKET)) {
            say(error);
        }
    }
    keeper_running(state)
}

fn finish_arriving(state: &Path, socket: &Path, mark: &Value) {
    let Some(from) = mark.get("from_dir").and_then(Value::as_str).map(PathBuf::from) else { return };
    let Some(id) = id_of(mark) else { return };
    if ![LEAVING, LEFT].iter().any(|name| marker(&from, name).as_ref().and_then(id_of) == Some(id)) {
        return;
    }
    if !keeper_running(state) {
        match move_keeper(&from, state) {
            Ok(true) => say(format!("took the keeper from {}, after the move", from.display())),
            Ok(false) => {}
            Err(error) => say(error),
        }
    }
    if let Some(old) = mark.get("socket").and_then(Value::as_str) {
        if let Err(error) = link_socket(Path::new(old), socket) {
            say(error);
        }
    }
}

pub fn settle(state: &Path, socket: &Path) -> Result<Option<PathBuf>, String> {
    if state.join(MOVING_DB).exists() {
        abandon(state);
        say(format!("a move into {} didn't finish, so it's as it was", state.display()));
    } else {
        install_staged(state, &stamp());
        if let Some(mark) = moved_from(state) {
            finish_arriving(state, socket, &mark);
        }
    }
    if let Some(left) = marker(state, LEFT) {
        return hand_over_left(state, &left);
    }
    let Some(leaving) = marker(state, LEAVING) else { return Ok(None) };
    let (Some(id), Some(to)) = (id_of(&leaving), leaving.get("to").and_then(Value::as_str).map(PathBuf::from)) else {
        stay(state);
        return Ok(None);
    };
    if moved_from(&to).as_ref().and_then(id_of) == Some(id) {
        let left = finish_leaving(state, &leaving, &to);
        let kept = retire(state, &to.display().to_string(), left);
        return match left {
            true => Err(left_behind(state, &to)),
            false => Ok(Some(kept)),
        };
    }
    stay(state);
    say(format!("the move to {} didn't finish, so this Kanna is as it was", to.display()));
    Ok(None)
}

pub fn keep_from(conn: &Connection, old: &Path) -> rusqlite::Result<()> {
    conn.execute("attach database ?1 as old", [old.to_string_lossy()])?;
    let copied = (|| {
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
        let db = crate::db::Db::open(dir).unwrap();
        let conn = Connection::open(dir.join(DB)).unwrap();
        conn.execute_batch(
            "insert into tasks (id, repo, place, terminal, made_at, doc) values
               ('task-00000001', '/r', 'shelf', null, 1, '{\"name\":\"Waiting\",\"message\":\"do it\",\"after\":[\"task-00000002\"],\"on\":[\"mbp\"]}'),
               ('task-00000002', '/r', 'shelf', 'term-4', 2, '{\"name\":\"Started\",\"folder\":\"/r/.kanna-worktrees/task-00000002\",\"session\":\"s-2\",\"workspace\":\"ws-3\"}'),
               ('task-00000003', '/r', 'desk', 'term-9', 3, '{\"name\":\"Desk\",\"folder\":\"/r/.kanna-worktrees/task-00000003\",\"session\":\"s-3\",\"workspace\":\"ws-4\"}'),
               ('task-00000004', '/r', 'done', null, 4, '{\"name\":\"Done\"}');
             insert into messages (recipient, repo, sender, at, delivered, text) values ('task-00000001', '/r', 'user', 5, null, 'later');
             insert into planners (path, doc) values ('/r', '{\"agent\":\"claude\",\"session\":\"p-1\",\"pane\":\"term-6\",\"running\":true}');
             insert into layout (id, doc) values (1, '{\"workspaces\":[\"source\"]}');
             insert into projects (path, at, seq) values ('/r', 1, 1);",
        )
        .unwrap();
        fs::create_dir_all(dir.join(TERMINALS)).unwrap();
        fs::write(dir.join(TERMINALS).join("terminals.json"), "{\"terminals\":[\"term-4\"]}").unwrap();
        fs::write(dir.join(TERMINALS).join("term-4.snap"), "screen").unwrap();
        fs::write(dir.join("app-state.json"), "{\"windows\":1}").unwrap();
        fs::write(dir.join(KEY), "key").unwrap();
        db
    }

    fn mark(source: &Path, receiver: &Path) -> Value {
        json!({ "id": "m-1", "from": label(source), "from_dir": source, "to": label(receiver), "at": 1.0, "socket": source.join("kanna.sock") })
    }

    fn staged_move(source: &Path, receiver: &Path) -> (crate::db::Db, Moved) {
        let db = records(source);
        leaving(source, "m-1", receiver, &source.join("kanna.sock")).unwrap();
        db.backup_to(&receiver.join(MOVING_DB)).unwrap();
        let moved = prepare(&receiver.join(MOVING_DB), &mark(source, receiver)).unwrap();
        stage_files(source, receiver).unwrap();
        (db, moved)
    }

    #[test]
    fn the_copy_keeps_every_task_where_it_was_with_its_terminals_and_layout() {
        let (source, receiver) = (scratch("src"), scratch("dst"));
        let (_db, moved) = staged_move(&source, &receiver);
        assert_eq!(moved, Moved { waiting: vec!["task-00000001".into()], started: vec!["task-00000002".into()], desk: vec!["task-00000003".into()] });
        commit(&receiver).unwrap();
        let conn = Connection::open(receiver.join(DB)).unwrap();
        let rows: Vec<(String, String, Option<String>, String)> = conn.prepare("select id, place, terminal, doc from tasks order by id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(), ["shelf", "shelf", "desk", "done"]);
        assert_eq!(rows.iter().map(|r| r.2.as_deref()).collect::<Vec<_>>(), [None, Some("term-4"), Some("term-9"), None], "each keeps its terminal: the keeper moves too");
        let waiting: Value = serde_json::from_str(&rows[0].3).unwrap();
        assert_eq!((waiting["message"].clone(), waiting["after"].clone(), waiting["on"].clone()), (json!("do it"), json!(["task-00000002"]), json!(["mbp"])));
        let planner: String = conn.query_row("select doc from planners", [], |r| r.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&planner).unwrap()["pane"], json!("term-6"));
        let layout: String = conn.query_row("select doc from layout", [], |r| r.get(0)).unwrap();
        assert_eq!(layout, "{\"workspaces\":[\"source\"]}", "its windows come with it");
        let waiting: i64 = conn.query_row("select count(*) from messages where delivered is null", [], |r| r.get(0)).unwrap();
        assert_eq!(waiting, 1);
        assert_eq!(moved_from(&receiver).unwrap()["id"], json!("m-1"));
        assert_eq!(fs::read_to_string(receiver.join(TERMINALS).join("term-4.snap")).unwrap(), "screen");
        assert_eq!(fs::read_to_string(receiver.join("app-state.json")).unwrap(), "{\"windows\":1}");
        assert_eq!(fs::read_to_string(receiver.join(KEY)).unwrap(), "key");
        assert!(staged().iter().all(|(s, _)| !receiver.join(s).exists()));
    }

    #[test]
    fn a_receiver_keeps_a_copy_of_its_own_records_and_terminals() {
        let (source, receiver) = (scratch("src2"), scratch("dst2"));
        drop(crate::db::Db::open(&receiver).unwrap());
        fs::create_dir_all(receiver.join(TERMINALS)).unwrap();
        fs::write(receiver.join(KEY), "own").unwrap();
        assert!(live_tasks(&receiver).unwrap().is_empty());
        staged_move(&source, &receiver);
        commit(&receiver).unwrap();
        let kept: Vec<String> = fs::read_dir(&receiver).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".before-move-")).collect();
        assert_eq!(kept.len(), 3, "its records, terminals and key are kept beside: {kept:?}");
        assert!(!receiver.join("kanna.db-wal").exists());
    }

    #[test]
    fn a_move_stopped_before_its_commit_leaves_both_sides_as_they_were() {
        let (source, receiver) = (scratch("src3"), scratch("dst3"));
        let (db, _) = staged_move(&source, &receiver);
        drop(db);
        assert_eq!(settle(&receiver, &receiver.join("kanna.sock")), Ok(None));
        assert!(fs::read_dir(&receiver).unwrap().next().is_none(), "the receiver throws the half-made copy away");
        assert_eq!(settle(&source, &source.join("kanna.sock")), Ok(None));
        assert!(source.join(KEY).exists() && source.join(DB).exists() && source.join(TERMINALS).exists() && !source.join(LEAVING).exists(), "the source is as it was");
    }

    #[test]
    fn a_move_stopped_after_its_commit_links_the_old_socket_and_retires_the_source() {
        let (source, receiver) = (scratch("src4"), scratch("dst4"));
        let (db, _) = staged_move(&source, &receiver);
        fs::rename(receiver.join(MOVING_DB), receiver.join(DB)).unwrap();
        drop(db);
        assert_eq!(settle(&receiver, &receiver.join("kanna.sock")), Ok(None));
        assert_eq!(fs::read_to_string(receiver.join(KEY)).unwrap(), "key", "the receiver finishes putting things in place");
        assert!(receiver.join(TERMINALS).join("term-4.snap").exists());
        assert_eq!(fs::read_link(source.join("kanna.sock")).unwrap(), receiver.join("kanna.sock"), "the old socket leads to the receiver");
        let old = settle(&source, &source.join("kanna.sock")).unwrap().expect("the source retires");
        assert!(!source.join(KEY).exists() && !source.join(DB).exists() && !source.join(TERMINALS).exists() && !source.join(LEAVING).exists());
        let fresh = crate::db::Db::open(&source).unwrap();
        fresh.keep_from_moved(&old).unwrap();
        assert!(fresh.tasks().unwrap().is_empty(), "it starts again empty");
        let conn = Connection::open(source.join(DB)).unwrap();
        let projects: i64 = conn.query_row("select count(*) from projects", [], |r| r.get(0)).unwrap();
        let layout: i64 = conn.query_row("select count(*) from layout", [], |r| r.get(0)).unwrap();
        assert_eq!((projects, layout), (1, 0), "its recent projects stay, its windows went with the terminals");
        assert_eq!(kanna_build::own_socket(&source.join("kanna.sock")), source.join(kanna_build::OWN_SOCKET));
    }

    #[test]
    fn a_keeper_left_behind_is_kept_for_the_receiver_after_the_source_retires() {
        let (source, receiver) = (scratch("src5"), scratch("dst5"));
        let (db, _) = staged_move(&source, &receiver);
        fs::rename(receiver.join(MOVING_DB), receiver.join(DB)).unwrap();
        drop(db);
        let keeper = flock(&keeper_paths::lock(&source), true).unwrap();
        let refused = settle(&source, &source.join("kanna.sock")).unwrap_err();
        assert!(refused.contains("still in") && refused.contains(&label(&receiver)), "{refused}");
        assert!(!source.join(DB).exists() && !source.join(LEAVING).exists(), "the source retires all the same");
        assert_eq!(marker(&source, LEFT).as_ref().and_then(id_of), Some("m-1"), "the move's id stays beside the keeper");
        assert!(settle(&source, &source.join("kanna.sock")).is_err(), "it doesn't start while the keeper holds the moved terminals");
        let _ = fs::remove_file(source.join("kanna.sock"));
        assert_eq!(settle(&receiver, &receiver.join("kanna.sock")), Ok(None));
        assert_eq!(fs::read_link(source.join("kanna.sock")).unwrap(), receiver.join("kanna.sock"), "the receiver knows the move by its id, the source's moving.json gone");
        drop(keeper);
        let kept = settle(&source, &source.join("kanna.sock")).unwrap().expect("the source starts once its keeper is gone");
        assert!(kept.file_name().unwrap().to_string_lossy().starts_with("kanna.db.moved-"), "{kept:?}");
        assert!(!source.join(LEFT).exists());
        let _ = fs::remove_file(source.join("kanna.sock"));
        assert_eq!(settle(&receiver, &receiver.join("kanna.sock")), Ok(None));
        assert!(fs::symlink_metadata(source.join("kanna.sock")).is_err(), "with the move done, the receiver leaves the old folder alone");
    }

    #[test]
    fn moved_from_alone_takes_nothing_from_the_old_folder() {
        let (source, receiver) = (scratch("src6"), scratch("dst6"));
        let (db, _) = staged_move(&source, &receiver);
        fs::rename(receiver.join(MOVING_DB), receiver.join(DB)).unwrap();
        drop(db);
        stay(&source);
        fs::write(source.join(LEFT), json!({ "id": "m-other", "to": receiver }).to_string()).unwrap();
        assert_eq!(settle(&receiver, &receiver.join("kanna.sock")), Ok(None));
        assert!(fs::symlink_metadata(source.join("kanna.sock")).is_err(), "another move's marker isn't this one");
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
        assert!(!keeper_running(&dir));
        let keeper = flock(&keeper_paths::lock(&dir), true).unwrap();
        assert!(keeper_running(&dir));
        drop(keeper);
    }
}
