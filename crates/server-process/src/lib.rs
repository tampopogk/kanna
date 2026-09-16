//! Who is listening on `kanna-server`'s port, and how to stop them.
//!
//! Every launcher faces the same question before it starts a server: the port
//! may already be bound, by this machine's own previous server or by nothing
//! at all. Answering it needs the *listening* pid — not the pid of a process
//! that happens to be called `kanna-server`, and not the pid of a child that
//! was just spawned and may already be dead. Attributing a port to the wrong
//! process is how a launcher ends up authorizing a pid that holds nothing.
//!
//! This lives outside both launchers because both need it: the desktop app
//! (`MobileServerManager::start`) and the headless `kanna-worker` supervisor.
//! A second copy would drift, and the two would disagree about who owns a
//! port, which is exactly the class of bug it exists to prevent.

#[cfg(any(target_os = "linux", test))]
use std::collections::BTreeSet;
use std::os::fd::{FromRawFd, OwnedFd};
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(not(target_os = "linux"))]
use tokio::process::Command;

/// Kernel-derived identity of the process holding a server listener.
///
/// A pid alone is not an identity: it can be recycled between observing a
/// listener and acting on its exit. Launchers pin the process start time and
/// executable alongside the pid so an adopted server can be supervised
/// without ever mistaking an unrelated successor for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerProcessIdentity {
    pub pid: u32,
    pub start: (u64, u64),
    pub executable: std::path::PathBuf,
}

impl ServerProcessIdentity {
    /// Pin the live process currently named by `pid`.
    pub fn pin(pid: u32) -> Result<Self, String> {
        let pid =
            libc::pid_t::try_from(pid).map_err(|_| format!("invalid kanna-server pid: {pid}"))?;
        let start = process_start_time(pid)
            .ok_or_else(|| format!("could not read kanna-server pid {pid} start time"))?;
        let executable = process_executable_path(pid)
            .ok_or_else(|| format!("could not read kanna-server pid {pid} executable"))?;
        Ok(Self {
            pid: pid as u32,
            start,
            executable,
        })
    }

    /// True only while the exact pinned process is still alive.
    pub fn is_alive(&self) -> bool {
        process_start_time(self.pid as libc::pid_t) == Some(self.start)
    }

    /// Verify that the kernel-derived executable is the expected sidecar.
    pub fn require_executable(&self, expected: &std::path::Path) -> Result<(), String> {
        let expected = canonical_process_path(expected);
        let actual = canonical_process_path(&self.executable);
        if actual == expected {
            return Ok(());
        }
        Err(format!(
            "kanna-server listener pid {} executable mismatch: expected {}, found {}",
            self.pid,
            expected.display(),
            actual.display()
        ))
    }
}

fn canonical_process_path(path: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether a pid still names some process. This deliberately does not claim
/// identity: callers use it only to avoid replacing a listener whose exact
/// identity could not be pinned.
pub fn server_process_exists(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// An event source registered against a pinned process identity.
///
/// Registration happens before readiness is published. Waiting is then a
/// kernel event, not a liveness polling loop: pidfd on Linux and EVFILT_PROC
/// on macOS.
pub struct ServerProcessExitWatcher {
    fd: Option<OwnedFd>,
    identity: ServerProcessIdentity,
}

impl ServerProcessExitWatcher {
    pub fn register(identity: ServerProcessIdentity) -> Result<Self, String> {
        if !identity.is_alive() {
            return Ok(Self { fd: None, identity });
        }
        let fd = match register_process_exit(identity.pid as libc::pid_t) {
            Ok(fd) => fd,
            Err(_) if !identity.is_alive() => return Ok(Self { fd: None, identity }),
            Err(error) => return Err(error),
        };
        // Close the observe/register race. If it exited after the first check,
        // the registered fd is already readable (or the identity no longer
        // matches), and either path resolves immediately.
        let fd = identity.is_alive().then_some(fd);
        Ok(Self { fd, identity })
    }

    pub async fn wait(self) -> Result<ServerProcessIdentity, String> {
        let identity = self.identity;
        let Some(fd) = self.fd else {
            return Ok(identity);
        };
        tokio::task::spawn_blocking(move || wait_for_registered_exit(fd))
            .await
            .map_err(|error| format!("kanna-server exit observer failed: {error}"))??;
        Ok(identity)
    }
}

#[cfg(target_os = "linux")]
fn register_process_exit(pid: libc::pid_t) -> Result<OwnedFd, String> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as libc::c_int };
    if fd < 0 {
        return Err(format!(
            "failed to register kanna-server pid {pid} exit observer: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(target_os = "linux")]
fn wait_for_registered_exit(fd: OwnedFd) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    let mut event = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let result = unsafe { libc::poll(&mut event, 1, -1) };
        if result > 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("failed waiting for kanna-server exit: {error}"));
        }
    }
}

