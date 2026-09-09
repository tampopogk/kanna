//! The startup terminal a launch runs its setup in.
//!
//! Setup used to have nowhere of its own. On a new task it was prepended to
//! the agent's own login shell, so `pnpm install` and the agent's first turn
//! shared one scrollback; on a stage transition it ran in a detached server
//! worker, where its output went nowhere a person could read at all. Both are
//! replaced here by a plain PTY session that the operator can watch: the
//! repo's setup commands run in it, visibly, and the agent is spawned only
//! after it exits cleanly.
//!
//! Splitting the shells splits the environment with them, which is the one
//! thing the old arrangement gave away for free: anything setup exported
//! simply applied to the agent, because it *was* the same shell. A launch
//! therefore ends its setup with the bundled `kanna-cli setup-receipt`
//! helper, which writes that shell's environment and working directory to a
//! private, launch-scoped file. The server reads it, spawns the agent with it,
//! and deletes it. The receipt says setup is ready and what it left behind —
//! it is not a completion signal, and nothing about the task's outcome passes
//! through it.

use super::commands::{build_local_config_override_banner, build_transfer_import_banner};
use crate::daemon_client::DaemonClient;
use crate::db::{Db, NewTaskTerminalSession, ROLE_SETUP};
use crate::mobile_api::TransferImportSummary;
use crate::task_creator::local_config::LocalConfigOverride;
use kanna_daemon::protocol::{Command as DaemonCommand, Event as DaemonEvent};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Matches `workspace_commands`' supervision of the old detached setup run, so
/// moving setup into a terminal does not quietly change how long a wedged
/// startup is allowed to hold a launch.
const SETUP_SOFT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
const SETUP_HARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Environment entries the shell itself owns. The receipt is the setup
/// shell's whole environment, and carrying its bookkeeping into the agent
/// would describe the wrong process.
/// What a wedged startup is reported as. It names the terminal deliberately:
/// the output that explains the hang is there, not in this message.
const TIMED_OUT_STARTUP: &str =
    "workspace startup timed out; see the startup terminal for this stage";

const SHELL_BOOKKEEPING_KEYS: &[&str] = &["_", "SHLVL", "PWD", "OLDPWD", "ZDOTDIR"];

#[derive(Debug, Clone)]
pub(crate) struct SetupTerminalPlan {
    /// The daemon session id. Distinct from the task's, because this is a
    /// different terminal with a different lifetime, and every launch gets a
    /// new one so a stage boundary is a terminal boundary.
    pub(crate) session_id: String,
    pub(crate) record_id: String,
    pub(crate) attempt: i64,
    pub(crate) stage: String,
    pub(crate) title: String,
    pub(crate) cwd: String,
    pub(crate) env: HashMap<String, String>,
    pub(crate) command: String,
    pub(crate) receipt_path: String,
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct SetupReceipt {
    pub(crate) cwd: String,
    pub(crate) env: HashMap<String, String>,
}

#[derive(Debug)]
pub(crate) enum SetupTerminalOutcome {
    /// Setup finished successfully and left this behind for the agent.
    Ready(SetupReceipt),
    /// Setup ran and failed. The terminal holds the output that says why.
    Failed { exit_code: i32, reason: String },
}

/// Where a launch's receipt lives: inside the daemon directory, which is
/// already server-owned and not part of any workspace, under a name that
/// belongs to this launch alone.
fn receipt_path(daemon_dir: &str, session_id: &str) -> PathBuf {
    Path::new(daemon_dir)
        .join("setup-receipts")
        .join(format!("{session_id}.json"))
}

/// Build the startup terminal for one launch.
///
/// `attempt` numbers launches rather than stages: a rerun, a resumed revision,
/// and a stage advance each open their own startup terminal, and none of them
/// may land on an existing one's id.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_setup_terminal(
    daemon_dir: &str,
    task_id: &str,
    stage: &str,
    attempt: i64,
    cwd: &str,
    env: &HashMap<String, String>,
    setup_cmds: &[String],
    transfer_import: Option<&TransferImportSummary>,
    local_config_override: Option<&LocalConfigOverride>,
    geometry: Option<(u16, u16)>,
) -> SetupTerminalPlan {
    let session_id = setup_session_id(task_id, attempt);
    let receipt = receipt_path(daemon_dir, &session_id);
    let receipt_path = receipt.to_string_lossy().into_owned();
    let command = build_setup_shell_command(
        setup_cmds,
        transfer_import,
        local_config_override,
        // A path that names no executable is the same as having no helper:
        // running it would end the startup shell with "command not found"
        // and fail a launch whose setup was fine.
        env.get("KANNA_CLI_PATH")
            .map(String::as_str)
            .filter(|path| is_executable(path)),
        env.get("PATH").map(String::as_str),
        &receipt_path,
    );
    let (cols, rows) = geometry.unwrap_or((80, 24));
    SetupTerminalPlan {
        record_id: format!("setup-{task_id}-{attempt}"),
        session_id,
        attempt,
        stage: stage.to_string(),
        title: format!("Startup · {stage}"),
        cwd: cwd.to_string(),
        env: env.clone(),
        command,
        receipt_path,
        cols,
        rows,
    }
}

