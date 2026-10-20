use kanna_mux::json::{json, Map, Value};
use kanna_mux::{CoreHandle, KmuxError};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const RECORD_VERSION: u64 = 2;

const MARK: &str = "# kanna agents setup";

pub struct Agents {
    pub home: PathBuf,
    pub skills: Option<PathBuf>,
    pub socket: String,
    pub asked_setup: Cell<bool>,
    pub asked_update: Cell<bool>,
}

#[derive(Default)]
struct Record {
    setup: Option<bool>,
    files: BTreeMap<String, String>,
    answered: Option<String>,
    sockets: Vec<String>,
    made: Vec<String>,
}

#[derive(Default, Debug)]
pub struct Report {
    pub changes: BTreeMap<String, &'static str>,
    pub conflicts: Vec<(String, String, String)>,
    pub kept: Vec<String>,
    pub settings: bool,
}

#[derive(Clone)]
enum Want {
    File(Vec<u8>),
    Link(String),
}

enum Step {
    Same,
    New,
    Update,
    Merge(Vec<u8>),
    Conflict,
    Yours,
    Theirs,
    Drop,
}

struct Planned {
    path: String,
    skill: String,
    want: Option<Want>,
    step: Step,
}

pub fn bundled_skills(daemon: &Path) -> Option<PathBuf> {
    let skills = daemon.parent()?.parent()?.join("Resources/kanna-skills");
    skills.is_dir().then_some(skills)
}

pub fn skills() -> Option<PathBuf> {
    match std::env::var_os("KANNA_AGENT_SKILLS").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)).filter(|d| d.is_dir()),
        None => std::env::current_exe().ok().as_deref().and_then(bundled_skills),
    }
}

pub fn home() -> PathBuf {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    PathBuf::from(var("KANNA_AGENTS_HOME").or_else(|| var("HOME")).unwrap_or_default())
}

impl Agents {
    pub fn new(home: PathBuf, skills: Option<PathBuf>, socket: String) -> Agents {
        Agents { home, skills, socket, asked_setup: Cell::new(false), asked_update: Cell::new(false) }
    }

    pub fn install(self, core: &CoreHandle) {
        let agents = std::rc::Rc::new(self);
        core.add_command(
            "agents",
            std::rc::Rc::new(move |args, responder| {
                let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
                responder.reply(match args.get("action").and_then(Value::as_str) {
                    Some("status") => agents.status(),
                    Some("ask") => agents.ask(args.get("kind").and_then(Value::as_str).unwrap_or("")),
                    Some("setup") => agents.setup(force).map(|report| reply("setup", &report)),
                    Some("update") => agents.update(force).map(|report| reply("update", &report)),
                    Some("decline") => agents.decline().map(|_| reply("decline", &Report::default())),
                    Some("skip") => agents.skip().map(|_| reply("skip", &Report::default())),
                    Some("resolved") => agents.resolved(args.get("path").and_then(Value::as_str).unwrap_or("")).map(|_| reply("resolved", &Report::default())),
                    Some("remove") => agents.remove().map(|report| reply("remove", &report)),
                    _ => Err(KmuxError::new("bad_request", "agents needs an action: status, ask, setup, update, decline, skip, resolved or remove")),
                })
            }),
        );
    }

    pub fn refresh(&self) {
        let mut record = self.record();
        if record.setup != Some(true) {
            return;
        }
        match self.settings(&mut record).and_then(|_| self.save(&record)) {
            Ok(()) => {}
            Err(error) => log!("kanna-daemon: can't keep agents' sandbox entry: {}", error.message),
        }
    }

    fn status(&self) -> Result<Map, KmuxError> {
        let record = self.record();
        let mut out = Map::new();
        out.insert("setup".into(), json!(match record.setup {
            Some(true) => "yes",
            Some(false) => "no",
            None => "ask",
        }));
        out.insert("skills".into(), json!(self.shipped().map(|s| s.keys().cloned().collect::<Vec<_>>()).unwrap_or_default()));
        out.insert("home".into(), json!(self.home.to_string_lossy()));
        out.insert("socket".into(), json!(self.socket));
        out.insert("sandbox".into(), json!(record.sockets.contains(&self.socket)));
        if record.setup == Some(true) {
            if let Some(shipped) = self.shipped() {
                out.insert("changes".into(), json!(summary(&self.plan(&record, &shipped, false))));
                out.insert("skipped".into(), json!(record.answered.as_deref() == Some(&fingerprint(&shipped))));
            }
        }
        Ok(out)
    }

    fn ask(&self, kind: &str) -> Result<Map, KmuxError> {
        let mut out = self.status()?;
        let record = self.record();
        let ask = match kind {
            "setup" => record.setup.is_none() && !self.asked_setup.replace(true) && self.skills.is_some(),
            "update" => {
                let changed = out.get("changes").and_then(Value::as_object).is_some_and(|c| !c.is_empty());
                let skipped = out.get("skipped").and_then(Value::as_bool).unwrap_or(false);
                record.setup == Some(true) && changed && !skipped && !self.asked_update.replace(true)
            }
            _ => return Err(KmuxError::new("bad_request", "ask needs a kind: setup or update")),
        };
        out.insert("ask".into(), json!(ask));
        Ok(out)
    }