#[cfg(target_os = "macos")]
fn register_process_exit(pid: libc::pid_t) -> Result<OwnedFd, String> {
    let fd = unsafe { libc::kqueue() };
    if fd < 0 {
        return Err(format!(
            "failed to create kanna-server exit observer: {}",
            std::io::Error::last_os_error()
        ));
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ENABLE | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let result = unsafe { libc::kevent(fd, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) };
    if result < 0 {
        return Err(format!(
            "failed to register kanna-server pid {pid} exit observer: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(owned)
}

#[cfg(target_os = "macos")]
fn wait_for_registered_exit(fd: OwnedFd) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
    loop {
        let result = unsafe {
            libc::kevent(
                fd.as_raw_fd(),
                std::ptr::null(),
                0,
                &mut event,
                1,
                std::ptr::null(),
            )
        };
        if result > 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(format!("failed waiting for kanna-server exit: {error}"));
        }
    }
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: libc::pid_t) -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    // `after_name` starts at field 3 (state); starttime is field 22.
    let start_ticks = after_name.split_whitespace().nth(19)?.parse().ok()?;
    Some((start_ticks, 0))
}

#[cfg(target_os = "linux")]
fn process_executable_path(pid: libc::pid_t) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: libc::pid_t) -> Option<(u64, u64)> {
    const PROC_PIDTBSDINFO: libc::c_int = 3;
    #[repr(C)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: libc::uid_t,
        pbi_gid: libc::gid_t,
        pbi_ruid: libc::uid_t,
        pbi_rgid: libc::gid_t,
        pbi_svuid: libc::uid_t,
        pbi_svgid: libc::gid_t,
        rfu_1: u32,
        pbi_comm: [libc::c_char; 16],
        pbi_name: [libc::c_char; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }
    extern "C" {
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: libc::c_int,
        ) -> libc::c_int;
    }
    let mut info: ProcBsdInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcBsdInfo>() as libc::c_int;
    let read = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut ProcBsdInfo).cast(),
            size,
        )
    };
    (read == size).then_some((info.pbi_start_tvsec, info.pbi_start_tvusec))
}

#[cfg(target_os = "macos")]
fn process_executable_path(pid: libc::pid_t) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;
    extern "C" {
        fn proc_pidpath(
            pid: libc::c_int,
            buffer: *mut libc::c_void,
            buffersize: u32,
        ) -> libc::c_int;
    }
    let mut path = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let read = unsafe {
        proc_pidpath(
            pid,
            path.as_mut_ptr().cast(),
            PROC_PIDPATHINFO_MAXSIZE as u32,
        )
    };
    if read <= 0 {
        return None;
    }
    let length = path[..read as usize]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(read as usize);
    path.truncate(length);
    Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(path)))
}

/// Stop whatever is listening on `port`, escalating SIGTERM to SIGKILL.
///
/// Returns once the port has no listeners left, so a caller may bind it
/// immediately afterwards.
pub async fn stop_server_on_port(port: u16) -> Result<(), String> {
    let pids = server_pids_on_port(port).await?;
    if pids.is_empty() {
        return Ok(());
    }

    for pid in &pids {
        signal_process(*pid, libc::SIGTERM)?;
    }
    let _ = wait_for_server_port_to_close(port, 20).await;

    let remaining_pids = server_pids_on_port(port).await?;
    if remaining_pids.is_empty() {
        return Ok(());
    }

    for pid in remaining_pids {
        signal_process(pid, libc::SIGKILL)?;
    }
    wait_for_server_port_to_close(port, 20).await
}

/// The pid holding `port`'s listening socket, when exactly one process does.
///
/// More than one is not a tie to break: it means something other than this
/// launcher's server is on the port, and guessing which to trust is how a
/// launcher authorizes the wrong process.
pub async fn listening_server_pid(port: u16) -> Result<u32, String> {
    let pids = server_pids_on_port(port).await?;
    let [pid] = pids.as_slice() else {
        return Err(format!(
            "expected exactly one kanna-server listener on port {port}, found {}",
            pids.len()
        ));
    };
    u32::try_from(*pid).map_err(|_| format!("invalid kanna-server pid: {pid}"))
}

