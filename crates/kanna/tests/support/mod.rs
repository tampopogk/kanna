#![allow(dead_code)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const KANNA: &str = env!("CARGO_BIN_EXE_kanna");

fn sibling(name: &str, package: &str) -> PathBuf {
    let binary = Path::new(KANNA).with_file_name(name);
    if !binary.exists() {
        let status = Command::new(env!("CARGO")).args(["build", "-p", package]).status().unwrap();
        assert!(status.success(), "building {package} failed");
    }
    binary
}

pub struct Daemon {
    pub dir: PathBuf,
    child: Child,
}

impl Daemon {
    pub fn start() -> Daemon {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("kcli{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let dir = dir.canonicalize().unwrap();
        let daemon = sibling("kanna-daemon", "kanna-daemon");
        let keeper = sibling("kanna-keeper", "kanna-keeper");
        let log = std::fs::File::create(dir.join("daemon.log")).unwrap();
        let mut command = Command::new(daemon);
        command.env_clear();
        for key in ["PATH", "USER", "LOGNAME", "TMPDIR"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let child = command
            .env("HOME", dir.join("home"))
            .env("SHELL", "/bin/sh")
            .env("KANNA_SOCKET", dir.join("kanna.sock"))
            .env("KANNA_STATE_DIR", dir.join("state"))
            .env("KANNA_KEEPER", keeper)
            .env("KANNA_AGENT_RULES_URL", "none")
            .env("KANNA_KEEPER_CLOSE_GRACE_MS", "300")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let daemon = Daemon { dir, child };
        wait_for("the daemon's socket", || UnixStream::connect(daemon.socket()).is_ok());
        daemon
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("kanna.sock")
    }

    pub fn kanna(&self, args: &[&str]) -> Run {
        let output = cli(&self.dir).env("KANNA_SOCKET", self.socket()).args(args).output().unwrap();
        Run::from(args, output)
    }

    pub fn kanna_as(&self, pane: &str, args: &[&str]) -> Run {
        let output = cli(&self.dir).env("KANNA_SOCKET", self.socket()).env("KANNA_TERMINAL", pane).args(args).output().unwrap();
        Run::from(args, output)
    }

    pub fn app(&self) -> App {
        App::attach(&self.socket())
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Ok(pid) = std::fs::read_to_string(self.dir.join("state/keeper.pid")) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                if pid > 0 {
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn cli(dir: &Path) -> Command {
    let mut command = Command::new(KANNA);
    command
        .current_dir(dir)
        .env("HOME", dir.join("home"))
        .env("KANNA_APP", "/nonexistent/Kanna.app")
        .env("KMUX_APP", "/nonexistent/kmux.app")
        .env_remove("KANNA_SOCKET")
        .env_remove("KANNA_INSTANCE")
        .env_remove("KANNA_PANE")
        .env_remove("KANNA_MUX")
        .env_remove("KANNA_BG")
        .env_remove("KANNA_FRESH")
        .env_remove("KMUX_SOCKET");
    command
}

pub struct Run {
    pub args: String,
    pub code: Option<i32>,
    pub out: String,
    pub err: String,
}

impl Run {
    pub fn from(args: &[&str], output: Output) -> Run {
        Run {
            args: args.join(" "),
            code: output.status.code(),
            out: String::from_utf8_lossy(&output.stdout).into_owned(),
            err: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    #[track_caller]
    pub fn ok(self) -> String {
        assert_eq!(self.code, Some(0), "kanna {}: {}", self.args, self.err);
        self.out.trim_end().to_string()
    }

    #[track_caller]
    pub fn json(self) -> Value {
        let text = self.ok();
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
    }

    #[track_caller]
    pub fn fails(self, code: i32, says: &str) -> String {
        assert_eq!(self.code, Some(code), "kanna {}: {}{}", self.args, self.out, self.err);
        assert!(self.err.contains(says), "kanna {}: {}", self.args, self.err);
        self.err
    }
}

pub struct App {
    pub thread: std::thread::JoinHandle<()>,
    stream: UnixStream,
    pub diffs: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
}

impl App {
    pub fn attach(socket: &Path) -> App {
        let stream = UnixStream::connect(socket).unwrap();
        let mut writer = stream.try_clone().unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        writeln!(writer, "{}", json!({"id": 0, "cmd": "app.hello", "args": {"pid": std::process::id(), "panes": []}})).unwrap();
        let (ready, hello) = std::sync::mpsc::channel();
        let diffs = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let heard = diffs.clone();
        let thread = std::thread::spawn(move || {
            let mut owner = kanna_mux::LayoutOwner::new();
            let mut ready = Some(ready);
            for line in reader.lines().map_while(Result::ok) {
                let message: Value = serde_json::from_str(&line).unwrap();
                if message["id"] == 0 && message.get("ok").is_some() {
                    owner.handle(&json!({"call": "attach", "panes": message["panes"]}));
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(message["ok"] == true);
                    }
                    continue;
                }
                if message["cmd"] == "diff" {
                    heard.lock().unwrap().push(message["args"].clone());
                } else if message.get("content").is_some() || (message.get("ok").is_some() && message["id"] != 0) {
                    heard.lock().unwrap().push(message.clone());
                }
                if message["cmd"] == "start" || message["cmd"] == "pane" {
                    owner.handle(&json!({"call": "pane", "pane": message["args"]["pane"]}));
                }
                if message["cmd"] == "start" {
                    let pane = message["args"]["pane"]["id"].clone();
                    let _ = writeln!(writer, "{}", json!({"cmd": "update", "args": {"pane": pane, "state": "running"}}));
                }
                if message["cmd"] == "layout" {
                    let mut reply = owner.handle(&json!({"call": "serve", "request": message["args"]}));
                    reply["id"] = message["id"].clone();
                    let _ = writeln!(writer, "{reply}");
                }
                if message["cmd"].as_str().is_some_and(|c| c.starts_with("debug.") || c == "screenshot") {
                    let reply = json!({"id": message["id"], "ok": true, "cmd": message["cmd"], "args": message["args"]});
                    let _ = writeln!(writer, "{reply}");
                }
            }
        });
        assert_eq!(hello.recv_timeout(Duration::from_secs(10)), Ok(true), "the app wasn't accepted");
        App { thread, stream, diffs }
    }

    pub fn send(&self, message: Value) {
        writeln!(&self.stream, "{message}").unwrap();
    }

    pub fn quit(self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        let _ = self.thread.join();
    }
}

#[track_caller]
pub fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn serve(path: &Path, answer: fn(&Value) -> Value) -> std::sync::mpsc::Receiver<Value> {
    let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
    let (seen, requests) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut writer = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let request: Value = serde_json::from_str(&line).unwrap();
                let mut reply = answer(&request);
                reply["id"] = request["id"].clone();
                let _ = seen.send(request);
                let _ = writeln!(writer, "{reply}");
            }
        }
    });
    requests
}
