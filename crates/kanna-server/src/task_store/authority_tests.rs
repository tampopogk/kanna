//! Focused tests for storage authority: the record, installation scoping,
//! the divergence verdicts, and a refused switch. The fixture installation
//! round trips (switch, restart at every checkpoint, rollback, rebuild,
//! recovery without replay) are in `http_api::tests::disk_authority`.

use super::*;
use crate::mutation_provenance::ChannelIdentity;
use crate::task_store::{flush_task, task_dir};
use rusqlite::OptionalExtension;

/// A migrated database under the production schema, with one repository
/// and `tasks` open tasks in it.
fn installation(label: &str, repo: &str, tasks: &[&str]) -> (Db, String) {
    let db_path = Db::test_db_path(&format!("authority-{label}"));
    let _ = std::fs::remove_file(&db_path);
    let db = Db::open_migrated(&db_path).unwrap();
    db.insert_test_repo_with_path(repo, &format!("/tmp/{repo}"), repo)
        .unwrap();
    for task in tasks {
        db.insert_test_pipeline_item(
            task,
            repo,
            &format!("Prompt of {task}"),
            Some(task),
            "in progress",
            "2026-09-23 09:00:00",
        )
        .unwrap();
        db.mark_task_ledger_backfilled(task, 0).unwrap();
    }
    (db, db_path)
}

#[test]
fn installation_ids_are_stable_and_distinct() {
    assert_eq!(
        installation_id("/a/kanna.db"),
        installation_id("/a/kanna.db")
    );
    assert_ne!(
        installation_id("/a/kanna.db"),
        installation_id("/b/kanna.db")
    );
    assert_eq!(installation_id("/a/kanna.db").len(), 16);
}

#[test]
fn no_record_means_sql_and_a_request_is_persisted_for_the_next_start() {
    let (_db, db_path) = installation("request", "repo-r", &[]);
    let root = root_for_db(&db_path);
    let record = load_record(&root, &db_path).unwrap();
    assert_eq!(record.mode, Mode::Sql);
    assert_eq!(record.requested, None);
    assert!(!record_path(&root, &record.installation).exists());

    let requested = request(&db_path, Mode::Disk).unwrap();
    assert_eq!(requested.requested, Some(Mode::Disk));
    assert_eq!(load_record(&root, &db_path).unwrap(), requested);
    // Asking for the mode it is already in asks for nothing.
    assert_eq!(request(&db_path, Mode::Sql).unwrap().requested, None);
}

#[test]
fn an_unreadable_or_foreign_record_is_refused_never_guessed() {
    let (_db, db_path) = installation("unreadable", "repo-u", &[]);
    let root = root_for_db(&db_path);
    let path = record_path(&root, &installation_id(&db_path));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut record = AuthorityRecord::new(&db_path);
    record.schema_version = 2;
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(load_record(&root, &db_path)
        .unwrap_err()
        .contains("schema_version 2"));
    let mut record = AuthorityRecord::new(&db_path);
    record.installation = "someone-else".into();
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(load_record(&root, &db_path)
        .unwrap_err()
        .contains("not this database's"));
    std::fs::write(&path, b"{").unwrap();
    assert!(load_record(&root, &db_path).is_err());
}

/// Production and staging both publish under `~/.kanna`: each rebuilds and
/// reconciles only the repositories it stamped, and switching one to disk
/// authority never takes in the other's tasks.
#[test]
fn installations_sharing_a_root_never_take_in_each_others_tasks() {
    let (production, production_path) = installation("shared-prod", "repo-prod", &["p1", "p2"]);
    let (staging, staging_path) = installation("shared-staging", "repo-staging", &["s1"]);
    let shared = PathBuf::from(format!("{production_path}.shared-root"));
    for path in [&production_path, &staging_path] {
        super::super::ROOTS
            .lock()
            .unwrap()
            .insert(path.clone(), shared.clone());
        register(&shared, path);
    }
    // One process per installation in production; here each flush stamps
    // under whichever registered last, so register before each.
    register(&shared, &production_path);
    assert!(flush_all(&production, &production_path).is_empty());
    let outcome = start(&production, &production_path, Some(Mode::Disk)).unwrap();
    assert_eq!(outcome.mode, Mode::Disk, "{:?}", outcome.refused);
    register(&shared, &staging_path);
    assert!(flush_all(&staging, &staging_path).is_empty());

    let (scoped, foreign) = scan_store_records(&shared)
        .unwrap()
        .for_installation(&installation_id(&production_path));
    let tasks: Vec<&str> = scoped
        .tasks
        .iter()
        .map(|dir| dir.snapshot.task_id.as_str())
        .collect();
    assert_eq!(tasks, ["p1", "p2"]);
    assert_eq!(foreign, ["repo-staging"]);

    // The production database is lost: its rebuild holds its own tasks only.
    drop(production);
    std::fs::remove_file(&production_path).unwrap();
    let report = rebuild_missing_database(&production_path).unwrap();
    assert_eq!(report.tasks, 2, "{report:?}");
    let rebuilt = Db::open(&production_path).unwrap();
    assert!(rebuilt.get_pipeline_item("s1").unwrap().is_none());
    assert!(rebuilt.get_pipeline_item("p1").unwrap().is_some());
    register(&shared, &production_path);
    let outcome = start(&rebuilt, &production_path, None).unwrap();
    let report = outcome.reconcile.unwrap();
    assert!(
        report.reconciled.is_empty() && report.failed.is_empty(),
        "{report:?}"
    );
    assert_eq!(report.foreign_repos, ["repo-staging"]);
    drop(staging);
}

