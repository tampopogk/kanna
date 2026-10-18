use crate::Event;
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
    Show(HashSet<String>),
    Changed(Vec<PathBuf>),
    Stop,
}

struct Watch {
    folder: String,
    args: Vec<String>,
    control: Sender<Control>,
    views: HashMap<String, HashSet<String>>,
    show: HashSet<String>,
    diff: Option<Value>,
    serial: u64,
    updating: bool,
}

impl Watch {
    fn reply(&self, id: &str) -> Map {
        let mut out = Map::new();
        out.insert("content".into(), json!(id));
        out.insert("serial".into(), json!(self.serial));
        out.insert("updating".into(), json!(self.updating));
        out.insert("diff".into(), self.diff.clone().unwrap_or(Value::Null));
        out
    }
}

pub struct Diffs {
    events: Sender<Event>,
    watches: RefCell<HashMap<String, Watch>>,
    pub on_diff: RefCell<Option<Box<dyn Fn(&[String], &Value)>>>,
}

pub fn diff_id(folder: &str, args: &[String]) -> String {
    let mut hash = Sha256::new();
    hash.update(folder.as_bytes());
    for arg in args {
        hash.update([0]);
        hash.update(arg.as_bytes());
    }
    let digest = hash.finalize();
    format!("diff-{}", digest.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>())
}

impl Diffs {
    pub fn new(events: Sender<Event>) -> Diffs {
        Diffs { events, watches: RefCell::new(HashMap::new()), on_diff: RefCell::new(None) }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let diffs = Rc::downgrade(self);
        core.add_command(
            "diffs",
            Rc::new(move |_, responder| {
                let Some(diffs) = diffs.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                let mut out = Map::new();
                out.insert("diffs".into(), diffs.list());
                responder.reply(Ok(out))
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
                    let mut views: Vec<&String> = watch.views.keys().collect();
                    views.sort();
                    json!({ "id": id, "folder": watch.folder, "args": watch.args, "views": views, "serial": watch.serial })
                })
                .collect(),
        )
    }

    pub fn subscribe(&self, view: &str, args: &Value) -> Result<Map, KmuxError> {
        let folder = string(args, "path").filter(|f| !f.is_empty()).ok_or_else(|| KmuxError::new("bad_request", "a diff needs the folder it's in (path)"))?;
        let git_args = match args.get("args") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(list)) => list.iter().map(|a| a.as_str().map(String::from).ok_or_else(|| KmuxError::new("bad_request", "a diff's args are strings"))).collect::<Result<_, _>>()?,
            Some(_) => return Err(KmuxError::new("bad_request", "a diff's args are a list")),
        };
        let id = diff_id(folder, &git_args);
        if self.watches.borrow().get(&id).is_some_and(|w| w.views.contains_key(view)) {
            return Ok(self.watches.borrow()[&id].reply(&id));
        }
        self.unsubscribe(view);
        let mut watches = self.watches.borrow_mut();
        let watch = watches.entry(id.clone()).or_insert_with(|| self.start(&id, folder, &git_args));
        watch.views.insert(view.to_string(), HashSet::new());
        Ok(watch.reply(&id))
    }

    fn start(&self, id: &str, folder: &str, args: &[String]) -> Watch {
        let (control, inbox) = mpsc::channel();
        let events = self.events.clone();
        let changes = control.clone();
        let (thread_id, thread_folder, thread_args) = (id.to_string(), folder.to_string(), args.to_vec());
        let _ = std::thread::Builder::new().name(format!("kanna.{id}")).spawn(move || work(thread_id, thread_folder, thread_args, inbox, changes, events));
        Watch { folder: folder.into(), args: args.to_vec(), control, views: HashMap::new(), show: HashSet::new(), diff: None, serial: 0, updating: true }
    }

    pub fn show(&self, view: &str, show: &Value) {
        let show: HashSet<String> = show.as_array().map_or(HashSet::new(), |list| list.iter().filter_map(Value::as_str).map(String::from).collect());
        let mut watches = self.watches.borrow_mut();
        let Some(watch) = watches.values_mut().find(|w| w.views.contains_key(view)) else { return };
        watch.views.insert(view.to_string(), show);
        Self::reshow(watch);
    }

    fn reshow(watch: &mut Watch) {
        let union: HashSet<String> = watch.views.values().flatten().cloned().collect();
        if union != watch.show {
            watch.show = union.clone();
            watch.updating = true;
            let _ = watch.control.send(Control::Show(union));
        }
    }

    pub fn unsubscribe(&self, view: &str) {
        self.leave(|v| v == view);
    }

    pub fn leave(&self, gone: impl Fn(&str) -> bool) {
        let mut watches = self.watches.borrow_mut();
        for watch in watches.values_mut() {
            let before = watch.views.len();
            watch.views.retain(|v, _| !gone(v));
            if watch.views.len() != before && !watch.views.is_empty() {
                Self::reshow(watch);
            }
        }
        watches.retain(|_, watch| {
            let keep = !watch.views.is_empty();
            if !keep {
                let _ = watch.control.send(Control::Stop);
            }
            keep
        });
    }

    fn tell(&self, id: &str, payload: Value) {
        let views: Vec<String> = self.watches.borrow().get(id).map_or(vec![], |w| w.views.keys().cloned().collect());
        if let Some(on_diff) = self.on_diff.borrow().as_ref() {
            on_diff(&views, &payload);
        }
    }

    pub fn updating(&self, id: &str) {
        let Some(()) = self.watches.borrow_mut().get_mut(id).map(|watch| watch.updating = true) else { return };
        self.tell(id, json!({ "content": id, "updating": true }));
    }

    pub fn loaded(&self, id: &str, diff: Value) {
        let payload = {
            let mut watches = self.watches.borrow_mut();
            let Some(watch) = watches.get_mut(id) else { return };
            watch.updating = false;
            if watch.diff.as_ref() == Some(&diff) {
                json!({ "content": id, "updating": false })
            } else {
                watch.serial += 1;
                watch.diff = Some(diff);
                Value::Object(watch.reply(id))
            }
        };
        self.tell(id, payload);
    }
}

