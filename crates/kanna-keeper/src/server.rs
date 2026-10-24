use crate::pty::{self, What};
use crate::upgrade::{self, ClosedState, ConnState, PendingState, PtyState, Requester, State};
use crate::{fdpass, paths, session, sig, ExitStatus, PROTOCOL_VERSION};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::rc::Rc;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_CONNS: usize = 16;
const ACCEPT_PAUSE: Duration = Duration::from_millis(100);
const MAX_LINE: usize = 1 << 20;
const MAX_OUT: usize = 8 << 20;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_LIMIT: Duration = Duration::from_secs(5);

pub struct Settings {
    pub state_dir: PathBuf,
    pub idle: Duration,
    pub close_grace: Duration,
    pub max_ptys: usize,
    pub spawn_timeout: Duration,
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

impl Settings {
    pub fn from_env(state_dir: PathBuf) -> Self {
        Settings {
            state_dir,
            idle: Duration::from_millis(env_u64("KANNA_KEEPER_IDLE_MS", 60_000)),
            close_grace: Duration::from_millis(env_u64("KANNA_KEEPER_CLOSE_GRACE_MS", 1_000)),
            max_ptys: env_u64("KANNA_KEEPER_MAX_PTYS", 256) as usize,
            spawn_timeout: Duration::from_millis(env_u64("KANNA_KEEPER_SPAWN_TIMEOUT_MS", 5_000)),
        }
    }
}

fn expected_uid() -> libc::uid_t {
    #[cfg(debug_assertions)]
    if let Some(uid) = std::env::var("KANNA_KEEPER_TEST_PEER_UID").ok().and_then(|v| v.parse().ok()) {
        return uid;
    }
    unsafe { libc::getuid() }
}

struct Pty {
    pid: i32,
    cmd: Vec<String>,
    cwd: String,
    master: Rc<OwnedFd>,
    exit: Option<ExitStatus>,
}

impl Pty {
    fn describe(&self, id: &str) -> Value {
        let mut v = json!({"id": id, "pid": self.pid, "cmd": self.cmd, "cwd": self.cwd});
        if let Some(exit) = self.exit {
            v["exit"] = exit.to_json();
        }
        v
    }
}

struct Out {
    bytes: Vec<u8>,
    sent: usize,
    fds: Vec<Rc<OwnedFd>>,
}

struct Conn {
    serial: u64,
    stream: UnixStream,
    inbuf: Vec<u8>,
    out: VecDeque<Out>,
    out_bytes: usize,
    out_fds: usize,
    max_out_fds: usize,
    dead: bool,
}

impl Conn {
    fn send(&mut self, value: Value, fds: Vec<Rc<OwnedFd>>) {
        let mut bytes = serde_json::to_vec(&value).unwrap_or_default();
        bytes.push(b'\n');
        self.out_bytes += bytes.len();
        self.out_fds += fds.len();
        if self.out_bytes > MAX_OUT || self.out_fds > self.max_out_fds {
            eprintln!("kanna-keeper: dropped a client that isn't reading");
            self.dead = true;
            return;
        }
        self.out.push_back(Out { bytes, sent: 0, fds });
    }

