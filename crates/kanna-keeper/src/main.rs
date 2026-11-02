use kanna_keeper::client::Client;
use kanna_keeper::{paths, server, upgrade};
use std::time::Duration;

const USAGE: &str = "usage: kanna-keeper [--state-dir DIR] [--detach | --upgrade]

Runs the keeper for DIR (default $KANNA_STATE_DIR, else Kanna's data
folder), listening on DIR/keeper.sock. With --detach it runs in the
background and this command returns once it's listening. With --upgrade
it asks the keeper running for DIR to become this binary, keeping every
process it holds.";

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2)
}

fn fail(error: impl std::fmt::Display) -> ! {
    eprintln!("kanna-keeper: {error:#}");
    std::process::exit(1)
}

enum Mode {
    Foreground,
    Detach,
    Upgrade,
    Resume(i32),
    CheckUpgrade,
}

fn main() {
    let mut state_dir = None;
    let mut mode = Mode::Foreground;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--state-dir" => state_dir = Some(args.next().unwrap_or_else(|| usage()).into()),
            "--detach" => mode = Mode::Detach,
            "--upgrade" => mode = Mode::Upgrade,
            "--resume" => mode = Mode::Resume(args.next().and_then(|fd| fd.parse().ok()).unwrap_or_else(|| usage())),
            "--check-upgrade" => mode = Mode::CheckUpgrade,
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            _ => usage(),
        }
    }
    let state_dir = state_dir.unwrap_or_else(paths::default_state_dir);
    if matches!(mode, Mode::Foreground | Mode::Detach | Mode::Resume(_)) {
        kanna_crash::install("keeper", state_dir.clone());
    }
    let settings = server::Settings::from_env(state_dir.clone());
    let result = match mode {
        Mode::Foreground => server::run(settings, None),
        Mode::Detach => server::detach(settings),
        Mode::Resume(fd) => server::resume(settings, fd),
        Mode::CheckUpgrade => std::process::exit(upgrade::check()),
        Mode::Upgrade => {
            let exe = std::env::current_exe().and_then(|e| e.canonicalize()).unwrap_or_else(|e| fail(e));
            let mut client = Client::connect(&paths::socket(&state_dir))
                .unwrap_or_else(|e| fail(format!("no keeper is listening for {}: {e}", state_dir.display())));
            let _ = client.set_timeout(Some(Duration::from_secs(30)));
            match client.upgrade(&exe) {
                Ok(hello) => {
                    println!("upgraded the keeper (pid {}) to {}", hello.pid, exe.display());
                    Ok(())
                }
                Err(e) => fail(format!("the keeper didn't upgrade: {e}")),
            }
        }
    };
    if let Err(e) = result {
        fail(e);
    }
}