    fn setup(&self, force: bool) -> Result<Report, KmuxError> {
        let mut record = self.record();
        record.setup = Some(true);
        self.sync(record, force)
    }

    fn update(&self, force: bool) -> Result<Report, KmuxError> {
        let record = self.record();
        if record.setup != Some(true) {
            return Err(KmuxError::new("not_set_up", "your agents aren't set up for Kanna: kanna agents setup"));
        }
        self.sync(record, force)
    }

    fn decline(&self) -> Result<(), KmuxError> {
        let mut record = self.record();
        if record.setup.is_none() {
            record.setup = Some(false);
            self.save(&record)?;
        }
        Ok(())
    }

    fn skip(&self) -> Result<(), KmuxError> {
        let mut record = self.record();
        record.answered = self.shipped().map(|s| fingerprint(&s));
        self.save(&record)
    }

    fn resolved(&self, path: &str) -> Result<(), KmuxError> {
        let mut record = self.record();
        let shipped = self.shipped().ok_or_else(no_skills)?;
        let content = self.wanted(&shipped).remove(path).and_then(|(_, want)| match want {
            Want::File(content) => Some(content),
            Want::Link(_) => None,
        });
        let content = content.ok_or_else(|| KmuxError::new("not_found", format!("{path} isn't one of Kanna's skill files")))?;
        self.keep_base(&mut record, path, &content)?;
        self.save(&record)
    }

    fn remove(&self) -> Result<Report, KmuxError> {
        let mut record = self.record();
        let mut report = Report::default();
        for (path, hash) in std::mem::take(&mut record.files) {
            if owned(Path::new(&path), &hash) {
                remove_file(Path::new(&path));
                report.changes.insert(path.clone(), "removed");
            } else if exists(Path::new(&path)) {
                report.kept.push(path.clone());
            }
            self.drop_base(&path);
        }
        report.settings = self.unsettings(&record).unwrap_or(false);
        record.sockets.clear();
        record.made.clear();
        record.answered = None;
        record.setup = Some(false);
        self.save(&record)?;
        Ok(report)
    }

    fn wanted(&self, shipped: &Shipped) -> BTreeMap<String, (String, Want)> {
        let mut wanted = BTreeMap::new();
        for (name, files) in shipped {
            for (relative, content) in files {
                wanted.insert(self.home.join(".agents/skills").join(name).join(relative).to_string_lossy().into(), (name.clone(), Want::File(content.clone())));
            }
            wanted.insert(self.home.join(".claude/skills").join(name).to_string_lossy().into(), (name.clone(), Want::Link(format!("../../.agents/skills/{name}"))));
        }
        wanted
    }

    fn plan(&self, record: &Record, shipped: &Shipped, force: bool) -> Vec<Planned> {
        let wanted = self.wanted(shipped);
        let mut plan = Vec::new();
        for (path, hash) in &record.files {
            if !wanted.contains_key(path) {
                let step = if owned(Path::new(path), hash) { Step::Drop } else { Step::Theirs };
                plan.push(Planned { path: path.clone(), skill: self.skill_of(path), want: None, step });
            }
        }
        for (path, (skill, want)) in wanted {
            let target = Path::new(&path);
            let now = current(target);
            let base = record.files.get(&path);
            let step = match (now.as_deref(), base) {
                (Some(now), _) if now == want.hash() => Step::Same,
                (None, None) => Step::New,
                (None, Some(_)) => Step::Update,
                (Some(now), Some(base)) if now == base => Step::Update,
                (Some(_), Some(base)) if base == &want.hash() && !force => Step::Yours,
                (Some(_), _) if force && !(target.is_dir() && !target.is_symlink()) => Step::Update,
                (Some(_), None) => Step::Theirs,
                (Some(_), Some(_)) => match (&want, fs::read(target), fs::read(self.base_file(&path))) {
                    (Want::File(new), Ok(mine), Ok(base)) => match merge(&mine, &base, new) {
                        Some(merged) => Step::Merge(merged),
                        None => Step::Conflict,
                    },
                    _ => Step::Theirs,
                },
            };
            plan.push(Planned { path, skill, want: Some(want), step });
        }
        plan
    }

