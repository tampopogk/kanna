mod common;

use common::{alive, data, is, wait_for, App, Instance};
use kanna_vt::{Lines, Terminal};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn shows(instance: &Instance, pane: &str, text: &str) -> bool {
    instance.screen(pane).contains(text)
}

fn saved(instance: &Instance) -> Value {
    let text = std::fs::read_to_string(instance.dir.join("state/terminals/terminals.json")).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

#[test]
fn a_restart_keeps_a_running_shell_its_screen_and_what_it_wrote_meanwhile() {
    let mut instance = Instance::start();
    let pane = instance.open("shell", None);
    let pid = instance.pane("shell")["pid"].as_i64().unwrap();
    instance.call("send", json!({"pane": "shell", "text": "echo before-$((6*7))"}));
    wait_for("the shell's output", || shows(&instance, &pane, "before-42"));
    instance.call("send", json!({"pane": "shell", "text": "sleep 1; echo during-$((6*7))"}));

    instance.stop(libc::SIGTERM);
    assert!(alive(pid), "stopping the daemon ended the shell");
    std::thread::sleep(Duration::from_millis(1500));
    instance.start_again();

    let shell = instance.pane("shell");
    assert_eq!((shell["terminal"].as_str(), shell["pid"].as_i64(), shell["state"].as_str()), (Some(pane.as_str()), Some(pid), Some("running")), "{shell}");
    assert!(shows(&instance, &pane, "before-42"), "{}", instance.screen(&pane));
    wait_for("what the shell wrote while the daemon was down", || shows(&instance, &pane, "during-42"));
    instance.call("send", json!({"pane": "shell", "text": "echo after-$((6*7))"}));
    wait_for("the shell to answer again", || shows(&instance, &pane, "after-42"));

    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    let snapshot = app.until("the snapshot", |m| is(m, "snapshot", &pane));
    let replica = Terminal::from_snapshot(&data(&snapshot)).unwrap().text(Lines::Screen);
    assert!(replica.contains("before-42") && replica.contains("during-42") && replica.contains("after-42"), "{replica}");
    assert!(instance.log().contains("took back 1 terminal"), "{}", instance.log());
}

#[test]
fn a_full_screen_program_redraws_after_a_restart() {
    let mut instance = Instance::start();
    let script = r#"exec /bin/sh -c 'printf "\033[?1049h\033[Hfull screen"; n=0; trap "n=\$((n+1)); printf \"\033[2J\033[Hredrawn %s\" \$n" WINCH; while :; do sleep 0.1; done'"#;
    let pane = instance.open("full", Some(script));
    wait_for("the program to draw", || shows(&instance, &pane, "full screen"));
    std::thread::sleep(Duration::from_millis(1500));

    instance.restart(libc::SIGTERM);
    wait_for("the program to redraw", || shows(&instance, &pane, "redrawn 1"));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(instance.screen(&pane), "redrawn 1");
}

#[test]
fn an_exit_while_the_daemon_is_down_is_reported_after_it_comes_back() {
    let mut instance = Instance::start();
    let early = instance.open("early", Some("echo early-out; exit 3"));
    wait_for("early to exit", || instance.pane("early")["state"] == "exited");
    let late = instance.open("late", Some("sleep 1; echo late-out; exit 7"));
    let late_pid = instance.pane("late")["pid"].as_i64().unwrap();

    instance.stop(libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(2000));
    instance.start_again();

    wait_for("late's exit", || instance.pane("late")["state"] == "exited");
    let pane = instance.pane("late");
    assert_eq!((pane["exitCode"].as_i64(), pane["pid"].as_i64()), (Some(7), Some(late_pid)), "{pane}");
    assert!(shows(&instance, &late, "late-out"), "{}", instance.screen(&late));
    let pane = instance.pane("early");
    assert_eq!((pane["state"].as_str(), pane["exitCode"].as_i64()), (Some("exited"), Some(3)), "{pane}");
    assert!(shows(&instance, &early, "early-out"), "{}", instance.screen(&early));

    let mut app = instance.app();
    app.tell("attach", json!({"terminal": late}));
    let exited = app.until("late's exit", |m| is(m, "exited", &late));
    assert_eq!(exited["args"]["code"], 7, "{exited}");

    instance.restart(libc::SIGTERM);
    let pane = instance.pane("late");
    assert_eq!((pane["state"].as_str(), pane["exitCode"].as_i64()), (Some("exited"), Some(7)), "{pane}");
}

#[test]
fn a_crashed_daemon_loses_nothing_it_had_saved() {
    let mut instance = Instance::start();
    let pane = instance.open("shell", None);
    let pid = instance.pane("shell")["pid"].as_i64().unwrap();
    instance.call("send", json!({"pane": "shell", "text": "printf 'saved-%s\\n' $((6*7))"}));
    wait_for("the output", || shows(&instance, &pane, "saved-42"));
    wait_for("the screen on disk", || instance.dir.join(format!("state/terminals/{pane}.snap")).exists());
    std::thread::sleep(Duration::from_millis(1500));
    let on_disk = std::fs::read(instance.dir.join(format!("state/terminals/{pane}.snap"))).unwrap();
    assert_eq!(Terminal::from_snapshot(&on_disk).unwrap().text(Lines::Screen), instance.screen(&pane));

    instance.restart(libc::SIGKILL);
    let shell = instance.pane("shell");
    assert_eq!((shell["terminal"].as_str(), shell["pid"].as_i64()), (Some(pane.as_str()), Some(pid)), "{shell}");
    assert!(shows(&instance, &pane, "saved-42"), "{}", instance.screen(&pane));
    instance.call("send", json!({"pane": "shell", "text": "echo again-$((6*7))"}));
    wait_for("the shell to answer", || shows(&instance, &pane, "again-42"));
}

#[test]
fn terminal_ids_never_repeat_across_restarts() {
    let mut instance = Instance::start();
    let first = instance.open("one", Some("exec sleep 600"));
    let second = instance.open("two", Some("exec sleep 600"));
    instance.call("close", json!({"pane": second}));
    assert_eq!(saved(&instance)["counters"]["terminal"], 2);

    instance.restart(libc::SIGKILL);
    let third = instance.open("three", Some("exec sleep 600"));
    assert_eq!((first.as_str(), second.as_str(), third.as_str()), ("term-1", "term-2", "term-3"));
    assert_eq!(instance.pane("three")["id"], "term-3");

    instance.call("close", json!({"pane": first}));
    instance.call("close", json!({"pane": third}));
    instance.restart(libc::SIGTERM);
    assert!(instance.call("list", json!({}))["panes"].as_array().unwrap().is_empty());
    assert_eq!(instance.open("four", None), "term-4");
    assert_eq!(instance.pane("four")["id"], "term-4");
    let restarted = instance.call("restart", json!({"pane": "four"}));
    assert_eq!(restarted["pane"]["id"], "term-5", "a restart is a new terminal: {restarted}");
    assert_eq!((instance.pane("four")["id"].clone(), instance.pane("four")["terminal"].clone()), (json!("term-5"), json!("term-5")));
    let list = instance.call("list", json!({}));
    assert_eq!(list["windows"][0]["tabs"][0]["layout"], json!({"pane": "four"}), "the layout shows the new terminal: {list}");
    assert_eq!(list["panes"].as_array().map(Vec::len), Some(1), "{list}");
}

#[test]
fn what_the_keeper_has_and_the_disk_does_not_is_taken_back_and_the_reverse_dropped() {
    let mut instance = Instance::start();
    let kept = instance.open("kept", Some("echo kept; exec sleep 600"));
    let lost = instance.open("lost", Some("exec sleep 600"));
    let kept_pid = instance.pane("kept")["pid"].as_i64().unwrap();
    let lost_pid = instance.pane("lost")["pid"].as_i64().unwrap();

    instance.stop(libc::SIGKILL);
    keeper(&instance, json!({"op": "close", "id": lost}));
    wait_for("the keeper to end lost", || !alive(lost_pid));
    std::fs::remove_file(instance.dir.join("state/terminals/terminals.json")).unwrap();
    instance.start_again();

    let panes = instance.call("list", json!({}))["panes"].clone();
    let panes = panes.as_array().unwrap();
    assert_eq!(panes.len(), 1, "{panes:?}");
    assert_eq!((panes[0]["terminal"].as_str(), panes[0]["pid"].as_i64(), panes[0]["cmd"].as_str()), (Some(kept.as_str()), Some(kept_pid), Some("echo kept; exec sleep 600")));
    assert!(panes[0]["name"].is_null());
    let new = instance.open("new", None);
    assert_ne!(new, kept);

    instance.restart(libc::SIGTERM);
    let restored = saved(&instance);
    let ids: Vec<&str> = restored["terminals"].as_array().unwrap().iter().filter_map(|t| t["id"].as_str()).collect();
    assert_eq!(ids, [kept.as_str(), new.as_str()]);
}

#[test]
fn a_second_daemon_on_the_same_folder_is_refused() {
    let instance = Instance::start();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_kanna-daemon"))
        .env("KANNA_SOCKET", instance.dir.join("other.sock"))
        .env("KANNA_STATE_DIR", instance.dir.join("state"))
        .env("HOME", instance.dir.join("home"))
        .stderr(std::process::Stdio::piped())
        .output()
        .unwrap();
    assert!(!status.status.success());
    assert!(String::from_utf8_lossy(&status.stderr).contains("another daemon"), "{}", String::from_utf8_lossy(&status.stderr));
}

fn keeper(instance: &Instance, request: Value) -> Value {
    let stream = UnixStream::connect(instance.dir.join("state/keeper.sock")).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    (&stream).write_all(format!("{request}\n").as_bytes()).unwrap();
    let mut reader = BufReader::new(&stream);
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        if reply.get("ok").is_some() {
            return reply;
        }
    }
}

