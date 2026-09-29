//! The delivery worker against a scripted daemon speaking the real protocol:
//! when feedback may be written, how often it is written, and what happens
//! when an answer is lost or a daemon restarts.

use super::*;
use crate::design::service::tests::seed_design_task;
use crate::design::service::{self as design_service, AnchorRequest, CreateThreadRequest};
use kanna_daemon::protocol::{Command, DesignDeliveryOutcome, Event, SessionInfo};
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// What the scripted daemon does with a submission.
#[derive(Clone, Copy, PartialEq)]
enum Answer {
    /// Write it (when free) and answer.
    Normally,
    /// Write it, keep the receipt, and hang up without answering: a lost
    /// round trip after the bytes reached the PTY.
    WriteThenHangUp,
    /// Hang up before doing anything: a lost round trip with nothing written.
    HangUpUnwritten,
    /// Accept it, start writing, and hang up: the answer is lost while the
    /// daemon's receipt still says `Accepted`.
    AcceptThenHangUp,
}

struct Script {
    status: SessionStatus,
    observed: bool,
    attestation: ComposerAttestation,
    answer: Answer,
    instance: String,
    known: Vec<String>,
    receipts: HashMap<String, DesignDeliveryOutcome>,
    /// Every message the PTY received.
    written: Vec<String>,
    session_present: bool,
}

type Shared = Arc<StdMutex<Script>>;

fn script() -> Shared {
    Arc::new(StdMutex::new(Script {
        status: SessionStatus::Idle,
        observed: true,
        attestation: ComposerAttestation::NotTyped,
        answer: Answer::Normally,
        instance: "daemon-a".into(),
        known: vec!["daemon-a".into()],
        receipts: HashMap::new(),
        written: Vec::new(),
        session_present: true,
    }))
}

fn serve(daemon_dir: &str, script: Shared) -> tokio::task::JoinHandle<()> {
    std::fs::create_dir_all(daemon_dir).unwrap();
    let socket = kanna_runtime_defaults::socket_path(std::path::Path::new(daemon_dir));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let script = Arc::clone(&script);
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut reader = BufReader::new(read);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let command: Command = serde_json::from_str(line.trim()).unwrap();
                    let event = {
                        let mut script = script.lock().unwrap();
                        match command {
                            Command::List => Event::SessionList {
                                sessions: script
                                    .session_present
                                    .then(|| SessionInfo {
                                        hosted_frontend: None,
                                        session_id: "task-d".into(),
                                        pid: 7,
                                        cwd: "/tmp".into(),
                                        state: SessionState::Active,
                                        idle_seconds: 0,
                                        status: script.status,
                                        status_observed: script.observed,
                                        kind: SessionKind::Pty,
                                        composer_text: None,
                                        composer_attestation: script.attestation,
                                        attempt_id: None,
                                    })
                                    .into_iter()
                                    .collect(),
                            },
                            Command::NegotiateDesignDelivery { version } => {
                                Event::DesignDeliveryReady {
                                    version,
                                    instance: script.instance.clone(),
                                }
                            }
                            Command::QueryDesignDelivery { delivery_id } => Event::DesignDelivery {
                                outcome: script
                                    .receipts
                                    .get(&delivery_id)
                                    .cloned()
                                    .unwrap_or(DesignDeliveryOutcome::Unknown),
                                delivery_id,
                                known_instances: script.known.clone(),
                            },
                            Command::SubmitDesignInput {
                                delivery_id, data, ..
                            } => {
                                if script.answer == Answer::HangUpUnwritten {
                                    return;
                                }
                                if script.answer == Answer::AcceptThenHangUp {
                                    script
                                        .receipts
                                        .insert(delivery_id, DesignDeliveryOutcome::Accepted);
                                    return;
                                }
                                if let Some(existing) = script.receipts.get(&delivery_id) {
                                    Event::DesignDelivery {
                                        outcome: existing.clone(),
                                        delivery_id,
                                        known_instances: script.known.clone(),
                                    }
                                } else if script.status != SessionStatus::Idle {
                                    Event::DesignDelivery {
                                        outcome: DesignDeliveryOutcome::NotFree {
                                            reason: "busy".into(),
                                        },
                                        delivery_id,
                                        known_instances: script.known.clone(),
                                    }
                                } else {
                                    script.written.push(String::from_utf8(data).unwrap());
                                    script.receipts.insert(
                                        delivery_id.clone(),
                                        DesignDeliveryOutcome::Delivered,
                                    );
                                    if script.answer == Answer::WriteThenHangUp {
                                        return;
                                    }
                                    Event::DesignDelivery {
                                        outcome: DesignDeliveryOutcome::Delivered,
                                        delivery_id,
                                        known_instances: script.known.clone(),
                                    }
                                }
                            }
                            other => panic!("unexpected daemon command {other:?}"),
                        }
                    };
                    let mut bytes = serde_json::to_vec(&event).unwrap();
                    bytes.push(b'\n');
                    if write.write_all(&bytes).await.is_err() {
                        return;
                    }
                }
            });
        }
    })
}

