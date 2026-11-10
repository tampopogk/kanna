use crate::target::{process_env, Choice, Env, Target};
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
    let name = call(socket, "tasks", json!({ "action": "list", "all": true })).and_then(|reply| {
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
    for name in ["KANNA_CHANNEL", "KANNA_VERSION", "CI", "RUST_BACKTRACE", "CARGO_TERM_COLOR", "NSUnbufferedIO", "DEVELOPER_DIR"] {
        if let Ok(value) = std::env::var(name) {
            env.insert(name.to_string(), value);
        }
    }
    if !env.contains_key("KANNA_VERSION") {
        if let Some(version) = kanna_build::tags::dev_version(&worktree.root) {
            env.insert("KANNA_VERSION".to_string(), version);
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

pub fn socket(env: Env) -> Result<PathBuf, Failure> {
    let target = Target::resolve(&kanna_build::Build::current(), Choice { mux: None, instance: None, background: false, fresh: false }, env)?;
    Ok(target.socket)
}

fn swift_goes_to_the_mac() -> bool {
    let Ok(here) = std::env::current_dir() else { return false };
    let Ok(worktree) = kanna_guest::client::worktree(&here) else { return false };
    let config = std::fs::read(worktree.root.join(".kanna/config.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).unwrap_or(Value::Null);
    config["build"]["swift"] == "mac"
}

pub fn shim(tool: &str, argv0: &str, args: Vec<String>) -> Result<i32, Failure> {
    if tool == "swift" && !swift_goes_to_the_mac() {
        use std::os::unix::process::CommandExt;
        let me = std::fs::canonicalize(std::env::current_exe().unwrap_or_default()).unwrap_or_default();
        let shim_dir = std::path::Path::new(argv0).parent().and_then(|d| d.canonicalize().ok());
        let path = std::env::var("PATH").unwrap_or_default();
        let next = path.split(':').map(|dir| std::path::Path::new(dir).join(tool)).find(|candidate| candidate.is_file() && candidate.parent().and_then(|d| d.canonicalize().ok()) != shim_dir && std::fs::canonicalize(candidate).ok().as_ref() != Some(&me));
        let Some(next) = next else { return Err(fail(exit::FAILED, "swift: there's no swift on this machine, and this repo doesn't send swift to the build Mac (build.swift in .kanna/config.json)")) };
        let error = std::process::Command::new(next).args(args).exec();
        return Err(fail(exit::FAILED, format!("swift: {error}")));
    }
    let argv = std::iter::once(tool.to_string()).chain(args).collect();
    run(socket(&process_env)?, Options { mac: None, fetch: fetch_policy(), ui: std::env::var("KANNA_BUILD_UI").is_ok_and(|v| v == "1"), argv, needs: needs_from_env() })
}

fn fetch_policy() -> Vec<String> {
    std::env::var("KANNA_BUILD_FETCH").map(|v| v.split(':').filter(|p| !p.is_empty()).map(String::from).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
