//! Opt-in checks against real, signed-in harnesses (costs a little usage).
//!
//!   cargo test --features live --test live -- --ignored --test-threads=1
//!
//! Each test drives the real `App` and child process headlessly: the same
//! code paths as the terminal UI minus drawing.
#![cfg(feature = "live")]

mod common;

use std::time::{Duration, Instant};

use agent_tui::app::transcript::EntryKind;
use agent_tui::app::{App, Status};
use agent_tui::harness::{make_adapter, HarnessOptions};
use agent_tui::process::{HarnessProcess, ProcEvent};
use agent_tui::protocol::{HarnessKind, MetaSource};
use agent_tui::ui::skins::SkinId;
use common::{ctrl, key, type_text};
use crossterm::event::KeyCode;
use tokio::sync::mpsc;

struct Live {
    app: App,
    proc: Option<HarnessProcess>,
    rx: mpsc::UnboundedReceiver<(u64, ProcEvent)>,
    dir: std::path::PathBuf,
}

impl Live {
    fn start(kind: HarnessKind, extra: &[&str]) -> Live {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!(".tmp/live-{kind:?}").to_lowercase());
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let opts = HarnessOptions {
            kind,
            model: (kind == HarnessKind::Claude).then(|| "haiku".to_string()),
            effort: Some("low".into()),
            cwd: Some(dir.to_string_lossy().into_owned()),
            program: None,
            extra_args: extra.iter().map(|s| s.to_string()).collect(),
        };
        let mut app = App::new(make_adapter(&opts), SkinId::Graphite);
        let (tx, rx) = mpsc::unbounded_channel();
        let proc = HarnessProcess::spawn(&app.adapter.spawn_spec(), opts.cwd.as_deref(), 1, tx)
            .expect("spawn harness");
        app.start();
        let mut live = Live {
            app,
            proc: Some(proc),
            rx,
            dir,
        };
        live.flush();
        live
    }

    fn flush(&mut self) {
        for line in self.app.take_outbox() {
            assert!(self.proc.as_ref().unwrap().send(line), "stdin closed");
        }
    }

    /// Processes harness output until `done` holds or the timeout passes.
    async fn until(&mut self, what: &str, secs: u64, done: impl Fn(&App) -> bool) {
        let end = Instant::now() + Duration::from_secs(secs);
        while !done(&self.app) {
            let left = end.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out waiting for {what}; status {:?}",
                self.app.status()
            );
            if let Ok(Some((_, ev))) = tokio::time::timeout(left, self.rx.recv()).await {
                match ev {
                    ProcEvent::Record(r) => self.app.on_record(r),
                    ProcEvent::Eof | ProcEvent::Exited(_) => {
                        let tail = self
                            .proc
                            .as_ref()
                            .map(|p| p.stderr_tail())
                            .unwrap_or_default();
                        panic!("harness ended while waiting for {what}: {tail:?}");
                    }
                }
                self.app.tick(Instant::now());
                self.flush();
            }
        }
    }

    fn send(&mut self, text: &str) {
        type_text(&mut self.app, text);
        self.app.on_key(key(KeyCode::Enter));
        assert_eq!(self.app.status(), Status::Working, "prompt was sent");
        self.flush();
    }

    fn assistant_texts(&self) -> Vec<String> {
        self.app
            .transcript
            .iter()
            .filter_map(|e| match &e.kind {
                EntryKind::Assistant { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    fn answer_card(&mut self, label: &str) {
        self.app.on_key(key(KeyCode::Tab));
        let card = self.app.transcript.first_pending_card().unwrap();
        let idx = match &self.app.transcript.get(card).unwrap().kind {
            EntryKind::Approval(c) => c
                .choices
                .iter()
                .position(|c| c.label == label)
                .expect("choice"),
            _ => panic!("not an approval card"),
        };
        for _ in 0..idx {
            self.app.on_key(key(KeyCode::Right));
        }
        self.app.on_key(key(KeyCode::Enter));
        self.flush();
    }

    async fn quit_and_check_no_children(mut self) {
        let mut p = self.proc.take().unwrap();
        let pid = p.pid.unwrap() as i32;
        p.shutdown(Duration::from_secs(3)).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        // SAFETY: signal 0 only checks for existence.
        let group_alive = unsafe { libc::killpg(pid, 0) } == 0;
        assert!(
            !group_alive,
            "processes remain in the harness process group {pid}"
        );
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn ends_with_stopped(app: &App) -> bool {
    matches!(app.transcript.iter().last().map(|e| &e.kind), Some(EntryKind::TurnEnd { text, .. }) if text == "■ Turn stopped")
}

async fn full_run(kind: HarnessKind, extra: &[&str], approval_prompt: &str, file: &str) {
    let mut l = Live::start(kind, extra);
    // 1. Header metadata from the harness.
    l.until("ready", 60, |a| a.status() == Status::Ready).await;
    l.send("Reply with exactly the word: purple");
    let mut saw_streaming = false;
    let end = Instant::now() + Duration::from_secs(90);
    while l.app.status() != Status::Ready {
        assert!(Instant::now() < end, "first turn timed out");
        if let Ok(Some((_, ProcEvent::Record(r)))) =
            tokio::time::timeout(Duration::from_secs(5), l.rx.recv()).await
        {
            l.app.on_record(r);
            l.flush();
            saw_streaming |= l.app.transcript.iter().any(|e| matches!(&e.kind, EntryKind::Assistant { done: false, text, .. } if !text.is_empty()));
        }
    }
    let model = l.app.meta.model.clone().expect("model reported");
    assert_eq!(model.source, MetaSource::Reported, "model {model:?}");
    if kind == HarnessKind::Codex {
        assert_eq!(
            l.app.meta.effort.as_ref().unwrap().source,
            MetaSource::Reported
        );
    }
    // 2. Streamed reply.
    assert!(l
        .assistant_texts()
        .last()
        .unwrap()
        .to_lowercase()
        .contains("purple"));
    eprintln!(
        "{kind:?}: model {} · streamed deltas seen: {saw_streaming}",
        model.value
    );
    // 3. Follow-up using context.
    l.send("What word did you just reply with? Answer with that one word only.");
    l.until("follow-up", 90, |a| a.status() == Status::Ready)
        .await;
    assert!(
        l.assistant_texts()
            .last()
            .unwrap()
            .to_lowercase()
            .contains("purple"),
        "context kept"
    );
    // 4. Approval: deny, then allow on retry.
    l.send(approval_prompt);
    l.until("first approval", 120, |a| {
        a.status() == Status::NeedsApproval
    })
    .await;
    l.answer_card("Deny");
    l.until("turn after deny", 120, |a| {
        matches!(a.status(), Status::Ready | Status::NeedsApproval)
    })
    .await;
    if l.app.status() == Status::NeedsApproval {
        // The agent retried within the same turn; allow it this time.
        l.answer_card("Allow once");
    } else {
        assert!(!l.dir.join(file).exists(), "denied command did not run");
        l.send("Please try that exact command again now.");
        l.until("retry approval", 120, |a| {
            a.status() == Status::NeedsApproval
        })
        .await;
        l.answer_card("Allow once");
    }
    l.until("turn after allow", 120, |a| a.status() == Status::Ready)
        .await;
    assert!(l.dir.join(file).exists(), "allowed command ran");
    // 5. Interrupt a long turn.
    l.send("Count from 1 to 500, one number per line, no other text.");
    l.until("output to start", 90, |a| {
        a.transcript
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::Assistant { done: false, .. }))
    })
    .await;
    l.app.on_key(ctrl('c'));
    assert_eq!(l.app.status(), Status::Stopping);
    l.flush();
    l.until("turn stopped", 60, |a| a.status() == Status::Ready)
        .await;
    assert!(
        ends_with_stopped(&l.app),
        "turn ends with only “Turn stopped”"
    );
    // 6. Quit leaves no child processes.
    l.quit_and_check_no_children().await;
}

#[tokio::test]
#[ignore]
async fn live_claude() {
    full_run(HarnessKind::Claude, &[], "Run the shell command `echo live-ok > live.txt` with the Bash tool. If it is denied, stop and tell me.", "live.txt").await;
}

#[tokio::test]
#[ignore]
async fn live_codex() {
    full_run(
        HarnessKind::Codex,
        &[
            "-c",
            "approval_policy=\"on-request\"",
            "-c",
            "sandbox_mode=\"read-only\"",
        ],
        "Run the shell command `echo live-ok > live.txt`. If it is declined, stop and tell me.",
        "live.txt",
    )
    .await;
}
