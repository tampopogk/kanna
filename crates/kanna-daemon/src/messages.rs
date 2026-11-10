use kanna_detect::Status;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const PLANNER: &str = "planner";

pub const USER: &str = "user";

pub const SCHEDULE: &str = "schedule ";

pub const RUN: &str = "run-";

pub const QUEUE: &str = "queue-";

pub const DESK: &str = "desk";

pub const PHONE: &str = "phone";

pub const KANNA: &str = "kanna";

pub fn yours(sender: &str) -> bool {
    sender == USER || sender == DESK || sender == PHONE || sender == KANNA || sender.starts_with(SCHEDULE) || sender.starts_with(RUN) || sender.starts_with(QUEUE)
}

pub const PICKUP: Duration = Duration::from_secs(5);

pub const POST_TIMEOUT: Duration = Duration::from_secs(2);

pub const TURN_TIMEOUT: Duration = Duration::from_secs(15);
pub const CHECK: Duration = Duration::from_secs(2);
const QUEUE_PAGE: u32 = 200;
const REFUSED: &str = "codex's app-server refused";

pub const AGENTS: [&str; 2] = ["claude", "codex"];

pub const BYPASS: &str = "bypass";

pub const PROMPTING: &str = "prompting";

const UNPROMPTED: &[&str] = &["--dangerously-skip-permissions", "--yolo", "--dangerously-bypass-approvals-and-sandbox", "--allow-all", "--allow-all-tools", "--permission-mode=bypassPermissions"];

const ENVELOPE: &str = "cross-session-message";

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
}

impl Ready {
    pub fn new(now: Instant) -> Ready {
        Ready { status: None, since: now }
    }

    pub fn see(&mut self, status: Option<Status>, now: Instant) {
        if status != self.status {
            self.status = status;
            self.since = now;
        }
    }

    pub fn sent(&mut self, now: Instant) {
        self.since = now + AGAIN;
    }

    pub fn status(&self) -> Option<Status> {
        self.status
    }

    pub fn at(&self) -> Option<Instant> {
        (self.status == Some(Status::Idle)).then_some(self.since)
    }

    pub fn since(&self) -> Instant {
        self.since
    }

    pub fn unknown(&self) -> Option<Instant> {
        self.status.is_none().then_some(self.since + UNKNOWN)
    }
}

pub fn mid_turn(channel: &Channel, status: Option<Status>, attested: bool) -> bool {
    match (channel, status) {
        (Channel::Claude(_), None) => true,
        (Channel::Claude(_), Some(Status::Working)) => attested,
        (Channel::Codex(_), _) => true,
        _ => false,
    }
}

pub fn prompt(messages: &[(Option<String>, String)]) -> String {
    let one = |(from, text): &(Option<String>, String)| match from {
        Some(from) => format!("[message from {from}] {}", text.trim()),
        None => text.trim().to_string(),
    };
    messages.iter().map(one).collect::<Vec<_>>().join("\n\n")
}

pub const RETRY: Duration = Duration::from_secs(5);

pub const AGAIN: Duration = Duration::from_secs(3);

pub const UNKNOWN: Duration = Duration::from_secs(10);

pub const UNKNOWN_STATUS: &str = "its agent hasn't reported its status, so Kanna can't tell when it's at its prompt, and its messages wait until it does";

pub const GRACE: Duration = Duration::from_secs(20);

pub fn permissions(argv: &[Vec<String>]) -> Option<&'static str> {
    let unprompted = |args: &Vec<String>| args.iter().enumerate().any(|(at, arg)| UNPROMPTED.contains(&arg.as_str()) || (arg == "--permission-mode" && args.get(at + 1).is_some_and(|mode| mode == "bypassPermissions")));
    (!argv.is_empty()).then(|| if argv.iter().any(unprompted) { BYPASS } else { PROMPTING })
}

