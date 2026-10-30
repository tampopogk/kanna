mod common;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use common::{data, is, wait_for, App, Instance};
use kanna_vt::{Lines, Terminal};
use serde_json::json;
use std::io::Write;
use std::time::{Duration, Instant};

#[test]
fn a_flood_is_held_back_at_the_pty() {
    let mut instance = Instance::start();
    let pane = instance.open("flood", Some("yes"));
    std::thread::sleep(Duration::from_secs(3));
    let alone = instance.rss_mb();
    assert!(alone < 150, "with no app the daemon grew to {alone} MB");

    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    app.until("the view to start", |m| is(m, "snapshot", &pane));
    let deadline = Instant::now() + Duration::from_secs(60);
    while !instance.log().contains("the app is") {
        assert!(Instant::now() < deadline, "the app never fell behind");
        assert!(instance.rss_mb() < 150, "with a stalled app the daemon grew to {} MB", instance.rss_mb());
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_secs(2));
    let stalled = instance.rss_mb();
    assert!(stalled < 150, "with a stalled app the daemon grew to {stalled} MB");

    app.until("the view to start over", |m| is(m, "snapshot", &pane));
    app.until("more output", |m| is(m, "output", &pane));

    let asked = Instant::now();
    unsafe { libc::kill(instance.pid(), libc::SIGTERM) };
    wait_for("the daemon to stop", || instance.daemon.try_wait().unwrap().is_some());
    assert!(asked.elapsed() < Duration::from_secs(2), "the daemon took {:?} to stop", asked.elapsed());
}

#[test]
fn a_resize_beyond_reason_is_refused() {
    let instance = Instance::start();
    let pane = instance.open("cat", Some("cat"));
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    app.until("the grid", |m| is(m, "grid", &pane));

    app.tell("resize", json!({"terminal": pane, "cols": 5000, "rows": 5000, "width": 0, "height": 0}));
    let grid = app.until("the grid", |m| is(m, "grid", &pane));
    assert_eq!((grid["args"]["cols"].as_u64(), grid["args"]["rows"].as_u64()), (Some(80), Some(24)), "{grid}");

    app.tell("resize", json!({"terminal": pane, "cols": 1000, "rows": 1000, "width": 0, "height": 0}));
    let grid = app.until("the grid", |m| is(m, "grid", &pane));
    assert_eq!((grid["args"]["cols"].as_u64(), grid["args"]["rows"].as_u64()), (Some(1000), Some(1000)), "{grid}");
}

#[test]
fn an_attach_sends_the_screen_then_its_grid() {
    let instance = Instance::start();
    let pane = instance.open("hello", Some("printf 'first\\n\\033[1;31mhello\\033[0m'; exec cat"));
    wait_for("the output", || instance.screen(&pane).contains("hello"));
    for _ in 0..2 {
        let mut app = instance.app();
        app.tell("attach", json!({"terminal": pane}));
        let first = app.until("the snapshot", |m| m["args"]["terminal"] == pane.as_str());
        assert_eq!(first["cmd"], "snapshot", "{first}");
        let replica = Terminal::from_snapshot(&data(&first)).unwrap();
        assert!(replica.text(Lines::Screen).starts_with("first\nhello"), "{}", replica.text(Lines::Screen));
        let grid = app.until("the grid", |m| m["args"]["terminal"] == pane.as_str());
        assert_eq!(grid["cmd"], "grid", "{grid}");
        assert_eq!((grid["args"]["cols"].as_u64(), grid["args"]["rows"].as_u64()), (Some(80), Some(24)));
        app.tell("input", json!({"terminal": pane, "data": BASE64.encode("typed\r")}));
        let echoed = app.until("the echo", |m| is(m, "output", &pane));
        assert!(String::from_utf8_lossy(&data(&echoed)).contains("typed"), "{echoed}");
    }
}

