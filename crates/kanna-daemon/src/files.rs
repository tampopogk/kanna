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
use std::time::{Duration, Instant};

const QUIET: Duration = Duration::from_millis(100);

const GATHER: Duration = Duration::from_secs(1);

const BASE_CHECK: Duration = Duration::from_secs(2);

pub const MAX_BYTES: usize = 8 << 20;

const SNIFF_BYTES: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Md,
    Code,
    Threads,
}

impl Kind {
    fn parse(kind: &str) -> Option<Kind> {
        match kind {
            "md" => Some(Kind::Md),
            "code" => Some(Kind::Code),
            "threads" => Some(Kind::Threads),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Md => "md",
            Kind::Code => "code",
            Kind::Threads => "threads",
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Read {
    Shown(Map),
    Failed { code: &'static str, message: String },
}

enum Control {
    Changed(Vec<PathBuf>),
    Stop,
}

impl Read {
    pub fn text(text: impl Into<String>) -> Read {
        let mut fields = Map::new();
        fields.insert("text".into(), json!(text.into()));
        Read::Shown(fields)
    }
}

struct Watch {
    path: String,
    kind: Kind,
    diff: Option<String>,
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
            Some(Read::Shown(fields)) => {
                out.extend(fields.clone());
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
    events: crate::runtime::Events,
    watches: RefCell<HashMap<String, Watch>>,
    pub on_file: RefCell<Option<Box<dyn Fn(&[String], &Value)>>>,
}

pub fn file_id(path: &str, kind: Kind, diff: Option<&str>) -> String {
    let mut hash = Sha256::new();
    hash.update(kind.name().as_bytes());
    hash.update([0]);
    hash.update(path.as_bytes());
    if let Some(diff) = diff {
        hash.update([0]);
        hash.update(diff.as_bytes());
    }
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
    pub fn new(events: crate::runtime::Events) -> Files {
        Files { events, watches: RefCell::new(HashMap::new()), on_file: RefCell::new(None) }
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle, name: &'static str) {
        let files = Rc::downgrade(self);
        core.add_command(
            name,
            Rc::new(move |_, responder| {
                let Some(files) = files.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                let mut out = Map::new();
                out.insert(name.into(), files.list());
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
                    let mut out = json!({ "id": id, "path": watch.path, "kind": watch.kind.name(), "views": views, "serial": watch.serial });
                    if let Some(diff) = &watch.diff {
                        out["diff"] = json!(diff);
                    }
                    out
                })
                .collect(),
        )
    }

    pub fn subscribe(&self, view: &str, args: &Value) -> Result<Map, KmuxError> {
        let path = string(args, "path").filter(|p| !p.is_empty()).ok_or_else(|| KmuxError::new("bad_request", "a file needs its path"))?;
        let kind = string(args, "kind").and_then(Kind::parse).ok_or_else(|| KmuxError::new("bad_request", "a file's kind is md or code"))?;
        let diff = string(args, "diff").filter(|d| !d.is_empty() && kind == Kind::Md).map(String::from);
        let path = absolute(path);
        let id = file_id(&path, kind, diff.as_deref());
        if self.watches.borrow().get(&id).is_some_and(|w| w.views.contains(view)) {
            return Ok(self.watches.borrow()[&id].reply(&id));
        }
        self.unsubscribe(view);
        let mut watches = self.watches.borrow_mut();
        let watch = watches.entry(id.clone()).or_insert_with(|| self.start(&id, &path, kind, diff));
        watch.views.insert(view.to_string());
        Ok(watch.reply(&id))
    }

    fn start(&self, id: &str, path: &str, kind: Kind, diff: Option<String>) -> Watch {
        let (control, inbox) = mpsc::channel();
        let events = self.events.clone();
        let changes = control.clone();
        let (thread_id, thread_path, thread_diff) = (id.to_string(), PathBuf::from(path), diff.clone());
        let _ = std::thread::Builder::new().name(format!("kanna.{id}")).spawn(move || work(thread_id, thread_path, kind, thread_diff, inbox, changes, events));
        Watch { path: path.into(), kind, diff, control, views: HashSet::new(), read: None, serial: 0 }
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
            if watch.read.as_ref() == Some(&read) || (gone && matches!(watch.read, Some(Read::Shown(_)))) {
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
    Read::text(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn load(path: &Path, kind: Kind, diff: Option<&str>) -> Read {
    if kind == Kind::Threads {
        let mut fields = Map::new();
        fields.insert("record".into(), kanna_threads::read(path));
        fields.insert("name".into(), json!(kanna_threads::relative(path)));
        return Read::Shown(fields);
    }
    match read(path, kind) {
        Read::Shown(mut fields) => {
            if kind == Kind::Md {
                fields.insert("name".into(), json!(kanna_threads::relative(path)));
            }
            if let Some(diff) = diff {
                compare(path, diff, &mut fields);
            }
            Read::Shown(fields)
        }
        read => read,
    }
}

pub fn range(diff: &str) -> Option<(&str, &str)> {
    if diff.contains("...") {
        return None;
    }
    let (from, to) = diff.split_once("..")?;
    Some((if from.is_empty() { "HEAD" } else { from }, if to.is_empty() { "HEAD" } else { to }))
}

fn git(folder: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(folder).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn place_in_git(path: &Path) -> (PathBuf, String) {
    let file = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    (folder, name)
}

pub fn merge_base(path: &Path, diff: &str) -> Option<String> {
    let (folder, _) = place_in_git(path);
    git(&folder, &["merge-base", diff, "HEAD"]).map(|c| c.trim().to_string())
}

fn compare(path: &Path, diff: &str, fields: &mut Map) {
    let (folder, name) = place_in_git(path);
    let error = |fields: &mut Map, message: String| {
        fields.insert("diff_error".into(), json!(message));
    };
    if git(&folder, &["rev-parse", "--show-toplevel"]).is_none() {
        return error(fields, format!("{name} isn't in a git repo, so it has nothing to be compared with"));
    }
    let known = |r: &str| git(&folder, &["rev-parse", "--verify", "--quiet", &format!("{r}^{{commit}}")]).is_some();
    let show = |r: &str| git(&folder, &["show", &format!("{r}:./{name}")]);
    if let Some((from, to)) = range(diff) {
        if let Some(unknown) = [from, to].into_iter().find(|r| !known(r)) {
            return error(fields, format!("no commit or branch \"{unknown}\" in its repo"));
        }
        let Some(new) = show(to) else { return error(fields, format!("{name} isn't in {to}")) };
        fields.insert("text".into(), json!(new));
        fields.insert("old".into(), json!(show(from)));
        return;
    }
    if !known(diff) {
        return error(fields, format!("no commit or branch \"{diff}\" in its repo"));
    }
    let Some(commit) = git(&folder, &["merge-base", diff, "HEAD"]).map(|c| c.trim().to_string()) else {
        return error(fields, format!("{diff} and HEAD share no history"));
    };
    fields.insert("base".into(), json!({ "commit": commit, "text": show(&commit) }));
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

fn work(id: String, doc: PathBuf, kind: Kind, diff: Option<String>, inbox: Receiver<Control>, changes: Sender<Control>, events: crate::runtime::Events) {
    let path = if kind == Kind::Threads { kanna_threads::store_path(&doc) } else { doc.clone() };
    let checks_base = diff.as_deref().is_some_and(|d| range(d).is_none());
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
    let mut last = load(&doc, kind, diff.as_deref());
    let mut base = checks_base.then(|| merge_base(&doc, diff.as_deref().unwrap_or_default())).flatten();
    let _ = events.send(Event::File { id: id.clone(), read: last.clone() });
    let mut check_at = Instant::now() + BASE_CHECK;
    loop {
        let mut changed: Vec<PathBuf> = vec![];
        let next = if checks_base { inbox.recv_timeout(check_at.saturating_duration_since(Instant::now())) } else { inbox.recv().map_err(|_| RecvTimeoutError::Disconnected) };
        match next {
            Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(Control::Changed(paths)) => changed.extend(paths),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if checks_base && Instant::now() >= check_at {
            check_at = Instant::now() + BASE_CHECK;
            let now = merge_base(&doc, diff.as_deref().unwrap_or_default());
            if now != base {
                base = now;
                let read = load(&doc, kind, diff.as_deref());
                if read != last {
                    last = read.clone();
                    let _ = events.send(Event::File { id: id.clone(), read });
                }
            }
        }
        if changed.is_empty() {
            continue;
        }
        let gathered = Instant::now() + GATHER;
        while Instant::now() < gathered {
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
        let read = load(&doc, kind, diff.as_deref());
        if read != last {
            last = read.clone();
            let _ = events.send(Event::File { id: id.clone(), read });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> (Files, crate::runtime::Inbox) {
        let (events, inbox) = crate::runtime::channel();
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
            Read::Shown(_) => "text",
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
        files.loaded(&id, Read::text("one"));
        let sent = Rc::new(RefCell::new(vec![]));
        let heard = sent.clone();
        *files.on_file.borrow_mut() = Some(Box::new(move |_, payload| heard.borrow_mut().push(payload.clone())));
        files.loaded(&id, Read::Failed { code: "missing", message: "gone".into() });
        assert!(sent.borrow().is_empty());
        files.loaded(&id, Read::text("two"));
        files.loaded(&id, Read::text("two"));
        assert_eq!(sent.borrow().len(), 1);
        assert_eq!(sent.borrow()[0]["text"], "two");
        assert_eq!(sent.borrow()[0]["serial"], 2);
    }

    #[test]
    fn the_watcher_follows_a_file_through_a_symlinked_folder_deleted_and_made_again() {
        let (files, mut inbox) = files();
        let dir = PathBuf::from(format!("/tmp/kanna-files-{}-watch", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.rs");
        std::fs::write(&file, "one").unwrap();
        files.subscribe("app1:v1", &json!({ "path": file, "kind": "code" })).unwrap();
        let mut next = || loop {
            match crate::runtime::next(&mut inbox, Duration::from_secs(5)) {
                Ok(Event::File { read, .. }) => return read,
                Ok(_) => continue,
                Err(_) => panic!("no file event"),
            }
        };
        assert_eq!(next(), Read::text("one"));
        std::fs::write(&file, "two").unwrap();
        assert_eq!(next(), Read::text("two"));
        std::fs::remove_file(&file).unwrap();
        assert!(matches!(next(), Read::Failed { code: "missing", .. }));
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&file, "three").unwrap();
        assert_eq!(next(), Read::text("three"));
        files.unsubscribe("app1:v1");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_docs_threads_are_watched_beside_it_and_follow_every_writer() {
        let (threads, mut inbox) = files();
        let dir = std::fs::canonicalize(scratch("threads")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let doc = dir.join("docs/spec.md");
        std::fs::create_dir_all(doc.parent().unwrap()).unwrap();
        std::fs::write(&doc, "# Spec\n").unwrap();
        let reply = threads.subscribe("app1:v1", &json!({ "path": doc, "kind": "threads" })).unwrap();
        assert_eq!(reply["content"], json!(file_id(&doc.to_string_lossy(), Kind::Threads, None)));
        let mut next = || loop {
            match crate::runtime::next(&mut inbox, Duration::from_secs(5)) {
                Ok(Event::File { read: Read::Shown(fields), .. }) => return fields,
                Ok(_) => continue,
                Err(_) => panic!("no threads event"),
            }
        };
        let first = next();
        assert_eq!((first["name"].clone(), first["record"]["threads"].clone()), (json!("docs/spec.md"), json!([])));
        kanna_threads::add_agent_thread(&doc, 1, "The title.", Some("term-3")).unwrap();
        let written = next();
        assert_eq!((written["record"]["agent"].clone(), written["record"]["threads"][0]["id"].clone()), (json!("term-3"), json!("c1")));
        threads.unsubscribe("app1:v1");
    }

    #[test]
    fn a_doc_compared_with_a_ref_comes_with_its_base() {
        let dir = std::fs::canonicalize(scratch("base")).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&dir).args(["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false"]).args(args).output().unwrap().status.success(), "{args:?}");
        git(&["init", "-q", "-b", "main"]);
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "old\n").unwrap();
        git(&["add", "doc.md"]);
        git(&["commit", "-qm", "a"]);
        git(&["checkout", "-qb", "work"]);
        std::fs::write(&doc, "new\n").unwrap();
        git(&["commit", "-qam", "b"]);
        std::fs::write(&doc, "newer\n").unwrap();
        let Read::Shown(fields) = load(&doc, Kind::Md, Some("main")) else { panic!() };
        assert_eq!((fields["text"].clone(), fields["base"]["text"].clone()), (json!("newer\n"), json!("old\n")));
        assert_eq!(fields["base"]["commit"], json!(merge_base(&doc, "main").unwrap()));
        let Read::Shown(ranged) = load(&doc, Kind::Md, Some("main..work")) else { panic!() };
        assert_eq!((ranged["text"].clone(), ranged["old"].clone()), (json!("new\n"), json!("old\n")));
        let Read::Shown(unknown) = load(&doc, Kind::Md, Some("nope")) else { panic!() };
        assert_eq!((unknown["text"].clone(), unknown["diff_error"].clone()), (json!("newer\n"), json!("no commit or branch \"nope\" in its repo")));
        assert_eq!((range("a..b"), range("..b"), range("a...b")), (Some(("a", "b")), Some(("HEAD", "b")), None));
    }

    #[test]
    fn a_moved_base_is_seen_while_the_folder_keeps_changing() {
        let (files, mut inbox) = files();
        let dir = std::fs::canonicalize(scratch("busy")).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(&dir).args(["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false"]).args(args).output().unwrap().status.success(), "{args:?}");
        git(&["init", "-q", "-b", "main"]);
        let doc = dir.join("doc.md");
        std::fs::write(&doc, "old\n").unwrap();
        git(&["add", "doc.md"]);
        git(&["commit", "-qm", "a"]);
        git(&["branch", "base"]);
        std::fs::write(&doc, "new\n").unwrap();
        git(&["commit", "-qam", "b"]);
        files.subscribe("app1:v1", &json!({ "path": doc, "kind": "md", "diff": "base" })).unwrap();
        let mut base = || loop {
            match crate::runtime::next(&mut inbox, Duration::from_secs(8)) {
                Ok(Event::File { read: Read::Shown(fields), .. }) => return fields["base"]["text"].clone(),
                Ok(_) => continue,
                Err(_) => panic!("no base"),
            }
        };
        assert_eq!(base(), json!("old\n"));
        let busy = dir.join("busy.txt");
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writing = stop.clone();
        let writer = std::thread::spawn(move || {
            let mut n = 0;
            while !writing.load(std::sync::atomic::Ordering::Relaxed) {
                n += 1;
                let _ = std::fs::write(&busy, format!("{n}"));
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        git(&["branch", "-f", "base", "HEAD"]);
        let moved = base();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        writer.join().unwrap();
        assert_eq!(moved, json!("new\n"));
        files.unsubscribe("app1:v1");
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
