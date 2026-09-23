//! Disk authority cutover, on copies of the migrated fixture installation
//! (spec §11, §16.11 — T13 third increment).
//!
//! The fixture is T13b's: a database created with the production
//! migrations and driven through the real endpoints and writers to every
//! representative open task shape. Each test copies it (database and task
//! store) to a new installation under the test root and exercises the
//! cutover there: the switch at a quiescent boundary, a restart at every
//! checkpoint, a rollback from every checkpoint, a database deleted and
//! rebuilt from disk, and an operation the disk holds and the database
//! lost, recovered without running it again.
use super::actions::{ledger_fixture_config, post_json, wait_for_running_task_stage};
use super::disk_rebuild::{build_fixture, differences, durable_state, files_under, Fixture};
use super::*;
use crate::mutation_provenance::ChannelIdentity;
use crate::task_store::authority::{self, AuthorityRecord, Mode, StartOutcome, INTERRUPTED};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The checkpoints of a switch to disk authority, in order.
const TO_DISK: &[&str] = &[
    "to_disk.begin",
    "to_disk.drained",
    "to_disk.repo_verified",
    "to_disk.verified",
    "to_disk.commit",
];

/// A copy of the fixture installation under a new database path: a new
/// installation, with the fixture's rows and records.
struct Installation {
    db_path: String,
    root: PathBuf,
    daemon_dir: PathBuf,
}

fn remove_database_files(db_path: &str) {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{suffix}"));
    }
}

