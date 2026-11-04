use crate::Event;
use kanna_log::TaskRef;
use kanna_mux::json::{string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError};
use notify::{RecursiveMode, Watcher};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

const QUIET: Duration = Duration::from_millis(200);

enum Control {
    Tasks(Vec<TaskRef>),
    Changed(Vec<PathBuf>),
    Stop,
}

struct Watch {
    folder: String,
    args: Vec<String>,
    control: Sender<Control>,
    views: HashSet<String>,
    log: Option<Value>,
    serial: u64,
}

impl Watch {
    fn reply(&self, id: &str) -> Map {
        let mut out = Map::new();
        out.insert("content".into(), json!(id));
        out.insert("serial".into(), json!(self.serial));
        out.insert("log".into(), self.log.clone().unwrap_or(Value::Null));
        out
    }
}

pub struct Logs {
    events: crate::runtime::Events,
    watches: RefCell<HashMap<String, Watch>>,
    tasks: RefCell<Vec<TaskRef>>,
    pub task_refs: RefCell<Option<Box<dyn Fn() -> Vec<TaskRef>>>>,
    pub on_log: RefCell<Option<Box<dyn Fn(&[String], &Value)>>>,
}

pub fn log_id(folder: &str, args: &[String]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"log\0");
    hash.update(folder.as_bytes());
    for arg in args {
        hash.update([0]);
        hash.update(arg.as_bytes());
    }
    let digest = hash.finalize();
    format!("log-{}", digest.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>())
}

impl Logs {
    pub fn new(events: crate::runtime::Events) -> Logs {
        Logs { events, watches: RefCell::new(HashMap::new()), tasks: RefCell::new(vec![]), task_refs: RefCell::new(None), on_log: RefCell::new(None) }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let logs = Rc::downgrade(self);
        core.add_command(
            "logs",
            Rc::new(move |_, responder| {
                let Some(logs) = logs.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                let mut out = Map::new();
                out.insert("logs".into(), logs.list());
                responder.reply(Ok(out))
            }),
        );
        core.add_command(
            "log.commit",
            Rc::new(move |args, responder| {
                let (Some(folder), Some(hash)) = (string(&args, "path").map(String::from), string(&args, "hash").map(String::from)) else {
                    return responder.reply(Err(KmuxError::new("bad_request", "log.commit needs the folder (path) and the commit (hash)")));
                };
                std::thread::spawn(move || {
                    responder.reply(match kanna_log::commit(&folder, &hash) {
                        Ok(Value::Object(detail)) => Ok(Map::from_iter([("commit".to_string(), Value::Object(detail))])),
                        Ok(_) => Err(KmuxError::new("failed", "git show said something unexpected")),
                        Err(error) => Err(KmuxError::new("git", error)),
                    })
                });
            }),
        );
    }

    fn list(&self) -> Value {
        let watches = self.watches.borrow();
        let mut ids: Vec<&String> = watches.keys().collect();
        ids.sort();
        Value::Array(
            ids.into_iter()
                .map(|id| {
                    let watch = &watches[id];
                    let mut views: Vec<&String> = watch.views.iter().collect();
                    views.sort();
                    json!({ "id": id, "folder": watch.folder, "args": watch.args, "views": views, "serial": watch.serial })
                })
                .collect(),
        )
    }

    fn refresh_tasks(&self) -> Vec<TaskRef> {
        let tasks = self.task_refs.borrow().as_ref().map(|refs| refs()).unwrap_or_default();
        *self.tasks.borrow_mut() = tasks.clone();
        tasks
    }

    pub fn subscribe(&self, view: &str, args: &Value) -> Result<Map, KmuxError> {
        let folder = string(args, "path").filter(|f| !f.is_empty()).ok_or_else(|| KmuxError::new("bad_request", "a log needs the folder it's in (path)"))?;
        let git_args = match args.get("args") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(list)) => list.iter().map(|a| a.as_str().map(String::from).ok_or_else(|| KmuxError::new("bad_request", "a log's args are strings"))).collect::<Result<_, _>>()?,
            Some(_) => return Err(KmuxError::new("bad_request", "a log's args are a list")),
        };
        let id = log_id(folder, &git_args);
        if self.watches.borrow().get(&id).is_some_and(|w| w.views.contains(view)) {
            return Ok(self.watches.borrow()[&id].reply(&id));
        }
        self.unsubscribe(view);
        if self.watches.borrow().is_empty() {
            self.refresh_tasks();
        }
        let mut watches = self.watches.borrow_mut();
        let watch = watches.entry(id.clone()).or_insert_with(|| self.start(&id, folder, &git_args));
        watch.views.insert(view.to_string());
        Ok(watch.reply(&id))
    }

    fn start(&self, id: &str, folder: &str, args: &[String]) -> Watch {
        let (control, inbox) = mpsc::channel();
        let events = self.events.clone();
        let changes = control.clone();
        let tasks = self.tasks.borrow().clone();
        let (thread_id, thread_folder, thread_args) = (id.to_string(), folder.to_string(), args.to_vec());
        let _ = std::thread::Builder::new().name(format!("kanna.{id}")).spawn(move || work(thread_id, thread_folder, thread_args, tasks, inbox, changes, events));
        Watch { folder: folder.into(), args: args.to_vec(), control, views: HashSet::new(), log: None, serial: 0 }
    }

    pub fn tasks_changed(&self) {
        if self.watches.borrow().is_empty() {
            return;
        }
        let before = self.tasks.borrow().clone();
        let tasks = self.refresh_tasks();
        if tasks == before {
            return;
        }
        for watch in self.watches.borrow().values() {
            let _ = watch.control.send(Control::Tasks(tasks.clone()));
        }
    }

    pub fn unsubscribe(&self, view: &str) {
        self.leave(|v| v == view);
    }

    pub fn leave(&self, gone: impl Fn(&str) -> bool) {
        self.watches.borrow_mut().retain(|_, watch| {
            watch.views.retain(|v| !gone(v));
            let keep = !watch.views.is_empty();
            if !keep {
                let _ = watch.control.send(Control::Stop);
            }
            keep
        });
    }

    pub fn loaded(&self, id: &str, log: Value) {
        let (views, payload) = {
            let mut watches = self.watches.borrow_mut();
            let Some(watch) = watches.get_mut(id) else { return };
            if watch.log.as_ref() == Some(&log) {
                return;
            }
            watch.serial += 1;
            watch.log = Some(log);
            (watch.views.iter().cloned().collect::<Vec<_>>(), Value::Object(watch.reply(id)))
        };
        if let Some(on_log) = self.on_log.borrow().as_ref() {
            on_log(&views, &payload);
        }
    }
}

