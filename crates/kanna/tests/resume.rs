mod support;

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use support::{wait_for, Daemon};

fn shelf_task(daemon: &Daemon, id: &str) -> Value {
    daemon.kanna(&["--json", "task", "list"]).json()["shelf"].as_array().unwrap().iter().find(|t| t["id"] == id).cloned().unwrap_or(Value::Null)
}

#[test]
fn a_task_whose_first_start_failed_starts_fresh_and_never_resumes_the_agent_s_other_session() {
    let daemon = Daemon::start();
    let _app = daemon.app();
    let repo = daemon.dir.join("home/.kanna-v4/repos/local/notes");
    std::fs::create_dir_all(repo.join(".kanna")).unwrap();
    let git = |args: &[&str]| assert!(Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(&repo).status().unwrap().success());
    git(&["init", "-q", "-b", "main"]);
    git(&["commit", "-q", "--allow-empty", "-m", "first"]);
    std::fs::write(repo.join(".kanna/config.json"), json!({ "version": 1, "shelf": { "runners": 0 } }).to_string()).unwrap();

    let agents = daemon.dir.join("agents");
    std::fs::create_dir_all(agents.join("real")).unwrap();
    let mock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/mock-agent/mock-agent").canonicalize().unwrap();
    std::os::unix::fs::symlink(mock, agents.join("real/codex")).unwrap();
    let (fail, tried) = (agents.join("fail"), agents.join("tried"));
    std::fs::write(&fail, "").unwrap();
    let codex = agents.join("codex");
    let script = format!("#!/bin/sh\nif [ -e '{fail}' ]; then touch '{tried}'; echo \"couldn't start codex app-server: No such file or directory\"; exit 1; fi\nexec '{real}' \"$@\"\n", fail = fail.display(), tried = tried.display(), real = agents.join("real/codex").display());
    std::fs::write(&codex, script).unwrap();
    std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o755)).unwrap();

    let id = daemon.kanna_in(&repo, &["--json", "task", "new", "Fails first", "say hi", "--agent", codex.to_str().unwrap()]).json()["task"]["id"].as_str().unwrap().to_string();
    daemon.kanna(&["task", "start", &id]).ok();
    wait_for("the agent's first start to fail", || tried.exists());
    wait_for("the task to stop", || shelf_task(&daemon, &id)["running"] == false);
    std::fs::remove_file(&fail).unwrap();
    assert_eq!(shelf_task(&daemon, &id)["session"], Value::Null, "its agent never recorded a session");

    let elsewhere = daemon.dir.join("home/Documents/Codex/other");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let day = String::from_utf8(Command::new("date").arg("+%Y/%m/%d").output().unwrap().stdout).unwrap().trim().to_string();
    let sessions = daemon.dir.join("home/.codex/sessions").join(&day);
    std::fs::create_dir_all(&sessions).unwrap();
    let meta = json!({ "timestamp": "2099-01-01T00-00-00", "type": "session_meta", "payload": { "id": "users-own-session", "cwd": elsewhere } });
    std::fs::write(sessions.join("rollout-2099-01-01T00-00-00-users-own-session.jsonl"), format!("{meta}\n")).unwrap();

    let started = daemon.kanna(&["--json", "task", "start", &id]).json();
    let pane = started["pane"].as_str().unwrap().to_string();
    wait_for("the agent's second start", || daemon.kanna(&["read", &pane]).ok().contains("args: "));
    let screen = daemon.kanna(&["read", &pane]).ok();
    let joined = screen.replace('\n', "");
    assert!(joined.contains("Your task: say hi") && !joined.contains("resume"), "it starts fresh from its first message: {screen}");
}
