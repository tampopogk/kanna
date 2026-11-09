mod common;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use common::{data, is, wait_for, App, Instance};
use kanna_vt::{Lines, Terminal};
use serde_json::json;
use std::io::Write;
use std::time::{Duration, Instant};

#[test]
fn a_stalled_app_gets_one_chunk_of_a_flood_and_the_rest_waits_for_its_ack() {
    let instance = Instance::start();
    let flooded = instance.dir.join("flooded");
    let pane = instance.open("flood", Some(&format!("yes | head -c 6000000; touch '{}'; exec yes", flooded.display())));
    let mut app = instance.app();
    app.acks = false;
    app.tell("attach", json!({"terminal": pane}));
    app.until("the view to start", |m| is(m, "snapshot", &pane));
    let first = app.until("a chunk", |m| is(m, "output", &pane));
    assert!(data(&first).len() <= 64 << 10, "a chunk of {} bytes", data(&first).len());
    app.stream.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
    let mut more = 0;
    while let Some(message) = app.next() {
        more += usize::from(is(&message, "output", &pane) || is(&message, "snapshot", &pane));
    }
    wait_for("the daemon to read more of the flood than the stalled app's outbox holds", || flooded.exists());
    app.stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    while let Some(message) = app.next() {
        more += usize::from(is(&message, "output", &pane) || is(&message, "snapshot", &pane));
    }
    assert_eq!(more, 0, "the daemon sent more before the app took the first chunk");
    assert!(instance.rss_mb() < 150, "with a stalled app the daemon grew to {} MB", instance.rss_mb());
    app.stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    app.acks = true;
    app.tell("taken", json!({"terminal": pane}));
    app.until("the view to start over past what it missed", |m| is(m, "snapshot", &pane));
    app.until("more output", |m| is(m, "output", &pane));
}

#[test]
fn an_echo_is_not_held_behind_another_terminals_flood() {
    let instance = Instance::start();
    let flood = instance.open("flood", Some("yes"));
    let typed = instance.open("typed", Some("exec cat"));
    let mut app = instance.app();
    app.acks = false;
    app.tell("attach", json!({"terminal": flood}));
    app.until("the flood's view to start", |m| is(m, "snapshot", &flood));
    app.until("the flood's first chunk", |m| is(m, "output", &flood));
    app.tell("attach", json!({"terminal": typed}));
    app.until("the typed terminal's view to start", |m| is(m, "snapshot", &typed));
    std::thread::sleep(Duration::from_millis(500));
    let asked = Instant::now();
    app.tell("input", json!({"terminal": typed, "data": BASE64.encode("k")}));
    let echo = app.until("the echo", |m| is(m, "output", &typed));
    assert_eq!(data(&echo), b"k");
    assert!(asked.elapsed() < Duration::from_millis(500), "the echo took {:?}", asked.elapsed());
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
    let daemon = instance.call("read", json!({"pane": instance.pane_of(&pane), "lines": 2000}))["text"].as_str().unwrap().to_string();
    assert_eq!(replica.text(Lines::Last(2000)), daemon);
    assert!(daemon.ends_with("199999\n200000\nDONE"), "{}", &daemon[daemon.len() - 40..]);
}

fn apply(replica: &mut Terminal, message: &serde_json::Value) -> u64 {
    match message["cmd"].as_str() {
        Some("output") => replica.write(&data(message)),
        Some("grid") => {
            let side = |k: &str| message["args"][k].as_u64().unwrap() as u16;
            replica.resize(side("cols"), side("rows")).unwrap();
        }
        other => panic!("unexpected {other:?}: {message}"),
    }
    message["args"]["offset"].as_u64().unwrap_or(0)
}

