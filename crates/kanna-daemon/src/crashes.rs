use crate::db::Db;
use kanna_build::{Build, Channel};
use kanna_crash::Crash;
use kanna_mux::json::{json, string, Map, Value};
use kanna_mux::{CoreHandle, KmuxError, Responder};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime};

pub const WINDOW: Duration = Duration::from_secs(600);

pub const DELAYS: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(10), Duration::from_secs(60)];

const STILL_RUNNING: Duration = Duration::from_secs(30);

const IPS_WAIT: Duration = Duration::from_secs(5);

const POLL: Duration = Duration::from_millis(250);

const FRAMES: usize = 15;

#[derive(Debug, PartialEq)]
pub enum Next {
    Relaunch(Duration),
    GiveUp,
}

pub fn next(crashes: usize) -> Next {
    match crashes {
        0 => Next::Relaunch(DELAYS[0]),
        n if n <= DELAYS.len() => Next::Relaunch(DELAYS[n - 1]),
        _ => Next::GiveUp,
    }
}

enum Watch {
    Idle,
    Attached { pid: i32, relaunch: Value, goodbye: bool },
    Gone { pid: i32, relaunch: Value, since: Instant, dead: Option<Instant> },
    Looking,
    Relaunching { at: Instant, relaunch: Value },
}

pub struct Crashes {
    db: Rc<Db>,
    state_dir: PathBuf,
    build: Build,
    watch: RefCell<Watch>,
    history: RefCell<Vec<Instant>>,
    stopping: Cell<bool>,
    banners: Cell<u64>,
    pub post: RefCell<Option<Box<dyn Fn(&str, &str) -> Result<i64, String>>>>,
    pub show: RefCell<Option<Box<dyn Fn(&Value) -> bool>>>,
}

fn alive(pid: i32) -> bool {
    pid > 0 && (unsafe { libc::kill(pid, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)) && !zombie(pid)
}

#[cfg(target_os = "macos")]
fn zombie(pid: i32) -> bool {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size) };
    got == size && info.pbi_status == libc::SZOMB
}

#[cfg(not(target_os = "macos"))]
fn zombie(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| rest.starts_with('Z'))).unwrap_or(false)
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn written(path: &Path) -> SystemTime {
    std::fs::metadata(path).and_then(|m| m.modified()).unwrap_or_else(|_| SystemTime::now())
}

fn frame_line(index: usize, frame: &Value) -> String {
    let image = string(frame, "image").unwrap_or("?");
    let symbol = string(frame, "symbol").unwrap_or("?");
    match (frame.get("offset").and_then(Value::as_u64), string(frame, "at")) {
        (Some(offset), _) => format!("{index}  {image}  {symbol} + {offset}"),
        (None, Some(at)) => format!("{index}  {image}  {symbol} ({at})"),
        (None, None) => format!("{index}  {image}  {symbol}"),
    }
}