    fn sync(&self, mut record: Record, force: bool) -> Result<Report, KmuxError> {
        let shipped = self.shipped().ok_or_else(no_skills)?;
        let plan = self.plan(&record, &shipped, force);
        let mut report = Report { changes: summary(&plan), ..Report::default() };
        for planned in &plan {
            let path = &planned.path;
            let failed = |e: io::Error| KmuxError::new("failed", format!("can't write {path}: {e}"));
            match (&planned.step, &planned.want) {
                (Step::Same, Some(want)) => self.keep(&mut record, path, want)?,
                (Step::New | Step::Update, Some(want)) => {
                    want.write(Path::new(path)).map_err(failed)?;
                    self.keep(&mut record, path, want)?;
                }
                (Step::Merge(merged), Some(want)) => {
                    write_atomic(Path::new(path), merged).map_err(failed)?;
                    self.keep(&mut record, path, want)?;
                }
                (Step::Conflict, _) => {
                    let relative = Path::new(path).strip_prefix(self.home.join(".agents/skills")).map(Path::to_path_buf).unwrap_or_default();
                    let new = self.skills.as_ref().map(|s| s.join(&relative)).unwrap_or_default();
                    report.conflicts.push((path.clone(), self.base_file(path).to_string_lossy().into(), new.to_string_lossy().into()));
                }
                (Step::Theirs, _) => {
                    record.files.remove(path);
                    self.drop_base(path);
                    report.kept.push(path.clone());
                }
                (Step::Drop, _) => {
                    remove_file(Path::new(path));
                    record.files.remove(path);
                    self.drop_base(path);
                }
                (Step::Yours, _) | (_, None) => {}
            }
        }
        record.answered = Some(fingerprint(&shipped));
        report.settings = self.settings(&mut record)?;
        self.save(&record)?;
        Ok(report)
    }

    fn keep(&self, record: &mut Record, path: &str, want: &Want) -> Result<(), KmuxError> {
        match want {
            Want::File(content) => self.keep_base(record, path, content),
            Want::Link(_) => {
                record.files.insert(path.into(), want.hash());
                Ok(())
            }
        }
    }

    fn keep_base(&self, record: &mut Record, path: &str, content: &[u8]) -> Result<(), KmuxError> {
        let base = self.base_file(path);
        if fs::read(&base).ok().as_deref() != Some(content) {
            write_atomic(&base, content).map_err(|e| KmuxError::new("failed", format!("can't write {}: {e}", base.display())))?;
        }
        record.files.insert(path.into(), digest(content));
        Ok(())
    }

    fn drop_base(&self, path: &str) {
        let base = self.base_file(path);
        if base.is_file() {
            let _ = fs::remove_file(&base);
            let mut dir = base.parent();
            while let Some(d) = dir {
                if fs::remove_dir(d).is_err() || d == self.base_root() {
                    break;
                }
                dir = d.parent();
            }
        }
    }

    fn base_root(&self) -> PathBuf {
        self.home.join(".kanna-v4/agents")
    }

    fn base_file(&self, path: &str) -> PathBuf {
        self.base_root().join(Path::new(path).strip_prefix(&self.home).unwrap_or(Path::new(path)))
    }

    fn skill_of(&self, path: &str) -> String {
        let path = Path::new(path);
        [".agents/skills", ".claude/skills"]
            .iter()
            .find_map(|dir| path.strip_prefix(self.home.join(dir)).ok())
            .and_then(|rest| rest.components().next())
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn settings(&self, record: &mut Record) -> Result<bool, KmuxError> {
        let claude = self.home.join(".claude/settings.json");
        let socket = self.socket.clone();
        let mut sockets = record.sockets.clone();
        let changed = edit(&claude, true, &mut record.made, |object, made| {
            let sandbox = ensure(object, "sandbox", made, "sandbox");
            let network = sandbox.and_then(|o| ensure(o, "network", made, "sandbox.network"));
            if let Some(list) = network.and_then(|o| list(o, "allowUnixSockets", made, "sandbox.network.allowUnixSockets")) {
                if !sockets.contains(&socket) {
                    sockets.push(socket.clone());
                }
                if !list.iter().any(|s| s.as_str() == Some(&socket)) {
                    list.push(json!(socket));
                }
            }
            drop_hook(object, made);
        })?;
        record.sockets = sockets;
        edit(&self.home.join(".codex/hooks.json"), false, &mut record.made, drop_hook)?;
        Ok(changed)
    }

    fn unsettings(&self, record: &Record) -> io::Result<bool> {
        let sockets = record.sockets.clone();
        let mut made = record.made.clone();
        let claude = edit(&self.home.join(".claude/settings.json"), false, &mut made, |object, made| {
            if let Some(network) = object.get_mut("sandbox").and_then(Value::as_object_mut).and_then(|s| s.get_mut("network")).and_then(Value::as_object_mut) {
                if let Some(list) = network.get_mut("allowUnixSockets").and_then(Value::as_array_mut) {
                    list.retain(|s| !s.as_str().is_some_and(|s| sockets.iter().any(|ours| ours == s)));
                }
                prune(network, "allowUnixSockets", made, "sandbox.network.allowUnixSockets");
            }
            if let Some(sandbox) = object.get_mut("sandbox").and_then(Value::as_object_mut) {
                prune(sandbox, "network", made, "sandbox.network");
            }
            prune(object, "sandbox", made, "sandbox");
            drop_hook(object, made);
        });
        let codex = edit(&self.home.join(".codex/hooks.json"), false, &mut made, drop_hook);
        Ok(claude.unwrap_or(false) | codex.unwrap_or(false))
    }

    fn shipped(&self) -> Option<Shipped> {
        let dir = self.skills.as_ref()?;
        let mut skills = BTreeMap::new();
        for entry in fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if !path.join("SKILL.md").is_file() {
                continue;
            }
            let mut files = BTreeMap::new();
            collect(&path, Path::new(""), &mut files);
            skills.insert(entry.file_name().to_string_lossy().into_owned(), files);
        }
        Some(skills)
    }

