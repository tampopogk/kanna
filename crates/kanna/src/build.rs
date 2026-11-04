use kanna_client::{exit, fail, Failure};
use kanna_guest::client::{self, Pipe, Request};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const SHIMS: &[&str] = &["xcodebuild", "xcrun", "codesign", "swift"];

pub const HELP: &str = "kanna build [--mac NAME] [--xcode VERSION] [--arch ARCH] [--macos VERSION] [--runtime VERSION] [--fetch PATH]... [--ui] [--] COMMAND [ARG]...

Runs COMMAND on a build Mac, inside a guest, against this worktree as it is now: modified, untracked and deleted files
included. Output and the exit code come back as if it ran here; diagnostics are repeated on stderr with this worktree's
paths, and results land in .kanna/build/JOB/ (test summaries, attachments, the job's record).

  --mac NAME      the build Mac to use (default: build.mac in ~/.kanna-v4/config.json, your only Mac, or the
                  Mac among several with a matching guest and the shortest queue)
  --xcode V       a guest with this Xcode, e.g. 26 or 26.1 (KANNA_BUILD_XCODE through the shim)
  --arch ARCH     a guest of this architecture, arm64 or x86_64 (KANNA_BUILD_ARCH)
  --macos V       a guest on this macOS (KANNA_BUILD_MACOS)
  --runtime V     a guest with this iOS Simulator runtime (KANNA_BUILD_RUNTIME)
  --fetch PATH    bring PATH (relative to the worktree) back from the build Mac's copy, e.g. the built .app
  --ui            the job needs the guest's console session (UI tests)

Jobs wait their turn in the build Mac's queue; `kanna builds` shows it and `kanna builds cancel JOB` takes one out.
xcodebuild, xcrun, codesign and swift reach it through the shim on a Linux task's PATH. To run a whole script there:
kanna build -- scripts/build.sh";

pub const BUILDS_HELP: &str = "kanna builds [--json]
kanna builds cancel JOB

Your build Macs' queues, in the order their jobs run, and each Mac's last finished jobs. cancel takes a queued job out
of its queue, or stops a running one.";

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn interrupted(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

pub fn shim_name(argv0: &str) -> Option<&'static str> {
    let name = Path::new(argv0).file_name()?.to_str()?;
    SHIMS.iter().copied().find(|s| *s == name)
}

pub struct Options {
    pub mac: Option<String>,
    pub fetch: Vec<String>,
    pub ui: bool,
    pub argv: Vec<String>,
    pub needs: BTreeMap<String, String>,
}

fn need_value(key: &str, value: String) -> String {
    if key == "arch" && value == "arm64" { "aarch64".into() } else { value }
}

pub fn needs_from_env() -> BTreeMap<String, String> {
    [("KANNA_BUILD_XCODE", "xcode"), ("KANNA_BUILD_ARCH", "arch"), ("KANNA_BUILD_MACOS", "macos"), ("KANNA_BUILD_RUNTIME", "runtime")]
        .into_iter().filter_map(|(name, key)| Some((key.to_string(), need_value(key, std::env::var(name).ok().filter(|v| !v.is_empty())?)))).collect()
}

pub fn parse(args: &[String]) -> Result<Options, Failure> {
    let mut options = Options { mac: None, fetch: vec![], ui: false, argv: vec![], needs: needs_from_env() };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--" => {
                options.argv = rest.cloned().collect();
                break;
            }
            "--mac" => options.mac = Some(rest.next().cloned().ok_or_else(|| fail(exit::USAGE, "build: --mac needs a NAME"))?),
            "--fetch" => options.fetch.push(rest.next().cloned().ok_or_else(|| fail(exit::USAGE, "build: --fetch needs a PATH"))?),
            "--ui" => options.ui = true,
            "--xcode" | "--arch" | "--macos" | "--runtime" => {
                let key = arg.trim_start_matches('-');
                let value = rest.next().cloned().ok_or_else(|| fail(exit::USAGE, format!("build: {arg} needs a value")))?;
                options.needs.insert(key.to_string(), need_value(key, value));
            }
            other if other.starts_with("--") => return Err(fail(exit::USAGE, format!("build: unknown option \"{other}\"\n\n{HELP}"))),
            _ => {
                options.argv = std::iter::once(arg.clone()).chain(rest.cloned()).collect();
                break;
            }
        }
    }
    if options.argv.is_empty() {
        return Err(fail(exit::USAGE, format!("build: missing COMMAND\n\n{HELP}")));
    }
    Ok(options)
}

struct Opening<'a> {
    socket: &'a Path,
    mac: Option<&'a str>,
    needs: &'a Value,
    repo: &'a str,
    task: &'a str,
    chosen: &'a std::sync::OnceLock<String>,
}