fn compared(sql: Option<(i64, i64)>, disk_revision: i64, state_equal: bool) -> Compared {
    Compared {
        sql,
        disk_revision,
        has_state: true,
        state_equal,
        ..Compared::default()
    }
}

#[test]
fn the_disk_is_ahead_only_when_it_holds_what_the_database_never_reached() {
    // The database is ahead: its outbox and task.json are owed.
    let mut sql_ahead = compared(Some((7, 5)), 6, false);
    sql_ahead.pending = vec![9];
    assert_eq!(verdict(&sql_ahead), Verdict::InSync);
    // A restored older database: task.json is at a revision it never
    // reached, with other rows.
    assert!(matches!(
        verdict(&compared(Some((4, 4)), 9, false)),
        Verdict::DiskAhead(_)
    ));
    // The same revision with other rows: changed on disk.
    assert!(matches!(
        verdict(&compared(Some((4, 4)), 4, false)),
        Verdict::DiskAhead(_)
    ));
    // Ledger entries the database lacks, or holds with other bytes.
    let mut entries = compared(Some((4, 4)), 4, true);
    entries.disk_only = vec![3];
    assert!(matches!(verdict(&entries), Verdict::DiskAhead(_)));
    let mut entries = compared(Some((4, 4)), 4, true);
    entries.differing = vec![2];
    assert!(matches!(verdict(&entries), Verdict::DiskAhead(_)));
    // Identical rows at a revision the database never reached: counters.
    assert_eq!(
        verdict(&compared(Some((4, 4)), 9, true)),
        Verdict::CountersBehind(9)
    );
    // The disk lost what the database published.
    let mut lost = compared(Some((4, 4)), 4, true);
    lost.published_missing = vec![1, 2];
    assert_eq!(verdict(&lost), Verdict::DiskBehind(vec![1, 2]));
    assert_eq!(
        verdict(&compared(Some((6, 6)), 3, true)),
        Verdict::DiskBehind(Vec::new())
    );
    // Not in the database at all.
    assert!(matches!(
        verdict(&compared(None, 3, false)),
        Verdict::DiskAhead(_)
    ));
    let mut legacy = compared(None, 3, false);
    legacy.has_state = false;
    assert!(matches!(verdict(&legacy), Verdict::Unreconcilable(_)));
}

/// A task directory the database does not account for refuses the switch:
/// the installation stays `sql`, the request stays, and the next startup
/// switches once the cause is gone.
#[test]
fn a_switch_that_cannot_be_verified_is_refused_and_retried() {
    let (db, db_path) = installation("refused", "repo-f", &["t1"]);
    db.record_task_input(
        "t1",
        crate::db::TaskInputSource::Operator,
        &ChannelIdentity::Unknown,
        "hello",
    )
    .unwrap();
    assert!(flush_all(&db, &db_path).is_empty());
    let root = root_for_db(&db_path);
    let stray = task_dir(&root, "repo-f", "stray");
    std::fs::create_dir_all(&stray).unwrap();
    let mut snapshot: Value = serde_json::from_slice(
        &std::fs::read(task_dir(&root, "repo-f", "t1").join("task.json")).unwrap(),
    )
    .unwrap();
    snapshot["task_id"] = json!("stray");
    snapshot["state"] = json!({ "version": 1, "tables": {} });
    std::fs::write(
        stray.join("task.json"),
        serde_json::to_vec(&snapshot).unwrap(),
    )
    .unwrap();

    let outcome = start(&db, &db_path, Some(Mode::Disk)).unwrap();
    assert_eq!(outcome.mode, Mode::Sql);
    assert!(
        outcome.refused.iter().any(|problem| problem
            .contains("task directory stray (repo repo-f) is not a task of this database")),
        "{:?}",
        outcome.refused
    );
    let record = load_record(&root, &db_path).unwrap();
    assert_eq!(record.mode, Mode::Sql);
    assert_eq!(record.requested, Some(Mode::Disk));
    assert!(record.switch.is_none());
    assert_eq!(
        record.checkpoints.last().unwrap().checkpoint,
        "to_disk.refused"
    );
    assert_eq!(mode_for_root(&root), Mode::Sql);

    std::fs::remove_dir_all(&stray).unwrap();
    let outcome = start(&db, &db_path, None).unwrap();
    assert_eq!(outcome.mode, Mode::Disk, "{:?}", outcome.refused);
    assert_eq!(mode_for_root(&root), Mode::Disk);
    assert_eq!(load_record(&root, &db_path).unwrap().requested, None);
}