pub(crate) fn setup_session_id(task_id: &str, attempt: i64) -> String {
    format!("setup-{task_id}-{attempt}")
}

/// Write the terminal down before it is spawned, so a shell that prints
/// immediately already has a row the desktop can open a tab against.
pub(crate) fn record_setup_terminal(
    db: &Db,
    repo_id: &str,
    task_id: &str,
    stage_run_id: Option<&str>,
    plan: &SetupTerminalPlan,
) -> Result<(), String> {
    db.upsert_task_terminal_session(NewTaskTerminalSession {
        id: &plan.record_id,
        repo_id,
        task_id: Some(task_id),
        daemon_session_id: Some(&plan.session_id),
        role: ROLE_SETUP,
        stage: Some(&plan.stage),
        attempt: plan.attempt,
        stage_run_id,
        title: Some(&plan.title),
        cwd: Some(&plan.cwd),
    })
    .map_err(|error| format!("db error: {error}"))
}

/// Whether this path names something the startup shell can actually run.
fn is_executable(path: &str) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        std::path::Path::new(path).is_file()
    }
}

fn shell_single_quote(value: &str) -> String {
    value.replace('\'', "'\\''")
}

/// The startup shell.
///
/// It runs the same ordered commands, with the same `$ cmd` echo and the same
/// `hash -r`, that the agent's bootstrap shell used to run — a launch's startup
/// output should not change shape because it moved terminals. What is new is
/// the tail: on success it writes the receipt and says the agent is starting;
/// on failure it says so, in the terminal holding the output that explains it,
/// and exits with the failing status so the server never spawns an agent into
/// a half-provisioned workspace.
fn build_setup_shell_command(
    setup_cmds: &[String],
    transfer_import: Option<&TransferImportSummary>,
    local_config_override: Option<&LocalConfigOverride>,
    kanna_cli_path: Option<&str>,
    spawn_path: Option<&str>,
    receipt_path: &str,
) -> String {
    let mut preamble = Vec::new();
    if let Some(kanna_cli_path) = kanna_cli_path {
        preamble.push(format!(
            "export KANNA_CLI_PATH='{}'",
            shell_single_quote(kanna_cli_path)
        ));
    }
    if let Some(spawn_path) = spawn_path.filter(|path| !path.is_empty()) {
        preamble.push(format!("export PATH='{}'", shell_single_quote(spawn_path)));
    } else if let Some(kanna_cli_path) = kanna_cli_path {
        if let Some(parent) = Path::new(kanna_cli_path).parent() {
            let parent = shell_single_quote(parent.to_string_lossy().as_ref());
            preamble.push(format!("export PATH='{}':\"$PATH\"", parent));
        }
    }
    // The import notice comes before setup: it explains where the workspace
    // the setup commands are about to run in came from.
    if let Some(transfer_import) = transfer_import {
        preamble.push(build_transfer_import_banner(transfer_import));
    }
    // Likewise before setup: the setup commands themselves are one of the
    // things the local layer can have replaced.
    if let Some(local_config_override) = local_config_override {
        preamble.push(build_local_config_override_banner(local_config_override));
    }

    let mut chain = Vec::new();
    if !setup_cmds.is_empty() {
        chain.push("printf '\\033[33mRunning startup...\\033[0m\\n'".to_string());
        for cmd in setup_cmds {
            chain.push(format!(
                "printf '\\033[2m$ %s\\033[0m\\n' '{}'",
                shell_single_quote(cmd)
            ));
            chain.push(cmd.clone());
        }
        // A shell may cache a provider found earlier on PATH before setup
        // installs a workspace-local executable. Refresh its command table so
        // the just-provisioned binary is what the receipt's PATH resolves to.
        // `hash -r` is POSIX and works in bash, dash and zsh alike; zsh's own
        // `rehash` does not exist in the other two.
        chain.push("hash -r".to_string());
    }
    let receipt_writer = match kanna_cli_path {
        Some(kanna_cli_path) => format!(
            "'{}' setup-receipt --output '{}'",
            shell_single_quote(kanna_cli_path),
            shell_single_quote(receipt_path)
        ),
        // Without the bundled helper there is nothing to carry the shell's
        // exports across. The launch still proceeds — setup ran, and its
        // output is right there — but the agent starts with the environment
        // the server prepared rather than the one setup left, and the terminal
        // says so rather than letting that difference go unmentioned.
        None => format!(
            "printf '\\033[31mKanna could not locate kanna-cli; anything startup exported will \
             not carry into the agent.\\033[0m\\n' && printf '%s' \
             '{{\"version\":1,\"cwd\":\"\",\"env\":{{}}}}' > '{}'",
            shell_single_quote(receipt_path)
        ),
    };
    chain.push(receipt_writer);

    // The failure notice hangs off the shell's exit rather than off an `else`
    // branch, because a setup command is free to call `exit` itself — and that
    // ends the shell outright, skipping any branch a conditional would have
    // taken. The trap catches every way this shell can stop failing, including
    // that one, so the terminal always says why nothing followed it.
    let mut lines = vec![
        "kanna_startup_failed() { printf '\\033[31mStartup failed (exit %s). The agent was not \
         started.\\033[0m\\n' \"$1\" ; }"
            .to_string(),
        "trap 'kanna_startup_status=$?; [ $kanna_startup_status -eq 0 ] || \
         kanna_startup_failed $kanna_startup_status' EXIT"
            .to_string(),
    ];
    lines.extend(preamble);
    lines.push(format!(
        "{} && printf '\\033[32mStartup complete; starting the agent.\\033[0m\\n'",
        chain.join(" && ")
    ));
    lines.join(" ; ")
}