fn open(opening: &Opening) -> io::Result<Box<dyn Pipe>> {
    let socket = opening.socket;
    let mac = opening.chosen.get().map(String::as_str).or(opening.mac);
    let mut stream = UnixStream::connect(socket).map_err(|e| io::Error::new(e.kind(), format!("can't reach Kanna at {}: {e}", socket.display())))?;
    let request = json!({ "id": 1, "cmd": "build.open", "args": { "mac": mac, "needs": opening.needs, "repo": opening.repo, "task": opening.task } });
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while byte[0] != b'\n' {
        if stream.read(&mut byte)? == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Kanna closed the connection"));
        }
        line.push(byte[0]);
        if line.len() > 64 << 10 {
            return Err(io::Error::other("Kanna's answer is too long"));
        }
    }
    let ack: Value = serde_json::from_slice(&line).map_err(|e| io::Error::other(e.to_string()))?;
    if ack["ok"] != true {
        let message = ack["error"]["message"].as_str().unwrap_or("Kanna refused the build");
        return Err(io::Error::new(io::ErrorKind::ConnectionRefused, message.to_string()));
    }
    if let Some(mac) = ack["mac"].as_str() {
        let _ = opening.chosen.set(mac.to_string());
    }
    Ok(Box::new(stream))
}

fn call(socket: &Path, cmd: &str, args: Value) -> Option<Value> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).ok()?;
    stream.write_all(format!("{}\n", json!({ "id": 1, "cmd": cmd, "args": args })).as_bytes()).ok()?;
    let mut line = String::new();
    io::BufRead::read_line(&mut io::BufReader::new(stream), &mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn label(socket: &Path, root: &Path) -> String {
    let folder = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let name = call(socket, "task", json!({ "action": "list" })).and_then(|reply| {
        reply["tasks"].as_array()?.iter().find(|t| t["task"] == folder.as_str())?["name"].as_str().map(String::from)
    });
    name.unwrap_or_else(|| {
        let branch = std::process::Command::new("git").arg("-C").arg(root).args(["rev-parse", "--abbrev-ref", "HEAD"]).output();
        branch.ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|b| !b.is_empty() && b != "HEAD").unwrap_or(folder)
    })
}

pub fn run(socket: PathBuf, options: Options) -> Result<i32, Failure> {
    let here = std::env::current_dir().map_err(|e| fail(exit::FAILED, format!("build: no current folder: {e}")))?;
    let worktree = client::worktree(&here).map_err(|e| fail(exit::FAILED, format!("build: {e}")))?;
    let mut env = BTreeMap::new();
    for name in ["KANNA_CHANNEL", "CI", "RUST_BACKTRACE", "CARGO_TERM_COLOR", "NSUnbufferedIO", "DEVELOPER_DIR"] {
        if let Ok(value) = std::env::var(name) {
            env.insert(name.to_string(), value);
        }
    }
    let needs = if options.needs.is_empty() { Value::Null } else { json!(options.needs) };
    let request = Request { root: worktree.root.clone(), cwd: worktree.cwd.clone(), argv: options.argv, env, fetch: options.fetch, ui: options.ui, job: client::new_job(), needs: needs.clone(), label: label(&socket, &worktree.root) };
    let chosen = std::sync::OnceLock::new();
    let opening = Opening { socket: &socket, mac: options.mac.as_deref(), needs: &needs, repo: &worktree.repo, task: &worktree.task, chosen: &chosen };
    let connect = || open(&opening);
    unsafe {
        libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, interrupted as *const () as libc::sighandler_t);
    }
    let job = request.job.clone();
    let outcome = std::thread::scope(|scope| {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let connect = &connect;
        scope.spawn(move || client::watch_cancel(connect, &job, &INTERRUPTED, &done_rx));
        let (stdout, stderr) = (io::stdout(), io::stderr());
        let outcome = client::run(connect, &request, &worktree, &mut stdout.lock(), &mut stderr.lock());
        let _ = done_tx.send(());
        outcome
    });
    Ok(outcome.code())
}

fn age(from: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let seconds = now.saturating_sub(from) / 1000;
    if seconds < 60 { format!("{seconds}s") } else if seconds < 3600 { format!("{}m {}s", seconds / 60, seconds % 60) } else { format!("{}h {}m", seconds / 3600, seconds / 60 % 60) }
}

fn result(status: &Value) -> String {
    match status["phase"].as_str() {
        Some("exited") if status["code"] == 0 => "passed".into(),
        Some("exited") => format!("exit {}", status["code"]),
        Some("signalled") => format!("signal {}", status["signal"]),
        Some("cancelled") => "cancelled".into(),
        _ => format!("failed: {}", status["error"].as_str().unwrap_or("unknown")),
    }
}

