use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const FLAG: &str = "--codex-remote";

const STARTUP: Duration = Duration::from_secs(10);

const GRACE: Duration = Duration::from_secs(5);

const UNPROMPTED: &[&str] = &["--yolo", "--dangerously-bypass-approvals-and-sandbox"];

pub fn command(helper: &str, dir: &str, agent: &str) -> String {
    format!("{helper} {FLAG} {dir} {agent}")
}

const SOCKET_PATH_MAX: usize = 100;

pub fn socket(dir: &Path, pid: u32) -> PathBuf {
    let socket = dir.join("codex").join(format!("codex-{pid}.sock"));
    match socket.as_os_str().len() <= SOCKET_PATH_MAX {
        true => socket,
        false => short_dir().join(format!("codex-{pid}.sock")),
    }
}

fn private(folder: &Path) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(folder);
    let meta = std::fs::symlink_metadata(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    let mine = meta.is_dir() && !meta.file_type().is_symlink() && meta.uid() == unsafe { libc::getuid() };
    if mine && meta.mode() & 0o077 != 0 {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700));
    }
    let meta = std::fs::symlink_metadata(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    match mine && !meta.file_type().is_symlink() && meta.mode() & 0o077 == 0 {
        true => Ok(()),
        false => Err(format!("{} isn't a folder only you can open", folder.display())),
    }
}

fn short_dir() -> PathBuf {
    PathBuf::from(format!("/tmp/kanna-{}", unsafe { libc::getuid() }))
}

pub fn server_args(args: &[String], listen: &str) -> Vec<String> {
    let mut server = vec!["app-server".to_string()];
    if args.iter().any(|a| UNPROMPTED.contains(&a.as_str())) {
        server.extend(["-c", "approval_policy=\"never\"", "-c", "sandbox_mode=\"danger-full-access\""].map(String::from));
    }
    server.extend(["--listen".to_string(), listen.to_string()]);
    server
}

pub fn tui_args(args: &[String], listen: &str) -> Vec<String> {
    let mut tui: Vec<String> = args.iter().filter(|a| !UNPROMPTED.contains(&a.as_str()) && *a != "--no-daemon").cloned().collect();
    tui.extend(["--remote".to_string(), listen.to_string()]);
    tui
}