fn copy_tree(from: &Path, to: &Path) {
    for (path, bytes) in files_under(from) {
        let target = to.join(path.strip_prefix(from).unwrap());
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
}

impl Installation {
    fn copy_of(fixture: &Fixture, label: &str) -> Self {
        let db_path = Db::test_db_path(&format!("authority-{label}"));
        remove_database_files(&db_path);
        fixture
            .db()
            .connection_for_e2e_tests()
            .execute("VACUUM main INTO ?1", [&db_path])
            .unwrap();
        let root = crate::task_store::root_for_db(&db_path);
        let _ = std::fs::remove_dir_all(&root);
        copy_tree(&crate::task_store::root_for_db(&fixture.db_path), &root);
        Self {
            db_path,
            root,
            daemon_dir: fixture.daemon_dir.clone(),
        }
    }

    fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    fn config(&self) -> crate::config::Config {
        let mut config = ledger_fixture_config("authority", &self.daemon_dir);
        config.db_path = self.db_path.clone();
        config
    }

    fn start(&self, request: Option<Mode>) -> Result<StartOutcome, String> {
        authority::start(&self.db(), &self.db_path, request)
    }

    fn record(&self) -> AuthorityRecord {
        authority::load_record(&self.root, &self.db_path).unwrap()
    }

    fn checkpoints(&self) -> Vec<String> {
        self.record()
            .checkpoints
            .into_iter()
            .map(|checkpoint| checkpoint.checkpoint)
            .collect()
    }

    /// Every file of the task and repository records: the ledger, every
    /// `task.json` and `repo.json`. The authority record is left out.
    fn records(&self) -> Vec<(PathBuf, Vec<u8>)> {
        files_under(&self.root.join("repos"))
    }

    fn ledger_files(&self) -> Vec<(PathBuf, Vec<u8>)> {
        self.records()
            .into_iter()
            .filter(|(path, _)| path.components().any(|part| part.as_os_str() == "ledger"))
            .collect()
    }

    fn backup(&self, label: &str) -> String {
        let backup = Db::test_db_path(&format!("authority-backup-{label}"));
        remove_database_files(&backup);
        self.db()
            .connection_for_e2e_tests()
            .execute("VACUUM main INTO ?1", [&backup])
            .unwrap();
        backup
    }

    /// The database goes back to an older copy; the disk keeps what
    /// happened since.
    fn restore(&self, backup: &str) {
        remove_database_files(&self.db_path);
        std::fs::copy(backup, &self.db_path).unwrap();
    }
}

fn assert_same_state(expected: &BTreeMap<String, Value>, db: &Db) {
    let differing = differences(expected, &durable_state(db));
    assert!(differing.is_empty(), "{}", differing.join("\n"));
}

fn assert_nothing_reconciled(outcome: &StartOutcome) {
    let report = outcome.reconcile.as_ref().expect("disk mode reconciles");
    assert!(
        report.reconciled.is_empty()
            && report.republished.is_empty()
            && report.removed.is_empty()
            && report.discarded_entries.is_empty()
            && report.failed.is_empty()
            && report.unreadable.is_empty(),
        "{report:#?}"
    );
}

/// The work restart reconciliation resumes: owed transitions, lifecycle
/// intents, commit steps, join members not yet created, dependents waiting
/// on edges, and transfers in flight.
fn restart_work(db: &Db) -> Value {
    let lifecycle: Vec<String> = db
        .list_lifecycle_operation_intents()
        .unwrap()
        .iter()
        .map(|intent| format!("{intent:?}"))
        .collect();
    let commit_step = format!(
        "{:?}",
        db.task_transition_commit("commit", "commit-run").unwrap()
    );
    let uncreated: Vec<String> = db
        .list_uncreated_join_members()
        .unwrap()
        .iter()
        .map(|member| format!("{member:?}"))
        .collect();
    let waiting: BTreeMap<String, Vec<String>> = db
        .list_open_stage_edge_dependents()
        .unwrap()
        .into_iter()
        .map(|task| {
            let upstreams = db.list_waiting_stage_edge_upstreams(&task).unwrap();
            (task, upstreams)
        })
        .collect();
    // The claim token and its lease are a capability that never reaches
    // disk (`rebuild::NOT_REBUILT`); recovery re-claims under a new one.
    let transfers: Vec<String> = ["incoming", "outgoing"]
        .iter()
        .flat_map(|task| db.list_task_transfers(task).unwrap())
        .map(|transfer| {
            format!(
                "{} {} {} {:?}",
                transfer.id, transfer.direction, transfer.status, transfer.local_task_id
            )
        })
        .collect();
    json!({
        "continuations": db.ledger_continuation_task_ids().unwrap(),
        "lifecycle": lifecycle,
        "commit_step": commit_step,
        "uncreated_members": uncreated,
        "waiting": waiting,
        "transfers": transfers,
    })
}

#[tokio::test]
async fn switching_to_disk_verifies_every_record_and_changes_no_row() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = build_fixture().await;
    let source = durable_state(&fixture.db());
    let copy = Installation::copy_of(&fixture, "switch");
    assert_eq!(copy.record().mode, Mode::Sql);
    let ledger = copy.ledger_files();

    let outcome = copy.start(Some(Mode::Disk)).unwrap();
    assert_eq!(outcome.mode, Mode::Disk, "{:#?}", outcome.refused);
    assert_nothing_reconciled(&outcome);
    let record = copy.record();
    assert_eq!((record.mode, record.requested), (Mode::Disk, None));
    assert!(record.switch.is_none());
    assert_eq!(copy.checkpoints(), TO_DISK);
    assert_eq!(record.checkpoints[2].detail["repo"], "repo-1");
    assert_eq!(record.checkpoints[4].detail["tasks"], 14);
    // The switch wrote no row and no ledger entry, and every repo.json now
    // names this installation.
    assert_same_state(&source, &copy.db());
    assert_eq!(copy.ledger_files(), ledger);
    let repo = crate::task_store::repo_dir(&copy.root, "repo-1").join("repo.json");
    let repo: Value = serde_json::from_slice(&std::fs::read(repo).unwrap()).unwrap();
    assert_eq!(
        repo["installation"],
        authority::installation_id(&copy.db_path)
    );
    assert!(copy.records().iter().all(
        |(_, bytes)| !String::from_utf8_lossy(bytes).contains(super::disk_rebuild::CLAIM_TOKEN)
    ));

    // A restart in disk mode finds the database and the disk in agreement.
    let records = copy.records();
    let again = copy.start(None).unwrap();
    assert_eq!(again.mode, Mode::Disk);
    assert_nothing_reconciled(&again);
    assert_eq!(copy.checkpoints(), TO_DISK);
    assert_same_state(&source, &copy.db());
    assert_eq!(copy.records(), records);
}

