mod agents;
mod app_link;
mod backlog;
mod db;
mod host;
mod keeper;
mod layout;
mod pty;
mod quiz;
mod reader;
mod settings;
mod shell;
mod status;
mod status_command;
mod diffs;
mod store;
mod task_scripts;
mod terminals;

use app_link::{AppLink, Outbox, APP_ASK, APP_PREFIX, HELLO};
use kanna_mux::json::Value;
use host::Host;
use kanna_build::Build;
use kanna_mux::json::json;
use kanna_mux::{Brand, Core, Instance, SocketServer, DEFAULT_NAME};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};
use reader::Taken;
use pty::Spawner;
use store::Store;
use terminals::{Restored, Terminals};

const USAGE: &str = "usage: kanna-daemon [--uses-keeper]

Serves Kanna's terminals and kmux's control protocol on $KANNA_SOCKET
(default: this build's instance's socket, such as kanna-dev/kanna.sock in
the app support folder; never the older Kanna.app's kanna/kanna.sock). Keeps its files in
$KANNA_STATE_DIR (default: that socket's folder). Gets its ptys from the
keeper of that folder, starting $KANNA_KEEPER (default: the kanna-keeper
beside this binary) if none runs. Stopping the daemon leaves the terminals
running in the keeper; it saves them in that folder (terminals/), and the
next daemon takes them back.

  --uses-keeper   exits 0: this daemon gets its ptys from a keeper";

const APP_TIMEOUT: Duration = Duration::from_secs(10);

const LOCK_WAIT: Duration = Duration::from_secs(5);

pub enum Event {
    Request(Value, Box<dyn FnOnce(Value) + Send>),
    Wake,
    Output { terminal: String, serial: u64 },
    Drained { terminal: String, serial: u64 },
    Status { terminal: String, serial: u64, status: Option<&'static str>, ended: bool },
    RulesUpdated { token: u64, report: kanna_detect::Report },
    Worktree { token: u64, result: Result<(String, String), String> },
    Finished { task: String, changes: Vec<db::Change> },
    Diff { id: String, diff: Value },
    DiffUpdating { id: String },
    KeeperReadable,
    KeeperExits,
    KeeperLost(String),
    AppHello { conn: u64, hello: Value, out: Outbox, stream: UnixStream },
    AppCaughtUp { conn: u64 },
    AppMessage { conn: u64, message: Value },
    AppGone { conn: u64 },
    Quit(i32),
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {}
        Some("--uses-keeper") => std::process::exit(0),
        Some("-h" | "--help") => return println!("{USAGE}"),
        Some(other) => {
            eprintln!("kanna-daemon: unknown argument {other}\n{USAGE}");
            std::process::exit(2);
        }
    }
    if let Err(message) = run() {
        eprintln!("kanna-daemon: {message}");
        std::process::exit(1);
    }
}

fn support_folder() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support");
    }
    std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

fn app_socket() -> PathBuf {
    support_folder().join("kanna/kanna.sock")
}

fn run() -> Result<(), String> {
    let _ = Brand::set_current(Brand::new("kanna", "Kanna"));
    let build_id = kanna_build::own_build_id();
    let build = Build::current();
    let socket = env_path("KANNA_SOCKET").unwrap_or_else(|| build.instance_folder(&support_folder(), &build.default_instance()).join(kanna_build::SOCKET));
    if socket.to_string_lossy().to_lowercase() == app_socket().to_string_lossy().to_lowercase() {
        return Err(format!("{} belongs to Kanna.app; use another KANNA_SOCKET", socket.display()));
    }
    let state_dir = env_path("KANNA_STATE_DIR").unwrap_or_else(|| socket.parent().map(Path::to_path_buf).unwrap_or_default());
    std::fs::create_dir_all(&state_dir).map_err(|e| format!("can't make {}: {e}", state_dir.display()))?;
    let socket_path = socket.to_string_lossy().into_owned();
    let _lock = lock(&state_dir)?;
    let _ = std::fs::write(state_dir.join(kanna_build::RECORD), format!("{:#}\n", build.record(app_bundle().as_deref())));

    let stop_signals = signal_set(&[libc::SIGTERM, libc::SIGINT, libc::SIGHUP]);
    unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &stop_signals, std::ptr::null_mut()) };

    let (events, inbox) = mpsc::channel::<Event>();
    let db = Rc::new(db::Db::open(&state_dir));
    let layout = layout::DaemonLayout::open(db.clone(), &state_dir);
    let core = Core::with_link(layout.clone());
    layout.connect(core.weak());
    let instance = instance_name(&socket);
    core.set_instance(Instance::with_socket(&instance, Some(socket_path.clone())));
    let wake = events.clone();
    core.set_wakeup(Some(Arc::new(move || {
        let _ = wake.send(Event::Wake);
    })));

    let mut server = SocketServer::new(
        &socket_path,
        Arc::new({
            let requests = events.clone();
            move |request, reply| {
                let _ = requests.send(Event::Request(request, reply));
            }
        }),
    );
    server.take_over(HELLO, app_link::takeover(events.clone()));
    server.start().map_err(|e| e.message)?;

    let keeper_bin = keeper::binary();
    let keeper = match keeper::Keeper::connect(&state_dir, &keeper_bin, events.clone()) {
        Ok(keeper) => Rc::new(keeper),
        Err(error) => {
            server.stop();
            return Err(format!("can't reach the keeper ({}): {error}", keeper_bin.display()));
        }
    };
    eprintln!("kanna-daemon: using the keeper of {} (pid {})", state_dir.display(), keeper.pid());
    let store = match Store::open(&state_dir) {
        Ok(store) => store,
        Err(error) => {
            server.stop();
            return Err(format!("can't keep the terminals in {}: {error}", state_dir.display()));
        }
    };

    let mut env = vec![("KANNA_SOCKET".to_string(), socket_path.clone()), ("KANNA_INSTANCE".to_string(), instance.clone())];
    env.extend(shell::env(&state_dir, &kanna_keeper::login_shell(), std::env::var("ZDOTDIR").ok()));
    let terminals = Rc::new(Terminals::new(keeper.clone(), store, events.clone(), env));
    *terminals.core.borrow_mut() = Some(core.weak());
    let rules = kanna_detect::Store::new(status_command::rules_config(&state_dir));
    let _rules_updater = rules.start();
    let detector = Rc::new(status::Detector::start(rules, events.clone()));
    *terminals.detector.borrow_mut() = Some(detector.clone());
    let status_commands = Rc::new(status_command::StatusCommand::new(detector.clone(), events.clone()));
    status_commands.install(&core);
    let backlog = Rc::new(backlog::Backlog::open(db.clone(), &state_dir, core.weak(), events.clone()));
    backlog.install(&core);
    let quiz = Rc::new(quiz::Quiz::new(db, Some(core.weak())));
    quiz.install(&core);
    let agents = agents::Agents::new(agents::home(), agents::skills(), socket_path.clone());
    agents.refresh();
    agents.install(&core);
    let diffs = Rc::new(diffs::Diffs::new(events.clone()));
    diffs.install(&core);
    let host = Rc::new(Host::new(terminals.clone()));
    core.set_host(Some(host.clone()));
    let on_change = Rc::downgrade(&terminals);
    let tasks_changed = Rc::downgrade(&backlog);
    let layout_changed = Rc::downgrade(&layout);
    core.set_on_change(Some(Rc::new(move || {
        if let Some(terminals) = on_change.upgrade() {
            terminals.mark_changed();
        }
        if let Some(layout) = layout_changed.upgrade() {
            layout.panes_changed();
        }
        if let Some(backlog) = tasks_changed.upgrade() {
            backlog.mark_changed();
        }
    })));
    let adopted = match keeper.adopt() {
        Ok(adopted) => adopted,
        Err(error) => {
            server.stop();
            return Err(format!("can't take the terminals back from the keeper: {error}"));
        }
    };
    let restored = terminals.restore(adopted);
    if restored != Restored::default() {
        eprintln!(
            "kanna-daemon: took back {} terminal(s) from the keeper ({} the disk didn't know), {} that had exited; {} gone",
            restored.adopted + restored.unknown,
            restored.unknown,
            restored.exited,
            restored.dropped
        );
    }
    core.install_panes(layout.saved_panes(), 0, 0);
    layout.restore(&core.with_panes(kanna_mux::pane_table));
    if let Err(error) = keeper.watch() {
        server.stop();
        terminals.shutdown();
        return Err(format!("can't watch the keeper: {error}"));
    }

    let app_timeout = std::env::var("KANNA_APP_TIMEOUT_MS").ok().and_then(|ms| ms.parse().ok()).map(Duration::from_millis).unwrap_or(APP_TIMEOUT);
    let app = Rc::new(AppLink::new(core.weak(), terminals.clone(), diffs.clone(), layout.clone(), app_timeout, build_id.clone()));
    let view_link = Rc::downgrade(&app);
    *layout.on_view.borrow_mut() = Some(Box::new(move |view| {
        if let Some(app) = view_link.upgrade() {
            app.layout(view);
        }
    }));
    app.connect_backlog(&backlog);
    host.connect(&app);
    let diff_link = Rc::downgrade(&app);
    *diffs.on_diff.borrow_mut() = Some(Box::new(move |views, payload| {
        if let Some(app) = diff_link.upgrade() {
            app.diff(views, payload);
        }
    }));
    let on_start = Rc::downgrade(&app);
    *terminals.on_start.borrow_mut() = Some(Box::new(move |pane| {
        if let Some(app) = on_start.upgrade() {
            app.started(pane);
        }
    }));
    let link = Rc::downgrade(&app);
    core.add_command(
        "screenshot",
        Rc::new(move |args, responder| {
            if let Some(link) = link.upgrade() {
                link.forward("screenshot", args, responder);
            }
        }),
    );
    for prefix in [APP_PREFIX, APP_ASK] {
        let link = Rc::downgrade(&app);
        core.add_prefix(
            prefix,
            Rc::new(move |cmd, args, responder| {
                if let Some(link) = link.upgrade() {
                    link.forward(cmd, args, responder);
                }
            }),
        );
    }

    let pid_file = state_dir.join("daemon.pid");
    let _ = std::fs::write(&pid_file, format!("{}\n", std::process::id()));
    eprintln!("kanna-daemon: listening on {socket_path} (pid {})", std::process::id());

    let quit = events.clone();
    let stopping = Arc::new(AtomicI32::new(0));
    let stop = stopping.clone();
    std::thread::Builder::new()
        .name("kanna.signals".into())
        .spawn(move || loop {
            let mut signal = 0;
            if unsafe { libc::sigwait(&stop_signals, &mut signal) } == 0 {
                stop.store(signal, Ordering::SeqCst);
                let _ = quit.send(Event::Quit(signal));
                return;
            }
        })
        .map_err(|e| e.to_string())?;
    drop(events);

    let mut failure = None;
    loop {
        let signal = stopping.load(Ordering::SeqCst);
        if signal != 0 {
            eprintln!("kanna-daemon: stopping (signal {signal})");
            break;
        }
        app.expire(Instant::now());
        terminals.save_if_changed();
        backlog.sweep();
        layout.flush();
        let deadline = [core.poll(), app.next_deadline(), backlog.tick(), layout.next_deadline()].into_iter().flatten().min();
        let event = match deadline {
            Some(at) => inbox.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => inbox.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let event = match event {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match event {
            Event::Request(request, reply) => match () {
                () if request.get("cmd").and_then(Value::as_str) == Some("capabilities") => {
                    let attached = app.attached();
                    let build = build_id.clone();
                    core.handle(request, move |mut answer| {
                        if answer.get("ok") == Some(&Value::Bool(true)) {
                            answer["app"] = Value::Bool(attached);
                            answer["build"] = json!(build);
                        }
                        reply(answer)
                    })
                }
                () => serve(&core, &backlog, request, reply),
            },
            Event::Wake => {}
            Event::Output { terminal, serial } => {
                let (taken, cwd) = terminals.output(&terminal, serial);
                if let (Some(cwd), Some(pane)) = (cwd, terminals.pane_of(&terminal)) {
                    app.folder(&pane, &cwd);
                }
                match taken {
                    Some(Taken::Bytes(bytes)) => app.output(&terminal, &bytes),
                    Some(Taken::Overflowed) => app.resync(&terminal),
                    None => {}
                }
            }
            Event::Drained { terminal, serial } => {
                if let Some(code) = terminals.drained(&terminal, serial) {
                    ended(&terminals, &backlog, &app, &terminal, code);
                }
            }
            Event::Status { terminal, serial, status, ended } => {
                if let Some(pane) = terminals.pane_of(&terminal).filter(|_| terminals.current(&terminal, serial)) {
                    core.with_pane_mut(&pane, |p| p.status = status.map(String::from));
                    app.status(&pane, status);
                    backlog.status(&pane, status, ended);
                }
            }
            Event::RulesUpdated { token, report } => status_commands.updated(token, report),
            Event::Worktree { token, result } => backlog.worktree_made(token, result),
            Event::Finished { task, changes } => backlog.finished(&task, changes),
            Event::Diff { id, diff } => diffs.loaded(&id, diff),
            Event::DiffUpdating { id } => diffs.updating(&id),
            Event::KeeperReadable | Event::KeeperExits => {
                let exits = keeper.take_exits();
                if matches!(event, Event::KeeperReadable) {
                    keeper.resume_watch();
                }
                match exits {
                    Ok(exits) => {
                        for exit in exits {
                            let code = exit.status.shell_status();
                            if let Some((terminal, code)) = terminals.exited(&exit.id, exit.pid, code) {
                                ended(&terminals, &backlog, &app, &terminal, code);
                            }
                        }
                    }
                    Err(reason) => {
                        failure = Some(reason);
                        break;
                    }
                }
            }
            Event::KeeperLost(reason) => {
                failure = Some(reason);
                break;
            }
            Event::AppHello { conn, hello, out, stream } => {
                if app.hello(conn, hello, out, stream) {
                    break;
                }
            }
            Event::AppMessage { conn, message } => {
                if let Some(request) = app.message(conn, message) {
                    let link = Rc::downgrade(&app);
                    serve(&core, &backlog, request, move |reply| {
                        if let Some(link) = link.upgrade() {
                            link.answer_app(conn, reply);
                        }
                    });
                }
            }
            Event::AppGone { conn } => app.gone(conn),
            Event::AppCaughtUp { conn } => app.caught_up(conn),
            Event::Quit(_) => continue,
        }
    }

    server.stop();
    app.close();
    layout.flush();
    layout.save_now();
    terminals.shutdown();
    let _ = std::fs::remove_file(&pid_file);
    match failure {
        Some(reason) => Err(format!("{reason}; stopping")),
        None => Ok(()),
    }
}

fn ended(terminals: &Terminals, backlog: &Rc<backlog::Backlog>, app: &AppLink, terminal: &str, code: i32) {
    app.exited(terminal, code);
    if let Some(pane) = terminals.pane_of(terminal) {
        backlog.exited(&pane, code);
        app.status(&pane, Some("exited"));
    }
}

fn instance_name(socket: &Path) -> String {
    let folder = socket.parent().and_then(Path::file_name).and_then(|name| name.to_str()).unwrap_or_default();
    kanna_build::parse_folder(folder).map_or_else(|| DEFAULT_NAME.to_string(), |(_, name)| name)
}

fn app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let helpers = exe.parent()?;
    let contents = helpers.parent()?;
    let app = contents.parent()?;
    (helpers.file_name()? == "Helpers" && contents.file_name()? == "Contents" && app.extension()? == "app").then(|| app.to_path_buf())
}

