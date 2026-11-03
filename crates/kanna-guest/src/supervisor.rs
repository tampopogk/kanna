use crate::frame::{self, bad, Frame};
use crate::manifest::{self, Entry, Kind};
use crate::paths::{self, Roots};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

const MAX_MANIFEST: u64 = 256 << 20;

const MAX_BUNDLE: u64 = 4 << 30;

const MAX_BLOB: u64 = 4 << 30;

const KILL_AFTER: Duration = Duration::from_secs(10);

const POLL: Duration = Duration::from_millis(100);

const ENV_ALLOWED: &[&str] = &["KANNA_CHANNEL", "CI", "LANG", "LC_ALL", "RUST_BACKTRACE", "CARGO_TERM_COLOR", "NSUnbufferedIO", "SWIFT_DETERMINISTIC_HASHING"];

#[derive(Clone)]
pub struct Config {
    pub state: PathBuf,
    pub path: String,
    pub proxy: Option<String>,
    pub require_console: bool,
}

impl Config {
    pub fn new(state: PathBuf) -> Config {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = format!("{home}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin");
        Config { state, path, proxy: None, require_console: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Queued,
    Running,
    Exited(i32),
    Signalled(i32),
    Failed(String),
    Cancelled,
}

impl Phase {
    fn done(&self) -> bool {
        !matches!(self, Phase::Queued | Phase::Running)
    }

    fn to_json(&self) -> Value {
        match self {
            Phase::Queued => json!({ "phase": "queued" }),
            Phase::Running => json!({ "phase": "running" }),
            Phase::Exited(code) => json!({ "phase": "exited", "code": code }),
            Phase::Signalled(signal) => json!({ "phase": "signalled", "signal": signal }),
            Phase::Failed(error) => json!({ "phase": "failed", "error": error }),
            Phase::Cancelled => json!({ "phase": "cancelled" }),
        }
    }

    fn from_json(value: &Value) -> Phase {
        match value["phase"].as_str() {
            Some("exited") => Phase::Exited(value["code"].as_i64().unwrap_or(1) as i32),
            Some("signalled") => Phase::Signalled(value["signal"].as_i64().unwrap_or(9) as i32),
            Some("cancelled") => Phase::Cancelled,
            Some("failed") => Phase::Failed(value["error"].as_str().unwrap_or("failed").into()),
            _ => Phase::Failed("the supervisor restarted while the job ran".into()),
        }
    }
}

struct JobState {
    phase: Phase,
    pid: Option<i32>,
    cancel: bool,
    started: Value,
    artifacts: Vec<Value>,
    artifacts_ready: bool,
}

struct Job {
    id: String,
    dir: PathBuf,
    state: Mutex<JobState>,
    changed: Condvar,
}

impl Job {
    fn status(&self) -> Value {
        let state = self.state.lock().unwrap();
        let mut status = state.phase.to_json();
        status["job"] = json!(self.id);
        status["started"] = state.started.clone();
        status["artifacts"] = json!(state.artifacts);
        status["artifacts_ready"] = json!(state.artifacts_ready);
        status
    }

    fn save(&self) {
        let _ = write_atomic(&self.dir.join("status.json"), &serde_json::to_vec_pretty(&self.status()).unwrap_or_default());
    }

    fn update(&self, change: impl FnOnce(&mut JobState)) {
        change(&mut self.state.lock().unwrap());
        self.save();
        self.changed.notify_all();
    }
}

pub struct Supervisor {
    config: Config,
    jobs: Mutex<HashMap<String, Arc<Job>>>,
    mirrors: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
    heavy: Mutex<()>,
    ui: Mutex<()>,
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(temp, path)
}

pub fn valid_id(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len()) && !name.starts_with('.') && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

fn is_commit(text: &str) -> bool {
    (text.len() == 40 || text.len() == 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

fn git(dir: &Path, args: &[&str]) -> io::Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).env("GIT_TERMINAL_PROMPT", "0").stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return Err(io::Error::other(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn output_of(program: &str, args: &[&str], env: &[(String, String)]) -> String {
    Command::new(program).args(args).env_clear().envs(env.iter().cloned()).stdin(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

impl Supervisor {
    pub fn new(config: Config) -> io::Result<Arc<Supervisor>> {
        for dir in ["jobs", "store", "repos", "mirrors"] {
            std::fs::create_dir_all(config.state.join(dir))?;
        }
        let _ = std::fs::set_permissions(&config.state, std::fs::Permissions::from_mode(0o700));
        let supervisor = Arc::new(Supervisor { config, jobs: Mutex::new(HashMap::new()), mirrors: Mutex::new(HashMap::new()), heavy: Mutex::new(()), ui: Mutex::new(()) });
        supervisor.recover();
        Ok(supervisor)
    }

    fn recover(&self) {
        let Ok(dirs) = std::fs::read_dir(self.config.state.join("jobs")) else { return };
        for dir in dirs.flatten() {
            let id = dir.file_name().to_string_lossy().into_owned();
            let Ok(status) = std::fs::read(dir.path().join("status.json")).map(|b| serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null)) else { continue };
            let phase = Phase::from_json(&status);
            let job = Arc::new(Job { id: id.clone(), dir: dir.path(), state: Mutex::new(JobState { phase, pid: None, cancel: false, started: status["started"].clone(), artifacts: status["artifacts"].as_array().cloned().unwrap_or_default(), artifacts_ready: true }), changed: Condvar::new() });
            job.save();
            self.jobs.lock().unwrap().insert(id, job);
        }
    }

    pub fn serve(self: &Arc<Self>, mut conn: impl Read + Write) -> io::Result<()> {
        let (first, payload) = match frame::read_header(&mut conn, MAX_MANIFEST) {
            Ok(first) => {
                let payload = frame::read_payload(&mut conn, &first)?;
                (first, payload)
            }
            Err(e) => return Err(e),
        };
        let result = match first.op() {
            "hello" => frame::send(&mut conn, &json!({ "op": "hello", "versions": self.versions(), "identity": identity() }), b""),
            "run" => self.run(&mut conn, &first, &payload),
            "attach" => {
                let job = self.job(first.str("job").unwrap_or_default())?;
                self.attach(&mut conn, &job, offsets(&first.header["at"]))
            }
            "cancel" => {
                let job = self.job(first.str("job").unwrap_or_default())?;
                self.cancel(&job);
                frame::send(&mut conn, &json!({ "op": "ok", "status": job.status() }), b"")
            }
            "status" => {
                let job = self.job(first.str("job").unwrap_or_default())?;
                frame::send(&mut conn, &json!({ "op": "status", "status": job.status() }), b"")
            }
            other => Err(bad(format!("no op {other:?}"))),
        };
        if let Err(e) = &result {
            let _ = frame::send(&mut conn, &json!({ "op": "error", "error": e.to_string() }), b"");
        }
        result
    }

    fn job(&self, id: &str) -> io::Result<Arc<Job>> {
        self.jobs.lock().unwrap().get(id).cloned().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("no such job {id}")))
    }

    fn versions(&self) -> Value {
        let env = self.base_env(None);
        json!({
            "macos": output_of("/usr/bin/sw_vers", &["-productVersion"], &env),
            "arch": std::env::consts::ARCH,
            "xcode": output_of("/usr/bin/xcodebuild", &["-version"], &env),
            "swift": output_of("/usr/bin/xcrun", &["swift", "--version"], &env).lines().next().unwrap_or_default(),
            "developer_dir": output_of("/usr/bin/xcode-select", &["-p"], &env),
        })
    }

    fn base_env(&self, developer_dir: Option<&str>) -> Vec<(String, String)> {
        let home = std::env::var("HOME").unwrap_or_default();
        let mut env = vec![("PATH".to_string(), self.config.path.clone()), ("HOME".to_string(), home), ("USER".to_string(), std::env::var("USER").unwrap_or_default()), ("LANG".to_string(), "en_US.UTF-8".to_string()), ("TMPDIR".to_string(), std::env::temp_dir().to_string_lossy().into_owned())];
        if let Some(dir) = developer_dir {
            env.push(("DEVELOPER_DIR".to_string(), dir.to_string()));
        }
        if let Some(proxy) = &self.config.proxy {
            for name in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY"] {
                env.push((name.to_string(), proxy.clone()));
            }
        }
        env
    }

    fn run(self: &Arc<Self>, conn: &mut (impl Read + Write), first: &Frame, payload: &[u8]) -> io::Result<()> {
        let id = first.str("job").unwrap_or_default().to_string();
        if !valid_id(&id) {
            return Err(bad("a job needs an ID of 8 to 64 letters, digits and dashes"));
        }
        let mirror = &first.header["mirror"];
        let names: Vec<&str> = ["machine", "repo", "task"].iter().map(|k| mirror[k].as_str().unwrap_or_default()).collect();
        if !names.iter().all(|n| valid_name(n)) {
            return Err(bad("a job's mirror needs a machine, repo and task name"));
        }
        let commit = first.str("commit").unwrap_or_default().to_string();
        if !is_commit(&commit) {
            return Err(bad("a job needs the commit its worktree is on"));
        }
        let argv: Vec<String> = first.header["argv"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        if argv.is_empty() || argv[0].is_empty() {
            return Err(bad("a job needs a command"));
        }
        if let Some(existing) = self.jobs.lock().unwrap().get(&id).cloned() {
            frame::send(conn, &json!({ "op": "exists" }), b"")?;
            return self.attach(conn, &existing, [0, 0]);
        }
        let entries = manifest::decode(payload)?;
        let dir = self.config.state.join("jobs").join(&id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("manifest.json"), payload)?;
        let mut request = first.header.clone();
        request["snapshot"] = json!(manifest::sha256(payload));
        std::fs::write(dir.join("request.json"), serde_json::to_vec_pretty(&request).unwrap_or_default())?;
        let repo = self.config.state.join("repos").join(names[0]).join(format!("{}.git", names[1]));
        if !repo.join("HEAD").exists() {
            std::fs::create_dir_all(&repo)?;
            git(&repo, &["init", "-q", "--bare"])?;
        }
        let known = git(&repo, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_ok();
        let heads: Vec<String> = if known { vec![] } else { git(&repo, &["for-each-ref", "--format=%(objectname)", "refs/kanna/"]).unwrap_or_default().lines().map(String::from).take(64).collect() };
        let store = self.config.state.join("store");
        let mut need: Vec<String> = entries.iter().filter_map(|e| match &e.kind { Kind::File { hash, .. } if !store.join(hash).exists() => Some(hash.clone()), _ => None }).collect();
        need.sort();
        need.dedup();
        frame::send(conn, &json!({ "op": "need", "blobs": need, "commit": known, "heads": heads }), b"")?;
        let mut missing: HashSet<String> = need.into_iter().collect();
        let mut bundle = None;
        loop {
            let frame = frame::read_header(conn, MAX_BLOB.max(MAX_BUNDLE))?;
            match frame.op() {
                "blob" => {
                    let hash = frame.str("hash").unwrap_or_default().to_string();
                    if !missing.contains(&hash) {
                        return Err(bad(format!("blob {hash} wasn't asked for")));
                    }
                    let temp = store.join(format!(".in-{id}"));
                    let mut out = File::create(&temp)?;
                    let (got, size) = manifest::hash_reader(Read::by_ref(conn).take(frame.size), Some(&mut out))?;
                    drop(out);
                    if size != frame.size || got != hash {
                        let _ = std::fs::remove_file(&temp);
                        return Err(bad(format!("blob {hash} didn't match its hash")));
                    }
                    std::fs::rename(&temp, store.join(&hash))?;
                    missing.remove(&hash);
                }
                "bundle" => {
                    let file = dir.join("bundle");
                    let mut out = File::create(&file)?;
                    let copied = io::copy(&mut Read::by_ref(conn).take(frame.size), &mut out)?;
                    if copied != frame.size {
                        return Err(bad("the bundle was cut short"));
                    }
                    bundle = Some(file);
                }
                "sent" => break,
                other => return Err(bad(format!("unexpected {other:?} while receiving a snapshot"))),
            }
        }
        if !missing.is_empty() {
            return Err(bad(format!("{} files of the snapshot never arrived", missing.len())));
        }
        if let Some(bundle) = &bundle {
            git(&repo, &["fetch", "-q", &bundle.to_string_lossy(), &format!("+HEAD:refs/kanna/{}", &commit[..12])])?;
            let _ = std::fs::remove_file(bundle);
        } else if !known {
            return Err(bad(format!("the build Mac lacks commit {commit} and no bundle came with it")));
        }
        let job = Arc::new(Job { id: id.clone(), dir, state: Mutex::new(JobState { phase: Phase::Queued, pid: None, cancel: false, started: Value::Null, artifacts: vec![], artifacts_ready: false }), changed: Condvar::new() });
        job.save();
        self.jobs.lock().unwrap().insert(id, job.clone());
        let (me, worker) = (self.clone(), job.clone());
        let mirror_dir = self.config.state.join("mirrors").join(names[0]).join(names[1]).join(names[2]);
        std::thread::Builder::new().name(format!("job {}", job.id)).spawn(move || me.work(&worker, &repo, &mirror_dir, request, entries))?;
        self.attach(conn, &job, [0, 0])
    }

    fn cancel(&self, job: &Job) {
        let pid = {
            let mut state = job.state.lock().unwrap();
            state.cancel = true;
            state.pid
        };
        if let Some(pid) = pid {
            unsafe { libc::kill(-pid, libc::SIGTERM) };
        }
        job.changed.notify_all();
    }

    fn mirror_lock(&self, dir: &Path) -> Arc<Mutex<()>> {
        self.mirrors.lock().unwrap().entry(dir.to_path_buf()).or_default().clone()
    }

    fn work(&self, job: &Arc<Job>, repo: &Path, mirror: &Path, request: Value, entries: Vec<Entry>) {
        let lock = self.mirror_lock(mirror);
        let _mirror = lock.lock().unwrap_or_else(|p| p.into_inner());
        let ui = request["ui"].as_bool().unwrap_or(false);
        let _slot = if ui { self.ui.lock() } else { self.heavy.lock() }.unwrap_or_else(|p| p.into_inner());
        let outcome = self.execute(job, repo, mirror, &request, &entries);
        let phase = match outcome {
            Ok(phase) => phase,
            Err(e) => Phase::Failed(e.to_string()),
        };
        let artifacts = if matches!(phase, Phase::Exited(_) | Phase::Signalled(_) | Phase::Cancelled) { self.artifacts(job, mirror, &request) } else { vec![] };
        job.update(|state| {
            state.phase = phase;
            state.pid = None;
            state.artifacts = artifacts;
            state.artifacts_ready = true;
        });
    }

    fn execute(&self, job: &Arc<Job>, repo: &Path, mirror: &Path, request: &Value, entries: &[Entry]) -> io::Result<Phase> {
        if job.state.lock().unwrap().cancel {
            return Ok(Phase::Cancelled);
        }
        let commit = request["commit"].as_str().unwrap_or_default();
        let src = mirror.join("src");
        if !src.join(".git").exists() {
            std::fs::create_dir_all(&src)?;
            git(&src, &["init", "-q"])?;
            let alternates = src.join(".git/objects/info/alternates");
            std::fs::create_dir_all(alternates.parent().unwrap_or(&src))?;
            std::fs::write(&alternates, format!("{}\n", repo.join("objects").canonicalize()?.display()))?;
        }
        git(&src, &["reset", "-q", commit])?;
        lay(&src, &self.config.state.join("store"), mirror, entries)?;
        let developer_dir = request["developer_dir"].as_str().map(String::from).unwrap_or_else(|| output_of("/usr/bin/xcode-select", &["-p"], &[]));
        let mut env = self.base_env((!developer_dir.is_empty()).then_some(developer_dir.as_str()));
        if let Some(extra) = request["env"].as_object() {
            for (name, value) in extra {
                if let (true, Some(value)) = (ENV_ALLOWED.contains(&name.as_str()), value.as_str()) {
                    env.retain(|(n, _)| n != name);
                    env.push((name.clone(), value.to_string()));
                }
            }
        }
        env.push(("KANNA_BUILD_JOB".to_string(), job.id.clone()));
        let xcode = output_of("/usr/bin/xcodebuild", &["-version"], &env);
        let cache = mirror.join("derived").join(&manifest::sha256(xcode.as_bytes())[..12]);
        let results = job.dir.join("results");
        std::fs::create_dir_all(&cache)?;
        std::fs::create_dir_all(&results)?;
        let src = src.canonicalize()?;
        let results = results.canonicalize()?;
        let cache = cache.canonicalize()?;
        let argv: Vec<String> = request["argv"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
        let cwd = request["cwd"].as_str().unwrap_or_default();
        if !cwd.is_empty() {
            manifest::safe_relative(cwd)?;
        }
        let linux = request["root"].as_str().unwrap_or("/worktree");
        let translated = paths::translate(&argv, &Roots { linux, cwd, src: &src, results: &results, cache: &cache }).map_err(io::Error::other)?;
        let who = identity();
        if request["ui"].as_bool().unwrap_or(false) && self.config.require_console && (who["uid"] != who["console_uid"] || who["launchd"] != "Aqua") {
            return Err(io::Error::other("a UI job needs its user to own the guest's Aqua console session"));
        }
        let started = json!({
            "src": src, "argv": translated.argv, "map": translated.map, "results": translated.results.iter().map(|(mac, linux)| json!([mac, linux])).collect::<Vec<_>>(),
            "versions": { "xcode": xcode, "developer_dir": developer_dir, "macos": output_of("/usr/bin/sw_vers", &["-productVersion"], &env), "arch": std::env::consts::ARCH },
            "identity": who, "snapshot": request["snapshot"],
        });
        let workdir = src.join(cwd);
        let stdout = File::create(job.dir.join("out1"))?;
        let stderr = File::create(job.dir.join("out2"))?;
        let program = resolve(&translated.argv[0], &env).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{} isn't on the build Mac's PATH", translated.argv[0])))?;
        let mut child = Command::new(program).args(&translated.argv[1..]).current_dir(&workdir).env_clear().envs(env.iter().cloned()).stdin(Stdio::null()).stdout(stdout).stderr(stderr).process_group(0).spawn().map_err(|e| io::Error::new(e.kind(), format!("can't start {}: {e}", translated.argv[0])))?;
        let pid = child.id() as i32;
        job.update(|state| {
            state.phase = Phase::Running;
            state.pid = Some(pid);
            state.started = started;
        });
        let mut term_at: Option<Instant> = None;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            let cancel = job.state.lock().unwrap().cancel;
            if cancel && term_at.is_none() {
                unsafe { libc::kill(-pid, libc::SIGTERM) };
                term_at = Some(Instant::now());
            }
            if term_at.is_some_and(|at| at.elapsed() > KILL_AFTER) {
                unsafe { libc::kill(-pid, libc::SIGKILL) };
            }
            std::thread::sleep(POLL);
        };
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        if term_at.is_some() || job.state.lock().unwrap().cancel {
            return Ok(Phase::Cancelled);
        }
        Ok(match (status.code(), status.signal()) {
            (Some(code), _) => Phase::Exited(code),
            (None, Some(signal)) => Phase::Signalled(signal),
            _ => Phase::Exited(1),
        })
    }

    fn artifacts(&self, job: &Job, mirror: &Path, request: &Value) -> Vec<Value> {
        let results = job.dir.join("results");
        let started = job.state.lock().unwrap().started.clone();
        let env = self.base_env(started["versions"]["developer_dir"].as_str());
        let mut list = vec![];
        let mut bundles: Vec<(PathBuf, Option<String>)> = vec![];
        if results.join("Tests.xcresult").exists() {
            bundles.push((results.join("Tests.xcresult"), None));
        }
        for pair in started["results"].as_array().into_iter().flatten() {
            let (Some(mac), Some(linux)) = (pair[0].as_str(), pair[1].as_str()) else { continue };
            if Path::new(mac).exists() {
                if mac.ends_with(".xcresult") {
                    bundles.push((PathBuf::from(mac), Some(linux.to_string())));
                } else {
                    list.push(json!({ "path": mac, "to": linux }));
                }
            }
        }
        for (index, (bundle, to)) in bundles.iter().enumerate() {
            let name = if index == 0 { String::new() } else { format!("-{index}") };
            let summary = Command::new("/usr/bin/xcrun").args(["xcresulttool", "get", "test-results", "summary", "--path"]).arg(bundle).env_clear().envs(env.iter().cloned()).stdin(Stdio::null()).output();
            if let Some(out) = summary.ok().filter(|o| o.status.success()) {
                let file = results.join(format!("tests{name}.json"));
                if std::fs::write(&file, &out.stdout).is_ok() {
                    list.push(json!({ "path": file, "name": format!("tests{name}.json") }));
                }
            }
            let attachments = results.join(format!("attachments{name}"));
            let exported = Command::new("/usr/bin/xcrun").args(["xcresulttool", "export", "attachments", "--path"]).arg(bundle).arg("--output-path").arg(&attachments).env_clear().envs(env.iter().cloned()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
            if exported.is_ok_and(|s| s.success()) && attachments.exists() {
                list.push(json!({ "path": attachments, "name": format!("attachments{name}") }));
            }
            let entry = match to {
                Some(to) => json!({ "path": bundle, "to": to }),
                None => json!({ "path": bundle, "name": "Tests.xcresult" }),
            };
            list.push(entry);
        }
        let src = mirror.join("src");
        for rel in request["fetch"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            if manifest::safe_relative(rel).is_err() {
                continue;
            }
            let path = src.join(rel);
            let entry = match std::fs::symlink_metadata(&path) {
                Ok(_) => json!({ "path": path, "fetch": rel }),
                Err(_) => json!({ "fetch": rel, "missing": true }),
            };
            list.push(entry);
        }
        list
    }

    fn attach(&self, conn: &mut impl Write, job: &Arc<Job>, mut at: [u64; 2]) -> io::Result<()> {
        let mut announced = false;
        loop {
            let (phase, started, ready) = {
                let state = job.state.lock().unwrap();
                (state.phase.clone(), state.started.clone(), state.artifacts_ready)
            };
            if !announced && started.is_object() {
                frame::send(conn, &json!({ "op": "started", "job": job.id, "started": started }), b"")?;
                announced = true;
            }
            let mut sent = false;
            for fd in 0..2 {
                let file = job.dir.join(format!("out{}", fd + 1));
                let Ok(meta) = std::fs::metadata(&file) else { continue };
                if meta.len() > at[fd] {
                    let length = (meta.len() - at[fd]).min(1 << 20);
                    let mut reader = File::open(&file)?;
                    reader.seek(SeekFrom::Start(at[fd]))?;
                    frame::send_reader(conn, &json!({ "op": "out", "fd": fd + 1, "at": at[fd] }), length, &mut reader)?;
                    at[fd] += length;
                    sent = true;
                }
            }
            if sent {
                continue;
            }
            if phase.done() && ready {
                if !announced && started.is_object() {
                    continue;
                }
                frame::send(conn, &json!({ "op": "exit", "job": job.id, "status": job.status() }), b"")?;
                let artifacts = job.state.lock().unwrap().artifacts.clone();
                for (index, artifact) in artifacts.iter().enumerate() {
                    send_artifact(conn, index, artifact)?;
                }
                return frame::send(conn, &json!({ "op": "done", "job": job.id, "status": job.status() }), b"");
            }
            let state = job.state.lock().unwrap();
            let _ = job.changed.wait_timeout(state, POLL);
        }
    }
}

fn send_artifact(conn: &mut impl Write, index: usize, artifact: &Value) -> io::Result<()> {
    let mut header = artifact.clone();
    header["op"] = json!("artifact");
    header["index"] = json!(index);
    let Some(path) = artifact["path"].as_str().map(PathBuf::from) else {
        return frame::send(conn, &header, b"");
    };
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(_) => {
            header["missing"] = json!(true);
            return frame::send(conn, &header, b"");
        }
    };
    header["dir"] = json!(meta.is_dir());
    frame::send(conn, &header, b"")?;
    let mut walk = vec![(path.clone(), String::new())];
    while let Some((dir, prefix)) = walk.pop() {
        let entries: Vec<(PathBuf, String)> = if prefix.is_empty() && !meta.is_dir() {
            vec![(dir.clone(), path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())]
        } else {
            let mut list: Vec<_> = std::fs::read_dir(&dir)?.flatten().map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (e.path(), if prefix.is_empty() { name } else { format!("{prefix}/{name}") })
            }).collect();
            list.sort();
            list
        };
        for (file, rel) in entries {
            let meta = std::fs::symlink_metadata(&file)?;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&file)?;
                frame::send(conn, &json!({ "op": "link", "path": rel, "target": target }), b"")?;
            } else if meta.is_dir() {
                frame::send(conn, &json!({ "op": "dir", "path": rel }), b"")?;
                walk.push((file, rel));
            } else if meta.is_file() {
                let (sha, _) = manifest::hash_reader(File::open(&file)?, None)?;
                let mut reader = File::open(&file)?;
                frame::send_reader(conn, &json!({ "op": "file", "path": rel, "exec": meta.permissions().mode() & 0o111 != 0, "sha": sha }), meta.len(), &mut reader)?;
            }
        }
    }
    frame::send(conn, &json!({ "op": "end", "index": index }), b"")
}

fn offsets(value: &Value) -> [u64; 2] {
    [value[0].as_u64().unwrap_or(0), value[1].as_u64().unwrap_or(0)]
}

fn resolve(program: &str, env: &[(String, String)]) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    let path = env.iter().find(|(n, _)| n == "PATH").map(|(_, v)| v.clone()).unwrap_or_default();
    path.split(':').map(|dir| Path::new(dir).join(program)).find(|candidate| std::fs::metadata(candidate).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

pub fn identity() -> Value {
    let uid = unsafe { libc::getuid() };
    let console = std::fs::metadata("/dev/console").map(|m| m.uid()).unwrap_or(u32::MAX);
    let launchd = output_of("/bin/launchctl", &["managername"], &[]);
    json!({ "uid": uid, "console_uid": console, "launchd": launchd, "user": std::env::var("USER").unwrap_or_default() })
}

#[derive(Clone, PartialEq)]
struct Laid {
    hash: Option<String>,
    link: Option<String>,
    size: u64,
    mtime: i64,
    exec: bool,
}

fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.mtime() * 1_000_000_000 + meta.mtime_nsec()
}

fn load_laid(file: &Path) -> BTreeMap<String, Laid> {
    let Ok(bytes) = std::fs::read(file) else { return BTreeMap::new() };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else { return BTreeMap::new() };
    value.as_object().into_iter().flatten().map(|(path, v)| (path.clone(), Laid { hash: v["h"].as_str().map(String::from), link: v["l"].as_str().map(String::from), size: v["n"].as_u64().unwrap_or(0), mtime: v["m"].as_i64().unwrap_or(0), exec: v["x"].as_bool().unwrap_or(false) })).collect()
}

fn save_laid(file: &Path, laid: &BTreeMap<String, Laid>) -> io::Result<()> {
    let map: serde_json::Map<String, Value> = laid.iter().map(|(p, l)| (p.clone(), json!({ "h": l.hash, "l": l.link, "n": l.size, "m": l.mtime, "x": l.exec }))).collect();
    write_atomic(file, &serde_json::to_vec(&Value::Object(map)).unwrap_or_default())
}

fn ensure_parent(src: &Path, rel: &str) -> io::Result<()> {
    let mut dir = src.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        dir.push(part);
        match std::fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                std::fs::remove_file(&dir)?;
                std::fs::create_dir(&dir)?;
            }
            Err(_) => std::fs::create_dir(&dir)?,
        }
    }
    Ok(())
}