#[tokio::test]
async fn a_switch_killed_at_any_checkpoint_resumes_to_the_same_result() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = build_fixture().await;
    let source = durable_state(&fixture.db());
    for (index, checkpoint) in TO_DISK.iter().enumerate() {
        let copy = Installation::copy_of(&fixture, &format!("resume-{index}"));
        let ledger = copy.ledger_files();
        authority::stop_after(&copy.root, checkpoint);
        let interrupted = copy.start(Some(Mode::Disk)).unwrap_err();
        assert_eq!(interrupted, format!("{INTERRUPTED} {checkpoint}"));
        let record = copy.record();
        assert_eq!(record.checkpoints.last().unwrap().checkpoint, *checkpoint);
        let committed = *checkpoint == "to_disk.commit";
        assert_eq!(
            record.mode,
            if committed { Mode::Disk } else { Mode::Sql },
            "{checkpoint}"
        );
        assert_eq!(record.switch.is_some(), !committed, "{checkpoint}");

        // The restart has no request of its own: the record carries it.
        let outcome = copy.start(None).unwrap();
        assert_eq!(
            outcome.mode,
            Mode::Disk,
            "{checkpoint}: {:#?}",
            outcome.refused
        );
        assert_nothing_reconciled(&outcome);
        let names = copy.checkpoints();
        assert_eq!(
            names
                .iter()
                .filter(|name| *name == "to_disk.commit")
                .count(),
            1,
            "{checkpoint}: {names:?}"
        );
        assert_eq!(names.last().unwrap(), "to_disk.commit");
        assert_same_state(&source, &copy.db());
        assert_eq!(copy.ledger_files(), ledger, "{checkpoint}");
        assert!(copy.db().consistency_problems_for_tests().is_empty());
    }
}

#[tokio::test]
async fn rolling_back_to_sql_from_any_checkpoint_leaves_a_consistent_database() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = build_fixture().await;
    let source = durable_state(&fixture.db());
    // From every checkpoint of the switch, the last one being disk mode.
    for (index, checkpoint) in TO_DISK.iter().enumerate() {
        let copy = Installation::copy_of(&fixture, &format!("rollback-{index}"));
        authority::stop_after(&copy.root, checkpoint);
        copy.start(Some(Mode::Disk)).unwrap_err();
        let outcome = copy.start(Some(Mode::Sql)).unwrap();
        assert_eq!(outcome.mode, Mode::Sql, "{checkpoint}");
        let record = copy.record();
        assert_eq!((record.mode, record.requested), (Mode::Sql, None));
        assert!(record.switch.is_none());
        let names = copy.checkpoints();
        assert_eq!(names.last().unwrap(), "to_sql.commit", "{names:?}");
        // Only a rollback from disk mode has anything to take from disk.
        assert_eq!(
            names.contains(&"to_sql.reconciled".to_string()),
            *checkpoint == "to_disk.commit",
            "{names:?}"
        );
        let db = copy.db();
        assert!(db.consistency_problems_for_tests().is_empty());
        assert_same_state(&source, &db);
        assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
        // And it stays sql.
        assert_eq!(copy.start(None).unwrap().mode, Mode::Sql);
    }

    // From disk mode after it has run, with a rollback itself killed at
    // each of its checkpoints.
    for checkpoint in ["to_sql.begin", "to_sql.reconciled", "to_sql.commit"] {
        let copy = Installation::copy_of(&fixture, &format!("rollback-{checkpoint}"));
        assert_eq!(copy.start(Some(Mode::Disk)).unwrap().mode, Mode::Disk);
        let db = copy.db();
        db.record_task_input(
            "active",
            crate::db::TaskInputSource::Operator,
            &ChannelIdentity::Unknown,
            "written in disk mode",
        )
        .unwrap();
        assert!(crate::task_store::flush_all(&db, &copy.db_path).is_empty());
        let before = durable_state(&db);
        drop(db);
        authority::stop_after(&copy.root, checkpoint);
        copy.start(Some(Mode::Sql)).unwrap_err();
        assert_eq!(
            copy.record().mode,
            if checkpoint == "to_sql.commit" {
                Mode::Sql
            } else {
                Mode::Disk
            }
        );
        let outcome = copy.start(None).unwrap();
        assert_eq!(outcome.mode, Mode::Sql, "{checkpoint}");
        let db = copy.db();
        assert!(db.consistency_problems_for_tests().is_empty());
        assert_same_state(&before, &db);
        assert_eq!(copy.checkpoints().last().unwrap(), "to_sql.commit");
    }
}

