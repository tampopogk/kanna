use crate::{sig, ExitStatus};
use serde_json::{json, Value};
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const STATE_VERSION: u64 = 1;
pub const CAN_RESUME: &str = "kanna-keeper: can resume state";

const HELD: [libc::c_int; 3] = [libc::SIGCHLD, libc::SIGTERM, libc::SIGINT];

pub struct State {
    pub written: Duration,
    pub lock: RawFd,
    pub listener: RawFd,
    pub next_serial: u64,
    pub nofile: Option<(u64, u64)>,
    pub requester: Requester,
    pub from_dir: Option<String>,
    pub conns: Vec<ConnState>,
    pub ptys: Vec<PtyState>,
    pub pending: Vec<PendingState>,
    pub closed: Vec<ClosedState>,
}

pub struct Requester {
    pub conn: u64,
    pub tag: Value,
    pub from: Option<String>,
}

pub struct ConnState {
    pub serial: u64,
    pub fd: RawFd,
    pub inbuf: Vec<u8>,
}

pub struct PtyState {
    pub id: String,
    pub pid: i32,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub master: RawFd,
    pub exit: Option<ExitStatus>,
}

pub struct PendingState {
    pub conn: u64,
    pub tag: Value,
    pub id: String,
    pub pid: i32,
    pub cmd: Vec<String>,
    pub cwd: String,
    pub master: RawFd,
    pub exec: RawFd,
    pub left: Duration,
    pub exit: Option<ExitStatus>,
}

pub struct ClosedState {
    pub pid: i32,
    pub kill_in: Option<Duration>,
}

fn with_exit(mut v: Value, exit: Option<ExitStatus>) -> Value {
    if let Some(exit) = exit {
        v["exit"] = exit.to_json();
    }
    v
}

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

impl State {
    pub fn fds(&self) -> Vec<RawFd> {
        let mut fds = vec![self.lock, self.listener];
        fds.extend(self.conns.iter().map(|c| c.fd));
        fds.extend(self.ptys.iter().map(|p| p.master));
        fds.extend(self.pending.iter().flat_map(|p| [p.master, p.exec]));
        fds
    }