fn state(label: &str) -> (Arc<AppState>, String) {
    let daemon_dir = crate::test_paths::unique_test_path_string(&format!("design-daemon-{label}"));
    let state = crate::http_api::test_support::test_state_with_daemon_dir(
        &format!("design-delivery-{label}"),
        "Design",
        &daemon_dir,
        seed_design_task,
    );
    (state, daemon_dir)
}

fn with_db<T>(state: &AppState, work: impl FnOnce(&Db) -> T) -> T {
    work(&Db::open(&state.config().db_path).unwrap())
}

fn message(state: &AppState, thread: &str, body: &str) {
    with_db(state, |db| {
        design_service::create_thread(
            db,
            &state.design,
            "task-d",
            CreateThreadRequest {
                thread_id: thread.into(),
                comment_id: format!("{thread}-c"),
                kind: "message".into(),
                body: body.into(),
                anchor: None,
            },
            None,
        )
        .unwrap();
    });
}

fn states(state: &AppState) -> Vec<String> {
    with_db(state, |db| {
        db.design_deliveries("task-d")
            .unwrap()
            .into_iter()
            .map(|row| row.state)
            .collect()
    })
}

fn inputs(state: &AppState) -> usize {
    with_db(state, |db| {
        db.list_task_inputs("task-d", 100)
            .map(|inputs| inputs.len())
            .unwrap_or(0)
    })
}

#[tokio::test]
async fn a_busy_agent_receives_nothing_and_a_free_one_receives_one_batch_once() {
    let (state, daemon_dir) = state("free");
    let script = script();
    let _daemon = serve(&daemon_dir, Arc::clone(&script));
    message(&state, "t1", "first");
    message(&state, "t2", "second");

    script.lock().unwrap().status = SessionStatus::Busy;
    deliver_task(&state, "task-d").await.unwrap();
    assert!(script.lock().unwrap().written.is_empty());
    assert_eq!(states(&state), vec!["queued", "queued"]);

    script.lock().unwrap().status = SessionStatus::Idle;
    deliver_task(&state, "task-d").await.unwrap();
    let written = script.lock().unwrap().written.clone();
    assert_eq!(written.len(), 1, "both items arrive as one batch");
    assert!(written[0].contains("#1 (thread t1)") && written[0].contains("#2 (thread t2)"));
    assert!(written[0].contains("kanna_design_reply"));
    assert_eq!(states(&state), vec!["delivered", "delivered"]);
    assert_eq!(inputs(&state), 1, "one task-input record for the batch");

    // Nothing is sent twice, and new feedback waits for the agent's turn.
    message(&state, "t3", "third");
    deliver_task(&state, "task-d").await.unwrap();
    assert_eq!(
        script.lock().unwrap().written.len(),
        1,
        "the turn fence holds the next batch"
    );
    script.lock().unwrap().status = SessionStatus::Busy;
    deliver_task(&state, "task-d").await.unwrap();
    script.lock().unwrap().status = SessionStatus::Idle;
    deliver_task(&state, "task-d").await.unwrap();
    assert_eq!(script.lock().unwrap().written.len(), 2);
    assert_eq!(states(&state), vec!["delivered", "delivered", "delivered"]);
}

