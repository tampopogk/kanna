mod common;

use common::{wait_for, App, Instance};
use serde_json::{json, Value};
use std::ffi::CStr;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

struct Device {
    master: std::fs::File,
    path: String,
}

impl Device {
    fn new() -> Device {
        unsafe {
            let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
            assert!(fd >= 0);
            assert_eq!(libc::grantpt(fd), 0);
            assert_eq!(libc::unlockpt(fd), 0);
            let path = CStr::from_ptr(libc::ptsname(fd)).to_string_lossy().into_owned();
            libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            Device { master: std::fs::File::from(OwnedFd::from_raw_fd(fd)), path }
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
    }

    fn read_until(&mut self, wanted: &[u8]) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while !got.windows(wanted.len()).any(|w| w == wanted) {
            assert!(Instant::now() < deadline, "the device never got {:?}; it got {:?}", String::from_utf8_lossy(wanted), String::from_utf8_lossy(&got));
            let mut buffer = [0u8; 256];
            match self.master.read(&mut buffer) {
                Ok(n) if n > 0 => got.extend_from_slice(&buffer[..n]),
                _ => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        got
    }

    fn name(&self) -> String {
        self.path.rsplit('/').next().unwrap().to_string()
    }
}

fn ports_file(folder: &Path, path: &str) {
    let ports = json!([{ "path": path, "vid": "303a", "pid": "1001", "serial": "SN1", "product": "USB JTAG/serial debug unit", "vendor": "Espressif" }]);
    let temp = folder.join("ports.json.new");
    std::fs::write(&temp, ports.to_string()).unwrap();
    std::fs::rename(temp, folder.join("ports.json")).unwrap();
}

fn serial(instance: &Instance, args: Value) -> Value {
    instance.call("serial", args)
}

fn texts(reply: &Value) -> Vec<String> {
    reply["rows"].as_array().unwrap().iter().map(|r| format!("{} {}", r["d"].as_str().unwrap(), r["text"].as_str().unwrap())).collect()
}

fn rows_until(instance: &Instance, pane: &str, wanted: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let reply = serial(instance, json!({ "action": "read", "pane": pane }));
        if texts(&reply).iter().any(|t| t.contains(wanted)) {
            return reply;
        }
        assert!(Instant::now() < deadline, "no row with {wanted:?}: {:?}", texts(&reply));
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn state(instance: &Instance, pane: &str) -> String {
    serial(instance, json!({ "action": "state", "pane": pane }))["port"]["state"].as_str().unwrap_or("?").to_string()
}

fn scratch() -> PathBuf {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let folder = std::env::temp_dir().join(format!("kserial-{}-{}", std::process::id(), COUNT.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder.canonicalize().unwrap()
}

#[test]
fn a_serial_pane_reads_writes_logs_lends_and_follows_its_device_through_a_replug() {
    let folder = scratch();
    let mut device = Device::new();
    ports_file(&folder, &device.path);
    let ports = folder.join("ports.json");
    let instance = Instance::start_with(&[("KANNA_SERIAL_PORTS", ports.to_str().unwrap())]);

    let listed = serial(&instance, json!({ "action": "list" }));
    assert_eq!(listed["ports"][0]["usb"], "303a:1001", "{listed}");
    let missing = instance.call("open", json!({ "type": "serial", "path": "nothing-here" }));
    assert_eq!(missing["ok"], false, "{missing}");
    assert!(missing["error"]["message"].as_str().unwrap().contains("kanna serial list"), "{missing}");

    let opened = instance.call("open", json!({ "type": "serial", "name": "esp", "path": "303a:1001" }));
    assert_eq!(opened["ok"], true, "{opened}");
    let id = opened["pane"]["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("serial-"), "{opened}");
    let pane = instance.pane("esp");
    assert_eq!(pane["path"], json!(device.path), "{pane}");
    assert_eq!(pane["device"], "303a:1001:SN1", "{pane}");
    assert_eq!(pane["baud"], 115200, "{pane}");
    assert_eq!(state(&instance, "esp"), "connected");

    device.write(b"\x1b[0;32mI (24) boot: hello\x1b[0m\r\nE (5341) wifi: connect failed\r\n\x00\xffbin\r\nuart:~$ ");
    let reply = rows_until(&instance, "esp", "wifi");
    let rows = reply["rows"].as_array().unwrap();
    let boot = rows.iter().find(|r| r["text"].as_str().unwrap().contains("boot")).unwrap();
    assert_eq!(boot["l"], "I", "{boot}");
    assert_eq!(boot["d"], "<");
    assert!(rows.iter().any(|r| r["l"] == "E"), "{reply}");
    assert!(texts(&reply).contains(&"< \\x00\\xffbin".to_string()), "{:?}", texts(&reply));
    assert_eq!(reply["partial"]["text"], "uart:~$ ", "{reply}");
    assert_eq!(reply["port"]["format"], "ESP-IDF", "{reply}");

    let next = reply["next"].as_u64().unwrap();
    let mut writer = Device { master: device.master.try_clone().unwrap(), path: device.path.clone() };
    let late = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        writer.write(b"late\n");
    });
    let started = Instant::now();
    let waited = serial(&instance, json!({ "action": "read", "pane": "esp", "after": next, "wait": 5000 }));
    late.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(4), "a read waits only until the next row");
    assert_eq!(texts(&waited), ["< uart:~$ late"], "{waited}");
    let quiet = serial(&instance, json!({ "action": "read", "pane": "esp", "after": waited["next"], "wait": 200 }));
    assert!(quiet["rows"].as_array().unwrap().is_empty(), "{quiet}");

    let sent = serial(&instance, json!({ "action": "send", "pane": "esp", "text": "help" }));
    assert!(sent["at"].as_i64().is_some(), "{sent}");
    device.read_until(b"help\n");
    serial(&instance, json!({ "action": "set", "pane": "esp", "lineEnding": "crlf" }));
    serial(&instance, json!({ "action": "send", "pane": "esp", "text": "x" }));
    device.read_until(b"x\r\n");
    serial(&instance, json!({ "action": "send", "pane": "esp", "hex": "1b 5b 41" }));
    device.read_until(b"\x1b[A");
    assert_eq!(instance.pane("esp")["lineEnding"], "crlf");
    let since = serial(&instance, json!({ "action": "read", "pane": "esp", "since": sent["at"] }));
    assert_eq!(texts(&since)[0], "> help", "{since}");
    let last = serial(&instance, json!({ "action": "read", "pane": "esp", "last": 1 }));
    assert_eq!(texts(&last), ["> \u{1b}[A"], "{last}");
    let wrong = serial(&instance, json!({ "action": "send", "pane": "nope", "text": "x" }));
    assert_eq!(wrong["error"]["code"], "not_found", "{wrong}");

    let log_path = serial(&instance, json!({ "action": "state", "pane": "esp" }))["port"]["log"].as_str().unwrap().to_string();
    assert_eq!(log_path, instance.dir.join("state/serial").join(format!("{}.log", device.name())).to_string_lossy());
    let log = std::fs::read_to_string(&log_path).unwrap();
    assert!(log.contains(" < E (5341) wifi: connect failed\n"), "{log}");
    assert!(log.contains(" > help\n"), "{log}");
    assert!(log.contains(" # connected "), "{log}");

    let lent = serial(&instance, json!({ "action": "lend", "pane": "esp", "cmd": "idf.py flash" }));
    assert_eq!(lent["lent"], true, "{lent}");
    assert_eq!(state(&instance, "esp"), "lent");
    let refused = serial(&instance, json!({ "action": "send", "pane": "esp", "text": "x" }));
    assert_eq!(refused["error"]["code"], "lent", "{refused}");
    let flasher = std::fs::OpenOptions::new().read(true).write(true).open(&device.path);
    assert!(flasher.is_ok(), "the lent port is free for the flasher: {flasher:?}");
    drop(flasher);
    serial(&instance, json!({ "action": "return", "lease": lent["lease"] }));
    wait_for("the port back after the lease", || state(&instance, "esp") == "connected");
    rows_until(&instance, "esp", "# back");

    let mut sleeper = std::process::Command::new("sleep").arg("0.3").spawn().unwrap();
    let lent = serial(&instance, json!({ "action": "lend", "pane": "esp", "cmd": "sleep", "pid": sleeper.id() }));
    assert_eq!(lent["lent"], true, "{lent}");
    sleeper.wait().unwrap();
    wait_for("the port back when the borrower exits", || state(&instance, "esp") == "connected");
    let unknown = serial(&instance, json!({ "action": "lend", "port": "/dev/cu.nothing", "cmd": "x" }));
    assert_eq!(unknown["lent"], false, "{unknown}");

    let mut app = App::connect(&instance.socket());
    app.send(json!({ "id": 7, "cmd": "serial.subscribe", "args": { "view": id, "pane": id } }));
    let subscribed = app.until("the subscribe reply", |m| m["id"] == 7);
    assert_eq!(subscribed["ok"], true, "{subscribed}");
    assert!(subscribed["rows"].as_array().unwrap().len() > 5, "{subscribed}");
    assert_eq!(subscribed["state"]["state"], "connected", "{subscribed}");
    assert_eq!(subscribed["history"], json!(["help", "x"]), "{subscribed}");
    device.write(b"pushed\n");
    let pushed = app.until("rows pushed to the app", |m| m["cmd"] == "serial" && m["args"]["rows"].to_string().contains("pushed"));
    assert_eq!(pushed["args"]["rows"].as_array().unwrap().len(), 1, "{pushed}");

    drop(device);
    wait_for("the pane to see the device unplugged", || state(&instance, "esp") == "unplugged");
    let gone = app.until("the unplugged state", |m| m["cmd"] == "serial" && m["args"]["state"]["state"] == "unplugged");
    assert_eq!(gone["args"]["content"], subscribed["content"], "{gone}");
    let refused = serial(&instance, json!({ "action": "send", "pane": "esp", "text": "x" }));
    assert_eq!(refused["error"]["code"], "unplugged", "{refused}");

    let mut device = Device::new();
    ports_file(&folder, &device.path);
    wait_for("the device back under its new path", || state(&instance, "esp") == "connected");
    assert_eq!(instance.pane("esp")["path"], json!(device.path));
    device.write(b"again\n");
    let reply = rows_until(&instance, "esp", "again");
    assert!(texts(&reply).iter().any(|t| t.starts_with("# back ")), "{:?}", texts(&reply));

    assert_eq!(instance.call("close", json!({ "pane": "esp" }))["ok"], true);
    let listed = serial(&instance, json!({ "action": "list" }));
    assert_eq!(listed["ports"][0]["panes"], json!([]), "{listed}");
    let reopened = std::fs::OpenOptions::new().read(true).write(true).open(&device.path);
    assert!(reopened.is_ok(), "closing the last pane lets go of the port: {reopened:?}");
    let _ = device.master.as_raw_fd();
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn panes_on_one_port_share_it_and_a_daemon_restart_opens_it_again() {
    let folder = scratch();
    let mut device = Device::new();
    ports_file(&folder, &device.path);
    let ports = folder.join("ports.json");
    let mut instance = Instance::start_with(&[("KANNA_SERIAL_PORTS", ports.to_str().unwrap())]);
    let name = device.name();
    for pane in ["a", "b"] {
        let opened = instance.call("open", json!({ "type": "serial", "name": pane, "path": name, "baud": 921600 }));
        assert_eq!(opened["ok"], true, "{opened}");
    }
    let listed = serial(&instance, json!({ "action": "list" }));
    assert_eq!(listed["ports"][0]["panes"], json!(["serial-1", "serial-2"]), "{listed}");
    device.write(b"shared\n");
    rows_until(&instance, "a", "shared");
    rows_until(&instance, "b", "shared");
    instance.call("close", json!({ "pane": "a" }));
    assert_eq!(state(&instance, "b"), "connected");
    let wrong = instance.call("open", json!({ "type": "web", "url": "http://a.test/", "baud": 9600 }));
    assert_eq!(wrong["error"]["code"], "bad_request", "{wrong}");

    instance.restart(libc::SIGTERM);
    wait_for("the port open again after the restart", || state(&instance, "b") == "connected");
    assert_eq!(instance.pane("b")["baud"], 921600);
    device.write(b"after\n");
    rows_until(&instance, "b", "after");
    let _ = std::fs::remove_dir_all(folder);
}