    pub fn to_json(&self) -> Value {
        json!({
            "state": STATE_VERSION,
            "written_ns": self.written.as_nanos() as u64,
            "lock": self.lock,
            "listener": self.listener,
            "next_serial": self.next_serial,
            "nofile": self.nofile.map(|(cur, max)| json!([cur, max])),
            "requester": {"conn": self.requester.conn, "req": self.requester.tag, "from": self.requester.from},
            "from_dir": self.from_dir,
            "conns": self.conns.iter().map(|c| json!({"serial": c.serial, "fd": c.fd, "inbuf": c.inbuf})).collect::<Vec<_>>(),
            "ptys": self.ptys.iter().map(|p| with_exit(
                json!({"id": p.id, "pid": p.pid, "cmd": p.cmd, "cwd": p.cwd, "master": p.master}),
                p.exit,
            )).collect::<Vec<_>>(),
            "pending": self.pending.iter().map(|p| with_exit(
                json!({"conn": p.conn, "req": p.tag, "id": p.id, "pid": p.pid, "cmd": p.cmd, "cwd": p.cwd,
                       "master": p.master, "exec": p.exec, "left_ms": ms(p.left)}),
                p.exit,
            )).collect::<Vec<_>>(),
            "closed": self.closed.iter().map(|c| json!({"pid": c.pid, "kill_in_ms": c.kill_in.map(ms)})).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Result<State, String> {
        match v["state"].as_u64() {
            Some(n) if n <= STATE_VERSION && n >= 1 => {}
            Some(n) => return Err(format!("state {n} is newer than this keeper reads ({STATE_VERSION})")),
            None => return Err("not a keeper's state".into()),
        }
        let nofile = match &v["nofile"] {
            Value::Null => None,
            n => Some((num(n, 0)?, num(n, 1)?)),
        };
        let requester = &v["requester"];
        Ok(State {
            written: Duration::from_nanos(num(v, "written_ns")?),
            lock: fd(v, "lock")?,
            listener: fd(v, "listener")?,
            next_serial: num(v, "next_serial")?,
            nofile,
            requester: Requester {
                conn: num(requester, "conn")?,
                tag: requester["req"].clone(),
                from: requester["from"].as_str().map(String::from),
            },
            from_dir: v["from_dir"].as_str().map(String::from),
            conns: list(v, "conns")?
                .iter()
                .map(|c| {
                    let inbuf = list(c, "inbuf")?
                        .iter()
                        .map(|b| b.as_u64().filter(|&b| b < 256).map(|b| b as u8).ok_or("inbuf: not a byte"))
                        .collect::<Result<_, _>>()?;
                    Ok(ConnState { serial: num(c, "serial")?, fd: fd(c, "fd")?, inbuf })
                })
                .collect::<Result<_, String>>()?,
            ptys: list(v, "ptys")?
                .iter()
                .map(|p| {
                    Ok(PtyState {
                        id: text(p, "id")?,
                        pid: pid(p)?,
                        cmd: strings(p)?,
                        cwd: text(p, "cwd")?,
                        master: fd(p, "master")?,
                        exit: exit(p)?,
                    })
                })
                .collect::<Result<_, String>>()?,
            pending: list(v, "pending")?
                .iter()
                .map(|p| {
                    Ok(PendingState {
                        conn: num(p, "conn")?,
                        tag: p["req"].clone(),
                        id: text(p, "id")?,
                        pid: pid(p)?,
                        cmd: strings(p)?,
                        cwd: text(p, "cwd")?,
                        master: fd(p, "master")?,
                        exec: fd(p, "exec")?,
                        left: Duration::from_millis(num(p, "left_ms")?),
                        exit: exit(p)?,
                    })
                })
                .collect::<Result<_, String>>()?,
            closed: list(v, "closed")?
                .iter()
                .map(|c| {
                    let kill_in = match &c["kill_in_ms"] {
                        Value::Null => None,
                        _ => Some(Duration::from_millis(num(c, "kill_in_ms")?)),
                    };
                    Ok(ClosedState { pid: pid(c)?, kill_in })
                })
                .collect::<Result<_, String>>()?,
        })
    }
}

fn num<I: serde_json::value::Index + std::fmt::Display + Copy>(v: &Value, key: I) -> Result<u64, String> {
    v.get(key).and_then(Value::as_u64).ok_or_else(|| format!("{key}: not a number"))
}

fn fd(v: &Value, key: &str) -> Result<RawFd, String> {
    num(v, key).and_then(|n| RawFd::try_from(n).map_err(|_| format!("{key}: not a descriptor")))
}

fn pid(v: &Value) -> Result<i32, String> {
    num(v, "pid").and_then(|n| i32::try_from(n).ok().filter(|&p| p > 0).ok_or_else(|| "pid: out of range".into()))
}

fn text(v: &Value, key: &str) -> Result<String, String> {
    v[key].as_str().map(String::from).ok_or_else(|| format!("{key}: not a string"))
}

fn list<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    v[key].as_array().ok_or_else(|| format!("{key}: not a list"))
}

fn strings(v: &Value) -> Result<Vec<String>, String> {
    list(v, "cmd")?.iter().map(|s| s.as_str().map(String::from).ok_or_else(|| "cmd: not strings".into())).collect()
}

fn exit(v: &Value) -> Result<Option<ExitStatus>, String> {
    match v.get("exit") {
        None | Some(Value::Null) => Ok(None),
        Some(e) => ExitStatus::from_json(e).map(Some).ok_or_else(|| "exit: neither code nor signal".into()),
    }
}

pub fn monotonic() -> Duration {
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

#[cfg(debug_assertions)]
fn test_probe_delay() {
    if let Some(ms) = std::env::var("KANNA_KEEPER_TEST_PROBE_DELAY_MS").ok().and_then(|v| v.parse().ok()) {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

#[cfg(not(debug_assertions))]
fn test_probe_delay() {}

pub fn write(dir: &Path, state: &State) -> std::io::Result<File> {
    let path = dir.join(format!("keeper.upgrade.{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&path)?;
    let _ = std::fs::remove_file(&path);
    file.write_all(&serde_json::to_vec(&state.to_json()).map_err(std::io::Error::other)?)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

pub fn read(file: &mut File) -> Result<State, String> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| format!("reading the state: {e}"))?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("the state isn't JSON: {e}"))?;
    State::from_json(&value)
}

pub fn check() -> i32 {
    test_probe_delay();
    let mut stdin = unsafe { File::from_raw_fd(0) };
    let result = read(&mut stdin);
    std::mem::forget(stdin);
    match result {
        Ok(_) => {
            println!("{CAN_RESUME} {STATE_VERSION}");
            0
        }
        Err(e) => {
            eprintln!("kanna-keeper: can't resume this state: {e}");
            1
        }
    }
}

pub fn probe(binary: &Path, state: &mut File, timeout: Duration) -> Result<(), String> {
    state.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let stdin = state.try_clone().map_err(|e| e.to_string())?;
    let mut child = Command::new(binary)
        .arg("--check-upgrade")
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("can't run {}: {e}", binary.display()))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} didn't answer in {timeout:?}", binary.display()));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let (mut out, mut err) = (String::new(), String::new());
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut err);
    }
    state.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let expected = format!("{CAN_RESUME} {STATE_VERSION}");
    if status.success() && out.lines().next() == Some(expected.as_str()) {
        return Ok(());
    }
    let said = err.lines().chain(out.lines()).find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
    Err(format!("{} can't take over this keeper ({status}){}", binary.display(), if said.is_empty() { String::new() } else { format!(": {said}") }))
}

fn set_held(how: libc::c_int) {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signo in HELD {
            libc::sigaddset(&mut set, signo);
        }
        libc::pthread_sigmask(how, &set, std::ptr::null_mut());
    }
}