    fn record_file(&self) -> PathBuf {
        self.home.join(".kanna-v4/agents.json")
    }

    fn record(&self) -> Record {
        let value = fs::read_to_string(self.record_file()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
        let strings = |key: &str| value.get(key).and_then(Value::as_array).map(|s| s.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        Record {
            setup: value.get("setup").and_then(Value::as_bool),
            files: value.get("files").and_then(Value::as_object).map(|f| f.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default(),
            answered: value.get("answered").and_then(Value::as_str).map(String::from),
            sockets: strings("sockets"),
            made: strings("made"),
        }
    }

    fn save(&self, record: &Record) -> Result<(), KmuxError> {
        let value = json!({ "version": RECORD_VERSION, "setup": record.setup, "files": record.files, "answered": record.answered, "sockets": record.sockets, "made": record.made });
        let file = self.record_file();
        write_atomic(&file, format!("{}\n", serde_json::to_string_pretty(&value).unwrap_or_default()).as_bytes()).map_err(|e| KmuxError::new("failed", format!("can't write {}: {e}", file.display())))
    }
}

type Shipped = BTreeMap<String, BTreeMap<PathBuf, Vec<u8>>>;

impl Want {
    fn hash(&self) -> String {
        match self {
            Want::File(content) => digest(content),
            Want::Link(target) => format!("-> {target}"),
        }
    }

    fn write(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        if fs::symlink_metadata(path).is_ok_and(|m| !m.is_dir()) {
            remove_file(path);
        }
        match self {
            Want::File(content) => write_atomic(path, content),
            Want::Link(target) => std::os::unix::fs::symlink(target, path),
        }
    }
}

fn summary(plan: &[Planned]) -> BTreeMap<String, &'static str> {
    let mut by_skill: BTreeMap<String, Vec<&Step>> = BTreeMap::new();
    for planned in plan {
        by_skill.entry(planned.skill.clone()).or_default().push(&planned.step);
    }
    let mut out = BTreeMap::new();
    for (skill, steps) in by_skill {
        let any = |f: fn(&Step) -> bool| steps.iter().any(|s| f(s));
        let all = |f: fn(&Step) -> bool| steps.iter().all(|s| f(s));
        let change = if any(|s| matches!(s, Step::Conflict)) {
            "conflict"
        } else if any(|s| matches!(s, Step::Merge(_))) {
            "merged"
        } else if all(|s| matches!(s, Step::New)) {
            "new"
        } else if all(|s| matches!(s, Step::Drop | Step::Theirs)) && any(|s| matches!(s, Step::Drop)) {
            "removed"
        } else if any(|s| matches!(s, Step::New | Step::Update | Step::Drop)) {
            "updated"
        } else {
            continue;
        };
        out.insert(skill, change);
    }
    out
}

fn reply(action: &str, report: &Report) -> Map {
    let mut out = Map::new();
    out.insert("action".into(), json!(action));
    out.insert("changes".into(), json!(report.changes));
    out.insert("conflicts".into(), Value::Array(report.conflicts.iter().map(|(path, base, new)| json!({ "path": path, "base": base, "new": new })).collect()));
    out.insert("kept".into(), json!(report.kept));
    out.insert("settings".into(), json!(report.settings));
    out
}

fn no_skills() -> KmuxError {
    KmuxError::new("not_found", "this Kanna has no agent skills to install (it isn't running from Kanna.app)")
}

fn fingerprint(shipped: &Shipped) -> String {
    let mut hasher = Sha256::new();
    for (name, files) in shipped {
        for (relative, content) in files {
            hasher.update(format!("{name}/{}\0{}\0", relative.display(), content.len()));
            hasher.update(content);
        }
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn merge(mine: &[u8], base: &[u8], new: &[u8]) -> Option<Vec<u8>> {
    let dir = std::env::temp_dir().join(format!("kanna-merge-{}-{}", std::process::id(), digest(mine).get(..12).unwrap_or_default()));
    fs::create_dir_all(&dir).ok()?;
    let files = [("mine", mine), ("base", base), ("new", new)].map(|(name, content)| {
        let file = dir.join(name);
        fs::write(&file, content).map(|_| file)
    });
    let out = match files {
        [Ok(mine), Ok(base), Ok(new)] => Command::new("git").args(["merge-file", "-p", "-q"]).arg(&mine).arg(&base).arg(&new).output().ok(),
        _ => None,
    };
    let _ = fs::remove_dir_all(&dir);
    out.filter(|o| o.status.code() == Some(0)).map(|o| o.stdout)
}

fn ensure<'a>(object: &'a mut Map, key: &str, made: &mut Made, tag: &str) -> Option<&'a mut Map> {
    if !object.contains_key(key) {
        made.add(tag);
    }
    object.entry(key).or_insert_with(|| json!({})).as_object_mut()
}

fn list<'a>(object: &'a mut Map, key: &str, made: &mut Made, tag: &str) -> Option<&'a mut Vec<Value>> {
    if !object.contains_key(key) {
        made.add(tag);
    }
    object.entry(key).or_insert_with(|| json!([])).as_array_mut()
}

