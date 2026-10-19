use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const FLAG: &str = "--codex-remote";

const STARTUP: Duration = Duration::from_secs(10);

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
    for pair in args.windows(2).filter(|pair| pair[0] == "-c" || pair[0] == "--config") {
        server.extend(["-c".to_string(), pair[1].clone()]);
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
    let socket = socket(dir, std::process::id());
    let folder = socket.parent().unwrap_or(dir).to_path_buf();
    if let Err(why) = private(&folder) {
        return alone(program, args, &why);
    }
    sweep(&folder);
    let listen = format!("unix://{}", socket.display());
    let server = Command::new(program).args(server_args(args, &listen)).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let mut server = match server {
        Ok(server) => server,
        Err(error) => return alone(program, args, &format!("couldn't start {program} app-server: {error}")),
    };
    let deadline = Instant::now() + STARTUP;
    while !socket.exists() {
        if Instant::now() > deadline || matches!(server.try_wait(), Ok(Some(_))) {
            let _ = server.kill();
            let _ = server.wait();
            return alone(program, args, &format!("{program} app-server didn't open {}", socket.display()));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = Command::new(program).args(tui_args(args, &listen)).status();
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

fn alone(program: &str, args: &[String], why: &str) -> i32 {
    eprintln!("kanna: {why}, so codex runs on its own and takes no messages");
    let status = Command::new(program).args(args).arg("--no-daemon").status();
    status.ok().and_then(|s| s.code()).unwrap_or(127)
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
        assert_eq!(server_args(&args("--yolo -c model_auto_compact_token_limit=300000 hi"), listen), args("app-server -c approval_policy=\"never\" -c sandbox_mode=\"danger-full-access\" -c model_auto_compact_token_limit=300000 --listen unix:///s/codex-7.sock"), "the app-server compacts, so it gets the settings");
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