#[test]
fn a_task_keeps_the_folder_it_opened_in_across_a_restart() {
    let mut instance = Instance::start();
    let folder = std::env::temp_dir().join(format!("kanna-kept-{}", std::process::id()));
    std::fs::create_dir_all(folder.join("sub")).unwrap();
    let folder = folder.canonicalize().unwrap();
    let opened = instance.call("open", json!({"type": "term", "name": "task", "cwd": folder, "cmd": "cd sub; exec cat"}));
    let task = opened["pane"]["id"].as_str().unwrap_or_else(|| panic!("{opened}")).to_string();
    instance.restart(libc::SIGTERM);
    let _answering = instance.app().answering();
    let beside = instance.call("open", json!({"type": "term", "from": task, "cmd": "pwd; exec cat"}));
    let beside = beside["pane"]["id"].as_str().unwrap_or_else(|| panic!("{beside}")).to_string();
    wait_for("the task's folder", || instance.screen(&beside).lines().any(|line| line == folder.to_string_lossy()));
    std::fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn an_app_from_another_build_stops_the_daemon_and_the_next_keeps_its_terminals() {
    let mut instance = Instance::start();
    let pane = instance.open("shell", None);
    let pid = instance.pane("shell")["pid"].as_i64().unwrap();
    let build = instance.call("capabilities", json!({}))["build"].as_str().unwrap().to_string();
    assert_eq!(build.len(), 64, "{build}");

    let mut same = App::raw(&instance.socket());
    same.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": 1, "build": build, "protocols": kanna_build::PROTOCOLS, "panes": []}}));
    let hello = same.next().unwrap();
    assert_eq!((hello["ok"].as_bool(), hello["daemon"]["build"].as_str()), (Some(true), Some(build.as_str())), "{hello}");
    drop(same);

    let mut other = App::raw(&instance.socket());
    wait_for("the first app to go", || {
        other.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": 2, "build": "0000", "protocols": kanna_build::PROTOCOLS, "panes": []}}));
        let reply = other.next().unwrap();
        if reply["error"]["code"] == "app_attached" {
            other = App::raw(&instance.socket());
            return false;
        }
        assert_eq!(reply["error"]["code"], "other_build", "{reply}");
        true
    });
    wait_for("the daemon to stop", || instance.daemon.try_wait().unwrap().is_some());
    assert!(alive(pid), "replacing the daemon ended the shell");

    instance.start_again();
    let shell = instance.pane("shell");
    assert_eq!((shell["terminal"].as_str(), shell["pid"].as_i64()), (Some(pane.as_str()), Some(pid)), "{shell}");
}

#[test]
fn a_terminal_started_again_after_a_restart_keeps_its_size_even_without_its_screen() {
    let mut instance = Instance::start();
    let pane = instance.open("sized", Some("read line; stty size"));
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    app.tell("resize", json!({"terminal": pane, "cols": 101, "rows": 31, "width": 0, "height": 0}));
    app.until("the new grid", |m| is(m, "grid", &pane) && m["args"]["cols"] == 101);
    instance.call("send", json!({"pane": "sized", "text": ""}));
    wait_for("the size", || shows(&instance, &pane, "31 101"));
    drop(app);
    wait_for("the exit", || instance.pane("sized")["state"] == "exited");

    instance.stop(libc::SIGTERM);
    assert_eq!(saved(&instance)["terminals"][0]["cols"], 101, "{}", saved(&instance));
    let _ = std::fs::remove_file(instance.dir.join(format!("state/terminals/{pane}.snap")));
    instance.start_again();

    instance.call("restart", json!({"pane": "sized"}));
    wait_for("the program again", || instance.pane("sized")["state"] == "running");
    instance.call("send", json!({"pane": "sized", "text": ""}));
    wait_for("the size again", || shows(&instance, "sized", "31 101"));
}
