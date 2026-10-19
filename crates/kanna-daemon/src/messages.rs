use kanna_detect::Status;
use serde_json::{json, Value};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const PLANNER: &str = "planner";

pub const USER: &str = "user";

pub const TYPED_HOLD: Duration = Duration::from_secs(5);

pub const DRAFT_HOLD: Duration = Duration::from_secs(120);

pub const PICKUP: Duration = Duration::from_secs(5);

pub const POST_TIMEOUT: Duration = Duration::from_secs(2);

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

pub fn inbox(home: &Path, pid: i32) -> Option<PathBuf> {
    let text = std::fs::read_to_string(home.join(".claude").join("sessions").join(format!("{pid}.json"))).ok()?;
    let session: Value = serde_json::from_str(&text).ok()?;
    if session["pid"].as_i64() != Some(pid.into()) || session["peerProtocol"].as_i64() != Some(1) {
        return None;
    }
    session["messagingSocketPath"].as_str().map(PathBuf::from).filter(|socket| socket.is_absolute())
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
        assert_eq!(inbox(&home, 41), Some(socket.clone()));
        assert_eq!(inbox(&home, 42), None, "an entry for another process");
        assert_eq!(inbox(&home, 43), None, "a protocol Kanna doesn't speak");
        assert_eq!(inbox(&home, 44), None, "no entry: not a Claude session");
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