    fn flush(&mut self) {
        while let Some(out) = self.out.front_mut() {
            let fds: Vec<_> = if out.sent == 0 { out.fds.iter().map(|f| f.as_raw_fd()).collect() } else { vec![] };
            match fdpass::send(self.stream.as_raw_fd(), &out.bytes[out.sent..], &fds, true) {
                Ok(n) => {
                    out.sent += n;
                    self.out_fds -= out.fds.len();
                    out.fds.clear();
                    if out.sent == out.bytes.len() {
                        self.out_bytes -= out.bytes.len();
                        self.out.pop_front();
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.dead = true;
                    break;
                }
            }
        }
    }
}

struct Pending {
    conn: u64,
    tag: Value,
    id: String,
    pid: i32,
    cmd: Vec<String>,
    cwd: String,
    master: OwnedFd,
    exec: OwnedFd,
    deadline: Instant,
    exit: Option<ExitStatus>,
}

struct Closed {
    pid: i32,
    kill_at: Option<Instant>,
}

struct Keeper {
    listener: UnixListener,
    sigs: OwnedFd,
    conns: Vec<Conn>,
    ptys: BTreeMap<String, Pty>,
    pending: Vec<Pending>,
    base: pty::Base,
    next_serial: u64,
    closed: Vec<Closed>,
    settings: Settings,
    idle_since: Instant,
    accept_paused_until: Option<Instant>,
    peer_uid: libc::uid_t,
    stop: bool,
    stopping: Option<Instant>,
    lock: Option<File>,
    build: Option<String>,
    upgrade: Option<(u64, Value, String)>,
}

pub const LISTENING: u8 = b'1';
pub const ALREADY_RUNNING: u8 = b'2';
pub const FAILED: u8 = b'E';

fn tell(ready: Option<&OwnedFd>, bytes: &[u8]) {
    if let Some(fd) = ready {
        unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
    }
}

pub fn run(settings: Settings, ready: Option<&OwnedFd>) -> Result<()> {
    sig::cloexec_inherited();
    pty::raise_nofile();
    let dir = settings.state_dir.clone();
    paths::ensure_dir(&dir).with_context(|| format!("state directory {}", dir.display()))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(paths::lock(&dir))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        tell(ready, &[ALREADY_RUNNING]);
        return Ok(());
    }
    let sigs = signals()?;
    let listener = listen(&paths::socket(&dir))?;
    std::fs::write(paths::pid(&dir), format!("{}\n", std::process::id()))?;
    tell(ready, &[LISTENING]);
    serve(Keeper::new(listener, sigs, settings, lock))
}

pub fn resume(settings: Settings, fd: RawFd) -> Result<()> {
    let mut file = unsafe { File::from_raw_fd(fd) };
    let state = upgrade::read(&mut file).map_err(anyhow::Error::msg).context("resuming after an upgrade")?;
    drop(file);
    upgrade::open(&state.fds()).map_err(anyhow::Error::msg)?;
    sig::cloexec_inherited();
    if let Some(nofile) = state.nofile {
        pty::keep_original_nofile(nofile);
    }
    let sigs = signals()?;
    let (lock, listener) = unsafe { (File::from_raw_fd(state.lock), UnixListener::from_raw_fd(state.listener)) };
    let mut keeper = Keeper::new(listener, sigs, settings, lock);
    keeper.next_serial = state.next_serial;
    let max_out_fds = keeper.max_out_fds();
    keeper.conns = state
        .conns
        .into_iter()
        .map(|c| Conn {
            serial: c.serial,
            stream: unsafe { UnixStream::from_raw_fd(c.fd) },
            inbuf: c.inbuf,
            out: VecDeque::new(),
            out_bytes: 0,
            out_fds: 0,
            max_out_fds,
            dead: false,
        })
        .collect();
    keeper.ptys = state
        .ptys
        .into_iter()
        .map(|p| {
            let master = Rc::new(unsafe { OwnedFd::from_raw_fd(p.master) });
            (p.id, Pty { pid: p.pid, cmd: p.cmd, cwd: p.cwd, master, exit: p.exit })
        })
        .collect();
    let now = Instant::now();
    let paused = upgrade::monotonic().saturating_sub(state.written);
    keeper.pending = state
        .pending
        .into_iter()
        .map(|p| Pending {
            conn: p.conn,
            tag: p.tag,
            id: p.id,
            pid: p.pid,
            cmd: p.cmd,
            cwd: p.cwd,
            master: unsafe { OwnedFd::from_raw_fd(p.master) },
            exec: unsafe { OwnedFd::from_raw_fd(p.exec) },
            deadline: now + p.left.saturating_sub(paused),
            exit: p.exit,
        })
        .collect();
    keeper.closed = state.closed.into_iter().map(|c| Closed { pid: c.pid, kill_at: c.kill_in.map(|d| now + d.saturating_sub(paused)) }).collect();
    keeper.reap();
    let requester = state.requester;
    eprintln!(
        "kanna-keeper: upgraded (pid {}) from build {} to {}",
        std::process::id(),
        short(requester.from.as_deref()),
        short(keeper.build.as_deref())
    );
    let mut reply = json!({"ok": true, "version": PROTOCOL_VERSION, "pid": std::process::id(), "build": keeper.build, "from": requester.from});
    if !requester.tag.is_null() {
        reply["req"] = requester.tag;
    }
    if let Some(c) = keeper.conns.iter_mut().find(|c| c.serial == requester.conn) {
        c.send(reply, vec![]);
    }
    for i in 0..keeper.conns.len() {
        keeper.handle_lines(i);
    }
    serve(keeper)
}

fn short(build: Option<&str>) -> &str {
    build.map_or("unknown", |b| &b[..b.len().min(8)])
}

fn signals() -> Result<OwnedFd> {
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
    }
    let sigs = sig::pipe(&[libc::SIGCHLD, libc::SIGTERM, libc::SIGINT])?;
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signo in [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT, libc::SIGALRM] {
            libc::sigaddset(&mut set, signo);
        }
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
    }
    Ok(sigs)
}

