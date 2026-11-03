use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kanna_mux::json::{json, Value};
use kanna_mux::{CoreHandle, KmuxError};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const COMMAND: &str = "build";

const CAPABILITY_LIFE: Duration = Duration::from_secs(12 * 3600);

const RESTART_AFTER: Duration = Duration::from_secs(5);

const DEFAULT_ALLOW: &[&str] = &["github.com:443", "codeload.github.com:443", "objects.githubusercontent.com:443", "crates.io:443", "index.crates.io:443", "static.crates.io:443", "static.rust-lang.org:443", "ziglang.org:443", "pkg.machengine.org:443", "swiftpackageindex.com:443", "deps.files.ghostty.org:443", "codeberg.org:443"];

#[derive(Clone)]
struct Capability {
    guest: String,
    job: String,
    issued: Instant,
}

#[derive(Default)]
struct Running {
    children: HashMap<String, Child>,
    adopted: HashMap<String, i32>,
    stopped: std::collections::HashSet<String>,
}

fn alive(pid: i32) -> bool {
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

fn runs_kanna_vm(pid: i32) -> bool {
    Command::new("/bin/ps").args(["-o", "command=", "-p", &pid.to_string()]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("kanna-vm run")).unwrap_or(false)
}

pub static GUESTS: std::sync::OnceLock<Arc<Guests>> = std::sync::OnceLock::new();

pub struct Guests {
    state_dir: PathBuf,
    capabilities: Mutex<HashMap<String, Capability>>,
    running: Mutex<Running>,
    vm: PathBuf,
}

fn capability() -> String {
    let mut random = [0u8; 24];
    let _ = getrandom::getrandom(&mut random);
    random.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !name.starts_with('-')
}

fn global(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_broadcast() || v4.is_documentation() || v4.is_unspecified() || v4.is_multicast() || o[0] == 0 || (o[0] == 100 && (64..128).contains(&o[1])) || (o[0] == 192 && o[1] == 0 && o[2] == 0) || (o[0] == 198 && (18..20).contains(&o[1])) || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            if let Some(v4) = v6.to_ipv4_mapped() {
                return global(&IpAddr::V4(v4));
            }
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 || s[0] == 0x2001 && s[1] == 0x0db8 || s[0] == 0x64 && s[1] == 0xff9b)
        }
    }
}

