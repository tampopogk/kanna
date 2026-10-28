macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("{} {}", $crate::stamp(), format_args!($($arg)*))
    };
}

mod agents;
mod app_link;
mod backlog;
mod db;
mod host;
mod keeper;
mod content_ids;
mod layout;
mod lifeline;
mod pty;
mod codex_remote;
mod messages;
mod projects;
mod quiz;
mod reader;
mod settings;
mod shell;
mod status;
mod status_command;
mod diffs;
mod machines;
mod files;
mod store;
mod task_scripts;
mod terminals;
mod toasts;

use app_link::{AppLink, Outbox, APP_ASK, APP_PREFIX, HELLO};
use kanna_mux::json::{Map, Value};
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

const STOP_GRACE: libc::c_uint = 5;
const SLOW_REQUEST: Duration = Duration::from_millis(50);

const SHUTDOWN_MARKER: &str = "daemon.shutdown";

const APP_QUIT_WAIT: Duration = Duration::from_secs(5);

const KEEPER_STOP_WAIT: Duration = Duration::from_secs(8);

enum Shutdown {
    App(Instant),
    Keeper(Instant),
    Done,
}

pub enum Event {
    Request(Value, Box<dyn FnOnce(Value) + Send>),
    Wake,
    Output { terminal: String, serial: u64 },
    Drained { terminal: String, serial: u64 },
    Status { terminal: String, serial: u64, status: Option<&'static str>, ended: bool },
    RulesUpdated { token: u64, report: kanna_detect::Report },
    Worktree { token: u64, result: Result<(String, String), String> },
    Branch { token: u64, branch: String },
    Turn { token: u64 },
    Posted { token: u64, result: Result<(), String> },
    Finished { task: String, changes: Vec<db::Change> },
    Diff { id: String, diff: Value },
    DiffUpdating { id: String },
    File { id: String, read: files::Read },
    KeeperReadable,
    KeeperExits,
    KeeperLost(String),
    AppHello { conn: u64, hello: Value, out: Outbox, stream: UnixStream, remote: Option<String> },
    AppCaughtUp { conn: u64 },
    AppMessage { conn: u64, message: Value },
    AppGone { conn: u64 },
    MachineConfirm { question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send> },
    MachinesSave(Value),
    Restart,
    Quit(i32),
    Shutdown(String),
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {}
        Some("--uses-keeper") => std::process::exit(0),
        Some(codex_remote::FLAG) if args.len() >= 3 => std::process::exit(codex_remote::run(Path::new(&args[1]), &args[2], &args[3..])),
        Some("-h" | "--help") => return println!("{USAGE}"),
        Some(other) => {
            eprintln!("kanna-daemon: unknown argument {other}\n{USAGE}");
            std::process::exit(2);
        }
    }
    let unstamped = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        eprint!("{} ", stamp());
        unstamped(info)
    }));
    match run() {
        Ok(Some(daemon)) => {
            use std::os::unix::process::CommandExt;
            log!("kanna-daemon: restarting as {}", daemon.display());
            let error = std::process::Command::new(&daemon).args(&args).exec();
            log!("kanna-daemon: can't restart as {}: {error}", daemon.display());
            std::process::exit(1);
        }
        Ok(None) => {}
        Err(message) => {
            log!("kanna-daemon: {message}");
            std::process::exit(1);
        }
    }
}

fn stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let seconds = now.as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&seconds, &mut tm) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec, now.subsec_millis())
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