fn serve(mut keeper: Keeper) -> Result<()> {
    let result = keeper.run();
    let dir = keeper.settings.state_dir.clone();
    let lock = keeper.lock.take();
    drop(keeper);
    let _ = std::fs::remove_file(paths::socket(&dir));
    let pid_file = paths::pid(&dir);
    if std::fs::read_to_string(&pid_file).is_ok_and(|p| p.trim() == std::process::id().to_string()) {
        let _ = std::fs::remove_file(&pid_file);
    }
    drop(lock);
    result
}

fn listen(sock: &Path) -> Result<UnixListener> {
    let _ = std::fs::remove_file(sock);
    let old = unsafe { libc::umask(0o177) };
    let listener = UnixListener::bind(sock);
    unsafe { libc::umask(old) };
    let listener = listener.with_context(|| format!("binding {}", sock.display()))?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

fn pollfd(fd: libc::c_int, events: libc::c_short) -> libc::pollfd {
    libc::pollfd { fd, events, revents: 0 }
}

impl Keeper {
    fn new(listener: UnixListener, sigs: OwnedFd, settings: Settings, lock: File) -> Keeper {
        Keeper {
            listener,
            sigs,
            conns: Vec::new(),
            ptys: BTreeMap::new(),
            pending: Vec::new(),
            base: pty::Base::load(),
            next_serial: 0,
            closed: Vec::new(),
            settings,
            idle_since: Instant::now(),
            accept_paused_until: None,
            peer_uid: expected_uid(),
            stop: false,
            stopping: None,
            lock: Some(lock),
            build: kanna_build::own_build_id(),
            upgrade: None,
        }
    }

    fn max_out_fds(&self) -> usize {
        2 * self.settings.max_ptys + 16
    }

    fn run(&mut self) -> Result<()> {
        while !self.stop {
            if self.accept_paused_until.is_some_and(|until| Instant::now() >= until) {
                self.accept_paused_until = None;
            }
            let listening = if self.accept_paused_until.is_none() && self.stopping.is_none() { self.listener.as_raw_fd() } else { -1 };
            let mut fds = vec![pollfd(self.sigs.as_raw_fd(), libc::POLLIN), pollfd(listening, libc::POLLIN)];
            for c in &self.conns {
                let events = if c.out.is_empty() { libc::POLLIN } else { libc::POLLIN | libc::POLLOUT };
                fds.push(pollfd(c.stream.as_raw_fd(), events));
            }
            let conns = self.conns.len();
            for p in &self.pending {
                fds.push(pollfd(p.exec.as_raw_fd(), libc::POLLIN));
            }
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, self.timeout()) };
            if n < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() != std::io::ErrorKind::Interrupted {
                    return Err(e.into());
                }
                continue;
            }
            if fds[0].revents != 0 {
                for signo in sig::drain(&self.sigs) {
                    match signo {
                        libc::SIGTERM | libc::SIGINT => self.stop = true,
                        libc::SIGCHLD => self.reap(),
                        _ => {}
                    }
                }
            }
            for i in 0..conns {
                if fds[2 + i].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                    self.read_conn(i);
                }
            }
            for k in (0..fds.len() - 2 - conns).rev() {
                if fds[2 + conns + k].revents != 0 {
                    self.exec_heard(k);
                }
            }
            self.spawn_timeouts();
            if fds[1].revents != 0 {
                self.accept();
            }
            for c in &mut self.conns {
                c.flush();
            }
            self.conns.retain(|c| !c.dead);
            if self.run_kills() {
                self.reap();
            }
            if let Some(wanted) = self.upgrade.take() {
                self.upgrade(wanted);
            }
            if let Some(since) = self.stopping {
                self.reap();
                let done = self.closed.is_empty() && self.pending.is_empty() && self.conns.iter().all(|c| c.out.is_empty());
                if done || since.elapsed() >= STOP_LIMIT {
                    self.stop = true;
                }
                continue;
            }
            self.check_idle();
        }
        Ok(())
    }

    fn timeout(&self) -> libc::c_int {
        let now = Instant::now();
        let mut wait = Duration::from_secs(1);
        if let Some(at) = self.closed.iter().filter_map(|c| c.kill_at).min() {
            wait = wait.min(at.saturating_duration_since(now));
        }
        if let Some(at) = self.pending.iter().map(|p| p.deadline).min() {
            wait = wait.min(at.saturating_duration_since(now));
        }
        if let Some(until) = self.accept_paused_until {
            wait = wait.min(until.saturating_duration_since(now));
        }
        if self.idle() {
            wait = wait.min((self.idle_since + self.settings.idle).saturating_duration_since(now));
        }
        wait.as_millis() as libc::c_int + 1
    }

    fn idle(&self) -> bool {
        self.conns.is_empty()
            && self.pending.is_empty()
            && self.closed.is_empty()
            && self.ptys.values().all(|p| p.exit.is_some())
    }

    fn check_idle(&mut self) {
        if !self.idle() {
            self.idle_since = Instant::now();
            return;
        }
        if self.idle_since.elapsed() < self.settings.idle {
            return;
        }
        let sock = paths::socket(&self.settings.state_dir);
        let _ = std::fs::remove_file(&sock);
        self.accept();
        if self.conns.is_empty() {
            self.stop = true;
            return;
        }
        match listen(&sock) {
            Ok(listener) => self.listener = listener,
            Err(e) => {
                eprintln!("kanna-keeper: {e:#}");
                self.stop = true;
            }
        }
        self.idle_since = Instant::now();
    }

    fn run_kills(&mut self) -> bool {
        let now = Instant::now();
        let mut killed = false;
        for c in &mut self.closed {
            if c.kill_at.is_none_or(|at| at > now) {
                continue;
            }
            for group in session::groups(c.pid) {
                unsafe { libc::killpg(group, libc::SIGKILL) };
            }
            c.kill_at = None;
            killed = true;
        }
        killed
    }

    fn accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    match fdpass::peer_uid(&stream) {
                        Ok(uid) if uid == self.peer_uid => {}
                        _ => {
                            eprintln!("kanna-keeper: refused a connection from another user");
                            continue;
                        }
                    }
                    if self.conns.len() >= MAX_CONNS {
                        eprintln!("kanna-keeper: refused a connection: too many clients");
                        continue;
                    }
                    let _ = stream.set_nonblocking(true);
                    self.next_serial += 1;
                    let mut conn = Conn {
                        serial: self.next_serial,
                        stream,
                        inbuf: Vec::new(),
                        out: VecDeque::new(),
                        out_bytes: 0,
                        out_fds: 0,
                        max_out_fds: self.max_out_fds(),
                        dead: false,
                    };
                    for (id, p) in &self.ptys {
                        if let Some(exit) = p.exit {
                            conn.send(exited_event(id, p.pid, exit), vec![]);
                        }
                    }
                    self.conns.push(conn);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) if matches!(e.raw_os_error(), Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)) => {
                    eprintln!("kanna-keeper: can't accept a connection: {e}");
                    self.accept_paused_until = Some(Instant::now() + ACCEPT_PAUSE);
                    break;
                }
                Err(_) => break,
            }
        }
    }

    fn read_conn(&mut self, i: usize) {
        let mut buf = [0u8; 65536];
        let n = match self.conns[i].stream.read(&mut buf) {
            Ok(0) => {
                self.conns[i].dead = true;
                return;
            }
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => return,
            Err(_) => {
                self.conns[i].dead = true;
                return;
            }
        };
        self.conns[i].inbuf.extend_from_slice(&buf[..n]);
        self.handle_lines(i);
    }

    fn handle_lines(&mut self, i: usize) {
        while let Some(end) = self.conns[i].inbuf.iter().position(|&b| b == b'\n') {
            if self.upgrade.is_some() {
                return;
            }
            let line: Vec<u8> = self.conns[i].inbuf.drain(..=end).collect();
            if self.conns[i].dead {
                return;
            }
            match serde_json::from_slice::<Value>(&line) {
                Ok(req) => self.request(i, &req),
                Err(e) => self.conns[i].send(json!({"ok": false, "error": format!("bad request: {e}")}), vec![]),
            }
        }
        if self.conns[i].inbuf.len() > MAX_LINE {
            eprintln!("kanna-keeper: dropped a client: request too long");
            self.conns[i].dead = true;
        }
    }

    fn request(&mut self, i: usize, req: &Value) {
        let tag = req.get("req").cloned().unwrap_or(Value::Null);
        let id = req["id"].as_str().unwrap_or("");
        let result: Result<(Value, Vec<Rc<OwnedFd>>), String> = match req["op"].as_str().unwrap_or("") {
            "hello" => Ok((json!({"version": PROTOCOL_VERSION, "pid": std::process::id(), "build": self.build}), vec![])),
            "spawn" => match self.spawn(i, &tag, id, req) {
                Ok(()) => return,
                Err(error) => Err(error),
            },
            "adopt" => {
                let lines: Vec<_> = self
                    .ptys
                    .iter()
                    .map(|(id, p)| (json!({"req": tag, "pty": p.describe(id), "fds": 1}), p.master.clone()))
                    .collect();
                let count = lines.len();
                for (line, fd) in lines {
                    self.conns[i].send(line, vec![fd]);
                }
                Ok((json!({"ptys": count}), vec![]))
            }
            "close" => self.close(id),
            "ack" => self.ack(id),
            "stop" => Ok(self.begin_stop()),
            "upgrade" => match req["binary"].as_str() {
                Some(binary) if Path::new(binary).is_absolute() => {
                    self.upgrade = Some((self.conns[i].serial, tag, binary.to_string()));
                    return;
                }
                _ => Err("upgrade needs the absolute path of a keeper binary".into()),
            },
            op => Err(format!("unknown op {op:?}")),
        };
        let (mut reply, fds) = match result {
            Ok((body, fds)) => {
                let mut reply = json!({"ok": true});
                if let Value::Object(body) = body {
                    reply.as_object_mut().unwrap().extend(body);
                }
                if !fds.is_empty() {
                    reply["fds"] = json!(fds.len());
                }
                (reply, fds)
            }
            Err(error) => (json!({"ok": false, "error": error}), vec![]),
        };
        if !tag.is_null() {
            reply["req"] = tag;
        }
        self.conns[i].send(reply, fds);
    }

    fn spawn(&mut self, i: usize, tag: &Value, id: &str, req: &Value) -> Result<(), String> {
        if self.stopping.is_some() {
            return Err("the keeper is stopping".into());
        }
        valid_id(id)?;
        if self.ptys.contains_key(id) || self.pending.iter().any(|p| p.id == id) {
            return Err(format!("pty {id:?} exists"));
        }
        if self.ptys.len() + self.pending.len() + self.closed.len() >= self.settings.max_ptys {
            return Err(format!("too many ptys ({})", self.settings.max_ptys));
        }
        let what = match (req["command"].as_str(), req["shell"].as_str()) {
            (Some(_), Some(_)) => return Err("give command or shell, not both".into()),
            (Some(command), None) if req["rc"] == Value::Bool(false) => What::LoginCommand(command),
            (Some(command), None) => What::Command(command),
            (None, shell) => What::Shell(shell),
        };
        let size = |key: &str, default: u16| req[key].as_u64().map_or(default, |v| v.clamp(1, 10_000) as u16);
        let env: Option<&Map<String, Value>> = req["env"].as_object();
        let (spawned, argv) = pty::spawn(&self.base, &what, req["cwd"].as_str(), env, size("rows", 24), size("cols", 80))
            .map_err(|e| format!("{e:#}"))?;
        self.pending.push(Pending {
            conn: self.conns[i].serial,
            tag: tag.clone(),
            id: id.to_string(),
            pid: spawned.pid,
            cmd: argv.iter().map(|a| a.to_string_lossy().into_owned()).collect(),
            cwd: req["cwd"].as_str().unwrap_or("").to_string(),
            master: spawned.master,
            exec: spawned.exec,
            deadline: Instant::now() + self.settings.spawn_timeout,
            exit: None,
        });
        Ok(())
    }

    fn exec_heard(&mut self, k: usize) {
        let mut errno = 0 as libc::c_int;
        let size = std::mem::size_of::<libc::c_int>();
        let n = unsafe { libc::read(self.pending[k].exec.as_raw_fd(), (&mut errno as *mut libc::c_int).cast(), size) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
            return;
        }
        let p = self.pending.remove(k);
        if n == 0 {
            self.started(p);
            return;
        }
        let error = if n as usize == size { std::io::Error::from_raw_os_error(errno).to_string() } else { "exec failed".into() };
        let program = p.cmd.first().cloned().unwrap_or_default();
        self.spawn_failed(p, format!("cannot run {program}: {error}"));
    }

    fn spawn_timeouts(&mut self) {
        let now = Instant::now();
        while let Some(k) = self.pending.iter().position(|p| p.deadline <= now) {
            let p = self.pending.remove(k);
            if p.exit.is_none() {
                unsafe { libc::kill(p.pid, libc::SIGKILL) };
            }
            let program = p.cmd.first().cloned().unwrap_or_default();
            let after = self.settings.spawn_timeout;
            self.spawn_failed(p, format!("spawning {program} timed out after {after:?}"));
        }
    }

    fn spawn_failed(&mut self, p: Pending, error: String) {
        if p.exit.is_none() {
            self.closed.push(Closed { pid: p.pid, kill_at: None });
        }
        let mut reply = json!({"ok": false, "error": error});
        if !p.tag.is_null() {
            reply["req"] = p.tag;
        }
        if let Some(c) = self.conns.iter_mut().find(|c| c.serial == p.conn) {
            c.send(reply, vec![]);
        }
        self.reap();
    }

    fn started(&mut self, p: Pending) {
        let master = Rc::new(p.master);
        let mut reply = json!({"ok": true, "id": p.id, "pid": p.pid, "cmd": p.cmd, "fds": 1});
        if !p.tag.is_null() {
            reply["req"] = p.tag;
        }
        if let Some(c) = self.conns.iter_mut().find(|c| c.serial == p.conn) {
            c.send(reply, vec![master.clone()]);
        }
        self.ptys.insert(p.id.clone(), Pty { pid: p.pid, cmd: p.cmd, cwd: p.cwd, master, exit: p.exit });
        if let Some(exit) = p.exit {
            let event = exited_event(&p.id, p.pid, exit);
            for c in &mut self.conns {
                c.send(event.clone(), vec![]);
            }
        }
    }

    fn close(&mut self, id: &str) -> Result<(Value, Vec<Rc<OwnedFd>>), String> {
        if self.pending.iter().any(|p| p.id == id) {
            return Err(format!("pty {id:?} is still starting"));
        }
        let Some(p) = self.ptys.remove(id) else {
            return Err(format!("no pty {id:?}"));
        };
        if p.exit.is_none() {
            for group in session::groups(p.pid) {
                unsafe { libc::killpg(group, libc::SIGHUP) };
            }
            self.closed.push(Closed { pid: p.pid, kill_at: Some(Instant::now() + self.settings.close_grace) });
        }
        Ok((json!({}), vec![]))
    }

    fn begin_stop(&mut self) -> (Value, Vec<Rc<OwnedFd>>) {
        if self.stopping.is_none() {
            self.stopping = Some(Instant::now());
            eprintln!("kanna-keeper: stopping: closing {} pty(s)", self.ptys.values().filter(|p| p.exit.is_none()).count());
            let _ = std::fs::remove_file(paths::socket(&self.settings.state_dir));
            let ids: Vec<String> = self.ptys.keys().cloned().collect();
            for id in ids {
                let _ = self.close(&id);
            }
            for p in std::mem::take(&mut self.pending) {
                if p.exit.is_none() {
                    unsafe { libc::kill(p.pid, libc::SIGKILL) };
                }
                self.spawn_failed(p, "the keeper is stopping".into());
            }
        }
        (json!({}), vec![])
    }

    fn ack(&mut self, id: &str) -> Result<(Value, Vec<Rc<OwnedFd>>), String> {
        match self.ptys.get(id) {
            Some(p) if p.exit.is_none() => Err(format!("pty {id:?} is still running")),
            Some(_) => {
                self.ptys.remove(id);
                Ok((json!({}), vec![]))
            }
            None => Ok((json!({}), vec![])),
        }
    }

    fn reap(&mut self) {
        self.closed.retain(|c| c.kill_at.is_some() || wait(c.pid).is_none());
        for p in self.pending.iter_mut().filter(|p| p.exit.is_none()) {
            p.exit = wait(p.pid);
        }
        let mut events = Vec::new();
        for (id, p) in self.ptys.iter_mut().filter(|(_, p)| p.exit.is_none()) {
            if let Some(exit) = wait(p.pid) {
                p.exit = Some(exit);
                events.push(exited_event(id, p.pid, exit));
            }
        }
        for event in events {
            for c in &mut self.conns {
                c.send(event.clone(), vec![]);
            }
        }
    }
}

