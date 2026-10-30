use kanna_keeper::client::{Client, Exited, SpawnRequest};
use kanna_keeper::ExitStatus;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_kanna-keeper");

static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

struct Fixture {
    dir: PathBuf,
    keeper: Option<Child>,
}

fn private_dir() -> PathBuf {
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("kk{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("home")).unwrap();
    dir.canonicalize().unwrap()
}

impl Fixture {
    fn new() -> Self {
        Self::with(&[])
    }

    fn with(env: &[(&str, &str)]) -> Self {
        let dir = private_dir();
        let mut cmd = Command::new(BIN);
        cmd.arg("--state-dir").arg(dir.join("state"));
        cmd.env_clear();
        for key in ["PATH", "USER", "LOGNAME", "TMPDIR"] {
            if let Some(v) = std::env::var_os(key) {
                cmd.env(key, v);
            }
        }
        cmd.env("HOME", dir.join("home"))
            .env("SHELL", "/bin/sh")
            .env("PS1", "$ ")
            .env("KANNA_KEEPER_IDLE_MS", "30000")
            .env("KANNA_KEEPER_CLOSE_GRACE_MS", "500");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let keeper = cmd.stdin(Stdio::null()).spawn().unwrap();
        let f = Fixture { dir, keeper: Some(keeper) };
        wait_for("the keeper's socket", || f.socket().exists() && Client::connect(&f.socket()).is_ok());
        f
    }

    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn socket(&self) -> PathBuf {
        self.state().join("keeper.sock")
    }

    fn client(&self) -> Client {
        Client::connect(&self.socket()).unwrap()
    }

    fn keeper_pid(&self) -> i32 {
        self.keeper.as_ref().unwrap().id() as i32
    }

    fn keeper_exited(&mut self) -> bool {
        self.keeper.as_mut().unwrap().try_wait().unwrap().is_some()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(mut k) = self.keeper.take() {
            let _ = k.kill();
            let _ = k.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[track_caller]
fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[track_caller]
fn read_until(master: &OwnedFd, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut out = Vec::new();
    loop {
        if String::from_utf8_lossy(&out).contains(needle) {
            return String::from_utf8_lossy(&out).into_owned();
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "no {needle:?} in {:?}", String::from_utf8_lossy(&out));
        let mut pfd = libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, left.as_millis() as i32) } <= 0 {
            continue;
        }
        let mut buf = [0u8; 4096];
        let n = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        assert!(n > 0, "pty closed; got {:?} without {needle:?}", String::from_utf8_lossy(&out));
        out.extend_from_slice(&buf[..n as usize]);
    }
}

fn write(master: &OwnedFd, s: &str) {
    let n = unsafe { libc::write(master.as_raw_fd(), s.as_ptr().cast(), s.len()) };
    assert_eq!(n, s.len() as isize);
}

#[track_caller]
fn next_exit(c: &mut Client) -> Exited {
    c.wait_event(Some(Duration::from_secs(5))).unwrap().expect("an exit event")
}

#[track_caller]
fn exit_reading(c: &mut Client, master: &OwnedFd) -> (Exited, String) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut out = Vec::new();
    loop {
        if let Some(e) = c.poll_events().unwrap().into_iter().next() {
            return (e, String::from_utf8_lossy(&out).into_owned());
        }
        assert!(Instant::now() < deadline, "no exit; read {:?}", String::from_utf8_lossy(&out));
        let mut pfd = libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, 50) } > 0 {
            let mut buf = [0u8; 4096];
            let n = unsafe { libc::read(master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                out.extend_from_slice(&buf[..n as usize]);
            } else {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

#[test]
fn spawn_then_read_and_write_through_the_fd() {
    let f = Fixture::new();
    let mut c = f.client();
    let mut req = SpawnRequest::command("t1", "printf 'ready\\n'; read line; echo \"got:$line\"; sleep 30");
    req.cols = 100;
    req.rows = 30;
    let pty = c.spawn(&req).unwrap();
    assert!(pty.pid > 0 && alive(pty.pid));
    assert_eq!(pty.cmd[1..], ["-l", "-i", "-c", req.command.as_deref().unwrap()]);
    read_until(&pty.master, "ready");
    write(&pty.master, "hello\n");
    read_until(&pty.master, "got:hello");
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    unsafe { libc::ioctl(pty.master.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
    assert_eq!((ws.ws_col, ws.ws_row), (100, 30));
    c.close("t1").unwrap();
}

#[test]
fn shell_gets_the_login_convention_env_and_cwd() {
    let f = Fixture::new();
    let mut c = f.client();
    let mut req = SpawnRequest::shell("sh");
    req.shell = Some("/bin/sh".into());
    req.cwd = Some(f.dir.clone());
    req.env.insert("KANNA_TEST_VAR".into(), Some("hi there".into()));
    req.env.insert("KANNA_KEEPER_IDLE_MS".into(), None);
    let pty = c.spawn(&req).unwrap();
    assert_eq!(pty.cmd, ["/bin/sh"]);
    write(&pty.master, "echo \"[$0][$KANNA_TEST_VAR][${KANNA_KEEPER_IDLE_MS-unset}][$(pwd -P)][$TERM]\"\n");
    let out = read_until(&pty.master, "]\r\n");
    assert!(
        out.contains(&format!("[-sh][hi there][unset][{}][xterm-256color]", f.dir.display())),
        "{out:?}"
    );
    c.close("sh").unwrap();
}

#[test]
fn a_command_that_hands_over_skips_the_rc_files() {
    let f = Fixture::new();
    let mut c = f.client();
    let mut req = SpawnRequest::command("handover", "case $- in *i*) echo with-rc;; *) echo without-rc;; esac; sleep 30");
    req.skip_rc = true;
    let pty = c.spawn(&req).unwrap();
    assert_eq!(pty.cmd[1..], ["-l", "-c", req.command.as_deref().unwrap()]);
    read_until(&pty.master, "without-rc");
    c.close("handover").unwrap();
}

#[test]
fn exit_codes_and_signals_are_the_commands_own() {
    let f = Fixture::new();
    let mut c = f.client();
    let p3 = c.spawn(&SpawnRequest::command("three", "exit 3")).unwrap();
    let e = exit_reading(&mut c, &p3.master).0;
    assert_eq!(e, Exited { id: "three".into(), pid: p3.pid, status: ExitStatus::Code(3) });
    let p9 = c.spawn(&SpawnRequest::command("nine", "kill -9 $$")).unwrap();
    let e = next_exit(&mut c);
    assert_eq!(e, Exited { id: "nine".into(), pid: p9.pid, status: ExitStatus::Signal(9) });
    assert_eq!(e.status.shell_status(), 137);
    let missing = c.spawn(&SpawnRequest::command("missing", "/nonexistent/x")).unwrap();
    assert_eq!(exit_reading(&mut c, &missing.master).0.status, ExitStatus::Code(127));
    let mut req = SpawnRequest::shell("shell");
    req.shell = Some("/bin/sh".into());
    let sh = c.spawn(&req).unwrap();
    write(&sh.master, "exit 4\n");
    assert_eq!(exit_reading(&mut c, &sh.master).0.status, ExitStatus::Code(4));
    for id in ["three", "nine", "missing", "shell"] {
        c.ack(id).unwrap();
    }
    assert!(c.adopt().unwrap().is_empty());
}

#[test]
fn adopt_after_the_client_goes_returns_the_same_process() {
    let f = Fixture::new();
    let (pid, cmd) = {
        let mut c = f.client();
        let pty = c
            .spawn(&SpawnRequest::command("t1", "sleep 0.5; echo while-away; read x; echo \"got:$x\"; sleep 30"))
            .unwrap();
        (pty.pid, pty.cmd)
    };
    std::thread::sleep(Duration::from_millis(1000));
    assert!(alive(pid));
    let mut c = f.client();
    let ptys = c.adopt().unwrap();
    assert_eq!(ptys.len(), 1);
    let pty = &ptys[0];
    assert_eq!((pty.id.as_str(), pty.pid, &pty.cmd, pty.exit), ("t1", pid, &cmd, None));
    read_until(&pty.master, "while-away");
    write(&pty.master, "back\n");
    read_until(&pty.master, "got:back");
    let again = c.adopt().unwrap();
    assert_eq!(again[0].pid, pid);
    c.close("t1").unwrap();
}

#[test]
fn an_exit_while_nobody_is_connected_waits_for_an_ack() {
    let f = Fixture::new();
    let (quiet, chatty) = {
        let mut c = f.client();
        let quiet = c.spawn(&SpawnRequest::command("quiet", "(exit 6)")).unwrap().pid;
        let chatty = c.spawn(&SpawnRequest::command("chatty", "sleep 0.2; echo last-words; exit 5")).unwrap().pid;
        (quiet, chatty)
    };
    wait_for("the quiet one to be reaped", || !alive(quiet));
    std::thread::sleep(Duration::from_millis(500));
    let mut c = f.client();
    assert_eq!(next_exit(&mut c), Exited { id: "quiet".into(), pid: quiet, status: ExitStatus::Code(6) });
    let ptys = c.adopt().unwrap();
    assert_eq!(ptys.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["chatty", "quiet"]);
    assert_eq!(ptys[1].exit, Some(ExitStatus::Code(6)));
    let (e, out) = exit_reading(&mut c, &ptys[0].master);
    assert_eq!(e, Exited { id: "chatty".into(), pid: chatty, status: ExitStatus::Code(5) });
    assert!(out.contains("last-words"), "{out:?}");
    drop(ptys);
    drop(c);
    let mut c = f.client();
    let mut told = vec![next_exit(&mut c).id, next_exit(&mut c).id];
    told.sort();
    assert_eq!(told, ["chatty", "quiet"]);
    c.ack("chatty").unwrap();
    c.ack("quiet").unwrap();
    drop(c);
    let mut c = f.client();
    assert_eq!(c.wait_event(Some(Duration::from_millis(300))).unwrap(), None);
    assert!(c.adopt().unwrap().is_empty());
    c.spawn(&SpawnRequest::command("t2", "sleep 30")).unwrap();
    assert!(c.ack("t2").is_err());
    c.close("t2").unwrap();
}

fn group(c: &mut Client, id: &str, script: &str) -> (i32, i32) {
    let pty = c.spawn(&SpawnRequest::command(id, script)).unwrap();
    let mut out = read_until(&pty.master, "kid:");
    while !out.rsplit("kid:").next().unwrap_or_default().contains('\n') {
        out += &read_until(&pty.master, "\n");
    }
    let kid = out.lines().find_map(|l| l.trim().strip_prefix("kid:")).unwrap_or_else(|| panic!("{out:?}")).parse().unwrap();
    (pty.pid, kid)
}

#[test]
fn close_ends_the_process_group() {
    let f = Fixture::new();
    let mut c = f.client();
    let (sh, kid) = group(&mut c, "plain", "sleep 60 & echo \"kid:$!\"; wait");
    let (sh2, kid2) = group(&mut c, "stubborn", "trap '' HUP; sleep 60 & echo \"kid:$!\"; wait");
    assert!(alive(sh) && alive(kid) && alive(sh2) && alive(kid2));
    c.close("plain").unwrap();
    c.close("stubborn").unwrap();
    for pid in [sh, kid, sh2, kid2] {
        wait_for(&format!("{pid} to end"), || !alive(pid));
    }
    let quick: Vec<i32> = (0..20)
        .map(|i| {
            let id = format!("quick{i}");
            let pid = c.spawn(&SpawnRequest::command(&id, "sleep 60")).unwrap().pid;
            c.close(&id).unwrap();
            pid
        })
        .collect();
    for pid in quick {
        wait_for(&format!("{pid} to end"), || !alive(pid));
    }
    assert_eq!(c.wait_event(Some(Duration::from_millis(300))).unwrap(), None);
    assert!(c.adopt().unwrap().is_empty());
    assert!(c.close("plain").is_err());
    c.spawn(&SpawnRequest::command("plain", "true")).unwrap();
    assert_eq!(next_exit(&mut c).status, ExitStatus::Code(0));
}

#[test]
fn errors_are_replies() {
    let f = Fixture::new();
    let mut c = f.client();
    c.spawn(&SpawnRequest::command("t1", "sleep 30")).unwrap();
    let dup = c.spawn(&SpawnRequest::command("t1", "true")).unwrap_err();
    assert!(dup.to_string().contains("exists"), "{dup}");
    assert!(c.spawn(&SpawnRequest::command("", "true")).is_err());
    let mut both = SpawnRequest::command("t2", "true");
    both.shell = Some("/bin/sh".into());
    assert!(c.spawn(&both).is_err());
    assert!(c.close("nope").is_err());
    assert_eq!(c.hello().unwrap().0, kanna_keeper::PROTOCOL_VERSION);
    c.close("t1").unwrap();
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "needs the keeper's debug-build test hooks")]
fn private_files_and_the_peer_check() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let mut c = f.client();
    let (version, pid) = c.hello().unwrap();
    assert_eq!((version, pid), (kanna_keeper::PROTOCOL_VERSION, f.keeper_pid()));
    let pid_file = std::fs::read_to_string(f.state().join("keeper.pid")).unwrap();
    assert_eq!(pid_file.trim(), pid.to_string());
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&f.state()), 0o700);
    assert_eq!(mode(&f.socket()), 0o600);
    let status = Command::new(BIN).arg("--state-dir").arg(f.state()).status().unwrap();
    assert!(status.success());
    assert_eq!(f.client().hello().unwrap().1, pid);

    let uid = unsafe { libc::getuid() } + 1;
    let other = Fixture::with(&[("KANNA_KEEPER_TEST_PEER_UID", &uid.to_string())]);
    let mut c = Client::connect(&other.socket()).unwrap();
    assert!(c.hello().is_err());
}

#[test]
fn idle_exit_only_without_processes_or_clients() {
    let mut f = Fixture::with(&[("KANNA_KEEPER_IDLE_MS", "300")]);
    let mut c = f.client();
    c.spawn(&SpawnRequest::command("t1", "sleep 30")).unwrap();
    drop(c);
    std::thread::sleep(Duration::from_millis(800));
    assert!(!f.keeper_exited());
    let mut c = f.client();
    c.close("t1").unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert!(!f.keeper_exited());
    drop(c);
    wait_for("the keeper to exit", || f.keeper_exited());
    assert!(!f.socket().exists());
    assert!(!f.state().join("keeper.pid").exists());
}

#[test]
fn connect_or_start_runs_one_detached_keeper() {
    let dir = private_dir();
    let state = dir.join("state");
    let mut c = Client::connect_or_start(&state, Path::new(BIN)).unwrap();
    let (_, pid) = c.hello().unwrap();
    let file: i32 = std::fs::read_to_string(state.join("keeper.pid")).unwrap().trim().parse().unwrap();
    assert_eq!(pid, file);
    let mut c2 = Client::connect_or_start(&state, Path::new(BIN)).unwrap();
    assert_eq!(c2.hello().unwrap().1, pid);
    let pty = c.spawn(&SpawnRequest::command("t1", "echo detached-ok; sleep 30")).unwrap();
    read_until(&pty.master, "detached-ok");
    c.close("t1").unwrap();
    unsafe { libc::kill(pid, libc::SIGTERM) };
    wait_for("the detached keeper to stop", || !alive(pid));
    let _ = std::fs::remove_dir_all(&dir);
}

struct KilledOnDrop(Child);

impl Drop for KilledOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_keeper_started_with_its_signals_blocked_still_stops_on_sigterm() {
    use std::os::unix::process::CommandExt;
    let dir = private_dir();
    let mut cmd = Command::new(BIN);
    cmd.arg("--state-dir").arg(dir.join("state")).stdin(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signo in [libc::SIGTERM, libc::SIGINT, libc::SIGCHLD] {
                libc::sigaddset(&mut set, signo);
            }
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            Ok(())
        });
    }
    let mut keeper = KilledOnDrop(cmd.spawn().unwrap());
    let socket = dir.join("state/keeper.sock");
    wait_for("the keeper's socket", || Client::connect(&socket).is_ok());
    let mut c = Client::connect(&socket).unwrap();
    c.set_timeout(Some(Duration::from_secs(5))).unwrap();
    let pty = c.spawn(&SpawnRequest::command("t1", "exit 6")).unwrap();
    assert_eq!(exit_reading(&mut c, &pty.master).0.status, ExitStatus::Code(6));
    unsafe { libc::kill(keeper.0.id() as i32, libc::SIGTERM) };
    wait_for("the keeper to stop on SIGTERM", || keeper.0.try_wait().unwrap().is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_request_to_a_keeper_that_doesnt_answer_times_out() {
    let dir = private_dir();
    let socket = dir.join("mute.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let mut c = Client::connect(&socket).unwrap();
    let _held = listener.accept().unwrap();
    c.set_timeout(Some(Duration::from_millis(200))).unwrap();
    assert_eq!(c.hello().unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    let _ = std::fs::remove_dir_all(&dir);
}

fn clean_command(state: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.arg("--state-dir").arg(state).env_clear().stdin(Stdio::null());
    for key in ["PATH", "USER", "LOGNAME", "TMPDIR"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.env("HOME", home).env("SHELL", "/bin/sh");
    cmd
}

fn with_inherited_fd(cmd: &mut Command, target: i32) -> OwnedFd {
    use std::os::fd::FromRawFd;
    use std::os::unix::process::CommandExt;
    let mut fds = [-1; 2];
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    for fd in fds {
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let read = fds[0];
    unsafe {
        cmd.pre_exec(move || {
            libc::dup2(read, target);
            Ok(())
        });
        libc::close(fds[1]);
        OwnedFd::from_raw_fd(read)
    }
}

const FD_PROBE: &str = "if [ -e /dev/fd/57 ]; then echo fd57-leaked; else echo fd57-clean; fi; sleep 30";

#[test]
fn spawned_processes_inherit_no_descriptor_the_keeper_was_started_with() {
    let dir = private_dir();
    let state = dir.join("state");
    let mut cmd = clean_command(&state, &dir.join("home"));
    let _pipe = with_inherited_fd(&mut cmd, 57);
    let _keeper = KilledOnDrop(cmd.spawn().unwrap());
    wait_for("the keeper's socket", || Client::connect(&state.join("keeper.sock")).is_ok());
    let mut c = Client::connect(&state.join("keeper.sock")).unwrap();
    let pty = c.spawn(&SpawnRequest::command("t1", FD_PROBE)).unwrap();
    read_until(&pty.master, "fd57-clean");
    c.close("t1").unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_detached_keeper_closes_what_it_inherited() {
    let dir = private_dir();
    let state = dir.join("state");
    let mut cmd = clean_command(&state, &dir.join("home"));
    cmd.arg("--detach");
    let _pipe = with_inherited_fd(&mut cmd, 57);
    assert!(cmd.status().unwrap().success());
    let mut c = Client::connect(&state.join("keeper.sock")).unwrap();
    let pid = c.hello().unwrap().1;
    let pty = c.spawn(&SpawnRequest::command("t1", FD_PROBE)).unwrap();
    let out = std::panic::catch_unwind(|| read_until(&pty.master, "fd57-clean"));
    c.close("t1").unwrap();
    unsafe { libc::kill(pid, libc::SIGTERM) };
    wait_for("the detached keeper to stop", || !alive(pid));
    out.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn detach_fails_when_the_keeper_cannot_listen() {
    let dir = private_dir();
    let state = dir.join("state");
    std::fs::create_dir_all(state.join("keeper.sock/in-the-way")).unwrap();
    let out = clean_command(&state, &dir.join("home")).arg("--detach").output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("keeper.sock"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_connect_or_start_all_reach_one_keeper() {
    for _ in 0..5 {
        let dir = private_dir();
        let state = dir.join("state");
        let threads: Vec<_> = (0..12)
            .map(|_| {
                let state = state.clone();
                std::thread::spawn(move || {
                    let mut c = Client::connect_or_start(&state, Path::new(BIN)).map_err(|e| e.to_string())?;
                    c.set_timeout(Some(Duration::from_secs(5))).unwrap();
                    c.hello().map(|h| h.1).map_err(|e| e.to_string())
                })
            })
            .collect();
        let pids: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        let keeper: Option<i32> =
            std::fs::read_to_string(state.join("keeper.pid")).ok().and_then(|p| p.trim().parse().ok());
        if let Some(pid) = keeper {
            unsafe { libc::kill(pid, libc::SIGTERM) };
            wait_for("the detached keeper to stop", || !alive(pid));
        }
        let _ = std::fs::remove_dir_all(&dir);
        for pid in &pids {
            assert_eq!(pid.as_ref().ok(), keeper.as_ref(), "{pids:?}");
        }
    }
}

#[test]
fn a_connection_made_while_the_keeper_goes_idle_is_served() {
    let dir = private_dir();
    let state = dir.join("state");
    let socket = state.join("keeper.sock");
    let mut keeper =
        KilledOnDrop(clean_command(&state, &dir.join("home")).env("KANNA_KEEPER_IDLE_MS", "3").spawn().unwrap());
    let mut served = 0;
    let mut i = 0u64;
    while keeper.0.try_wait().unwrap().is_none() {
        i += 1;
        let Ok(mut c) = Client::connect(&socket) else { continue };
        c.set_timeout(Some(Duration::from_secs(5))).unwrap();
        if let Err(e) = c.hello() {
            panic!("connected after {served} served, then: {e}");
        }
        served += 1;
        drop(c);
        std::thread::sleep(Duration::from_micros((i % 9) * 700));
        assert!(served < 20_000);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn cpu_seconds(pid: i32) -> f64 {
    let out = Command::new("ps").args(["-o", "time=", "-p", &pid.to_string()]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().split(':').rev().zip([1.0, 60.0, 3600.0]).map(|(f, scale)| f.parse::<f64>().unwrap() * scale).sum()
}

#[test]
fn out_of_descriptors_the_keeper_waits_instead_of_spinning() {
    use std::os::unix::process::CommandExt;
    let dir = private_dir();
    let state = dir.join("state");
    let mut cmd = clean_command(&state, &dir.join("home"));
    unsafe {
        cmd.pre_exec(|| {
            let limit = libc::rlimit { rlim_cur: 16, rlim_max: 16 };
            libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
            Ok(())
        });
    }
    let keeper = KilledOnDrop(cmd.spawn().unwrap());
    let socket = state.join("keeper.sock");
    wait_for("the keeper's socket", || Client::connect(&socket).is_ok());
    let clients: Vec<_> = (0..14).map(|_| Client::connect(&socket).unwrap()).collect();
    std::thread::sleep(Duration::from_millis(300));
    let pid = keeper.0.id() as i32;
    let before = cpu_seconds(pid);
    std::thread::sleep(Duration::from_secs(1));
    let used = cpu_seconds(pid) - before;
    drop(clients);
    let mut c = Client::connect(&socket).unwrap();
    c.set_timeout(Some(Duration::from_secs(5))).unwrap();
    assert!(c.hello().is_ok());
    let _ = std::fs::remove_dir_all(&dir);
    assert!(used < 0.3, "the keeper used {used} s of CPU in 1 s");
}

#[test]
fn adopt_hands_back_every_pty_even_near_the_descriptor_limit() {
    use std::os::unix::process::CommandExt;
    let dir = private_dir();
    let state = dir.join("state");
    let mut cmd = clean_command(&state, &dir.join("home"));
    unsafe {
        cmd.pre_exec(|| {
            let mut limit: libc::rlimit = std::mem::zeroed();
            libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit);
            limit.rlim_cur = 34;
            libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
            Ok(())
        });
    }
    let _keeper = KilledOnDrop(cmd.spawn().unwrap());
    let socket = state.join("keeper.sock");
    wait_for("the keeper's socket", || Client::connect(&socket).is_ok());
    let mut c = Client::connect(&socket).unwrap();
    c.set_timeout(Some(Duration::from_secs(5))).unwrap();
    let ids: Vec<String> = (0..40).map(|i| format!("p{i:02}")).collect();
    for id in &ids {
        drop(c.spawn(&SpawnRequest::command(id, "sleep 60")).unwrap());
    }
    let probe = c.spawn(&SpawnRequest::command("limit", "echo \"limit:$(ulimit -n)\"; sleep 60")).unwrap();
    read_until(&probe.master, "limit:34");
    c.close("limit").unwrap();
    let adopted = c.adopt();
    for id in &ids {
        let _ = c.close(id);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let adopted: Vec<String> = adopted.unwrap().into_iter().map(|p| p.id).collect();
    assert_eq!(adopted, ids);
}

#[test]
fn closed_processes_still_alive_count_against_the_pty_limit() {
    let f = Fixture::with(&[("KANNA_KEEPER_MAX_PTYS", "2"), ("KANNA_KEEPER_CLOSE_GRACE_MS", "3000")]);
    let mut c = f.client();
    let (sh, _) = group(&mut c, "stubborn", "trap '' HUP; sleep 60 & echo \"kid:$!\"; wait");
    c.close("stubborn").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(alive(sh));
    c.spawn(&SpawnRequest::command("a", "sleep 60")).unwrap();
    let refused = c.spawn(&SpawnRequest::command("b", "sleep 60")).unwrap_err();
    assert!(refused.to_string().contains("too many"), "{refused}");
    c.close("a").unwrap();
}

#[test]
fn a_client_that_queues_too_many_descriptors_is_dropped() {
    use std::io::{Read, Write};
    let f = Fixture::with(&[("KANNA_KEEPER_MAX_PTYS", "4")]);
    let mut c = f.client();
    for i in 0..4 {
        c.spawn(&SpawnRequest::command(&format!("s{i}"), "sleep 60")).unwrap();
    }
    let mut raw = std::os::unix::net::UnixStream::connect(f.socket()).unwrap();
    for _ in 0..400 {
        if raw.write_all(b"{\"op\":\"adopt\"}\n").is_err() {
            break;
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    let _ = raw.set_read_timeout(Some(Duration::from_secs(3)));
    let mut buf = [0u8; 65536];
    loop {
        match raw.read(&mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("still connected: {e}"),
        }
    }
    assert_eq!(c.hello().unwrap().0, kanna_keeper::PROTOCOL_VERSION);
    for i in 0..4 {
        c.close(&format!("s{i}")).unwrap();
    }
}

fn process_state(pid: i32) -> String {
    let out = Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn a_closed_process_keeps_its_pid_until_the_kill_is_done() {
    let f = Fixture::with(&[("KANNA_KEEPER_CLOSE_GRACE_MS", "1500")]);
    let mut c = f.client();
    let pid = c.spawn(&SpawnRequest::command("t1", "sleep 60")).unwrap().pid;
    c.close("t1").unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert!(process_state(pid).starts_with('Z'), "{pid} is {:?}", process_state(pid));
    wait_for(&format!("{pid} to be reaped"), || !alive(pid));
}

#[test]
fn spawned_processes_start_with_default_signal_dispositions() {
    use std::os::unix::process::CommandExt;
    let dir = private_dir();
    let state = dir.join("state");
    let mut cmd = clean_command(&state, &dir.join("home"));
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGUSR1, libc::SIG_IGN);
            libc::signal(libc::SIGALRM, libc::SIG_IGN);
            Ok(())
        });
    }
    let _keeper = KilledOnDrop(cmd.spawn().unwrap());
    let socket = state.join("keeper.sock");
    wait_for("the keeper's socket", || Client::connect(&socket).is_ok());
    let mut c = Client::connect(&socket).unwrap();
    for (id, signo, name) in [("usr1", libc::SIGUSR1, "USR1"), ("alrm", libc::SIGALRM, "ALRM")] {
        let pty = c.spawn(&SpawnRequest::command(id, &format!("kill -{name} $$; echo survived"))).unwrap();
        assert_eq!(exit_reading(&mut c, &pty.master).0.status, ExitStatus::Signal(signo), "{name}");
        c.ack(id).unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_keepers_own_settings_stay_out_of_spawned_environments() {
    let f = Fixture::with(&[("KANNA_KEEPER_MAX_PTYS", "9"), ("KANNA_STATE_DIR", "/elsewhere"), ("KANNA_KEEPER", "/k")]);
    let mut c = f.client();
    let mut req = SpawnRequest::command(
        "env",
        "echo \"[${KANNA_KEEPER_CLOSE_GRACE_MS-unset}][${KANNA_KEEPER_MAX_PTYS-unset}][${KANNA_STATE_DIR-unset}][${KANNA_KEEPER-unset}][${KANNA_KEEPER_ASKED-unset}]\"; sleep 30",
    );
    req.env.insert("KANNA_KEEPER_ASKED".into(), Some("yes".into()));
    let pty = c.spawn(&req).unwrap();
    read_until(&pty.master, "[unset][unset][unset][unset][yes]");
    c.close("env").unwrap();
}

#[test]
fn a_shell_not_on_path_is_an_error_not_a_file_in_cwd() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let repo = f.dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let planted = repo.join("kk-no-such-shell");
    std::fs::write(&planted, "#!/bin/sh\necho planted-ran\nsleep 30\n").unwrap();
    std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut c = f.client();
    let mut req = SpawnRequest::shell("s");
    req.shell = Some("kk-no-such-shell".into());
    req.cwd = Some(repo);
    let result = c.spawn(&req);
    if let Ok(pty) = &result {
        let mut buf = [0u8; 256];
        std::thread::sleep(Duration::from_millis(300));
        let n = unsafe { libc::read(pty.master.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        let _ = c.close("s");
        panic!("spawned: {:?}", String::from_utf8_lossy(&buf[..n.max(0) as usize]));
    }
    let error = result.unwrap_err().to_string();
    assert!(error.contains("kk-no-such-shell"), "{error}");
}

#[test]
fn a_slow_spawn_doesnt_hold_up_other_clients() {
    let f = Fixture::with(&[("KANNA_KEEPER_TEST_SPAWN_DELAY_MS", "1500")]);
    let socket = f.socket();
    let slow = std::thread::spawn(move || {
        let mut c = Client::connect(&socket).unwrap();
        c.set_timeout(Some(Duration::from_secs(5))).unwrap();
        let pty = c.spawn(&SpawnRequest::command("slow", "echo slow-ok; sleep 30")).map_err(|e| e.to_string())?;
        read_until(&pty.master, "slow-ok");
        c.close("slow").map_err(|e| e.to_string())
    });
    std::thread::sleep(Duration::from_millis(200));
    let mut c = f.client();
    c.set_timeout(Some(Duration::from_secs(5))).unwrap();
    let started = Instant::now();
    c.hello().unwrap();
    let took = started.elapsed();
    slow.join().unwrap().unwrap();
    assert!(took < Duration::from_millis(500), "hello took {took:?}");
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "needs the keeper's debug-build test hooks")]
fn a_spawn_that_doesnt_exec_in_time_fails_and_is_killed() {
    let f = Fixture::with(&[("KANNA_KEEPER_TEST_SPAWN_DELAY_MS", "3000"), ("KANNA_KEEPER_SPAWN_TIMEOUT_MS", "300")]);
    let mut c = f.client();
    c.set_timeout(Some(Duration::from_secs(5))).unwrap();
    let error = c.spawn(&SpawnRequest::command("late", "echo too-late")).unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error}");
    assert!(c.adopt().unwrap().is_empty());
    let again = c.spawn(&SpawnRequest::command("late", "true")).unwrap_err();
    assert!(again.to_string().contains("timed out"), "{again}");
}

#[test]
fn programs_start_from_the_session_not_from_whoever_started_the_keeper() {
    let f = Fixture::with(&[("EDITOR", "nvim"), ("KANNA_LEAK", "yes"), ("LANG", "C")]);
    let mut c = f.client();
    let req = SpawnRequest::command("t1", "printf 'E=[%s] K=[%s] L=[%s] P=[%s] END\\n' \"$EDITOR\" \"$KANNA_LEAK\" \"$LANG\" \"$PATH\"; sleep 30");
    let pty = c.spawn(&req).unwrap();
    let out = read_until(&pty.master, "] END");
    assert!(out.contains("E=[] K=[] L=[C]"), "{out}");
    assert!(out.contains("/usr/bin"), "{out}");
}

#[track_caller]
fn exits_reading(c: &mut Client, masters: &[&OwnedFd], n: usize) -> Vec<Exited> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut exits = Vec::new();
    while exits.len() < n {
        exits.extend(c.poll_events().unwrap());
        assert!(Instant::now() < deadline, "only {exits:?}");
        let mut pfds: Vec<_> = masters.iter().map(|m| libc::pollfd { fd: m.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
        pfds.push(libc::pollfd { fd: c.as_raw_fd(), events: libc::POLLIN, revents: 0 });
        unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as _, 50) };
        for p in &pfds[..masters.len()] {
            if p.revents & libc::POLLIN != 0 {
                let mut buf = [0u8; 4096];
                unsafe { libc::read(p.fd, buf.as_mut_ptr().cast(), buf.len()) };
            }
        }
    }
    exits.sort_by(|a, b| a.id.cmp(&b.id));
    exits
}

fn new_build(f: &Fixture) -> PathBuf {
    let bin = f.dir.join("new-build").join("kanna-keeper");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::copy(BIN, &bin).unwrap();
    bin
}

fn ps(pid: i32, field: &str) -> String {
    let out = Command::new("ps").args(["-o", &format!("{field}="), "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn an_upgrade_keeps_every_session_running_and_parented() {
    let f = Fixture::new();
    let mut c = f.client();
    let before = c.info().unwrap();
    assert!(before.build.as_ref().is_some_and(|b| b.len() == 64), "{before:?}");
    let shell = c.spawn(&SpawnRequest::command("shell", "echo shell-ready; read line; echo \"got:$line\"; read code; exit $code")).unwrap();
    read_until(&shell.master, "shell-ready");
    let agent = c.spawn(&SpawnRequest::command("agent", "echo agent-ready; while read line; do echo \"agent:$line\"; done")).unwrap();
    read_until(&agent.master, "agent-ready");
    let done = c.spawn(&SpawnRequest::command("done", "exit 3")).unwrap();
    assert_eq!(exits_reading(&mut c, &[&done.master], 1)[0].status, ExitStatus::Code(3));

    let bin = new_build(&f);
    let after = c.upgrade(&bin).unwrap();
    assert_eq!(after.pid, before.pid);
    assert_eq!(after.version, kanna_keeper::PROTOCOL_VERSION);
    assert!(ps(f.keeper_pid(), "command").starts_with(&bin.display().to_string()), "{}", ps(f.keeper_pid(), "command"));
    assert_eq!(std::fs::read_to_string(f.state().join("keeper.pid")).unwrap().trim(), before.pid.to_string());
    for pid in [shell.pid, agent.pid] {
        assert!(alive(pid));
        assert_eq!(ps(pid, "ppid"), before.pid.to_string());
    }
    assert_eq!(c.info().unwrap().pid, before.pid);

    write(&agent.master, "still-here\n");
    read_until(&agent.master, "agent:still-here");
    let mut fresh = f.client();
    assert_eq!(fresh.wait_event(Some(Duration::from_secs(5))).unwrap().map(|e| e.status), Some(ExitStatus::Code(3)));
    let mut adopted = fresh.adopt().unwrap();
    adopted.sort_by(|a, b| a.id.cmp(&b.id));
    let ids: Vec<_> = adopted.iter().map(|p| (p.id.as_str(), p.pid, p.exit)).collect();
    assert_eq!(ids[0], ("agent", agent.pid, None));
    assert_eq!(ids[1].0, "done");
    assert_eq!(ids[1].2, Some(ExitStatus::Code(3)));
    assert_eq!(ids[2], ("shell", shell.pid, None));
    write(&adopted[2].master, "after\n");
    read_until(&shell.master, "got:after");
    write(&shell.master, "7\n");
    let exit = exits_reading(&mut c, &[&shell.master], 1).remove(0);
    assert_eq!((exit.id.as_str(), exit.pid, exit.status), ("shell", shell.pid, ExitStatus::Code(7)));
    assert_eq!(next_exit(&mut fresh).status, ExitStatus::Code(7));

    let again = c.spawn(&SpawnRequest::command("later", "echo later-ok; sleep 30")).unwrap();
    read_until(&again.master, "later-ok");
    assert_eq!(ps(again.pid, "ppid"), before.pid.to_string());
    c.close("later").unwrap();
    c.close("agent").unwrap();
    wait_for("the agent to end", || !alive(agent.pid));
}

#[test]
fn an_exit_straight_after_an_upgrade_reports_its_code() {
    let f = Fixture::new();
    let mut c = f.client();
    let pty = c.spawn(&SpawnRequest::command("t1", "echo ready; read x; exit 42")).unwrap();
    read_until(&pty.master, "ready");
    let quick = c.spawn(&SpawnRequest::command("quick", "sleep 0.2; exit 9")).unwrap();
    c.upgrade(&new_build(&f)).unwrap();
    write(&pty.master, "\n");
    let exits = exits_reading(&mut c, &[&pty.master, &quick.master], 2);
    assert_eq!((exits[0].id.as_str(), exits[0].pid, exits[0].status), ("quick", quick.pid, ExitStatus::Code(9)));
    assert_eq!((exits[1].id.as_str(), exits[1].pid, exits[1].status), ("t1", pty.pid, ExitStatus::Code(42)));
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "needs the keeper's debug-build test hooks")]
fn a_spawn_waiting_for_its_exec_is_answered_by_the_new_code() {
    let f = Fixture::with(&[("KANNA_KEEPER_TEST_SPAWN_DELAY_MS", "1500")]);
    let mut spawner = f.client();
    let keeper = f.keeper_pid();
    let spawning = std::thread::spawn(move || spawner.spawn(&SpawnRequest::command("slow", "echo slow-ok; read x; exit 5")).map(|p| (p, spawner)));
    std::thread::sleep(Duration::from_millis(300));
    let mut c = f.client();
    let bin = new_build(&f);
    c.upgrade(&bin).unwrap();
    let (pty, mut spawner) = spawning.join().unwrap().unwrap();
    assert!(ps(keeper, "command").starts_with(&bin.display().to_string()));
    assert_eq!(ps(pty.pid, "ppid"), keeper.to_string());
    read_until(&pty.master, "slow-ok");
    write(&pty.master, "\n");
    assert_eq!(exits_reading(&mut spawner, &[&pty.master], 1)[0].status, ExitStatus::Code(5));
}

#[test]
fn a_binary_that_cant_take_over_leaves_the_old_code_running() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let mut c = f.client();
    let pty = c.spawn(&SpawnRequest::command("t1", "echo ready; read x; exit 6")).unwrap();
    read_until(&pty.master, "ready");
    let before = ps(f.keeper_pid(), "command");
    let refuser = f.dir.join("refuser");
    std::fs::write(&refuser, "#!/bin/sh\necho 'kanna-keeper: state 99 only' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&refuser, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = c.upgrade(&refuser).unwrap_err().to_string();
    assert!(error.contains("can't take over") && error.contains("state 99 only"), "{error}");
    let error = c.upgrade(&f.dir.join("missing")).unwrap_err().to_string();
    assert!(error.contains("can't run"), "{error}");
    assert!(c.upgrade(Path::new("relative/kanna-keeper")).is_err());
    assert_eq!(ps(f.keeper_pid(), "command"), before);
    write(&pty.master, "\n");
    assert_eq!(exits_reading(&mut c, &[&pty.master], 1)[0].status, ExitStatus::Code(6));
    let later = c.spawn(&SpawnRequest::command("t2", "echo works; sleep 30")).unwrap();
    read_until(&later.master, "works");
    c.close("t2").unwrap();
}

#[test]
fn upgrade_by_hand_and_a_state_too_new_is_refused() {
    let f = Fixture::new();
    let mut c = f.client();
    let pty = c.spawn(&SpawnRequest::command("t1", "echo ready; sleep 30")).unwrap();
    read_until(&pty.master, "ready");
    let bin = new_build(&f);
    let out = Command::new(&bin).arg("--state-dir").arg(f.state()).arg("--upgrade").output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said} {}", String::from_utf8_lossy(&out.stderr));
    assert!(said.contains(&format!("upgraded the keeper (pid {})", f.keeper_pid())), "{said}");
    assert!(ps(f.keeper_pid(), "command").starts_with(&bin.display().to_string()));
    assert!(alive(pty.pid));
    c.close("t1").unwrap();

    let mut check = Command::new(BIN).arg("--check-upgrade").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    use std::io::Write;
    check.stdin.take().unwrap().write_all(br#"{"state": 999}"#).unwrap();
    let out = check.wait_with_output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("newer"), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
#[cfg_attr(not(debug_assertions), ignore = "needs the keeper's debug-build test hooks")]
fn the_upgrades_pause_counts_against_a_waiting_spawns_timeout() {
    let f = Fixture::with(&[
        ("KANNA_KEEPER_TEST_SPAWN_DELAY_MS", "10000"),
        ("KANNA_KEEPER_SPAWN_TIMEOUT_MS", "2000"),
        ("KANNA_KEEPER_TEST_PROBE_DELAY_MS", "1500"),
    ]);
    let mut spawner = f.client();
    let asked = Instant::now();
    let spawning = std::thread::spawn(move || spawner.spawn(&SpawnRequest::command("stuck", "true")).map(|p| p.pid));
    std::thread::sleep(Duration::from_millis(300));
    f.client().upgrade(&new_build(&f)).unwrap();
    let error = spawning.join().unwrap().unwrap_err().to_string();
    let took = asked.elapsed();
    assert!(error.contains("timed out"), "{error}");
    assert!(took < Duration::from_millis(3000), "the spawn failed after {took:?}, its timeout is 2 s");
}

#[test]
fn stop_closes_every_pty_and_exits_once_they_are_gone() {
    let mut f = Fixture::new();
    let mut c = f.client();
    let (sh, kid) = group(&mut c, "plain", "sleep 60 & echo \"kid:$!\"; wait");
    let (sh2, kid2) = group(&mut c, "stubborn", "trap '' HUP; sleep 60 & echo \"kid:$!\"; wait");
    c.spawn(&SpawnRequest::command("done", "true")).unwrap();
    assert_eq!(next_exit(&mut c).status, ExitStatus::Code(0));
    c.stop().unwrap();
    assert!(!f.socket().exists(), "a stopping keeper takes no new clients");
    assert!(c.spawn(&SpawnRequest::command("late", "true")).is_err());
    wait_for("the keeper to exit", || f.keeper_exited());
    for pid in [sh, kid, sh2, kid2] {
        assert!(!alive(pid), "{pid} outlived its keeper");
    }
    assert!(!f.state().join("keeper.pid").exists());
}
