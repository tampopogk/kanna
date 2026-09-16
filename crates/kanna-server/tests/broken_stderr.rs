//! Process-level regression for a desktop exit leaving `kanna-server` alive
//! with the read end of its stderr pipe gone.

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn toml_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn reserve_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve loopback port");
    let port = listener.local_addr().expect("reserved port address").port();
    (listener, port)
}

#[test]
fn server_survives_when_its_desktop_stderr_reader_is_gone() {
    let root = tempfile::tempdir().expect("server fixture");
    let daemon_dir = root.path().join("daemon");
    std::fs::create_dir(&daemon_dir).unwrap();
    let db_path = root.path().join("kanna.db");
    let pairing_store = root.path().join("pairings.json");
    let config_path = root.path().join("server.toml");
    let (lan, lan_port) = reserve_port();
    let (transfer, transfer_port) = reserve_port();
    let (routing, routing_port) = reserve_port();
    std::fs::write(
        &config_path,
        format!(
            "relay_url = \"\"\n\
             device_token = \"\"\n\
             firebase_project_id = \"kanna-local\"\n\
             daemon_dir = \"{}\"\n\
             db_path = \"{}\"\n\
             desktop_id = \"desktop-broken-stderr\"\n\
             desktop_name = \"Broken stderr regression\"\n\
             version = \"test\"\n\
             environment = \"development\"\n\
             lan_host = \"127.0.0.1\"\n\
             lan_port = {lan_port}\n\
             transfer_port = {transfer_port}\n\
             lan_routing_port = {routing_port}\n\
             pairing_store_path = \"{}\"\n",
            toml_path(&daemon_dir),
            toml_path(&db_path),
            toml_path(&pairing_store),
        ),
    )
    .unwrap();

    // Model the desktop's lifecycle precisely: the server inherits the write
    // end while the desktop owns the reader. Only after the server has logged
    // successfully do we drop that reader, as happens when the desktop exits
    // but leaves the adopted server running.
    let (reader, writer) = std::io::pipe().expect("stderr pipe");
    drop(lan);
    drop(transfer);
    drop(routing);
    let child = Command::new(env!("CARGO_BIN_EXE_kanna-server"))
        .env("KANNA_SERVER_CONFIG", &config_path)
        .env("RUST_LOG", "kanna_server=info,kanna_daemon=warn")
        .stdout(Stdio::null())
        .stderr(Stdio::from(writer))
        .spawn()
        .expect("launch real kanna-server");
    let mut server = ChildGuard(child);

    let log_path = daemon_dir.join("kanna-server.log");
    let startup_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = server.0.try_wait().expect("poll kanna-server startup") {
            panic!("kanna-server exited during startup: {status}");
        }
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if log.contains("logging to ") {
            break;
        }
        assert!(
            Instant::now() < startup_deadline,
            "kanna-server did not initialize durable logging"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    // Let the successful record finish on both sinks before simulating the
    // desktop reader's disappearance.
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        server.0.try_wait().unwrap().is_none(),
        "kanna-server exited before its stderr reader disappeared"
    );
    drop(reader);
    std::thread::sleep(Duration::from_millis(100));
    if let Some(status) = server.0.try_wait().expect("poll after desktop exit") {
        panic!("kanna-server exited when its stderr reader disappeared: {status}");
    }

    let warning = "terminal state watcher reconnecting after error";
    let baseline_warnings = std::fs::read_to_string(&log_path)
        .unwrap_or_default()
        .matches(warning)
        .count();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(status) = server.0.try_wait().expect("poll kanna-server") {
            panic!("kanna-server exited when its stderr reader disappeared: {status}");
        }
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if log.matches(warning).count() > baseline_warnings {
            // The watcher warning is the post-startup write corresponding to
            // the published daemon-shutdown trigger. Seeing it in the durable
            // file proves logging continued after stderr broke.
            assert!(
                server.0.try_wait().unwrap().is_none(),
                "kanna-server exited after logging with broken stderr"
            );
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let log = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|error| format!("<log unavailable: {error}>"));
    panic!("kanna-server did not record the watcher warning with broken stderr; log={log}");
}