impl Keeper {
    fn upgrade(&mut self, (conn, tag, binary): (u64, Value, String)) {
        eprintln!("kanna-keeper: upgrading to {binary}");
        let error = self.try_upgrade(conn, &tag, Path::new(&binary));
        eprintln!("kanna-keeper: not upgraded: {error}");
        let mut reply = json!({"ok": false, "error": error});
        if !tag.is_null() {
            reply["req"] = tag;
        }
        if let Some(c) = self.conns.iter_mut().find(|c| c.serial == conn) {
            c.send(reply, vec![]);
        }
        for i in 0..self.conns.len() {
            self.handle_lines(i);
        }
    }

    fn try_upgrade(&mut self, conn: u64, tag: &Value, binary: &Path) -> String {
        self.reap();
        self.flush_all();
        let state = self.state(conn, tag);
        let mut file = match upgrade::write(&self.settings.state_dir, &state) {
            Ok(file) => file,
            Err(e) => return format!("can't write the state: {e}"),
        };
        if let Err(e) = upgrade::probe(binary, &mut file, PROBE_TIMEOUT) {
            return e;
        }
        upgrade::exec(binary, &self.settings.state_dir, &file, &state.fds())
    }

    fn flush_all(&mut self) {
        let deadline = Instant::now() + FLUSH_TIMEOUT;
        loop {
            for c in &mut self.conns {
                c.flush();
            }
            self.conns.retain(|c| !c.dead);
            let mut fds: Vec<_> =
                self.conns.iter().filter(|c| !c.out.is_empty()).map(|c| pollfd(c.stream.as_raw_fd(), libc::POLLOUT)).collect();
            if fds.is_empty() {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                eprintln!("kanna-keeper: dropped {} client(s) that weren't reading, to upgrade", fds.len());
                self.conns.retain(|c| c.out.is_empty());
                return;
            }
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, left.as_millis() as libc::c_int + 1) };
        }
    }

    fn state(&self, conn: u64, tag: &Value) -> State {
        let now = Instant::now();
        State {
            written: upgrade::monotonic(),
            lock: self.lock.as_ref().map_or(-1, |l| l.as_raw_fd()),
            listener: self.listener.as_raw_fd(),
            next_serial: self.next_serial,
            nofile: pty::original_nofile(),
            requester: Requester { conn, tag: tag.clone(), from: self.build.clone() },
            conns: self
                .conns
                .iter()
                .map(|c| ConnState { serial: c.serial, fd: c.stream.as_raw_fd(), inbuf: c.inbuf.clone() })
                .collect(),
            ptys: self
                .ptys
                .iter()
                .map(|(id, p)| PtyState {
                    id: id.clone(),
                    pid: p.pid,
                    cmd: p.cmd.clone(),
                    cwd: p.cwd.clone(),
                    master: p.master.as_raw_fd(),
                    exit: p.exit,
                })
                .collect(),
            pending: self
                .pending
                .iter()
                .map(|p| PendingState {
                    conn: p.conn,
                    tag: p.tag.clone(),
                    id: p.id.clone(),
                    pid: p.pid,
                    cmd: p.cmd.clone(),
                    cwd: p.cwd.clone(),
                    master: p.master.as_raw_fd(),
                    exec: p.exec.as_raw_fd(),
                    left: p.deadline.saturating_duration_since(now),
                    exit: p.exit,
                })
                .collect(),
            closed: self
                .closed
                .iter()
                .map(|c| ClosedState { pid: c.pid, kill_in: c.kill_at.map(|at| at.saturating_duration_since(now)) })
                .collect(),
        }
    }
}

