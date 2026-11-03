use kanna_guest::supervisor::{Config, Supervisor};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

const USAGE: &str = "usage: kanna-guest serve (--socket PATH | --vsock) [--egress LOCAL_PORT] [--state DIR] [--path PATH] [--require-console]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("serve") {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let mut socket = None;
    let mut vsock = None;
    let mut egress = None;
    let home = std::env::var("HOME").unwrap_or_default();
    let mut config = Config::new(PathBuf::from(home).join(".kanna-build"));
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--socket" => socket = rest.next().map(PathBuf::from),
            "--state" => config.state = rest.next().map(PathBuf::from).unwrap_or(config.state),
            "--vsock" => vsock = Some(kanna_guest::vsock::JOB_PORT),
            "--egress" => egress = rest.next().and_then(|p| p.parse::<u16>().ok()),
            "--path" => config.path = rest.next().cloned().unwrap_or(config.path),
            "--require-console" => config.require_console = true,
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    if let Err(e) = wait_for_volume(&config.state) {
        eprintln!("kanna-guest: {e}");
        std::process::exit(1);
    }
    if let Some(port) = egress {
        config.egress = Some(port);
        if let Err(e) = kanna_guest::vsock::relay_egress(port, kanna_guest::vsock::EGRESS_PORT) {
            eprintln!("kanna-guest: can't relay egress on 127.0.0.1:{port}: {e}");
            std::process::exit(1);
        }
    }
    if socket.is_none() && vsock.is_none() {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let supervisor = match Supervisor::new(config) {
        Ok(supervisor) => supervisor,
        Err(e) => {
            eprintln!("kanna-guest: can't keep jobs: {e}");
            std::process::exit(1);
        }
    };
    if let Some(port) = vsock {
        let listener = match kanna_guest::vsock::Listener::bind(port) {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("kanna-guest: can't listen on vsock port {port}: {e}");
                std::process::exit(1);
            }
        };
        eprintln!("kanna-guest: serving jobs on vsock port {port}");
        loop {
            let Ok(conn) = listener.accept() else { continue };
            let supervisor = supervisor.clone();
            let _ = std::thread::Builder::new().name("kanna-guest conn".into()).spawn(move || {
                if let Err(e) = supervisor.serve(conn) {
                    eprintln!("kanna-guest: {e}");
                }
            });
        }
    }
    let Some(socket) = socket else { return };
    let _ = std::fs::remove_file(&socket);
    let listener = match UnixListener::bind(&socket) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("kanna-guest: can't listen on {}: {e}", socket.display());
            std::process::exit(1);
        }
    };
    let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600));
    eprintln!("kanna-guest: serving jobs on {}", socket.display());
    for conn in listener.incoming().flatten() {
        let supervisor = supervisor.clone();
        let _ = std::thread::Builder::new().name("kanna-guest conn".into()).spawn(move || {
            if let Err(e) = supervisor.serve(conn) {
                eprintln!("kanna-guest: {e}");
            }
        });
    }
}

fn wait_for_volume(state: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let mut parts = state.components();
    let (Some(std::path::Component::RootDir), Some(first), Some(name)) = (parts.next(), parts.next(), parts.next()) else { return Ok(()) };
    if first.as_os_str() != "Volumes" {
        return Ok(());
    }
    let volume = std::path::Path::new("/Volumes").join(name);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        let mounted = match (std::fs::metadata("/Volumes"), std::fs::metadata(&volume)) {
            (Ok(parent), Ok(child)) => parent.dev() != child.dev(),
            _ => false,
        };
        if mounted {
            return Ok(());
        }
        if std::time::Instant::now() > deadline {
            return Err(format!("{} isn't mounted; the state folder {} lives on it", volume.display(), state.display()));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