/// A startup terminal the daemon has acknowledged and is now running.
///
/// Starting and waiting are separate because they answer different callers. A
/// create request has to know *now* whether the terminal started — a daemon
/// that refused the spawn is a failure the caller can retry — but must not be
/// held open for setup itself, which is repo work of unbounded length. So the
/// start is awaited and the wait is handed to a background task.
pub(crate) struct StartedSetupTerminal {
    events: DaemonClient,
}

/// Spawn the startup terminal and wait for it to finish.
///
/// `armed_timeout` fires the hard timeout on an explicit signal instead of the
/// wall clock, and is `None` everywhere but a test. A test that proves timeout
/// handling must first get the startup terminal into the state under test —
/// output produced, descendants spawned — and a fixed budget cannot express
/// that ordering, so the caller observes that state itself and arms the flag.
pub(crate) async fn run_setup_terminal(
    daemon_dir: &str,
    plan: &SetupTerminalPlan,
    armed_timeout: Option<&std::sync::atomic::AtomicBool>,
) -> Result<SetupTerminalOutcome, String> {
    let started = start_setup_terminal(daemon_dir, plan).await?;
    wait_setup_terminal(started, daemon_dir, plan, armed_timeout).await
}

/// Start the startup terminal.
///
/// The event subscription is established *before* the spawn, so an exit can
/// never land in the gap between the two and leave a launch waiting on a
/// terminal that already finished.
pub(crate) async fn start_setup_terminal(
    daemon_dir: &str,
    plan: &SetupTerminalPlan,
) -> Result<StartedSetupTerminal, String> {
    // The receipt lands in a server-owned directory the startup shell does not
    // create for itself; making it here keeps that out of the shell and out of
    // the workspace. A receipt left by an earlier attempt at this id would be
    // read as this one's, so it is cleared before anything can write.
    if let Some(parent) = std::path::Path::new(&plan.receipt_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create the startup receipt directory: {error}"))?;
    }
    let _ = std::fs::remove_file(&plan.receipt_path);

    let mut events = DaemonClient::connect(daemon_dir)
        .await
        .map_err(|error| format!("daemon connection failed: {error}"))?;
    match events
        .send_command(&DaemonCommand::Subscribe)
        .await
        .map_err(|error| format!("daemon subscribe failed: {error}"))?
    {
        DaemonEvent::Ok => {}
        DaemonEvent::Error { message, .. } => {
            return Err(format!("daemon subscribe error: {message}"))
        }
        other => return Err(format!("unexpected daemon subscribe response: {other:?}")),
    }

    let mut control = DaemonClient::connect(daemon_dir)
        .await
        .map_err(|error| format!("daemon connection failed: {error}"))?;
    // The startup shell is the user's login shell, like every other task shell:
    // what setup can rely on being on PATH is whatever that shell's profiles
    // put there, and hard-coding zsh would have made a Linux workspace run its
    // setup in a shell the machine does not use.
    let shell = crate::login_shell::login_shell();
    let spawn = DaemonCommand::Spawn {
        session_id: plan.session_id.clone(),
        executable: shell.path().to_string(),
        args: shell.login_interactive_args(&plan.command),
        cwd: plan.cwd.clone(),
        env: plan.env.clone(),
        cols: plan.cols,
        rows: plan.rows,
        // A startup shell is not a provider session. Spawning it without one
        // is what keeps its output out of prompt and composer detection:
        // `Classifier::with_version(None, _)` resolves no rules, so a setup
        // script that prints something shaped like CLI chrome cannot be read
        // as an agent waiting for an answer.
        agent_provider: None,
        agent_executable: None,
        terminal_prelude: None,
        operator_input_only: false,
    };
    match control
        .send_command_retrying_successor(&spawn)
        .await
        .map_err(|error| format!("daemon spawn failed: {error}"))?
    {
        DaemonEvent::SessionCreated { .. } => {}
        DaemonEvent::Error { message, .. } => {
            return Err(format!("startup terminal failed to start: {message}"))
        }
        other => {
            return Err(format!(
                "unexpected daemon response starting the startup terminal: {other:?}"
            ))
        }
    }
    Ok(StartedSetupTerminal { events })
}

