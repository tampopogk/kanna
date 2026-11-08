macro_rules! log {
    ($($arg:tt)*) => {
        eprintln!("{} {}", $crate::stamp(), format_args!($($arg)*))
    };
}

mod accounts;
mod agent_stats;
mod agents;
mod app_link;
mod builds;
mod guests;
mod tasks;
mod db;
mod desk_news;
mod host;
mod keeper;
mod content_ids;
mod crashes;
mod layout;
mod lifeline;
mod perf;
mod phone;
mod pty;
mod codex_remote;
mod messages;
mod project_import;
mod projects;
mod quiz;
mod quiz_map;
mod quiz_round;
mod quiz_writer;
mod replica;
mod reports;
mod stamped;
mod stats;
mod quality;
mod coverage;
mod reader;
mod runtime;
mod runs;
mod schedule;
mod settings;
mod shell;
mod sizes;
mod status;
mod status_command;
mod diffs;
mod logs;
mod machines;
mod files;
mod reads;
mod screenshot;
mod serial;
mod store;
mod stacks;
mod task_events;
mod task_scripts;
mod terminals;
mod transfer;
mod toasts;
mod wire;

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
use std::sync::Arc;
use std::time::{Duration, Instant};
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
next daemon takes them back. With $KANNA_LISTEN_FD (Kanna.app sets it), it
serves on that listening socket, which the app keeps, instead of binding one.

  --uses-keeper   exits 0: this daemon gets its ptys from a keeper";

const APP_TIMEOUT: Duration = Duration::from_secs(10);

const LOCK_WAIT: Duration = Duration::from_secs(5);
const LISTEN_FD: &str = "KANNA_LISTEN_FD";

const STOP_GRACE: libc::c_uint = 5;

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
    Output { terminal: String, serial: u64 },
    Drained { terminal: String, serial: u64 },
    Status { terminal: String, serial: u64, status: Option<&'static str>, ended: bool },
    TooOld { terminal: String, serial: u64, why: String },
    Reported { terminal: String, serial: u64, kept: reports::Kept },
    Unreported { terminal: String },
    Stacks { seen: Vec<stacks::Seen> },
    Heads,
    Turn { token: u64 },
    Merged { task: String, merge: desk_news::Merge },
    Diff { id: String, diff: Value },
    DiffUpdating { id: String },
    Log { id: String, log: Value },
    File { id: String, read: files::Read },
    Serial(serial::SerialEvent),
    Sampled(stats::Sampled),
    Covered(coverage::Covered),
    KeeperReadable,
    KeeperHeard,
    KeeperLost(String),
    AppHello { conn: u64, hello: Value, out: Outbox, stream: UnixStream, remote: Option<String> },
    AppCaughtUp { conn: u64 },
    AppBehind { conn: u64, queued: usize },
    AppUsed { conn: u64, terminal: String },
    AppMessage { conn: u64, message: Value },
    AppGone { conn: u64 },
    MachineConfirm { question: Value, answer: Box<dyn FnOnce(Result<bool, String>) + Send> },
    MachinesSave(Value),
    PeerRequest { from: String, request: Value, reply: Box<dyn FnOnce(Value) + Send> },
    AccountBroken(Value),
    AccountUnreadable(Value),
    Replica { token: u64, reply: Result<Value, String> },
    Quit(i32),
    Shutdown(String),
}