fn run() -> Result<Option<PathBuf>, String> {
    let _ = Brand::set_current(Brand::new("kanna", "Kanna"));
    let build_id = kanna_build::own_build_id();
    let build = Build::current();
    let socket = env_path("KANNA_SOCKET").unwrap_or_else(|| build.instance_folder(&support_folder(), &build.default_instance()).join(kanna_build::SOCKET));
    if socket.to_string_lossy().to_lowercase() == app_socket().to_string_lossy().to_lowercase() {
        return Err(format!("{} belongs to Kanna.app; use another KANNA_SOCKET", socket.display()));
    }
    if let Some(owner) = lifeline::owner().filter(|pid| lifeline::gone(*pid)) {
        log!("kanna-daemon: its owner (pid {owner}, {}) is gone; not starting", lifeline::OWNER);
        return Ok(None);
    }
    let state_dir = env_path("KANNA_STATE_DIR").unwrap_or_else(|| socket.parent().map(Path::to_path_buf).unwrap_or_default());
    if let Ok(helper) = std::env::current_exe() {
        backlog::serve_codex_remotely(&helper, &state_dir);
    }
    std::fs::create_dir_all(&state_dir).map_err(|e| format!("can't make {}: {e}", state_dir.display()))?;
    let socket_path = socket.to_string_lossy().into_owned();
    let (_lock, waited) = lock(&state_dir)?;
    let marker = state_dir.join(SHUTDOWN_MARKER);
    if waited && marker.exists() {
        log!("kanna-daemon: the daemon before it shut this Kanna down while it waited; not starting");
        return Ok(None);
    }
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::write(state_dir.join(kanna_build::RECORD), format!("{:#}\n", build.record(app_bundle().as_deref())));

    let stop_signals = signal_set(&[libc::SIGTERM, libc::SIGINT, libc::SIGHUP]);
    unsafe {
        libc::pthread_sigmask(libc::SIG_BLOCK, &stop_signals, std::ptr::null_mut());
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &signal_set(&[libc::SIGALRM]), std::ptr::null_mut());
    }

    let (events, inbox) = mpsc::channel::<Event>();
    let db = Rc::new(db::Db::open(&state_dir));
    content_ids::migrate(&state_dir, &db);
    let layout = layout::DaemonLayout::open(db.clone(), &state_dir);
    let core = Core::with_link(layout.clone());
    layout.connect(core.weak());
    core.use_content_ids();
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
    log!("kanna-daemon: using the keeper of {} (pid {})", state_dir.display(), keeper.pid());
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
    let renaming = Rc::downgrade(&terminals);
    core.on_renamed(Rc::new(move |from, to| {
        if let Some(terminals) = renaming.upgrade() {
            terminals.renamed(from, to);
        }
    }));
    let rules = kanna_detect::Store::new(status_command::rules_config(&state_dir));
    let rules_updater = rules.start();
    let detector = Rc::new(status::Detector::start(rules, events.clone()));
    *terminals.detector.borrow_mut() = Some(detector.clone());
    let status_commands = Rc::new(status_command::StatusCommand::new(detector.clone(), events.clone()));
    status_commands.install(&core);
    let backlog = Rc::new(backlog::Backlog::open(db.clone(), &state_dir, core.weak(), events.clone()));
    backlog.install(&core);
    let planners = Rc::downgrade(&layout);
    *backlog.planners.borrow_mut() = Some(Box::new(move || {
        let Some(layout) = planners.upgrade() else { return vec![] };
        let view = layout.view();
        let workspaces = view["workspaces"].as_array().cloned().unwrap_or_default();
        workspaces.iter().filter(|w| w["where"].get("planner").is_some()).filter_map(|w| w["dock"]["pane"].as_str().map(String::from)).collect()
    }));
    let programs = Rc::downgrade(&detector);
    *backlog.channel.borrow_mut() = Some(Box::new(move |pane| {
        let detector = programs.upgrade()?;
        let program = detector.program(pane)?;
        Some(match program.agent.as_str() {
            "claude" => {
                let config = kanna_detect::program::environment(program.pid, "CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()).map(PathBuf::from).unwrap_or_else(|| agents::home().join(".claude"));
                messages::Channel::Claude(messages::inbox(&config, program.pid))
            }
            "codex" => messages::Channel::Codex(messages::codex_socket(&detector.argv(pane))),
            other => messages::Channel::Other(other.to_string()),
        })
    }));
    let modes = Rc::downgrade(&detector);
    *backlog.line_mode.borrow_mut() = Some(Box::new(move |pane| modes.upgrade().is_some_and(|detector| detector.line_mode(pane))));
    let argvs = Rc::downgrade(&detector);
    *backlog.permissions.borrow_mut() = Some(Box::new(move |pane| messages::permissions(&argvs.upgrade()?.argv(pane))));
    let projects = Rc::new(projects::Projects::new(db.clone(), std::env::var_os("HOME").map(std::path::PathBuf::from), [".kanna-v4/repos", ".kanna/repos"].map(|r| agents::home().join(r)).to_vec()));
    projects.install(&core);
    let toasts = Rc::new(toasts::Toasts::new(core.weak()));
    toasts.connect_backlog(&backlog);
    toasts.install(&core);
    let warnings = Rc::downgrade(&toasts);
    *backlog.on_warn.borrow_mut() = Some(Box::new(move |pane, name, why| {
        let Some(toasts) = warnings.upgrade() else { return };
        let toast = toasts::Toast { title: format!("Messages wait for {name}"), detail: Some(why.to_string()), pane: Some(pane.to_string()), task: None, key: Some(format!("messages:{pane}:{why}")) };
        if let Err(error) = toasts.raise(toast) {
            log!("kanna-daemon: can't raise a toast for {pane}: {}", error.message);
        }
    }));
    let quiz = Rc::new(quiz::Quiz::new(db.clone(), Some(core.weak())));
    quiz.install(&core);
    let machines = match machines::Machines::start(&state_dir, &db, events.clone()) {
        Ok(machines) => {
            log!("kanna-daemon: {}", machines.describe());
            let machines = Rc::new(machines);
            machines.install(&core);
            Some(machines)
        }
        Err(reason) => {
            log!("kanna-daemon: {reason}");
            None
        }
    };
    let agents = agents::Agents::new(agents::home(), agents::skills(), socket_path.clone(), agents::keys(&support_folder(), &state_dir));
    agents.refresh();
    agents.install(&core);
    let diffs = Rc::new(diffs::Diffs::new(events.clone()));
    diffs.install(&core);
    let files = Rc::new(files::Files::new(events.clone()));
    files.install(&core);
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
        log!(
            "kanna-daemon: took back {} terminal(s) from the keeper ({} the disk didn't know), {} that had exited; {} gone",
            restored.adopted + restored.unknown,
            restored.unknown,
            restored.exited,
            restored.dropped
        );
    }
    core.note_numbers(&layout.content_numbers());
    core.install_panes(layout.saved_panes(), 0, 0);
    layout.restore(&core.with_panes(kanna_mux::pane_table));
    if let Err(error) = keeper.watch() {
        server.stop();
        terminals.shutdown();
        return Err(format!("can't watch the keeper: {error}"));
    }

    let app_timeout = std::env::var("KANNA_APP_TIMEOUT_MS").ok().and_then(|ms| ms.parse().ok()).map(Duration::from_millis).unwrap_or(APP_TIMEOUT);
    let app = Rc::new(AppLink::new(core.weak(), terminals.clone(), diffs.clone(), files.clone(), layout.clone(), app_timeout, build_id.clone()));
    if let Some(machines) = &machines {
        let machines = Rc::downgrade(machines);
        *app.machine.borrow_mut() = Some(Box::new(move || machines.upgrade().map_or(Value::Null, |m| m.machine())));
    }
    let view_link = Rc::downgrade(&app);
    *layout.on_view.borrow_mut() = Some(Box::new(move |view| {
        if let Some(app) = view_link.upgrade() {
            app.layout(view);
        }
    }));
    app.connect_backlog(&backlog);
    let (folder_link, folder_core) = (Rc::downgrade(&app), core.weak());
    core.on_renamed(Rc::new(move |_, to| {
        let cwd = folder_core.upgrade().and_then(|core| core.pane(to)).and_then(|pane| pane.cwd);
        if let (Some(app), Some(cwd)) = (folder_link.upgrade(), cwd) {
            app.folder(to, &cwd);
        }
    }));
    let toast_link = Rc::downgrade(&app);
    *toasts.on_toast.borrow_mut() = Some(Box::new(move |record| toast_link.upgrade().is_some_and(|app| app.toast(record))));
    host.connect(&app);
    let diff_link = Rc::downgrade(&app);
    *diffs.on_diff.borrow_mut() = Some(Box::new(move |views, payload| {
        if let Some(app) = diff_link.upgrade() {
            app.diff(views, payload);
        }
    }));
    let file_link = Rc::downgrade(&app);
    *files.on_file.borrow_mut() = Some(Box::new(move |views, payload| {
        if let Some(app) = file_link.upgrade() {
            app.file(views, payload);
        }
    }));
    let on_start = Rc::downgrade(&app);
    *terminals.on_start.borrow_mut() = Some(Box::new(move |pane| {
        if let Some(app) = on_start.upgrade() {
            app.started(pane);
        }
    }));
    let link = Rc::downgrade(&app);
    let screenshot = link.clone();
    core.add_command(
        "screenshot",
        Rc::new(move |args, responder| {
            if let Some(link) = screenshot.upgrade() {
                link.forward("screenshot", args, responder);
            }
        }),
    );
    let front = link.clone();
    core.on_front(Rc::new(move |args, responder| match front.upgrade().filter(|app| app.attached()) {
        Some(app) => app.forward("app.front", args, responder),
        None => responder.reply(Ok(Map::new())),
    }));
    let resolving = Rc::downgrade(&backlog);
    core.add_command(
        "desk",
        Rc::new(move |mut args, responder| {
            let (Some(app), Some(backlog)) = (link.upgrade(), resolving.upgrade()) else { return };
            let Some(task) = args.get("task").and_then(Value::as_str).map(String::from) else {
                if let Some(folder) = args.get("project").and_then(Value::as_str).filter(|_| args.get("action").and_then(Value::as_str) == Some("desk")) {
                    args["place"] = projects::place(folder);
                }
                return app.forward("desk", args, responder);
            };
            backlog.agent(&task, move |found| match found {
                Ok(pane) => {
                    args["task"] = Value::String(pane);
                    app.forward("desk", args, responder);
                }
                Err(error) => responder.reply(Err(error)),
            });
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
    log!("kanna-daemon: listening on {socket_path} (pid {})", std::process::id());

    let quit = events.clone();
    let stopping = Arc::new(AtomicI32::new(0));
    let stop = stopping.clone();
    std::thread::Builder::new()
        .name("kanna.signals".into())
        .spawn(move || loop {
            let mut signal = 0;
            if unsafe { libc::sigwait(&stop_signals, &mut signal) } == 0 {
                unsafe { libc::alarm(STOP_GRACE) };
                stop.store(signal, Ordering::SeqCst);
                let _ = quit.send(Event::Quit(signal));
                return;
            }
        })
        .map_err(|e| e.to_string())?;
    if let Some(owner) = lifeline::watch(events.clone()) {
        log!("kanna-daemon: stops when its owner (pid {owner}, {}) exits", lifeline::OWNER);
    }
    drop(events);

    let mut failure = None;
    let mut restart = None;
    let mut shutdown: Option<Shutdown> = None;
    let begin = |why: &str, shutdown: &mut Option<Shutdown>| {
        if shutdown.is_some() {
            return;
        }
        log!("kanna-daemon: shutting down: {why}");
        let _ = std::fs::write(state_dir.join(SHUTDOWN_MARKER), format!("{why}\n"));
        *shutdown = Some(Shutdown::App(Instant::now() + APP_QUIT_WAIT));
        app.quit();
    };
    loop {
        let signal = stopping.load(Ordering::SeqCst);
        if signal != 0 {
            log!("kanna-daemon: stopping (signal {signal})");
            break;
        }
        if let Some(Shutdown::App(by)) = shutdown {
            if !app.attached() || Instant::now() >= by {
                if app.attached() {
                    log!("kanna-daemon: the app didn't quit in {}s; going on without it", APP_QUIT_WAIT.as_secs());
                    app.close();
                }
                let hung_up = terminals.hang_up_all();
                match keeper.stop() {
                    Ok(()) => {
                        log!("kanna-daemon: hung up {hung_up} terminal(s); the keeper is stopping");
                        shutdown = Some(Shutdown::Keeper(Instant::now() + KEEPER_STOP_WAIT));
                    }
                    Err(error) => {
                        log!("kanna-daemon: the keeper (pid {}) didn't take stop: {error}", keeper.pid());
                        shutdown = Some(Shutdown::Done);
                    }
                }
            }
        }
        match shutdown {
            Some(Shutdown::Keeper(by)) if Instant::now() >= by => {
                log!("kanna-daemon: the keeper (pid {}) didn't stop in {}s", keeper.pid(), KEEPER_STOP_WAIT.as_secs());
                break;
            }
            Some(Shutdown::Done) => break,
            _ => {}
        }
        app.expire(Instant::now());
        terminals.save_if_changed();
        layout.flush();
        let ticks = if shutdown.is_none() {
            backlog.sweep();
            backlog.tick()
        } else {
            Some(Instant::now() + Duration::from_millis(100))
        };
        let deadline = [core.poll(), app.next_deadline(), ticks, layout.next_deadline()].into_iter().flatten().min();
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
            Event::Request(request, reply) => timed(request_name(&request), "a client", || match request.get("cmd").and_then(Value::as_str) {
                Some("shutdown") => {
                    begin("asked to", &mut shutdown);
                    reply(json!({ "id": request.get("id").cloned().unwrap_or(Value::Null), "ok": true, "stopping": true, "pid": std::process::id() }))
                }
                _ if shutdown.is_some() => {
                    reply(json!({ "id": request.get("id").cloned().unwrap_or(Value::Null), "ok": false, "error": { "code": "stopping", "message": "this Kanna is stopping" } }))
                }
                Some("capabilities") => {
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
                _ => core.handle(request, reply),
            }),
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
            Event::Branch { token, branch } => backlog.branch_found(token, branch),
            Event::Turn { token } => backlog.next_turn(token),
            Event::Posted { token, result } => backlog.posted(token, result),
            Event::Finished { task, changes } => backlog.finished(&task, changes),
            Event::Diff { id, diff } => diffs.loaded(&id, diff),
            Event::DiffUpdating { id } => diffs.updating(&id),
            Event::File { id, read } => files.loaded(&id, read),
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
                    Err(_) if matches!(shutdown, Some(Shutdown::Keeper(_))) => {
                        log!("kanna-daemon: the keeper stopped");
                        break;
                    }
                    Err(reason) => {
                        failure = Some(reason);
                        break;
                    }
                }
            }
            Event::KeeperLost(_) if matches!(shutdown, Some(Shutdown::Keeper(_))) => {
                log!("kanna-daemon: the keeper stopped");
                break;
            }
            Event::KeeperLost(reason) => {
                failure = Some(reason);
                break;
            }
            Event::AppHello { conn, hello, out, stream, remote } => {
                if app.hello(conn, hello, out, stream, remote) {
                    break;
                }
                toasts.attached();
            }
            Event::AppMessage { conn, message } => {
                if let Some(request) = app.message(conn, message) {
                    if let Some(refused) = machines::refuse_remote(&request).filter(|_| !app.attached_here()) {
                        app.answer_app(conn, refused);
                        continue;
                    }
                    let link = Rc::downgrade(&app);
                    timed(request_name(&request), "the app", || {
                        core.handle(request, move |reply| {
                            if let Some(link) = link.upgrade() {
                                link.answer_app(conn, reply);
                            }
                        })
                    });
                }
            }
            Event::AppGone { conn } => app.gone(conn),
            Event::AppCaughtUp { conn } => app.caught_up(conn),
            Event::MachineConfirm { question, answer } => match &machines {
                Some(machines) => machines.confirm(&app, question, answer),
                None => answer(Err("this daemon has no endpoint".into())),
            },
            Event::MachinesSave(doc) => {
                if let Err(error) = db.put_machines(&doc) {
                    log!("kanna-daemon: can't save your machines in kanna.db: {error}");
                }
            }
            Event::Restart => match machines.as_ref().and_then(|m| m.install.as_ref()) {
                Some(install) if shutdown.is_none() => {
                    log!("kanna-daemon: updated to {}; restarting, and the keeper keeps the terminals", install.current().unwrap_or_default());
                    restart = Some(install.daemon());
                    break;
                }
                _ => {}
            },
            Event::Quit(_) => continue,
            Event::Shutdown(why) => begin(&why, &mut shutdown),
        }
    }

    app.close();
    layout.flush();
    layout.save_now();
    terminals.shutdown();
    if let Some(machines) = &machines {
        machines.stop();
    }
    let _ = std::fs::remove_file(&pid_file);
    std::mem::forget(rules_updater);
    server.stop();
    match failure {
        Some(reason) => Err(format!("{reason}; stopping")),
        None => Ok(restart),
    }
}

fn timed(name: String, from: &str, handle: impl FnOnce()) {
    let started = Instant::now();
    handle();
    let took = started.elapsed();
    if took > SLOW_REQUEST {
        log!("kanna-daemon: {name} from {from} took {}ms", took.as_millis());
    }
}

fn request_name(request: &Value) -> String {
    let cmd = request.get("cmd").and_then(Value::as_str).unwrap_or("?");
    match request.get("args").and_then(|args| args.get("action")).and_then(Value::as_str) {
        Some(action) => format!("{cmd} {action}"),
        None => cmd.to_string(),
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

fn lock(state_dir: &Path) -> Result<(std::fs::File, bool), String> {
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
    let mut waited = false;
    while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        if Instant::now() >= deadline {
            let other = std::fs::read_to_string(state_dir.join("daemon.pid")).unwrap_or_default();
            return Err(format!("another daemon (pid {}) uses {}", other.trim(), state_dir.display()));
        }
        waited = true;
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok((file, waited))
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