fn local_time(at: SystemTime) -> String {
    let seconds = at.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

pub fn summary(record: &Value) -> String {
    format!("{}: {}", string(record, "process").unwrap_or("?"), string(record, "reason").unwrap_or("?"))
}

impl Crashes {
    pub fn new(db: Rc<Db>, state_dir: &Path, build: Build) -> Crashes {
        Crashes {
            db,
            state_dir: state_dir.to_path_buf(),
            build,
            watch: RefCell::new(Watch::Idle),
            history: RefCell::default(),
            stopping: Cell::new(false),
            banners: Cell::new(0),
            post: RefCell::default(),
            show: RefCell::default(),
        }
    }

    pub fn reports(&self) -> bool {
        matches!(self.build.channel, Channel::Dev | Channel::Staging)
    }

    pub fn scan(&self) {
        let known = self.db.crashes().unwrap_or_default();
        let mut reported: Vec<String> = known.iter().filter(|row| row.3).map(|row| row.1.clone()).collect();
        for (path, record) in kanna_crash::records(&self.state_dir) {
            let file = file_name(&path);
            if known.iter().any(|row| row.0 == file) {
                continue;
            }
            let signature = string(&record, "signature").unwrap_or_default().to_string();
            log!("kanna-daemon: found the crash record {} ({}, signature {signature})", path.display(), summary(&record));
            if let Err(error) = self.db.put_crash(&file, &signature) {
                log!("kanna-daemon: can't keep the crash record {file} in kanna.db: {error}");
                continue;
            }
            if !self.reports() || reported.contains(&signature) {
                continue;
            }
            if self.report(&path, &record) {
                reported.push(signature.clone());
                if let Err(error) = self.db.crash_reported(&signature) {
                    log!("kanna-daemon: can't note that crash {signature} was reported: {error}");
                }
            }
        }
    }

    fn report(&self, path: &Path, record: &Value) -> bool {
        let repo = kanna_mux::project(self.build.checkout.to_str()).unwrap_or_else(|| self.build.checkout.clone());
        let repo = repo.to_string_lossy().into_owned();
        let build = &record["build"];
        let ips = kanna_crash::ips_for(record, written(path)).map_or_else(|| "none found yet (kanna crashes looks again)".to_string(), |p| p.display().to_string());
        let stack: Vec<Value> = record["stack"].as_array().cloned().unwrap_or_default();
        let frames: Vec<String> = kanna_crash::site(&stack).iter().take(FRAMES).enumerate().map(|(i, f)| format!("  {}", frame_line(i, f))).collect();
        let text = format!(
            "Kanna crashed: {} (pid {}): {}\nbuild: {} {} (build ID {})\nrecord: {}\nreport: {ips}\nsignature: {} (the first crash of this signature; later ones aren't sent)\nstack:\n{}",
            string(record, "process").unwrap_or("?"),
            record["pid"],
            string(record, "reason").unwrap_or("?"),
            string(build, "channel").unwrap_or("?"),
            string(build, "commit").unwrap_or("?"),
            string(build, "id").unwrap_or("?"),
            path.display(),
            string(record, "signature").unwrap_or("?"),
            frames.join("\n")
        );
        match self.post.borrow().as_ref().map(|post| post(&repo, &text)) {
            Some(Ok(_)) => {
                log!("kanna-daemon: told the planner of {repo} about crash {}", string(record, "signature").unwrap_or("?"));
                true
            }
            Some(Err(error)) => {
                log!("kanna-daemon: can't tell the planner about crash {}: {error}", string(record, "signature").unwrap_or("?"));
                false
            }
            None => false,
        }
    }

    pub fn banner(&self) -> Option<Value> {
        let rows = self.db.crashes().unwrap_or_default();
        let waiting: Vec<&(String, String, bool, bool)> = rows.iter().filter(|row| !row.2).collect();
        let newest = waiting.last()?;
        let path = kanna_crash::folder(&self.state_dir).join(&newest.0);
        let record: Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
        let process = match string(&record, "process") {
            Some("app") => "Kanna.app",
            Some("daemon") => "the daemon",
            Some("keeper") => "the keeper",
            _ => "Kanna",
        };
        let reason = string(&record, "reason").unwrap_or("?");
        let mut detail = format!("{reason} · {process} at {}", local_time(written(&path)));
        if waiting.len() > 1 {
            detail.push_str(&format!(" · and {} more", waiting.len() - 1));
        }
        self.banners.set(self.banners.get() + 1);
        let title: String = format!("Kanna crashed: {reason}").chars().take(160).collect();
        Some(json!({ "id": format!("crash-{}", self.banners.get()), "title": title, "detail": detail, "pane": null, "task": null, "about": null, "sticky": true, "record": path }))
    }

    pub fn show_banner(&self) {
        if let Some(banner) = self.banner() {
            if let Some(show) = self.show.borrow().as_ref() {
                show(&banner);
            }
        }
    }

    fn dismiss(&self, file: Option<&str>) -> Result<usize, String> {
        let through = match file {
            Some(file) => file_name(Path::new(file)),
            None => self.db.crashes().map_err(|e| e.to_string())?.last().map(|row| row.0.clone()).unwrap_or_default(),
        };
        self.db.dismiss_crashes(&through).map_err(|e| e.to_string())
    }

    fn list(&self, all: bool) -> Value {
        self.scan();
        let rows = self.db.crashes().unwrap_or_default();
        let mut out: Vec<Value> = kanna_crash::records(&self.state_dir)
            .into_iter()
            .filter_map(|(path, mut record)| {
                let file = file_name(&path);
                let dismissed = rows.iter().any(|row| row.0 == file && row.2);
                if dismissed && !all {
                    return None;
                }
                record["file"] = json!(path);
                record["ips"] = json!(kanna_crash::ips_for(&record, written(&path)));
                record["dismissed"] = json!(dismissed);
                Some(record)
            })
            .collect();
        out.reverse();
        Value::Array(out)
    }

    pub fn install(self: &Rc<Self>, core: &CoreHandle) {
        let this: Weak<Crashes> = Rc::downgrade(self);
        core.add_command(
            "crashes",
            Rc::new(move |args, responder: Responder| {
                let Some(this) = this.upgrade() else { return responder.reply(Err(KmuxError::new("app_gone", "the daemon is stopping"))) };
                responder.reply(match string(&args, "action").unwrap_or("list") {
                    "list" => Ok(Map::from_iter([("crashes".to_string(), this.list(args.get("all").and_then(Value::as_bool) == Some(true)))])),
                    "dismiss" => this.dismiss(string(&args, "file")).map(|n| Map::from_iter([("dismissed".to_string(), json!(n))])).map_err(|e| KmuxError::new("failed", e)),
                    other => Err(KmuxError::new("bad_request", format!("crashes takes list or dismiss, not {other}"))),
                })
            }),
        );
        if std::env::var("KANNA_DEBUG").as_deref() == Ok("1") {
            core.add_command(
                "debug.panic",
                Rc::new(move |args, responder: Responder| {
                    if args.get("task").and_then(Value::as_bool) != Some(true) {
                        panic!("debug.panic in the daemon's main loop");
                    }
                    responder.reply(Ok(Map::new()));
                    crate::runtime::spawn("debug.panic", async {
                        tokio::task::yield_now().await;
                        panic!("debug.panic in a task on the daemon's main loop");
                    });
                }),
            );
        }
    }

    pub fn stopping(&self) {
        self.stopping.set(true);
    }

    pub fn attached(&self, hello: &Value) {
        let args = &hello["args"];
        let pid = args["pid"].as_i64().unwrap_or(0) as i32;
        *self.watch.borrow_mut() = Watch::Attached { pid, relaunch: args["relaunch"].clone(), goodbye: false };
        self.scan();
        self.show_banner();
    }

    pub fn goodbye(&self) {
        if let Watch::Attached { goodbye, .. } = &mut *self.watch.borrow_mut() {
            *goodbye = true;
        }
    }

    pub fn gone(&self) {
        let watch = std::mem::replace(&mut *self.watch.borrow_mut(), Watch::Idle);
        match watch {
            Watch::Attached { goodbye: true, .. } => log!("kanna-daemon: the app quit"),
            Watch::Attached { pid, relaunch, goodbye: false } if !self.stopping.get() => {
                log!("kanna-daemon: the app (pid {pid}) went away without saying goodbye; watching for its crash");
                *self.watch.borrow_mut() = Watch::Gone { pid, relaunch, since: Instant::now(), dead: None };
            }
            _ => {}
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        match &*self.watch.borrow() {
            Watch::Gone { .. } | Watch::Looking => Some(Instant::now() + POLL),
            Watch::Relaunching { at, .. } => Some(*at),
            _ => None,
        }
    }

    fn record_of(state_dir: &Path, pid: i32) -> Option<(PathBuf, Value)> {
        kanna_crash::records(state_dir).into_iter().rev().find(|(_, r)| r["pid"].as_i64() == Some(pid as i64) && string(r, "process") == Some("app"))
    }

    fn from_ips(state_dir: &Path, pid: i32, relaunch: &Value) -> Option<(PathBuf, Value)> {
        let executable = string(relaunch, "path").map(Path::new).and_then(Path::file_name)?.to_string_lossy().into_owned();
        let ips = kanna_crash::find_ips(&executable, pid as u64, SystemTime::now())?;
        let body = kanna_crash::ips_body(&ips)?;
        let crash = Crash {
            process: "app".into(),
            executable,
            pid: pid as u32,
            kind: "signal".into(),
            reason: kanna_crash::ips_reason(&body),
            stack: kanna_crash::ips_stack(&body),
            build: kanna_crash::build(),
            panes: vec![],
        };
        match kanna_crash::write(state_dir, &crash) {
            Ok(path) => {
                log!("kanna-daemon: the app (pid {pid}) left no crash record; wrote {} from {}", path.display(), ips.display());
                let record = kanna_crash::records(state_dir).into_iter().find(|(p, _)| *p == path)?.1;
                Some((path, record))
            }
            Err(error) => {
                log!("kanna-daemon: can't write a crash record from {}: {error}", ips.display());
                None
            }
        }
    }

    fn look_for_record(self: &Rc<Self>, pid: i32, relaunch: Value, since: Instant, dead_at: Instant) {
        *self.watch.borrow_mut() = Watch::Looking;
        let (this, state_dir, wanted) = (Rc::downgrade(self), self.state_dir.clone(), relaunch.clone());
        crate::runtime::spawn("the app's crash record", async move {
            let found = crate::runtime::off_loop(move || Self::record_of(&state_dir, pid).or_else(|| Self::from_ips(&state_dir, pid, &wanted))).await.flatten();
            let Some(this) = this.upgrade() else { return };
            if !matches!(*this.watch.borrow(), Watch::Looking) || this.stopping.get() {
                return;
            }
            let now = Instant::now();
            let next = match found {
                Some((path, record)) => this.crashed(now, &path, &record, relaunch),
                None if now.duration_since(dead_at) < IPS_WAIT => Watch::Gone { pid, relaunch, since, dead: Some(dead_at) },
                None => {
                    log!("kanna-daemon: the app (pid {pid}) is gone with no crash record and no .ips: it was killed, so it isn't relaunched");
                    Watch::Idle
                }
            };
            *this.watch.borrow_mut() = next;
        });
    }

    pub fn tick(self: &Rc<Self>, now: Instant, app_attached: bool) {
        if self.stopping.get() {
            *self.watch.borrow_mut() = Watch::Idle;
            return;
        }
        let watch = std::mem::replace(&mut *self.watch.borrow_mut(), Watch::Idle);
        let next = match watch {
            Watch::Gone { .. } | Watch::Looking | Watch::Relaunching { .. } if app_attached => Watch::Idle,
            Watch::Gone { pid, relaunch, since, dead } => {
                if alive(pid) {
                    if now.duration_since(since) >= STILL_RUNNING {
                        log!("kanna-daemon: the app (pid {pid}) lost its link but is still running; not watching it any more");
                        Watch::Idle
                    } else {
                        Watch::Gone { pid, relaunch, since, dead }
                    }
                } else {
                    return self.look_for_record(pid, relaunch, since, dead.unwrap_or(now));
                }
            }
            Watch::Relaunching { at, relaunch } if now >= at => {
                match self.relaunch(&relaunch) {
                    Ok(pid) => log!("kanna-daemon: relaunched the app (pid {pid}); its output goes to {}", self.state_dir.join("app.log").display()),
                    Err(error) => log!("kanna-daemon: can't relaunch the app: {error}"),
                }
                Watch::Idle
            }
            other => other,
        };
        *self.watch.borrow_mut() = next;
    }

    fn crashed(&self, now: Instant, path: &Path, record: &Value, relaunch: Value) -> Watch {
        self.scan();
        let mut history = self.history.borrow_mut();
        history.retain(|at| now.duration_since(*at) < WINDOW);
        history.push(now);
        let what = format!("{} ({})", string(record, "reason").unwrap_or("?"), path.display());
        if relaunch.get("path").and_then(Value::as_str).is_none() {
            log!("kanna-daemon: the app crashed: {what}; it didn't say how to relaunch it");
            return Watch::Idle;
        }
        match next(history.len()) {
            Next::Relaunch(delay) => {
                log!("kanna-daemon: the app crashed: {what}; relaunching it in {}s", delay.as_secs());
                Watch::Relaunching { at: now + delay, relaunch }
            }
            Next::GiveUp => {
                log!("kanna-daemon: the app crashed {} times in {} minutes; not relaunching it (last: {what})", history.len(), WINDOW.as_secs() / 60);
                Watch::Idle
            }
        }
    }

    fn relaunch(&self, relaunch: &Value) -> Result<u32, String> {
        use std::os::unix::process::CommandExt;
        let path = string(relaunch, "path").ok_or("no executable")?;
        let log = std::fs::OpenOptions::new().create(true).append(true).open(self.state_dir.join("app.log")).map_err(|e| format!("can't open app.log: {e}"))?;
        let mut command = std::process::Command::new(path);
        command.args(relaunch["args"].as_array().into_iter().flatten().filter_map(Value::as_str));
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy();
            if key.starts_with("KANNA_") || key.starts_with("KMUX_") {
                command.env_remove(key.as_ref());
            }
        }
        if let Some(env) = relaunch["env"].as_object() {
            for (key, value) in env {
                if let Some(value) = value.as_str() {
                    command.env(key, value);
                }
            }
        }
        command.stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|e| format!("{path}: {e}"))?;
        let pid = child.id();
        let _ = std::thread::Builder::new().name("kanna.app.relaunched".into()).spawn(move || child.wait());
        Ok(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_crashes_back_off_then_stop() {
        assert_eq!(next(1), Next::Relaunch(Duration::from_secs(1)));
        assert_eq!(next(2), Next::Relaunch(Duration::from_secs(10)));
        assert_eq!(next(3), Next::Relaunch(Duration::from_secs(60)));
        assert_eq!(next(4), Next::GiveUp);
        assert_eq!(next(9), Next::GiveUp);
    }

    #[test]
    fn a_frame_reads_as_its_image_symbol_and_offset_or_line() {
        assert_eq!(frame_line(0, &json!({ "image": "Kanna", "symbol": "f", "offset": 4 })), "0  Kanna  f + 4");
        assert_eq!(frame_line(1, &json!({ "image": "kanna-daemon", "symbol": "g", "at": "src/main.rs:3:1" })), "1  kanna-daemon  g (src/main.rs:3:1)");
    }
}
