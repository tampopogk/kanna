//! The harness process wrapper, driven with small `sh` scripts.

use std::time::Duration;

use agent_tui::process::{ExitInfo, HarnessProcess, ProcEvent};
use agent_tui::protocol::SpawnSpec;
use tokio::sync::mpsc;

fn sh(script: &str) -> SpawnSpec {
    SpawnSpec {
        program: "sh".into(),
        args: vec!["-c".into(), script.into()],
    }
}

async fn collect(
    mut rx: mpsc::UnboundedReceiver<(u64, ProcEvent)>,
) -> (Vec<String>, Option<ExitInfo>) {
    let mut records = Vec::new();
    let mut exit = None;
    let mut eof = false;
    while !(eof && exit.is_some()) {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Some((_, ProcEvent::Record(r)))) => records.push(r.raw),
            Ok(Some((_, ProcEvent::Eof))) => eof = true,
            Ok(Some((_, ProcEvent::Exited(i)))) => exit = Some(i),
            Ok(None) | Err(_) => break,
        }
    }
    (records, exit)
}

#[tokio::test]
async fn non_utf8_stderr_does_not_kill_the_harness() {
    // Invalid UTF-8 on stderr, then a burst of stderr larger than a pipe buffer
    // (which blocks or SIGPIPEs if stderr stops being drained), then stdout.
    let script = r#"printf 'bad \377\376\n' >&2
i=0; while [ $i -lt 2000 ]; do echo "more stderr line $i padding padding padding" >&2; i=$((i+1)); done
echo '{"type":"result","ok":true}'
exit 0"#;
    let (tx, rx) = mpsc::unbounded_channel();
    let proc = HarnessProcess::spawn(&sh(script), None, 1, tx).unwrap();
    let (records, exit) = collect(rx).await;
    assert_eq!(
        records,
        vec![r#"{"type":"result","ok":true}"#.to_string()],
        "final stdout record arrives"
    );
    assert_eq!(
        exit,
        Some(ExitInfo {
            code: Some(0),
            signal: None
        }),
        "clean exit, not SIGPIPE"
    );
    let tail = proc.stderr_tail();
    assert!(tail.last().unwrap().contains("more stderr line 1999"));
    drop(proc);
}

#[tokio::test]
async fn shutdown_kills_the_whole_process_group() {
    // A harness that ignores stdin EOF and leaves a tool subprocess running.
    let (tx, _rx) = mpsc::unbounded_channel();
    let mut proc =
        HarnessProcess::spawn(&sh("sleep 30 & trap '' TERM; wait"), None, 1, tx).unwrap();
    let pgid = proc.pid.unwrap() as i32;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // SAFETY: signal 0 only checks for existence.
    assert_eq!(unsafe { libc::killpg(pgid, 0) }, 0, "group is running");
    proc.shutdown(Duration::from_millis(300)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_ne!(
        unsafe { libc::killpg(pgid, 0) },
        0,
        "no process left in the group"
    );
}