fn work(id: String, folder: String, args: Vec<String>, inbox: Receiver<Control>, changes: Sender<Control>, events: Sender<Event>) {
    let mut show = HashSet::new();
    let diff = kanna_diff::load(&folder, &args, &show);
    let top = PathBuf::from(&diff.top);
    let dirs: Vec<PathBuf> = [&diff.git_dir, &diff.common_dir].into_iter().filter(|d| !d.is_empty()).map(PathBuf::from).collect();
    let _ = events.send(Event::Diff { id: id.clone(), diff: diff.to_json() });
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let _ = changes.send(Control::Changed(event.paths));
        }
    })
    .ok();
    if let Some(watcher) = watcher.as_mut() {
        let root = if diff.top.is_empty() { PathBuf::from(&folder) } else { top.clone() };
        let _ = watcher.watch(&root, RecursiveMode::Recursive);
        for dir in &dirs {
            if !dir.starts_with(&root) {
                let _ = watcher.watch(dir, RecursiveMode::Recursive);
            }
        }
    }
    loop {
        let mut changed: Vec<PathBuf> = vec![];
        let mut asked = false;
        match inbox.recv() {
            Ok(Control::Stop) | Err(_) => return,
            Ok(Control::Show(next)) => {
                show = next;
                asked = true;
            }
            Ok(Control::Changed(paths)) => changed.extend(paths),
        }
        loop {
            match inbox.recv_timeout(QUIET) {
                Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Control::Show(next)) => {
                    show = next;
                    asked = true;
                }
                Ok(Control::Changed(paths)) => changed.extend(paths),
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        if !asked && !matters(&changed, &top, &dirs) {
            continue;
        }
        let _ = events.send(Event::DiffUpdating { id: id.clone() });
        let diff = kanna_diff::load(&folder, &args, &show);
        let _ = events.send(Event::Diff { id: id.clone(), diff: diff.to_json() });
    }
}

