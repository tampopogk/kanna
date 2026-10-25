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

const QUIET: Duration = Duration::from_millis(100);

pub const MAX_BYTES: usize = 8 << 20;

const SNIFF_BYTES: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Md,
    Code,
}

impl Kind {
    fn parse(kind: &str) -> Option<Kind> {
        match kind {
            "md" => Some(Kind::Md),
            "code" => Some(Kind::Code),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Md => "md",
            Kind::Code => "code",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Read {
    Text(String),
    Failed { code: &'static str, message: String },
}

enum Control {
    Changed(Vec<PathBuf>),
    Stop,
}

struct Watch {
    path: String,
    kind: Kind,
    control: Sender<Control>,
    views: HashSet<String>,
    read: Option<Read>,
    serial: u64,
}

impl Watch {
    fn reply(&self, id: &str) -> Map {
        let mut out = Map::new();
        out.insert("content".into(), json!(id));
        out.insert("path".into(), json!(self.path));
        out.insert("serial".into(), json!(self.serial));
        match &self.read {
            Some(Read::Text(text)) => {
                out.insert("text".into(), json!(text));
            }
            Some(Read::Failed { code, message }) => {
                out.insert("error".into(), json!({ "code": code, "message": message }));
            }
            None => {}
        }
        out
    }
}

pub struct Files {
    events: Sender<Event>,
    watches: RefCell<HashMap<String, Watch>>,
    pub on_file: RefCell<Option<Box<dyn Fn(&[String], &Value)>>>,
}

pub fn file_id(path: &str, kind: Kind) -> String {
    let mut hash = Sha256::new();
    hash.update(kind.name().as_bytes());
    hash.update([0]);
    hash.update(path.as_bytes());
    let digest = hash.finalize();
    format!("file-{}", digest.iter().take(4).map(|b| format!("{b:02x}")).collect::<String>())
}

pub fn absolute(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = if path == "~" {
        home.clone()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else {
        path.to_string()
    };
    if expanded.starts_with('/') {
        expanded
    } else {
        format!("{home}/{expanded}")
    }
}

impl Files {
    pub fn new(events: Sender<Event>) -> Files {
        Files { events, watches: RefCell::new(HashMap::new()), on_file: RefCell::new(None) }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let files = Rc::downgrade(self);
        core.add_command(
            "files",
            Rc::new(move |_, responder| {
                let Some(files) = files.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                let mut out = Map::new();
                out.insert("files".into(), files.list());
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
                    let mut views: Vec<&String> = watch.views.iter().collect();
                    views.sort();
                    json!({ "id": id, "path": watch.path, "kind": watch.kind.name(), "views": views, "serial": watch.serial })
                })
                .collect(),
        )
    }

    pub fn subscribe(&self, view: &str, args: &Value) -> Result<Map, KmuxError> {
        let path = string(args, "path").filter(|p| !p.is_empty()).ok_or_else(|| KmuxError::new("bad_request", "a file needs its path"))?;
        let kind = string(args, "kind").and_then(Kind::parse).ok_or_else(|| KmuxError::new("bad_request", "a file's kind is md or code"))?;
        let path = absolute(path);
        let id = file_id(&path, kind);
        if self.watches.borrow().get(&id).is_some_and(|w| w.views.contains(view)) {
            return Ok(self.watches.borrow()[&id].reply(&id));
        }
        self.unsubscribe(view);
        let mut watches = self.watches.borrow_mut();
        let watch = watches.entry(id.clone()).or_insert_with(|| self.start(&id, &path, kind));
        watch.views.insert(view.to_string());
        Ok(watch.reply(&id))
    }

    fn start(&self, id: &str, path: &str, kind: Kind) -> Watch {
        let (control, inbox) = mpsc::channel();
        let events = self.events.clone();
        let changes = control.clone();
        let (thread_id, thread_path) = (id.to_string(), PathBuf::from(path));
        let _ = std::thread::Builder::new().name(format!("kanna.{id}")).spawn(move || work(thread_id, thread_path, kind, inbox, changes, events));
        Watch { path: path.into(), kind, control, views: HashSet::new(), read: None, serial: 0 }
    }

    pub fn unsubscribe(&self, view: &str) {
        self.leave(|v| v == view);
    }

    pub fn leave(&self, gone: impl Fn(&str) -> bool) {
        let mut watches = self.watches.borrow_mut();
        for watch in watches.values_mut() {
            watch.views.retain(|v| !gone(v));
        }
        watches.retain(|_, watch| {
            let keep = !watch.views.is_empty();
            if !keep {
                let _ = watch.control.send(Control::Stop);
            }
            keep
        });
    }

    pub fn loaded(&self, id: &str, read: Read) {
        let (views, payload) = {
            let mut watches = self.watches.borrow_mut();
            let Some(watch) = watches.get_mut(id) else { return };
            let gone = matches!(&read, Read::Failed { code: "missing", .. });
            if watch.read.as_ref() == Some(&read) || (gone && matches!(watch.read, Some(Read::Text(_)))) {
                return;
            }
            watch.serial += 1;
            watch.read = Some(read);
            (watch.views.iter().cloned().collect::<Vec<_>>(), Value::Object(watch.reply(id)))
        };
        if let Some(on_file) = self.on_file.borrow().as_ref() {
            on_file(&views, &payload);
        }
    }
}

pub fn read(path: &Path, kind: Kind) -> Read {
    let shown = path.to_string_lossy();
    let failed = |code, message: String| Read::Failed { code, message };
    let Ok(meta) = std::fs::metadata(path) else { return failed("missing", format!("No such file: {shown}")) };
    if meta.is_dir() {
        let what = if kind == Kind::Md { "a markdown file" } else { "a file" };
        return failed("folder", format!("{shown} is a folder, not {what}"));
    }
    if meta.len() > MAX_BYTES as u64 {
        return failed("too_big", format!("{shown} is too big to show, over {} MiB", MAX_BYTES >> 20));
    }
    let Ok(bytes) = std::fs::read(path) else { return failed("missing", format!("No such file: {shown}")) };
    if bytes.len() > MAX_BYTES {
        return failed("too_big", format!("{shown} is too big to show, over {} MiB", MAX_BYTES >> 20));
    }
    if kind == Kind::Code && bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0) {
        return failed("binary", format!("{shown} isn't a text file"));
    }
    Read::Text(String::from_utf8_lossy(&bytes).into_owned())
}

struct Place {
    dirs: Vec<PathBuf>,
    names: Vec<PathBuf>,
}

impl Place {
    fn of(path: &Path) -> Place {
        let mut dirs = vec![];
        let mut names = vec![path.to_path_buf()];
        if let Some(anchor) = path.ancestors().skip(1).find(|dir| dir.is_dir()) {
            let real = std::fs::canonicalize(anchor).unwrap_or_else(|_| anchor.to_path_buf());
            if let Ok(rest) = path.strip_prefix(anchor) {
                names.push(real.join(rest));
            }
            dirs.push(real);
        }
        if let Ok(real) = std::fs::canonicalize(path) {
            if let Some(dir) = real.parent() {
                dirs.push(dir.to_path_buf());
            }
            names.push(real);
        }
        dirs.sort();
        dirs.dedup();
        Place { dirs, names }
    }

    fn touched(&self, changed: &[PathBuf]) -> bool {
        changed.iter().any(|p| self.names.iter().any(|name| name.starts_with(p)))
    }
}

fn work(id: String, path: PathBuf, kind: Kind, inbox: Receiver<Control>, changes: Sender<Control>, events: Sender<Event>) {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let _ = changes.send(Control::Changed(event.paths));
        }
    })
    .ok();
    let mut place = Place::of(&path);
    if let Some(watcher) = watcher.as_mut() {
        for dir in &place.dirs {
            let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
        }
    }
    let mut last = read(&path, kind);
    let _ = events.send(Event::File { id: id.clone(), read: last.clone() });
    loop {
        let mut changed: Vec<PathBuf> = vec![];
        match inbox.recv() {
            Ok(Control::Stop) | Err(_) => return,
            Ok(Control::Changed(paths)) => changed.extend(paths),
        }
        loop {
            match inbox.recv_timeout(QUIET) {
                Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Control::Changed(paths)) => changed.extend(paths),
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
        if !place.touched(&changed) {
            continue;
        }
        let now = Place::of(&path);
        if let Some(watcher) = watcher.as_mut() {
            for dir in place.dirs.iter().filter(|d| !now.dirs.contains(d)) {
                let _ = watcher.unwatch(dir);
            }
            for dir in now.dirs.iter().filter(|d| !place.dirs.contains(d)) {
                let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
            }
        }
        place = now;
        let read = read(&path, kind);
        if read != last {
            last = read.clone();
            let _ = events.send(Event::File { id: id.clone(), read });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> (Files, Receiver<Event>) {
        let (events, inbox) = mpsc::channel();
        (Files::new(events), inbox)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn views_of_one_file_share_its_watcher_until_the_last_leaves() {
        let (files, _inbox) = files();
        let dir = scratch("share");
        std::fs::write(dir.join("a.md"), "# A\n").unwrap();
        let args = json!({ "path": dir.join("a.md"), "kind": "md" });
        let first = files.subscribe("app1:v1", &args).unwrap();
        let second = files.subscribe("app1:v2", &args).unwrap();
        assert_eq!(first["content"], second["content"]);
        assert_eq!(first["serial"], 0);
        assert_eq!(files.list()[0]["views"], json!(["app1:v1", "app1:v2"]));
        files.unsubscribe("app1:v1");
        assert_eq!(files.list()[0]["views"], json!(["app1:v2"]));
        files.unsubscribe("app1:v2");
        assert_eq!(files.list(), json!([]));
    }

    #[test]
    fn the_same_path_as_md_and_as_code_is_two_files() {
        let (files, _inbox) = files();
        let dir = scratch("kinds");
        std::fs::write(dir.join("a.md"), "x").unwrap();
        let md = files.subscribe("app1:v1", &json!({ "path": dir.join("a.md"), "kind": "md" })).unwrap();
        let code = files.subscribe("app1:v2", &json!({ "path": dir.join("a.md"), "kind": "code" })).unwrap();
        assert_ne!(md["content"], code["content"]);
        let moved = files.subscribe("app1:v1", &json!({ "path": dir.join("a.md"), "kind": "code" })).unwrap();
        assert_eq!(moved["content"], code["content"]);
        assert_eq!(files.list().as_array().unwrap().len(), 1);
    }

    #[test]
    fn what_cant_be_shown_says_why() {
        let dir = scratch("errors");
        std::fs::write(dir.join("bin"), b"a\0b").unwrap();
        let code = |read: Read| match read {
            Read::Failed { code, .. } => code,
            Read::Text(_) => "text",
        };
        assert_eq!(code(read(&dir.join("none"), Kind::Code)), "missing");
        assert_eq!(code(read(&dir, Kind::Code)), "folder");
        assert_eq!(read(&dir, Kind::Md), Read::Failed { code: "folder", message: format!("{} is a folder, not a markdown file", dir.display()) });
        assert_eq!(code(read(&dir.join("bin"), Kind::Code)), "binary");
        assert_eq!(code(read(&dir.join("bin"), Kind::Md)), "text");
        std::fs::write(dir.join("big"), vec![b'a'; MAX_BYTES + 1]).unwrap();
        assert_eq!(code(read(&dir.join("big"), Kind::Code)), "too_big");
    }

    #[test]
    fn a_deleted_file_keeps_its_text_and_a_new_one_is_sent() {
        let (files, _inbox) = files();
        let dir = scratch("deleted");
        std::fs::write(dir.join("a.rs"), "one").unwrap();
        let reply = files.subscribe("app1:v1", &json!({ "path": dir.join("a.rs"), "kind": "code" })).unwrap();
        let id = reply["content"].as_str().unwrap().to_string();
        files.loaded(&id, Read::Text("one".into()));
        let sent = Rc::new(RefCell::new(vec![]));
        let heard = sent.clone();
        *files.on_file.borrow_mut() = Some(Box::new(move |_, payload| heard.borrow_mut().push(payload.clone())));
        files.loaded(&id, Read::Failed { code: "missing", message: "gone".into() });
        assert!(sent.borrow().is_empty());
        files.loaded(&id, Read::Text("two".into()));
        files.loaded(&id, Read::Text("two".into()));
        assert_eq!(sent.borrow().len(), 1);
        assert_eq!(sent.borrow()[0]["text"], "two");
        assert_eq!(sent.borrow()[0]["serial"], 2);
    }

    #[test]
    fn the_watcher_follows_a_file_through_a_symlinked_folder_deleted_and_made_again() {
        let (files, inbox) = files();
        let dir = PathBuf::from(format!("/tmp/kanna-files-{}-watch", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.rs");
        std::fs::write(&file, "one").unwrap();
        files.subscribe("app1:v1", &json!({ "path": file, "kind": "code" })).unwrap();
        let next = || loop {
            match inbox.recv_timeout(Duration::from_secs(5)) {
                Ok(Event::File { read, .. }) => return read,
                Ok(_) => continue,
                Err(_) => panic!("no file event"),
            }
        };
        assert_eq!(next(), Read::Text("one".into()));
        std::fs::write(&file, "two").unwrap();
        assert_eq!(next(), Read::Text("two".into()));
        std::fs::remove_file(&file).unwrap();
        assert!(matches!(next(), Read::Failed { code: "missing", .. }));
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&file, "three").unwrap();
        assert_eq!(next(), Read::Text("three".into()));
        files.unsubscribe("app1:v1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn paths_are_made_absolute_against_home() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(absolute("~/a.md"), format!("{home}/a.md"));
        assert_eq!(absolute("a.md"), format!("{home}/a.md"));
        assert_eq!(absolute("/x/a.md"), "/x/a.md");
    }

    #[test]
    fn a_file_without_a_path_or_kind_is_refused() {
        let (files, _inbox) = files();
        assert_eq!(files.subscribe("app1:v1", &json!({ "kind": "md" })).unwrap_err().code, "bad_request");
        assert_eq!(files.subscribe("app1:v1", &json!({ "path": "/a", "kind": "web" })).unwrap_err().code, "bad_request");
        assert_eq!(files.list(), json!([]));
    }
}