#[tokio::test]
async fn a_draft_a_prompt_or_no_verdict_is_not_free() {
    let (state, daemon_dir) = state("not-free");
    let script = script();
    let _daemon = serve(&daemon_dir, Arc::clone(&script));
    message(&state, "t1", "hello");
    for configure in [
        |script: &mut Script| script.attestation = ComposerAttestation::Typed,
        |script: &mut Script| {
            script.attestation = ComposerAttestation::NotTyped;
            script.status = SessionStatus::Waiting;
        },
        |script: &mut Script| {
            script.status = SessionStatus::Idle;
            script.observed = false;
        },
        |script: &mut Script| {
            script.observed = true;
            script.session_present = false;
        },
    ] {
        configure(&mut script.lock().unwrap());
        deliver_task(&state, "task-d").await.unwrap();
        assert!(script.lock().unwrap().written.is_empty());
        assert_eq!(states(&state), vec!["queued"]);
    }
    let detail = with_db(&state, |db| {
        db.design_deliveries("task-d").unwrap()[0].detail.clone()
    });
    assert!(detail.unwrap().contains("not running"));
}

#[tokio::test]
async fn a_lost_answer_is_asked_about_and_never_retyped() {
    let (state, daemon_dir) = state("lost-answer");
    let script = script();
    let _daemon = serve(&daemon_dir, Arc::clone(&script));
    message(&state, "t1", "hello");
    script.lock().unwrap().answer = Answer::WriteThenHangUp;
    deliver_task(&state, "task-d").await.unwrap();
    // The daemon wrote it and kept the receipt; the server asked and settled.
    assert_eq!(states(&state), vec!["delivered"]);
    assert_eq!(script.lock().unwrap().written.len(), 1);
    assert_eq!(inputs(&state), 1);
}

#[tokio::test]
async fn a_lost_answer_with_nothing_written_is_queued_again() {
    let (state, daemon_dir) = state("unwritten");
    let script = script();
    let _daemon = serve(&daemon_dir, Arc::clone(&script));
    message(&state, "t1", "hello");
    script.lock().unwrap().answer = Answer::HangUpUnwritten;
    deliver_task(&state, "task-d").await.unwrap();
    assert_eq!(
        states(&state),
        vec!["queued"],
        "the daemon that would have written it never received it"
    );
    script.lock().unwrap().answer = Answer::Normally;
    deliver_task(&state, "task-d").await.unwrap();
    assert_eq!(states(&state), vec!["delivered"]);
    assert_eq!(script.lock().unwrap().written.len(), 1);
}

/// Review round 1, finding 2: a batch whose answer was lost while the daemon
/// was still writing it is settled on a later pass, not left `delivering`
/// with every later comment stuck behind it.
#[tokio::test]
async fn a_lost_answer_while_still_writing_is_settled_on_a_later_pass() {
    let (state, daemon_dir) = state("still-writing");
    let script = script();
    let _daemon = serve(&daemon_dir, Arc::clone(&script));
    message(&state, "t1", "one");
    script.lock().unwrap().answer = Answer::AcceptThenHangUp;
    deliver_task(&state, "task-d").await.unwrap();
    assert_eq!(states(&state), vec!["delivering"]);
    message(&state, "t2", "two");

    // Still being written: asked again, left alone, and the next comment waits.
    settle_in_flight(&state, false).await;
    assert_eq!(states(&state), vec!["delivering", "queued"]);
    // The write completes; the next pass applies the receipt, typing nothing.
    let attempt = with_db(&state, |db| {
        db.design_deliveries("task-d").unwrap()[0]
            .attempt_id
            .clone()
            .unwrap()
    });
    {
        let mut script = script.lock().unwrap();
        script
            .receipts
            .insert(attempt, DesignDeliveryOutcome::Delivered);
        script.answer = Answer::Normally;
    }
    settle_in_flight(&state, false).await;
    assert_eq!(states(&state), vec!["delivered", "queued"]);
    assert_eq!(inputs(&state), 1);
    assert!(
        script.lock().unwrap().written.is_empty(),
        "settling never types"
    );
    // And the comment behind it is delivered.
    state.design.delivery.lock().unwrap().fences.clear();
    deliver_task(&state, "task-d").await.unwrap();
    assert_eq!(states(&state), vec!["delivered", "delivered"]);
}