/// A closed task that predates the disk records has no `task.json`; the
/// switch owes it one, so a rebuild from disk restores closed tasks too.
#[test]
fn the_switch_publishes_closed_tasks_that_were_never_on_disk() {
    let (db, db_path) = installation("closed", "repo-c", &["open-1", "closed-1"]);
    db.connection_for_e2e_tests()
        .execute_batch(
            "UPDATE pipeline_item SET closed_at = '2026-09-01 00:00:00' WHERE id = 'closed-1';
             UPDATE task_ledger_snapshot SET revision = 0, published_revision = 0
             WHERE task_id = 'closed-1';",
        )
        .unwrap();
    assert!(flush_all(&db, &db_path).is_empty());
    let root = root_for_db(&db_path);
    assert!(!task_dir(&root, "repo-c", "closed-1")
        .join("task.json")
        .exists());
    let outcome = start(&db, &db_path, Some(Mode::Disk)).unwrap();
    assert_eq!(outcome.mode, Mode::Disk, "{:?}", outcome.refused);
    assert!(task_dir(&root, "repo-c", "closed-1")
        .join("task.json")
        .exists());
}

/// A removal the database committed, whose tombstone the disk does not
/// hold yet, stands: the stale `task.json` does not bring the task back.
#[test]
fn a_committed_removal_is_not_undone_by_a_stale_task_json() {
    let (db, db_path) = installation("removal", "repo-x", &["keep", "gone"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    db.delete_task_creation_artifacts("gone").unwrap();
    let task_json = task_dir(&root_for_db(&db_path), "repo-x", "gone").join("task.json");
    assert!(!super::super::rebuild::is_tombstone(
        &std::fs::read(&task_json).unwrap()
    ));

    let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert!(
        report.reconciled.is_empty() && report.failed.is_empty(),
        "{report:#?}"
    );
    assert!(db.get_pipeline_item("gone").unwrap().is_none());
    assert!(super::super::rebuild::is_tombstone(
        &std::fs::read(&task_json).unwrap()
    ));
}

/// Records the disk lost (a restored or damaged store) are written again
/// from what the database published, byte for byte.
#[test]
fn records_the_disk_lost_are_written_again() {
    let (db, db_path) = installation("lost", "repo-l", &["t1"]);
    db.record_task_input(
        "t1",
        crate::db::TaskInputSource::Operator,
        &ChannelIdentity::Unknown,
        "kept in the database",
    )
    .unwrap();
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    let dir = task_dir(&root_for_db(&db_path), "repo-l", "t1");
    let ledger: Vec<(std::ffi::OsString, Vec<u8>)> = std::fs::read_dir(dir.join("ledger"))
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect();
    assert!(!ledger.is_empty());
    std::fs::remove_dir_all(dir.join("ledger")).unwrap();
    std::fs::remove_file(dir.join("task.json")).unwrap();

    let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert_eq!(report.republished, ["t1"], "{report:#?}");
    for (name, bytes) in &ledger {
        assert_eq!(
            &std::fs::read(dir.join("ledger").join(name)).unwrap(),
            bytes
        );
    }
    assert!(dir.join("task.json").exists());
    let again = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert!(again.republished.is_empty(), "{again:#?}");
}

// ---------------------------------------------------------------------------
// Review round 1
// ---------------------------------------------------------------------------

/// A copy of the database as it stands, to restore later: the database
/// goes back while the disk keeps what happened since.
fn backup(db: &Db, db_path: &str) -> String {
    let copy = format!("{db_path}.backup");
    let _ = std::fs::remove_file(&copy);
    db.connection_for_e2e_tests()
        .execute("VACUUM main INTO ?1", [&copy])
        .unwrap();
    copy
}

fn restore(db_path: &str, copy: &str) -> Db {
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{suffix}"));
    }
    std::fs::copy(copy, db_path).unwrap();
    Db::open(db_path).unwrap()
}

fn operator_input(db: &Db, task: &str, text: &str) -> i64 {
    db.record_task_input(
        task,
        crate::db::TaskInputSource::Operator,
        &ChannelIdentity::Unknown,
        text,
    )
    .unwrap()
    .unwrap()
    .id
}

fn revision(db: &Db, task: &str) -> i64 {
    db.task_snapshot_revisions(task).unwrap().unwrap().0
}

/// Disk mode, with `t1`'s `task.json` renamed to "on disk" at a revision
/// the restored database never reached, and flagged by a flush.
fn flagged_installation(label: &str) -> (Db, String, PathBuf) {
    let (db, db_path) = installation(label, &format!("repo-{label}"), &["t1"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    let copy = backup(&db, &db_path);
    db.connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET display_name = 'on disk' WHERE id = 't1'",
            [],
        )
        .unwrap();
    // Two revisions ahead, so the restored database's next write does not
    // simply reach it.
    db.mark_task_snapshot_dirty("t1").unwrap();
    assert!(flush_all(&db, &db_path).is_empty());
    drop(db);
    let db = restore(&db_path, &copy);
    let root = root_for_db(&db_path);
    db.mark_task_snapshot_dirty("t1").unwrap();
    let error = flush_task(&db, &db_path, "t1").unwrap_err();
    assert!(error.contains("beyond the database's"), "{error}");
    assert!(is_diverged(&root, "t1"));
    let task_json = task_dir(&root, &format!("repo-{label}"), "t1").join("task.json");
    (db, db_path, task_json)
}

fn display_name(db: &Db) -> Option<String> {
    db.get_pipeline_item("t1").unwrap().unwrap().display_name
}

/// Finding 1: once flagged, a task publishes nothing, even after a live
/// write raises its revision past the disk's, until the repair has run;
/// the repair then takes the disk's rows.
#[test]
fn a_flagged_task_publishes_nothing_until_it_is_repaired() {
    let (db, db_path, task_json) = flagged_installation("fence");
    let root = root_for_db(&db_path);
    let on_disk = std::fs::read(&task_json).unwrap();
    let disk_revision = serde_json::from_slice::<Value>(&on_disk).unwrap()["snapshot_revision"]
        .as_i64()
        .unwrap();
    db.connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET display_name = 'live write' WHERE id = 't1'",
            [],
        )
        .unwrap();
    while revision(&db, "t1") <= disk_revision + 1 {
        db.mark_task_snapshot_dirty("t1").unwrap();
    }
    assert!(flush_task(&db, &db_path, "t1").is_err());
    assert!(!flush_all(&db, &db_path).is_empty());
    assert_eq!(std::fs::read(&task_json).unwrap(), on_disk);
    assert!(is_diverged(&root, "t1"));

    let report =
        reconcile_from_disk(&db, &db_path, Some(&BTreeSet::from(["t1".to_string()]))).unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    assert!(!is_diverged(&root, "t1"));
    assert_eq!(display_name(&db).as_deref(), Some("on disk"));
    assert!(flush_all(&db, &db_path).is_empty());
    let rewritten: Value = serde_json::from_slice(&std::fs::read(&task_json).unwrap()).unwrap();
    assert_eq!(rewritten["title"], "on disk");
}

/// Finding 2: a durable write outside the repair's lease, committed after
/// the comparison, aborts the repair (the flag stays) and survives it; an
/// active transfer claim also survives the repair that does run.
#[test]
fn a_write_after_the_comparison_survives_the_repair() {
    let (db, db_path, _) = flagged_installation("interleaved");
    let root = root_for_db(&db_path);
    let writer_path = db_path.clone();
    interleave_before_reconcile(&root, move || {
        Db::open(&writer_path)
            .unwrap()
            .connection_for_e2e_tests()
            .execute(
                "INSERT INTO task_transfer_workflow_claim (pipeline_item_id, transfer_id)
                 VALUES ('t1', 'xfer-live')",
                [],
            )
            .unwrap();
    });
    let only = BTreeSet::from(["t1".to_string()]);
    let report = reconcile_from_disk(&db, &db_path, Some(&only)).unwrap();
    assert!(report.reconciled.is_empty(), "{report:#?}");
    assert!(
        report
            .failed
            .iter()
            .any(|(task, error)| task == "t1" && error.contains(crate::db::CHANGED_SINCE_COMPARED)),
        "{report:#?}"
    );
    assert!(is_diverged(&root, "t1"));
    let claims = |db: &Db| -> i64 {
        db.connection_for_e2e_tests()
            .query_row(
                "SELECT COUNT(*) FROM task_transfer_workflow_claim
                 WHERE pipeline_item_id = 't1' AND transfer_id = 'xfer-live'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(claims(&db), 1);
    assert_ne!(display_name(&db).as_deref(), Some("on disk"));

    // The next pass compares again and repairs, keeping the live claim.
    let report = reconcile_from_disk(&db, &db_path, Some(&only)).unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    assert!(!is_diverged(&root, "t1"));
    assert_eq!(display_name(&db).as_deref(), Some("on disk"));
    assert_eq!(claims(&db), 1);
}

/// Finding 3: a restored backup handed task B the input id of task A's
/// disk-only input. Reconciling A never rewrites B's input, and still
/// recovers A's, under a new id.
#[test]
fn a_reused_input_id_never_rewrites_another_tasks_input() {
    let (db, db_path) = installation("input-collision", "repo-i", &["a", "b"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    let copy = backup(&db, &db_path);
    let a_id = operator_input(&db, "a", "A's input, on disk");
    assert!(flush_all(&db, &db_path).is_empty());
    drop(db);
    let db = restore(&db_path, &copy);
    let b_id = operator_input(&db, "b", "B's input");
    assert_eq!(a_id, b_id, "the restored database hands the id out again");
    assert!(flush_all(&db, &db_path).is_empty());

    let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert_eq!(
        report
            .reconciled
            .iter()
            .map(|(task, _)| task.as_str())
            .collect::<Vec<_>>(),
        ["a"],
        "{report:#?}"
    );
    let inputs: Vec<(i64, String, String)> = db
        .connection_for_e2e_tests()
        .prepare("SELECT id, task_id, message FROM task_input ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        inputs.contains(&(b_id, "b".into(), "B's input".into())),
        "{inputs:?}"
    );
    assert!(
        inputs.iter().any(|(id, task, message)| *id != b_id
            && task == "a"
            && message == "A's input, on disk"),
        "{inputs:?}"
    );
    assert!(report
        .diagnostics
        .iter()
        .any(|note| note.contains(&format!("input {a_id} is recovered as input"))));
    // A later startup changes nothing.
    let again = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert!(again.reconciled.is_empty(), "{again:#?}");
    let after: i64 = db
        .connection_for_e2e_tests()
        .query_row("SELECT COUNT(*) FROM task_input", [], |row| row.get(0))
        .unwrap();
    assert_eq!(after, 2);
}

/// Finding 4: a branch counter the database holds and the disk does not
/// (reserved after the disk's snapshot) survives reconciliation.
#[test]
fn a_counter_the_disk_does_not_hold_is_never_lowered() {
    let (db, db_path) = installation("counter", "repo-n", &["t1"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    let copy = backup(&db, &db_path);
    operator_input(&db, "t1", "on disk only");
    assert!(flush_all(&db, &db_path).is_empty());
    drop(db);
    let db = restore(&db_path, &copy);
    let number = db.reserve_task_branch_number("t1", 0).unwrap();
    assert!(number >= 1);

    let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    let counter: Option<i64> = db
        .connection_for_e2e_tests()
        .query_row(
            "SELECT last_allocated FROM task_branch_counter WHERE task_id = 't1'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(counter, Some(number));
    assert!(db.reserve_task_branch_number("t1", 0).unwrap() > number);
}

/// Finding 5: an explicit request for the mode in force withdraws a
/// pending request the other way; no cutover runs.
#[test]
fn an_explicit_request_for_the_current_mode_withdraws_a_pending_switch() {
    let (db, db_path) = installation("withdraw", "repo-w", &["t1"]);
    let root = root_for_db(&db_path);
    assert_eq!(
        request(&db_path, Mode::Disk).unwrap().requested,
        Some(Mode::Disk)
    );
    let outcome = start(&db, &db_path, Some(Mode::Sql)).unwrap();
    assert_eq!(outcome.mode, Mode::Sql);
    let record = load_record(&root, &db_path).unwrap();
    assert_eq!((record.mode, record.requested), (Mode::Sql, None));
    assert!(record.switch.is_none());
    assert!(record
        .checkpoints
        .iter()
        .all(|checkpoint| !checkpoint.checkpoint.starts_with("to_disk")));
    // And nothing is pending for the next start either.
    assert_eq!(start(&db, &db_path, None).unwrap().mode, Mode::Sql);
}