impl Guests {
    pub fn new(state_dir: &Path) -> Arc<Guests> {
        let vm = std::env::var_os("KANNA_VM").map(PathBuf::from).or_else(|| std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.join("kanna-vm")))).unwrap_or_default();
        Arc::new(Guests { state_dir: state_dir.to_path_buf(), capabilities: Mutex::new(HashMap::new()), running: Mutex::new(Running::default()), vm })
    }

    fn dir(&self) -> PathBuf {
        crate::builds::guests_dir(&self.state_dir)
    }

    pub fn issue(&self, guest: &str, job: &str) -> String {
        let cap = capability();
        let mut caps = self.capabilities.lock().unwrap_or_else(|p| p.into_inner());
        caps.retain(|_, c| c.issued.elapsed() < CAPABILITY_LIFE);
        caps.insert(cap.clone(), Capability { guest: guest.into(), job: job.into(), issued: Instant::now() });
        cap
    }

    fn allowed(&self) -> Vec<String> {
        let file = self.dir().join("egress.json");
        std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v["allow"].as_array().map(|a| a.iter().filter_map(|h| h.as_str().map(String::from)).collect())).unwrap_or_else(|| DEFAULT_ALLOW.iter().map(|s| s.to_string()).collect())
    }

    fn log(&self, guest: &str, entry: Value) {
        let mut entry = entry;
        entry["at"] = json!(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(self.dir().join(guest).join("egress.log")) {
            let _ = file.write_all(format!("{entry}\n").as_bytes());
        }
    }

    pub fn start(self: &Arc<Self>) {
        let guests = self.clone();
        let _ = std::thread::Builder::new().name("kanna.guests".into()).spawn(move || loop {
            guests.keep_running();
            std::thread::sleep(RESTART_AFTER);
        });
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir()).into_iter().flatten().flatten().filter(|e| e.path().join("vm/config.json").exists()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| valid_name(n)).collect();
        names.sort();
        names
    }

    fn keep_running(self: &Arc<Self>) {
        for name in self.names() {
            let mut running = self.running.lock().unwrap_or_else(|p| p.into_inner());
            if running.stopped.contains(&name) {
                continue;
            }
            let owned = running.children.get_mut(&name).is_some_and(|c| matches!(c.try_wait(), Ok(None)));
            if owned || running.adopted.get(&name).is_some_and(|pid| alive(*pid)) {
                continue;
            }
            running.children.remove(&name);
            running.adopted.remove(&name);
            let recorded = std::fs::read_to_string(self.dir().join(&name).join("vm.pid")).ok().and_then(|p| p.trim().parse::<i32>().ok());
            if let Some(pid) = recorded.filter(|pid| alive(*pid) && runs_kanna_vm(*pid)) {
                log!("kanna-daemon: guest {name} is already running (pid {pid}); adopting it");
                running.adopted.insert(name.clone(), pid);
                drop(running);
                self.serve_egress(&name);
                continue;
            }
            drop(running);
            if let Some(child) = self.spawn(&name) {
                self.running.lock().unwrap_or_else(|p| p.into_inner()).children.insert(name, child);
            }
        }
    }

    fn spawn(self: &Arc<Self>, name: &str) -> Option<Child> {
        let dir = self.dir().join(name);
        let settings: Value = std::fs::read(dir.join("guest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        let cpus = settings["cpus"].as_u64().unwrap_or(6).to_string();
        let memory = settings["memory_gib"].as_u64().unwrap_or(8).to_string();
        self.serve_egress(name);
        let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("vm.log")).ok()?;
        let child = Command::new(&self.vm).arg("run").arg(dir.join("vm")).arg(&dir).arg(cpus).arg(memory).stdin(Stdio::null()).stdout(log.try_clone().ok()?).stderr(log).spawn();
        match child {
            Ok(child) => {
                log!("kanna-daemon: guest {name} started (pid {})", child.id());
                let _ = std::fs::write(dir.join("vm.pid"), child.id().to_string());
                Some(child)
            }
            Err(e) => {
                log!("kanna-daemon: can't start guest {name} with {}: {e}", self.vm.display());
                None
            }
        }
    }

    fn serve_egress(self: &Arc<Self>, guest: &str) {
        let socket = self.dir().join(guest).join("egress.sock");
        let _ = std::fs::remove_file(&socket);
        let Ok(listener) = UnixListener::bind(&socket) else { return };
        let (guests, guest) = (self.clone(), guest.to_string());
        let _ = std::thread::Builder::new().name(format!("kanna.egress.{guest}")).spawn(move || {
            for conn in listener.incoming().flatten() {
                let (guests, guest) = (guests.clone(), guest.clone());
                let _ = std::thread::Builder::new().name("kanna.egress.conn".into()).spawn(move || guests.proxy(&guest, conn));
            }
        });
    }

    fn proxy(&self, guest: &str, conn: UnixStream) {
        let Ok(reader_side) = conn.try_clone() else { return };
        let mut reader = BufReader::new(reader_side);
        let mut writer = conn;
        let mut request = String::new();
        let mut headers = vec![];
        if reader.by_ref().take(8192).read_line(&mut request).is_err() {
            return;
        }
        loop {
            let mut line = String::new();
            if reader.by_ref().take(8192).read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" || headers.len() > 64 {
                break;
            }
            headers.push(line);
        }
        let refuse = |writer: &mut UnixStream, code: &str, why: &str| {
            let challenge = if code.starts_with("407") { "Proxy-Authenticate: Basic realm=\"kanna job\"\r\n" } else { "" };
            let _ = writer.write_all(format!("HTTP/1.1 {code}\r\n{challenge}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
            why.to_string()
        };
        let mut parts = request.split_whitespace();
        let (method, target) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default().to_string());
        if method != "CONNECT" {
            let why = refuse(&mut writer, "405 Method Not Allowed", "only CONNECT");
            return self.log(guest, json!({ "target": target, "allowed": false, "why": why }));
        }
        let token = headers.iter().find_map(|h| {
            let (name, value) = h.split_once(':')?;
            if !name.eq_ignore_ascii_case("proxy-authorization") {
                return None;
            }
            let encoded = value.trim().strip_prefix("Basic ")?;
            let decoded = String::from_utf8(BASE64.decode(encoded.trim()).ok()?).ok()?;
            decoded.split_once(':').map(|(_, pass)| pass.to_string())
        });
        let job = token.and_then(|t| {
            let caps = self.capabilities.lock().unwrap_or_else(|p| p.into_inner());
            caps.get(&t).filter(|c| c.guest == guest && c.issued.elapsed() < CAPABILITY_LIFE).map(|c| c.job.clone())
        });
        let Some(job) = job else {
            let why = refuse(&mut writer, "407 Proxy Authentication Required", "no capability of a job on this guest");
            return self.log(guest, json!({ "target": target, "allowed": false, "why": why }));
        };
        if !self.allowed().iter().any(|a| a.eq_ignore_ascii_case(&target)) {
            let why = refuse(&mut writer, "403 Forbidden", "not on the allowlist");
            return self.log(guest, json!({ "job": job, "target": target, "allowed": false, "why": why }));
        }
        let addrs: Vec<SocketAddr> = target.to_socket_addrs().map(|a| a.collect()).unwrap_or_default();
        if addrs.is_empty() || !addrs.iter().all(|a| global(&a.ip())) {
            let why = refuse(&mut writer, "403 Forbidden", "resolves to an address that isn't global");
            return self.log(guest, json!({ "job": job, "target": target, "allowed": false, "why": why, "addrs": addrs.iter().map(|a| a.to_string()).collect::<Vec<_>>() }));
        }
        let Some(upstream) = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_secs(10)).ok()) else {
            let why = refuse(&mut writer, "502 Bad Gateway", "couldn't connect");
            return self.log(guest, json!({ "job": job, "target": target, "allowed": false, "why": why }));
        };
        self.log(guest, json!({ "job": job, "target": target, "allowed": true }));
        if writer.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").is_err() {
            return;
        }
        let buffered = reader.buffer().to_vec();
        let client = reader.into_inner();
        let (Ok(mut up_write), Ok(mut up_read)) = (upstream.try_clone(), upstream.try_clone()) else { return };
        let mut client_read = client;
        let forward = std::thread::spawn(move || {
            let _ = up_write.write_all(&buffered);
            let _ = std::io::copy(&mut client_read, &mut up_write);
            let _ = up_write.shutdown(std::net::Shutdown::Write);
        });
        let _ = std::io::copy(&mut up_read, &mut writer);
        let _ = writer.shutdown(std::net::Shutdown::Write);
        let _ = forward.join();
    }

    pub fn install(self: &Arc<Self>, core: &CoreHandle) {
        let guests = self.clone();
        core.add_command(COMMAND, std::rc::Rc::new(move |args, responder| responder.reply(guests.command(&args).map(|v| v.as_object().cloned().unwrap_or_default()))));
    }

    fn command(&self, args: &Value) -> Result<Value, KmuxError> {
        let action = args["action"].as_str().unwrap_or("list");
        let name = args["name"].as_str().unwrap_or_default().to_string();
        let needs_name = || if valid_name(&name) { Ok(()) } else { Err(KmuxError::new("bad_request", "a guest's name is lowercase letters, digits and dashes")) };
        match action {
            "list" => Ok(json!({ "guests": self.list() })),
            "create" => {
                needs_name()?;
                let master = args["from"].as_str().ok_or_else(|| KmuxError::new("bad_request", "build guest create NAME --from MASTER_VM_DIR"))?;
                self.create(&name, Path::new(master), args["cpus"].as_u64(), args["memory"].as_u64())?;
                if let Err(e) = data_disk(&self.dir().join(&name).join("vm/data.img"), args["data"].as_u64().unwrap_or(DATA_GIB)) {
                    let _ = std::fs::remove_dir_all(self.dir().join(&name));
                    return Err(e);
                }
                Ok(json!({ "guest": name }))
            }
            "data" => {
                needs_name()?;
                let vm = self.dir().join(&name).join("vm");
                if !vm.is_dir() {
                    return Err(KmuxError::new("not_found", format!("no guest {name}")));
                }
                data_disk(&vm.join("data.img"), args["data"].as_u64().unwrap_or(DATA_GIB))?;
                Ok(json!({ "guest": name, "done": "data disk added; it's attached when the guest next starts" }))
            }
            "stop" | "retire" => {
                needs_name()?;
                self.stop(&name);
                if action == "retire" {
                    let dir = self.dir().join(&name);
                    std::fs::remove_dir_all(&dir).map_err(|e| KmuxError::new("failed", format!("can't remove {}: {e}", dir.display())))?;
                }
                Ok(json!({ "guest": name, "done": action }))
            }
            "start" => {
                needs_name()?;
                self.running.lock().unwrap_or_else(|p| p.into_inner()).stopped.remove(&name);
                Ok(json!({ "guest": name }))
            }
            other => Err(KmuxError::new("bad_request", format!("no build action {other:?}"))),
        }
    }

    fn list(&self) -> Vec<Value> {
        let running = self.running.lock().unwrap_or_else(|p| p.into_inner());
        self.names().into_iter().map(|name| {
            let pid = running.children.get(&name).map(|c| c.id() as i32).or_else(|| running.adopted.get(&name).copied());
            let ready = self.dir().join(&name).join("agent.sock").exists();
            json!({ "name": name, "pid": pid, "stopped": running.stopped.contains(&name), "socket": ready })
        }).collect()
    }

    fn stop(&self, name: &str) {
        let mut running = self.running.lock().unwrap_or_else(|p| p.into_inner());
        running.stopped.insert(name.to_string());
        if let Some(pid) = running.adopted.remove(name) {
            unsafe { libc::kill(pid, libc::SIGINT) };
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline && alive(pid) {
                std::thread::sleep(Duration::from_millis(200));
            }
            if alive(pid) {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        if let Some(mut child) = running.children.remove(name) {
            unsafe { libc::kill(child.id() as i32, libc::SIGINT) };
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline && matches!(child.try_wait(), Ok(None)) {
                std::thread::sleep(Duration::from_millis(200));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(self.dir().join(name).join("agent.sock"));
    }

    fn create(&self, name: &str, master: &Path, cpus: Option<u64>, memory: Option<u64>) -> Result<(), KmuxError> {
        let failed = |what: &str, e: std::io::Error| KmuxError::new("failed", format!("{what}: {e}"));
        let dir = self.dir().join(name);
        if dir.exists() {
            return Err(KmuxError::new("exists", format!("guest {name} already exists; retire it first")));
        }
        for file in ["config.json", "disk.img", "nvram.bin"] {
            if !master.join(file).exists() {
                return Err(KmuxError::new("bad_request", format!("{} has no {file}; a master VM folder holds config.json, disk.img and nvram.bin", master.display())));
            }
        }
        std::fs::create_dir_all(dir.join("vm")).map_err(|e| failed("can't make the guest's folder", e))?;
        for file in ["config.json", "disk.img", "nvram.bin"] {
            let status = Command::new("/bin/cp").arg("-c").arg(master.join(file)).arg(dir.join("vm").join(file)).status().map_err(|e| failed("can't clone the master", e))?;
            if !status.success() {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(KmuxError::new("failed", format!("cp -c {} failed", master.join(file).display())));
            }
        }
        let status = Command::new(&self.vm).arg("new-identity").arg(dir.join("vm")).status().map_err(|e| failed("can't give the guest its own identity", e))?;
        if !status.success() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(KmuxError::new("failed", "kanna-vm new-identity failed"));
        }
        let settings = json!({ "cpus": cpus.unwrap_or(6), "memory_gib": memory.unwrap_or(8), "master": master });
        std::fs::write(dir.join("guest.json"), serde_json::to_vec_pretty(&settings).unwrap_or_default()).map_err(|e| failed("can't save the guest's settings", e))?;
        Ok(())
    }
}

const DATA_GIB: u64 = 64;

pub const DATA_VOLUME: &str = "KannaData";

fn data_disk(path: &Path, gib: u64) -> Result<(), KmuxError> {
    let failed = |what: String| KmuxError::new("failed", what);
    if path.exists() {
        return Err(KmuxError::new("exists", format!("{} already exists", path.display())));
    }
    if !(8..=4096).contains(&gib) {
        return Err(KmuxError::new("bad_request", "a data disk is 8 to 4096 GiB"));
    }
    let file = std::fs::File::create_new(path).map_err(|e| failed(format!("can't make {}: {e}", path.display())))?;
    let made = (|| {
        file.set_len(gib << 30).map_err(|e| failed(format!("can't size {}: {e}", path.display())))?;
        let attached = Command::new("/usr/bin/hdiutil").args(["attach", "-nomount", "-imagekey", "diskimage-class=CRawDiskImage"]).arg(path).output().map_err(|e| failed(format!("hdiutil: {e}")))?;
        let device = String::from_utf8_lossy(&attached.stdout).split_whitespace().next().filter(|d| d.starts_with("/dev/disk")).map(String::from);
        let Some(device) = device.filter(|_| attached.status.success()) else {
            return Err(failed(format!("hdiutil couldn't attach {}: {}", path.display(), String::from_utf8_lossy(&attached.stderr).trim())));
        };
        let erased = Command::new("/usr/sbin/diskutil").args(["eraseDisk", "APFS", DATA_VOLUME, "GPT", &device]).output();
        let opened = match erased {
            Ok(out) if out.status.success() => open_to_the_guest(&device),
            Ok(out) => Err(failed(format!("diskutil couldn't format {device}: {}", String::from_utf8_lossy(&out.stderr).trim()))),
            Err(e) => Err(failed(format!("diskutil: {e}"))),
        };
        let _ = Command::new("/usr/sbin/diskutil").args(["eject", &device]).output();
        opened
    })();
    if made.is_err() {
        let _ = std::fs::remove_file(path);
    }
    made
}

fn open_to_the_guest(device: &str) -> Result<(), KmuxError> {
    let store = format!("{}s2", device.trim_start_matches("/dev/"));
    let mount = std::fs::read_dir("/Volumes").into_iter().flatten().flatten().map(|e| e.path()).find(|path| {
        Command::new("/usr/sbin/diskutil").arg("info").arg(path).output().is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout).lines().any(|line| line.trim_start().starts_with("APFS Physical Store:") && line.trim_end().ends_with(&store))
        })
    });
    let Some(mount) = mount else { return Err(KmuxError::new("failed", format!("the new data volume on {device} didn't mount"))) };
    std::fs::set_permissions(&mount, std::os::unix::fs::PermissionsExt::from_mode(0o1777)).map_err(|e| KmuxError::new("failed", format!("can't open {} to the guest's user: {e}", mount.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn a_data_disk_is_an_apfs_volume_named_for_the_guest_to_find() {
        let dir = std::env::temp_dir().join(format!("kanna-data-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("data.img");
        data_disk(&image, 8).unwrap();
        assert!(data_disk(&image, 8).is_err(), "an existing data disk isn't replaced");
        let attached = Command::new("/usr/bin/hdiutil").args(["attach", "-readonly", "-nomount", "-imagekey", "diskimage-class=CRawDiskImage"]).arg(&image).output().unwrap();
        let text = String::from_utf8_lossy(&attached.stdout).to_string();
        let device = text.split_whitespace().next().unwrap().to_string();
        let store = format!("Physical Store {}", device.trim_start_matches("/dev/"));
        let listed = Command::new("/usr/sbin/diskutil").arg("list").output().unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout).split("\n\n").filter(|block| block.contains(&store)).collect::<Vec<_>>().join("\n");
        let _ = Command::new("/usr/sbin/diskutil").args(["eject", &device]).output();
        assert!(listed.contains(DATA_VOLUME), "{listed}");
        let _ = Command::new("/usr/bin/hdiutil").args(["attach", "-readonly", "-imagekey", "diskimage-class=CRawDiskImage", "-mountroot"]).arg(&dir).arg(&image).output().unwrap();
        let mode = std::fs::metadata(dir.join(DATA_VOLUME)).map(|m| std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o7777);
        let _ = Command::new("/usr/sbin/diskutil").args(["unmountDisk", "force", &device]).output();
        let _ = Command::new("/usr/sbin/diskutil").args(["eject", &device]).output();
        assert_eq!(mode.ok(), Some(0o1777), "the guest's user, whatever its uid, can make its state folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_global_addresses_are_reachable() {
        for ip in ["10.0.0.1", "172.16.0.102", "192.168.1.1", "127.0.0.1", "169.254.1.1", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:10.0.0.1", "64:ff9b::a00:1"] {
            assert!(!global(&ip.parse().unwrap()), "{ip}");
        }
        for ip in ["140.82.112.3", "2606:4700::6810:84e5"] {
            assert!(global(&ip.parse().unwrap()), "{ip}");
        }
    }
}