fn prune(object: &mut Map, key: &str, made: &mut Made, tag: &str) {
    let empty = match object.get(key) {
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        Some(_) => false,
        None => true,
    };
    if empty && made.has(tag) {
        object.remove(key);
        made.forget(tag);
    }
}

fn drop_hook(object: &mut Map, made: &mut Made) {
    if let Some(hooks) = object.get_mut("hooks").and_then(Value::as_object_mut) {
        if let Some(starts) = hooks.get_mut("SessionStart").and_then(Value::as_array_mut) {
            starts.retain(|g| {
                let hooks = g.get("hooks").and_then(Value::as_array);
                !hooks.is_some_and(|h| !h.is_empty() && h.iter().all(|h| h.get("command").and_then(Value::as_str).is_some_and(|c| c.ends_with(MARK))))
            });
            for group in starts.iter_mut() {
                if let Some(list) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                    list.retain(|h| !h.get("command").and_then(Value::as_str).is_some_and(|c| c.ends_with(MARK)));
                }
            }
        }
        prune(hooks, "SessionStart", made, "hooks.SessionStart");
    }
    prune(object, "hooks", made, "hooks");
}

struct Made<'a> {
    list: &'a mut Vec<String>,
    file: String,
}

impl Made<'_> {
    fn tag(&self, tag: &str) -> String {
        if tag.is_empty() { self.file.clone() } else { format!("{}#{tag}", self.file) }
    }

    fn add(&mut self, tag: &str) {
        let tag = self.tag(tag);
        if !self.list.contains(&tag) {
            self.list.push(tag);
        }
    }

    fn has(&self, tag: &str) -> bool {
        self.list.contains(&self.tag(tag))
    }

    fn forget(&mut self, tag: &str) {
        let tag = self.tag(tag);
        self.list.retain(|m| m != &tag);
    }
}

fn edit(file: &Path, create: bool, made: &mut Vec<String>, change: impl FnOnce(&mut Map, &mut Made)) -> Result<bool, KmuxError> {
    let failed = |e: io::Error| KmuxError::new("failed", format!("can't write {}: {e}", file.display()));
    let mut made = Made { list: made, file: file.to_string_lossy().into_owned() };
    let mut settings = match read_settings(file) {
        Some(settings) => settings,
        None if create && !exists(file) => {
            made.add("");
            json!({})
        }
        None => return Ok(false),
    };
    let before = settings.clone();
    let Some(object) = settings.as_object_mut() else { return Ok(false) };
    change(object, &mut made);
    if !create && object.is_empty() && made.has("") {
        fs::remove_file(file).map_err(failed)?;
        made.forget("");
    } else if settings != before || create && !exists(file) {
        write_settings(file, &settings).map_err(failed)?;
    }
    if !create {
        let prefix = format!("{}#", made.file);
        made.list.retain(|m| !m.starts_with(&prefix));
    }
    Ok(create || settings != before)
}

fn digest(content: &[u8]) -> String {
    Sha256::digest(content).iter().map(|b| format!("{b:02x}")).collect()
}

fn current(path: &Path) -> Option<String> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        return Some(format!("-> {}", fs::read_link(path).ok()?.to_string_lossy()));
    }
    if meta.is_dir() {
        return Some("dir".into());
    }
    fs::read(path).ok().map(|content| digest(&content))
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn owned(path: &Path, hash: &str) -> bool {
    current(path).as_deref() == Some(hash)
}

fn remove_file(path: &Path) {
    let _ = fs::remove_file(path);
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d.file_name().is_some_and(|n| n == "skills") || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

fn collect(dir: &Path, relative: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = relative.join(entry.file_name());
        if path.is_dir() {
            collect(&path, &name, files);
        } else if let Ok(content) = fs::read(&path) {
            files.insert(name, content);
        }
    }
}

fn read_settings(file: &Path) -> Option<Value> {
    let text = fs::read_to_string(file).ok()?;
    serde_json::from_str::<Value>(&text).ok().filter(Value::is_object)
}

fn write_settings(file: &Path, settings: &Value) -> io::Result<()> {
    let real = fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    write_atomic(&real, format!("{}\n", serde_json::to_string_pretty(settings).unwrap_or_default()).as_bytes())
}

