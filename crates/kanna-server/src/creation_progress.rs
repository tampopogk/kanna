//! Short-lived live projection of creation. Completed setup remains in workspace_setup_run.
//! The scoped observer is only installed on synchronous creation workers, never across await.
use std::{
    cell::RefCell,
    collections::HashMap,
    process::{Command, Output},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub phase: String,
    pub status: String,
    pub output: String,
    pub error: Option<String>,
}
struct Entry {
    snapshot: Snapshot,
    touched: Instant,
    bytes: Vec<u8>,
}
const LIMIT: usize = 1024 * 1024;
fn entries() -> &'static Mutex<HashMap<String, Entry>> {
    static ENTRIES: OnceLock<Mutex<HashMap<String, Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(|| Mutex::new(HashMap::new()))
}
thread_local! { static CURRENT: RefCell<Option<String>> = const { RefCell::new(None) }; }
pub(crate) fn scoped<T>(id: &str, work: impl FnOnce() -> T) -> T {
    struct Restore(Option<String>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|current| {
                current.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(CURRENT.with(|current| current.replace(Some(id.into()))));
    work()
}
fn update(work: impl FnOnce(&mut Entry)) {
    CURRENT.with(|current| {
        if let Some(id) = current.borrow().as_deref() {
            if let Some(entry) = entries()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(id)
            {
                work(entry);
                entry.touched = Instant::now();
            }
        }
    });
}
fn append_entry(entry: &mut Entry, bytes: &[u8]) {
    let kept = LIMIT.saturating_sub(entry.bytes.len()).min(bytes.len());
    entry.bytes.extend_from_slice(&bytes[..kept]);
    if kept < bytes.len() && entry.bytes.len() == LIMIT {
        entry.bytes.extend_from_slice(b"\r\n[output truncated]\r\n");
    }
}
pub(crate) fn append(bytes: &[u8]) {
    update(|entry| append_entry(entry, bytes));
}
pub(crate) fn begin(id: &str) {
    let mut entries = entries().lock().unwrap_or_else(|e| e.into_inner());
    entries.retain(|_, e| e.touched.elapsed() < Duration::from_secs(15 * 60));
    if entries.len() >= 128 {
        if let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, e)| e.touched)
            .map(|(id, _)| id.clone())
        {
            entries.remove(&oldest);
        }
    }
    entries.insert(
        id.into(),
        Entry {
            snapshot: Snapshot {
                phase: "Preparing task".into(),
                status: "running".into(),
                output: String::new(),
                error: None,
            },
            touched: Instant::now(),
            bytes: Vec::new(),
        },
    );
}
pub(crate) fn read(id: &str) -> Option<Snapshot> {
    entries()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .map(|e| {
            let mut snapshot = e.snapshot.clone();
            snapshot.output = String::from_utf8_lossy(&e.bytes).into_owned();
            snapshot
        })
}
pub(crate) fn phase(phase: &str) {
    update(|e| {
        e.snapshot.phase = phase.into();
        append_entry(e, format!("\r\n━━ {phase} ━━\r\n").as_bytes());
    });
}
#[cfg(test)]
pub(crate) fn output(output: &str) {
    append(output.as_bytes());
}

