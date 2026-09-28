use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use agent_tui::app::App;
use agent_tui::harness::{make_adapter, HarnessOptions};
use agent_tui::host::Host;
use agent_tui::launch::HostedLaunch;
use agent_tui::process::{HarnessProcess, ProcEvent};
use agent_tui::protocol::HarnessKind;
use agent_tui::ui::skins::{ColorMode, SkinId};
use agent_tui::ui::{self, RenderCache};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum HarnessArg {
    Claude,
    Codex,
}

/// Interactive terminal client for Claude Code or Codex.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Which harness to drive.
    harness: HarnessArg,
    /// Model to request (reported model is shown once the harness confirms it).
    #[arg(long)]
    model: Option<String>,
    /// Reasoning effort to request.
    #[arg(long)]
    effort: Option<String>,
    /// Working directory for the session.
    #[arg(long)]
    cwd: Option<PathBuf>,
    /// Skin: graphite, matrix, dracula, nord, solarized, duke.
    #[arg(long, default_value = "graphite")]
    skin: String,
    /// Path to the harness executable (defaults to `claude` / `codex` on PATH).
    #[arg(long)]
    bin: Option<String>,
    /// Run inside Kanna using its authenticated hosting protocol.
    #[arg(long)]
    kanna: bool,
    /// Extra arguments passed to the harness after `--`.
    #[arg(last = true)]
    harness_args: Vec<String>,
}

type Term = Terminal<CrosstermBackend<Stdout>>;

fn log_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            if cfg!(target_os = "macos") {
                home.join("Library/Caches")
            } else {
                home.join(".cache")
            }
        });
    base.join("agent-tui").join("agent-tui.log")
}

fn init_logging() {
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let filter = std::env::var("AGENT_TUI_LOG").unwrap_or_else(|_| "info".into());
        let _ = tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .try_init();
    }
}

fn setup_terminal() -> Result<(Term, bool)> {
    terminal::enable_raw_mode().context("enable raw mode")?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    let enhanced = matches!(terminal::supports_keyboard_enhancement(), Ok(true));
    if enhanced {
        execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    let term = Terminal::new(CrosstermBackend::new(out))?;
    Ok((term, enhanced))
}

fn restore_terminal(enhanced: bool) {
    let mut out = io::stdout();
    if enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = terminal::disable_raw_mode();
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging();
    let skin = SkinId::parse(&cli.skin).with_context(|| format!("unknown skin {:?}", cli.skin))?;
    let cwd = match &cli.cwd {
        Some(p) => std::fs::canonicalize(p).with_context(|| format!("--cwd {}", p.display()))?,
        None => std::env::current_dir()?,
    };
    let opts = HarnessOptions {
        kind: match cli.harness {
            HarnessArg::Claude => HarnessKind::Claude,
            HarnessArg::Codex => HarnessKind::Codex,
        },
        model: cli.model.clone(),
        effort: cli.effort.clone(),
        cwd: Some(cwd.to_string_lossy().into_owned()),
        program: cli.bin.clone(),
        extra_args: cli.harness_args.clone(),
    };

    let hosted_launch = if cli.kanna {
        if cli.model.is_some() || cli.effort.is_some() {
            anyhow::bail!("in --kanna mode pass native model/effort flags after --");
        }
        let program = cli
            .bin
            .clone()
            .context("--kanna requires --bin with the real provider executable")?;
        Some(
            HostedLaunch::parse(
                opts.kind,
                program,
                cwd.to_string_lossy().into_owned(),
                &cli.harness_args,
            )
            .map_err(anyhow::Error::msg)?,
        )
    } else {
        None
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let (mut term, enhanced) = setup_terminal()?;
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(enhanced);
        default_hook(info);
    }));
    let result = rt.block_on(run(&mut term, enhanced, opts, skin, cwd, hosted_launch));
    restore_terminal(enhanced);
    // Background reader tasks may still hold pipes; don't wait on them.
    rt.shutdown_timeout(Duration::from_millis(200));
    result
}