#[test]
fn a_replica_misses_nothing_and_repeats_nothing() {
    let instance = Instance::start();
    let pane = instance.open("count", Some("sleep 0.2; seq 1 200000; printf 'DONE\\n'; exec cat"));
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    let snapshot = app.until("the snapshot", |m| is(m, "snapshot", &pane));
    let mut replica = Terminal::from_snapshot(&data(&snapshot)).unwrap();
    let mut done = replica.text(Lines::Screen).contains("DONE");
    while !done {
        let message = app.until("more", |m| m["args"]["terminal"] == pane.as_str());
        match message["cmd"].as_str() {
            Some("output") => replica.write(&data(&message)),
            Some("grid") => {
                let side = |k: &str| message["args"][k].as_u64().unwrap() as u16;
                replica.resize(side("cols"), side("rows")).unwrap();
            }
            other => panic!("unexpected {other:?}: {message}"),
        }
        done = replica.text(Lines::Screen).contains("DONE");
    }
    let daemon = instance.call("read", json!({"pane": pane, "lines": 2000}))["text"].as_str().unwrap().to_string();
    assert_eq!(replica.text(Lines::Last(2000)), daemon);
    assert!(daemon.ends_with("199999\n200000\nDONE"), "{}", &daemon[daemon.len() - 40..]);
}

#[test]
fn a_snapshot_mid_sequence_waits_for_more_output() {
    let instance = Instance::start();
    let pane = instance.open("apc", Some("printf 'before\\n\\033_a\\235'; read line; printf '\\033\\\\after\\n'; exec cat"));
    wait_for("the output", || instance.screen(&pane).contains("before"));
    std::thread::sleep(Duration::from_millis(200));
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    app.stream.set_read_timeout(Some(Duration::from_millis(700))).unwrap();
    while let Some(message) = app.next() {
        assert!(!is(&message, "snapshot", &pane) && !is(&message, "output", &pane), "sent mid-APC: {message}");
    }
    app.stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    instance.call("send", json!({"pane": pane, "text": "go"}));
    let snapshot = app.until("the snapshot", |m| m["args"]["terminal"] == pane.as_str());
    assert_eq!(snapshot["cmd"], "snapshot", "{snapshot}");
    let replica = Terminal::from_snapshot(&data(&snapshot)).unwrap();
    assert!(replica.text(Lines::Screen).contains("after"), "{}", replica.text(Lines::Screen));
}

#[test]
fn the_daemon_answers_queries_with_no_app() {
    let instance = Instance::start();
    let probe = r#"python3 -c 'import os,tty; tty.setraw(0); os.write(1,b"\033[3;5H\033[6n\033[c\033[18t"); r=b""
while not r.endswith(b"t"): r+=os.read(0,64)
os.write(1,b"\r\ngot "+r.replace(b"\033",b"^")+b"\r\n")'; exec cat"#;
    let pane = instance.open("queries", Some(probe));
    wait_for("the replies", || instance.screen(&pane).contains("got "));
    let screen = instance.screen(&pane);
    assert!(screen.contains("got ^[3;5R^[?62;22;52c^[8;24;80t"), "{screen}");
}

#[test]
fn a_resize_reaches_programs_that_asked_for_in_band_reports() {
    let instance = Instance::start();
    let probe = r#"python3 -c 'import os,tty; tty.setraw(0); os.write(1,b"\033[?2048hready"); r=b""
while r.count(b"t") < 2: r+=os.read(0,64)
os.write(1,b"\r\ngot "+r.replace(b"\033",b"^")+b"\r\n")'; exec cat"#;
    let pane = instance.open("inband", Some(probe));
    wait_for("the probe", || instance.screen(&pane).contains("ready"));
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    app.tell("resize", json!({"terminal": pane, "cols": 100, "rows": 30, "width": 1000, "height": 600}));
    wait_for("the report", || instance.screen(&pane).contains("got "));
    let screen = instance.screen(&pane);
    assert!(screen.contains("got ^[48;24;80;0;0t^[48;30;100;600;1000t"), "{screen}");
}