fn write_atomic(file: &Path, content: &[u8]) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mode = fs::metadata(file).map(|m| m.permissions().mode() & 0o777).unwrap_or(0o644);
    let temp = file.with_file_name(format!(".{}.kanna-{}", file.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    let _ = fs::remove_file(&temp);
    let mut out = fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&temp)?;
    out.write_all(content)?;
    out.sync_all()?;
    fs::rename(&temp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        agents: Agents,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("kanna-agents-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let skills = root.join("app/kanna-skills");
        fs::create_dir_all(skills.join("kanna-tasks/notes")).unwrap();
        fs::write(skills.join("kanna-tasks/SKILL.md"), "tasks v1").unwrap();
        fs::write(skills.join("kanna-tasks/notes/more.md"), "more").unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        Fixture { agents: Agents::new(root.join("home"), Some(skills), "/S/kanna.sock".into()), root }
    }

    fn read(f: &Fixture, path: &str) -> String {
        fs::read_to_string(f.agents.home.join(path)).unwrap()
    }

    fn write(f: &Fixture, path: &str, content: &str) {
        fs::write(f.agents.home.join(path), content).unwrap();
    }

    fn ship(f: &Fixture, path: &str, content: &str) {
        let file = f.root.join("app/kanna-skills").join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }

    fn changes(f: &Fixture) -> Value {
        Value::Object(f.agents.status().unwrap()["changes"].as_object().unwrap().clone())
    }

    const SKILL: &str = ".agents/skills/kanna-tasks/SKILL.md";

    #[test]
    fn nothing_is_written_until_the_user_agrees() {
        let f = fixture("ask");
        assert_eq!(f.agents.status().unwrap()["setup"], "ask");
        assert_eq!(f.agents.ask("setup").unwrap()["ask"], true);
        assert_eq!(f.agents.ask("setup").unwrap()["ask"], false, "one terminal asks per launch");
        f.agents.refresh();
        assert!(!f.agents.home.join(".agents").exists() && !f.agents.home.join(".claude").exists());
        f.agents.decline().unwrap();
        assert_eq!(f.agents.status().unwrap()["setup"], "no");
        f.agents.refresh();
        assert!(!f.agents.home.join(".agents").exists());
    }

    #[test]
    fn setup_installs_for_codex_and_links_for_claude() {
        let f = fixture("setup");
        let report = f.agents.setup(false).unwrap();
        assert!(report.kept.is_empty());
        assert_eq!(report.changes.get("kanna-tasks"), Some(&"new"));
        assert_eq!(read(&f, SKILL), "tasks v1");
        assert_eq!(read(&f, ".agents/skills/kanna-tasks/notes/more.md"), "more");
        assert_eq!(fs::read_link(f.agents.home.join(".claude/skills/kanna-tasks")).unwrap(), Path::new("../../.agents/skills/kanna-tasks"));
        assert_eq!(read(&f, ".claude/skills/kanna-tasks/SKILL.md"), "tasks v1");
        assert_eq!(read(&f, ".kanna-v4/agents/.agents/skills/kanna-tasks/SKILL.md"), "tasks v1", "a copy as installed, to merge against");
        assert_eq!(f.agents.status().unwrap()["setup"], "yes");
        assert_eq!(changes(&f), json!({}));
    }

    #[test]
    fn a_new_kanna_asks_before_it_updates() {
        let f = fixture("update");
        f.agents.setup(false).unwrap();
        ship(&f, "kanna-tasks/SKILL.md", "tasks v2");
        fs::remove_file(f.root.join("app/kanna-skills/kanna-tasks/notes/more.md")).unwrap();
        ship(&f, "kanna-planning/SKILL.md", "planning");
        f.agents.refresh();
        assert_eq!(read(&f, SKILL), "tasks v1", "a launch doesn't update");
        assert_eq!(changes(&f), json!({"kanna-planning": "new", "kanna-tasks": "updated"}));
        assert_eq!(f.agents.ask("update").unwrap()["ask"], true);
        assert_eq!(f.agents.ask("update").unwrap()["ask"], false, "not now: not again this launch");
        f.agents.update(false).unwrap();
        assert_eq!(read(&f, SKILL), "tasks v2");
        assert!(!f.agents.home.join(".agents/skills/kanna-tasks/notes").exists());
        assert_eq!(read(&f, ".claude/skills/kanna-planning/SKILL.md"), "planning");
        fs::remove_dir_all(f.root.join("app/kanna-skills/kanna-planning")).unwrap();
        assert_eq!(changes(&f), json!({"kanna-planning": "removed"}));
        f.agents.update(false).unwrap();
        assert!(!f.agents.home.join(".agents/skills/kanna-planning").exists());
        assert!(fs::symlink_metadata(f.agents.home.join(".claude/skills/kanna-planning")).is_err());
        assert!(!f.agents.home.join(".kanna-v4/agents/.agents/skills/kanna-planning").exists());
    }

    #[test]
    fn skip_this_version_waits_for_a_newer_one() {
        let f = fixture("skip");
        f.agents.setup(false).unwrap();
        ship(&f, "kanna-tasks/SKILL.md", "tasks v2");
        f.agents.skip().unwrap();
        let fresh = Agents::new(f.agents.home.clone(), f.agents.skills.clone(), f.agents.socket.clone());
        assert_eq!(fresh.ask("update").unwrap()["ask"], false);
        ship(&f, "kanna-tasks/SKILL.md", "tasks v3");
        assert_eq!(fresh.ask("update").unwrap()["ask"], true);
    }

    #[test]
    fn your_changes_are_merged_in() {
        let f = fixture("merge");
        ship(&f, "kanna-tasks/SKILL.md", "one\ntwo\nthree\n");
        f.agents.setup(false).unwrap();
        write(&f, SKILL, "one\ntwo\nthree\nmine\n");
        ship(&f, "kanna-tasks/SKILL.md", "ONE\ntwo\nthree\n");
        assert_eq!(changes(&f), json!({"kanna-tasks": "merged"}));
        f.agents.update(false).unwrap();
        assert_eq!(read(&f, SKILL), "ONE\ntwo\nthree\nmine\n");
        ship(&f, "kanna-tasks/SKILL.md", "ONE\nTWO\nthree\n");
        assert_eq!(changes(&f), json!({"kanna-tasks": "merged"}), "merged against what Kanna last installed");
        f.agents.update(false).unwrap();
        assert_eq!(read(&f, SKILL), "ONE\nTWO\nthree\nmine\n");
    }

    #[test]
    fn a_conflict_keeps_yours_until_you_merge_it() {
        let f = fixture("conflict");
        f.agents.setup(false).unwrap();
        write(&f, SKILL, "tasks mine");
        ship(&f, "kanna-tasks/SKILL.md", "tasks v2");
        let report = f.agents.update(false).unwrap();
        assert_eq!(report.changes.get("kanna-tasks"), Some(&"conflict"));
        let (path, base, new) = &report.conflicts[0];
        assert_eq!(path, &f.agents.home.join(SKILL).to_string_lossy());
        assert_eq!(fs::read_to_string(base).unwrap(), "tasks v1");
        assert_eq!(fs::read_to_string(new).unwrap(), "tasks v2");
        assert_eq!(read(&f, SKILL), "tasks mine");
        assert_eq!(changes(&f), json!({"kanna-tasks": "conflict"}), "kanna agents update offers again");
        write(&f, SKILL, "tasks mine and v2");
        f.agents.resolved(path).unwrap();
        assert_eq!(changes(&f), json!({}));
        ship(&f, "kanna-tasks/SKILL.md", "tasks v3");
        f.agents.update(true).unwrap();
        assert_eq!(read(&f, SKILL), "tasks v3", "--force takes Kanna's");
    }

    #[test]
    fn a_file_that_was_there_before_is_yours() {
        let f = fixture("theirs");
        fs::create_dir_all(f.agents.home.join(".claude/skills/kanna-tasks")).unwrap();
        write(&f, ".claude/skills/kanna-tasks/SKILL.md", "mine");
        let report = f.agents.setup(false).unwrap();
        assert_eq!(report.kept, [f.agents.home.join(".claude/skills/kanna-tasks").to_string_lossy()]);
        assert_eq!(read(&f, ".claude/skills/kanna-tasks/SKILL.md"), "mine");
        f.agents.update(true).unwrap();
        assert_eq!(read(&f, ".claude/skills/kanna-tasks/SKILL.md"), "mine", "a folder of the user's is never replaced");
    }

    #[test]
    fn an_identical_copy_is_adopted() {
        let f = fixture("adopt");
        fs::create_dir_all(f.agents.home.join(".agents/skills/kanna-tasks")).unwrap();
        write(&f, SKILL, "tasks v1");
        assert!(f.agents.setup(false).unwrap().kept.is_empty());
        ship(&f, "kanna-tasks/SKILL.md", "tasks v2");
        f.agents.update(false).unwrap();
        assert_eq!(read(&f, SKILL), "tasks v2");
    }

    #[test]
    fn remove_takes_back_only_what_kanna_wrote() {
        let f = fixture("remove");
        fs::create_dir_all(f.agents.home.join(".agents/skills/other")).unwrap();
        write(&f, ".agents/skills/other/SKILL.md", "x");
        f.agents.setup(false).unwrap();
        write(&f, ".agents/skills/kanna-tasks/notes/more.md", "edited");
        f.agents.remove().unwrap();
        assert!(!f.agents.home.join(SKILL).exists());
        assert_eq!(read(&f, ".agents/skills/kanna-tasks/notes/more.md"), "edited");
        assert!(fs::symlink_metadata(f.agents.home.join(".claude/skills/kanna-tasks")).is_err());
        assert!(f.agents.home.join(".agents/skills/other/SKILL.md").exists());
        assert!(!f.agents.home.join(".kanna-v4/agents").exists());
        assert_eq!(f.agents.status().unwrap()["setup"], "no");
    }

    #[test]
    fn claude_s_sandbox_lets_agents_reach_kanna() {
        let f = fixture("sandbox");
        let settings = f.agents.home.join(".claude/settings.json");
        f.agents.setup(false).unwrap();
        let made: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(made, json!({"sandbox": {"network": {"allowUnixSockets": ["/S/kanna.sock"]}}}), "no hooks");
        f.agents.remove().unwrap();
        assert!(!settings.exists(), "a settings file Kanna made goes when it's emptied");

        fs::write(&settings, r#"{"model":"opus","sandbox":{"enabled":true,"network":{"allowUnixSockets":["/u/mine.sock"]}},"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"echo mine"}]}]},"env":{}}"#).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&settings, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(f.agents.setup(false).unwrap().settings);
        assert_eq!(fs::metadata(&settings).unwrap().permissions().mode() & 0o777, 0o600);
        let text = fs::read_to_string(&settings).unwrap();
        assert!(text.find("\"model\"").unwrap() < text.find("\"sandbox\"").unwrap() && text.find("\"sandbox\"").unwrap() < text.find("\"env\"").unwrap(), "{text}");
        f.agents.refresh();
        let value: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(value["sandbox"]["network"]["allowUnixSockets"], json!(["/u/mine.sock", "/S/kanna.sock"]));
        f.agents.remove().unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(value, json!({"model": "opus", "sandbox": {"enabled": true, "network": {"allowUnixSockets": ["/u/mine.sock"]}}, "hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "echo mine"}]}]}, "env": {}}));

        fs::write(&settings, "{ not json").unwrap();
        f.agents.setup(false).unwrap();
        assert_eq!(fs::read_to_string(&settings).unwrap(), "{ not json");
    }

    const EARLIER_HOOK: &str = "if [ -n \"$KANNA_PLANNER\" ]; then echo \"You're running in Kanna, as a window's planning agent: use the kanna-planning skill now.\"; fi # kanna agents setup";

    fn earlier_kanna(f: &Fixture, made: &[&str]) {
        let home = f.agents.home.to_string_lossy().into_owned();
        let made: Vec<String> = made.iter().map(|m| format!("{home}/{m}")).collect();
        fs::create_dir_all(f.agents.home.join(".kanna-v4")).unwrap();
        fs::write(f.agents.record_file(), json!({ "version": RECORD_VERSION, "setup": true, "files": {}, "answered": null, "sockets": ["/S/kanna.sock"], "made": made }).to_string()).unwrap();
    }

    fn hooks(file: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap()
    }

    #[test]
    fn setup_takes_out_an_earlier_kanna_s_hooks_and_leaves_yours() {
        let f = fixture("old-hooks");
        let claude = f.agents.home.join(".claude/settings.json");
        let codex = f.agents.home.join(".codex/hooks.json");
        fs::create_dir_all(claude.parent().unwrap()).unwrap();
        fs::create_dir_all(codex.parent().unwrap()).unwrap();
        let ours = json!({ "type": "command", "command": EARLIER_HOOK });
        let mine = json!({ "type": "command", "command": "echo mine" });
        fs::write(&claude, json!({ "model": "opus", "sandbox": { "network": { "allowUnixSockets": ["/S/kanna.sock"] } }, "hooks": { "SessionStart": [{ "hooks": [mine, ours] }, { "hooks": [ours] }], "Stop": [{ "hooks": [mine] }] } }).to_string()).unwrap();
        fs::write(&codex, json!({ "hooks": { "SessionStart": [{ "hooks": [ours] }] } }).to_string()).unwrap();
        earlier_kanna(&f, &[".claude/settings.json#sandbox", ".claude/settings.json#sandbox.network", ".claude/settings.json#sandbox.network.allowUnixSockets", ".codex/hooks.json#hooks.SessionStart"]);

        f.agents.setup(false).unwrap();
        assert_eq!(hooks(&claude), json!({ "model": "opus", "sandbox": { "network": { "allowUnixSockets": ["/S/kanna.sock"] } }, "hooks": { "SessionStart": [{ "hooks": [mine] }], "Stop": [{ "hooks": [mine] }] } }));
        assert_eq!(hooks(&codex), json!({ "hooks": {} }), "the SessionStart list Kanna made goes; the file and its hooks are yours");
    }

    #[test]
    fn update_and_each_launch_take_out_an_earlier_kanna_s_hooks_too() {
        let f = fixture("old-hooks-update");
        let claude = f.agents.home.join(".claude/settings.json");
        let codex = f.agents.home.join(".codex/hooks.json");
        fs::create_dir_all(claude.parent().unwrap()).unwrap();
        fs::create_dir_all(codex.parent().unwrap()).unwrap();
        let ours = json!({ "type": "command", "command": EARLIER_HOOK });
        fs::write(&claude, json!({ "hooks": { "SessionStart": [{ "hooks": [ours] }] } }).to_string()).unwrap();
        fs::write(&codex, json!({ "hooks": { "SessionStart": [{ "hooks": [ours] }] } }).to_string()).unwrap();
        let made = [".claude/settings.json#hooks", ".claude/settings.json#hooks.SessionStart", ".codex/hooks.json", ".codex/hooks.json#hooks", ".codex/hooks.json#hooks.SessionStart"];
        earlier_kanna(&f, &made);
        f.agents.update(false).unwrap();
        assert_eq!(hooks(&claude), json!({ "sandbox": { "network": { "allowUnixSockets": ["/S/kanna.sock"] } } }));
        assert!(!codex.exists(), "a hooks.json Kanna made goes when its hook does");

        fs::write(&claude, json!({ "hooks": { "SessionStart": [{ "hooks": [ours] }] } }).to_string()).unwrap();
        fs::write(&codex, json!({ "hooks": { "SessionStart": [{ "hooks": [ours] }] } }).to_string()).unwrap();
        earlier_kanna(&f, &made);
        f.agents.refresh();
        assert_eq!(hooks(&claude)["hooks"], Value::Null);
        assert!(!codex.exists());
    }
}