pub(crate) fn finish(id: &str, error: Option<&str>) {
    scoped(id, || {
        update(|e| {
            e.snapshot.status = if error.is_some() {
                "failed"
            } else {
                "succeeded"
            }
            .into();
            e.snapshot.phase = if error.is_some() {
                "Task creation failed"
            } else {
                "Task created"
            }
            .into();
            e.snapshot.error = error.map(str::to_owned);
            if let Some(error) = error {
                append_entry(e, format!("\r\n{error}\r\n").as_bytes());
            }
        })
    });
}
/// Git phases are reported before execution; their actual output and exit status are retained.
pub(crate) fn command(phase_name: &str, command: &mut Command) -> std::io::Result<Output> {
    use std::io::Read;
    use std::process::Stdio;
    phase(phase_name);
    let mut child = match command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            append(format!("Failed to start: {error}\r\n").as_bytes());
            return Err(error);
        }
    };
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let (sender, receiver) = std::sync::mpsc::sync_channel(16);
    // Reader threads tee both pipes into a bounded channel; the scoped creation
    // worker publishes the bytes and retains the normal Command::output result.
    std::thread::scope(|scope| {
        for (is_stderr, pipe) in [
            (
                false,
                Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            ),
            (
                true,
                Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
            ),
        ] {
            let sender = sender.clone();
            scope.spawn(move || {
                let mut pipe = pipe;
                let mut buffer = [0u8; 8192];
                loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            if sender
                                .send((is_stderr, Ok(buffer[..count].to_vec())))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            let _ = sender.send((is_stderr, Err(error)));
                            break;
                        }
                    }
                }
            });
        }
        drop(sender);
        let mut read_error = None;
        for (is_stderr, chunk) in receiver {
            match chunk {
                Ok(bytes) => {
                    append(&bytes);
                    if is_stderr {
                        stderr.extend(bytes);
                    } else {
                        stdout.extend(bytes);
                    }
                }
                Err(error) => {
                    read_error = Some(error);
                }
            }
        }
        let status = child.wait()?;
        append(format!("\r\n{status}\r\n").as_bytes());
        if let Some(error) = read_error {
            return Err(error);
        }
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_runner_publishes_before_exit_and_preserves_failure() {
        let dir = tempfile::tempdir().unwrap();
        let gate = dir.path().join("continue");
        let id = format!("live-{}", std::process::id());
        begin(&id);
        let worker_id = id.clone();
        let cwd = dir.path().to_owned();
        let worker = std::thread::spawn(move || {
            scoped(&worker_id, || {
                phase("Running workspace setup");
                crate::workspace_commands::run_workspace_command_captured(
                "workspace setup",
                "printf FIRST; while [ ! -f continue ]; do sleep 0.02; done; printf SECOND; printf CONTROLLED_FAILURE >&2; exit 23",
                &cwd, &HashMap::new(),
            ).unwrap()
            })
        });
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let snapshot = read(&id).unwrap();
            if snapshot.output.contains("FIRST") {
                assert_eq!(snapshot.status, "running");
                assert!(!snapshot.output.contains("SECOND"));
                assert!(!worker.is_finished());
                break;
            }
            if Instant::now() > deadline {
                std::fs::write(&gate, "").unwrap();
                worker.join().unwrap();
                panic!("live output did not arrive before process exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::write(gate, "").unwrap();
        let outcome = worker.join().unwrap();
        assert_eq!(outcome.exit_code, Some(23));
        finish(&id, outcome.failure.as_deref());
        let snapshot = read(&id).unwrap();
        assert_eq!(snapshot.status, "failed");
        assert!(snapshot.output.contains("SECOND"));
        assert!(snapshot.output.contains("CONTROLLED_FAILURE"));
        assert!(snapshot.error.unwrap().contains("23"));
    }
    #[test]
    fn command_tees_ansi_stderr_before_exit_and_keeps_the_command_result() {
        let dir = tempfile::tempdir().unwrap();
        let gate = dir.path().join("continue");
        let id = "command-tee";
        begin(id);
        let cwd = dir.path().to_owned();
        let worker = std::thread::spawn(move || {
            scoped(id, || {
                command(
            "Git fetch origin",
            Command::new("sh").current_dir(cwd).args(["-c", "printf '\\033[32mFETCH_LIVE\\033[0m' >&2; while [ ! -f continue ]; do sleep 0.02; done; printf DONE; exit 23"]),
        ).unwrap()
            })
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !read(id).unwrap().output.contains("FETCH_LIVE") {
            if Instant::now() >= deadline {
                std::fs::write(&gate, "").unwrap();
                worker.join().unwrap();
                panic!("git command output was buffered until exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!worker.is_finished());
        assert!(read(id)
            .unwrap()
            .output
            .contains("\x1b[32mFETCH_LIVE\x1b[0m"));
        std::fs::write(&gate, "").unwrap();
        let outcome = worker.join().unwrap();
        assert_eq!(outcome.status.code(), Some(23));
        assert_eq!(outcome.stdout, b"DONE");
        assert_eq!(outcome.stderr, b"\x1b[32mFETCH_LIVE\x1b[0m");
    }

    #[test]
    fn tee_preserves_split_utf8_and_bounds_retention() {
        let id = "tee-utf8";
        begin(id);
        scoped(id, || {
            append(&[0xe2, 0x82]);
            append(&[0xac]);
        });
        assert_eq!(read(id).unwrap().output, "€");
        scoped(id, || {
            append(&vec![b'x'; LIMIT]);
            append(b"discarded");
        });
        let output = read(id).unwrap().output;
        assert_eq!(output.matches("[output truncated]").count(), 1);
        assert!(!output.contains("discarded"));
        assert!(output.len() < LIMIT + 100);
    }

    #[test]
    fn scoped_commands_keep_task_identity_and_actual_git_output() {
        begin("progress-a");
        begin("progress-b");
        scoped("progress-a", || {
            command("Git fetch origin", Command::new("git").arg("--version")).unwrap();
            scoped("progress-b", || {
                phase("Other task");
                output("B");
            });
            phase("Creating workspace / git worktree");
        });
        let a = read("progress-a").unwrap();
        assert!(a.output.contains("git version"));
        assert!(a.output.contains("Git fetch origin"));
        assert_eq!(a.phase, "Creating workspace / git worktree");
        assert!(!a.output.contains("Other task"));
        assert!(read("progress-b").unwrap().output.ends_with('B'));
    }
}