#[test]
fn an_attach_can_bound_the_scrollback_and_the_rest_comes_when_asked() {
    let instance = Instance::start();
    let pane = instance.open("long", Some("seq 1 60000; exec cat"));
    wait_for("the output", || instance.screen(&pane).contains("60000"));
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    let full = app.until("the snapshot", |m| is(m, "snapshot", &pane));
    assert_eq!(full["args"]["more"], false, "{}", full["args"]["more"]);
    drop(app);
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane, "scrollback": 100}));
    let bounded = app.until("the snapshot", |m| is(m, "snapshot", &pane));
    assert_eq!(bounded["args"]["more"], true);
    assert_eq!(bounded["args"]["offset"], full["args"]["offset"]);
    assert!(data(&bounded).len() * 10 < data(&full).len(), "{} of {} bytes", data(&bounded).len(), data(&full).len());
    let replica = Terminal::from_snapshot(&data(&bounded)).unwrap();
    assert!(replica.text(Lines::Last(101)).ends_with("59999\n60000"), "{}", replica.text(Lines::Last(3)));
    assert!(replica.scrollback_rows() < 10_000);
    app.tell("scrollback", json!({"terminal": pane}));
    let rest = app.until("the whole scrollback", |m| is(m, "snapshot", &pane));
    assert_eq!(rest["args"]["more"], false);
    assert_eq!(Terminal::from_snapshot(&data(&rest)).unwrap().text(Lines::Last(100_000)), Terminal::from_snapshot(&data(&full)).unwrap().text(Lines::Last(100_000)));
}

#[test]
fn a_reconnect_from_an_offset_replays_only_the_gap() {
    let instance = Instance::start();
    let pane = instance.open("gap", Some("seq 1 200000; exec cat"));
    wait_for("the output", || instance.screen(&pane).contains("200000"));
    let mut app = instance.app();
    let run = app.hello["daemon"]["run"].as_str().unwrap().to_string();
    app.tell("attach", json!({"terminal": pane, "scrollback": 50}));
    let snapshot = app.until("the snapshot", |m| is(m, "snapshot", &pane));
    let mut replica = Terminal::from_snapshot(&data(&snapshot)).unwrap();
    let mut offset = snapshot["args"]["offset"].as_u64().unwrap();
    app.until("the grid", |m| is(m, "grid", &pane));
    app.tell("input", json!({"terminal": pane, "data": BASE64.encode("before\r")}));
    while !replica.text(Lines::Screen).contains("before\nbefore") {
        offset = apply(&mut replica, &app.until("the echo", |m| m["args"]["terminal"] == pane.as_str()));
    }
    drop(app);

    instance.call("send", json!({"pane": instance.pane_of(&pane), "text": "while away"}));
    wait_for("the input while away", || instance.screen(&pane).matches("while away").count() == 2);

    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane, "scrollback": 50, "from": offset, "run": run}));
    let first = app.until("the gap", |m| m["args"]["terminal"] == pane.as_str());
    assert_eq!(first["cmd"], "output", "a replay, not a snapshot: {first}");
    assert!(data(&first).len() < 100, "only the gap: {first}");
    apply(&mut replica, &first);
    app.tell("input", json!({"terminal": pane, "data": BASE64.encode("after\r")}));
    while !replica.text(Lines::Screen).contains("after\nafter") {
        apply(&mut replica, &app.until("the echo", |m| m["args"]["terminal"] == pane.as_str()));
    }
    let daemon = instance.call("read", json!({"pane": instance.pane_of(&pane), "lines": 30}))["text"].as_str().unwrap().to_string();
    assert_eq!(replica.text(Lines::Last(30)), daemon);
    drop(app);

    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane, "from": offset, "run": "another-daemon"}));
    let first = app.until("the screen", |m| m["args"]["terminal"] == pane.as_str());
    assert_eq!(first["cmd"], "snapshot", "an offset from another run of the daemon means nothing: {first}");
    drop(app);
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane, "from": 3, "run": run}));
    let first = app.until("the screen", |m| m["args"]["terminal"] == pane.as_str());
    assert_eq!(first["cmd"], "snapshot", "an offset older than the replay ring: {first}");
}

