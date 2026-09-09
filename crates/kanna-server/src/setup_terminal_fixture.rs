//! A fake daemon that actually runs the startup terminals it is asked to spawn.
//!
//! A launch's setup runs in a PTY session of its own now, so the server no
//! longer runs it: it asks the daemon to, and waits for that session's `Exit`.
//! A fixture that only acknowledged the spawn would prove nothing about the
//! thing under test — that setup really ran, in the workspace, and that what it
//! exported reached the agent — so this one executes the command, puts it in a
//! process group of its own, reports the status it actually finished with, and
//! kills that whole group on `Kill`, which is what a timed-out startup relies
//! on. Agent spawns are recorded rather than executed, and answered the way the
//! daemon answers them.
//!
//! It is not a PTY: no terminal behaviour is claimed here, only the launch
//! handshake around one.

use kanna_daemon::protocol::{Command as DaemonCommand, Event as DaemonEvent};
use std::collections::HashMap;
use std::os::unix::process::CommandExt;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

pub(crate) struct SetupTerminalDaemon {
    pub(crate) task: tokio::task::JoinHandle<()>,
    /// What the launch did to the task's *own* session, in order: the kills and
    /// the agent spawns. A launch's startup terminals are run rather than
    /// recorded — they are the thing this fixture exists to execute.
    commands: Arc<Mutex<Vec<DaemonCommand>>>,
}

impl SetupTerminalDaemon {
    pub(crate) fn commands(&self) -> Vec<DaemonCommand> {
        self.commands.lock().unwrap().clone()
    }

    /// Just the agent spawns and the seeds that precede them.
    pub(crate) fn spawns(&self) -> Vec<DaemonCommand> {
        self.commands()
            .into_iter()
            .filter(|command| {
                matches!(
                    command,
                    DaemonCommand::Spawn { .. } | DaemonCommand::SpawnAgent { .. }
                )
            })
            .collect()
    }

    pub(crate) fn abort(&self) {
        self.task.abort();
    }
}

/// Whether a session id names a launch's startup terminal rather than its
/// agent. The fixture runs the first and records the second.
fn is_startup_session(session_id: &str) -> bool {
    session_id.starts_with("setup-")
}

pub(crate) async fn spawn_setup_terminal_daemon(daemon_dir: &str) -> SetupTerminalDaemon {
    let socket_path = kanna_runtime_defaults::socket_path(std::path::Path::new(daemon_dir));
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).unwrap();
    let (exit_tx, _) = tokio::sync::broadcast::channel::<String>(16);
    // Session id -> process group, so `Kill` can reach a setup script's own
    // children the way the real daemon's reaper does.
    let groups: Arc<Mutex<HashMap<String, i32>>> = Arc::default();
    let commands: Arc<Mutex<Vec<DaemonCommand>>> = Arc::default();
    let recorded = Arc::clone(&commands);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let exit_tx = exit_tx.clone();
            let groups = Arc::clone(&groups);
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);
                let mut exits = exit_tx.subscribe();
                // Only a subscribed connection is fed events. Pushing one down
                // the connection that issued a command would be read as that
                // command's reply, which is the same reason the server keeps
                // its control socket unsubscribed.
                let mut subscribed = false;
                loop {
                    let line = tokio::select! {
                        forwarded = exits.recv(), if subscribed => {
                            let Ok(event) = forwarded else { continue };
                            if write_half.write_all(format!("{event}\n").as_bytes()).await.is_err() {
                                return;
                            }
                            continue;
                        }
                        line = read_line(&mut reader) => match line {
                            Some(line) => line,
                            None => return,
                        },
                    };
                    let Ok(command) = serde_json::from_str::<DaemonCommand>(line.trim()) else {
                        return;
                    };
                    let response = answer(&command, &exit_tx, &groups, &recorded);
                    if matches!(command, DaemonCommand::Subscribe) {
                        subscribed = true;
                    }
                    let rendered = serde_json::to_string(&response).unwrap();
                    if write_half
                        .write_all(format!("{rendered}\n").as_bytes())
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    SetupTerminalDaemon { task, commands }
}