#[test]
fn exited_comes_after_the_last_output() {
    let instance = Instance::start();
    for round in 0..5 {
        let name = format!("burst{round}");
        let pane = instance.open(&name, Some("sleep 0.3; head -c 2000000 /dev/zero | tr '\\0' x; printf END; (exit 7)"));
        let mut app = instance.app();
        app.tell("attach", json!({"terminal": pane}));
        let mut replica: Option<Terminal> = None;
        let exited = app.until("the exit", |m| {
            if is(m, "snapshot", &pane) {
                replica = Some(Terminal::from_snapshot(&data(m)).unwrap());
            } else if is(m, "output", &pane) {
                replica.as_mut().expect("output before the snapshot").write(&data(m));
            }
            is(m, "exited", &pane)
        });
        assert_eq!(exited["args"]["code"], 7, "{exited}");
        let screen = replica.expect("no snapshot").text(Lines::Screen);
        assert!(screen.ends_with("END"), "round {round}: exited came before the last output (the screen ends {:?})", &screen[screen.len().saturating_sub(20)..]);
        drop(app);
        instance.call("close", json!({"pane": pane}));
    }
}

#[test]
fn a_request_the_app_never_answers_times_out() {
    let instance = Instance::start_with(&[("KANNA_APP_TIMEOUT_MS", "500")]);
    let mut app = instance.app();
    let asked = Instant::now();
    let reply = instance.call("debug.app", json!({}));
    assert_eq!(reply["ok"], false, "{reply}");
    assert_eq!(reply["error"]["code"], "timeout", "{reply}");
    assert!(asked.elapsed() < Duration::from_secs(3));

    let request = app.until("the request", |m| m["cmd"] == "debug.app");
    app.send(json!({"id": request["id"], "ok": true, "late": true}));
    let mut second = App::raw(&instance.socket());
    second.send(json!({"id": 0, "cmd": "app.hello", "args": {"protocols": kanna_build::PROTOCOLS}}));
    assert_eq!(second.next().unwrap()["error"]["code"], "app_attached");
}