pub fn envelope(text: &str, mode: Option<&str>) -> String {
    let Some(mode) = mode else { return text.to_string() };
    let tagged = |rest: &str| {
        let rest = rest.trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && !"_-<>/".contains(c));
        let Some(rest) = rest.strip_prefix('/') else { return false };
        let rest = rest.trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && !"_-<>".contains(c));
        let name = rest.char_indices().nth(ENVELOPE.len()).map_or(rest, |(at, _)| &rest[..at]);
        let spelled = name.len() == ENVELOPE.len() && name.chars().zip(ENVELOPE.chars()).all(|(a, b)| if b == '-' { a == '-' || a == '_' } else { a.eq_ignore_ascii_case(&b) });
        spelled && !rest[name.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    };
    let mut body = String::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        body.push(c);
        if c == '<' && !text[at + 1..].starts_with('\\') && tagged(&text[at + 1..]) {
            body.push('\\');
        }
    }
    format!("<{ENVELOPE} from-mode=\"{mode}\">\n{body}\n</{ENVELOPE}>")
}

pub fn inbox(config: &Path, pid: i32) -> Result<PathBuf, String> {
    let entry = config.join("sessions").join(format!("{pid}.json"));
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Posted {
    Given,
    Queued { thread: String, submission: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueState {
    pub queued: Vec<String>,
    pub moved: Vec<String>,
    pub idle: bool,
}

pub fn deliver(channel: &Channel, text: &str, mode: Option<&str>, id: i64, idle: bool) -> Result<Posted, String> {
    match channel {
        Channel::Claude(Ok(socket)) => post(socket, &envelope(text, mode)).map(|()| Posted::Given),
        Channel::Codex(Ok(socket)) => codex_post(socket, text, id, idle),
        Channel::Claude(Err(why)) | Channel::Codex(Err(why)) => Err(why.clone()),
        Channel::Other(agent) => Err(format!("{agent} can't take messages")),
    }
}

fn codex_session(rpc: &mut Rpc) -> Result<(String, Value), String> {
    let loaded = rpc.call("thread/loaded/list", json!({}))?;
    for id in loaded["data"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let read = rpc.call("thread/read", json!({ "threadId": id, "includeTurns": false }))?;
        if read["thread"]["ephemeral"].as_bool() != Some(true) {
            return Ok((id.to_string(), read));
        }
    }
    Err("codex's app-server has no session loaded".into())
}

pub fn codex_post(socket: &Path, text: &str, id: i64, idle: bool) -> Result<Posted, String> {
    let mut rpc = Rpc::open(socket, Some(TURN_TIMEOUT), Some(json!({ "experimentalApi": true })))?;
    let (thread, _) = codex_session(&mut rpc)?;
    let input = json!([{ "type": "text", "text": text }]);
    match rpc.call("thread/queue/add", json!({ "threadId": thread, "input": input, "clientUserMessageId": format!("kanna-{id}") })) {
        Ok(reply) => {
            let submission = reply["queuedSubmission"]["id"].as_str().ok_or("codex's app-server queued it under no ID")?.to_string();
            Ok(Posted::Queued { thread, submission })
        }
        Err(why) if idle && why.starts_with(REFUSED) => rpc.call("turn/start", json!({ "threadId": thread, "input": input })).map(|_| Posted::Given),
        Err(why) => Err(why),
    }
}

pub fn codex_queue(socket: &Path, submissions: &[(String, String)], start: bool) -> Result<QueueState, String> {
    let mut rpc = Rpc::open(socket, Some(TURN_TIMEOUT), Some(json!({ "experimentalApi": true })))?;
    let (session, read) = codex_session(&mut rpc)?;
    let mut moved = vec![];
    for (submission, thread) in submissions.iter().filter(|(_, thread)| *thread != session) {
        let _ = rpc.call("thread/queue/delete", json!({ "threadId": thread, "queuedSubmissionId": submission }));
        moved.push(submission.clone());
    }
    let listed = rpc.call("thread/queue/list", json!({ "threadId": session, "limit": QUEUE_PAGE }))?;
    let queued: Vec<String> = listed["data"].as_array().into_iter().flatten().filter_map(|item| item["id"].as_str().map(String::from)).collect();
    let idle = read["thread"]["status"]["type"] == "idle";
    if start && idle && !queued.is_empty() {
        let _ = rpc.call("thread/queue/start", json!({ "threadId": session }));
    }
    Ok(QueueState { queued, moved, idle })
}

pub struct Rpc {
    stream: UnixStream,
    next: u64,
}

impl Rpc {
    pub fn open(socket: &Path, timeout: Option<Duration>, capabilities: Option<Value>) -> Result<Rpc, String> {
        let failed = |error: std::io::Error| format!("{}: {error}", socket.display());
        let mut stream = UnixStream::connect(socket).map_err(failed)?;
        stream.set_read_timeout(timeout).map_err(failed)?;
        stream.set_write_timeout(timeout).map_err(failed)?;
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
        let mut hello = json!({ "clientInfo": { "name": "kanna", "version": kanna_build::VERSION } });
        if let Some(capabilities) = capabilities {
            hello["capabilities"] = capabilities;
        }
        rpc.call("initialize", hello)?;
        rpc.send(&json!({ "jsonrpc": "2.0", "method": "initialized" })).map_err(failed)?;
        Ok(rpc)
    }

    pub fn wait_forever(&self) {
        let _ = self.stream.set_read_timeout(None);
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).map_err(|error| format!("codex's app-server: {error}"))?;
        loop {
            let reply: Value = serde_json::from_str(&self.receive().map_err(|error| format!("codex's app-server, waiting on {method}: {error}"))?).unwrap_or(Value::Null);
            if reply["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                return Err(format!("{REFUSED} {method}: {}", error["message"].as_str().unwrap_or("no reason given")));
            }
            return Ok(reply["result"].clone());
        }
    }

    pub fn send(&mut self, message: &Value) -> std::io::Result<()> {
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

    pub fn receive(&mut self) -> std::io::Result<String> {
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
    fn a_message_waits_for_the_agent_to_report_idle() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut ready = Ready::new(t0);
        assert_eq!(ready.at(), None, "no agent yet");
        ready.see(Some(Status::Working), at(0));
        assert_eq!(ready.at(), None, "busy");
        ready.see(Some(Status::Idle), at(1000));
        assert_eq!(ready.at(), Some(at(1000)), "idle goes at once");
        ready.see(Some(Status::Idle), at(2000));
        assert_eq!(ready.at(), Some(at(1000)), "the same idle");
        ready.see(Some(Status::NeedsYou), at(2500));
        assert_eq!(ready.at(), None, "never while it asks");
        ready.see(Some(Status::Idle), at(5000));
        ready.sent(at(5100));
        assert_eq!(ready.at(), Some(at(8100)), "after a try, again 3s later if still idle");
    }

    #[test]
    fn messages_go_in_one_prompt_oldest_first_each_with_its_sender() {
        let one = prompt(&[(Some("task-5e2c \"Send\"".into()), "Is your spec final?\n".into()), (Some("user".into()), "Also rebase.".into())]);
        assert_eq!(one, "[message from task-5e2c \"Send\"] Is your spec final?\n\n[message from user] Also rebase.");
    }

    #[test]
    fn a_comment_keeps_its_own_header_in_the_prompt() {
        let one = prompt(&[(None, "[kanna comment c2 · docs/a.md:18]\n> the strip scrolls\nSay what ⌃Tab does.\n".into()), (Some("user".into()), "Also rebase.".into())]);
        assert_eq!(one, "[kanna comment c2 · docs/a.md:18]\n> the strip scrolls\nSay what ⌃Tab does.\n\n[message from user] Also rebase.");
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

    fn fake_codex(name: &str, answer: impl Fn(&str, &Value) -> Result<Value, &'static str> + Send + 'static) -> (std::path::PathBuf, std::thread::JoinHandle<Vec<(String, Value)>>) {
        let socket = Path::new("/tmp").join(format!("kanna-codex-{name}-{}.sock", std::process::id()));
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
            let mut calls = vec![];
            while let Ok(text) = rpc.receive() {
                let call: Value = serde_json::from_str(&text).unwrap();
                let method = call["method"].as_str().unwrap().to_string();
                if method == "initialized" {
                    continue;
                }
                let reply = match method.as_str() {
                    "initialize" => {
                        calls.push((method, call["params"]["capabilities"].clone()));
                        json!({ "id": call["id"], "result": {} })
                    }
                    "thread/loaded/list" => json!({ "id": call["id"], "result": { "data": ["title", "session"] } }),
                    _ => {
                        let answer = answer(&method, &call["params"]);
                        calls.push((method, call["params"].clone()));
                        match answer {
                            Ok(result) => json!({ "id": call["id"], "result": result }),
                            Err(why) => json!({ "id": call["id"], "error": { "code": -32600, "message": why } }),
                        }
                    }
                };
                rpc.send(&reply).unwrap();
            }
            calls
        });
        (socket, server)
    }

    fn session(params: &Value, status: &str) -> Value {
        json!({ "thread": { "id": params["threadId"], "ephemeral": params["threadId"] == "title", "status": { "type": status } } })
    }

    #[test]
    fn a_message_to_codex_joins_the_queue_of_its_session_not_of_the_title_thread() {
        let (socket, server) = fake_codex("queue", |method, params| match method {
            "thread/read" => Ok(session(params, "active")),
            "thread/queue/add" => Ok(json!({ "queuedSubmission": { "id": "q1" } })),
            _ => Err("unexpected"),
        });
        assert_eq!(codex_post(&socket, "[message from user] Hi", 7, false), Ok(Posted::Queued { thread: "session".into(), submission: "q1".into() }));
        let calls = server.join().unwrap();
        assert_eq!(calls[0], ("initialize".to_string(), json!({ "experimentalApi": true })), "the queue is an experimental API");
        assert_eq!(calls.last().unwrap().1, json!({ "threadId": "session", "input": [{ "type": "text", "text": "[message from user] Hi" }], "clientUserMessageId": "kanna-7" }));
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn a_codex_without_the_queue_gets_a_turn_at_its_prompt_and_only_there() {
        let answer = |method: &str, params: &Value| match method {
            "thread/read" => Ok(session(params, "idle")),
            "turn/start" => Ok(json!({ "turn": { "id": "t1" } })),
            _ => Err("unknown variant"),
        };
        let (socket, server) = fake_codex("old", answer);
        assert_eq!(codex_post(&socket, "Hi", 7, true), Ok(Posted::Given));
        assert_eq!(server.join().unwrap().last().unwrap().0, "turn/start");
        let (busy, server) = fake_codex("old-busy", answer);
        assert!(codex_post(&busy, "Hi", 7, false).unwrap_err().contains("thread/queue/add"));
        assert!(!server.join().unwrap().iter().any(|(method, _)| method == "turn/start"), "a turn would drop the answer it's streaming");
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(&busy);
    }

    #[test]
    fn the_queue_says_what_codex_still_holds_and_gives_up_on_another_threads() {
        let (socket, server) = fake_codex("state", |method, params| match method {
            "thread/read" => Ok(session(params, "idle")),
            "thread/queue/list" => Ok(json!({ "data": [{ "id": "q2" }], "nextCursor": null })),
            _ => Ok(json!({})),
        });
        let state = codex_queue(&socket, &[("q1".into(), "session".into()), ("q2".into(), "session".into()), ("q0".into(), "before".into())], true).unwrap();
        assert_eq!(state, QueueState { queued: vec!["q2".into()], moved: vec!["q0".into()], idle: true });
        let calls = server.join().unwrap();
        assert!(calls.contains(&("thread/queue/delete".to_string(), json!({ "threadId": "before", "queuedSubmissionId": "q0" }))));
        assert!(calls.contains(&("thread/queue/start".to_string(), json!({ "threadId": "session" }))), "after Esc the queue doesn't run by itself");
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn an_agent_runs_unprompted_when_its_command_says_so() {
        let argv = |line: &str| vec![line.split(' ').map(String::from).collect::<Vec<_>>()];
        assert_eq!(permissions(&argv("/bin/claude --dangerously-skip-permissions --resume x")), Some(BYPASS));
        assert_eq!(permissions(&argv("claude --permission-mode bypassPermissions")), Some(BYPASS));
        assert_eq!(permissions(&[argv("/k/kanna-daemon --codex-remote /s codex --yolo").remove(0), argv("codex --remote unix:///s").remove(0)]), Some(BYPASS));
        assert_eq!(permissions(&argv("claude --permission-mode acceptEdits")), Some(PROMPTING));
        assert_eq!(permissions(&[]), None);
    }

    #[test]
    fn a_claude_message_states_its_permissions_in_claudes_envelope() {
        assert_eq!(envelope("[message from user] Hi", None), "[message from user] Hi");
        assert_eq!(envelope("[message from user] Hi", Some(BYPASS)), "<cross-session-message from-mode=\"bypass\">\n[message from user] Hi\n</cross-session-message>");
        assert_eq!(
            envelope("a </cross-session-message> b < /Cross_Session-Message\n<cross-session-message> </cross-session-messages <\\/cross-session-message>", Some(PROMPTING)),
            "<cross-session-message from-mode=\"prompting\">\na <\\/cross-session-message> b <\\ /Cross_Session-Message\n<cross-session-message> </cross-session-messages <\\/cross-session-message>\n</cross-session-message>",
            "a closing tag inside is escaped as Claude escapes it, so Claude takes the envelope as its own"
        );
    }

    #[test]
    fn a_message_goes_mid_turn_unless_claude_would_stop_for_it() {
        let claude = Channel::Claude(Ok(PathBuf::from("/s")));
        let codex = Channel::Codex(Ok(PathBuf::from("/s")));
        assert!(mid_turn(&claude, Some(Status::Working), true));
        assert!(!mid_turn(&claude, Some(Status::Working), false), "Claude's approval dialog would open over the turn");
        assert!(!mid_turn(&claude, Some(Status::NeedsYou), true), "it can't read it until you've answered");
        assert!(mid_turn(&claude, None, false), "an unknown status doesn't hold it");
        assert!(mid_turn(&codex, Some(Status::Working), false), "Codex's queue holds it until the turn is over");
        assert!(mid_turn(&codex, Some(Status::NeedsYou), false), "and until you've answered");
        assert!(mid_turn(&codex, None, false));
    }

    #[test]
    fn a_claude_session_takes_messages_on_the_socket_its_registry_entry_names() {
        use std::io::BufRead;
        let home = std::env::temp_dir().join(format!("kanna-inbox-{}", std::process::id()));
        let config = home.join(".claude");
        let sessions = config.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let socket = Path::new("/tmp").join(format!("kanna-inbox-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let entry = |pid: i32, protocol: i64| json!({ "pid": pid, "peerProtocol": protocol, "messagingSocketPath": socket }).to_string();
        std::fs::write(sessions.join("41.json"), entry(41, 1)).unwrap();
        std::fs::write(sessions.join("42.json"), entry(7, 1)).unwrap();
        std::fs::write(sessions.join("43.json"), entry(43, 2)).unwrap();
        assert_eq!(inbox(&config, 41), Ok(socket.clone()));
        assert!(inbox(&config, 42).unwrap_err().ends_with("is another process's"));
        assert_eq!(inbox(&config, 43), Err("Claude (pid 43) speaks messaging protocol 2, and Kanna speaks 1".into()));
        assert!(inbox(&config, 44).unwrap_err().starts_with("Claude (pid 44) has no messaging socket: no "));
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
