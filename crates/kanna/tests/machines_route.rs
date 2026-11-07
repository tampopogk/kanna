mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use support::{cli, Daemon, Run};

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

fn machine(name: &str, agents: &Path) -> Daemon {
    let daemon = Daemon::start_with(&[("KANNA_AGENTS_HOME", agents.to_str().unwrap())]);
    daemon.kanna(&["machine", "name", name]).ok();
    daemon
}

fn ask(daemon: &Daemon, cmd: &str, args: Value) -> Value {
    let stream = UnixStream::connect(daemon.socket()).unwrap();
    writeln!(&stream, "{}", json!({ "id": 1, "cmd": cmd, "args": args })).unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success(), "git {args:?}");
}

fn clone(home: &Path) -> PathBuf {
    let repo = home.join(".kanna-v4/repos/team/notes");
    std::fs::create_dir_all(repo.join(".kanna")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "first"]);
    git(&repo, &["remote", "add", "origin", "https://example.com/team/notes.git"]);
    std::fs::write(repo.join(".kanna/config.json"), json!({ "version": 1, "shelf": { "runners": 0 } }).to_string()).unwrap();
    repo.canonicalize().unwrap()
}

fn claude(daemon: &Daemon) -> String {
    let dir = daemon.dir.join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    let mock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/mock-agent/mock-agent").canonicalize().unwrap();
    let _ = std::os::unix::fs::symlink(mock, dir.join("claude"));
    dir.join("claude").to_string_lossy().into_owned()
}

fn started(daemon: &Daemon, repo: &Path, name: &str) -> (String, String) {
    let args = ["task", "new", name, &format!("the brief of {name}"), "--agent", &claude(daemon), "--json"];
    let output = cli(&daemon.dir).current_dir(repo).env("KANNA_SOCKET", daemon.socket()).args(args).output().unwrap();
    let made: Value = serde_json::from_str(&Run::from(&args, output).ok()).unwrap();
    let task = made["task"]["id"].as_str().unwrap().to_string();
    let reply = daemon.kanna(&["task", "start", &task, "--json"]).json();
    let pane = reply["pane"].as_str().or(reply["task"].as_str()).unwrap_or_default().to_string();
    (task, pane)
}

fn within(seconds: u64, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn record(daemon: &Daemon, task: &str) -> Value {
    ask(daemon, "replica", json!({ "action": "list" }))["records"].as_array().unwrap().iter().find(|r| r["task"] == task).cloned().unwrap_or(Value::Null)
}

fn shows(daemon: &Daemon, pane: &str, text: &str) -> bool {
    daemon.kanna(&["read", pane]).out.replace(['\r', '\n'], "").contains(text)
}

#[test]
fn a_task_on_another_machine_is_read_screenshot_messaged_and_stopped_by_its_id_alone() {
    let agents = std::env::temp_dir().join(format!("kroute{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&agents);
    std::fs::create_dir_all(&agents).unwrap();
    let agents = agents.canonicalize().unwrap();
    let (mbp, mut vm) = (machine("mbp", &agents.join("mbp")), machine("vm", &agents.join("vm")));
    pair(&mbp, &vm);
    let (here, there) = (clone(&agents.join("mbp")), clone(&agents.join("vm")));
    let (task, agent) = started(&vm, &there, "Remote work");
    let (asker, asker_pane) = started(&mbp, &here, "Asker");
    let short = task.strip_prefix("task-").unwrap()[..4].to_string();
    let _vm_app = vm.app();
    within(15, "mbp to know vm runs the task", || record(&mbp, &task)["state"] == "started");

    within(15, "mbp to read vm's agent by the task's ID", || shows(&mbp, &short, "the brief of Remote work"));
    assert!(shows(&mbp, &format!("task-{short}"), "the brief of Remote work"), "task-ac2b works as well as ac2b");
    within(15, "a task here to be read here", || shows(&mbp, &asker, "the brief of Asker"));
    assert!(!shows(&mbp, &agent, "the brief of Remote work"), "a plain pane ID is this machine's");
    let located = ask(&mbp, "tasks", json!({ "action": "locate", "task": short }));
    assert_eq!((located["machine"].as_str(), located["task"]["id"].as_str()), (Some("vm"), Some(task.as_str())), "{located}");

    let shot = mbp.dir.join("remote.png");
    let saved = mbp.kanna(&["screenshot", &short, "-o", shot.to_str().unwrap(), "--json"]).json();
    assert_eq!(saved["path"].as_str(), Some(shot.to_str().unwrap()));
    assert_eq!(saved["args"]["pane"].as_str(), Some(agent.as_str()), "vm's app was asked for vm's agent pane: {saved}");
    assert!(std::fs::read(&shot).unwrap().starts_with(b"\x89PNG"), "the picture is saved on this machine");
    mbp.kanna(&["screenshot", &short, "-o", shot.to_str().unwrap()]).fails(1, "already exists");

    let sent = mbp.kanna(&["task", "send", &short, "mock: say hi"]).ok();
    assert!(sent.contains(&format!("task-{short}")), "{sent}");
    mbp.kanna_as(&asker_pane, &["task", "send", &short, "from a task on mbp"]).ok();
    let held = vm.kanna(&["task", "messages", &task, "--json"]).json();
    let senders: Vec<(String, String)> = held["messages"].as_array().unwrap().iter().map(|m| (m["from"]["id"].as_str().or(m["from"].as_str()).unwrap_or_default().to_string(), m["text"].as_str().unwrap_or_default().to_string())).collect();
    assert_eq!(senders, [("user".to_string(), "mock: say hi".to_string()), (asker.clone(), "from a task on mbp".to_string())], "vm keeps them, from mbp's user and mbp's task: {held}");
    let messages = mbp.kanna(&["task", "messages", &short]).ok();
    assert!(messages.contains("from a task on mbp") && messages.contains("mock: say hi"), "{messages}");

    mbp.kanna(&["send-key", &short, "ctrl+l"]).ok();
    mbp.kanna(&["read", &short, "--lines", "5"]).ok();
    mbp.kanna(&["task", "stop", &short]).ok();
    within(15, "vm's agent to stop", || ask(&vm, "tasks", json!({ "action": "list" }))["shelf"].as_array().unwrap().iter().any(|t| t["id"] == task.as_str() && t["running"] == false));

    mbp.kanna(&["--host", "vm", "read", &short]).ok();
    mbp.kanna(&["--host", "vm", "read", &asker]).fails(4, "runs on mbp, not vm");
    mbp.kanna(&["read", "beef"]).fails(4, "no pane \"beef\"");

    vm.stop();
    let refused = mbp.kanna(&["read", &short]).fails(3, &format!("task-{short} \"Remote work\" runs on vm, which is unreachable"));
    assert!(refused.contains("can't reach vm"), "{refused}");
    mbp.kanna(&["task", "send", &short, "anyone there?"]).fails(3, "runs on vm, which is unreachable");
    let _ = std::fs::remove_dir_all(&agents);
}