fn answer(
    command: &DaemonCommand,
    exit_tx: &tokio::sync::broadcast::Sender<String>,
    groups: &Arc<Mutex<HashMap<String, i32>>>,
    recorded: &Arc<Mutex<Vec<DaemonCommand>>>,
) -> DaemonEvent {
    match command {
        // The client negotiates before it spawns; answering these is what
        // makes this fixture reachable at all.
        DaemonCommand::NegotiateProtectedInput { .. } => DaemonEvent::ProtectedInputReady {
            version: kanna_daemon::protocol::PROTECTED_INPUT_PROTOCOL_VERSION,
        },
        DaemonCommand::NegotiateRawInput { .. } => DaemonEvent::RawInputReady {
            version: kanna_daemon::protocol::RAW_INPUT_PROTOCOL_VERSION,
        },
        DaemonCommand::NegotiateTerminalGeometry { .. } => DaemonEvent::TerminalGeometryReady {
            version: kanna_daemon::protocol::TERMINAL_GEOMETRY_PROTOCOL_VERSION,
        },
        DaemonCommand::Subscribe => DaemonEvent::Ok,
        // A stage swap kills the outgoing session, which this fixture never
        // created; the daemon's own answer for that is session-not-found.
        // A recovery seed precedes the agent spawn it belongs to; recording it
        // keeps the launch's sequence readable.
        DaemonCommand::SeedSnapshot { .. } => {
            recorded.lock().unwrap().push(command.clone());
            DaemonEvent::Ok
        }
        DaemonCommand::Snapshot { session_id } => DaemonEvent::Error {
            code: Some(kanna_daemon::protocol::ErrorCode::SessionNotFound),
            message: format!("session not found: {session_id}"),
        },
        DaemonCommand::Kill { session_id } if !groups.lock().unwrap().contains_key(session_id) => {
            // A stage swap or rerun kills the outgoing agent session, which
            // this fixture never created; the daemon's own answer for that is
            // session-not-found, and the launch is expected to tolerate it.
            recorded.lock().unwrap().push(command.clone());
            DaemonEvent::Error {
                code: Some(kanna_daemon::protocol::ErrorCode::SessionNotFound),
                message: format!("session not found: {session_id}"),
            }
        }
        DaemonCommand::Kill { session_id } => {
            let pid = groups.lock().unwrap().get(session_id).copied();
            if let Some(pid) = pid {
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            }
            DaemonEvent::Ok
        }
        DaemonCommand::Spawn {
            session_id,
            executable,
            args,
            cwd,
            env,
            ..
        } if is_startup_session(session_id) => {
            run_startup_terminal(session_id, executable, args, cwd, env, exit_tx, groups)
        }
        DaemonCommand::Spawn { session_id, .. } | DaemonCommand::SpawnAgent { session_id, .. } => {
            recorded.lock().unwrap().push(command.clone());
            DaemonEvent::SessionCreated {
                session_id: session_id.clone(),
            }
        }
        _ => DaemonEvent::Ok,
    }
}

fn run_startup_terminal(
    session_id: &str,
    executable: &str,
    args: &[String],
    cwd: &str,
    env: &HashMap<String, String>,
    exit_tx: &tokio::sync::broadcast::Sender<String>,
    groups: &Arc<Mutex<HashMap<String, i32>>>,
) -> DaemonEvent {
    let mut process = std::process::Command::new(executable);
    process
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null());
    unsafe {
        process.pre_exec(|| {
            // Its own group, so a kill reaches every descendant rather than
            // the shell alone.
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    match process.spawn() {
        Ok(child) => {
            let pid = child.id() as i32;
            groups.lock().unwrap().insert(session_id.to_string(), pid);
            let exiting_session_id = session_id.to_string();
            let exit_tx = exit_tx.clone();
            let groups = Arc::clone(groups);
            // On a blocking thread: waiting here would stop the runtime that
            // has to deliver the Exit and answer the caller's Kill.
            std::thread::spawn(move || {
                let mut child = child;
                let code = child
                    .wait()
                    .map(|status| status.code().unwrap_or(-1))
                    .unwrap_or(-1);
                groups.lock().unwrap().remove(&exiting_session_id);
                let exit = serde_json::to_string(&DaemonEvent::Exit {
                    session_id: exiting_session_id,
                    code,
                    resume_session_id: None,
                    killed: false,
                })
                .unwrap();
                let _ = exit_tx.send(exit);
            });
            DaemonEvent::SessionCreated {
                session_id: session_id.to_string(),
            }
        }
        Err(error) => DaemonEvent::Error {
            code: None,
            message: error.to_string(),
        },
    }
}

async fn read_line(reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>) -> Option<String> {
    let mut line = String::new();
    match reader.read_line(&mut line).await {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// Stand-in for `kanna-cli setup-receipt`: writes the calling shell's exported
/// environment and working directory as the launch receipt the server reads.
///
/// A launch's startup shell ends by asking the bundled `kanna-cli` to write its
/// environment down for the agent that follows it, so a stand-in that silently
/// did nothing would make every setup test look like a launch whose startup
/// left nothing behind. This answers that one subcommand for real; everything
/// else stays the no-op it was.
const TEST_KANNA_CLI_STUB: &str = r#"#!/bin/zsh -f
[[ "$1" == "setup-receipt" ]] || exit 0
out="$3"
mkdir -p "${out:h}"
esc() { print -rn -- "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'; }
{
  print -n '{"version":1,"cwd":"'
  esc "$PWD"
  print -n '","env":{'
  sep=''
  for name in ${(f)"$(typeset -x +)"}; do
    print -n "$sep\""
    esc "$name"
    print -n '":"'
    esc "${(P)name}"
    print -n '"'
    sep=','
  done
  print -n '}}'
} > "$out"
"#;

const TEST_NOOP_STUB: &str = "#!/bin/sh\nexit 0\n";

/// Put a sidecar beside the test executable, keeping a real one if it is there.
///
/// The stub is deliberately never removed: tests run in parallel and share this
/// one path, so deleting it when the first of them finishes pulls the binary
/// out from under every other test still running a startup shell that is about
/// to invoke it. It lives in the build directory, which the next real sidecar
/// build overwrites anyway. A stub left by an *older* build is rewritten rather
/// than reused, which is what a shebang check distinguishes: a real sidecar is
/// a binary and never starts with `#!`.
pub(crate) fn ensure_test_sidecar_stub(name: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let sidecar_path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join(name);
    let existing = std::fs::read(&sidecar_path).unwrap_or_default();
    if !existing.is_empty() && !existing.starts_with(b"#!") {
        return sidecar_path;
    }
    let stub = if name == "kanna-cli" {
        TEST_KANNA_CLI_STUB
    } else {
        TEST_NOOP_STUB
    };
    if existing != stub.as_bytes() {
        std::fs::write(&sidecar_path, stub).unwrap();
        let mut permissions = std::fs::metadata(&sidecar_path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&sidecar_path, permissions).unwrap();
    }
    sidecar_path
}
