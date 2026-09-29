//! SIGTERM / SIGHUP take the same Stop & quit path as Ctrl+Q: the harness's
//! process group is shut down and nothing is orphaned. Runs the real binary
//! under a pseudo-terminal (via `script`) with a fake harness script.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn pgrep(pattern: &str) -> Vec<i32> {
    let out = Command::new("pgrep")
        .arg("-f")
        .arg(pattern)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|p| p.parse().ok())
        .collect()
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Fixture {
    dir: PathBuf,
    fake: PathBuf,
    tui: Child,
}

impl Fixture {
    fn start(tag: &str) -> Fixture {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".tmp/signal-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A harness that never answers, ignores SIGTERM and runs a tool
        // subprocess in its group: only a group kill cleans it up.
        let fake = dir.join("fake-harness.sh");
        std::fs::write(
            &fake,
            "#!/bin/sh\ntrap '' TERM HUP\nsleep 300 &\nwait\nwait\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bin = env!("CARGO_BIN_EXE_agent-tui");
        let tui_cmd = format!(
            "{bin} claude --bin {} --cwd {}",
            fake.display(),
            dir.display()
        );
        let mut cmd = Command::new("script");
        if cfg!(target_os = "macos") {
            cmd.arg("-q").arg("/dev/null").args(tui_cmd.split(' '));
        } else {
            cmd.arg("-qec").arg(&tui_cmd).arg("/dev/null");
        }
        let tui = cmd
            .env("TERM", "xterm-256color")
            .stdin(Stdio::piped()) // kept open: EOF would close the pty
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("run `script` to provide a pty");
        Fixture { dir, fake, tui }
    }

    /// The fake harness itself (`sh <script> -p …`), not the `script` or
    /// agent-tui processes whose command lines also mention its path.
    fn fake_pids(&self) -> Vec<i32> {
        pgrep(&format!("sh {} -p", self.fake.display()))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.tui.kill();
        for p in self.fake_pids() {
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn run(sig: i32, tag: &str) {
    let fx = Fixture::start(tag);
    let fake = fx.fake.display().to_string();
    wait_for("the fake harness to start", 15, || {
        !fx.fake_pids().is_empty()
    });
    let harness_pid = fx.fake_pids()[0];
    // SAFETY: getpgid on a pid we just observed.
    let pgid = unsafe { libc::getpgid(harness_pid) };
    assert!(pgid > 0);
    wait_for("the tool subprocess", 5, || {
        let out = Command::new("pgrep")
            .arg("-g")
            .arg(pgid.to_string())
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .count()
            >= 2
    });
    let tui_pid = pgrep(&format!("agent-tui claude --bin {fake}"))
        .into_iter()
        .find(|p| *p != fx.tui.id() as i32)
        .expect("agent-tui pid");
    unsafe { libc::kill(tui_pid, sig) };
    // Shutdown waits up to 3 s for a clean exit, then signals the group.
    wait_for(
        "agent-tui to exit",
        10,
        || unsafe { libc::kill(tui_pid, 0) } != 0,
    );
    wait_for(
        "the harness process group to be gone",
        5,
        || unsafe { libc::killpg(pgid, 0) } != 0,
    );
    assert!(fx.fake_pids().is_empty(), "fake harness orphaned");
}

#[test]
fn sigterm_shuts_down_the_harness_group() {
    run(libc::SIGTERM, "term");
}

#[test]
fn sighup_shuts_down_the_harness_group() {
    run(libc::SIGHUP, "hup");
}
