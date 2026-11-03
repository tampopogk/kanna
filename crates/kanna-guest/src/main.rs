use kanna_guest::supervisor::{Config, Supervisor};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

const USAGE: &str = "usage: kanna-guest serve --socket PATH [--state DIR] [--proxy URL] [--require-console]";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("serve") {
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    let mut socket = None;
    let home = std::env::var("HOME").unwrap_or_default();
    let mut config = Config::new(PathBuf::from(home).join(".kanna-build"));
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--socket" => socket = rest.next().map(PathBuf::from),
            "--state" => config.state = rest.next().map(PathBuf::from).unwrap_or(config.state),
            "--proxy" => config.proxy = rest.next().cloned(),
            "--path" => config.path = rest.next().cloned().unwrap_or(config.path),
            "--require-console" => config.require_console = true,
            _ => {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
        }
    }
    let Some(socket) = socket else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let supervisor = match Supervisor::new(config) {
        Ok(supervisor) => supervisor,
        Err(e) => {
            eprintln!("kanna-guest: can't keep jobs: {e}");
            std::process::exit(1);
        }
    };
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