#[cfg(not(target_os = "linux"))]
pub async fn server_pids_on_port(port: u16) -> Result<Vec<i32>, String> {
    let output = Command::new("/usr/sbin/lsof")
        .args(["-nP", "-ti", &format!("TCP:{port}"), "-sTCP:LISTEN"])
        .output()
        .await
        .map_err(|e| format!("failed to inspect kanna-server port owner: {}", e))?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_lsof_pids(&stdout))
}

#[cfg(target_os = "linux")]
pub async fn server_pids_on_port(port: u16) -> Result<Vec<i32>, String> {
    tokio::task::spawn_blocking(move || linux_server_pids_on_port(Path::new("/proc"), port))
        .await
        .map_err(|error| format!("failed to inspect kanna-server port owner: {error}"))?
}

/// procfs has no `lsof`: find the listening socket's inode in the TCP tables,
/// then the process holding a descriptor for it.
#[cfg(target_os = "linux")]
pub fn linux_server_pids_on_port(proc_root: &Path, port: u16) -> Result<Vec<i32>, String> {
    let mut socket_inodes = BTreeSet::new();
    let mut found_socket_table = false;
    for table_name in ["tcp", "tcp6"] {
        let table_path = proc_root.join("net").join(table_name);
        match std::fs::read_to_string(&table_path) {
            Ok(table) => {
                found_socket_table = true;
                socket_inodes.extend(listening_socket_inodes(&table, port));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to inspect kanna-server port owner: could not read {}: {}",
                    table_path.display(),
                    error
                ));
            }
        }
    }
    if !found_socket_table {
        return Err(format!(
            "failed to inspect kanna-server port owner: no TCP socket table under {}",
            proc_root.display()
        ));
    }
    if socket_inodes.is_empty() {
        return Ok(Vec::new());
    }

    let process_entries = std::fs::read_dir(proc_root).map_err(|error| {
        format!(
            "failed to inspect kanna-server port owner: could not read {}: {}",
            proc_root.display(),
            error
        )
    })?;
    let mut pids = BTreeSet::new();
    for process_entry in process_entries.flatten() {
        let Some(pid) = process_entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(fd_entries) = std::fs::read_dir(process_entry.path().join("fd")) else {
            continue;
        };
        for fd_entry in fd_entries.flatten() {
            let Ok(target) = std::fs::read_link(fd_entry.path()) else {
                continue;
            };
            let Some(inode) = socket_inode_from_link(&target) else {
                continue;
            };
            if socket_inodes.contains(&inode) {
                pids.insert(pid);
                break;
            }
        }
    }
    Ok(pids.into_iter().collect())
}