#[test]
fn a_snapshot_mid_sequence_waits_for_more_output() {
    let instance = Instance::start();
    let osc = format!("python3 -c 'import sys; sys.stdout.buffer.write(b\"\\033]2;\" + b\"x\" * {})'", kanna_vt::MAX_UNFINISHED_SEQUENCE_BYTES + 1);
    let written = std::env::temp_dir().join(format!("kanna-osc-written-{}", std::process::id()));
    let _ = std::fs::remove_file(&written);
    let pane = instance.open("osc", Some(&format!("printf 'before\\n'; {osc}; touch '{}'; read line; printf '\\007after\\n'; exec cat", written.display())));
    wait_for("the sequence to be written", || written.exists());
    std::thread::sleep(Duration::from_millis(500));
    let _ = std::fs::remove_file(&written);
    let mut app = instance.app();
    app.tell("attach", json!({"terminal": pane}));
    app.stream.set_read_timeout(Some(Duration::from_millis(700))).unwrap();
    while let Some(message) = app.next() {
        assert!(!is(&message, "snapshot", &pane) && !is(&message, "output", &pane), "sent mid-sequence: {message}");
    }
    app.stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    instance.call("send", json!({"pane": instance.pane_of(&pane), "text": "go"}));
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
        instance.call("close", json!({"pane": instance.pane_of(&pane)}));
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
    second.send(json!({"id": 0, "cmd": "app.hello", "args": {"protocol": kanna_build::PROTOCOL}}));
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
    instance.call("send", json!({"pane": instance.pane_of(&pane), "text": probe}));
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
    instance.call("close", json!({"pane": instance.pane_of(&pane)}));
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
    app.tell("attach", json!({"terminal": "pty-1"}));
    let socket = instance.socket();
    let opening = std::thread::spawn(move || common::call(&socket, "open", json!({"type": "term", "name": "early", "cmd": "sleep 0.5; echo EARLY-READY; exec cat"})));
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("EARLY-READY") {
        let Some(m) = app.next() else {
            panic!("no output; open answered {:?}; log:\n{}", opening.join().ok(), instance.log());
        };
        if is(&m, "output", "pty-1") {
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
    other.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": 1, "protocols": [2, 3]}}));
    let reply = other.next().unwrap();
    assert_eq!(reply["error"]["code"], "other_protocol", "{reply}");
    assert_eq!(reply["protocol"], json!(kanna_build::PROTOCOL), "{reply}");
    assert!(reply["version"].is_string() && reply["commit"].is_string(), "{reply}");
    assert_eq!(instance.call("capabilities", json!({}))["ok"], true);

    let mut app = App::raw(&instance.socket());
    app.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": 2, "protocol": kanna_build::PROTOCOL + 1}}));
    let reply = app.next().unwrap();
    assert_eq!(reply["error"]["code"], "other_protocol", "{reply}");

    let mut app = App::raw(&instance.socket());
    app.send(json!({"id": 0, "cmd": "app.hello", "args": {"pid": 2, "protocol": kanna_build::PROTOCOL}}));
    let reply = app.next().unwrap();
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["daemon"]["protocol"], json!(kanna_build::PROTOCOL), "{reply}");
}

#[test]
fn a_relays_token_goes_only_to_this_machines_own_app() {
    let instance = Instance::start();
    let set = instance.call("machine", json!({"action": "relay", "url": "https://relay.example.com:8444", "token": "s3cret"}));
    assert_eq!(set["token"], true, "{set}");
    let status = instance.call("machine", json!({"action": "status"}));
    assert_eq!((status["relay"].as_str(), status["relay_token"].as_bool()), (Some("https://relay.example.com:8444/"), Some(true)), "the status says only that there is a token: {status}");
    let mut app = instance.app();
    app.send(json!({"id": 7, "cmd": "machine.relay_token", "args": {}}));
    let reply = app.until("the token", |m| m["id"] == 7);
    assert_eq!(reply["token"], "s3cret", "{reply}");
}