pub fn render(reply: &Value) -> String {
    let macs = reply["macs"].as_array().cloned().unwrap_or_default();
    if macs.is_empty() {
        return "no build Macs: pair one (kanna machine pair) and give it a guest (kanna guests create)".into();
    }
    let mut out = String::new();
    for mac in &macs {
        let name = mac["mac"].as_str().unwrap_or("?");
        if let Some(why) = mac["unreachable"].as_str() {
            out += &format!("{name}: unreachable ({why})\n");
            continue;
        }
        let guests: Vec<String> = mac["guests"].as_array().into_iter().flatten().map(|g| format!("{} {} Xcode {}", g["name"].as_str().unwrap_or("?"), g["arch"].as_str().unwrap_or("?"), g["xcode"].as_str().unwrap_or("?"))).collect();
        if guests.is_empty() && mac["jobs"].as_array().is_none_or(Vec::is_empty) {
            continue;
        }
        out += &format!("{name}  {}\n", guests.join(", "));
        for job in mac["jobs"].as_array().into_iter().flatten() {
            let id = job["job"].as_str().unwrap_or("?");
            let label = job["label"].as_str().unwrap_or("?");
            let from = job["machine"].as_str().unwrap_or("?");
            let (mark, state) = match job["stage"].as_str() {
                Some("queued") => ((job["ahead"].as_u64().unwrap_or(0) + 1).to_string(), format!("queued {}", age(job["submitted"].as_u64().unwrap_or(0)))),
                stage => ("▶".to_string(), format!("{} {} in {}", stage.unwrap_or("running"), age(job["started"].as_u64().or(job["submitted"].as_u64()).unwrap_or(0)), job["guest"].as_str().unwrap_or("?"))),
            };
            out += &format!("  {mark} {label}  {from}  {state}  {id}\n");
        }
        for job in mac["recent"].as_array().into_iter().flatten().take(5) {
            out += &format!("  · {}  {}  {}  {} ago  {}\n", job["label"].as_str().unwrap_or("?"), job["machine"].as_str().unwrap_or("?"), result(&job["status"]), age(job["finished"].as_u64().unwrap_or(0)), job["job"].as_str().unwrap_or("?"));
        }
    }
    if out.is_empty() { "no build Macs with guests".into() } else { out.trim_end().into() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_reads_in_run_order_with_finished_jobs_after() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let reply = json!({ "macs": [
            { "mac": "studio", "guests": [{ "name": "vm-a", "arch": "aarch64", "xcode": "26.1" }], "jobs": [
                { "job": "b-1", "label": "Login screen", "machine": "linux", "stage": "running", "started": now, "guest": "vm-a" },
                { "job": "b-2", "label": "Settings page", "machine": "r720", "stage": "queued", "submitted": now, "ahead": 0 },
            ], "recent": [{ "job": "b-0", "label": "Widgets", "machine": "linux", "status": { "phase": "exited", "code": 65 }, "finished": now }] },
            { "mac": "mini", "unreachable": "mini didn't answer in time" },
        ] });
        let text = render(&reply);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "studio  vm-a aarch64 Xcode 26.1");
        assert!(lines[1].contains("▶ Login screen  linux  running 0s in vm-a  b-1"), "{text}");
        assert!(lines[2].contains("1 Settings page  r720  queued 0s  b-2"), "{text}");
        assert!(lines[3].contains("· Widgets  linux  exit 65"), "{text}");
        assert!(lines[4].contains("mini: unreachable"), "{text}");
        assert!(render(&json!({ "macs": [] })).starts_with("no build Macs"));
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_come_before_the_command_and_everything_after_is_its() {
        let options = parse(&strings(&["--mac", "mini", "--fetch", "a.app", "--", "xcodebuild", "--fetch", "-scheme"])).unwrap();
        assert_eq!((options.mac.as_deref(), options.fetch, options.argv), (Some("mini"), strings(&["a.app"]), strings(&["xcodebuild", "--fetch", "-scheme"])));
        let options = parse(&strings(&["--ui", "xcodebuild", "test"])).unwrap();
        assert!(options.ui);
        assert_eq!(options.argv, strings(&["xcodebuild", "test"]));
        assert!(parse(&strings(&["--mac", "mini"])).is_err());
        let options = parse(&strings(&["--xcode", "26.x", "--arch", "arm64", "swift", "test"])).unwrap();
        assert_eq!(options.needs.get("xcode").map(String::as_str), Some("26.x"));
        assert_eq!(options.needs.get("arch").map(String::as_str), Some("aarch64"));
        assert_eq!(shim_name("/opt/kanna/current/shim/xcodebuild"), Some("xcodebuild"));
        assert_eq!(shim_name("kanna"), None);
    }
}