async fn run(
    term: &mut Term,
    enhanced: bool,
    opts: HarnessOptions,
    skin: SkinId,
    cwd: PathBuf,
    hosted_launch: Option<HostedLaunch>,
) -> Result<()> {
    // Installed before the harness starts so no signal can skip its shutdown.
    let mut signals = shutdown_signals()?;
    let mode = ColorMode::detect();
    let mut host = match &hosted_launch {
        Some(launch) => {
            let path = std::env::var_os(kanna_agent_protocol::hosted_frontend::CONFIG_ENV)
                .context("Kanna host configuration is missing; launch through Kanna")?;
            Some(Host::open(
                std::path::Path::new(&path),
                launch.initial_prompt.clone(),
            )?)
        }
        None => None,
    };
    let adapter = hosted_launch
        .as_ref()
        .map_or_else(|| make_adapter(&opts), HostedLaunch::adapter);
    let mut app = App::new(adapter, skin);
    app.hosted = host.is_some();
    if let Some(launch) = &hosted_launch {
        if let agent_tui::launch::HostedConfig::Claude(config) = &launch.config {
            if config.extra_args.iter().any(|arg| arg == "--resume") {
                if let Some(id) = &config.expected_session_id {
                    let home = std::env::var_os("CLAUDE_CONFIG_DIR")
                        .map(PathBuf::from)
                        .or_else(|| {
                            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".claude"))
                        });
                    if let Some(home) = home {
                        match agent_tui::history::claude(&home, &cwd.to_string_lossy(), id) {
                            Ok(messages) => app.load_history(messages),
                            Err(error) => {
                                tracing::warn!("previous Claude history unavailable: {error}")
                            }
                        }
                    }
                }
            }
        }
    }

    app.shift_enter = enhanced;
    app.cwd_label = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut cache = RenderCache::default();

    let (ptx, mut prx) = mpsc::unbounded_channel::<(u64, ProcEvent)>();
    let mut generation = 1u64;
    let cwd_str = opts.cwd.clone();
    let mut proc = spawn(&mut app, &opts, cwd_str.as_deref(), generation, &ptx);

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = true;
    let mut last_second = 0u64;
    let mut outcome = Ok(());

    loop {
        if let Some(host) = host.as_mut() {
            if let Err(error) = host.advance(&mut app) {
                app.take_outbox();
                outcome = Err(error.context("persisting Kanna input state; dispatch stopped"));
                break;
            }
            dirty = true;
        }
        flush(&mut app, proc.as_ref());
        if dirty {
            app.tick(Instant::now());
            if let Err(e) = term.draw(|f| ui::draw(f, &mut app, &mut cache, mode)) {
                // The terminal is gone (e.g. its window closed): still shut the
                // harness down properly below.
                tracing::warn!("draw failed: {e}");
                app.stop_and_quit();
                break;
            }
            dirty = false;
        }
        tokio::select! {
            incoming = async {
                match host.as_mut() {
                    Some(host) => host.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let (Some(host), Some(incoming)) = (host.as_mut(), incoming) {
                    if let Err(error) = host.handle(incoming) {
                        outcome = Err(error);
                        break;
                    }
                }
                dirty = true;
            }
            ev = events.next() => {
                match ev {
                    Some(Ok(Event::Key(k))) => app.on_key(k),
                    Some(Ok(Event::Paste(s))) => app.on_paste(&s),
                    Some(Ok(Event::Resize(_, _))) => {}
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => tracing::warn!("terminal event error: {e}"),
                    None => break,
                }
                dirty = true;
            }
            Some((gen, pe)) = prx.recv() => {
                handle_proc(&mut app, proc.as_ref(), generation, gen, pe);
                // Drain a burst of records before drawing again.
                let mut n = 0;
                while n < 512 {
                    match prx.try_recv() {
                        Ok((gen, pe)) => handle_proc(&mut app, proc.as_ref(), generation, gen, pe),
                        Err(_) => break,
                    }
                    n += 1;
                }
                dirty = true;
            }
            Some(sig) = signals.recv() => {
                tracing::info!("received {sig}; stopping and quitting");
                app.stop_and_quit();
            }
            _ = tick.tick() => {
                app.tick(Instant::now());
                if let Some(e) = app.elapsed() {
                    if e.as_secs() != last_second {
                        last_second = e.as_secs();
                        dirty = true;
                    }
                }
            }
        }
        // Hosted writes are flushed at the top of the loop, after the
        // correlated receipt and submitting boundary have been persisted.
        if host.is_none() {
            flush(&mut app, proc.as_ref());
        }

        if app.restart_requested {
            if let Some(mut p) = proc.take() {
                p.shutdown(Duration::from_secs(3)).await;
            }
            generation += 1;
            app.reset_session(make_adapter(&opts));
            proc = spawn(&mut app, &opts, cwd_str.as_deref(), generation, &ptx);
            flush(&mut app, proc.as_ref());
            dirty = true;
        }
        if app.should_quit {
            break;
        }
    }
    if let Some(host) = host.as_mut() {
        host.snapshot.retired = true;
        if let Err(error) = host
            .advance(&mut app)
            .and_then(|_| host.retire("frontend exited"))
        {
            app.take_outbox();
            outcome = Err(error);
        }
    }
    if app.phase == agent_tui::app::Phase::Working {
        app.stop_turn();
    }
    flush(&mut app, proc.as_ref());
    if let Some(mut p) = proc.take() {
        p.shutdown(Duration::from_secs(3)).await;
    }
    outcome
}

