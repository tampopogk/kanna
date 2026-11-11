mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use support::{cli, Daemon, Run};

const ORIGIN: &str = "https://example.com/team/factory.git";

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

fn mock(daemon: &Daemon) -> String {
    let dir = daemon.dir.join("agents");
    std::fs::create_dir_all(&dir).unwrap();
    let mock = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/mock-agent/mock-agent").canonicalize().unwrap();
    let _ = std::os::unix::fs::symlink(mock, dir.join("claude"));
    dir.join("claude").to_string_lossy().into_owned()
}

fn machine(name: &str, roles: &[&str]) -> Daemon {
    let daemon = Daemon::start_with(&[("KANNA_MOCK_REPORTS", "1")]);
    daemon.kanna(&["machine", "name", name]).ok();
    let config = json!({ "version": 1, "shelf": { "runners": 2 }, "machine": { "roles": roles }, "pipelines": { "agents": { "claude": mock(&daemon) } } });
    std::fs::create_dir_all(daemon.dir.join("home/.kanna-v4")).unwrap();
    std::fs::write(daemon.dir.join("home/.kanna-v4/config.json"), config.to_string()).unwrap();
    daemon
}

fn ask(daemon: &Daemon, cmd: &str, args: Value) -> Value {
    let stream = UnixStream::connect(daemon.socket()).unwrap();
    writeln!(&stream, "{}", json!({ "id": 1, "cmd": cmd, "args": args })).unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(["-c", "user.email=e@e", "-c", "user.name=e"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

const BUILD: &str = "Build it.\nmock: run echo \"$KANNA_REPO\" > built.txt && git add built.txt && git commit -qm build && echo draft > draft.txt && echo 'Built it.' > \"$KANNA_ANSWER\" && \"$KANNA_CLI\" run done\n";

const CHECK: &str = "echo \"checked in $KANNA_REPO\"\ncat built.txt draft.txt || exit 1\necho e2e ok\n";

fn clone(daemon: &Daemon) -> PathBuf {
    let repo = daemon.dir.join("home/.kanna-v4/repos/team/factory");
    std::fs::create_dir_all(repo.join(".kanna/pipelines")).unwrap();
    std::fs::create_dir_all(repo.join(".kanna/prompts")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "e@e"]);
    git(&repo, &["config", "user.name", "e"]);
    std::fs::write(repo.join("README.md"), "# Factory\n").unwrap();
    std::fs::write(repo.join("check.sh"), CHECK).unwrap();
    std::fs::write(repo.join(".kanna/prompts/build.md"), BUILD).unwrap();
    std::fs::write(repo.join(".kanna/pipelines/remote.kojo"), "build = claude on vm\ne2e = run sh check.sh on e2e\nbuild -> e2e -> done\n").unwrap();
    std::fs::write(repo.join(".kanna/pipelines/far.kojo"), "far = run sh check.sh on vm\n").unwrap();
    std::fs::write(repo.join(".kanna/pipelines/late.kojo"), "late = run sh check.sh on vm within 4s\n").unwrap();
    std::fs::write(repo.join(".kanna/pipelines/lost.kojo"), "lost = run sh check.sh on gpu\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    git(&repo, &["remote", "add", "origin", ORIGIN]);
    daemon.kanna_in(&repo, &["task", "list"]).ok();
    repo.canonicalize().unwrap()
}

fn within(seconds: u64, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn shelf(daemon: &Daemon) -> Vec<Value> {
    ask(daemon, "tasks", json!({ "action": "list" }))["shelf"].as_array().cloned().unwrap_or_default()
}

fn task_on(daemon: &Daemon, repo: &Path) -> (String, String, String) {
    let all = ["task", "new", "Factory", "work 1", "--agent", &mock(daemon), "--on", "mbp", "--json"];
    let output = cli(&daemon.dir).current_dir(repo).env("KANNA_SOCKET", daemon.socket()).args(all).output().unwrap();
    let made: Value = serde_json::from_str(&Run::from(&all, output).ok()).unwrap();
    let task = made["task"]["id"].as_str().unwrap().to_string();
    let entry = || shelf(daemon).into_iter().find(|t| t["id"] == task.as_str()).unwrap_or(Value::Null);
    within(30, "the task to start on mbp", || entry()["held"].is_string() && entry()["folder"].is_string());
    let found = entry();
    (task, found["folder"].as_str().unwrap().to_string(), found["held"].as_str().unwrap().to_string())
}

fn start(daemon: &Daemon, pane: &str, pipeline: &str) -> Value {
    ask(daemon, "run", json!({ "action": "start", "pipeline": pipeline, "by": pane }))
}

fn show(daemon: &Daemon, run: &str) -> Value {
    ask(daemon, "run", json!({ "action": "show", "run": run }))
}

fn log_of(daemon: &Daemon) -> String {
    std::fs::read_to_string(daemon.dir.join("daemon.log")).unwrap_or_default()
}

fn ended(daemon: &Daemon, run: &str, seconds: u64) -> Value {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        let last = show(daemon, run);
        if last["run"]["endedAt"].is_number() {
            return last;
        }
        assert!(Instant::now() < deadline, "{run} didn't end: {last}\n{}", log_of(daemon));
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[test]
fn an_agent_stage_and_a_command_stage_run_on_another_machine_and_their_work_comes_back() {
    let (mbp, vm) = (machine("mbp", &[]), machine("vm", &["e2e"]));
    pair(&mbp, &vm);
    let (here, there) = (clone(&mbp), clone(&vm));
    let (task, folder, pane) = task_on(&mbp, &here);
    within(20, "mbp to learn vm's roles", || ask(&mbp, "replica", json!({ "action": "list" }))["records"].as_array().unwrap().iter().any(|r| r["task"] == "machine" && r["roles"].as_array().is_some_and(|l| l.contains(&json!("e2e")))));

    let refused = start(&mbp, &pane, "lost");
    assert_eq!(refused["error"]["code"], "no_such_machine", "{refused}");
    let said = refused["error"]["message"].as_str().unwrap();
    assert!(said.contains("lost.kojo:1: lost is on gpu: no such machine: none of your machines is named gpu or declares the role gpu (machines: mbp, vm; roles: e2e, "), "{said}");

    let started = start(&mbp, &pane, "remote");
    let run = started["run"].as_str().unwrap_or_else(|| panic!("{started}")).to_string();
    let branch = started["branch"].as_str().unwrap().to_string();
    let mut watched = Value::Null;
    within(60, "the build stage to run in a worker terminal on vm", || {
        watched = show(&mbp, &run)["workers"].as_array().and_then(|w| w.iter().find(|w| w["terminal"].as_str().is_some_and(|t| t.ends_with("@vm"))).cloned()).unwrap_or(Value::Null);
        !watched.is_null() || show(&mbp, &run)["run"]["endedAt"].is_number()
    });
    if !watched.is_null() {
        assert_eq!((watched["stage"].as_str(), watched["machine"].as_str()), (Some("build"), Some("vm")), "{watched}");
        let seen = ask(&mbp, "run", json!({ "action": "watch", "run": run }));
        assert_eq!((seen["machine"].as_str(), seen["terminal"].as_str()), (Some("vm"), watched["terminal"].as_str()), "watching names the worker's terminal on vm: {seen}");
    }
    assert!(shelf(&vm).is_empty(), "a worker is on no Shelf: {:?}", shelf(&vm));

    let done = ended(&mbp, &run, 120);
    assert_eq!(done["run"]["outcome"], "handed back", "{done}\n{}\n{}", log_of(&mbp), log_of(&vm));
    let visits = done["visits"].as_array().unwrap();
    let visit = |stage: &str| visits.iter().find(|v| v["stage"] == stage).cloned().unwrap_or_else(|| panic!("no {stage} visit in {visits:?}"));
    let (build, e2e) = (visit("build"), visit("e2e"));
    assert_eq!((build["machine"].as_str(), build["verdict"].as_str(), build["doc"]["answer"].as_str().map(str::trim)), (Some("vm"), Some("done"), Some("Built it.")), "{build}");
    assert_eq!(build["doc"]["commits"], 1, "the agent's commit came back as a bundle: {build}");
    assert_eq!((e2e["machine"].as_str(), e2e["verdict"].as_str()), (Some("vm"), Some("pass")), "{e2e}");
    let output = e2e["doc"]["feedback"].as_str().unwrap();
    let theirs = there.to_string_lossy().into_owned();
    assert!(output.contains(&format!("checked in {theirs}")) && output.contains("e2e ok") && output.contains("draft"), "the command's output comes back, and it saw the agent's uncommitted file: {output}");
    assert_eq!(git(&here, &["log", "-1", "--format=%s", &branch]), "build", "the run's branch was fast-forwarded to the peer's commit");
    assert_eq!(git(&here, &["show", &format!("{branch}:built.txt")]), theirs, "which was made in vm's clone");
    assert_eq!(git(Path::new(&folder), &["log", "-1", "--format=%s"]), "first", "the task's worktree is left alone");

    assert_eq!(shelf(&mbp).len(), 1, "one row for the task throughout");
    assert!(shelf(&mbp)[0]["id"] == task.as_str() && shelf(&vm).is_empty());
    within(30, "vm to remove the workers' worktrees once their work landed", || std::fs::read_dir(there.join(".kanna-worktrees")).map_or(true, |d| d.flatten().count() == 0));
    assert_eq!(git(&there, &["branch", "--list", "run/*"]), "", "and their branches");
    let records = ask(&mbp, "replica", json!({ "action": "list" }))["records"].as_array().unwrap().clone();
    let workers: Vec<&Value> = records.iter().filter(|r| r["kind"] == "worker").collect();
    assert_eq!(workers.len(), 2, "{workers:?}");
    assert!(workers.iter().all(|w| w["state"] == "done" && w["machine"] == "vm" && w["closed"] == true), "each worker's record replicated back with its end: {workers:?}");
}

#[test]
fn a_stage_placed_on_a_machine_that_is_offline_waits_for_it_and_within_bounds_the_wait() {
    let (mbp, mut vm) = (machine("mbp", &[]), machine("vm", &["e2e"]));
    pair(&mbp, &vm);
    let (here, _there) = (clone(&mbp), clone(&vm));
    let (_task, _folder, pane) = task_on(&mbp, &here);
    vm.stop();

    let started = start(&mbp, &pane, "far");
    let run = started["run"].as_str().unwrap_or_else(|| panic!("{started}")).to_string();
    let deadline = Instant::now() + Duration::from_secs(90);
    while show(&mbp, &run)["run"]["why"].as_str() != Some("far waits for vm, which is offline") {
        assert!(Instant::now() < deadline, "the run doesn't say its stage waits for vm, which is offline: {}\n{}", show(&mbp, &run)["run"], log_of(&mbp));
        std::thread::sleep(Duration::from_millis(300));
    }
    assert_eq!(show(&mbp, &run)["run"]["state"], "running");

    let late = start(&mbp, &pane, "late");
    let late = ended(&mbp, late["run"].as_str().unwrap_or_else(|| panic!("{late}")), 60);
    let visit = &late["visits"][0];
    assert_eq!((visit["verdict"].as_str(), visit["doc"]["note"].as_str()), (Some("fail"), Some("timed out")), "within bounds the wait: {late}");

    vm.launch();
    let done = ended(&mbp, &run, 180);
    let visit = &done["visits"][0];
    assert_eq!((done["run"]["outcome"].as_str(), visit["machine"].as_str(), visit["verdict"].as_str()), (Some("handed back"), Some("vm"), Some("fail")), "once vm is back the stage runs there (its check fails: the branch has no built.txt): {done}\n{}\n{}", log_of(&mbp), log_of(&vm));
    assert!(visit["doc"]["feedback"].as_str().unwrap().contains("checked in"), "{visit}");
}