fn work(id: String, folder: String, args: Vec<String>, mut tasks: Vec<TaskRef>, inbox: Receiver<Control>, changes: Sender<Control>, events: crate::runtime::Events) {
    let log = kanna_log::load(&folder, &args, &tasks);
    let common = PathBuf::from(&log.common_dir);
    let _ = events.send(Event::Log { id: id.clone(), log: log.to_json() });
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let _ = changes.send(Control::Changed(event.paths));
        }
    })
    .ok();
    if let Some(watcher) = watcher.as_mut() {
        let _ = watcher.watch(if log.common_dir.is_empty() { Path::new(&folder) } else { &common }, RecursiveMode::Recursive);
    }
    loop {
        let mut changed: Vec<PathBuf> = vec![];
        let mut asked = false;
        match inbox.recv() {
            Ok(Control::Stop) | Err(_) => return,
            Ok(Control::Tasks(next)) => {
                tasks = next;
                asked = true;
            }
            Ok(Control::Changed(paths)) => changed.extend(paths),
        }
        loop {
            match inbox.recv_timeout(QUIET) {
                Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Control::Tasks(next)) => {
                    tasks = next;
                    asked = true;
                }
                Ok(Control::Changed(paths)) => changed.extend(paths),
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        if !asked && !matters(&changed, &common) {
            continue;
        }
        let log = kanna_log::load(&folder, &args, &tasks);
        let _ = events.send(Event::Log { id: id.clone(), log: log.to_json() });
    }
}

pub fn matters(paths: &[PathBuf], common: &Path) -> bool {
    paths.iter().any(|path| {
        let Ok(inside) = path.strip_prefix(common) else { return true };
        let parts: Vec<String> = inside.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        if parts.last().is_some_and(|last| last.ends_with(".lock")) {
            return false;
        }
        match parts.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["refs", ..] | ["packed-refs"] | ["HEAD"] => true,
            ["worktrees", _, "HEAD"] => true,
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logs() -> (Logs, crate::runtime::Inbox) {
        let (events, inbox) = crate::runtime::channel();
        (Logs::new(events), inbox)
    }

    fn folder() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    #[test]
    fn views_of_one_log_share_its_worker_until_the_last_leaves() {
        let (logs, _inbox) = logs();
        let args = json!({ "path": folder(), "args": ["main"] });
        let first = logs.subscribe("app1:p5", &args).unwrap();
        let second = logs.subscribe("app1:p6", &args).unwrap();
        assert_eq!(first["content"], second["content"]);
        assert_eq!(logs.list()[0]["views"], json!(["app1:p5", "app1:p6"]));
        logs.unsubscribe("app1:p5");
        assert_eq!(logs.list()[0]["views"], json!(["app1:p6"]));
        logs.leave(|view| view.starts_with("app1:"));
        assert_eq!(logs.list(), json!([]));
    }

    #[test]
    fn a_log_and_a_diff_of_the_same_folder_are_different_content() {
        assert_ne!(log_id(&folder(), &[]), crate::diffs::diff_id(&folder(), &[]).replace("diff-", "log-"));
    }

    #[test]
    fn a_log_without_a_folder_is_refused() {
        let (logs, _inbox) = logs();
        assert_eq!(logs.subscribe("app1:p5", &json!({})).unwrap_err().code, "bad_request");
    }

    #[test]
    fn only_refs_and_heads_move_a_log() {
        let common = Path::new("/r/.git");
        let at = |p: &str| vec![common.join(p)];
        assert!(matters(&at("refs/heads/main"), common));
        assert!(matters(&at("packed-refs"), common));
        assert!(matters(&at("HEAD"), common));
        assert!(matters(&at("worktrees/task-1/HEAD"), common));
        assert!(!matters(&at("refs/heads/main.lock"), common));
        assert!(!matters(&at("objects/ab/cdef"), common));
        assert!(!matters(&at("index"), common));
        assert!(!matters(&at("worktrees/task-1/index"), common));
        assert!(!matters(&at("logs/HEAD"), common));
    }
}