/// Wait for a started startup terminal to finish, and read what it left.
pub(crate) async fn wait_setup_terminal(
    started: StartedSetupTerminal,
    daemon_dir: &str,
    plan: &SetupTerminalPlan,
    armed_timeout: Option<&std::sync::atomic::AtomicBool>,
) -> Result<SetupTerminalOutcome, String> {
    let mut events = started.events;
    let exit_code =
        await_setup_exit(&mut events, daemon_dir, &plan.session_id, armed_timeout).await?;
    Ok(read_setup_outcome(plan, exit_code))
}

async fn await_setup_exit(
    events: &mut DaemonClient,
    daemon_dir: &str,
    session_id: &str,
    armed_timeout: Option<&std::sync::atomic::AtomicBool>,
) -> Result<i32, String> {
    let started = std::time::Instant::now();
    let mut warned = false;
    loop {
        let remaining = SETUP_HARD_TIMEOUT.saturating_sub(started.elapsed());
        let mut slice = if warned {
            remaining
        } else {
            SETUP_SOFT_TIMEOUT.saturating_sub(started.elapsed())
        };
        if let Some(armed) = armed_timeout {
            if armed.load(std::sync::atomic::Ordering::SeqCst) {
                kill_setup_session(daemon_dir, session_id).await;
                return Err(TIMED_OUT_STARTUP.to_string());
            }
            slice = slice.min(std::time::Duration::from_millis(25));
            if slice.is_zero() {
                slice = std::time::Duration::from_millis(25);
            }
            match tokio::time::timeout(slice, events.read_event()).await {
                Ok(Ok(DaemonEvent::Exit {
                    session_id: exited,
                    code,
                    ..
                })) if exited == session_id => return Ok(code),
                Ok(Ok(DaemonEvent::ShuttingDown)) => {
                    return Err("daemon shut down while startup was running".to_string())
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => return Err(format!("daemon read failed: {error}")),
                Err(_) => {}
            }
            continue;
        }
        if slice.is_zero() {
            if !warned {
                warned = true;
                log::warn!(
                    "startup terminal {session_id} exceeded its soft threshold of {}s",
                    SETUP_SOFT_TIMEOUT.as_secs()
                );
                continue;
            }
            log::error!(
                "startup terminal {session_id} timed out after {}s; killing it",
                SETUP_HARD_TIMEOUT.as_secs()
            );
            kill_setup_session(daemon_dir, session_id).await;
            return Err(format!(
                "{TIMED_OUT_STARTUP} after {}s",
                SETUP_HARD_TIMEOUT.as_secs()
            ));
        }
        let event = match tokio::time::timeout(slice, events.read_event()).await {
            Ok(Ok(event)) => event,
            Ok(Err(error)) => return Err(format!("daemon read failed: {error}")),
            Err(_) => continue,
        };
        match event {
            DaemonEvent::Exit {
                session_id: exited,
                code,
                ..
            } if exited == session_id => return Ok(code),
            DaemonEvent::ShuttingDown => {
                return Err("daemon shut down while startup was running".to_string())
            }
            _ => {}
        }
    }
}

async fn kill_setup_session(daemon_dir: &str, session_id: &str) {
    let Ok(mut daemon) = DaemonClient::connect(daemon_dir).await else {
        log::warn!("could not reconnect to kill the timed-out startup terminal {session_id}");
        return;
    };
    if let Err(error) = daemon
        .send_command(&DaemonCommand::Kill {
            session_id: session_id.to_string(),
        })
        .await
    {
        log::warn!("failed to kill the timed-out startup terminal {session_id}: {error}");
    }
}

fn read_setup_outcome(plan: &SetupTerminalPlan, exit_code: i32) -> SetupTerminalOutcome {
    if exit_code != 0 {
        let _ = std::fs::remove_file(&plan.receipt_path);
        return SetupTerminalOutcome::Failed {
            exit_code,
            reason: format!(
                "workspace setup failed (exit {exit_code}); see the startup terminal for this stage"
            ),
        };
    }
    match read_receipt(&plan.receipt_path) {
        Ok(receipt) => {
            let _ = std::fs::remove_file(&plan.receipt_path);
            SetupTerminalOutcome::Ready(receipt)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&plan.receipt_path);
            SetupTerminalOutcome::Failed {
                exit_code,
                // Setup exited 0 but left nothing behind: the shell's exports
                // cannot be carried, and starting the agent without them would
                // silently drop whatever setup provisioned.
                reason: format!("workspace setup left no startup receipt: {error}"),
            }
        }
    }
}