fn wait(pid: i32) -> Option<ExitStatus> {
    let mut raw = 0;
    if unsafe { libc::waitpid(pid, &mut raw, libc::WNOHANG) } != pid {
        return None;
    }
    Some(if libc::WIFSIGNALED(raw) {
        ExitStatus::Signal(libc::WTERMSIG(raw))
    } else {
        ExitStatus::Code(libc::WEXITSTATUS(raw))
    })
}

fn exited_event(id: &str, pid: i32, exit: ExitStatus) -> Value {
    let mut v = json!({"exited": id, "pid": pid});
    v.as_object_mut().unwrap().extend(exit.to_json().as_object().unwrap().clone());
    v
}

fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return Err(format!("bad id {id:?}"));
    }
    Ok(())
}

pub fn detach(mut settings: Settings) -> Result<()> {
    paths::ensure_dir(&settings.state_dir).with_context(|| format!("state directory {}", settings.state_dir.display()))?;
    settings.state_dir = std::path::absolute(&settings.state_dir)?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(paths::log(&settings.state_dir))?;
    let mut fds = [-1; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    sig::set_cloexec(read.as_raw_fd());
    sig::set_cloexec(write.as_raw_fd());
    match unsafe { libc::fork() } {
        -1 => Err(std::io::Error::last_os_error().into()),
        0 => {
            drop(read);
            unsafe { libc::setsid() };
            match unsafe { libc::fork() } {
                -1 => unsafe { libc::_exit(1) },
                0 => {}
                _ => unsafe { libc::_exit(0) },
            }
            unsafe { libc::chdir(c"/".as_ptr()) };
            let null = std::fs::OpenOptions::new().read(true).write(true).open("/dev/null");
            unsafe {
                if let Ok(null) = &null {
                    libc::dup2(null.as_raw_fd(), 0);
                    libc::dup2(null.as_raw_fd(), 1);
                }
                libc::dup2(log.as_raw_fd(), 2);
            }
            drop(null);
            drop(log);
            sig::close_inherited(&[write.as_raw_fd()]);
            let code = match run(settings, Some(&write)) {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("kanna-keeper: {e:#}");
                    tell(Some(&write), format!("{}{e:#}", FAILED as char).as_bytes());
                    1
                }
            };
            std::process::exit(code);
        }
        child => {
            drop(write);
            let mut status = 0;
            unsafe { libc::waitpid(child, &mut status, 0) };
            let mut read = std::fs::File::from(read);
            let mut byte = [0u8; 1];
            match read.read(&mut byte) {
                Ok(1) if byte[0] == LISTENING || byte[0] == ALREADY_RUNNING => Ok(()),
                Ok(1) if byte[0] == FAILED => {
                    let mut error = String::new();
                    let _ = read.read_to_string(&mut error);
                    Err(anyhow::anyhow!(error))
                }
                _ => Err(anyhow::anyhow!(
                    "the keeper stopped before it listened; see {}",
                    paths::log(&settings.state_dir).display()
                )),
            }
        }
    }
}
