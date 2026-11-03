use crate::db::{self, Db, Sample};
use crate::stats::{day_start, local_day, main_ref, DAYS};
use crate::tasks::{coverage_settings, git, CoverageSettings};
use crate::Event;
use kanna_mux::json::{json, Value};
use kanna_quality::coverage::{changed, parse, totals};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

pub const WORKTREE: &str = ".kanna-worktrees/kanna-coverage";

pub struct Covered {
    pub repo: String,
    pub commit: String,
    pub result: Result<Sample, String>,
}

struct Running {
    commit: String,
    started: f64,
}

pub struct Coverage {
    db: Rc<Db>,
    events: Sender<Event>,
    state: PathBuf,
    running: RefCell<HashMap<String, Running>>,
    tried: RefCell<HashMap<String, (f64, Option<String>)>>,
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn clock(at: f64) -> String {
    let seconds = at as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(8)]
}

impl Coverage {
    pub fn new(db: Rc<Db>, events: Sender<Event>, state: PathBuf) -> Coverage {
        Coverage { db, events, state, running: RefCell::default(), tried: RefCell::default() }
    }

    pub fn status(&self, repo: &str) -> Option<String> {
        if let Some(run) = self.running.borrow().get(repo) {
            return Some(format!("running at {} since {}", short(&run.commit), clock(run.started)));
        }
        self.tried.borrow().get(repo).and_then(|(_, failed)| failed.clone())
    }

    pub fn consider(&self, repo: &str) {
        let Some(settings) = coverage_settings(&crate::agents::home(), Path::new(repo)) else { return };
        if self.running.borrow().contains_key(repo) {
            return;
        }
        let Ok(head) = git(Path::new(repo), &["rev-parse", &main_ref(Path::new(repo))]) else { return };
        let last = self.db.latest_sample(repo, "coverage").ok().flatten();
        if last.as_ref().is_some_and(|s| s.commit == head) {
            return;
        }
        let ran = last.as_ref().and_then(|s| s.doc["ran"]["at"].as_f64()).unwrap_or(0.0).max(self.tried.borrow().get(repo).map_or(0.0, |t| t.0));
        if now() - ran < settings.every as f64 * 3600.0 {
            return;
        }
        self.start(repo, &head, settings);
    }

    fn start(&self, repo: &str, commit: &str, settings: CoverageSettings) {
        let started = now();
        self.running.borrow_mut().insert(repo.to_string(), Running { commit: commit.to_string(), started });
        self.tried.borrow_mut().insert(repo.to_string(), (started, None));
        let (events, repo, commit) = (self.events.clone(), repo.to_string(), commit.to_string());
        let folder = self.state.join("coverage").join(format!("{:016x}", folder_hash(&repo)));
        std::thread::spawn(move || {
            let result = run(Path::new(&repo), &commit, &settings, &folder, started);
            let _ = events.send(Event::Covered(Covered { repo, commit, result }));
        });
    }

    pub fn covered(&self, covered: Covered) {
        let Covered { repo, commit, result } = covered;
        self.running.borrow_mut().remove(&repo);
        match result {
            Ok(sample) => {
                if let Err(error) = self.db.put_samples(&repo, &[sample]) {
                    log!("kanna-daemon: couldn't save {repo}'s coverage at {} in {}: {error}", short(&commit), db::FILE);
                }
            }
            Err(error) => {
                log!("kanna-daemon: coverage of {repo} at {} failed: {error}", short(&commit));
                if let Some(tried) = self.tried.borrow_mut().get_mut(&repo) {
                    tried.1 = Some(format!("last run, at {}, failed: {error}", short(&commit)));
                }
            }
        }
    }
}