pub fn run(dir: &Path, program: &str, args: &[String]) -> i32 {
    let children = Children::stopped_with_us();
    let socket = socket(dir, std::process::id());
    let folder = socket.parent().unwrap_or(dir).to_path_buf();
    if let Err(why) = private(&folder) {
        return alone(&children, program, args, &why);
    }
    sweep(&folder);
    let listen = format!("unix://{}", socket.display());
    let server = children.spawn(Command::new(program).args(server_args(args, &listen)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()));
    let mut server = match server {
        Ok(server) => server,
        Err(error) => return alone(&children, program, args, &format!("couldn't start {program} app-server: {error}")),
    };
    let deadline = Instant::now() + STARTUP;
    while !socket.exists() {
        if Instant::now() > deadline || matches!(server.try_wait(), Ok(Some(_))) {
            let _ = server.kill();
            let _ = server.wait();
            return alone(&children, program, args, &format!("{program} app-server didn't open {}", socket.display()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = children.spawn(Command::new(program).args(tui_args(args, &listen))).and_then(|mut tui| tui.wait());
    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_file(&socket);
    match status {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("kanna: couldn't run {program}: {error}");
            127
        }
    }
}

fn alone(children: &Children, program: &str, args: &[String], why: &str) -> i32 {
    eprintln!("kanna: {why}, so codex runs on its own and takes no messages");
    let status = children.spawn(Command::new(program).args(args).arg("--no-daemon")).and_then(|mut codex| codex.wait());
    status.ok().and_then(|s| s.code()).unwrap_or(127)
}

struct Children(Arc<Mutex<Vec<i32>>>);

impl Children {
    fn stopped_with_us() -> Children {
        let pids = Arc::new(Mutex::new(Vec::new()));
        let set = unsafe {
            let mut set: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut set);
            for signal in [libc::SIGHUP, libc::SIGTERM, libc::SIGINT] {
                libc::signal(signal, libc::SIG_DFL);
                libc::sigaddset(&mut set, signal);
            }
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            set
        };
        let stopping = pids.clone();
        let _ = std::thread::Builder::new().name("kanna.codex-signals".into()).spawn(move || {
            let mut signal = 0;
            while unsafe { libc::sigwait(&set, &mut signal) } != 0 {
                std::thread::sleep(Duration::from_millis(100));
            }
            let pids = stopping.lock().map(|pids| pids.clone()).unwrap_or_default();
            for &pid in &pids {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
            let deadline = Instant::now() + GRACE;
            let running = |pid: i32| unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) } == 0;
            while Instant::now() < deadline && pids.iter().any(|&pid| running(pid)) {
                std::thread::sleep(Duration::from_millis(50));
            }
            for &pid in pids.iter().filter(|&&pid| running(pid)) {
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
            unsafe { libc::_exit(128 + signal) };
        });
        Children(pids)
    }

    fn spawn(&self, command: &mut Command) -> std::io::Result<Child> {
        let mut pids = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let child = command.spawn()?;
        pids.push(child.id() as i32);
        Ok(child)
    }
}

fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("codex-").and_then(|n| n.strip_suffix(".sock")).and_then(|n| n.parse::<i32>().ok()) else { continue };
        if unsafe { libc::kill(pid, 0) } != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_permissions_go_to_the_server_and_the_tui_attaches_to_it() {
        let args = |line: &str| line.split(' ').map(String::from).collect::<Vec<_>>();
        let listen = "unix:///s/codex-7.sock";
        assert_eq!(server_args(&args("--yolo resume 019f"), listen), args("app-server -c approval_policy=\"never\" -c sandbox_mode=\"danger-full-access\" --listen unix:///s/codex-7.sock"));
        assert_eq!(tui_args(&args("--yolo resume 019f"), listen), args("resume 019f --remote unix:///s/codex-7.sock"), "a remote resume refuses permission flags");
        assert_eq!(server_args(&args("-a on-request"), listen), args("app-server --listen unix:///s/codex-7.sock"), "a codex that asks keeps asking");
        assert_eq!(tui_args(&args("-a on-request --no-daemon"), listen), args("-a on-request --remote unix:///s/codex-7.sock"));
    }

    #[test]
    fn a_socket_path_too_long_for_unix_sockets_moves_to_a_short_folder() {
        assert_eq!(socket(Path::new("/Users/u/Library/Application Support/kanna"), 7), PathBuf::from("/Users/u/Library/Application Support/kanna/codex/codex-7.sock"));
        let long = Path::new("/private/tmp").join("x".repeat(90));
        assert_eq!(socket(&long, 7), short_dir().join("codex-7.sock"));
    }

    #[test]
    fn the_socket_folder_is_only_yours_or_codex_runs_without_one() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("kanna-codex-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(private(&root.join("codex")), Ok(()), "made owner-only");
        assert_eq!(std::fs::metadata(root.join("codex")).unwrap().permissions().mode() & 0o777, 0o700);
        std::fs::create_dir_all(root.join("open")).unwrap();
        std::fs::set_permissions(root.join("open"), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(private(&root.join("open")), Ok(()), "your own folder that others can open is made owner-only");
        assert_eq!(std::fs::metadata(root.join("open")).unwrap().permissions().mode() & 0o777, 0o700);
        std::os::unix::fs::symlink(root.join("codex"), root.join("link")).unwrap();
        assert!(private(&root.join("link")).is_err(), "a symlink someone could point elsewhere");
        let _ = std::fs::remove_dir_all(&root);
    }
}