fn cstring(bytes: &[u8]) -> Result<CString, String> {
    CString::new(bytes).map_err(|_| "an argument or environment entry contains NUL".to_string())
}

pub fn exec(binary: &Path, state_dir: &Path, state: &File, fds: &[RawFd]) -> String {
    let built = (|| {
        let args = [
            cstring(binary.as_os_str().as_bytes())?,
            cstring(b"--state-dir")?,
            cstring(state_dir.as_os_str().as_bytes())?,
            cstring(b"--resume")?,
            cstring(state.as_raw_fd().to_string().as_bytes())?,
        ];
        let env = std::env::vars_os()
            .map(|(k, v)| {
                let mut kv = k.as_bytes().to_vec();
                kv.push(b'=');
                kv.extend_from_slice(v.as_bytes());
                cstring(&kv)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok::<_, String>((args, env))
    })();
    let (args, env) = match built {
        Ok(built) => built,
        Err(e) => return e,
    };
    let mut argv: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let mut envp: Vec<*const libc::c_char> = env.iter().map(|e| e.as_ptr()).collect();
    envp.push(std::ptr::null());
    let carried: Vec<RawFd> = fds.iter().copied().chain([state.as_raw_fd()]).collect();
    for &fd in &carried {
        sig::clear_cloexec(fd);
    }
    set_held(libc::SIG_BLOCK);
    unsafe { libc::execve(args[0].as_ptr(), argv.as_ptr(), envp.as_ptr()) };
    let error = std::io::Error::last_os_error();
    for &fd in &carried {
        sig::set_cloexec(fd);
    }
    set_held(libc::SIG_UNBLOCK);
    format!("can't run {}: {error}", binary.display())
}

pub fn open(fds: &[RawFd]) -> Result<(), String> {
    for &fd in fds {
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
            return Err(format!("descriptor {fd} didn't cross the exec"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> State {
        State {
            written: Duration::from_nanos(123_456_789),
            lock: 3,
            listener: 4,
            next_serial: 9,
            nofile: Some((256, u64::MAX)),
            requester: Requester { conn: 2, tag: json!(7), from: Some("abc".into()) },
            from_dir: Some("/old".into()),
            conns: vec![ConnState { serial: 2, fd: 5, inbuf: b"{\"op\":\"hel".to_vec() }],
            ptys: vec![
                PtyState { id: "t1".into(), pid: 100, cmd: vec!["-sh".into()], cwd: "/".into(), master: 6, exit: None },
                PtyState { id: "t2".into(), pid: 101, cmd: vec![], cwd: "".into(), master: 7, exit: Some(ExitStatus::Signal(9)) },
            ],
            pending: vec![PendingState {
                conn: 2,
                tag: json!(3),
                id: "t3".into(),
                pid: 102,
                cmd: vec!["zsh".into()],
                cwd: "/tmp".into(),
                master: 8,
                exec: 10,
                left: Duration::from_millis(4200),
                exit: Some(ExitStatus::Code(3)),
            }],
            closed: vec![ClosedState { pid: 103, kill_in: Some(Duration::from_millis(500)) }, ClosedState { pid: 104, kill_in: None }],
        }
    }

    #[test]
    fn a_state_reads_back_as_written() {
        let state = sample();
        let again = State::from_json(&state.to_json()).unwrap();
        assert_eq!(again.to_json(), state.to_json());
        assert_eq!(again.fds(), [3, 4, 5, 6, 7, 8, 10]);
    }

    #[test]
    fn a_newer_or_broken_state_is_refused() {
        let mut v = sample().to_json();
        v["state"] = json!(STATE_VERSION + 1);
        assert!(State::from_json(&v).err().unwrap().contains("newer"));
        let mut v = sample().to_json();
        v["ptys"][0]["master"] = json!("six");
        assert!(State::from_json(&v).is_err());
        assert!(State::from_json(&json!({"hello": 1})).is_err());
    }
}
