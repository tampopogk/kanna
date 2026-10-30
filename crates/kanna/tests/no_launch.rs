use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn with_kanna_socket_set_an_unreachable_daemon_fails_with_3_and_starts_nothing() {
    let dir = std::env::temp_dir().join(format!("kanna-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for mux in [None, Some("kanna")] {
        let started = Instant::now();
        let mut command = Command::new(env!("CARGO_BIN_EXE_kanna"));
        if let Some(mux) = mux {
            command.args(["--mux", mux]);
        }
        let output = command
            .arg("capabilities")
            .env("KANNA_SOCKET", dir.join("nothing.sock"))
            .env("KANNA_APP", "/nonexistent/Kanna.app")
            .env_remove("KANNA_MUX")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(3), "{mux:?}: {stderr}");
        assert!(stderr.contains("doesn't start an app"), "{mux:?}: {stderr}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn kanna(dir: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kanna"));
    command
        .args(args)
        .env("KANNA_SOCKET", dir.join("nothing.sock"))
        .env("KANNA_APP", "/nonexistent/Kanna.app")
        .env("KMUX_APP", "/nonexistent/kmux.app")
        .env_remove("KANNA_INSTANCE")
        .env_remove("KANNA_PANE")
        .env_remove("KANNA_TERMINAL")
        .env_remove("KMUX_SOCKET")
        .env_remove("KANNA_MUX");
    command
}

fn folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn serve(path: &std::path::Path, answer: fn(&serde_json::Value) -> serde_json::Value) -> std::sync::mpsc::Receiver<serde_json::Value> {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
    let (seen, requests) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut writer = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                let mut reply = answer(&request);
                reply["id"] = request["id"].clone();
                let _ = seen.send(request);
                let _ = writeln!(writer, "{reply}");
            }
        }
    });
    requests
}

#[test]
fn with_the_kmux_backend_its_own_socket_variable_decides_and_is_named() {
    let dir = folder("kmux");
    let output = kanna(&dir, &["--mux", "kmux", "capabilities"]).env_remove("KANNA_SOCKET").env("KMUX_SOCKET", dir.join("nothing.sock")).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "{stderr}");
    assert!(stderr.contains("KMUX_SOCKET is set"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_pane_type_is_bad_usage_before_connecting() {
    let dir = folder("type");
    let output = kanna(&dir, &["open", "foo"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("foo"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsupported_from_the_mux_exits_5() {
    let dir = folder("unsupported");
    let _requests = serve(&dir.join("nothing.sock"), |_| serde_json::json!({ "ok": false, "error": { "code": "unsupported", "message": "p1 can't be read here" } }));
    let output = kanna(&dir, &["read", "p1"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(5), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn after_a_double_dash_flags_are_text() {
    let dir = folder("dashes");
    let requests = serve(&dir.join("nothing.sock"), |_| serde_json::json!({ "ok": true }));
    let output = kanna(&dir, &["send", "p1", "--", "--json"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(request["cmd"], "send");
    assert_eq!(request["args"], serde_json::json!({ "pane": "p1", "text": "--json" }));
    let output = kanna(&dir, &["send", "p1", "--", "--mux"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(requests.recv_timeout(Duration::from_secs(5)).unwrap()["args"]["text"], "--mux");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_kanna_socket_set_a_command_that_needs_the_app_fails_with_6_and_starts_nothing() {
    let dir = folder("no-app");
    let requests = serve(&dir.join("nothing.sock"), |request| match request["cmd"].as_str() {
        Some("capabilities") => serde_json::json!({ "ok": true, "paneTypes": ["term"] }),
        _ => serde_json::json!({ "ok": false, "error": { "code": "no_app", "message": "opening a pane with --split, --size, --tab, --window or --dock needs Kanna.app, and none is attached to this daemon" } }),
    });
    let started = Instant::now();
    let output = kanna(&dir, &["open", "--window", "win-1", "--split", "right"]).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(6), "{stderr}");
    assert!(stderr.contains("[no_app]") && stderr.contains("KANNA_SOCKET is set, so kanna doesn't start Kanna.app"), "{stderr}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let sent: Vec<String> = requests.try_iter().filter_map(|r| r["cmd"].as_str().map(String::from)).collect();
    assert_eq!(sent, ["open"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn with_nothing_running_kanna_starts_the_app_and_says_so_when_it_cant() {
    let dir = folder("dev");
    for args in [&["capabilities"][..], &["--mux", "kanna-dev", "capabilities"], &["--instance", "work", "capabilities"]] {
        let output = kanna(&dir, args).env_remove("KANNA_SOCKET").env("HOME", &dir).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(3), "{args:?}: {stderr}");
        let build = kanna_build::Build::current();
        let folder = if args[0] == "--instance" { build.channel.instance_folder("work") } else { build.channel.instance_folder(&build.default_instance()) };
        assert!(stderr.contains(&format!("Kanna.app ({}) couldn't be started", build.app_name())) && stderr.contains(&format!("{folder}/kanna.sock")), "{args:?}: {stderr}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn output_into_a_closed_pipe_ends_quietly_as_a_unix_tool_does() {
    use std::os::unix::process::ExitStatusExt;
    let dir = folder("pipe");
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let output = kanna(&dir, &["help"]).stdout(writer).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.signal(), Some(libc::SIGPIPE), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