/// SIGHUP (terminal closed), SIGTERM and SIGINT all take the Stop & quit
/// path, so the harness's process group is shut down and the terminal restored.
fn shutdown_signals() -> Result<mpsc::UnboundedReceiver<&'static str>> {
    use tokio::signal::unix::{signal, SignalKind};
    let (tx, rx) = mpsc::unbounded_channel();
    for (kind, name) in [
        (SignalKind::hangup(), "SIGHUP"),
        (SignalKind::terminate(), "SIGTERM"),
        (SignalKind::interrupt(), "SIGINT"),
    ] {
        let mut s = signal(kind).with_context(|| format!("install {name} handler"))?;
        let tx = tx.clone();
        tokio::spawn(async move {
            while s.recv().await.is_some() {
                if tx.send(name).is_err() {
                    break;
                }
            }
        });
    }
    Ok(rx)
}

fn spawn(
    app: &mut App,
    opts: &HarnessOptions,
    cwd: Option<&str>,
    generation: u64,
    ptx: &mpsc::UnboundedSender<(u64, ProcEvent)>,
) -> Option<HarnessProcess> {
    let spec = app.adapter.spawn_spec();
    match HarnessProcess::spawn(&spec, cwd, generation, ptx.clone()) {
        Ok(p) => {
            app.start();
            Some(p)
        }
        Err(e) => {
            let _ = opts;
            app.on_spawn_error(&spec.program, &e.to_string());
            None
        }
    }
}

fn handle_proc(
    app: &mut App,
    proc: Option<&HarnessProcess>,
    current: u64,
    gen: u64,
    pe: ProcEvent,
) {
    if gen != current {
        return;
    }
    let tail = proc.map(|p| p.stderr_tail()).unwrap_or_default();
    let who = app.harness.label();
    match pe {
        ProcEvent::Record(r) => app.on_record(r),
        ProcEvent::Eof => app.on_disconnect(&format!("{who} closed its output"), &tail),
        ProcEvent::Exited(info) => app.on_disconnect(&format!("{who} exited ({info})"), &tail),
    }
}

fn flush(app: &mut App, proc: Option<&HarnessProcess>) {
    let out = app.take_outbox();
    if out.is_empty() {
        return;
    }
    let who = app.harness.label();
    match proc {
        Some(p) => {
            for line in out {
                if !p.send(line) {
                    let tail = p.stderr_tail();
                    app.on_disconnect(&format!("could not write to {who}"), &tail);
                    break;
                }
            }
        }
        None => app.on_disconnect(&format!("{who} is not running"), &[]),
    }
}
