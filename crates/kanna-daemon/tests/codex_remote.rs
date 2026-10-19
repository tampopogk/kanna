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
