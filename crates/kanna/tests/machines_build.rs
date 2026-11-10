mod support;

use kanna_guest::supervisor::{Config, Supervisor};
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use support::{cli, Daemon};

fn pair(joiner: &Daemon, offerer: &Daemon) {
    let app = joiner.app();
    let mut waiting = cli(&offerer.dir).env("KANNA_SOCKET", offerer.socket()).args(["machine", "pair", "--json"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(waiting.stdout.take().unwrap()).lines();
    let offer: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let peer = format!("{}@127.0.0.1:{}", offer["key"].as_str().unwrap(), offer["port"]);
    joiner.kanna(&["machine", "pair", offer["code"].as_str().unwrap(), "--peer", &peer]).ok();
    assert!(waiting.wait().unwrap().success());
    app.quit();
}

fn machine(name: &str) -> Daemon {
    let daemon = Daemon::start();
    daemon.kanna(&["machine", "name", name]).ok();
    daemon
}

fn guest(mac: &Daemon, name: &str) -> PathBuf {
    let dir = mac.dir.join("state/guests").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let supervisor = Supervisor::new(Config::new(mac.dir.join(format!("guest-{name}")))).unwrap();
    let listener = UnixListener::bind(dir.join("agent.sock")).unwrap();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let supervisor = supervisor.clone();
            std::thread::spawn(move || {
                let _ = supervisor.serve(conn);
            });
        }
    });
    mac.dir.join(format!("guest-{name}"))
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn repo(linux: &Daemon) -> PathBuf {
    let repo = linux.dir.join("home/app");
    std::fs::create_dir_all(repo.join("Sources")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("Sources/App.swift"), "let a = 1\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    repo.canonicalize().unwrap()
}

#[test]
fn a_task_machine_builds_on_the_build_macs_guest_through_its_daemon() {
    let linux = machine("linux");
    let mac = machine("mac");
    pair(&linux, &mac);
    let guest_state = guest(&mac, "vm-a");
    let repo = repo(&linux);
    std::fs::write(repo.join("Sources/App.swift"), "let a = 2\n").unwrap();
    std::fs::write(repo.join("new.swift"), "untracked\n").unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(repo.join("big.bin"), &big).unwrap();

    let out = cli(&linux.dir).current_dir(&repo).env("KANNA_SOCKET", linux.socket()).args(["build", "--mac", "mac", "--", "sh", "-c", "cat Sources/App.swift new.swift; echo \"$PWD/Sources/App.swift:1:5: error: nope\"; exit 7"]).output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(7), "{stdout}\n{stderr}");
    assert!(stdout.starts_with("let a = 2\nuntracked\n"), "{stdout}");
    assert!(stdout.contains("/mirrors/linux/app/"), "the build Mac names the task machine's mirror after the machine that asked: {stdout}");
    assert!(stderr.contains(&format!("{}/Sources/App.swift:1:5: error: nope", repo.display())), "{stderr}");
    assert!(guest_state.join("mirrors/linux/app").is_dir());
    let laid = std::fs::read(guest_state.join("mirrors/linux/app").read_dir().unwrap().next().unwrap().unwrap().path().join("src/big.bin")).unwrap();
    assert!(laid == big, "a 3 MB file arrives whole");

    std::fs::create_dir_all(linux.dir.join("home/.kanna-v4")).unwrap();
    std::fs::write(linux.dir.join("home/.kanna-v4/config.json"), r#"{"build": {"mac": "mac"}}"#).unwrap();
    let out = cli(&linux.dir).current_dir(repo.join("Sources")).env("KANNA_SOCKET", linux.socket()).arg0("xcrun").args(["--find", "git"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).trim().ends_with("/git"), "xcrun ran on the build Mac's guest: {}", String::from_utf8_lossy(&out.stdout));

    std::fs::remove_dir_all(mac.dir.join("state/guests/vm-a")).unwrap();
    let out = cli(&linux.dir).current_dir(&repo).env("KANNA_SOCKET", linux.socket()).args(["build", "--mac", "mac", "--", "true"]).output().unwrap();
    assert_eq!(out.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no guest running; builds run only inside a guest"), "{}", String::from_utf8_lossy(&out.stderr));
}


fn worktree(repo: &Path, name: &str) -> PathBuf {
    let path = repo.parent().unwrap().join(name);
    git(repo, &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()]);
    path.canonicalize().unwrap()
}

fn queue(linux: &Daemon) -> Value {
    let out = cli(&linux.dir).env("KANNA_SOCKET", linux.socket()).args(["builds", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn stages(linux: &Daemon) -> Vec<(String, String)> {
    let reply = queue(linux);
    reply["macs"][0]["jobs"].as_array().into_iter().flatten().map(|j| (j["label"].as_str().unwrap_or_default().to_string(), j["stage"].as_str().unwrap_or_default().to_string())).collect()
}

fn wait_for(linux: &Daemon, wanted: &[(&str, &str)]) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let seen = stages(linux);
        if seen.iter().map(|(a, b)| (a.as_str(), b.as_str())).eq(wanted.iter().copied()) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "the queue stayed at {seen:?}, wanted {wanted:?}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn two_tasks_builds_queue_in_order_and_a_queued_one_can_be_cancelled() {
    let linux = machine("linux");
    let mac = machine("mac");
    pair(&linux, &mac);
    guest(&mac, "vm-a");
    let repo = repo(&linux);
    let (second, third) = (worktree(&repo, "second"), worktree(&repo, "third"));
    let go = linux.dir.join("go");
    let wait = format!("while [ ! -f {} ]; do sleep 0.1; done; echo first", go.display());
    let build = |dir: &Path, script: &str| cli(&linux.dir).current_dir(dir).env("KANNA_SOCKET", linux.socket()).args(["build", "--mac", "mac", "--", "sh", "-c", script]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();

    let first = build(&repo, &wait);
    wait_for(&linux, &[("main", "running")]);
    let next = build(&second, "echo second");
    wait_for(&linux, &[("main", "running"), ("second", "queued")]);
    let last = build(&third, "echo third");
    wait_for(&linux, &[("main", "running"), ("second", "queued"), ("third", "queued")]);

    let text = cli(&linux.dir).env("KANNA_SOCKET", linux.socket()).arg("builds").output().unwrap();
    let text = String::from_utf8_lossy(&text.stdout).into_owned();
    let (running, one, two) = (text.find("▶ main").unwrap(), text.find("1 second").unwrap(), text.find("2 third").unwrap());
    assert!(running < one && one < two, "{text}");

    let reply = queue(&linux);
    let job = reply["macs"][0]["jobs"][2]["job"].as_str().unwrap();
    let cancelled = cli(&linux.dir).env("KANNA_SOCKET", linux.socket()).args(["builds", "cancel", job]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&cancelled.stdout).trim(), "taken out of the queue");
    let last = last.wait_with_output().unwrap();
    assert_eq!(last.status.code(), Some(130), "{}", String::from_utf8_lossy(&last.stderr));
    assert!(String::from_utf8_lossy(&last.stderr).contains("queued on the build Mac, 1 ahead"), "{}", String::from_utf8_lossy(&last.stderr));

    std::fs::write(&go, "").unwrap();
    let (first, next) = (first.wait_with_output().unwrap(), next.wait_with_output().unwrap());
    assert_eq!((first.status.code(), String::from_utf8_lossy(&first.stdout).trim()), (Some(0), "first"));
    assert_eq!((next.status.code(), String::from_utf8_lossy(&next.stdout).trim()), (Some(0), "second"));
    assert!(String::from_utf8_lossy(&next.stderr).contains("running in vm-a"), "{}", String::from_utf8_lossy(&next.stderr));
    wait_for(&linux, &[]);
    let reply = queue(&linux);
    let recent: Vec<&str> = reply["macs"][0]["recent"].as_array().unwrap().iter().map(|j| j["label"].as_str().unwrap()).collect();
    assert_eq!(recent, ["second", "main", "third"]);
    assert_eq!(reply["macs"][0]["recent"][2]["status"]["phase"], "cancelled");
}

#[test]
fn a_running_job_is_cancelled_from_another_terminal() {
    let linux = machine("linux");
    let mac = machine("mac");
    pair(&linux, &mac);
    guest(&mac, "vm-a");
    let repo = repo(&linux);
    let running = cli(&linux.dir).current_dir(&repo).env("KANNA_SOCKET", linux.socket()).args(["build", "--mac", "mac", "--", "sleep", "60"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    wait_for(&linux, &[("main", "running")]);
    let reply = queue(&linux);
    let job = reply["macs"][0]["jobs"][0]["job"].as_str().unwrap();
    let out = cli(&linux.dir).env("KANNA_SOCKET", linux.socket()).args(["builds", "cancel", job]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "stopping", "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(running.wait_with_output().unwrap().status.code(), Some(130));
    let missing = cli(&linux.dir).env("KANNA_SOCKET", linux.socket()).args(["builds", "cancel", "b-0000"]).output().unwrap();
    assert!(!missing.status.success());
}

#[cfg(target_os = "macos")]
#[test]
fn a_build_goes_to_the_mac_with_a_matching_guest() {
    let linux = machine("linux");
    let bare = machine("bare");
    let mac = machine("mac");
    pair(&linux, &bare);
    pair(&linux, &mac);
    guest(&mac, "vm-a");
    let repo = repo(&linux);
    let out = cli(&linux.dir).current_dir(&repo).env("KANNA_SOCKET", linux.socket()).args(["build", "--", "sh", "-c", "echo built"]).output().unwrap();
    assert_eq!((out.status.code(), String::from_utf8_lossy(&out.stdout).trim()), (Some(0), "built"), "{}", String::from_utf8_lossy(&out.stderr));
    let out = cli(&linux.dir).current_dir(&repo).env("KANNA_SOCKET", linux.socket()).args(["build", "--xcode", "999", "--", "true"]).output().unwrap();
    assert_eq!(out.status.code(), Some(125));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no build Mac has a guest with xcode 999"), "{}", String::from_utf8_lossy(&out.stderr));
}
