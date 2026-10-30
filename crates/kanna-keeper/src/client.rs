use crate::{fdpass, paths, ExitStatus};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const START_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub struct Pty {
    pub id: String,
    pub pid: i32,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub master: OwnedFd,
    pub exit: Option<ExitStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub version: u64,
    pub pid: i32,
    pub build: Option<String>,
}

impl Hello {
    fn from_reply(reply: &Value) -> Hello {
        Hello {
            version: reply["version"].as_u64().unwrap_or(0),
            pid: reply["pid"].as_i64().unwrap_or(0) as i32,
            build: reply["build"].as_str().map(String::from),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exited {
    pub id: String,
    pub pid: i32,
    pub status: ExitStatus,
}

#[derive(Clone, Debug, Default)]
pub struct SpawnRequest {
    pub id: String,
    pub command: Option<String>,
    pub shell: Option<String>,
    pub skip_rc: bool,
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, Option<String>>,
    pub cols: u16,
    pub rows: u16,
}

impl SpawnRequest {
    pub fn shell(id: &str) -> Self {
        SpawnRequest { id: id.into(), cols: 80, rows: 24, ..Default::default() }
    }

    pub fn command(id: &str, command: &str) -> Self {
        SpawnRequest { command: Some(command.into()), ..Self::shell(id) }
    }

    fn to_json(&self) -> Value {
        let mut v = json!({"op": "spawn", "id": self.id, "cols": self.cols, "rows": self.rows});
        if let Some(c) = &self.command {
            v["command"] = c.as_str().into();
        }
        if self.skip_rc {
            v["rc"] = false.into();
        }
        if let Some(s) = &self.shell {
            v["shell"] = s.as_str().into();
        }
        if let Some(cwd) = &self.cwd {
            v["cwd"] = cwd.to_string_lossy().into();
        }
        if !self.env.is_empty() {
            let env: Map<String, Value> =
                self.env.iter().map(|(k, v)| (k.clone(), v.as_deref().map_or(Value::Null, Value::from))).collect();
            v["env"] = Value::Object(env);
        }
        v
    }
}

pub struct Client {
    stream: UnixStream,
    inbuf: Vec<u8>,
    readbuf: Box<[u8]>,
    fds: VecDeque<OwnedFd>,
    events: VecDeque<Exited>,
    next_req: u64,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn keeper_error(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

impl Client {
    pub fn connect(socket: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(socket)?;
        if fdpass::peer_uid(&stream)? != unsafe { libc::getuid() } {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the keeper runs as another user"));
        }
        Ok(Client { stream, inbuf: Vec::new(), readbuf: vec![0u8; 65536].into_boxed_slice(), fds: VecDeque::new(), events: VecDeque::new(), next_req: 1 })
    }

    pub fn connect_or_start(state_dir: &Path, keeper_bin: &Path) -> io::Result<Self> {
        let socket = paths::socket(state_dir);
        let deadline = Instant::now() + START_TIMEOUT;
        let mut backoff = Duration::from_millis(5);
        loop {
            let error = match Self::connect(&socket) {
                Ok(client) => return Ok(client),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => return Err(e),
                Err(e) => e,
            };
            if Instant::now() >= deadline {
                return Err(keeper_error(format!("no keeper is listening on {}: {error}", socket.display())));
            }
            let out = std::process::Command::new(keeper_bin)
                .arg("--state-dir")
                .arg(state_dir)
                .arg("--detach")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .output()?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(keeper_error(format!(
                    "{} --detach failed ({}): {}",
                    keeper_bin.display(),
                    out.status,
                    stderr.trim()
                )));
            }
            if let Ok(client) = Self::connect(&socket) {
                return Ok(client);
            }
            std::thread::sleep(backoff.min(deadline.saturating_duration_since(Instant::now())));
            backoff = (backoff * 2).min(Duration::from_millis(200));
        }
    }

    pub fn set_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)?;
        self.stream.set_write_timeout(timeout)
    }

    pub fn hello(&mut self) -> io::Result<(u64, i32)> {
        self.info().map(|h| (h.version, h.pid))
    }

    pub fn info(&mut self) -> io::Result<Hello> {
        let (reply, _) = self.request(json!({"op": "hello"}), |_, _| {})?;
        Ok(Hello::from_reply(&reply))
    }

    pub fn upgrade(&mut self, binary: &Path) -> io::Result<Hello> {
        let (reply, _) = self.request(json!({"op": "upgrade", "binary": binary.to_string_lossy()}), |_, _| {})?;
        Ok(Hello::from_reply(&reply))
    }

    pub fn spawn(&mut self, req: &SpawnRequest) -> io::Result<Pty> {
        let (reply, mut fds) = self.request(req.to_json(), |_, _| {})?;
        let master = fds.pop().ok_or_else(|| keeper_error("spawn: no pty in the reply"))?;
        Ok(Pty {
            id: req.id.clone(),
            pid: reply["pid"].as_i64().unwrap_or(0) as i32,
            cmd: strings(&reply["cmd"]),
            cwd: req.cwd.as_ref().map(|c| c.to_string_lossy().into_owned()).unwrap_or_default(),
            master,
            exit: None,
        })
    }

    pub fn adopt(&mut self) -> io::Result<Vec<Pty>> {
        let mut ptys = Vec::new();
        let mut missing = Vec::new();
        let (reply, _) = self.request(json!({"op": "adopt"}), |line, fds| {
            let p = &line["pty"];
            let id: String = p["id"].as_str().unwrap_or("").into();
            match fds.pop() {
                Some(master) => ptys.push(Pty {
                    id,
                    pid: p["pid"].as_i64().unwrap_or(0) as i32,
                    cmd: strings(&p["cmd"]),
                    cwd: p["cwd"].as_str().unwrap_or("").into(),
                    master,
                    exit: p.get("exit").and_then(ExitStatus::from_json),
                }),
                None => missing.push(id),
            }
        })?;
        if !missing.is_empty() {
            return Err(keeper_error(format!("adopt: no pty came with {}", missing.join(", "))));
        }
        if reply["ptys"].as_u64() != Some(ptys.len() as u64) {
            return Err(keeper_error(format!("adopt: the keeper holds {} ptys, {} came", reply["ptys"], ptys.len())));
        }
        Ok(ptys)
    }

    pub fn close(&mut self, id: &str) -> io::Result<()> {
        self.request(json!({"op": "close", "id": id}), |_, _| {}).map(|_| ())
    }

    pub fn stop(&mut self) -> io::Result<()> {
        self.request(json!({"op": "stop"}), |_, _| {}).map(|_| ())
    }

    pub fn ack(&mut self, id: &str) -> io::Result<()> {
        self.request(json!({"op": "ack", "id": id}), |_, _| {}).map(|_| ())
    }

    pub fn poll_events(&mut self) -> io::Result<Vec<Exited>> {
        while self.fill(false)? {}
        self.parse_lines(None, &mut |_, _| {});
        Ok(self.events.drain(..).collect())
    }

    pub fn wait_event(&mut self, timeout: Option<Duration>) -> io::Result<Option<Exited>> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            self.parse_lines(None, &mut |_, _| {});
            if let Some(e) = self.events.pop_front() {
                return Ok(Some(e));
            }
            let wait = match deadline {
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(None);
                    }
                    left.as_millis() as libc::c_int + 1
                }
                None => -1,
            };
            let mut pfd = libc::pollfd { fd: self.stream.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut pfd, 1, wait) } > 0 {
                self.fill(true)?;
            }
        }
    }

    fn fill(&mut self, block: bool) -> io::Result<bool> {
        match fdpass::recv(self.stream.as_raw_fd(), &mut self.readbuf, !block) {
            Ok((0, _)) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the keeper closed the connection")),
            Ok((n, fds)) => {
                self.inbuf.extend_from_slice(&self.readbuf[..n]);
                self.fds.extend(fds);
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && !block => Ok(false),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                Err(io::Error::new(io::ErrorKind::TimedOut, "the keeper didn't answer in time"))
            }
            Err(e) => Err(e),
        }
    }

    fn parse_lines(
        &mut self,
        req: Option<u64>,
        on_line: &mut dyn FnMut(&Value, &mut Vec<OwnedFd>),
    ) -> Option<(Value, Vec<OwnedFd>)> {
        while let Some(end) = self.inbuf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.inbuf.drain(..=end).collect();
            let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
            let n = v["fds"].as_u64().unwrap_or(0) as usize;
            let mut fds: Vec<OwnedFd> = (0..n).filter_map(|_| self.fds.pop_front()).collect();
            if let Some(id) = v["exited"].as_str() {
                if let Some(status) = ExitStatus::from_json(&v) {
                    self.events.push_back(Exited { id: id.into(), pid: v["pid"].as_i64().unwrap_or(0) as i32, status });
                }
                continue;
            }
            if req.is_some() && v["req"].as_u64() == req {
                if v.get("ok").is_some() {
                    return Some((v, fds));
                }
                on_line(&v, &mut fds);
            }
        }
        None
    }

    fn request(
        &mut self,
        mut body: Value,
        mut on_line: impl FnMut(&Value, &mut Vec<OwnedFd>),
    ) -> io::Result<(Value, Vec<OwnedFd>)> {
        let tag = self.next_req;
        self.next_req += 1;
        body["req"] = tag.into();
        let mut bytes = serde_json::to_vec(&body).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let mut sent = 0;
        while sent < bytes.len() {
            sent += fdpass::send(self.stream.as_raw_fd(), &bytes[sent..], &[], false)?;
        }
        loop {
            if let Some((reply, fds)) = self.parse_lines(Some(tag), &mut on_line) {
                if reply["ok"].as_bool() == Some(true) {
                    return Ok((reply, fds));
                }
                return Err(keeper_error(reply["error"].as_str().unwrap_or("keeper error").to_string()));
            }
            self.fill(true)?;
        }
    }
}

impl AsRawFd for Client {
    fn as_raw_fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }
}
