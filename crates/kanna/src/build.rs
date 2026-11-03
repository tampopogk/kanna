use kanna_client::{exit, fail, Failure};
use kanna_guest::client::{self, Pipe, Request};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const SHIMS: &[&str] = &["xcodebuild", "xcrun", "codesign", "swift"];

pub const HELP: &str = "kanna build [--mac NAME] [--fetch PATH]... [--ui] [--] COMMAND [ARG]...

Runs COMMAND on a build Mac, inside a guest, against this worktree as it is now: modified, untracked and deleted files
included. Output and the exit code come back as if it ran here; diagnostics are repeated on stderr with this worktree's
paths, and results land in .kanna/build/JOB/ (test summaries, attachments, the job's record).

  --mac NAME      the build Mac to use (default: build.mac in ~/.kanna-v4/config.json, or your only Mac)
  --fetch PATH    bring PATH (relative to the worktree) back from the build Mac's copy, e.g. the built .app
  --ui            the job needs the guest's console session (UI tests)

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
}

pub fn parse(args: &[String]) -> Result<Options, Failure> {
    let mut options = Options { mac: None, fetch: vec![], ui: false, argv: vec![] };
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

fn open(socket: &Path, mac: Option<&str>) -> io::Result<Box<dyn Pipe>> {
    let mut stream = UnixStream::connect(socket).map_err(|e| io::Error::new(e.kind(), format!("can't reach Kanna at {}: {e}", socket.display())))?;
    let request = json!({ "id": 1, "cmd": "build.open", "args": { "mac": mac } });
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
    Ok(Box::new(stream))
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
    let request = Request { root: worktree.root.clone(), cwd: worktree.cwd.clone(), argv: options.argv, env, fetch: options.fetch, ui: options.ui, job: client::new_job() };
    let mac = options.mac.clone();
    let connect = move || open(&socket, mac.as_deref());
    unsafe {
        libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, interrupted as *const () as libc::sighandler_t);
    }
    let job = request.job.clone();
    let outcome = std::thread::scope(|scope| {
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let connect = &connect;
        scope.spawn(move || {
            while done_rx.recv_timeout(Duration::from_millis(200)).is_err() {
                if INTERRUPTED.swap(false, Ordering::SeqCst) {
                    let _ = client::cancel(connect, &job);
                }
            }
        });
        let (stdout, stderr) = (io::stdout(), io::stderr());
        let outcome = client::run(connect, &request, &worktree, &mut stdout.lock(), &mut stderr.lock());
        let _ = done_tx.send(());
        outcome
    });
    Ok(outcome.code())
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
        assert_eq!(shim_name("/opt/kanna/current/shim/xcodebuild"), Some("xcodebuild"));
        assert_eq!(shim_name("kanna"), None);
    }
}
