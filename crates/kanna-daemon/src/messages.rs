use kanna_detect::Status;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const PLANNER: &str = "planner";

pub const USER: &str = "user";

pub const PICKUP: Duration = Duration::from_secs(5);

pub const POST_TIMEOUT: Duration = Duration::from_secs(2);

pub const TURN_TIMEOUT: Duration = Duration::from_secs(15);

pub const AGENTS: [&str; 2] = ["claude", "codex"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Channel {
    Claude(Result<PathBuf, String>),
    Codex(Result<PathBuf, String>),
    Other(String),
}

impl Channel {
    pub fn socket(&self) -> Option<Result<&Path, &str>> {
        match self {
            Channel::Claude(socket) | Channel::Codex(socket) => Some(socket.as_deref().map_err(String::as_str)),
            Channel::Other(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ready {
    status: Option<Status>,
    since: Instant,
    reported: bool,
}

impl Ready {
    pub fn new(now: Instant) -> Ready {
        Ready { status: None, since: now, reported: false }
    }

    pub fn see(&mut self, status: Option<Status>, ended: bool, now: Instant) {
        if status != self.status {
            self.status = status;
            self.since = now;
        }
        self.reported = ended && status == Some(Status::Idle);
    }

    pub fn sent(&mut self, now: Instant) {
        self.since = now;
        self.reported = false;
    }

    pub fn at(&self, settle: Duration) -> Option<Instant> {
        (self.status == Some(Status::Idle)).then(|| if self.reported { self.since } else { self.since + settle })
    }
}

pub fn prompt(messages: &[(String, String)]) -> String {
    messages.iter().map(|(from, text)| format!("[message from {from}] {}", text.trim())).collect::<Vec<_>>().join("\n\n")
}

pub const RETRY: Duration = Duration::from_secs(5);

pub fn inbox(home: &Path, pid: i32) -> Result<PathBuf, String> {
    let entry = home.join(".claude").join("sessions").join(format!("{pid}.json"));
    let text = std::fs::read_to_string(&entry).map_err(|_| format!("Claude (pid {pid}) has no messaging socket: no {}", entry.display()))?;
    let session: Value = serde_json::from_str(&text).map_err(|error| format!("Claude's {} isn't JSON: {error}", entry.display()))?;
    if session["pid"].as_i64() != Some(pid.into()) {
        return Err(format!("Claude's {} is another process's", entry.display()));
    }
    if session["peerProtocol"].as_i64() != Some(1) {
        return Err(format!("Claude (pid {pid}) speaks messaging protocol {}, and Kanna speaks 1", session["peerProtocol"]));
    }
    session["messagingSocketPath"].as_str().map(PathBuf::from).filter(|socket| socket.is_absolute()).ok_or_else(|| format!("Claude's {} names no messaging socket", entry.display()))
}

pub fn codex_socket(argv: &[Vec<String>]) -> Result<PathBuf, String> {
    argv.iter()
        .find_map(|args| {
            let at = args.iter().position(|a| a == "--remote")?;
            let codex = args[..at].iter().any(|a| Path::new(a).file_name().is_some_and(|n| n == "codex"));
            codex.then(|| args.get(at + 1)?.strip_prefix("unix://").map(PathBuf::from)).flatten()
        })
        .ok_or_else(|| "its codex isn't attached to an app-server (codex --remote unix://SOCKET)".into())
}

pub fn deliver(channel: &Channel, text: &str) -> Result<(), String> {
    match channel {
        Channel::Claude(Ok(socket)) => post(socket, text),
        Channel::Codex(Ok(socket)) => codex_turn(socket, text),
        Channel::Claude(Err(why)) | Channel::Codex(Err(why)) => Err(why.clone()),
        Channel::Other(agent) => Err(format!("{agent} can't take messages")),
    }
}

pub fn codex_turn(socket: &Path, text: &str) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("{}: {error}", socket.display());
    let mut stream = UnixStream::connect(socket).map_err(failed)?;
    stream.set_read_timeout(Some(TURN_TIMEOUT)).map_err(failed)?;
    stream.set_write_timeout(Some(TURN_TIMEOUT)).map_err(failed)?;
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: a2FubmEta2FubmEta2FubmE=\r\nSec-WebSocket-Version: 13\r\n\r\n").map_err(failed)?;
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).map_err(failed)?;
        head.push(byte[0]);
    }
    if !head.starts_with(b"HTTP/1.1 101") {
        return Err(format!("codex's app-server refused the connection: {}", String::from_utf8_lossy(&head).lines().next().unwrap_or("")));
    }
    let mut rpc = Rpc { stream, next: 0 };
    rpc.call("initialize", json!({ "clientInfo": { "name": "kanna", "version": env!("CARGO_PKG_VERSION") } }))?;
    rpc.send(&json!({ "jsonrpc": "2.0", "method": "initialized" })).map_err(failed)?;
    let loaded = rpc.call("thread/loaded/list", json!({}))?;
    let mut thread = None;
    for id in loaded["data"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let read = rpc.call("thread/read", json!({ "threadId": id, "includeTurns": false }))?;
        if read["thread"]["ephemeral"].as_bool() != Some(true) {
            thread = Some(id.to_string());
            break;
        }
    }
    let thread = thread.ok_or("codex's app-server has no session loaded")?;
    rpc.call("turn/start", json!({ "threadId": thread, "input": [{ "type": "text", "text": text }] })).map(|_| ())
}

struct Rpc {
    stream: UnixStream,
    next: u64,
}

impl Rpc {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).map_err(|error| format!("codex's app-server: {error}"))?;
        loop {
            let reply: Value = serde_json::from_str(&self.receive().map_err(|error| format!("codex's app-server, waiting on {method}: {error}"))?).unwrap_or(Value::Null);
            if reply["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                return Err(format!("codex's app-server refused {method}: {}", error["message"].as_str().unwrap_or("no reason given")));
            }
            return Ok(reply["result"].clone());
        }
    }

    fn send(&mut self, message: &Value) -> std::io::Result<()> {
        self.frame(0x1, message.to_string().as_bytes())
    }

    fn frame(&mut self, opcode: u8, data: &[u8]) -> std::io::Result<()> {
        let mask = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos()) | 1).to_be_bytes();
        let mut frame = vec![0x80 | opcode];
        match data.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n < 1 << 16 => {
                frame.push(0x80 | 126);
                frame.extend((n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend((n as u64).to_be_bytes());
            }
        }
        frame.extend(mask);
        frame.extend(data.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&frame)
    }

    fn receive(&mut self) -> std::io::Result<String> {
        let mut message = Vec::new();
        loop {
            let mut head = [0u8; 2];
            self.stream.read_exact(&mut head)?;
            let len = match head[1] & 0x7f {
                126 => {
                    let mut n = [0u8; 2];
                    self.stream.read_exact(&mut n)?;
                    u16::from_be_bytes(n) as usize
                }
                127 => {
                    let mut n = [0u8; 8];
                    self.stream.read_exact(&mut n)?;
                    u64::from_be_bytes(n) as usize
                }
                n => n as usize,
            };
            let mut mask = [0u8; 4];
            if head[1] & 0x80 != 0 {
                self.stream.read_exact(&mut mask)?;
            }
            let mut data = vec![0u8; len];
            self.stream.read_exact(&mut data)?;
            if head[1] & 0x80 != 0 {
                data.iter_mut().enumerate().for_each(|(i, b)| *b ^= mask[i % 4]);
            }
            match head[0] & 0x0f {
                0x8 => return Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, "it closed the connection")),
                0x9 => self.frame(0xa, &data)?,
                0x0 | 0x1 | 0x2 => {
                    message.extend(data);
                    if head[0] & 0x80 != 0 {
                        return Ok(String::from_utf8_lossy(&message).into_owned());
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn post(socket: &Path, text: &str) -> Result<(), String> {
    let mut stream = UnixStream::connect(socket).map_err(|error| format!("{}: {error}", socket.display()))?;
    stream.set_write_timeout(Some(POST_TIMEOUT)).map_err(|error| error.to_string())?;
    let line = json!({ "type": "user", "message": { "role": "user", "content": text } });
    stream.write_all(format!("{line}\n").as_bytes()).map_err(|error| format!("{}: {error}", socket.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_waits_for_the_agent_to_stay_idle_or_to_report_its_turn_over() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let settle = Duration::from_millis(3000);
        let mut ready = Ready::new(t0);
        assert_eq!(ready.at(settle), None, "no agent yet");
        ready.see(Some(Status::Working), false, at(0));
        assert_eq!(ready.at(settle), None, "busy");
        ready.see(Some(Status::Idle), false, at(1000));
        assert_eq!(ready.at(settle), Some(at(4000)), "idle, held 3s");
        ready.see(Some(Status::Idle), false, at(2000));
        assert_eq!(ready.at(settle), Some(at(4000)), "the same idle");
        ready.see(Some(Status::NeedsYou), false, at(2500));
        assert_eq!(ready.at(settle), None, "never while it asks");
        ready.see(Some(Status::Working), false, at(2600));
        ready.see(Some(Status::Idle), true, at(5000));
        assert_eq!(ready.at(settle), Some(at(5000)), "the agent said its turn is over");
        ready.sent(at(5100));
        assert_eq!(ready.at(settle), Some(at(8100)), "after a delivery, it settles again");
    }

    #[test]
    fn messages_go_in_one_prompt_oldest_first_each_with_its_sender() {
        let one = prompt(&[("task-5e2c \"Send\"".into(), "Is your spec final?\n".into()), ("user".into(), "Also rebase.".into())]);
        assert_eq!(one, "[message from task-5e2c \"Send\"] Is your spec final?\n\n[message from user] Also rebase.");
    }

    #[test]
    fn a_codex_attached_to_an_app_server_names_its_socket() {
        let argv = |line: &str| line.split(' ').map(String::from).collect::<Vec<_>>();
        let found = codex_socket(&[argv("/bin/zsh -c x"), argv("/opt/bin/codex app-server --listen unix:///s/a.sock"), argv("/opt/bin/codex resume 019f --remote unix:///s/a.sock")]);
        assert_eq!(found, Ok(PathBuf::from("/s/a.sock")));
        assert!(codex_socket(&[argv("codex --yolo --no-daemon")]).is_err(), "a codex in its terminal has no door");
        assert!(codex_socket(&[argv("claude --remote unix:///s/b.sock")]).is_err(), "only codex's own --remote");
        assert_eq!(codex_socket(&[argv("/usr/bin/node /n/bin/codex --remote unix:///s/c.sock")]), Ok(PathBuf::from("/s/c.sock")), "a codex run by node");
    }

    #[test]
    fn a_message_to_codex_starts_a_turn_on_its_session_not_on_the_title_thread() {
        let socket = Path::new("/tmp").join(format!("kanna-codex-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut byte = [0u8; 1];
                stream.read_exact(&mut byte).unwrap();
                head.push(byte[0]);
            }
            stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n").unwrap();
            let mut rpc = Rpc { stream, next: 0 };
            let mut started = Value::Null;
            while let Ok(text) = rpc.receive() {
                let call: Value = serde_json::from_str(&text).unwrap();
                let result = match call["method"].as_str().unwrap() {
                    "initialized" => continue,
                    "thread/loaded/list" => json!({ "data": ["title", "session"] }),
                    "thread/read" => json!({ "thread": { "id": call["params"]["threadId"], "ephemeral": call["params"]["threadId"] == "title" } }),
                    "turn/start" => {
                        started = call["params"].clone();
                        json!({ "turn": { "id": "t1" } })
                    }
                    _ => json!({}),
                };
                rpc.send(&json!({ "jsonrpc": "2.0", "id": call["id"], "result": result })).unwrap();
            }
            started
        });
        codex_turn(&socket, "[message from user] Hi").unwrap();
        assert_eq!(server.join().unwrap(), json!({ "threadId": "session", "input": [{ "type": "text", "text": "[message from user] Hi" }] }));
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn a_claude_session_takes_messages_on_the_socket_its_registry_entry_names() {
        use std::io::BufRead;
        let home = std::env::temp_dir().join(format!("kanna-inbox-{}", std::process::id()));
        let sessions = home.join(".claude").join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let socket = Path::new("/tmp").join(format!("kanna-inbox-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let entry = |pid: i32, protocol: i64| json!({ "pid": pid, "peerProtocol": protocol, "messagingSocketPath": socket }).to_string();
        std::fs::write(sessions.join("41.json"), entry(41, 1)).unwrap();
        std::fs::write(sessions.join("42.json"), entry(7, 1)).unwrap();
        std::fs::write(sessions.join("43.json"), entry(43, 2)).unwrap();
        assert_eq!(inbox(&home, 41), Ok(socket.clone()));
        assert!(inbox(&home, 42).unwrap_err().ends_with("is another process's"));
        assert_eq!(inbox(&home, 43), Err("Claude (pid 43) speaks messaging protocol 2, and Kanna speaks 1".into()));
        assert!(inbox(&home, 44).unwrap_err().starts_with("Claude (pid 44) has no messaging socket: no "));
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        post(&socket, "[message from user] Hi\nthere").unwrap();
        let mut line = String::new();
        std::io::BufReader::new(listener.accept().unwrap().0).read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), json!({ "type": "user", "message": { "role": "user", "content": "[message from user] Hi\nthere" } }));
        drop(listener);
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_dir_all(&home);
        assert!(post(&socket, "gone").is_err(), "the session went away");
    }
}
