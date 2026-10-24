#![allow(dead_code)]

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const DAEMON: &str = env!("CARGO_BIN_EXE_kanna-daemon");

fn keeper_binary() -> PathBuf {
    let keeper = Path::new(DAEMON).with_file_name("kanna-keeper");
    if !keeper.exists() {
        let status = Command::new(env!("CARGO")).args(["build", "-p", "kanna-keeper"]).status().unwrap();
        assert!(status.success(), "building kanna-keeper failed");
    }
    keeper
}

pub struct Instance {
    pub dir: PathBuf,
    pub daemon: Child,
    env: Vec<(String, String)>,
}

impl Instance {
    pub fn start() -> Instance {
        Instance::start_with(&[])
    }

    pub fn start_with(env: &[(&str, &str)]) -> Instance {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("kd{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home")).unwrap();
        let dir = dir.canonicalize().unwrap();
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let daemon = spawn_daemon(&dir, &env);
        let instance = Instance { daemon, dir, env };
        wait_for("the daemon's socket", || UnixStream::connect(instance.socket()).is_ok());
        instance
    }

    pub fn stop(&mut self, signal: i32) {
        unsafe { libc::kill(self.pid(), signal) };
        let _ = self.daemon.wait();
    }

    pub fn start_again(&mut self) {
        self.daemon = spawn_daemon(&self.dir, &self.env);
        wait_for("the daemon's socket", || UnixStream::connect(self.socket()).is_ok());
    }

    pub fn restart(&mut self, signal: i32) {
        self.stop(signal);
        self.start_again();
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("kanna.sock")
    }

    pub fn keeper_pid(&self) -> i32 {
        std::fs::read_to_string(self.dir.join("state/keeper.pid")).unwrap().trim().parse().unwrap()
    }

    pub fn pid(&self) -> i32 {
        self.daemon.id() as i32
    }

    pub fn call(&self, cmd: &str, args: Value) -> Value {
        call(&self.socket(), cmd, args)
    }

    pub fn open(&self, name: &str, cmd: Option<&str>) -> String {
        let mut args = json!({"type": "term", "name": name});
        if let Some(cmd) = cmd {
            args["cmd"] = json!(cmd);
        }
        let opened = self.call("open", args);
        opened["pane"]["terminal"].as_str().unwrap_or_else(|| panic!("no terminal in {opened}")).to_string()
    }

    pub fn pane(&self, name: &str) -> Value {
        let list = self.call("list", json!({}));
        list["panes"].as_array().unwrap().iter().find(|p| p["name"] == name).cloned().unwrap_or(Value::Null)
    }

    pub fn screen(&self, pane: &str) -> String {
        self.call("read", json!({"pane": pane}))["text"].as_str().unwrap_or("").to_string()
    }

    pub fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default()
    }

    pub fn rss_mb(&self) -> u64 {
        let out = Command::new("ps").args(["-o", "rss=", "-p", &self.pid().to_string()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0) / 1024
    }

    pub fn ptys_open(&self) -> usize {
        let out = Command::new("lsof").args(["-n", "-P", "-p", &self.pid().to_string()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.contains("ptmx") || l.contains("/dev/ptmx")).count()
    }

    pub fn app(&self) -> App {
        App::connect(&self.socket())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
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

fn spawn_daemon(dir: &Path, env: &[(String, String)]) -> Child {
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("daemon.log")).unwrap();
    let mut command = Command::new(DAEMON);
    command.env_clear();
    for key in ["PATH", "USER", "LOGNAME", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("HOME", dir.join("home"))
        .env("SHELL", "/bin/sh")
        .env("KANNA_SOCKET", dir.join("kanna.sock"))
        .env("KANNA_STATE_DIR", dir.join("state"))
        .env("KANNA_AGENT_RULES_URL", "none")
        .env("KANNA_KEEPER", keeper_binary())
        .env("KANNA_KEEPER_CLOSE_GRACE_MS", "300")
            .env("KANNA_FLEET_MDNS", "0")
            .env("KANNA_FLEET_PORT", "0")
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    command.spawn().unwrap()
}

pub struct App {
    pub stream: UnixStream,
    reader: BufReader<UnixStream>,
    pub view: Value,
    pub shows: bool,
}

impl App {
    pub fn connect(socket: &Path) -> App {
        App::connect_with(socket, json!([]))
    }

    pub fn connect_with(socket: &Path, panes: Value) -> App {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut app = App::raw(socket);
            app.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": std::process::id(), "panes": panes}}));
            let hello = app.next().expect("no answer to app.hello");
            if hello["ok"] == true {
                return app;
            }
            assert!(hello["error"]["code"] == "app_attached" && Instant::now() < deadline, "{hello}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn raw(socket: &Path) -> App {
        let stream = UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let reader = BufReader::new(stream.try_clone().unwrap());
        App { stream, reader, view: Value::Null, shows: true }
    }

    pub fn answering(self) -> std::thread::JoinHandle<()> {
        self.stream.set_read_timeout(None).unwrap();
        let mut app = self;
        std::thread::spawn(move || while app.next().is_some() {})
    }

    pub fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.stream.write_all(line.as_bytes()).unwrap();
    }

    pub fn tell(&mut self, cmd: &str, args: Value) {
        self.send(json!({"cmd": cmd, "args": args}));
    }

    pub fn next(&mut self) -> Option<Value> {
        loop {
            let mut line = String::new();
            let message: Value = match self.reader.read_line(&mut line) {
                Ok(0) | Err(_) => return None,
                Ok(_) => serde_json::from_str(&line).unwrap(),
            };
            if message["id"] == 0 && message["ok"] == true {
                self.view = message["layout"].clone();
            }
            if message["cmd"] == "layout" {
                self.view = message["args"].clone();
            }
            if message["cmd"] == "start" && self.shows {
                let pane = message["args"]["pane"]["id"].clone();
                self.tell("update", json!({"pane": pane, "state": "running"}));
            }
            if message["cmd"] == "app.snap" {
                let line = message["args"]["line"].as_u64().unwrap_or(1);
                let last = message["args"]["last"].as_u64().unwrap_or(line + 2);
                self.send(json!({"id": message["id"], "ok": true, "first": line, "last": last}));
                continue;
            }
            return Some(message);
        }
    }

    #[track_caller]
    pub fn until(&mut self, what: &str, mut wanted: impl FnMut(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            let message = self.next().unwrap_or_else(|| panic!("the daemon closed the link waiting for {what}"));
            if wanted(&message) {
                return message;
            }
        }
    }
}

pub fn call(socket: &Path, cmd: &str, args: Value) -> Value {
    request(socket, json!({"id": 1, "cmd": cmd, "args": args}))
}

pub fn request(socket: &Path, request: Value) -> Value {
    let mut stream = UnixStream::connect(socket).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut line = request.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes()).unwrap();
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).unwrap();
    serde_json::from_str(&reply).unwrap()
}

pub fn is(message: &Value, cmd: &str, pane: &str) -> bool {
    message["cmd"] == cmd && message["args"]["terminal"] == pane
}

pub fn data(message: &Value) -> Vec<u8> {
    BASE64.decode(message["args"]["data"].as_str().unwrap_or("")).unwrap()
}

#[track_caller]
pub fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn alive(pid: i64) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}
