//! The harness child process: spawned in its own process group, fed through
//! stdin, framed from stdout, with a stderr tail kept for diagnostics.
//!
//! Shutdown closes stdin, waits up to three seconds, then signals the whole
//! process group so no tool subprocess outlives the client.

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::protocol::SpawnSpec;
use crate::transport::{JsonlFramer, Record};

const STDERR_TAIL_LINES: usize = 40;
/// Longest stderr line kept for display.
const STDERR_LINE_MAX: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

impl std::fmt::Display for ExitInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code, self.signal) {
            (Some(c), _) => write!(f, "exit code {c}"),
            (None, Some(s)) => write!(f, "signal {s}"),
            _ => write!(f, "unknown exit status"),
        }
    }
}

#[derive(Debug)]
pub enum ProcEvent {
    Record(Record),
    /// stdout closed.
    Eof,
    Exited(ExitInfo),
}

pub struct HarnessProcess {
    /// Session generation; events from older sessions are ignored.
    pub generation: u64,
    pub pid: Option<u32>,
    stdin_tx: Option<mpsc::UnboundedSender<String>>,
    exit_rx: watch::Receiver<Option<ExitInfo>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl HarnessProcess {
    pub fn spawn(
        spec: &SpawnSpec,
        cwd: Option<&str>,
        generation: u64,
        events: mpsc::UnboundedSender<(u64, ProcEvent)>,
    ) -> std::io::Result<Self> {
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn()?;
        let pid = child.id();
        tracing::info!(program = %spec.program, args = ?spec.args, ?pid, "spawned harness");

        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = stdin_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    tracing::warn!("harness stdin closed while writing");
                    break;
                }
            }
            // Dropping stdin closes the pipe: the harness sees EOF.
        });

        let tx = events.clone();
        tokio::spawn(async move {
            let mut framer = JsonlFramer::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for rec in framer.push(&buf[..n]) {
                            let _ = tx.send((generation, ProcEvent::Record(rec)));
                        }
                    }
                }
            }
            if let Some(rec) = framer.finish() {
                let _ = tx.send((generation, ProcEvent::Record(rec)));
            }
            let _ = tx.send((generation, ProcEvent::Eof));
        });

        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = stderr_tail.clone();
        tokio::spawn(async move {
            // Read raw bytes: stderr is not guaranteed to be UTF-8, and if we
            // stopped draining it the harness would die of SIGPIPE on its next write.
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
                let bytes = buf.strip_suffix(b"\n").unwrap_or(&buf);
                let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
                let mut line = String::from_utf8_lossy(bytes).into_owned();
                if line.len() > STDERR_LINE_MAX {
                    let mut cut = STDERR_LINE_MAX;
                    while !line.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    line.truncate(cut);
                    line.push('…');
                }
                tracing::debug!(target: "harness_stderr", "{line}");
                let mut t = tail.lock().unwrap();
                t.push_back(line);
                while t.len() > STDERR_TAIL_LINES {
                    t.pop_front();
                }
            }
        });

        let (exit_tx, exit_rx) = watch::channel(None);
        tokio::spawn(async move {
            let status = child.wait().await;
            let info = match status {
                Ok(s) => {
                    #[cfg(unix)]
                    let signal = std::os::unix::process::ExitStatusExt::signal(&s);
                    #[cfg(not(unix))]
                    let signal = None;
                    ExitInfo {
                        code: s.code(),
                        signal,
                    }
                }
                Err(_) => ExitInfo {
                    code: None,
                    signal: None,
                },
            };
            tracing::info!(%info, "harness exited");
            let _ = exit_tx.send(Some(info.clone()));
            let _ = events.send((generation, ProcEvent::Exited(info)));
        });

        Ok(Self {
            generation,
            pid,
            stdin_tx: Some(stdin_tx),
            exit_rx,
            stderr_tail,
        })
    }

    /// Queue one record for stdin. Returns false if the pipe is gone.
    pub fn send(&self, line: String) -> bool {
        self.stdin_tx
            .as_ref()
            .is_some_and(|tx| tx.send(line).is_ok())
    }

    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail.lock().unwrap().iter().cloned().collect()
    }

    pub fn has_exited(&self) -> bool {
        self.exit_rx.borrow().is_some()
    }

    /// Close stdin, wait for a clean exit, then terminate the process group.
    pub async fn shutdown(&mut self, grace: Duration) {
        self.stdin_tx = None;
        let mut rx = self.exit_rx.clone();
        let exited = tokio::time::timeout(grace, rx.wait_for(|e| e.is_some()))
            .await
            .is_ok();
        if !exited {
            self.signal_group(libc::SIGTERM);
            let mut rx = self.exit_rx.clone();
            if tokio::time::timeout(Duration::from_secs(1), rx.wait_for(|e| e.is_some()))
                .await
                .is_err()
            {
                self.signal_group(libc::SIGKILL);
            }
        }
        // Tool subprocesses in the group may outlive the harness itself.
        self.signal_group(libc::SIGKILL);
        self.pid = None;
    }

    fn signal_group(&self, sig: i32) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // SAFETY: plain syscall; pid is the group leader we created.
            unsafe {
                libc::killpg(pid as libc::pid_t, sig);
            }
        }
    }
}

impl Drop for HarnessProcess {
    fn drop(&mut self) {
        self.signal_group(libc::SIGKILL);
    }
}