#[tokio::test]
async fn a_batch_in_flight_too_long_becomes_uncertain() {
    let (state, daemon_dir) = state("in-flight-limit");
    let script = script();
    let daemon = serve(&daemon_dir, Arc::clone(&script));
    message(&state, "t1", "one");
    message(&state, "t2", "two");
    with_db(&state, |db| {
        let rows = db.design_deliveries("task-d").unwrap();
        db.reserve_design_deliveries(&[rows[0].id.clone()], "da-accepted", "daemon-a")
            .unwrap();
    });
    script
        .lock()
        .unwrap()
        .receipts
        .insert("da-accepted".into(), DesignDeliveryOutcome::Accepted);
    let age = |state: &AppState, attempt: &str| {
        with_db(state, |db| {
            db.execute_test_sql(&format!(
                "UPDATE design_delivery SET updated_at = '2026-01-01T00:00:00.000Z' WHERE attempt_id = '{attempt}'"
            ))
            .unwrap()
        })
    };
    // Accepted and never confirmed, past the limit: uncertain, typed by nobody.
    age(&state, "da-accepted");
    settle_in_flight(&state, false).await;
    assert_eq!(states(&state)[0], "uncertain");

    // A daemon that cannot be asked: asked again while young…
    daemon.abort();
    let _ = std::fs::remove_file(kanna_runtime_defaults::socket_path(std::path::Path::new(
        &daemon_dir,
    )));
    with_db(&state, |db| {
        let rows = db.design_deliveries("task-d").unwrap();
        db.reserve_design_deliveries(&[rows[1].id.clone()], "da-unasked", "daemon-a")
            .unwrap();
    });
    settle_in_flight(&state, false).await;
    assert_eq!(states(&state)[1], "delivering");
    // …and uncertain once past the limit, so the person can resend it.
    age(&state, "da-unasked");
    settle_in_flight(&state, false).await;
    assert_eq!(states(&state), vec!["uncertain", "uncertain"]);
    assert!(script.lock().unwrap().written.is_empty());
}

#[tokio::test]
async fn after_a_restart_in_flight_batches_are_settled_from_receipts_or_marked_uncertain() {
    let (state, daemon_dir) = state("restart");
    let script = script();
    let _daemon = serve(&daemon_dir, Arc::clone(&script));
    message(&state, "t1", "one");
    message(&state, "t2", "two");
    // Two batches were being written when the server stopped: the daemon
    // wrote the first; the second went to a daemon that has since died.
    with_db(&state, |db| {
        let rows = db.design_deliveries("task-d").unwrap();
        db.reserve_design_deliveries(&[rows[0].id.clone()], "da-written", "daemon-a")
            .unwrap();
        db.reserve_design_deliveries(&[rows[1].id.clone()], "da-lost", "daemon-dead")
            .unwrap();
    });
    script
        .lock()
        .unwrap()
        .receipts
        .insert("da-written".into(), DesignDeliveryOutcome::Delivered);
    reconcile_in_flight(&state).await;
    assert_eq!(states(&state), vec!["delivered", "uncertain"]);
    assert!(
        script.lock().unwrap().written.is_empty(),
        "reconciliation never types"
    );
    assert_eq!(inputs(&state), 1);

    // Nothing retries an uncertain batch on its own…
    deliver_task(&state, "task-d").await.unwrap();
    assert!(script.lock().unwrap().written.is_empty());
    // …until the person resends it.
    let id = with_db(&state, |db| {
        db.design_deliveries("task-d").unwrap()[1].id.clone()
    });
    with_db(&state, |db| {
        design_service::retry_delivery(db, &state.design, "task-d", &id).unwrap()
    });
    state.design.delivery.lock().unwrap().fences.clear();
    deliver_task(&state, "task-d").await.unwrap();
    assert_eq!(states(&state), vec!["delivered", "delivered"]);
    assert_eq!(script.lock().unwrap().written.len(), 1);
}