fn matters(paths: &[PathBuf], top: &Path, dirs: &[PathBuf]) -> bool {
    let mut files = vec![];
    for path in paths {
        if let Some(dir) = dirs.iter().find(|d| path.starts_with(d)) {
            let inside = path.strip_prefix(dir).unwrap_or(path);
            let first = inside.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
            if !matches!(first.as_str(), "objects" | "logs" | "worktrees" | "hooks" | "lfs") && !inside.to_string_lossy().ends_with(".lock") {
                return true;
            }
            continue;
        }
        if let Ok(inside) = path.strip_prefix(top) {
            if inside.components().next().is_some_and(|c| c.as_os_str() == ".git") {
                continue;
            }
            files.push(inside.to_string_lossy().into_owned());
        }
    }
    if files.is_empty() {
        return false;
    }
    let ignored = kanna_diff::ignored(&files, &top.to_string_lossy());
    files.iter().any(|f| !ignored.contains(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diffs() -> (Diffs, Receiver<Event>) {
        let (events, inbox) = mpsc::channel();
        (Diffs::new(events), inbox)
    }

    fn folder() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    #[test]
    fn views_of_one_diff_share_its_watcher_until_the_last_leaves() {
        let (diffs, _inbox) = diffs();
        let args = json!({ "path": folder(), "args": ["--staged"] });
        let first = diffs.subscribe("app1:p5", &args).unwrap();
        let second = diffs.subscribe("app1:p6", &args).unwrap();
        assert_eq!(first["content"], second["content"]);
        assert_eq!(diffs.list().as_array().unwrap().len(), 1);
        assert_eq!(diffs.list()[0]["views"], json!(["app1:p5", "app1:p6"]));
        diffs.unsubscribe("app1:p5");
        assert_eq!(diffs.list()[0]["views"], json!(["app1:p6"]));
        diffs.unsubscribe("app1:p6");
        assert_eq!(diffs.list(), json!([]));
    }

    #[test]
    fn other_args_are_another_diff_and_a_view_moves_to_it() {
        let (diffs, _inbox) = diffs();
        let staged = diffs.subscribe("app1:p5", &json!({ "path": folder(), "args": ["--staged"] })).unwrap();
        let plain = diffs.subscribe("app1:p5", &json!({ "path": folder() })).unwrap();
        assert_ne!(staged["content"], plain["content"]);
        assert_eq!(diffs.list().as_array().unwrap().len(), 1);
        assert_eq!(diffs.list()[0]["args"], json!([]));
        assert_eq!(diff_id(&folder(), &[]), plain["content"].as_str().unwrap());
    }

    #[test]
    fn an_app_leaving_drops_only_its_views() {
        let (diffs, _inbox) = diffs();
        let args = json!({ "path": folder() });
        diffs.subscribe("app1:p5", &args).unwrap();
        diffs.subscribe("app2:p5", &args).unwrap();
        diffs.leave(|view| view.starts_with("app1:"));
        assert_eq!(diffs.list()[0]["views"], json!(["app2:p5"]));
        diffs.leave(|view| view.starts_with("app2:"));
        assert_eq!(diffs.list(), json!([]));
    }

    #[test]
    fn the_files_shown_are_every_views_together() {
        let (diffs, _inbox) = diffs();
        let args = json!({ "path": folder() });
        diffs.subscribe("app1:p5", &args).unwrap();
        diffs.subscribe("app1:p6", &args).unwrap();
        diffs.show("app1:p5", &json!(["a.rs"]));
        diffs.show("app1:p6", &json!(["b.rs"]));
        let shown = |diffs: &Diffs| diffs.watches.borrow().values().next().unwrap().show.clone();
        assert_eq!(shown(&diffs), HashSet::from(["a.rs".to_string(), "b.rs".to_string()]));
        diffs.unsubscribe("app1:p6");
        assert_eq!(shown(&diffs), HashSet::from(["a.rs".to_string()]));
    }

    #[test]
    fn a_diff_without_a_folder_is_refused() {
        let (diffs, _inbox) = diffs();
        let error = diffs.subscribe("app1:p5", &json!({ "args": [] })).unwrap_err();
        assert_eq!(error.code, "bad_request");
        assert_eq!(diffs.list(), json!([]));
    }
}