/// Read a receipt a startup terminal left behind, for a launch this process
/// did not run itself. Reconciliation on the next boot has only the path.
pub(crate) fn read_setup_receipt(path: &str) -> Result<SetupReceipt, String> {
    let receipt = read_receipt(path)?;
    let _ = std::fs::remove_file(path);
    Ok(receipt)
}

fn read_receipt(path: &str) -> Result<SetupReceipt, String> {
    let raw = std::fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    if raw.is_empty() {
        return Err("the receipt was empty".to_string());
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RawReceipt {
        cwd: String,
        env: HashMap<String, String>,
    }
    let parsed: RawReceipt =
        serde_json::from_slice(&raw).map_err(|error| format!("cannot parse {path}: {error}"))?;
    Ok(SetupReceipt {
        cwd: parsed.cwd,
        env: parsed.env,
    })
}

/// Carry the startup shell's exports into the agent's spawn environment.
///
/// This is the deliberate replacement for the inheritance the single-shell
/// arrangement gave away: setup's exports won there because they *were* the
/// agent's environment, so they win here too. Only the shell's own
/// bookkeeping is dropped, because it describes a process that has exited.
pub(crate) fn apply_setup_receipt(env: &mut HashMap<String, String>, receipt: &SetupReceipt) {
    for (key, value) in &receipt.env {
        if SHELL_BOOKKEEPING_KEYS.contains(&key.as_str()) {
            continue;
        }
        env.insert(key.clone(), value.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_for(setup: &[String], cli: Option<&str>) -> String {
        build_setup_shell_command(
            setup,
            None,
            None,
            cli,
            Some("/usr/bin"),
            "/tmp/receipt.json",
        )
    }

    #[test]
    fn startup_runs_the_commands_in_order_and_then_writes_the_receipt() {
        let command = plan_for(
            &["pnpm install".to_string(), "make build".to_string()],
            Some("/bin/echo"),
        );
        let install = command.find("pnpm install").expect("setup command");
        let build = command.find("make build").expect("setup command");
        let receipt = command.find("setup-receipt").expect("receipt writer");
        assert!(install < build, "setup commands keep their order");
        assert!(build < receipt, "the receipt is written after setup");
        assert!(command.contains("hash -r"));
    }

    #[test]
    fn a_failing_setup_says_the_agent_did_not_start() {
        let command = plan_for(&["false".to_string()], Some("/bin/echo"));
        assert!(command.contains("The agent was not started"));
        // Hung off the shell's exit rather than a branch, because a setup
        // command that calls `exit` itself ends the shell outright and would
        // skip any conditional.
        assert!(command.contains("trap 'kanna_startup_status=$?"));
        assert!(command.contains("EXIT"));
    }

    #[test]
    fn an_empty_setup_still_produces_a_terminal_that_writes_a_receipt() {
        let command = plan_for(&[], Some("/bin/echo"));
        assert!(!command.contains("Running startup..."));
        assert!(command.contains("setup-receipt"));
    }

    #[test]
    fn the_receipt_carries_setup_exports_but_not_the_shell_s_own_bookkeeping() {
        let mut env = HashMap::from([
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("SHLVL".to_string(), "1".to_string()),
        ]);
        let receipt = SetupReceipt {
            cwd: "/work".to_string(),
            env: HashMap::from([
                (
                    "PATH".to_string(),
                    "/work/node_modules/.bin:/usr/bin".to_string(),
                ),
                ("SHLVL".to_string(), "9".to_string()),
                ("TOOLCHAIN".to_string(), "1.2.3".to_string()),
            ]),
        };
        apply_setup_receipt(&mut env, &receipt);
        assert_eq!(env["PATH"], "/work/node_modules/.bin:/usr/bin");
        assert_eq!(env["TOOLCHAIN"], "1.2.3");
        assert_eq!(
            env["SHLVL"], "1",
            "the setup shell's depth is not the agent's"
        );
    }

    #[test]
    fn every_launch_gets_its_own_startup_terminal() {
        assert_ne!(setup_session_id("t1", 1), setup_session_id("t1", 2));
        assert!(kanna_runtime_defaults::session_id::is_safe(
            &setup_session_id("0b11968b-e2b1-4c00-9a52-2c2f0f4e1f77", 3)
        ));
    }
}
