mod common;

use common::{alive, request, App, Instance};
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn exits_within(instance: &mut Instance, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if instance.daemon.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn cpu_seconds(pid: i32) -> f64 {
    let out = Command::new("ps").args(["-o", "time=", "-p", &pid.to_string()]).output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim().split(':').rev().zip([1.0, 60.0, 3600.0]).map(|(f, scale)| f.parse::<f64>().unwrap_or(0.0) * scale).sum()
}

fn saved(instance: &Instance) -> Value {
    let text = std::fs::read_to_string(instance.dir.join("state/terminals/terminals.json")).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

#[test]
fn shutdown_quits_the_app_hangs_up_every_terminal_stops_the_keeper_and_removes_the_socket_last() {
    let mut instance = Instance::start();
    let terminal = instance.open("sleeper", Some("sleep 600 & wait"));
    let pid = instance.pane("sleeper")["pid"].as_i64().unwrap();
    let keeper = instance.keeper_pid();
    let mut app = instance.app();

    let reply = instance.call("shutdown", json!({}));
    assert_eq!((reply["ok"].clone(), reply["stopping"].clone()), (json!(true), json!(true)), "{reply}");
    app.until("the quit notice", |m| m["cmd"] == "quit");
    let refused = request(&instance.socket(), json!({"id": 1, "cmd": "list"}));
    assert_eq!(refused["error"]["code"], "stopping", "{refused}");
    let mut late = App::raw(&instance.socket());
    late.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": 1, "protocols": kanna_build::PROTOCOLS}}));
    assert_eq!(late.next().unwrap()["error"]["code"], "stopping");
    assert!(instance.socket().exists() && alive(pid), "nothing stops before the app has gone");
    drop(app);

    assert!(exits_within(&mut instance, Duration::from_secs(10)), "{}", instance.log());
    assert!(instance.daemon.try_wait().unwrap().unwrap().success(), "{}", instance.log());
    assert!(!instance.socket().exists() && !instance.dir.join("state/daemon.pid").exists());
    assert!(!alive(pid) && !alive(keeper as i64), "{}", instance.log());
    let record = &saved(&instance)["terminals"][0];
    assert_eq!((record["id"].as_str(), record["state"].as_str(), record["hungUp"].as_bool()), (Some(terminal.as_str()), Some("exited"), Some(true)), "{record}");

    instance.start_again();
    let pane = instance.pane("sleeper");
    assert_eq!((pane["terminal"].as_str(), pane["state"].as_str()), (Some(terminal.as_str()), Some("exited")), "{pane}");
}

#[test]
fn the_daemon_shuts_down_when_its_owner_exits_even_with_an_app_that_doesnt_quit() {
    let mut owner = Command::new("/bin/sleep").arg("600").stdin(Stdio::null()).spawn().unwrap();
    let owner_pid = owner.id().to_string();
    let mut instance = Instance::start_with(&[("KANNA_E2E_OWNER", &owner_pid)]);
    instance.open("sleeper", Some("exec sleep 600"));
    let pid = instance.pane("sleeper")["pid"].as_i64().unwrap();
    let keeper = instance.keeper_pid();
    let mut app = instance.app();

    unsafe { libc::kill(owner.id() as i32, libc::SIGKILL) };
    let _ = owner.wait();
    app.until("the quit notice", |m| m["cmd"] == "quit");
    assert!(exits_within(&mut instance, Duration::from_secs(15)), "{}", instance.log());
    assert!(instance.log().contains("didn't quit in 5s"), "{}", instance.log());
    assert!(!instance.socket().exists() && !alive(pid) && !alive(keeper as i64), "{}", instance.log());

    let dir = instance.dir.join("again");
    let status = Command::new(env!("CARGO_BIN_EXE_kanna-daemon"))
        .env("KANNA_E2E_OWNER", &owner_pid)
        .env("KANNA_SOCKET", dir.join("kanna.sock"))
        .env("KANNA_STATE_DIR", &dir)
        .env("KANNA_AGENT_RULES_URL", "none")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success() && !dir.join("kanna.sock").exists() && !dir.join("keeper.sock").exists(), "a daemon whose owner is already gone doesn't start");
}

#[test]
fn a_keeper_left_by_a_sigtermed_daemon_stops_when_their_owner_exits() {
    let mut owner = Command::new("/bin/sleep").arg("600").stdin(Stdio::null()).spawn().unwrap();
    let mut instance = Instance::start_with(&[("KANNA_E2E_OWNER", &owner.id().to_string())]);
    instance.open("sleeper", Some("exec sleep 600"));
    let pid = instance.pane("sleeper")["pid"].as_i64().unwrap();
    let keeper = instance.keeper_pid();
    let _app = instance.app();

    instance.stop(libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(500));
    let before = cpu_seconds(keeper);
    std::thread::sleep(Duration::from_secs(2));
    let used = cpu_seconds(keeper) - before;
    assert!(used < 0.3, "the orphaned keeper used {used} s of CPU in 2 s");
    assert!(alive(keeper as i64) && alive(pid), "{}", instance.log());
    unsafe { libc::kill(owner.id() as i32, libc::SIGKILL) };
    let _ = owner.wait();
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(keeper as i64) || alive(pid) {
        assert!(Instant::now() < deadline, "the keeper outlived its owner");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_daemon_waiting_for_the_folder_while_it_shuts_down_doesnt_start() {
    let mut instance = Instance::start_with(&[("KANNA_KEEPER_CLOSE_GRACE_MS", "2000")]);
    instance.open("stubborn", Some("trap '' HUP; sleep 600 & wait"));
    let mut waiting = Command::new(env!("CARGO_BIN_EXE_kanna-daemon"))
        .env("HOME", instance.dir.join("home"))
        .env("KANNA_SOCKET", instance.socket())
        .env("KANNA_STATE_DIR", instance.dir.join("state"))
        .env("KANNA_AGENT_RULES_URL", "none")
        .stdin(Stdio::null())
        .stderr(std::fs::File::create(instance.dir.join("waiting.log")).unwrap())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    instance.call("shutdown", json!({}));
    assert!(exits_within(&mut instance, Duration::from_secs(10)), "{}", instance.log());
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = waiting.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the waiting daemon started: {}", std::fs::read_to_string(instance.dir.join("waiting.log")).unwrap_or_default());
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success() && !instance.socket().exists());
}

#[test]
fn a_daemon_started_during_the_shutdown_doesnt_start_however_late_it_gets_to_the_folder() {
    let mut instance = Instance::start_with(&[("KANNA_KEEPER_CLOSE_GRACE_MS", "2000")]);
    instance.open("stubborn", Some("trap '' HUP; sleep 600 & wait"));
    let mut late = Command::new("/bin/sh")
        .args(["-c", "while [ -e \"$KANNA_SOCKET\" ]; do sleep 0.05; done; sleep 0.5; exec \"$0\"", env!("CARGO_BIN_EXE_kanna-daemon")])
        .env("HOME", instance.dir.join("home"))
        .env("KANNA_SOCKET", instance.socket())
        .env("KANNA_STATE_DIR", instance.dir.join("state"))
        .env("KANNA_AGENT_RULES_URL", "none")
        .stdin(Stdio::null())
        .stderr(std::fs::File::create(instance.dir.join("late.log")).unwrap())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    instance.call("shutdown", json!({}));
    assert!(exits_within(&mut instance, Duration::from_secs(10)), "{}", instance.log());
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = late.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the late daemon started: {}", std::fs::read_to_string(instance.dir.join("late.log")).unwrap_or_default());
        std::thread::sleep(Duration::from_millis(50));
    };
    let log = std::fs::read_to_string(instance.dir.join("late.log")).unwrap_or_default();
    assert!(status.success() && !instance.socket().exists() && log.contains("not starting"), "{log}");
}