#[test]
fn an_endless_line_drops_the_app() {
    let instance = Instance::start();
    let mut app = instance.app();
    let mut writer = app.stream.try_clone().unwrap();
    std::thread::spawn(move || {
        let chunk = vec![b'x'; 1 << 20];
        for _ in 0..64 {
            if writer.write_all(&chunk).is_err() {
                return;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.next().is_some() {
        assert!(Instant::now() < deadline, "the daemon kept reading the line");
    }
    assert!(instance.rss_mb() < 100, "the daemon grew to {} MB", instance.rss_mb());
    wait_for("the daemon to let the app go", || instance.log().contains("the app went away"));
    instance.app();
}

#[test]
fn pixel_sizes_reach_the_pty() {
    let instance = Instance::start();
    let pane = instance.open("sh", None);
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    app.tell("resize", json!({"terminal": pane, "cols": 90, "rows": 30, "width": 900, "height": 600}));
    app.until("the grid", |m| is(m, "grid", &pane) && m["args"]["cols"] == 90);
    app.tell("resize", json!({"terminal": pane, "cols": 90, "rows": 30, "width": 1800, "height": 1200}));
    app.until("the grid", |m| is(m, "grid", &pane));
    let probe = "python3 -c 'import fcntl,termios,struct;print(\"size\",*struct.unpack(\"HHHH\",fcntl.ioctl(0,termios.TIOCGWINSZ,bytes(8))))'";
    instance.call("send", json!({"pane": pane, "text": probe}));
    wait_for("the probe", || instance.screen(&pane).contains("size "));
    let screen = instance.screen(&pane);
    assert!(screen.contains("size 30 90 1800 1200"), "{screen}");
}

#[test]
fn a_closed_pane_lets_go_of_its_pty() {
    let instance = Instance::start();
    let pidfile = instance.dir.join("orphan.pid");
    let cmd = format!("perl -MPOSIX -e 'if (!fork) {{ setsid; open my $f, \">\", \"{}\"; print $f $$; close $f; sleep 60; exit }}'; sleep 60", pidfile.display());
    let pane = instance.open("orphan", Some(&cmd));
    wait_for("the orphan", || std::fs::read_to_string(&pidfile).is_ok_and(|p| !p.is_empty()));
    let orphan: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    assert!(instance.ptys_open() > 0);
    instance.call("close", json!({"pane": pane}));
    let deadline = Instant::now() + Duration::from_secs(5);
    while instance.ptys_open() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let left = instance.ptys_open();
    unsafe { libc::kill(orphan, libc::SIGKILL) };
    assert_eq!(left, 0, "the daemon still holds the closed pane's pty");
}

#[test]
fn output_inside_an_open_sequence_leaves_the_daemon_responsive() {
    let instance = Instance::start();
    let pane = instance.open("apc-flood", Some("printf 'x\\n'; i=0; line=$(printf '%0200d' 0); while [ $i -lt 12000 ]; do printf '\\033[3%dm%s\\033[0m\\n' $((i % 8)) \"$line\"; i=$((i+1)); done; printf '\\033_a\\235'; exec yes 0123456789abcdef"));
    wait_for("the scrollback", || instance.screen(&pane).contains("0000000000"));
    std::thread::sleep(Duration::from_secs(3));
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    for _ in 0..20 {
        app.tell("attach", json!({"terminal": pane}));
    }
    let _answering = app.answering();
    std::thread::sleep(Duration::from_millis(500));
    let started = Instant::now();
    for _ in 0..20 {
        instance.call("list", json!({}));
    }
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(2), "20 lists took {elapsed:?}");
}

#[test]
fn an_attach_that_comes_before_the_terminal_starts_gets_its_output() {
    let instance = Instance::start();
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": "term-1"}));
    let socket = instance.socket();
    let opening = std::thread::spawn(move || common::call(&socket, "open", json!({"type": "term", "name": "early", "cmd": "sleep 0.5; echo EARLY-READY; exec cat"})));
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("EARLY-READY") {
        let Some(m) = app.next() else {
            panic!("no output; open answered {:?}; log:\n{}", opening.join().ok(), instance.log());
        };
        if is(&m, "output", "term-1") {
            seen.extend(data(&m));
        }
    }
    assert_eq!(opening.join().unwrap()["ok"], true);
}

#[test]
fn every_debug_command_goes_to_the_app_with_pane_names_resolved() {
    let instance = Instance::start();
    instance.open("named", Some("exec sleep 60"));
    let opened = instance.pane("named")["id"].clone();
    let reply = instance.call("debug.click", json!({"pane": "named"}));
    assert_eq!(reply["error"]["code"], "no_app", "{reply}");
    assert_eq!(instance.call("capabilities", json!({}))["app"], false);

    let mut app = instance.app();
    let socket = instance.socket();
    let asking = std::thread::spawn(move || common::call(&socket, "debug.whatever", json!({"pane": "named", "x": 1})));
    let request = app.until("the request", |m| m["cmd"] == "debug.whatever");
    assert_eq!(request["args"], json!({"pane": opened, "x": 1}));
    app.send(json!({"id": request["id"], "ok": true, "seen": true}));
    let reply = asking.join().unwrap();
    assert_eq!(reply["seen"], true, "{reply}");
    assert_eq!(instance.call("debug.click", json!({"pane": "nope"}))["error"]["code"], "not_found");
    assert_eq!(instance.call("nope.click", json!({}))["error"]["code"], "bad_request");

    let socket = instance.socket();
    let asking = std::thread::spawn(move || common::call(&socket, "capabilities", json!({})));
    let capabilities = asking.join().unwrap();
    assert_eq!(capabilities["app"], true, "{capabilities}");
    assert_eq!(capabilities["mux"], "kmux");
    assert_eq!(capabilities["instance"], "default");
    assert!(capabilities["commands"].as_array().unwrap().contains(&json!("read")));
}

#[test]
fn an_app_of_another_protocol_is_told_so_and_the_daemon_keeps_running() {
    let instance = Instance::start();
    let mut other = App::raw(&instance.socket());
    other.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": 1, "protocols": [7, 9]}}));
    let reply = other.next().unwrap();
    assert_eq!(reply["error"]["code"], "other_protocol", "{reply}");
    assert_eq!(reply["mine"], json!(kanna_build::PROTOCOLS), "{reply}");
    assert!(reply["version"].is_string() && reply["commit"].is_string(), "{reply}");
    assert_eq!(instance.call("capabilities", json!({}))["ok"], true);

    let mut app = App::raw(&instance.socket());
    app.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": 2, "protocols": [1, 4]}}));
    let reply = app.next().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["daemon"]["protocols"], json!(kanna_build::PROTOCOLS), "{reply}");
}