fn remove_any(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(_) => Ok(()),
    }
}

pub fn lay(src: &Path, store: &Path, mirror: &Path, entries: &[Entry]) -> io::Result<()> {
    let record = mirror.join("laid.json");
    let before = load_laid(&record);
    let mut after = BTreeMap::new();
    let wanted: HashSet<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    for (path, _) in before.iter().filter(|(p, _)| !wanted.contains(p.as_str())) {
        let file = src.join(path);
        remove_any(&file)?;
        let mut parent = file.parent().map(Path::to_path_buf);
        while let Some(dir) = parent.filter(|d| d.starts_with(src) && d != src) {
            if std::fs::remove_dir(&dir).is_err() {
                break;
            }
            parent = dir.parent().map(Path::to_path_buf);
        }
    }
    for entry in entries {
        let file = src.join(&entry.path);
        let current = std::fs::symlink_metadata(&file).ok();
        let previous = before.get(&entry.path);
        match &entry.kind {
            Kind::Link { target } => {
                let same = current.as_ref().is_some_and(|m| m.file_type().is_symlink()) && std::fs::read_link(&file).ok().is_some_and(|t| t.to_string_lossy() == target.as_str());
                if !same {
                    ensure_parent(src, &entry.path)?;
                    remove_any(&file)?;
                    std::os::unix::fs::symlink(target, &file)?;
                }
                after.insert(entry.path.clone(), Laid { hash: None, link: Some(target.clone()), size: 0, mtime: 0, exec: false });
            }
            Kind::File { hash, size, exec } => {
                let unchanged = match (&current, previous) {
                    (Some(meta), Some(laid)) => meta.is_file() && laid.hash.as_deref() == Some(hash.as_str()) && laid.size == meta.len() && laid.mtime == mtime_of(meta) && laid.exec == *exec,
                    _ => false,
                };
                if unchanged {
                    after.insert(entry.path.clone(), previous.cloned().unwrap_or(Laid { hash: Some(hash.clone()), link: None, size: *size, mtime: 0, exec: *exec }));
                    continue;
                }
                ensure_parent(src, &entry.path)?;
                if current.as_ref().is_some_and(|m| !m.is_file()) {
                    remove_any(&file)?;
                }
                let temp = file.with_file_name(format!(".kanna-lay-{}", std::process::id()));
                std::fs::copy(store.join(hash), &temp)?;
                std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(if *exec { 0o755 } else { 0o644 }))?;
                std::fs::rename(&temp, &file)?;
                let meta = std::fs::metadata(&file)?;
                after.insert(entry.path.clone(), Laid { hash: Some(hash.clone()), link: None, size: meta.len(), mtime: mtime_of(&meta), exec: *exec });
            }
        }
    }
    save_laid(&record, &after)
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanna-lay-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(store: &Path, text: &str) -> String {
        let hash = manifest::sha256(text.as_bytes());
        std::fs::write(store.join(&hash), text).unwrap();
        hash
    }

    fn file(path: &str, hash: &str, size: usize) -> Entry {
        Entry { path: path.into(), kind: Kind::File { hash: hash.into(), size: size as u64, exec: false } }
    }

    #[test]
    fn laying_a_snapshot_keeps_unchanged_files_and_removes_deleted_ones() {
        let root = temp("mirror");
        let (src, store) = (root.join("src"), root.join("store"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&store).unwrap();
        let (a, b) = (put(&store, "a1"), put(&store, "b1"));
        lay(&src, &store, &root, &[file("a.txt", &a, 2), file("dir/b.txt", &b, 2)]).unwrap();
        let first = std::fs::metadata(src.join("a.txt")).unwrap().modified().unwrap();
        std::fs::write(src.join("generated.txt"), "made by the build").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let b2 = put(&store, "b2");
        lay(&src, &store, &root, &[file("a.txt", &a, 2), file("dir/c.txt", &b2, 2), Entry { path: "l".into(), kind: Kind::Link { target: "a.txt".into() } }]).unwrap();
        assert_eq!(std::fs::metadata(src.join("a.txt")).unwrap().modified().unwrap(), first, "an unchanged file keeps its time, so builds stay warm");
        assert!(!src.join("dir/b.txt").exists());
        assert_eq!(std::fs::read_to_string(src.join("dir/c.txt")).unwrap(), "b2");
        assert_eq!(std::fs::read_link(src.join("l")).unwrap(), PathBuf::from("a.txt"));
        assert!(src.join("generated.txt").exists(), "files the build made are left alone");
        std::fs::write(src.join("a.txt"), "edited on the Mac").unwrap();
        lay(&src, &store, &root, &[file("a.txt", &a, 2)]).unwrap();
        assert_eq!(std::fs::read_to_string(src.join("a.txt")).unwrap(), "a1", "a file changed on the Mac is put back");
        assert!(!src.join("dir").exists());
    }
}