#[cfg(any(target_os = "linux", test))]
fn listening_socket_inodes(table: &str, port: u16) -> BTreeSet<u64> {
    let port_hex = format!("{port:04X}");
    table
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let local_port = fields.get(1)?.rsplit_once(':')?.1;
            let state = *fields.get(3)?;
            let inode = fields.get(9)?.parse::<u64>().ok()?;
            (local_port.eq_ignore_ascii_case(&port_hex) && state == "0A").then_some(inode)
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn socket_inode_from_link(target: &std::path::Path) -> Option<u64> {
    target
        .to_str()?
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// `lsof -t` output is one pid per line. Linux does not use it (procfs answers
/// the same question directly), but the parser is still covered there so a
/// change to it cannot pass on one platform and break the other.
#[cfg(any(not(target_os = "linux"), test))]
fn parse_lsof_pids(output: &str) -> Vec<i32> {
    output
        .lines()
        .filter_map(|line| line.trim().parse::<i32>().ok())
        .collect()
}

fn signal_process(pid: i32, signal: i32) -> Result<(), String> {
    let rc = unsafe { libc::kill(pid, signal) };
    if rc == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(format!(
            "failed to signal stale kanna-server process {}: {}",
            pid, error
        ))
    }
}

async fn wait_for_server_port_to_close(port: u16, attempts: usize) -> Result<(), String> {
    for _ in 0..attempts {
        if server_pids_on_port(port).await?.is_empty() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(format!("stale kanna-server did not stop on port {}", port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::{Child, Command};

    fn process_is_running(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[test]
    fn parse_lsof_pids_ignores_non_pid_lines() {
        assert_eq!(parse_lsof_pids("123\nnot-a-pid\n456\n"), vec![123, 456]);
    }

    #[test]
    fn parses_listening_socket_inodes_for_the_requested_linux_port() {
        let table = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:12D9 00000000:0000 0A 00000000:00000000 00:00000000 00000000  501        0 12345 1
   1: 0100007F:12DA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  501        0 23456 1
   2: 0100007F:12D9 00000000:0000 01 00000000:00000000 00:00000000 00000000  501        0 34567 1
";

        assert_eq!(
            listening_socket_inodes(table, 4825),
            std::collections::BTreeSet::from([12345])
        );
    }

    #[test]
    fn signal_process_treats_an_already_exited_process_as_stopped() {
        signal_process(i32::MAX, libc::SIGTERM)
            .expect("a process that no longer exists should already count as stopped");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pinned_process_exit_watcher_resolves_for_the_exact_process() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .expect("sleep fixture should start");
        let pid = child.id().expect("sleep fixture should have a pid");
        assert!(server_process_exists(pid));
        let identity = ServerProcessIdentity::pin(pid).expect("fixture identity should pin");
        identity
            .require_executable(std::path::Path::new("/bin/sleep"))
            .expect("fixture executable should match");
        let watcher = ServerProcessExitWatcher::register(identity.clone())
            .expect("fixture exit observer should register");

        child.kill().await.expect("fixture should stop");
        let _ = child.wait().await;
        let observed = tokio::time::timeout(Duration::from_secs(5), watcher.wait())
            .await
            .expect("exit event should arrive")
            .expect("exit observation should succeed");

        assert_eq!(observed, identity);
        assert!(!identity.is_alive());
        assert!(!server_process_exists(pid));
    }

    /// The single-listener rule: a launcher may only trust a port it can
    /// attribute to exactly one process.
    #[tokio::test(flavor = "current_thread")]
    async fn listening_server_pid_names_the_process_holding_the_socket() {
        let (mut child, port) = start_sigterm_ignoring_listener().await;
        let child_pid = child.id().expect("listener should have pid");

        assert_eq!(
            listening_server_pid(port).await.expect("one listener"),
            child_pid
        );

        stop_server_on_port(port).await.expect("shutdown");
        let _ = child.wait().await;
        assert!(
            listening_server_pid(port).await.is_err(),
            "a port with no listener has no pid to attribute it to"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn stop_server_on_port_escalates_to_sigkill_when_sigterm_is_ignored() {
        let (mut child, port) = start_sigterm_ignoring_listener().await;
        let child_pid = child.id().expect("listener should have pid");

        stop_server_on_port(port)
            .await
            .expect("shutdown should escalate and free the port");

        let status = child
            .wait()
            .await
            .expect("listener process should be reaped");
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "SIGTERM-ignoring listener should be killed with SIGKILL"
        );
        assert!(
            !process_is_running(child_pid),
            "SIGTERM-ignoring listener should no longer be running"
        );
        assert!(
            server_pids_on_port(port).await.unwrap().is_empty(),
            "port should not have remaining listener pids"
        );
        let rebound = std::net::TcpListener::bind(("127.0.0.1", port))
            .expect("port should be reusable after stale listener is killed");
        drop(rebound);
    }

    async fn start_sigterm_ignoring_listener() -> (Child, u16) {
        let script = r#"
import signal
import socket
import time

signal.signal(signal.SIGTERM, signal.SIG_IGN)
sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("127.0.0.1", 0))
sock.listen(1)
print(sock.getsockname()[1], flush=True)
while True:
    time.sleep(1)
"#;
        let mut command = Command::new("python3");
        command
            .arg("-c")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .expect("python3 should start SIGTERM-ignoring listener");
        let stdout = child
            .stdout
            .take()
            .expect("listener stdout should be piped");
        let mut lines = BufReader::new(stdout).lines();
        let ready = tokio::time::timeout(Duration::from_secs(5), lines.next_line()).await;
        let port = match ready {
            Ok(Ok(Some(line))) => line
                .trim()
                .parse::<u16>()
                .expect("listener should report a valid port"),
            Ok(Ok(None)) => panic!("listener closed stdout before reporting its port"),
            Ok(Err(error)) => panic!("failed to read listener port: {error}"),
            Err(_) => panic!("timed out waiting for listener port"),
        };
        (child, port)
    }
}