#[tokio::test]
async fn a_comment_waits_for_its_anchor_and_order_is_kept() {
    let (state, daemon_dir) = state("anchor");
    let script = script();
    let _daemon = serve(&daemon_dir, Arc::clone(&script));
    // A state vector the server's document does not cover yet: the client's
    // anchor update has not arrived.
    let ahead = {
        let mut other = crate::design::document::DesignDocument::new();
        other.seed_empty();
        other.state_vector()
    };
    with_db(&state, |db| {
        let block = design_service::view(db, &state.design, db.db_path(), "task-d", true)
            .unwrap()
            .document
            .unwrap()
            .blocks[0]
            .id
            .clone();
        use base64::Engine;
        design_service::create_thread(
            db,
            &state.design,
            "task-d",
            CreateThreadRequest {
                thread_id: "t-anchor".into(),
                comment_id: "t-anchor-c".into(),
                kind: "comment".into(),
                body: "anchored".into(),
                anchor: Some(AnchorRequest {
                    block_id: block,
                    quoted_text: "the text".into(),
                    state_vector: Some(base64::engine::general_purpose::STANDARD.encode(&ahead)),
                    element: None,
                }),
            },
            None,
        )
        .unwrap();
    });
    message(&state, "t-after", "after it");
    deliver_task(&state, "task-d").await.unwrap();
    assert!(
        script.lock().unwrap().written.is_empty(),
        "the anchor has not arrived"
    );
    assert_eq!(states(&state), vec!["queued", "queued"]);
}

#[tokio::test]
async fn nothing_reaches_the_session_once_the_design_is_handed_off() {
    let (state, daemon_dir) = state("fence");
    let script = script();
    let _daemon = serve(&daemon_dir, Arc::clone(&script));
    message(&state, "t1", "late feedback");
    with_db(&state, |db| {
        db.set_design_session_status("task-d", crate::db::design::DesignSessionRow::HANDED_OFF)
            .unwrap();
        db.execute_test_sql("UPDATE pipeline_item SET stage = 'plan' WHERE id = 'task-d'")
            .unwrap();
    });
    deliver_task(&state, "task-d").await.unwrap();
    assert!(script.lock().unwrap().written.is_empty());
    assert_eq!(states(&state), vec!["queued"]);
}

#[test]
fn items_name_their_thread_quote_and_kind() {
    let rendered = render_message("task-d", &["#1 (thread t) /agent message:\n  hi".into()]);
    assert!(rendered.starts_with("Kanna App Design feedback (1 item,"));
    assert!(rendered.contains("\"task_id\": \"task-d\""));
    assert!(older_than(
        "2026-09-26T10:00:00.000Z",
        "2026-09-26T10:00:31.000Z",
        ANCHOR_GRACE
    ));
    assert!(!older_than(
        "2026-09-26T10:00:00.000Z",
        "2026-09-26T10:00:10.000Z",
        ANCHOR_GRACE
    ));
}

/// The whole path against a real `kanna-daemon` process: the daemon's own
/// classifier judges the session idle, the server negotiates the
/// design-delivery capability, the message reaches the PTY once, and the
/// daemon keeps its receipt.
mod real_daemon {
    use super::*;
    use kanna_agent_protocol::hosted_frontend::{
        Command as HostCommand, Config as HostConfig, Delivery, DeliveryState, Request, Response,
        RuntimeState, Snapshot, VERSION,
    };
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixListener as StdUnixListener;
    use std::process::{Child, Command as ProcessCommand};
    use std::sync::{Arc as StdArc, Mutex as ProcessMutex};
    use std::time::Duration;

    fn daemon_artifact_from_cargo_output(output: &[u8]) -> Option<std::path::PathBuf> {
        String::from_utf8_lossy(output).lines().find_map(|line| {
            let message: serde_json::Value = serde_json::from_str(line).ok()?;
            if message["reason"] != "compiler-artifact"
                || message["target"]["name"] != "kanna-daemon"
                || !message["target"]["kind"]
                    .as_array()?
                    .iter()
                    .any(|kind| kind == "bin")
            {
                return None;
            }
            message["executable"].as_str().map(std::path::PathBuf::from)
        })
    }

