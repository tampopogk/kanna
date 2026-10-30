use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const DAEMON: &str = env!("CARGO_BIN_EXE_kanna-daemon");

const STUBBORN_CODEX: &str = "#!/bin/bash
if [[ $1 == app-server ]]; then
  trap '' HUP TERM INT
  for a in \"$@\"; do [[ $a == unix://* ]] && : > \"${a#unix://}\"; done
  while :; do sleep 1; done
fi
exec sleep 600
";

fn children(parent: u32) -> Vec<i32> {
    let out = Command::new("ps").args(["-axo", "pid=,ppid="]).output().unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace().map(|f| f.parse::<u32>().ok());
            match (fields.next()??, fields.next()??) {
                (pid, ppid) if ppid == parent => Some(pid as i32),
                _ => None,
            }
        })
        .collect()
}

struct Started(Vec<i32>);

impl Drop for Started {
    fn drop(&mut self) {
        for &pid in &self.0 {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

fn gone(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) != 0 }
}

fn stopped_by(signal: i32) {
    let dir = std::env::temp_dir().join(format!("kcr{}-{signal}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let codex = dir.join("stubborn-codex");
    std::fs::write(&codex, STUBBORN_CODEX).unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut helper = Command::new(DAEMON).arg("--codex-remote").arg(&dir).arg(&codex).arg("--yolo").spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut started = Started(vec![helper.id() as i32]);
    let mut kids = children(helper.id());
    while kids.len() < 2 {
        started.0.extend(&kids);
        assert!(Instant::now() < deadline, "the helper didn't start the server and the TUI: {kids:?}");
        std::thread::sleep(Duration::from_millis(50));
        kids = children(helper.id());
    }
    started.0.extend(&kids);
    unsafe { libc::kill(helper.id() as i32, signal) };
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = helper.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the helper didn't exit on signal {signal}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(128 + signal), "{status:?} {:?}", status.signal());
    let deadline = Instant::now() + Duration::from_secs(2);
    while !kids.iter().all(|&pid| gone(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(kids.iter().all(|&pid| gone(pid)), "left running: {kids:?}");
    started.0.clear();
    let _ = std::fs::remove_dir_all(Path::new(&dir));
}

#[test]
fn a_hangup_stops_the_server_even_one_that_ignores_it() {
    stopped_by(libc::SIGHUP);
}

#[test]
fn sigterm_stops_the_server_and_the_tui() {
    stopped_by(libc::SIGTERM);
}

fn cpu_seconds(pid: i32) -> f64 {
    let out = Command::new("ps").args(["-o", "time=", "-p", &pid.to_string()]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    text.split(':').fold(0.0, |total, part| total * 60.0 + part.parse::<f64>().unwrap_or(0.0))
}

fn wait_gone(pid: i32, within: Duration) -> f64 {
    let deadline = Instant::now() + within;
    let mut used = 0.0;
    while !gone(pid) && Instant::now() < deadline {
        used = cpu_seconds(pid).max(used);
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(gone(pid), "the orphaned helper (pid {pid}) is still running after {within:?}, {used}s of CPU");
    used
}

fn orphan(dir: &Path, codex: &Path, terminal: bool) -> (i32, Vec<i32>) {
    use std::os::unix::process::CommandExt;
    let script = format!("'{DAEMON}' --codex-remote '{}' '{}' --yolo & echo $! > '{}/helper.pid'; wait", dir.display(), codex.display(), dir.display());
    let mut shell = Command::new("/bin/sh");
    shell.arg("-c").arg(script);
    let mut master = -1;
    if terminal {
        let mut slave = -1;
        assert_eq!(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) }, 0);
        let io = || unsafe { std::process::Stdio::from_raw_fd(libc::dup(slave)) };
        shell.stdin(io()).stdout(io()).stderr(io());
        unsafe {
            shell.pre_exec(|| {
                libc::setsid();
                libc::ioctl(0, libc::TIOCSCTTY as _, 0);
                Ok(())
            });
        }
        unsafe { libc::close(slave) };
    } else {
        shell.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    }
    let mut shell = shell.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let helper = loop {
        if let Some(pid) = std::fs::read_to_string(dir.join("helper.pid")).ok().and_then(|p| p.trim().parse::<i32>().ok()) {
            break pid;
        }
        assert!(Instant::now() < deadline, "the shell didn't start the helper");
        std::thread::sleep(Duration::from_millis(50));
    };
    let want = if terminal { 2 } else { 1 };
    let mut kids = children(helper as u32);
    while kids.len() < want {
        assert!(Instant::now() < deadline, "the helper didn't start its children: {kids:?}");
        std::thread::sleep(Duration::from_millis(50));
        kids = children(helper as u32);
    }
    let _ = shell.kill();
    let _ = shell.wait();
    if master >= 0 {
        unsafe { libc::close(master) };
    }
    (helper, kids)
}

use std::os::fd::FromRawFd;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kcr{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let codex = dir.join("stubborn-codex");
    std::fs::write(&codex, STUBBORN_CODEX).unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

#[test]
fn a_helper_whose_shell_and_terminal_are_gone_exits_without_spinning() {
    let dir = scratch("orphan");
    let (helper, kids) = orphan(&dir, &dir.join("stubborn-codex"), true);
    let started = Started(std::iter::once(helper).chain(kids.iter().copied()).collect());
    let used = wait_gone(helper, Duration::from_secs(10));
    assert!(used < 1.0, "the orphaned helper used {used}s of CPU on its way out");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !kids.iter().all(|&pid| gone(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(kids.iter().all(|&pid| gone(pid)), "left running: {kids:?}");
    std::mem::forget(started);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_orphaned_helper_running_codex_alone_stays_idle_and_quits_on_sigterm() {
    let dir = scratch("alone");
    std::fs::write(dir.join("codex"), "not the helper's folder").unwrap();
    let (helper, kids) = orphan(&dir, &dir.join("stubborn-codex"), false);
    let started = Started(std::iter::once(helper).chain(kids.iter().copied()).collect());
    std::thread::sleep(Duration::from_secs(2));
    let idle = cpu_seconds(helper);
    assert!(idle < 0.5, "the orphaned helper used {idle}s of CPU while its codex ran");
    unsafe { libc::kill(helper, libc::SIGTERM) };
    let used = wait_gone(helper, Duration::from_secs(8));
    assert!(used < 1.0, "the orphaned helper used {used}s of CPU on its way out");
    let deadline = Instant::now() + Duration::from_secs(2);
    while !kids.iter().all(|&pid| gone(pid)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(kids.iter().all(|&pid| gone(pid)), "left running: {kids:?}");
    std::mem::forget(started);
    let _ = std::fs::remove_dir_all(&dir);
}
