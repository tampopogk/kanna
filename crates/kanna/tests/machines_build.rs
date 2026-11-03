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

    let out = cli(&linux.dir).current_dir(&repo).env("KANNA_SOCKET", linux.socket()).args(["build", "--mac", "mac", "--", "sh", "-c", "cat Sources/App.swift new.swift; echo \"$PWD/Sources/App.swift:1:5: error: nope\"; exit 7"]).output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(7), "{stdout}\n{stderr}");
    assert!(stdout.starts_with("let a = 2\nuntracked\n"), "{stdout}");
    assert!(stdout.contains("/mirrors/linux/app/"), "the build Mac names the task machine's mirror after the machine that asked: {stdout}");
    assert!(stderr.contains(&format!("{}/Sources/App.swift:1:5: error: nope", repo.display())), "{stderr}");
    assert!(guest_state.join("mirrors/linux/app").is_dir());

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