impl Event {
    fn kind(&self) -> &'static str {
        match self {
            Event::Request(..) => "loop.request",
            Event::Output { .. } => "loop.output",
            Event::Drained { .. } => "loop.drained",
            Event::Status { .. } => "loop.status",
            Event::TooOld { .. } => "loop.status",
            Event::Reported { .. } | Event::Unreported { .. } => "loop.reported",
            Event::Stacks { .. } | Event::Heads | Event::Turn { .. } | Event::Merged { .. } => "loop.tasks",
            Event::Diff { .. } | Event::DiffUpdating { .. } => "loop.diff",
            Event::Log { .. } => "loop.log",
            Event::File { .. } => "loop.file",
            Event::Serial(_) => "loop.serial",
            Event::Sampled(_) => "loop.stats",
            Event::Covered(_) => "loop.coverage",
            Event::KeeperReadable | Event::KeeperHeard | Event::KeeperLost(_) => "loop.keeper",
            Event::AppHello { .. } | Event::AppCaughtUp { .. } | Event::AppBehind { .. } | Event::AppUsed { .. } | Event::AppGone { .. } => "loop.app",
            Event::AppMessage { .. } => "loop.app.message",
            Event::MachineConfirm { .. } | Event::MachinesSave(_) | Event::Replica { .. } => "loop.machines",
            Event::PeerRequest { .. } | Event::AccountBroken(_) | Event::AccountUnreadable(_) => "loop.accounts",
            Event::Quit(_) | Event::Shutdown(_) => "loop.stop",
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {}
        Some("--uses-keeper") => std::process::exit(0),
        Some(codex_remote::FLAG) if args.len() >= 3 => std::process::exit(codex_remote::run(Path::new(&args[1]), &args[2], &args[3..])),
        Some(accounts::FLAG) if args.len() >= 3 => std::process::exit(accounts::run_with_account(Path::new(&args[1]), &args[2], &args[3..])),
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
        Ok(()) => {}
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

fn run() -> Result<(), String> {
    let _ = Brand::set_current(Brand::new("kanna", "Kanna"));
    let build_id = kanna_build::own_build_id();
    let build = Build::current();
    let socket = env_path("KANNA_SOCKET").unwrap_or_else(|| build.instance_folder(&support_folder(), &build.default_instance()).join(kanna_build::SOCKET));
    if socket.to_string_lossy().to_lowercase() == app_socket().to_string_lossy().to_lowercase() {
        return Err(format!("{} belongs to Kanna.app; use another KANNA_SOCKET", socket.display()));
    }
    if let Some(owner) = lifeline::owner().filter(|pid| lifeline::gone(*pid)) {
        log!("kanna-daemon: its owner (pid {owner}, {}) is gone; not starting", lifeline::OWNER);
        return Ok(());
    }
    let state_dir = env_path("KANNA_STATE_DIR").unwrap_or_else(|| socket.parent().map(Path::to_path_buf).unwrap_or_default());
    if let Ok(helper) = std::env::current_exe() {
        tasks::serve_codex_remotely(&helper, &state_dir);
    }
    std::fs::create_dir_all(&state_dir).map_err(|e| format!("can't make {}: {e}", state_dir.display()))?;
    let socket_path = socket.to_string_lossy().into_owned();
    let (_lock, waited) = lock(&state_dir)?;
    let marker = state_dir.join(SHUTDOWN_MARKER);
    if marker.exists() && (waited || started_before(&marker)) {
        log!("kanna-daemon: the daemon before it shut this Kanna down while it waited; not starting");
        return Ok(());
    }
    let _ = std::fs::remove_file(&marker);
    kanna_crash::install("daemon", state_dir.clone());
    let _ = std::fs::write(state_dir.join(kanna_build::RECORD), format!("{:#}\n", build.record(app_bundle().as_deref())));

    let stop_signals = signal_set(&[libc::SIGTERM, libc::SIGINT, libc::SIGHUP]);
    unsafe {
        libc::pthread_sigmask(libc::SIG_BLOCK, &stop_signals, std::ptr::null_mut());
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &signal_set(&[libc::SIGALRM]), std::ptr::null_mut());
    }

    let (events, mut inbox) = runtime::channel();
    let db = Rc::new(db::Db::open(&state_dir));
    content_ids::migrate(&state_dir, &db);
    let layout = layout::DaemonLayout::open(db.clone(), &state_dir);
    let core = Core::with_link(layout.clone());
    layout.connect(core.weak());
    core.use_content_ids();
    let instance = instance_name(&socket);
    core.set_instance(Instance::with_socket(&instance, Some(socket_path.clone())));
    let _on_the_loop = core.enter();
    core.set_timing(Some(Rc::new(runtime::timed)));

    let mut server = SocketServer::new(
        &socket_path,
        Arc::new({
            let requests = events.clone();
            move |request, reply| {
                let _ = requests.send(Event::Request(request, reply));
            }
        }),
    );
    let wire = Arc::new(wire::Wire::new(events.clone()));
    server.take_over(HELLO, app_link::takeover(events.clone(), wire.clone()));
    let build_callers: builds::Callers = Default::default();
    server.take_over(builds::OPEN, builds::takeover(build_callers.clone()));
    let server = std::mem::ManuallyDrop::new(server);
    match inherited_listener()? {
        Some(listener) => server.start_with(listener),
        None => server.start(),
    }
    .map_err(|e| e.message)?;

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

    let mut env = vec![("KANNA_SOCKET".to_string(), socket_path.clone()), ("KANNA_INSTANCE".to_string(), instance.clone()), ("AGY_CLI_OSC7501".to_string(), "on".to_string())];
    env.extend(shell::env(&state_dir, &kanna_keeper::login_shell(), std::env::var("ZDOTDIR").ok()));
    let kanna_cli = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join("kanna"))).unwrap_or_default();
    if let Some(shim) = builds::install_shim(&state_dir, &kanna_cli, builds::shim_wanted()) {
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        env.push(("PATH".to_string(), format!("{}:{path}", shim.display())));
        env.push(("KANNA_BUILD_SHIM_DIR".to_string(), shim.to_string_lossy().into_owned()));
    }
    let terminals = Rc::new(Terminals::new(keeper.clone(), store, events.clone(), env, wire.clone()));
    *terminals.core.borrow_mut() = Some(core.weak());
    let detector = Rc::new(status::Detector::start(events.clone()));
    *terminals.detector.borrow_mut() = Some(detector.clone());
    let status_commands = Rc::new(status_command::StatusCommand::new(detector.clone()));
    status_commands.install(&core);
    let tasks = Rc::new(tasks::Tasks::open(db.clone(), &state_dir, core.weak(), events.clone()));
    tasks.install(&core);
    *tasks.repos.borrow_mut() = Some(projects::repos());
    let planners = Rc::downgrade(&layout);
    *tasks.planners.borrow_mut() = Some(Box::new(move || {
        let Some(layout) = planners.upgrade() else { return vec![] };
        let view = layout.view();
        let workspaces = view["workspaces"].as_array().cloned().unwrap_or_default();
        workspaces.iter().filter(|w| w["where"].get("planner").is_some()).filter_map(|w| w["dock"]["pane"].as_str().map(String::from)).collect()
    }));
    let planners = Rc::downgrade(&layout);
    *tasks.windowless.borrow_mut() = Some(Box::new(move || {
        let Some(layout) = planners.upgrade() else { return vec![] };
        let view = layout.view();
        let workspaces = view["workspaces"].as_array().cloned().unwrap_or_default();
        workspaces.iter().filter(|w| w["where"]["planner"] == tasks::WINDOWLESS).filter_map(|w| w["dock"]["pane"].as_str().map(String::from)).collect()
    }));
    let run_of = {
        let core = core.weak();
        Rc::new(move |pane: &str| core.upgrade()?.pane(pane)?.terminal)
    };
    let programs = Rc::downgrade(&detector);
    let run = run_of.clone();
    *tasks.channel.borrow_mut() = Some(Box::new(move |pane| {
        let detector = programs.upgrade()?;
        let terminal = run(pane)?;
        let program = detector.program(&terminal)?;
        Some(match program.agent.as_str() {
            "claude" => {
                let config = kanna_detect::program::environment(program.pid, "CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()).map(PathBuf::from).unwrap_or_else(|| agents::home().join(".claude"));
                messages::Channel::Claude(messages::inbox(&config, program.pid))
            }
            "codex" => messages::Channel::Codex(messages::codex_socket(&detector.argv(&terminal))),
            "antigravity" => messages::Channel::Other("agy".to_string()),
            other => messages::Channel::Other(other.to_string()),
        })
    }));
    let (ages, run) = (Rc::downgrade(&detector), run_of.clone());
    *tasks.age.borrow_mut() = Some(Box::new(move |pane| lifeline::age_of(ages.upgrade()?.program(&run(pane)?)?.pid)));
    let (modes, run) = (Rc::downgrade(&detector), run_of.clone());
    *tasks.line_mode.borrow_mut() = Some(Box::new(move |pane| modes.upgrade().zip(run(pane)).is_some_and(|(detector, terminal)| detector.line_mode(&terminal))));
    let (trees, run) = (Rc::downgrade(&detector), run_of.clone());
    *tasks.background.borrow_mut() = Some(Box::new(move |pane| {
        let program = trees.upgrade()?.program(&run(pane)?)?;
        kanna_detect::agents::idle_with_background(&program.agent).then(|| kanna_detect::program::background(program.pid))
    }));
    let argvs = Rc::downgrade(&detector);
    *tasks.permissions.borrow_mut() = Some(Box::new(move |pane| messages::permissions(&argvs.upgrade()?.argv(&run_of(pane)?))));
    let projects = Rc::new(projects::Projects::new(db.clone(), std::env::var_os("HOME").map(std::path::PathBuf::from), Some(projects::repos())));
    projects.install(&core);
    let toasts = Rc::new(toasts::Toasts::new(core.weak()));
    toasts.connect_tasks(&tasks);
    toasts.install(&core);
    let warnings = Rc::downgrade(&toasts);
    *tasks.on_warn.borrow_mut() = Some(Box::new(move |pane, name, why| {
        let Some(toasts) = warnings.upgrade() else { return };
        let toast = toasts::Toast { title: format!("Messages wait for {name}"), detail: Some(why.to_string()), pane: Some(pane.to_string()), task: None, key: Some(format!("messages:{pane}:{why}")), from: None };
        if let Err(error) = toasts.raise(toast) {
            log!("kanna-daemon: can't raise a toast for {pane}: {}", error.message);
        }
    }));
    let runs = Rc::new(runs::Runs::open(db.clone(), core.weak(), Rc::downgrade(&tasks)));
    runs.install(&core);
    let closing = Rc::downgrade(&runs);
    *tasks.on_close.borrow_mut() = Some(Box::new(move |task| {
        if let Some(runs) = closing.upgrade() {
            runs.task_closed(task);
        }
    }));
    let (stopping, starting) = (Rc::downgrade(&runs), Rc::downgrade(&runs));
    *tasks.on_stop.borrow_mut() = Some(Box::new(move |task| {
        if let Some(runs) = stopping.upgrade() {
            runs.task_stopped(task);
        }
    }));
    *tasks.on_start.borrow_mut() = Some(Box::new(move |task| {
        if let Some(runs) = starting.upgrade() {
            runs.task_started(task);
        }
    }));
    let writers = {
        let layout = Rc::downgrade(&layout);
        quiz_writer::Writers::install(db.clone(), tasks.clone(), &core, Box::new(move || {
            let Some(layout) = layout.upgrade() else { return vec![] };
            let view = layout.view();
            view["workspaces"].as_array().into_iter().flatten().filter(|w| w["where"]["quiz"] == tasks::WINDOWLESS).filter_map(|w| w["dock"]["pane"].as_str().map(String::from)).collect()
        }))
    };
    let quiz = Rc::new(quiz::Quiz::new(db.clone(), Some(core.weak()), Some(writers)));
    quiz.install(&core);
    let scheduler = Rc::new(schedule::Scheduler::new(db.clone(), tasks.clone()));
    scheduler.install(&core, std::env::var("KANNA_DEBUG").is_ok_and(|v| v == "1"));
    scheduler.watch_deliveries(&tasks);
    let guests = guests::Guests::new(&state_dir);
    guests.install(&core);
    if cfg!(target_os = "macos") {
        guests.start();
    }
    let _ = guests::GUESTS.set(guests);
    let machines = match machines::Machines::start(&state_dir, &db, events.clone(), wire.clone()) {
        Ok(machines) => {
            log!("kanna-daemon: {}", machines.describe());
            let machines = Rc::new(machines);
            machines.install(&core);
            if let Some(caller) = machines.caller() {
                let _ = build_callers.set(caller);
            }
            Some(machines)
        }
        Err(reason) => {
            log!("kanna-daemon: {reason}");
            None
        }
    };
    let accounts = accounts::Service::start(&state_dir, machines.as_ref().and_then(|m| m.caller()));
    let broken = events.clone();
    accounts.on_broken(Box::new(move |account| {
        let _ = broken.send(Event::AccountBroken(account));
    }));
    let unreadable = events.clone();
    accounts.on_unreadable(Box::new(move |account| {
        let _ = unreadable.send(Event::AccountUnreadable(account));
    }));
    accounts.install(&core);
    if let Some(machines) = &machines {
        let withdrawing = Arc::downgrade(&accounts);
        machines.before_revoke(Arc::new(move |host| withdrawing.upgrade().and_then(|accounts| accounts.withdraw(host))));
    }
    if let Some(machines) = &machines {
        let roster = Rc::downgrade(machines);
        *tasks.roster.borrow_mut() = Some(Box::new(move || roster.upgrade().map(|m| m.roster()).unwrap_or_default()));
        let (exchanging, replies) = (Rc::downgrade(machines), events.clone());
        *tasks.exchange.borrow_mut() = Some(Box::new(move |peer, request, token| {
            let replies = replies.clone();
            let done: Box<dyn FnOnce(Result<Value, String>) + Send> = Box::new(move |reply| {
                let _ = replies.send(Event::Replica { token, reply });
            });
            match exchanging.upgrade() {
                Some(machines) => machines.exchange(peer, request, done),
                None => done(Err("the daemon is stopping".into())),
            }
        }));
    }
    if let Some(machines) = &machines {
        let (sending, replies) = (Rc::downgrade(machines), events.clone());
        *tasks.payload.borrow_mut() = Some(Box::new(move |peer, id, args, path, token| {
            let replies = replies.clone();
            let done: Box<dyn FnOnce(Result<Value, String>) + Send> = Box::new(move |reply| {
                let _ = replies.send(Event::Replica { token, reply });
            });
            match sending.upgrade() {
                Some(machines) => machines.transfer(peer, id, args, path, done),
                None => done(Err("the daemon is stopping".into())),
            }
        }));
    }
    tasks.join();
    let agents = agents::Agents::new(agents::home(), agents::skills(), socket_path.clone(), agents::keys(&support_folder(), &state_dir));
    agents.refresh();
    agents.install(&core);
    let diffs = Rc::new(diffs::Diffs::new(events.clone()));
    diffs.install(&core);
    perf::install(&core);
    let files = Rc::new(files::Files::new(events.clone()));
    files.install(&core, "files");
    let threads = Rc::new(files::Files::new(events.clone()));
    threads.install(&core, "threads");
    reads::install(&core);
    let serial = Rc::new(serial::Serial::new(events.clone(), &state_dir));
    serial.install(&core);
    let logs = Rc::new(logs::Logs::new(events.clone()));
    logs.install(&core);
    let refs = Rc::downgrade(&tasks);
    *logs.task_refs.borrow_mut() = Some(Box::new(move || refs.upgrade().map(|tasks| tasks.refs()).unwrap_or_default()));
    let stats = Rc::new(stats::Stats::new(db.clone(), events.clone(), &state_dir));
    stats.install(&core);
    let host = Rc::new(Host::new(terminals.clone(), serial.clone()));
    core.set_host(Some(host.clone()));
    let noted = core.weak();
    let on_change = Rc::downgrade(&terminals);
    let tasks_changed = Rc::downgrade(&tasks);
    let layout_changed = Rc::downgrade(&layout);
    core.set_on_change(Some(Rc::new(move || {
        if let Some(terminals) = on_change.upgrade() {
            terminals.mark_changed();
        }
        if let Some(core) = noted.upgrade() {
            kanna_crash::note_panes(core.with_panes(|panes| panes.iter().map(|p| json!({ "id": p.id, "name": p.name, "type": p.kind.as_str() })).collect()));
        }
        if let Some(layout) = layout_changed.upgrade() {
            layout.panes_changed();
        }
        if let Some(tasks) = tasks_changed.upgrade() {
            tasks.mark_changed();
        }
    })));
    let adopted = match keeper.adopt() {
        Ok(adopted) => adopted,
        Err(error) => {
            server.stop();
            return Err(format!("can't take the terminals back from the keeper: {error}"));
        }
    };
    let restored = terminals.restore(adopted, reports::load(&db));
    reports::keep_only(&db, &terminals.running());
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
    core.install_panes(layout.saved_panes(), 0, 0, 0);
    layout.restore(&core.with_panes(kanna_mux::pane_table));
    serial.restore(core.with_panes(|panes| panes.iter().cloned().collect()));
    tasks.repair_comment_targets();
    tasks.settle_statuses();
    if let Err(error) = keeper.watch() {
        server.stop();
        terminals.shutdown();
        return Err(format!("can't watch the keeper: {error}"));
    }

    let app_timeout = std::env::var("KANNA_APP_TIMEOUT_MS").ok().and_then(|ms| ms.parse().ok()).map(Duration::from_millis).unwrap_or(APP_TIMEOUT);
    let app = Rc::new(AppLink::new(core.weak(), terminals.clone(), diffs.clone(), (files.clone(), threads.clone()), serial.clone(), layout.clone(), app_timeout, build_id.clone(), server.listener()));
    let (shown, statuses) = (core.weak(), Rc::downgrade(&app));
    *runs.on_status.borrow_mut() = Some(Box::new(move |pane| {
        if let (Some(core), Some(app)) = (shown.upgrade(), statuses.upgrade()) {
            app.status(pane, core.pane(pane).and_then(|p| p.status().map(String::from)).as_deref());
        }
    }));
    let phone = phone::Phone::install(&core, db.clone(), tasks.clone(), quiz.clone(), machines.as_ref().and_then(|m| m.caller()), app.clone());
    let (stats_link, stats_phone) = (Rc::downgrade(&app), Rc::downgrade(&phone));
    *stats.on_change.borrow_mut() = Some(Box::new(move |repo| {
        if let Some(app) = stats_link.upgrade() {
            app.stats_changed(repo);
        }
        if let Some(phone) = stats_phone.upgrade() {
            phone.stats_changed(repo);
        }
    }));
    if let Some(machines) = &machines {
        let machines = Rc::downgrade(machines);
        *app.machine.borrow_mut() = Some(Box::new(move || machines.upgrade().map_or(Value::Null, |m| m.machine())));
    }
    let (found_tasks, found_app) = (Rc::downgrade(&tasks), Rc::downgrade(&app));
    projects::on_found(Rc::new(move || {
        if let Some(tasks) = found_tasks.upgrade() {
            tasks.mark_changed();
        }
        if let Some(app) = found_app.upgrade() {
            app.project_found();
        }
    }));
    tasks.prime_projects();
    let view_link = Rc::downgrade(&app);
    *layout.on_view.borrow_mut() = Some(Box::new(move |view| {
        if let Some(app) = view_link.upgrade() {
            app.layout(view);
        }
    }));
    *app.logs.borrow_mut() = Some(logs.clone());
    app.connect_tasks(&tasks);
    app.connect_schedules(&scheduler);
    let shown = tasks.on_change.borrow_mut().take();
    let logs_told = Rc::downgrade(&logs);
    *tasks.on_change.borrow_mut() = Some(Box::new(move |view| {
        if let Some(logs) = logs_told.upgrade() {
            logs.tasks_changed();
        }
        if let Some(shown) = &shown {
            shown(view);
        }
    }));
    let run_link = Rc::downgrade(&app);
    core.on_new_run(Rc::new(move |pane| {
        if let Some(app) = run_link.upgrade() {
            app.status(pane, None);
        }
    }));
    let toast_link = Rc::downgrade(&app);
    *toasts.on_toast.borrow_mut() = Some(Box::new(move |record| toast_link.upgrade().is_some_and(|app| app.toast(record))));
    host.connect(&app);
    let crashes = Rc::new(crashes::Crashes::new(db.clone(), &state_dir, build.clone()));
    crashes.install(&core);
    let posting = Rc::downgrade(&tasks);
    *crashes.post.borrow_mut() = Some(Box::new(move |repo, text| posting.upgrade().ok_or_else(|| "the daemon is stopping".to_string())?.post(messages::PLANNER, Some(repo), "kanna", text).map_err(|error| error.message)));
    let banner_link = Rc::downgrade(&app);
    *crashes.show.borrow_mut() = Some(Box::new(move |record| banner_link.upgrade().is_some_and(|app| app.toast(record))));
    crashes.scan();
    let diff_link = Rc::downgrade(&app);
    *diffs.on_diff.borrow_mut() = Some(Box::new(move |views, payload| {
        if let Some(app) = diff_link.upgrade() {
            app.diff(views, payload);
        }
    }));
    let log_link = Rc::downgrade(&app);
    *logs.on_log.borrow_mut() = Some(Box::new(move |views, payload| {
        if let Some(app) = log_link.upgrade() {
            app.log(views, payload);
        }
    }));
    let serial_link = Rc::downgrade(&app);
    *serial.on_serial.borrow_mut() = Some(Box::new(move |views, payload| {
        if let Some(app) = serial_link.upgrade() {
            app.serial(views, payload);
        }
    }));
    let file_link = Rc::downgrade(&app);
    *files.on_file.borrow_mut() = Some(Box::new(move |views, payload| {
        if let Some(app) = file_link.upgrade() {
            app.file("file", views, payload);
        }
    }));
    let threads_link = Rc::downgrade(&app);
    *threads.on_file.borrow_mut() = Some(Box::new(move |views, payload| {
        if let Some(app) = threads_link.upgrade() {
            app.file("markup", views, payload);
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
    let shooting = machines.as_ref().and_then(|m| m.caller());
    core.add_command(
        "screenshot",
        Rc::new(move |args, responder| {
            if let Some(link) = screenshot.upgrade() {
                screenshot::take(&link, shooting.clone(), args, responder);
            }
        }),
    );
    let front = link.clone();
    core.on_front(Rc::new(move |args, responder| match front.upgrade().filter(|app| app.answers_for(&args)) {
        Some(app) => app.forward("app.front", args, responder),
        None => responder.reply(Ok(Map::new())),
    }));
    let resolving = Rc::downgrade(&tasks);
    core.add_command(
        "desk",
        Rc::new(move |mut args, responder| {
            let (Some(app), Some(tasks)) = (link.upgrade(), resolving.upgrade()) else { return };
            let Some(task) = args.get("task").and_then(Value::as_str).map(String::from) else {
                if let Some(folder) = args.get("project").and_then(Value::as_str).filter(|_| args.get("action").and_then(Value::as_str) == Some("desk")) {
                    if let Err(error) = projects::desk_here(folder) {
                        return responder.reply(Err(error));
                    }
                    let folder = folder.to_string();
                    return runtime::spawn("the Desk's place", async move {
                        args["place"] = projects::place_now(&folder).await;
                        app.forward("desk", args, responder);
                    });
                }
                return app.forward("desk", args, responder);
            };
            if task.contains('@') {
                return app.forward("desk", args, responder);
            }
            tasks.agent(&task, move |found| match found {
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
    let mut shutdown: Option<Shutdown> = None;
    let begin = |why: &str, shutdown: &mut Option<Shutdown>| {
        if shutdown.is_some() {
            return;
        }
        log!("kanna-daemon: shutting down: {why}");
        let _ = std::fs::write(state_dir.join(SHUTDOWN_MARKER), format!("{why}\n"));
        *shutdown = Some(Shutdown::App(Instant::now() + APP_QUIT_WAIT));
        crashes.stopping();
        app.quit();
    };
    core.block_on(async { loop {
        tokio::task::yield_now().await;
        let signal = stopping.load(Ordering::SeqCst);
        if signal != 0 {
            log!("kanna-daemon: stopping (signal {signal})");
            break;
        }
        if let Some(Shutdown::App(by)) = shutdown {
            if !app.attached_here() || Instant::now() >= by {
                if app.attached_here() {
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
        let chores = Instant::now();
        app.expire(Instant::now());
        terminals.save_if_changed();
        layout.flush();
        let ticks = if shutdown.is_none() {
            crashes.tick(Instant::now(), app.attached_here());
            tasks.sweep();
            [tasks.tick(), scheduler.tick(), runs.tick()].into_iter().flatten().min()
        } else {
            Some(Instant::now() + Duration::from_millis(100))
        };
        let deadline = [app.next_deadline(), ticks, layout.next_deadline(), crashes.next_deadline()].into_iter().flatten().min();
        perf::since("loop.chores", chores);
        runtime::check("the loop's chores", chores.elapsed());
        let event = match deadline {
            Some(at) => match tokio::time::timeout_at(at.into(), inbox.recv()).await {
                Ok(event) => event,
                Err(_) => continue,
            },
            None => inbox.recv().await,
        };
        let Some(event) = event else { break };
        let (kind, handling) = (event.kind(), Instant::now());
        let asked = match &event {
            Event::Request(request, _) => Some(format!("{} from a client", request_name(request))),
            Event::AppMessage { message, .. } => Some(format!("{} from the app", request_name(message))),
            _ => None,
        };
        match event {
            Event::Request(request, reply) => match request.get("cmd").and_then(Value::as_str) {
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
            },
            Event::Output { terminal, serial } => {
                let Some(noticed) = terminals.noticed(&terminal, serial) else { continue };
                if let (Some(cwd), Some(pane)) = (noticed.cwd, terminals.pane_of(&terminal)) {
                    app.folder(&pane, &cwd);
                }
                for conn in noticed.overflowed {
                    app.resync(conn, &terminal);
                }
            }
            Event::Drained { terminal, serial } => {
                if let Some(code) = terminals.drained(&terminal, serial) {
                    ended(&terminals, &tasks, &app, &terminal, code);
                }
            }
            Event::Status { terminal, serial, status, ended } => {
                if let Some(pane) = terminals.pane_of(&terminal).filter(|_| terminals.current(&terminal, serial)) {
                    core.with_pane_mut(&pane, |p| p.status = status.map(String::from));
                    app.status(&pane, core.pane(&pane).and_then(|p| p.status().map(String::from)).as_deref());
                    tasks.status(&pane, status, ended);
                    runs.status(&pane, ended);
                }
            }
            Event::Reported { terminal, serial, kept } => {
                if terminals.current(&terminal, serial) {
                    reports::save(&db, &[(terminal, Some(kept))]);
                }
            }
            Event::Unreported { terminal } => reports::save(&db, &[(terminal, None)]),
            Event::TooOld { terminal, serial, why } => {
                if let Some(pane) = terminals.pane_of(&terminal).filter(|_| terminals.current(&terminal, serial)) {
                    let refused = tasks.too_old(&pane, &why).or_else(|| runs.too_old(&pane, &why).then(|| "a run's stage".to_string()));
                    if let Some(name) = refused {
                        let toast = toasts::Toast { title: format!("Kanna can't run {name}'s agent"), detail: Some(why), pane: Some(pane.clone()), task: None, key: Some(format!("too-old:{pane}")), from: None };
                        if let Err(error) = toasts.raise(toast) {
                            log!("kanna-daemon: can't raise a toast for {pane}: {}", error.message);
                        }
                    }
                }
            }
            Event::Stacks { seen } => tasks.watched(seen),
            Event::Heads => tasks.heads_moved(),
            Event::Turn { token } => tasks.next_turn(token),
            Event::Merged { task, merge } => tasks.merged(&task, merge),
            Event::Diff { id, diff } => diffs.loaded(&id, diff),
            Event::DiffUpdating { id } => diffs.updating(&id),
            Event::Log { id, log } => logs.loaded(&id, log),
            Event::File { id, read } => {
                files.loaded(&id, read.clone());
                threads.loaded(&id, read);
            }
            Event::Serial(event) => serial.event_in(event),
            Event::Sampled(sampled) => stats.sampled(sampled),
            Event::Covered(covered) => stats.covered(covered),
            Event::KeeperReadable | Event::KeeperHeard => {
                let heard = keeper.take();
                if matches!(event, Event::KeeperReadable) {
                    keeper.resume_watch();
                }
                match heard {
                    Ok(heard) => {
                        for (terminal, code) in terminals.heard(heard) {
                            ended(&terminals, &tasks, &app, &terminal, code);
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
                if app.hello(conn, hello.clone(), out, stream, remote) {
                    break;
                }
                toasts.attached();
                if app.is_here(conn) {
                    crashes.attached(&hello);
                }
            }
            Event::AppMessage { conn, message } if message.get("cmd").and_then(Value::as_str) == Some("app.goodbye") => {
                if app.is_here(conn) {
                    crashes.goodbye();
                }
            }
            Event::AppMessage { conn, message } => {
                if let Some(mut request) = app.message(conn, message) {
                    if request["cmd"] == "replica" {
                        if let Some(args) = request.get_mut("args").and_then(Value::as_object_mut) {
                            args.remove(kanna_net::PEER);
                        }
                    }
                    if let Some(refused) = machines::refuse_remote(&request).filter(|_| !app.is_here(conn)) {
                        app.answer_app(conn, refused);
                        continue;
                    }
                    let link = Rc::downgrade(&app);
                    core.handle(request, move |reply| {
                        if let Some(link) = link.upgrade() {
                            link.answer_app(conn, reply);
                        }
                    });
                }
            }
            Event::AppGone { conn } => {
                let here = app.is_here(conn);
                app.gone(conn);
                if here {
                    crashes.gone();
                }
            }
            Event::AppCaughtUp { conn } => app.caught_up(conn),
            Event::AppBehind { conn, queued } => {
                if app.is_current(conn) {
                    app.fell_behind(conn, queued);
                }
            }
            Event::AppUsed { conn, terminal } => app.used(conn, &terminal),
            Event::MachineConfirm { question, answer } => match &machines {
                Some(machines) => machines.confirm(&app, question, answer),
                None => answer(Err("this daemon has no endpoint".into())),
            },
            Event::MachinesSave(doc) => {
                if let Err(error) = db.put_machines(&doc) {
                    log!("kanna-daemon: can't save your machines in kanna.db: {error}");
                }
                accounts.changed();
            }
            Event::PeerRequest { from, request, reply } => accounts.peer(&from, request, reply),
            Event::AccountBroken(account) => {
                let agent = match account["agent"].as_str() {
                    Some("claude") => "Claude",
                    _ => "Codex",
                };
                let toast = toasts::Toast { title: format!("Sign in to {agent} again"), detail: Some(format!("{}: {}. Add it again with kanna accounts add {}.", account["label"].as_str().unwrap_or(agent), account["why"].as_str().unwrap_or("it stopped working"), account["agent"].as_str().unwrap_or("codex"))), key: Some(format!("account:{}", account["id"].as_str().unwrap_or_default())), ..Default::default() };
                if let Err(error) = toasts.raise(toast) {
                    log!("kanna-daemon: can't raise a toast for a broken account: {}", error.message);
                }
            }
            Event::AccountUnreadable(account) => {
                let agent = match account["agent"].as_str() {
                    Some("claude") => "Claude",
                    _ => "Codex",
                };
                let toast = toasts::Toast { title: format!("{agent} started without your account"), detail: Some(format!("{}. It runs on this machine's own sign-in.", account["why"].as_str().unwrap_or("its secret couldn't be read"))), key: Some(format!("account-unreadable:{}", account["agent"].as_str().unwrap_or_default())), ..Default::default() };
                if let Err(error) = toasts.raise(toast) {
                    log!("kanna-daemon: can't raise a toast for an unreadable account: {}", error.message);
                }
            }
            Event::Replica { token, reply } => tasks.replied(token, reply),
            Event::Quit(_) => continue,
            Event::Shutdown(why) => begin(&why, &mut shutdown),
        }
        perf::since(kind, handling);
        runtime::check(asked.as_deref().unwrap_or(kind), handling.elapsed());
    } });
    core.turn();

    let keep_socket = shutdown.is_none() && app.holds_socket();
    app.close();
    layout.flush();
    layout.save_now();
    terminals.shutdown();
    reports::save(&db, &detector.kept());
    if let Some(machines) = &machines {
        machines.stop();
    }
    let _ = std::fs::remove_file(&pid_file);
    if keep_socket {
        server.stop_leaving_path();
    } else {
        server.stop();
    }
    if shutdown.is_some() {
        let _ = std::fs::File::options().write(true).open(state_dir.join(SHUTDOWN_MARKER)).and_then(|file| file.set_modified(std::time::SystemTime::now()));
    }
    match failure {
        Some(reason) => Err(format!("{reason}; stopping")),
        None => Ok(()),
    }
}

fn request_name(request: &Value) -> String {
    let cmd = request.get("cmd").and_then(Value::as_str).unwrap_or("?");
    match request.get("args").and_then(|args| args.get("action")).and_then(Value::as_str) {
        Some(action) => format!("{cmd} {action}"),
        None => cmd.to_string(),
    }
}

fn ended(terminals: &Terminals, tasks: &Rc<tasks::Tasks>, app: &AppLink, terminal: &str, code: i32) {
    app.exited(terminal, code);
    if let Some(pane) = terminals.pane_of(terminal) {
        tasks.exited(&pane, code);
        let waits = tasks.waits_on_you(&pane);
        app.status(&pane, Some(if waits { "needs-you" } else { "exited" }));
    }
}

fn inherited_listener() -> Result<Option<std::os::unix::net::UnixListener>, String> {
    use std::os::fd::FromRawFd;
    let Some(value) = std::env::var_os(LISTEN_FD) else { return Ok(None) };
    std::env::remove_var(LISTEN_FD);
    let fd: libc::c_int = value.to_str().and_then(|v| v.parse().ok()).filter(|fd| *fd > 2).ok_or_else(|| format!("{LISTEN_FD} isn't a descriptor: {value:?}"))?;
    let mut kind: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let stream = unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_TYPE, (&mut kind as *mut libc::c_int).cast(), &mut size) } == 0 && kind == libc::SOCK_STREAM;
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let local = unsafe { libc::getsockname(fd, (&mut address as *mut libc::sockaddr_storage).cast(), &mut length) } == 0 && address.ss_family as libc::c_int == libc::AF_UNIX;
    if !stream || !local {
        return Err(format!("{LISTEN_FD}={fd} isn't a Unix stream socket"));
    }
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    Ok(Some(unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) }))
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

fn started_before(file: &Path) -> bool {
    let since = std::fs::metadata(file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    matches!((lifeline::age(), since), (Some(age), Some(since)) if age > since)
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

