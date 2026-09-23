//! Focused tests for the task-store bridge: ordered, idempotent, crash-safe
//! publication of the SQL outbox, backfill, `task.json`, and delivery.

use super::*;
use crate::db::task_store::{LedgerEntryKind, NewLedgerEntry};
use serde_json::json;

struct Fixture {
    db: Db,
    db_path: String,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let db_path = Db::test_db_path(&format!("task-store-{label}"));
        let db = Db::open_for_tests(&db_path).expect("open test db");
        db.insert_test_repo_with_path("repo-1", "/tmp/repo-one", "Repo One")
            .unwrap();
        db.insert_test_pipeline_item(
            "task-1",
            "repo-1",
            "Build the ledger",
            Some("Ledger"),
            "in progress",
            "2026-09-22 00:00:00",
        )
        .unwrap();
        Self { db, db_path }
    }

    fn root(&self) -> PathBuf {
        root_for_db(&self.db_path)
    }

    fn task_dir(&self) -> PathBuf {
        task_dir(&self.root(), "repo-1", "task-1")
    }

    fn ledger_names(&self) -> Vec<String> {
        let mut names = std::fs::read_dir(self.task_dir().join("ledger"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn event_count(&self, event_type: &str) -> i64 {
        self.db
            .count_task_events_of_type_for_tests("task-1", event_type)
            .unwrap()
    }

    fn enqueue_result(
        &self,
        source_id: &str,
        message: &str,
    ) -> crate::db::task_store::LedgerEntryRef {
        self.db
            .with_immediate_transaction(|db| {
                let floor = db.ledger_event_floor()?;
                db.append_task_event(
                    "task-1",
                    crate::db::TaskEventKind::RunFinished,
                    json!({ "runId": source_id }),
                )?;
                db.enqueue_ledger_entry(NewLedgerEntry {
                    task_id: "task-1",
                    kind: LedgerEntryKind::Result,
                    operation_id: Some(&format!("op-{source_id}")),
                    source_kind: "stage_run",
                    source_id,
                    source_origin: None,
                    historical: false,
                    recorded_at: None,
                    run_id: Some(source_id),
                    declared_role: None,
                    body: json!({ "status": "success", "stage": "in progress" }),
                    message: Some(message),
                    hold_events_after: Some(floor),
                    reserved_sequence: None,
                })
            })
            .unwrap()
    }
}

#[test]
fn publishes_in_sequence_and_a_retry_writes_nothing_new() {
    let fixture = Fixture::new("ordered");
    let first = fixture.enqueue_result("run-1", "first\n\nbody");
    let second = fixture.enqueue_result("run-2", "second");
    assert_eq!((first.sequence, second.sequence), (1, 2));
    assert_eq!(first.entry_id, "task-1-000001");

    // Held until published: nobody hears about a result that is not on disk.
    assert_eq!(fixture.event_count("run.finished"), 0);
    let outcome = flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    assert_eq!(outcome.published, 2);
    assert_eq!(
        fixture.ledger_names(),
        vec!["000001-result.md", "000002-result.md"]
    );
    assert_eq!(fixture.event_count("run.finished"), 2);

    // Re-enqueueing the same source identity resolves the original entry.
    let replay = fixture
        .db
        .enqueue_ledger_entry(NewLedgerEntry {
            task_id: "task-1",
            kind: LedgerEntryKind::Result,
            operation_id: None,
            source_kind: "stage_run",
            source_id: "run-1",
            source_origin: None,
            historical: false,
            recorded_at: None,
            run_id: Some("run-1"),
            declared_role: None,
            body: json!({ "status": "failure" }),
            message: Some("different text"),
            hold_events_after: None,
            reserved_sequence: None,
        })
        .unwrap();
    assert_eq!(replay, first);
    let again = flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    assert_eq!(again.published, 0);
    assert_eq!(fixture.ledger_names().len(), 2);
    assert_eq!(fixture.event_count("run.finished"), 2);

    let files = read_ledger(&fixture.task_dir()).unwrap();
    assert_eq!(files[0].message.as_deref(), Some("first\n\nbody"));
    assert_eq!(files[0].body()["result_id"], "task-1-000001");
    assert_eq!(files[0].envelope["session_ref"]["kind"], "stage_run");
    assert_eq!(files[0].envelope["session_ref"]["id"], "run-1");
    assert!(files[0].envelope["channel_identity"].is_null());
    assert_eq!(files[0].envelope["artifacts"], json!({}));
}

#[test]
fn refuses_to_overwrite_a_published_file_with_different_bytes() {
    let fixture = Fixture::new("conflict");
    fixture.enqueue_result("run-1", "recorded");
    let ledger = fixture.task_dir().join("ledger");
    std::fs::create_dir_all(&ledger).unwrap();
    std::fs::write(ledger.join("000001-result.md"), b"someone else's bytes").unwrap();

    let error = flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap_err();
    assert!(error.contains("different content"), "{error}");
    assert_eq!(
        std::fs::read(ledger.join("000001-result.md")).unwrap(),
        b"someone else's bytes"
    );
    assert_eq!(
        fixture.db.pending_ledger_entries("task-1").unwrap().len(),
        1
    );
    assert!(fixture
        .db
        .ledger_pending_error("task-1")
        .unwrap()
        .is_some_and(|error| error.contains("different content")));
    assert_eq!(fixture.event_count("run.finished"), 0);
}

#[test]
fn crash_after_sql_commit_before_publication_is_recovered_once() {
    let fixture = Fixture::new("crash-before-publish");
    fixture.enqueue_result("run-1", "accepted");
    inject_fault(&fixture.root(), FlushFault::BeforePublish(1));
    assert!(flush_task(&fixture.db, &fixture.db_path, "task-1").is_err());
    assert!(fixture.ledger_names().is_empty());
    assert_eq!(fixture.event_count("run.finished"), 0);

    // A new server generation opening the same database recovers it.
    let reopened = Db::open(&fixture.db_path).unwrap();
    recover_on_startup(&reopened, &fixture.db_path);
    assert_eq!(fixture.ledger_names(), vec!["000001-result.md"]);
    assert_eq!(fixture.event_count("run.finished"), 1);
    assert!(fixture
        .db
        .pending_ledger_entries("task-1")
        .unwrap()
        .is_empty());
}

#[test]
fn crash_after_publication_before_acknowledgement_does_not_duplicate() {
    let fixture = Fixture::new("crash-after-publish");
    fixture.enqueue_result("run-1", "accepted");
    inject_fault(&fixture.root(), FlushFault::AfterPublishBeforeAck(1));
    assert!(flush_task(&fixture.db, &fixture.db_path, "task-1").is_err());
    // The file is there but not acknowledged: the entry is still pending and
    // its announcement still held.
    assert_eq!(fixture.ledger_names(), vec!["000001-result.md"]);
    assert_eq!(
        fixture.db.pending_ledger_entries("task-1").unwrap().len(),
        1
    );
    assert_eq!(fixture.event_count("run.finished"), 0);

    let outcome = flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    assert_eq!(outcome.published, 1);
    assert_eq!(fixture.ledger_names(), vec!["000001-result.md"]);
    assert_eq!(fixture.event_count("run.finished"), 1);
    // Acknowledging again releases nothing twice.
    assert!(!fixture.db.acknowledge_ledger_entry("task-1", 1).unwrap());
    assert_eq!(fixture.event_count("run.finished"), 1);
}

#[test]
fn a_combined_operation_is_not_continued_until_every_entry_is_published() {
    let fixture = Fixture::new("combined");
    let result = fixture.enqueue_result("run-plan", "plan");
    fixture
        .db
        .with_immediate_transaction(|db| {
            db.enqueue_ledger_entry(NewLedgerEntry {
                task_id: "task-1",
                kind: LedgerEntryKind::Plan,
                operation_id: Some(&result.operation_id),
                source_kind: "workflow_replacement",
                source_id: "plan-1",
                source_origin: None,
                historical: false,
                recorded_at: None,
                run_id: None,
                declared_role: None,
                body: json!({ "result_id": result.entry_id }),
                message: None,
                hold_events_after: None,
                reserved_sequence: None,
            })?;
            db.put_ledger_continuation(
                "task-1",
                &result.operation_id,
                crate::db::task_store::STAGE_COMPLETION_CONTINUATION,
                &json!({ "kind": "main" }),
            )
        })
        .unwrap();
    // Crash between the two files of one operation.
    inject_fault(&fixture.root(), FlushFault::BeforePublish(2));
    assert!(flush_task(&fixture.db, &fixture.db_path, "task-1").is_err());
    assert_eq!(fixture.ledger_names(), vec!["000001-result.md"]);
    assert!(fixture
        .db
        .claim_ledger_continuation("task-1")
        .unwrap()
        .is_none());
    assert!(fixture.db.has_ledger_continuation("task-1").unwrap());

    flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    assert_eq!(
        fixture.ledger_names(),
        vec!["000001-result.md", "000002-plan.json"]
    );
    let claimed = fixture.db.claim_ledger_continuation("task-1").unwrap();
    assert_eq!(claimed.unwrap().operation_id, result.operation_id);
    // Claimed exactly once.
    assert!(fixture
        .db
        .claim_ledger_continuation("task-1")
        .unwrap()
        .is_none());
    let files = read_ledger(&fixture.task_dir()).unwrap();
    assert_eq!(
        files[1].envelope["operation_id"],
        files[0].envelope["operation_id"]
    );
    assert_eq!(files[1].body()["result_id"], "task-1-000001");
}

#[test]
fn a_reservation_holds_later_entries_back_until_it_is_filled_or_released() {
    let fixture = Fixture::new("reservation");
    let reserved = fixture.db.reserve_ledger_sequence("task-1").unwrap();
    assert_eq!(reserved, 1);
    fixture.enqueue_result("run-input-like", "later");
    let outcome = flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    assert!(outcome.waiting_on_reservation);
    assert!(fixture.ledger_names().is_empty());

    fixture
        .db
        .enqueue_ledger_entry(NewLedgerEntry {
            task_id: "task-1",
            kind: LedgerEntryKind::Result,
            operation_id: None,
            source_kind: "stage_run",
            source_id: "run-review",
            source_origin: None,
            historical: false,
            recorded_at: None,
            run_id: Some("run-review"),
            declared_role: None,
            body: json!({ "status": "failure" }),
            message: Some("findings"),
            hold_events_after: None,
            reserved_sequence: Some(reserved),
        })
        .unwrap();
    flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    assert_eq!(
        fixture.ledger_names(),
        vec!["000001-result.md", "000002-result.md"]
    );

    // A released reservation is a gap, and the startup sweep drops orphans.
    let orphan = fixture.db.reserve_ledger_sequence("task-1").unwrap();
    assert_eq!(fixture.db.release_stale_ledger_reservations().unwrap(), 1);
    assert_eq!(orphan, 3);
    fixture.enqueue_result("run-after-gap", "after");
    flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    assert_eq!(fixture.ledger_names().last().unwrap(), "000003-result.md");
}

#[test]
fn task_json_is_replaced_atomically_with_prompt_workflow_and_links() {
    let fixture = Fixture::new("snapshot");
    fixture
        .db
        .insert_test_pipeline_item(
            "task-parent",
            "repo-1",
            "Parent",
            None,
            "plan",
            "2026-09-22 00:00:00",
        )
        .unwrap();
    fixture
        .db
        .update_pipeline_item_parent("task-1", Some("task-parent"))
        .unwrap();
    fixture
        .db
        .insert_task_blocker("task-1", "task-parent")
        .unwrap();
    fixture
        .db
        .update_pipeline_item_pr("task-1", Some(7), "https://example.com/acme/repo/pull/7")
        .unwrap();
    flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    let snapshot: Value =
        serde_json::from_slice(&std::fs::read(fixture.task_dir().join("task.json")).unwrap())
            .unwrap();
    assert_eq!(snapshot["origin_prompt"], "Build the ledger");
    assert_eq!(snapshot["links"]["parent"], "task-parent");
    assert_eq!(snapshot["links"]["dependencies"], json!(["task-parent"]));
    assert_eq!(snapshot["links"]["pr"]["number"], 7);
    assert_eq!(snapshot["stage"], "in progress");
    assert!(snapshot["owning_machine"].is_null());
    let (revision, published) = fixture
        .db
        .task_snapshot_revisions("task-1")
        .unwrap()
        .unwrap();
    assert_eq!(revision, published);

    // A failed rewrite leaves the previous snapshot whole.
    fixture
        .db
        .update_pipeline_item_display_name("task-1", Some("Renamed"))
        .unwrap();
    inject_fault(&fixture.root(), FlushFault::BeforeSnapshot);
    assert!(flush_task(&fixture.db, &fixture.db_path, "task-1").is_err());
    let unchanged: Value =
        serde_json::from_slice(&std::fs::read(fixture.task_dir().join("task.json")).unwrap())
            .unwrap();
    assert_eq!(unchanged["title"], "Ledger");
    flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    let renamed: Value =
        serde_json::from_slice(&std::fs::read(fixture.task_dir().join("task.json")).unwrap())
            .unwrap();
    assert_eq!(renamed["title"], "Renamed");
    let leftovers = std::fs::read_dir(fixture.task_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
        .count();
    assert_eq!(leftovers, 0);
}

fn seed_history(db: &Db) {
    let run = |id: &'static str, stage: &'static str| crate::db::NewStageRun {
        id,
        task_id: "task-1",
        stage,
        kind: "main",
        agent: None,
        agent_provider: None,
        model: None,
        effort: None,
        status: "running",
        result: None,
        feedback: None,
        session_id: None,
        provider_session_id: None,
        cwd: None,
        resumed_from_run_id: None,
    };
    db.insert_stage_run(run("run-plan", "plan")).unwrap();
    db.finish_stage_run(
        "run-plan",
        "succeeded",
        Some(&json!({"status": "success", "summary": "the plan", "metadata": null}).to_string()),
        Some("the plan"),
    )
    .unwrap();
    db.insert_stage_run(run("run-legacy", "plan")).unwrap();
    db.finish_stage_run(
        "run-legacy",
        "succeeded",
        Some("free-form legacy result"),
        None,
    )
    .unwrap();
    db.insert_stage_run(run("run-interrupted", "in progress"))
        .unwrap();
    db.finish_stage_run_without_work(
        "run-interrupted",
        "failed",
        None,
        None,
        crate::db::no_work_termination::SESSION_INTERRUPTED,
    )
    .unwrap();
    db.record_task_input(
        "task-1",
        crate::db::TaskInputSource::Operator,
        &crate::mutation_provenance::ChannelIdentity::Unknown,
        "owner says: keep it small",
    )
    .unwrap();
    db.append_task_event(
        "task-1",
        crate::db::TaskEventKind::StageChanged,
        json!({"fromStage": "plan", "toStage": "in progress", "branch": null, "trigger": "operator"}),
    )
    .unwrap();
}

#[test]
fn backfill_imports_available_history_once_and_honestly() {
    let fixture = Fixture::new("backfill");
    // History recorded before the bridge: drop what live mirroring queued,
    // as a pre-bridge database would not have it.
    seed_history(&fixture.db);
    fixture
        .db
        .delete_ledger_entries_for_tests("task-1")
        .unwrap();

    recover_on_startup(&fixture.db, &fixture.db_path);
    let files = read_ledger(&fixture.task_dir()).unwrap();
    let kinds = files.iter().map(|file| file.kind).collect::<Vec<_>>();
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    assert!(files.iter().all(|file| file.envelope["historical"] == true));
    let plan = files
        .iter()
        .find(|file| file.envelope["source"]["id"] == "run-plan")
        .unwrap();
    assert_eq!(plan.message.as_deref(), Some("the plan"));
    assert!(plan.body()["committed_sha"].is_null());
    assert_eq!(plan.body()["provenance"]["committed_sha"], "unknown");
    let legacy = files
        .iter()
        .find(|file| file.envelope["source"]["id"] == "run-legacy")
        .unwrap();
    assert_eq!(legacy.body()["legacy_format"], true);
    assert_eq!(legacy.message.as_deref(), Some("free-form legacy result"));
    // A no-verdict run ending is not a result.
    assert!(!files
        .iter()
        .any(|file| file.envelope["source"]["id"] == "run-interrupted"));
    let transition = files
        .iter()
        .find(|file| file.kind == LedgerEntryKind::Transition)
        .unwrap();
    assert!(transition.body()["triggering_result_id"].is_null());
    assert_eq!(transition.envelope["declared_role"], "operator");
    assert!(transition.envelope["channel_identity"].is_null());
    let input = files
        .iter()
        .find(|file| file.kind == LedgerEntryKind::Input)
        .unwrap();
    assert_eq!(input.message.as_deref(), Some("owner says: keep it small"));

    // A second restart imports nothing and writes nothing.
    recover_on_startup(&Db::open(&fixture.db_path).unwrap(), &fixture.db_path);
    assert_eq!(read_ledger(&fixture.task_dir()).unwrap().len(), 4);
    assert_eq!(fixture.db.backfill_task_ledger("task-1").unwrap(), 0);
}

#[test]
fn live_stage_changes_record_their_trigger_and_never_borrow_one() {
    let fixture = Fixture::new("transition-trigger");
    let result = fixture.enqueue_result("run-1", "done");
    fixture
        .db
        .update_pipeline_item_stage_with_trigger(
            "task-1",
            "review",
            crate::db::StageTrigger::Auto,
            &crate::mutation_provenance::ChannelIdentity::Unknown,
        )
        .unwrap();
    // Manual advance with no result since the previous transition.
    fixture
        .db
        .update_pipeline_item_stage_with_trigger(
            "task-1",
            "pr",
            crate::db::StageTrigger::Operator,
            &crate::mutation_provenance::ChannelIdentity::Unknown,
        )
        .unwrap();
    // A rewrite to the same stage is not a transition.
    fixture
        .db
        .update_pipeline_item_stage_with_trigger(
            "task-1",
            "pr",
            crate::db::StageTrigger::Operator,
            &crate::mutation_provenance::ChannelIdentity::Unknown,
        )
        .unwrap();
    // One event per real transition, appended with the stage write.
    assert_eq!(fixture.event_count("stage.changed"), 2);
    flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    let files = read_ledger(&fixture.task_dir()).unwrap();
    let transitions = files
        .iter()
        .filter(|file| file.kind == LedgerEntryKind::Transition)
        .collect::<Vec<_>>();
    assert_eq!(transitions.len(), 2);
    assert_eq!(
        transitions[0].body()["triggering_result_id"],
        result.entry_id
    );
    assert!(transitions[1].body()["triggering_result_id"].is_null());
    assert_eq!(transitions[1].envelope["declared_role"], "operator");
    assert!(transitions[1].body()["exit"].is_null());
}

#[test]
fn a_following_session_is_given_the_result_that_caused_it() {
    let fixture = Fixture::new("delivery");
    let result = fixture.enqueue_result("run-1", "implemented X\n\nNext stage: check Y");
    flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    let dir = fixture.task_dir();

    // Entering review: the result recorded since the last transition.
    let trigger = resolve_trigger(&dir, "review").unwrap();
    assert_eq!(trigger.entry_id, result.entry_id);
    assert_eq!(trigger.file, "ledger/000001-result.md");
    assert_eq!(trigger.message, "implemented X\n\nNext stage: check Y");

    fixture
        .db
        .update_pipeline_item_stage_with_trigger(
            "task-1",
            "review",
            crate::db::StageTrigger::Auto,
            &crate::mutation_provenance::ChannelIdentity::Unknown,
        )
        .unwrap();
    flush_task(&fixture.db, &fixture.db_path, "task-1").unwrap();
    // A rerun of review is caused by the same result.
    assert_eq!(
        resolve_trigger(&dir, "review").map(|trigger| trigger.entry_id),
        Some(result.entry_id.clone())
    );
    // A session of any other stage is not handed a result that did not cause it.
    assert!(resolve_trigger(&dir, "pr").is_none());

    // An explicitly identified pending trigger wins, and only inside its scope.
    let pending = TriggeringResult {
        entry_id: "task-1-000009".into(),
        file: "ledger/000009-result.md".into(),
        status: "failure".into(),
        stage: Some("review".into()),
        run_id: Some("run-review".into()),
        branch: None,
        committed_sha: None,
        message: "findings".into(),
    };
    let inside = with_pending_trigger(pending.clone(), || resolve_trigger(&dir, "in progress"));
    assert_eq!(inside, Some(pending));
    assert!(resolve_trigger(&dir, "in progress").is_none());
}

#[test]
fn entries_round_trip_and_marker_text_stays_literal() {
    let message = "---\n{{COMPLETION}} $& ${TASK_PROMPT}\n---\n\ntrailing";
    let bytes = crate::db::task_store::render_ledger_entry(
        &json!({ "entry_id": "t-000001" }),
        Some(message),
    );
    let parsed = parse_ledger_file("000001-result.md", &bytes).unwrap();
    assert_eq!(parsed.message.as_deref(), Some(message));
    assert_eq!(parsed.entry_id(), Some("t-000001"));

    let section = render_ledger_section(&SessionLedger {
        task_dir: "/ledger/task-1".into(),
        trigger: Some(TriggeringResult {
            entry_id: "task-1-000001".into(),
            file: "ledger/000001-result.md".into(),
            status: "success".into(),
            stage: Some("plan".into()),
            run_id: Some("run-1".into()),
            branch: Some("task-1".into()),
            committed_sha: Some("abc123".into()),
            message: message.into(),
        }),
    });
    assert!(section.contains(message));
    assert!(section.contains("commit `abc123`"));
    let none = render_ledger_section(&SessionLedger {
        task_dir: "/ledger/task-1".into(),
        trigger: None,
    });
    assert!(none.ends_with("No recorded result caused this session."));
}

#[test]
fn observing_a_workspace_reads_its_exact_commit_and_never_invents_one() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        observe_workspace(None).unwrap(),
        WorkspaceObservation::none()
    );
    assert_eq!(
        observe_workspace(Some(&temp.path().join("gone").to_string_lossy())).unwrap(),
        WorkspaceObservation::none()
    );
    let not_a_checkout = observe_workspace(Some(&temp.path().to_string_lossy())).unwrap();
    assert_eq!(not_a_checkout.provenance, "not_a_checkout");
    assert!(not_a_checkout.committed_sha.is_none());

    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-q", "-b", "task-1"]);
    let unborn = observe_workspace(Some(&repo.to_string_lossy())).unwrap();
    assert_eq!(unborn.provenance, "unborn");
    assert!(unborn.committed_sha.is_none());
    git(&[
        "-c",
        "user.email=t@example.com",
        "-c",
        "user.name=t",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "one",
    ]);
    let observed = observe_workspace(Some(&repo.to_string_lossy())).unwrap();
    assert_eq!(observed.branch.as_deref(), Some("task-1"));
    assert_eq!(observed.committed_sha.as_ref().map(String::len), Some(40));
    assert_eq!(observed.provenance, "workspace");
}