fn lock(state_dir: &Path) -> Result<std::fs::File, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let path = state_dir.join("daemon.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| format!("can't open {}: {e}", path.display()))?;
    let deadline = Instant::now() + LOCK_WAIT;
    while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if Instant::now() >= deadline {
            let other = std::fs::read_to_string(state_dir.join("daemon.pid")).unwrap_or_default();
            return Err(format!("another daemon (pid {}) uses {}", other.trim(), state_dir.display()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(file)
}

fn signal_set(signals: &[libc::c_int]) -> libc::sigset_t {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in signals {
            libc::sigaddset(&mut set, *signal);
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_is_named_by_its_folder() {
        let name = |path: &str| instance_name(Path::new(path));
        assert_eq!(name("/Users/x/Library/Application Support/kanna-dev/kanna.sock"), "default");
        assert_eq!(name("/Users/x/Library/Application Support/kanna-dev-work/kanna.sock"), "work");
        assert_eq!(name("/tmp/kanna-dev-a b/kanna.sock"), "default");
        assert_eq!(name("/x/kanna-prod/kanna.sock"), "default");
        assert_eq!(name("/x/kanna-staging-work/kanna.sock"), "work");
        assert_eq!(name("/x/kanna-dev-task-3f9a1c2e/kanna.sock"), "task-3f9a1c2e");
        assert_eq!(name("/tmp/kanna-e2e.x/kanna.sock"), "default");
    }
}

fn serve(core: &kanna_mux::CoreHandle, backlog: &Rc<backlog::Backlog>, request: Value, reply: impl FnOnce(Value) + 'static) {
    let by = request.get("args").and_then(|a| a.get("by")).and_then(Value::as_str).unwrap_or("user").to_string();
    let tasks = Rc::downgrade(backlog);
    core.handle(request, move |answer| {
        if let Some(backlog) = tasks.upgrade().filter(|_| answer.get("moves").and_then(Value::as_array).is_some_and(|m| !m.is_empty())) {
            backlog.moved(&answer, Some(&by));
        }
        reply(answer)
    })
}