fn folder_hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn worktree(repo: &Path, commit: &str) -> Result<PathBuf, String> {
    let tree = repo.join(WORKTREE);
    if tree.join(".git").exists() {
        git(&tree, &["checkout", "-q", "--detach", "--force", commit])?;
        git(&tree, &["reset", "-q", "--hard", commit])?;
    } else {
        crate::tasks::exclude_worktrees(repo)?;
        let _ = git(repo, &["worktree", "prune"]);
        git(repo, &["worktree", "add", "-q", "--detach", &tree.to_string_lossy(), commit])?;
    }
    Ok(tree)
}

pub fn run(repo: &Path, commit: &str, settings: &CoverageSettings, folder: &Path, started: f64) -> Result<Sample, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("can't make {}: {e}", folder.display()))?;
    let tree = worktree(repo, commit)?;
    let lcov = folder.join("coverage.lcov");
    let _ = std::fs::remove_file(&lcov);
    let log = std::fs::File::create(folder.join("coverage.log")).map_err(|e| format!("can't write the log: {e}"))?;
    let mut child = Command::new("/bin/sh")
        .args(["-c", &settings.command])
        .current_dir(&tree)
        .env("PWD", &tree)
        .env("KANNA_REPO", repo)
        .env("KANNA_COVERAGE", &lcov)
        .env_remove("KANNA_SOCKET")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .map_err(|e| format!("can't run {}: {e}", settings.command))?;
    let deadline = Instant::now() + Duration::from_secs(settings.timeout);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() > deadline => {
                unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
                let _ = child.wait();
                return Err(format!("it ran past {} s; its log is {}", settings.timeout, folder.join("coverage.log").display()));
            }
            Ok(None) => std::thread::sleep(Duration::from_secs(1)),
            Err(error) => return Err(error.to_string()),
        }
    };
    let text = std::fs::read_to_string(&lcov).map_err(|_| format!("it exited {} without writing $KANNA_COVERAGE; its log is {}", status.code().unwrap_or(-1), folder.join("coverage.log").display()))?;
    let files = parse(&text, &tree);
    if files.is_empty() {
        return Err(format!("its LCOV covers no Rust or Swift file of the repo; its log is {}", folder.join("coverage.log").display()));
    }
    Ok(sample(repo, commit, &files, started))
}

pub fn sample(repo: &Path, commit: &str, files: &std::collections::BTreeMap<String, kanna_quality::coverage::FileCoverage>, started: f64) -> Sample {
    let since = day_start(local_day(now()) - DAYS + 1);
    let (lines, branches) = totals(files);
    let landed = changed(repo, commit, files, since);
    let at = git(repo, &["show", "-s", "--format=%ct", commit]).ok().and_then(|t| t.trim().parse().ok()).unwrap_or(now());
    let times: HashMap<String, f64> = git(repo, &["log", "--first-parent", "--format=%H %ct", &format!("--since=@{}", since as i64), commit])
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once(' ').and_then(|(c, t)| Some((c.to_string(), t.parse().ok()?))))
        .collect();
    let file_rows: Vec<Value> = files
        .iter()
        .map(|(file, coverage)| {
            let changed = landed.files.get(file).copied().unwrap_or_default();
            json!({ "file": file, "hit": coverage.lines.values().filter(|c| **c > 0).count(), "found": coverage.lines.len(), "changedHit": changed.hit, "changedFound": changed.found })
        })
        .collect();
    let landings: Vec<Value> = landed.landings.iter().map(|(landing, count)| json!({ "commit": landing, "at": times.get(landing).copied(), "hit": count.hit, "found": count.found })).collect();
    let value = if lines.found == 0 { 0.0 } else { (lines.hit as f64 * 1000.0 / lines.found as f64).round() / 10.0 };
    Sample {
        commit: commit.to_string(),
        metric: "coverage".to_string(),
        at,
        value,
        doc: json!({ "lines": { "hit": lines.hit, "found": lines.found }, "branches": { "hit": branches.hit, "found": branches.found }, "files": file_rows, "landings": landings, "ran": { "at": started, "seconds": (now() - started).round() } }),
        revision: 0,
    }
}