    fn current_daemon_binary() -> std::path::PathBuf {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        let output = ProcessCommand::new(env!("CARGO"))
            .args([
                "build",
                "-p",
                "kanna-daemon",
                "--message-format=json-render-diagnostics",
            ])
            .current_dir(workspace)
            .output()
            .expect("build the real daemon fixture from this checkout");
        assert!(
            output.status.success(),
            "the real daemon fixture must build:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let executable = daemon_artifact_from_cargo_output(&output.stdout)
            .expect("cargo must report the current kanna-daemon executable artifact");
        assert!(executable.is_file(), "reported daemon artifact must exist");
        executable
    }

    #[test]
    fn cargo_artifact_parser_uses_the_reported_target_directory() {
        let output = br#"{"reason":"compiler-artifact","target":{"kind":["bin"],"name":"kanna-daemon"},"executable":"/redirected/target/debug/kanna-daemon"}
"#;
        assert_eq!(
            daemon_artifact_from_cargo_output(output).unwrap(),
            std::path::Path::new("/redirected/target/debug/kanna-daemon")
        );
    }

    fn persist_host_snapshot(config: &HostConfig, snapshot: &Snapshot) {
        std::fs::write(&config.journal_path, serde_json::to_vec(snapshot).unwrap()).unwrap();
    }

    /// A separate-process hosted frontend with a file-controlled provider
    /// acknowledgement. The real daemon creates and authenticates its socket
    /// capability; this fixture owns the durable journal and only marks a
    /// delivery submitted after the test's controlled provider acknowledges.
    #[test]
    fn hosted_frontend_process_fixture() {
        let Ok(control) = std::env::var("KANNA_TEST_HOSTED_CONTROL") else {
            return;
        };
        let submissions = std::env::var("KANNA_TEST_HOSTED_SUBMISSIONS").unwrap();
        let config_path = std::env::var(kanna_agent_protocol::hosted_frontend::CONFIG_ENV).unwrap();
        let config: HostConfig =
            serde_json::from_slice(&std::fs::read(config_path).unwrap()).unwrap();
        let snapshot = StdArc::new(ProcessMutex::new(Snapshot {
            notice: None,
            version: VERSION,
            binding: config.binding.clone(),
            frontend_pid: std::process::id(),
            active_run_id: config.binding.run_id.clone(),
            sequence: 1,
            provider_session_id: Some("controlled-provider".into()),
            state: RuntimeState::Idle,
            diagnostic: None,
            composer_text: String::new(),
            queued_count: 0,
            deliveries: Vec::new(),
            retired: false,
        }));
        persist_host_snapshot(&config, &snapshot.lock().unwrap());

        let journal_config = config.clone();
        let journal_snapshot = StdArc::clone(&snapshot);
        std::thread::spawn(move || loop {
            let instruction = std::fs::read_to_string(&control).unwrap_or_default();
            let mut snapshot = journal_snapshot.lock().unwrap();
            let mut changed = false;
            for delivery in &mut snapshot.deliveries {
                if !delivery.state.pending() {
                    continue;
                }
                match instruction.trim() {
                    "submitted" => {
                        delivery.state = DeliveryState::Submitted;
                        delivery.text = None;
                        changed = true;
                    }
                    "failed" => {
                        delivery.state = DeliveryState::Failed;
                        delivery.error = Some("controlled provider rejected the input".into());
                        changed = true;
                    }
                    _ => {}
                }
            }
            if changed {
                snapshot.sequence += 1;
                snapshot.queued_count = snapshot
                    .deliveries
                    .iter()
                    .filter(|delivery| delivery.state == DeliveryState::Queued)
                    .count();
                persist_host_snapshot(&journal_config, &snapshot);
            }
            drop(snapshot);
            std::thread::sleep(Duration::from_millis(20));
        });

        let _ = std::fs::remove_file(&config.socket_path);
        let listener = StdUnixListener::bind(&config.socket_path).unwrap();
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut line = String::new();
            std::io::BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Request = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(request.version, VERSION);
            assert_eq!(request.binding, config.binding);
            assert_eq!(request.capability, config.capability);
            let response = match request.command {
                HostCommand::Inspect => Response::Snapshot {
                    snapshot: snapshot.lock().unwrap().clone(),
                },
                HostCommand::Submit {
                    delivery_id,
                    text,
                    workflow_prompt,
                    run_id,
                } => {
                    assert!(!workflow_prompt, "design feedback is ordinary input");
                    assert_eq!(run_id, None, "the frontend selects its active run");
                    let mut snapshot = snapshot.lock().unwrap();
                    let delivery = if let Some(existing) = snapshot
                        .deliveries
                        .iter()
                        .find(|delivery| delivery.delivery_id == delivery_id)
                    {
                        existing.clone()
                    } else {
                        let delivery = Delivery {
                            run_id: snapshot.active_run_id.clone(),
                            initial_prompt: false,
                            delivery_id: delivery_id.clone(),
                            sequence: snapshot.deliveries.len() as u64 + 1,
                            payload_hash: format!("controlled:{}", text.len()),
                            text: Some(text),
                            state: DeliveryState::Queued,
                            error: None,
                        };
                        snapshot.deliveries.push(delivery.clone());
                        snapshot.queued_count += 1;
                        snapshot.sequence += 1;
                        persist_host_snapshot(&config, &snapshot);
                        let mut file = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&submissions)
                            .unwrap();
                        writeln!(file, "{delivery_id}").unwrap();
                        delivery
                    };
                    Response::Accepted { delivery }
                }
                HostCommand::Retire { reason } => Response::Rejected { reason },
            };
            serde_json::to_writer(&mut stream, &response).unwrap();
            stream.write_all(b"\n").unwrap();
        }
    }

    struct OwnedDaemon(Child);
    impl Drop for OwnedDaemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    async fn start_daemon(
        daemon_dir: &str,
        binary: &std::path::Path,
    ) -> (OwnedDaemon, crate::daemon_client::DaemonClient) {
        std::fs::create_dir_all(daemon_dir).unwrap();
        let mut owned = OwnedDaemon(
            ProcessCommand::new(binary)
                .env("KANNA_DAEMON_DIR", daemon_dir)
                .env(
                    "KANNA_TERMINAL_RECOVERY_BIN",
                    "/nonexistent-design-fixture-sidecar",
                )
                .spawn()
                .unwrap(),
        );
        let client = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(owned.0.try_wait().unwrap().is_none(), "the daemon exited");
                if let Ok(client) = crate::daemon_client::DaemonClient::connect(daemon_dir).await {
                    break client;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("daemon starts");
        (owned, client)
    }

    #[tokio::test]
    async fn feedback_reaches_a_real_idle_session_exactly_once() {
        let binary = current_daemon_binary();
        let (state, daemon_dir) = super::state("real-daemon");
        let (_daemon, mut client) = start_daemon(&daemon_dir, &binary).await;
        let received = std::path::Path::new(&daemon_dir).join("received.txt");
        // A stand-in agent whose screen the Codex classifier reads as idle (an
        // empty `›` composer),
        // and which records every line it is sent.
        let spawn = serde_json::from_value::<Command>(serde_json::json!({
            "type": "Spawn",
            "session_id": "task-d",
            "executable": "/bin/sh",
            "args": ["-c", format!("printf 'OpenAI Codex\\r\\n\\r\\n\\342\\200\\272 '; cat >> {}", received.display())],
            "cwd": daemon_dir,
            "env": {},
            "cols": 100,
            "rows": 30,
            "agent_provider": "codex",
        }))
        .unwrap();
        assert!(matches!(
            client.send_command(&spawn).await.unwrap(),
            Event::SessionCreated { .. }
        ));
        let idle = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if matches!(
                    session_readiness(&state, "task-d").await,
                    Readiness::Free { .. }
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        })
        .await;
        if idle.is_err() {
            eprintln!(
                "skipping: the daemon never classified the stand-in session idle on this host"
            );
            return;
        }

        message(&state, "t1", "hello from the person");
        deliver_task(&state, "task-d").await.unwrap();
        assert_eq!(states(&state), vec!["delivered"]);
        let text = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let text = std::fs::read_to_string(&received).unwrap_or_default();
                if text.contains("hello from the person") {
                    break text;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the message reaches the session");
        assert_eq!(text.matches("hello from the person").count(), 1);

        // Nothing more to send, and asking again types nothing.
        state.design.delivery.lock().unwrap().fences.clear();
        deliver_task(&state, "task-d").await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let text = std::fs::read_to_string(&received).unwrap();
        assert_eq!(text.matches("hello from the person").count(), 1);

        // The daemon kept the receipt a lost answer would be settled from.
        let attempt = with_db(&state, |db| {
            db.design_deliveries("task-d").unwrap()[0]
                .attempt_id
                .clone()
                .unwrap()
        });
        match client
            .send_command(&Command::QueryDesignDelivery {
                delivery_id: attempt,
            })
            .await
            .unwrap()
        {
            Event::DesignDelivery { outcome, .. } => {
                assert_eq!(outcome, DesignDeliveryOutcome::Delivered)
            }
            other => panic!("unexpected answer {other:?}"),
        }
    }

    #[tokio::test]
    async fn hosted_feedback_waits_for_provider_receipts_and_never_submits_twice() {
        let binary = current_daemon_binary();
        let (state, daemon_dir) = super::state("real-hosted-daemon");
        let (_daemon, mut client) = start_daemon(&daemon_dir, &binary).await;
        let control = std::path::Path::new(&daemon_dir).join("provider-state");
        let submissions = std::path::Path::new(&daemon_dir).join("provider-submissions");
        std::fs::write(&control, "pending").unwrap();
        let executable = std::env::current_exe().unwrap();
        let spawn = serde_json::from_value::<Command>(serde_json::json!({
            "type": "Spawn",
            "session_id": "task-d",
            "executable": executable,
            "args": ["--exact", "design::delivery::tests::real_daemon::hosted_frontend_process_fixture", "--nocapture"],
            "cwd": daemon_dir,
            "env": {
                "KANNA_TASK_ID": "task-d",
                "KANNA_STAGE_RUN_ID": "run-design",
                "KANNA_AGENT_FRONTENDS": "{\"codex\":\"agent-tui\"}",
                "KANNA_TEST_HOSTED_CONTROL": control,
                "KANNA_TEST_HOSTED_SUBMISSIONS": submissions,
            },
            "cols": 100,
            "rows": 30,
            "agent_provider": "codex",
        }))
        .unwrap();
        match client.send_command(&spawn).await.unwrap() {
            Event::SessionCreated { .. } => {}
            other => panic!("unexpected spawn answer {other:?}"),
        }
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if matches!(
                    session_readiness(&state, "task-d").await,
                    Readiness::Free { .. }
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("hosted frontend becomes idle");
        let pid = match client.send_command(&Command::List).await.unwrap() {
            Event::SessionList { sessions } => {
                sessions
                    .into_iter()
                    .find(|session| session.session_id == "task-d")
                    .expect("hosted session remains present")
                    .pid
            }
            other => panic!("unexpected list answer {other:?}"),
        };

        message(&state, "hosted-ack", "wait for provider acknowledgement");
        deliver_task(&state, "task-d").await.unwrap();
        assert_eq!(states(&state), vec!["delivering"]);
        let attempt = with_db(&state, |db| {
            db.design_deliveries("task-d").unwrap()[0]
                .attempt_id
                .clone()
                .unwrap()
        });
        assert!(matches!(
            client
                .send_command(&Command::QueryDesignDelivery {
                    delivery_id: attempt.clone()
                })
                .await
                .unwrap(),
            Event::DesignDelivery {
                outcome: DesignDeliveryOutcome::Accepted,
                ..
            }
        ));
        let repeat = Command::SubmitDesignInput {
            session_id: "task-d".into(),
            expected_pid: pid,
            delivery_id: attempt.clone(),
            data: b"ignored duplicate payload".to_vec(),
        };
        assert!(matches!(
            client.send_command(&repeat).await.unwrap(),
            Event::DesignDelivery {
                outcome: DesignDeliveryOutcome::Accepted,
                ..
            }
        ));
        assert_eq!(
            std::fs::read_to_string(&submissions)
                .unwrap()
                .lines()
                .count(),
            1
        );

        std::fs::write(&control, "submitted").unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        settle_in_flight(&state, false).await;
        assert_eq!(states(&state), vec!["delivered"]);

        state.design.delivery.lock().unwrap().fences.clear();
        std::fs::write(&control, "pending").unwrap();
        message(&state, "hosted-fail", "provider will reject this");
        deliver_task(&state, "task-d").await.unwrap();
        assert_eq!(states(&state), vec!["delivered", "delivering"]);
        std::fs::write(&control, "failed").unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        settle_in_flight(&state, false).await;
        assert_eq!(states(&state), vec!["delivered", "uncertain"]);
        assert_eq!(
            std::fs::read_to_string(&submissions)
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
}