#[tokio::test]
async fn a_deleted_database_is_rebuilt_from_disk_with_its_owed_work_intact() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = build_fixture().await;
    let pristine = Installation::copy_of(&fixture, "rebuild-pristine");
    assert_eq!(pristine.start(Some(Mode::Disk)).unwrap().mode, Mode::Disk);
    let copy = Installation::copy_of(&fixture, "rebuild");
    assert_eq!(copy.start(Some(Mode::Disk)).unwrap().mode, Mode::Disk);
    let before = durable_state(&copy.db());
    let work = restart_work(&copy.db());
    let records = copy.records();

    remove_database_files(&copy.db_path);
    // The server's own open path: the missing database is rebuilt from
    // this installation's records, migrated, and reconciled.
    let db = authority::open_database(&copy.config()).unwrap();
    assert_eq!(authority::mode_for_root(&copy.root), Mode::Disk);
    assert_same_state(&before, &db);
    assert_eq!(restart_work(&db), work);
    assert!(db.consistency_problems_for_tests().is_empty());
    // Nothing is owed, nothing was written again.
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
    assert!(db.repos_with_pending_disk_record().unwrap().is_empty());
    assert!(db.tasks_needing_ledger_backfill().unwrap().is_empty());
    assert!(crate::task_store::flush_all(&db, &copy.db_path).is_empty());
    assert_eq!(copy.records(), records);
    let restarted = copy.start(None).unwrap();
    assert_nothing_reconciled(&restarted);

    // The facts the representative tasks exist for.
    let state = durable_state(&db);
    let row = |table: &str, id: &str| {
        state
            .iter()
            .find(|(key, value)| key.starts_with(&format!("{table}|")) && value["id"] == id)
            .map(|(_, value)| value.clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(row("pipeline_item", "gate")["stage"], "in progress");
    assert_eq!(row("stage_run", "gate-run")["status"], "succeeded");
    assert_eq!(row("stage_run", "active-run")["status"], "running");
    assert_eq!(
        row("stage_run", "active-run")["session_branch"],
        "task-active-1"
    );
    assert_eq!(row("pipeline_item", "revision")["revision_rounds"], 1);
    assert_eq!(row("pipeline_item", "post")["stage"], "review");
    assert_eq!(row("pipeline_item", "exits")["stage"], "review");
    assert_eq!(row("task_transfer", "xfer-in")["status"], "claimed");
    assert!(db.get_pipeline_item("doomed").unwrap().is_none());
    assert!(db.has_ledger_continuation("joiner").unwrap());
    // The claimed incoming transfer is re-claimed by restart recovery, as
    // after a crash whose claim lease has lapsed.
    assert!(db
        .claim_pending_incoming_transfer("xfer-in", "recovery-token", true)
        .unwrap());

    // Resumed as a crash would resume them: the startup resumers do the
    // same on the rebuilt database as on the one that was never lost, and
    // send the daemon the same commands.
    let mut after = Vec::new();
    for installation in [&pristine, &copy] {
        let state = Arc::new(super::AppState::new(installation.config()));
        let commands = fixture.daemon_commands.lock().unwrap().len();
        crate::http_api::task_actions::resume_ledger_continuations(Arc::clone(&state)).await;
        crate::http_api::stage_dependencies::resume_stage_dependency_readiness(state).await;
        let sent = fixture.daemon_commands.lock().unwrap().len() - commands;
        after.push((durable_state(&installation.db()), sent));
    }
    let (pristine_after, rebuilt_after) =
        (generic(&after[0].0, &before), generic(&after[1].0, &before));
    let only_pristine: Vec<&String> = pristine_after
        .iter()
        .filter(|row| !rebuilt_after.contains(row))
        .collect();
    let only_rebuilt: Vec<&String> = rebuilt_after
        .iter()
        .filter(|row| !pristine_after.contains(row))
        .collect();
    assert!(
        only_pristine.is_empty() && only_rebuilt.is_empty(),
        "only after resuming the pristine database: {only_pristine:#?}\nonly after resuming the rebuilt one: {only_rebuilt:#?}"
    );
    assert_eq!(after[0].1, after[1].1);
    assert_ne!(
        after[0].0, before,
        "the resumers started the waiting dependent"
    );
}

/// A durable state with what a resumer generates afresh (run ids, provider
/// session ids, and the times in every row it wrote or changed since
/// `before`) replaced by a placeholder, so two databases that did the same work a
/// second apart compare equal.
fn generic(state: &BTreeMap<String, Value>, before: &BTreeMap<String, Value>) -> Vec<String> {
    let fresh: Vec<String> = state
        .values()
        .filter(|row| row["id"].as_str().is_some_and(|id| id.starts_with("run-")))
        .flat_map(|row| {
            [row["id"].as_str(), row["provider_session_id"].as_str()]
                .into_iter()
                .flatten()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    // SQLite's `datetime('now')` shape: `YYYY-MM-DD HH:MM:SS`.
    let is_time = |text: &str| {
        let bytes = text.as_bytes();
        text.len() == 19 && bytes[4] == b'-' && bytes[10] == b' ' && bytes[13] == b':'
    };
    let mut rows: Vec<String> = state
        .iter()
        .map(|(key, value)| {
            let mut value = value.clone();
            let written = before.get(key) != Some(&value);
            if let Some(row) = value.as_object_mut().filter(|_| written) {
                for column in row.values_mut() {
                    if column.as_str().is_some_and(is_time) {
                        *column = Value::String("<now>".into());
                    }
                }
            }
            let mut row = format!("{} {value}", key.split('|').next().unwrap_or(key));
            for id in &fresh {
                row = row.replace(id.as_str(), "<generated>");
            }
            row
        })
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn an_operation_on_disk_and_missing_from_the_database_is_recovered_without_replay() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = build_fixture().await;
    let copy = Installation::copy_of(&fixture, "recover");
    assert_eq!(copy.start(Some(Mode::Disk)).unwrap().mode, Mode::Disk);
    let backup = copy.backup("recover");

    // The operation: an operator advances the parked gate, which records
    // the transition and starts the reviewer's session.
    let before_commands = fixture.daemon_commands.lock().unwrap().len();
    {
        let state = Arc::new(super::AppState::new(copy.config()));
        let app = super::router(Arc::clone(&state));
        let (status, text) = post_json(
            &app,
            "/v1/tasks/gate/actions/advance-stage",
            json!({ "source": "operator" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        crate::http_api::wait_for_task_mutation_to_finish(&state, "gate").await;
        wait_for_running_task_stage(&copy.db(), "gate", "review").await;
    }
    assert!(crate::task_store::flush_all(&copy.db(), &copy.db_path).is_empty());
    let commands = fixture.daemon_commands.lock().unwrap().clone();
    let sent = &commands[before_commands..];
    assert!(
        sent.iter().any(|command| matches!(
            command,
            kanna_daemon::protocol::Command::Spawn { .. }
                | kanna_daemon::protocol::Command::SpawnAgent { .. }
        )),
        "the advance started the reviewer's session: {sent:?}"
    );
    let with_operation = durable_state(&copy.db());
    let ledger = copy.ledger_files();

    // The database loses it (restored from before); the disk keeps it.
    copy.restore(&backup);
    assert_eq!(
        copy.db()
            .get_pipeline_item("gate")
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("in progress")
    );
    let outcome = copy.start(None).unwrap();
    let report = outcome.reconcile.unwrap();
    assert_eq!(
        report
            .reconciled
            .iter()
            .map(|(task, _)| task.as_str())
            .collect::<Vec<_>>(),
        ["gate"],
        "{report:#?}"
    );
    assert!(report.reconciled[0]
        .1
        .iter()
        .any(|reason| reason.contains("on disk, not in the database")));
    assert!(report.discarded_entries.is_empty() && report.failed.is_empty());
    let db = copy.db();
    assert_same_state(&with_operation, &db);
    assert!(db.consistency_problems_for_tests().is_empty());
    // Recovered, not replayed: no entry recorded again, no session started
    // again, nothing owed.
    assert_eq!(copy.ledger_files(), ledger);
    assert_eq!(
        fixture.daemon_commands.lock().unwrap().len(),
        commands.len()
    );
    assert!(!db.has_ledger_continuation("gate").unwrap());
    {
        let state = Arc::new(super::AppState::new(copy.config()));
        crate::http_api::task_actions::resume_ledger_continuations(state).await;
    }
    assert_eq!(
        fixture.daemon_commands.lock().unwrap().len(),
        commands.len()
    );
    assert_same_state(&with_operation, &copy.db());
    // Idempotent.
    assert_nothing_reconciled(&copy.start(None).unwrap());
    assert_eq!(copy.ledger_files(), ledger);
}

#[tokio::test]
async fn the_publisher_finds_the_disk_ahead_and_the_disk_wins() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = build_fixture().await;
    let copy = Installation::copy_of(&fixture, "diverge");
    assert_eq!(copy.start(Some(Mode::Disk)).unwrap().mode, Mode::Disk);
    let backup = copy.backup("diverge");
    let input = |text: &str| {
        let db = copy.db();
        let recorded = db
            .record_task_input(
                "active",
                crate::db::TaskInputSource::Operator,
                &ChannelIdentity::Unknown,
                text,
            )
            .unwrap()
            .unwrap();
        (db, recorded)
    };
    let (db, first) = input("first, and on disk");
    assert!(crate::task_store::flush_all(&db, &copy.db_path).is_empty());
    drop(db);
    let with_first = durable_state(&copy.db());

    // A database restored while the server runs records another input at
    // the same sequence; publishing it finds the disk's entry there.
    copy.restore(&backup);
    let (db, second) = input("second, only in the database");
    let failures = crate::task_store::flush_all(&db, &copy.db_path);
    assert!(
        failures.iter().any(|(task, error)| task == "active"
            && error.contains("already exists with different content")),
        "{failures:?}"
    );
    assert_eq!(authority::diverged_tasks(&copy.root), ["active"]);

    // The publisher's reconciliation, under the task's lease.
    let state = Arc::new(super::AppState::new(copy.config()));
    crate::http_api::storage_authority::reconcile_diverged_tasks(state).await;
    assert!(authority::diverged_tasks(&copy.root).is_empty());
    let db = copy.db();
    assert!(crate::task_store::flush_all(&db, &copy.db_path).is_empty());
    assert_same_state(&with_first, &db);
    let inputs: Vec<String> = db
        .connection_for_e2e_tests()
        .prepare("SELECT message FROM task_input WHERE task_id = 'active'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(inputs.contains(&"first, and on disk".to_string()));
    assert!(!inputs.contains(&"second, only in the database".to_string()));
    assert_ne!(first.id, 0);
    let _ = second;

    // task.json at a revision the database never reached is reconciled
    // from, not written over.
    let task_json = crate::task_store::task_dir(&copy.root, "repo-1", "gate").join("task.json");
    let mut snapshot: Value = serde_json::from_slice(&std::fs::read(&task_json).unwrap()).unwrap();
    let revision = snapshot["snapshot_revision"].as_i64().unwrap();
    snapshot["snapshot_revision"] = json!(revision + 10);
    let ahead = serde_json::to_vec_pretty(&snapshot).unwrap();
    std::fs::write(&task_json, &ahead).unwrap();
    db.mark_task_snapshot_dirty("gate").unwrap();
    let error = crate::task_store::flush_task(&db, &copy.db_path, "gate").unwrap_err();
    assert!(error.contains("beyond the database's"), "{error}");
    assert_eq!(std::fs::read(&task_json).unwrap(), ahead);
    assert_eq!(authority::diverged_tasks(&copy.root), ["gate"]);
    let state = Arc::new(super::AppState::new(copy.config()));
    crate::http_api::storage_authority::reconcile_diverged_tasks(state).await;
    assert!(authority::diverged_tasks(&copy.root).is_empty());
    assert!(crate::task_store::flush_all(&db, &copy.db_path).is_empty());
    let rewritten: Value = serde_json::from_slice(&std::fs::read(&task_json).unwrap()).unwrap();
    assert!(rewritten["snapshot_revision"].as_i64().unwrap() > revision + 10);
    assert_same_state(&with_first, &db);
}
