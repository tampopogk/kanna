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
    record.schema_version = 3;
    std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(load_record(&root, &db_path)
        .unwrap_err()
        .contains("schema_version 3"));
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
    let failures = flush_all(&staging, &staging_path);
    assert!(failures.is_empty(), "{failures:?}");

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
    outside_writer(db.db_path())
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

/// A removal in `disk` mode writes its tombstone before the database
/// commits it (T13d), and a restart keeps it removed.
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
    assert!(super::super::rebuild::is_tombstone(
        &std::fs::read(&task_json).unwrap()
    ));
    assert!(db.pending_disk_removals().unwrap().is_empty());

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

/// A writer outside this build's disk-first gate (an older build, or a
/// person with `sqlite3`): it commits SQLite first whatever the mode, which
/// is how these fixtures put the database behind or ahead of the disk.
fn outside_writer(db_path: &str) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn
}

/// A copy of the database as it stands, to restore later: the database
/// goes back while the disk keeps what happened since.
fn backup(db: &Db, db_path: &str) -> String {
    let copy = format!("{db_path}.backup");
    let _ = std::fs::remove_file(&copy);
    outside_writer(db.db_path())
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

fn operator_input_result(
    db: &Db,
    task: &str,
    text: &str,
) -> Result<Option<crate::db::TaskInputRecord>, rusqlite::Error> {
    db.record_task_input(
        task,
        crate::db::TaskInputSource::Operator,
        &ChannelIdentity::Unknown,
        text,
    )
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
    outside_writer(db.db_path())
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
    // Disk-first (T13d): the restored database's next write finds the
    // disk ahead before anything is written, and is refused.
    let error = db.mark_task_snapshot_dirty("t1").unwrap_err().to_string();
    assert!(error.contains("beyond the database's"), "{error}");
    assert!(is_diverged(&db, "t1"));
    assert!(flush_task(&db, &db_path, "t1").is_err());
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
    let on_disk = std::fs::read(&task_json).unwrap();
    let disk_revision = serde_json::from_slice::<Value>(&on_disk).unwrap()["snapshot_revision"]
        .as_i64()
        .unwrap();
    outside_writer(db.db_path())
        .execute(
            "UPDATE pipeline_item SET display_name = 'live write' WHERE id = 't1'",
            [],
        )
        .unwrap();
    // This build refuses every write to the fenced task (T13d); only a
    // writer outside the gate can raise its revision past the disk's.
    assert!(db.mark_task_snapshot_dirty("t1").is_err());
    while revision(&db, "t1") <= disk_revision + 1 {
        outside_writer(db.db_path())
            .execute(
                "UPDATE task_ledger_snapshot SET revision = revision + 1 WHERE task_id = 't1'",
                [],
            )
            .unwrap();
    }
    assert!(flush_task(&db, &db_path, "t1").is_err());
    assert!(!flush_all(&db, &db_path).is_empty());
    assert_eq!(std::fs::read(&task_json).unwrap(), on_disk);
    assert!(is_diverged(&db, "t1"));

    let report =
        reconcile_from_disk(&db, &db_path, Some(&BTreeSet::from(["t1".to_string()]))).unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    assert!(!is_diverged(&db, "t1"));
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
        outside_writer(&writer_path)
            .execute_batch(
                "INSERT INTO task_transfer (id, direction, status, source_task_id, local_task_id)
                 VALUES ('xfer-live', 'outgoing', 'streaming', 't1', 't1');
                 INSERT INTO task_transfer_workflow_claim (pipeline_item_id, transfer_id)
                 VALUES ('t1', 'xfer-live');",
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
    assert!(is_diverged(&db, "t1"));
    let claims = |db: &Db| -> i64 {
        outside_writer(db.db_path())
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
    assert!(!is_diverged(&db, "t1"));
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
    let inputs: Vec<(i64, String, String)> = outside_writer(db.db_path())
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
    let after: i64 = outside_writer(db.db_path())
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
    // This build refuses the write (the disk is ahead, T13d); a counter the
    // database holds and the disk does not comes from outside the gate.
    assert!(db.reserve_task_branch_number("t1", 0).is_err());
    let number = 7;
    outside_writer(db.db_path())
        .execute(
            "INSERT INTO task_branch_counter (task_id, last_allocated) VALUES ('t1', ?1)",
            [number],
        )
        .unwrap();

    let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    let counter: Option<i64> = outside_writer(db.db_path())
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

// ---------------------------------------------------------------------------
// Review round 2
// ---------------------------------------------------------------------------

fn inputs(db: &Db) -> Vec<(i64, String, String)> {
    outside_writer(db.db_path())
        .prepare("SELECT id, task_id, message FROM task_input ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Disk mode; task `a` records `a_inputs` on disk only (the database is
/// restored from before them), then task `b` takes the first of their ids.
/// `before_flush` runs on the live database before its records reach disk.
fn restored_with_reused_input_id(
    label: &str,
    a_inputs: &[&str],
    before_flush: impl FnOnce(&Db, &[i64]),
) -> (Db, String, Vec<i64>, i64) {
    let (db, db_path) = installation(label, &format!("repo-{label}"), &["a", "b"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    let copy = backup(&db, &db_path);
    let a_ids: Vec<i64> = a_inputs
        .iter()
        .map(|text| operator_input(&db, "a", text))
        .collect();
    before_flush(&db, &a_ids);
    assert!(flush_all(&db, &db_path).is_empty());
    drop(db);
    let db = restore(&db_path, &copy);
    let b_id = operator_input(&db, "b", "B's input");
    assert_eq!(
        b_id, a_ids[0],
        "the restored database hands the id out again"
    );
    assert!(flush_all(&db, &db_path).is_empty());
    (db, db_path, a_ids, b_id)
}

/// Round 2, finding 1: two disk-only inputs, the first colliding. The moved
/// one never lands on the other's id: every projected id is reserved first.
#[test]
fn a_moved_input_never_lands_on_another_projected_input() {
    let (db, db_path, a_ids, b_id) =
        restored_with_reused_input_id("two-inputs", &["A first", "A second"], |_, _| {});
    let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    let rows = inputs(&db);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(
        rows.contains(&(b_id, "b".into(), "B's input".into())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&(a_ids[1], "a".into(), "A second".into())),
        "{rows:?}"
    );
    let moved = rows
        .iter()
        .find(|(_, task, message)| task == "a" && message == "A first")
        .expect("A's first input is recovered");
    assert!(moved.0 != b_id && moved.0 != a_ids[1], "{rows:?}");
}

/// Round 2, finding 2: a join member's delivered outcome names the input
/// that moved; it follows the input, so the parent's notice is its own.
#[test]
fn a_join_member_follows_its_moved_input() {
    let (db, db_path, a_ids, b_id) =
        restored_with_reused_input_id("join-input", &["the child's outcome"], |db, ids| {
            outside_writer(db.db_path())
                .execute_batch(&format!(
                    "INSERT INTO task_join (id, parent_task_id, base_sha)
                     VALUES ('join-a', 'a', 'sha');
                     INSERT INTO task_join_member
                        (join_id, position, child_task_id, spec, resolved_at, outcome, input_id)
                     VALUES ('join-a', 1, 'child-a', '{{}}', '2026-09-23T10:00:00Z',
                             'success', {});",
                    ids[0]
                ))
                .unwrap();
        });
    let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    let member: i64 = outside_writer(db.db_path())
        .query_row(
            "SELECT input_id FROM task_join_member WHERE join_id = 'join-a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(member, a_ids[0]);
    assert_ne!(member, b_id);
    // No member names an input of another task, or none at all.
    let dangling: i64 = outside_writer(db.db_path())
        .query_row(
            "SELECT COUNT(*) FROM task_join_member member
             JOIN task_join ON task_join.id = member.join_id
             LEFT JOIN task_input input ON input.id = member.input_id
             WHERE member.input_id IS NOT NULL
               AND (input.id IS NULL OR input.task_id != task_join.parent_task_id)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dangling, 0);
    let notices = db.pending_join_notices().unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].parent_task_id, "a");
    assert_eq!(notices[0].message, "the child's outcome");
}

/// Every carried column that holds an input id is one a moved input is
/// applied to.
#[test]
fn every_carried_input_reference_follows_a_moved_input() {
    for table in crate::db::task_state::CARRIED_TABLES {
        for column in table.columns {
            if column.ends_with("input_id") {
                assert!(
                    crate::db::INPUT_ID_REFERENCES.contains(&(table.table, column)),
                    "{}.{column} holds an input id and is not in INPUT_ID_REFERENCES",
                    table.table
                );
            }
        }
    }
}

/// Round 2, finding 3: the fence is durable. Flagged, a live write lifts
/// the database past the disk's revision, and the process dies before the
/// repair: the restart still takes the disk's rows and never writes the
/// database's over them.
#[test]
fn the_fence_survives_a_restart_after_the_database_caught_up() {
    let (db, db_path, task_json) = flagged_installation("restart");
    let on_disk = std::fs::read(&task_json).unwrap();
    let disk_revision = serde_json::from_slice::<Value>(&on_disk).unwrap()["snapshot_revision"]
        .as_i64()
        .unwrap();
    outside_writer(db.db_path())
        .execute(
            "UPDATE pipeline_item SET display_name = 'live write' WHERE id = 't1'",
            [],
        )
        .unwrap();
    while revision(&db, "t1") <= disk_revision + 1 {
        outside_writer(db.db_path())
            .execute(
                "UPDATE task_ledger_snapshot SET revision = revision + 1 WHERE task_id = 't1'",
                [],
            )
            .unwrap();
    }
    drop(db);

    // A new process: nothing is remembered but the database and the disk.
    let db = Db::open(&db_path).unwrap();
    assert!(is_diverged(&db, "t1"));
    assert!(flush_task(&db, &db_path, "t1").is_err());
    assert_eq!(std::fs::read(&task_json).unwrap(), on_disk);
    let outcome = start(&db, &db_path, None).unwrap();
    let report = outcome.reconcile.unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    assert!(!is_diverged(&db, "t1"));
    assert_eq!(display_name(&db).as_deref(), Some("on disk"));
    let rewritten: Value = serde_json::from_slice(&std::fs::read(&task_json).unwrap()).unwrap();
    assert_eq!(rewritten["title"], "on disk");
}

/// Round 2, finding 4: a transfer whose display status is failed but whose
/// finalization retry is still pending owns its source, so its claim (and
/// its row) survive the repair. A settled transfer's claim does not.
#[test]
fn a_claim_held_by_a_failed_transfers_retry_survives_the_repair() {
    let (db, db_path, _) = flagged_installation("failed-transfer");
    outside_writer(db.db_path())
        .execute_batch(
            "INSERT INTO task_transfer (id, direction, status, source_task_id, local_task_id)
             VALUES ('xfer-retry', 'outgoing', 'failed', 't1', 't1');
             INSERT INTO transfer_work (id, kind, transfer_id, payload_json, status)
             VALUES ('work-retry', 'finalize', 'xfer-retry', '{}', 'pending');
             INSERT INTO task_transfer_workflow_claim (pipeline_item_id, transfer_id)
             VALUES ('t1', 'xfer-retry');",
        )
        .unwrap();
    assert_eq!(
        db.task_workflow_is_claimed_by_transfer("t1")
            .unwrap()
            .as_deref(),
        Some("xfer-retry")
    );
    let only = BTreeSet::from(["t1".to_string()]);
    let report = reconcile_from_disk(&db, &db_path, Some(&only)).unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    assert_eq!(display_name(&db).as_deref(), Some("on disk"));
    assert_eq!(
        db.task_workflow_is_claimed_by_transfer("t1")
            .unwrap()
            .as_deref(),
        Some("xfer-retry")
    );
}

/// The contrast: display status and ownership agree that a failed transfer
/// with no retry left is done, and a repair takes the disk's word for its
/// claim.
#[test]
fn a_claim_whose_transfer_has_finished_is_the_disks_to_decide() {
    let (db, db_path, _) = flagged_installation("finished-transfer");
    outside_writer(db.db_path())
        .execute_batch(
            "INSERT INTO task_transfer (id, direction, status, source_task_id, local_task_id)
             VALUES ('xfer-done', 'outgoing', 'failed', 't1', 't1');
             INSERT INTO transfer_work (id, kind, transfer_id, payload_json, status)
             VALUES ('work-done', 'finalize', 'xfer-done', '{}', 'failed');
             INSERT INTO task_transfer_workflow_claim (pipeline_item_id, transfer_id)
             VALUES ('t1', 'xfer-done');",
        )
        .unwrap();
    assert_eq!(db.task_workflow_is_claimed_by_transfer("t1").unwrap(), None);
    let only = BTreeSet::from(["t1".to_string()]);
    let report = reconcile_from_disk(&db, &db_path, Some(&only)).unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    let claims: i64 = outside_writer(db.db_path())
        .query_row(
            "SELECT COUNT(*) FROM task_transfer_workflow_claim WHERE pipeline_item_id = 't1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(claims, 0);
}

// ---------------------------------------------------------------------------
// Review round 3
// ---------------------------------------------------------------------------

/// The disk's claim names an older transfer that has settled; the live
/// claim names another that still owns the source (display status failed,
/// finalization running). The repair keeps the live claim and its transfer
/// as they are, and takes the rest of the disk's rows.
#[test]
fn a_live_claim_is_not_rewritten_to_the_disks_settled_one() {
    let (db, db_path) = installation("claim-swap", "repo-claim-swap", &["t1"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    outside_writer(db.db_path())
        .execute_batch(
            "INSERT INTO task_transfer (id, direction, status, source_task_id, local_task_id)
             VALUES ('xfer-old', 'outgoing', 'completed', 't1', 't1');
             INSERT INTO task_transfer_workflow_claim (pipeline_item_id, transfer_id)
             VALUES ('t1', 'xfer-old');",
        )
        .unwrap();
    assert!(flush_all(&db, &db_path).is_empty());
    let copy = backup(&db, &db_path);
    outside_writer(db.db_path())
        .execute(
            "UPDATE pipeline_item SET display_name = 'on disk' WHERE id = 't1'",
            [],
        )
        .unwrap();
    assert!(flush_all(&db, &db_path).is_empty());
    drop(db);
    let db = restore(&db_path, &copy);
    outside_writer(db.db_path())
        .execute_batch(
            "INSERT INTO task_transfer (id, direction, status, source_task_id, local_task_id)
             VALUES ('xfer-live', 'outgoing', 'failed', 't1', 't1');
             INSERT INTO transfer_work (id, kind, transfer_id, payload_json, status)
             VALUES ('work-live', 'finalize', 'xfer-live', '{}', 'running');
             UPDATE task_transfer_workflow_claim SET transfer_id = 'xfer-live'
             WHERE pipeline_item_id = 't1';",
        )
        .unwrap();
    assert_eq!(
        db.task_workflow_is_claimed_by_transfer("t1")
            .unwrap()
            .as_deref(),
        Some("xfer-live")
    );
    let transfer_row = |db: &Db, id: &str| -> Option<(String, String)> {
        outside_writer(db.db_path())
            .query_row(
                "SELECT status, direction FROM task_transfer WHERE id = ?",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
    };
    let live_transfer = transfer_row(&db, "xfer-live");
    db.flag_disk_divergence("t1", "the disk is ahead").unwrap();

    let only = BTreeSet::from(["t1".to_string()]);
    let report = reconcile_from_disk(&db, &db_path, Some(&only)).unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    assert_eq!(display_name(&db).as_deref(), Some("on disk"));
    let claim: String = outside_writer(db.db_path())
        .query_row(
            "SELECT transfer_id FROM task_transfer_workflow_claim WHERE pipeline_item_id = 't1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(claim, "xfer-live");
    assert_eq!(transfer_row(&db, "xfer-live"), live_transfer);
    assert_eq!(
        db.task_workflow_is_claimed_by_transfer("t1")
            .unwrap()
            .as_deref(),
        Some("xfer-live")
    );
    // The transfer that owns it can still finalize.
    assert_eq!(
        db.claim_task_workflow_for_transfer_finalization("xfer-live", "t1")
            .unwrap(),
        Ok(())
    );
}

/// Every carried column that names a claim, an owner, a lease or its expiry
/// is an ownership column, and none of them is ever in what a repair
/// writes over a live row while the database holds a claim.
#[test]
fn ownership_columns_are_never_in_the_disk_wins_update_set() {
    use crate::db::TRANSFER_OWNERSHIP_COLUMNS;
    let listed = |table: &str, column: &str| {
        TRANSFER_OWNERSHIP_COLUMNS
            .iter()
            .any(|(name, columns)| *name == table && columns.contains(&column))
    };
    // Columns the words match that are not a transfer's ownership, each
    // classified on purpose: a new match must be listed on one side.
    const NOT_TRANSFER_OWNERSHIP: &[(&str, &str, &str)] = &[(
        "human_review_decision",
        "owner_desktop_id",
        "the desktop that delivers a review decision's merge; T9 never reads it",
    )];
    for table in crate::db::task_state::CARRIED_TABLES {
        for column in table.columns {
            let ownership = ["claim", "owner", "lease", "expir"]
                .iter()
                .any(|word| column.contains(word))
                || table.table == "task_transfer_workflow_claim" && *column != "pipeline_item_id";
            let exempt = NOT_TRANSFER_OWNERSHIP
                .iter()
                .any(|(name, exempt, _)| *name == table.table && exempt == column);
            if ownership && !exempt {
                assert!(
                    listed(table.table, column),
                    "{}.{column} is ownership state and is not in TRANSFER_OWNERSHIP_COLUMNS",
                    table.table
                );
            }
        }
        let every: Map<String, Value> = table
            .columns
            .iter()
            .map(|column| (column.to_string(), json!("from disk")))
            .collect();
        let written = crate::db::disk_wins_update(table.table, every.clone(), true);
        assert!(
            written.keys().all(|column| !listed(table.table, column)),
            "{} would take ownership columns from disk",
            table.table
        );
        // Without a live claim the disk's word stands, ownership included.
        assert_eq!(
            crate::db::disk_wins_update(table.table, every.clone(), false),
            every
        );
    }
}

// ---------------------------------------------------------------------------
// Disk-first writes (T13d)
// ---------------------------------------------------------------------------

fn disk_installation(label: &str) -> (Db, String, PathBuf) {
    let (db, db_path) = installation(label, &format!("repo-{label}"), &["t1"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    let task_json =
        task_dir(&root_for_db(&db_path), &format!("repo-{label}"), "t1").join("task.json");
    (db, db_path, task_json)
}

fn title_on_disk(task_json: &Path) -> Value {
    serde_json::from_slice::<Value>(&std::fs::read(task_json).unwrap()).unwrap()["title"].clone()
}

/// A row change with no ledger entry is on disk when its statement
/// returns, with nothing left for the publisher; in `sql` mode the same
/// write waits for the publisher as before.
#[test]
fn a_row_change_is_on_disk_before_it_commits() {
    let (db, _db_path, task_json) = disk_installation("row-first");
    db.connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET display_name = 'written first' WHERE id = 't1'",
            [],
        )
        .unwrap();
    assert_eq!(title_on_disk(&task_json), "written first");
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
    operator_input(&db, "t1", "an input");
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
    let ledger = crate::task_store::read_ledger(task_json.parent().unwrap()).unwrap();
    assert_eq!(
        ledger
            .last()
            .and_then(|file| file.message.clone())
            .as_deref(),
        Some("an input")
    );

    let (sql, sql_path) = installation("row-sql", "repo-row-sql", &["t1"]);
    assert!(flush_all(&sql, &sql_path).is_empty());
    let sql_json = task_dir(&root_for_db(&sql_path), "repo-row-sql", "t1").join("task.json");
    sql.connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET display_name = 'published later' WHERE id = 't1'",
            [],
        )
        .unwrap();
    assert_ne!(title_on_disk(&sql_json), "published later");
    assert!(!sql.ledger_tasks_with_pending_work().unwrap().is_empty());
}

/// A commit that reaches the outbox outside the gate (a raw transaction)
/// is refused in `disk` mode, so no path can write SQLite first; the same
/// transaction commits in `sql` mode.
#[test]
fn a_commit_outside_the_gate_is_refused_in_disk_mode() {
    let (db, db_path, task_json) = disk_installation("outside-gate");
    let before = title_on_disk(&task_json);
    let refused = crate::db::disk_first::refused_commits(&db_path);
    let raw = |db: &Db| {
        let conn: &rusqlite::Connection = db.connection_for_e2e_tests();
        conn.execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE pipeline_item SET display_name = 'behind the gate' WHERE id = 't1';
             COMMIT;",
        )
    };
    assert!(raw(&db).is_err());
    let _ = db.connection_for_e2e_tests().execute_batch("ROLLBACK");
    assert_eq!(
        crate::db::disk_first::refused_commits(&db_path),
        refused + 1
    );
    assert_ne!(display_name(&db).as_deref(), Some("behind the gate"));
    assert_eq!(title_on_disk(&task_json), before);

    let (sql, _) = installation("outside-gate-sql", "repo-og-sql", &["t1"]);
    raw(&sql).unwrap();
    assert_eq!(
        sql.get_pipeline_item("t1")
            .unwrap()
            .unwrap()
            .display_name
            .as_deref(),
        Some("behind the gate")
    );
}

/// SQLite fails to commit after the disk did: the caller sees the error,
/// the task is fenced (every later write refused, durably), and the repair
/// takes the mutation from disk, where it was committed.
#[test]
fn a_commit_that_fails_after_the_disk_write_is_taken_from_disk() {
    let (db, db_path, task_json) = disk_installation("failed-commit");
    crate::db::disk_first::fail_next_commit(&db_path);
    let error = db
        .connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET display_name = 'on disk only' WHERE id = 't1'",
            [],
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("injected COMMIT failure"), "{error}");
    assert_eq!(title_on_disk(&task_json), "on disk only");
    assert_ne!(display_name(&db).as_deref(), Some("on disk only"));
    assert!(is_diverged(&db, "t1"));
    assert!(db.is_disk_divergent("t1").unwrap());
    assert!(operator_input_result(&db, "t1", "refused").is_err());
    assert_eq!(title_on_disk(&task_json), "on disk only");

    let only = BTreeSet::from(["t1".to_string()]);
    let report = reconcile_from_disk(&db, &db_path, Some(&only)).unwrap();
    assert_eq!(report.reconciled.len(), 1, "{report:#?}");
    assert!(!is_diverged(&db, "t1"));
    assert_eq!(display_name(&db).as_deref(), Some("on disk only"));
    operator_input(&db, "t1", "accepted again");
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
}

/// What a build before T13d does with the record: it understands only
/// schema version 1, and refuses anything else.
fn older_build_accepts(root: &Path, db_path: &str) -> bool {
    let bytes = std::fs::read(record_path(root, &installation_id(db_path))).unwrap();
    serde_json::from_slice::<Value>(&bytes).unwrap()["schema_version"] == 1
}

/// The rollback window (T13d). Older builds: closed from this build's
/// first disk-first start (the record becomes schema version 2, which they
/// refuse) until a completed rollback reopens it. This build: a rollback
/// commits only when the database, reconciled from disk, verifies equal to
/// it; otherwise it is refused with the differences, the installation
/// stays `disk`, and the request is retried at the next start.
#[test]
fn the_rollback_window_refuses_what_it_cannot_verify_and_closes_to_older_builds() {
    let (db, db_path) = installation("window", "repo-window", &["t1", "t2"]);
    let root = root_for_db(&db_path);
    assert!(flush_all(&db, &db_path).is_empty());

    // An installation a T13c build switched to disk: its record is still
    // version 1 and open to that build, until this build starts on it.
    let mut record = AuthorityRecord::new(&db_path);
    record.mode = Mode::Disk;
    std::fs::create_dir_all(root.join("authority")).unwrap();
    std::fs::write(
        record_path(&root, &installation_id(&db_path)),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    assert!(older_build_accepts(&root, &db_path));
    assert_eq!(start(&db, &db_path, None).unwrap().mode, Mode::Disk);
    let record = load_record(&root, &db_path).unwrap();
    assert_eq!(record.schema_version, DISK_FIRST_RECORD_SCHEMA_VERSION);
    assert!(record.disk_first_since.is_some() && record.note.is_some());
    assert!(!older_build_accepts(&root, &db_path));
    operator_input(&db, "t1", "written disk-first");

    // The disk holds what the database cannot be shown to hold: t2's
    // task.json is ahead of the database and carries no rows to project.
    // The rollback is refused and retried.
    let t2_json = task_dir(&root, "repo-window", "t2").join("task.json");
    let intact = std::fs::read(&t2_json).unwrap();
    let mut ahead: Value = serde_json::from_slice(&intact).unwrap();
    ahead.as_object_mut().unwrap().remove("state");
    ahead["snapshot_revision"] = json!(ahead["snapshot_revision"].as_i64().unwrap() + 5);
    std::fs::write(&t2_json, serde_json::to_vec_pretty(&ahead).unwrap()).unwrap();
    let outcome = start(&db, &db_path, Some(Mode::Sql)).unwrap();
    assert_eq!(outcome.mode, Mode::Disk);
    assert!(
        outcome.refused.iter().any(|problem| problem.contains("t2")),
        "{:?}",
        outcome.refused
    );
    let record = load_record(&root, &db_path).unwrap();
    assert_eq!(
        (record.mode, record.requested),
        (Mode::Disk, Some(Mode::Sql))
    );
    assert!(record.switch.is_none());
    let last = record.checkpoints.last().unwrap();
    assert_eq!(last.checkpoint, "to_sql.refused");
    assert!(
        last.detail["problems"].to_string().contains("t2"),
        "{last:?}"
    );
    assert!(!older_build_accepts(&root, &db_path));
    assert_eq!(mode_for_root(&root), Mode::Disk);

    // Fixed, the next start verifies and commits the rollback, and the
    // record is open to older builds again.
    std::fs::write(&t2_json, intact).unwrap();
    let outcome = start(&db, &db_path, None).unwrap();
    assert_eq!(outcome.mode, Mode::Sql, "{:?}", outcome.refused);
    let record = load_record(&root, &db_path).unwrap();
    assert_eq!(record.schema_version, RECORD_SCHEMA_VERSION);
    assert!(record.disk_first_since.is_none() && record.requested.is_none());
    let names: Vec<&str> = record
        .checkpoints
        .iter()
        .rev()
        .take(3)
        .map(|checkpoint| checkpoint.checkpoint.as_str())
        .collect();
    assert_eq!(
        names,
        ["to_sql.commit", "to_sql.reconciled", "to_sql.begin"]
    );
    assert!(older_build_accepts(&root, &db_path));
    let inputs = inputs(&db);
    assert!(inputs
        .iter()
        .any(|(_, task, text)| task == "t1" && text == "written disk-first"));
}

/// Writers on their own connections allocate ledger sequences (entries,
/// and reservations released as gaps) while publishers flush on theirs, in
/// both authority modes. No number is handed out twice, every entry
/// reaches disk with its committed bytes, and nothing is left owed.
#[test]
fn ledger_sequences_stay_unique_under_concurrent_writers_and_publishers() {
    for mode in [Mode::Sql, Mode::Disk] {
        let label = format!("stress-{}", mode.as_str());
        let repo = format!("repo-{label}");
        let (db, db_path) = installation(&label, &repo, &["t1", "t2"]);
        assert!(flush_all(&db, &db_path).is_empty());
        assert_eq!(start(&db, &db_path, Some(mode)).unwrap().mode, mode);
        drop(db);
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let publishers: Vec<_> = (0..2)
            .map(|_| {
                let (db_path, done) = (db_path.clone(), std::sync::Arc::clone(&done));
                std::thread::spawn(move || {
                    let db = Db::open(&db_path).unwrap();
                    while !done.load(std::sync::atomic::Ordering::SeqCst) {
                        let _ = flush_all(&db, &db_path);
                    }
                })
            })
            .collect();
        let writers: Vec<_> = (0..4)
            .map(|writer| {
                let db_path = db_path.clone();
                std::thread::spawn(move || {
                    let db = Db::open(&db_path).unwrap();
                    let mut released = Vec::new();
                    for op in 0..20 {
                        let task = if (writer + op) % 2 == 0 { "t1" } else { "t2" };
                        if op % 3 == 2 {
                            let sequence = db.reserve_ledger_sequence(task).unwrap();
                            db.release_ledger_reservation(task, sequence).unwrap();
                            released.push((task.to_string(), sequence));
                        } else {
                            operator_input(&db, task, &format!("writer {writer} op {op}"));
                        }
                    }
                    released
                })
            })
            .collect();
        let released: Vec<(String, i64)> = writers
            .into_iter()
            .flat_map(|writer| writer.join().unwrap())
            .collect();
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        for publisher in publishers {
            publisher.join().unwrap();
        }
        let db = Db::open(&db_path).unwrap();
        let failures = flush_all(&db, &db_path);
        assert!(failures.is_empty(), "{mode:?}: {failures:?}");
        assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
        let root = root_for_db(&db_path);
        for task in ["t1", "t2"] {
            let rows = db.ledger_rows_for_authority(task).unwrap();
            let entries: Vec<i64> = rows.iter().map(|row| row.sequence).collect();
            let gaps: Vec<i64> = released
                .iter()
                .filter(|(released_task, _)| released_task == task)
                .map(|(_, sequence)| *sequence)
                .collect();
            let mut handed_out: Vec<i64> = entries.iter().chain(&gaps).copied().collect();
            handed_out.sort();
            let unique = handed_out.len();
            handed_out.dedup();
            assert_eq!(
                handed_out.len(),
                unique,
                "{mode:?} {task}: a number was reused"
            );
            let high_water: i64 = outside_writer(&db_path)
                .query_row(
                    "SELECT high_water FROM task_ledger_sequence WHERE task_id = ?",
                    [task],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(Some(&high_water), handed_out.last(), "{mode:?} {task}");
            let files = crate::task_store::read_ledger(&task_dir(&root, &repo, task)).unwrap();
            assert_eq!(
                files.iter().map(|file| file.sequence).collect::<Vec<_>>(),
                entries,
                "{mode:?} {task}"
            );
            for row in &rows {
                let on_disk = std::fs::read(task_dir(&root, &repo, task).join("ledger").join(
                    crate::db::task_store::ledger_file_name(
                        row.sequence,
                        crate::db::task_store::LedgerEntryKind::Input,
                    ),
                ))
                .unwrap();
                assert_eq!(Some(on_disk), row.payload.clone(), "{mode:?} {task}");
            }
        }
        if mode == Mode::Disk {
            assert!(diverged_tasks(&db).is_empty());
            assert_eq!(crate::db::disk_first::refused_commits(&db_path), 0);
            let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
            assert!(
                report.reconciled.is_empty() && report.failed.is_empty(),
                "{report:#?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Review round 1 (T13d)
// ---------------------------------------------------------------------------

/// A process that holds only the database path, opening it the way the
/// `worktree-cleanup` subcommand does, takes the installation's persisted
/// `disk` mode and root from its authority record: its writes go through
/// the disk-first gate, and a commit around the gate is refused.
#[test]
fn a_process_that_only_holds_the_database_path_is_gated_on_a_disk_installation() {
    let (db, db_path, task_json) = disk_installation("path-only");
    drop(db);
    crate::task_store::forget_for_tests(&db_path);
    let root = root_for_db(&db_path);
    assert_eq!(
        mode_for_root(&root),
        Mode::Sql,
        "a fresh process knows nothing yet"
    );

    let db = Db::open(&db_path).unwrap();
    assert_eq!(mode_for_root(&root), Mode::Disk);
    db.connection_for_e2e_tests()
        .execute(
            "UPDATE pipeline_item SET display_name = 'from a subcommand' WHERE id = 't1'",
            [],
        )
        .unwrap();
    assert_eq!(title_on_disk(&task_json), "from a subcommand");
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
    let refused = crate::db::disk_first::refused_commits(&db_path);
    let raw: &rusqlite::Connection = db.connection_for_e2e_tests();
    assert!(raw
        .execute_batch(
            "BEGIN IMMEDIATE;
             UPDATE pipeline_item SET display_name = 'around the gate' WHERE id = 't1';
             COMMIT;"
        )
        .is_err());
    let _ = raw.execute_batch("ROLLBACK");
    assert_eq!(
        crate::db::disk_first::refused_commits(&db_path),
        refused + 1
    );
    assert_eq!(title_on_disk(&task_json), "from a subcommand");
}

/// A repository removal whose tombstone reached disk and whose SQLite
/// commit did not (the process died between them): the restart applies
/// the removal, with the repository's tasks, exactly as a rebuild from the
/// same disk does.
#[test]
fn a_repository_removal_that_died_before_sqlite_committed_restarts_as_it_rebuilds() {
    use crate::task_store::disk_first::{crash_at, CrashPoint};
    let (db, db_path, task_json) = disk_installation("repo-removal");
    let root = root_for_db(&db_path);
    let repo = "repo-repo-removal";
    crash_at(&root, &format!("repo:{repo}"), CrashPoint::BeforeCommit);
    assert!(db.delete_repo(repo).is_err());
    let repo_json = super::super::repo_dir(&root, repo).join("repo.json");
    assert!(super::super::rebuild::is_tombstone(
        &std::fs::read(&repo_json).unwrap()
    ));
    assert!(
        db.get_repo(repo).unwrap().is_some(),
        "SQLite never committed it"
    );
    assert!(db.get_pipeline_item("t1").unwrap().is_some());
    drop(db);

    // Restart.
    let db = Db::open(&db_path).unwrap();
    let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert_eq!(report.removed_repos, [repo], "{report:#?}");
    assert!(report.failed.is_empty(), "{report:#?}");
    assert!(db.get_repo(repo).unwrap().is_none());
    assert!(db.get_pipeline_item("t1").unwrap().is_none());
    assert!(super::super::rebuild::is_tombstone(
        &std::fs::read(&task_json).unwrap()
    ));

    // A rebuild from the same disk agrees.
    let rebuilt_path = format!("{db_path}.rebuilt");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{rebuilt_path}{suffix}"));
    }
    let (scan, _) = scan_store_records(&root)
        .unwrap()
        .for_installation(&installation_id(&db_path));
    rebuild_scan_into_new_database(scan, Path::new(&rebuilt_path)).unwrap();
    let rebuilt = Db::open(&rebuilt_path).unwrap();
    assert_eq!(rebuilt.sql_repo_ids().unwrap(), db.sql_repo_ids().unwrap());
    let tasks = |db: &Db| -> Vec<String> {
        db.sql_tasks_for_authority()
            .unwrap()
            .into_iter()
            .map(|task| task.id)
            .collect()
    };
    assert_eq!(tasks(&rebuilt), tasks(&db));
    assert!(tasks(&db).is_empty());
    // And the next start has nothing to do.
    let again = start(&db, &db_path, None).unwrap().reconcile.unwrap();
    assert!(
        again.removed_repos.is_empty() && again.reconciled.is_empty(),
        "{again:#?}"
    );
}

/// An entry committed behind another operation's open reservation is on
/// disk at once (durability is never held back), but no consumer reads it
/// in order until the reservation closes: the task's readable watermark
/// stops below the reservation, and reconciling another task neither moves
/// that watermark nor publishes anything of the other task.
#[test]
fn reconciling_one_task_never_publishes_another_tasks_entries_past_its_reservation() {
    let (db, db_path) = installation("reservation", "repo-reservation", &["a", "b"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    let root = root_for_db(&db_path);
    let b_dir = task_dir(&root, "repo-reservation", "b");
    let reserved = db.reserve_ledger_sequence("b").unwrap();
    operator_input(&db, "b", "behind the reservation");
    let behind = reserved + 1;
    let behind_file = b_dir
        .join("ledger")
        .join(crate::db::task_store::ledger_file_name(
            behind,
            crate::db::task_store::LedgerEntryKind::Input,
        ));
    assert!(behind_file.exists(), "durable before its commit returned");
    assert_eq!(
        crate::task_store::readable_through(&b_dir),
        Some(reserved - 1)
    );
    let task_json_before = std::fs::read(b_dir.join("task.json")).unwrap();

    let report =
        reconcile_from_disk(&db, &db_path, Some(&BTreeSet::from(["a".to_string()]))).unwrap();
    assert_eq!(report.materialized, 0, "{report:#?}");
    assert_eq!(
        std::fs::read(b_dir.join("task.json")).unwrap(),
        task_json_before
    );
    assert_eq!(
        crate::task_store::readable_through(&b_dir),
        Some(reserved - 1)
    );

    // Once the reservation closes, the entry is readable in order.
    db.release_ledger_reservation("b", reserved).unwrap();
    assert_eq!(crate::task_store::readable_through(&b_dir), Some(behind));
}

fn input_messages(db: &Db, task: &str) -> Vec<String> {
    inputs(db)
        .into_iter()
        .filter(|(_, owner, _)| owner == task)
        .map(|(_, _, message)| message)
        .collect()
}

/// Review round 2: operation A holds a reservation while commit B records
/// an input behind it. B's entry is durable on disk (as a file, and in its
/// `task.json` until then) before B's commit returns, so it survives a kill
/// at every write step, a rebuild from disk alone, and a later commit of
/// the task that dies before SQLite commits; it is readable in order only
/// once A's reservation closes (filled, released, or abandoned at restart).
#[test]
fn a_commit_behind_an_open_reservation_is_durable_and_readable_once_it_closes() {
    use crate::task_store::disk_first::{crash_at, CrashPoint};
    // A crash point, given the sequence of the entry behind the reservation.
    type At = fn(i64) -> CrashPoint;
    let points: [(&str, Option<At>); 5] = [
        ("none", None),
        ("before-task-json", Some(|_| CrashPoint::BeforeTaskJson)),
        ("after-task-json", Some(|_| CrashPoint::AfterTaskJson)),
        ("after-entry", Some(CrashPoint::AfterEntry)),
        ("before-commit", Some(|_| CrashPoint::BeforeCommit)),
    ];
    for (label, point) in points {
        let repo = format!("repo-behind-{label}");
        let (db, db_path) = installation(&format!("behind-{label}"), &repo, &["t1"]);
        assert!(flush_all(&db, &db_path).is_empty());
        assert_eq!(
            start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
            Mode::Disk
        );
        let root = root_for_db(&db_path);
        let dir = task_dir(&root, &repo, "t1");
        let reserved = db.reserve_ledger_sequence("t1").unwrap();
        let behind = reserved + 1;
        let behind_file = dir
            .join("ledger")
            .join(crate::db::task_store::ledger_file_name(
                behind,
                crate::db::task_store::LedgerEntryKind::Input,
            ));
        if let Some(point) = point {
            crash_at(&root, "t1", point(behind));
        }
        let recorded = operator_input_result(&db, "t1", "behind the reservation");
        assert_eq!(recorded.is_err(), point.is_some(), "{label}: {recorded:?}");
        let applied = !matches!(
            point.map(|point| point(behind)),
            Some(CrashPoint::BeforeTaskJson)
        );
        if point.is_none() {
            assert!(behind_file.exists(), "{label}");
            assert_eq!(
                crate::task_store::readable_through(&dir),
                Some(reserved - 1)
            );

            // (b) A later commit of the task writes task.json and dies
            // before SQLite commits; the restart below reconciles from that
            // task.json, which still holds the entry.
            crash_at(&root, "t1", CrashPoint::AfterTaskJson);
            assert!(db
                .connection_for_e2e_tests()
                .execute(
                    "UPDATE pipeline_item SET display_name = 'dies before commit' WHERE id = 't1'",
                    [],
                )
                .is_err());
        }
        drop(db);

        // Restart: reconciliation runs before the stale reservation is
        // released.
        let db = Db::open(&db_path).unwrap();
        let report = start(&db, &db_path, None).unwrap().reconcile.unwrap();
        assert!(report.failed.is_empty(), "{label}: {report:#?}");
        let expected: Vec<String> = if applied {
            vec!["behind the reservation".to_string()]
        } else {
            Vec::new()
        };
        assert_eq!(input_messages(&db, "t1"), expected, "{label}");
        assert_eq!(behind_file.exists(), applied, "{label}");

        // (a) A rebuild from disk alone holds the same inputs.
        let rebuilt_path = format!("{db_path}.rebuilt");
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{rebuilt_path}{suffix}"));
        }
        let (scan, _) = scan_store_records(&root)
            .unwrap()
            .for_installation(&installation_id(&db_path));
        rebuild_scan_into_new_database(scan, Path::new(&rebuilt_path)).unwrap();
        assert_eq!(
            input_messages(&Db::open(&rebuilt_path).unwrap(), "t1"),
            expected,
            "{label}"
        );

        // The reservation died with its process: startup recovery releases
        // it, and the watermark passes the gap.
        db.release_stale_ledger_reservations().unwrap();
        if applied {
            assert_eq!(
                crate::task_store::readable_through(&dir),
                Some(behind),
                "{label}"
            );
        }
        assert!(
            db.ledger_tasks_with_pending_work().unwrap().is_empty(),
            "{label}"
        );
    }

    // Filled instead of released: operation A records its entry at the
    // reserved sequence, and the watermark moves past both.
    let (db, db_path) = installation("behind-filled", "repo-behind-filled", &["t1"]);
    assert!(flush_all(&db, &db_path).is_empty());
    assert_eq!(
        start(&db, &db_path, Some(Mode::Disk)).unwrap().mode,
        Mode::Disk
    );
    let dir = task_dir(&root_for_db(&db_path), "repo-behind-filled", "t1");
    let reserved = db.reserve_ledger_sequence("t1").unwrap();
    operator_input(&db, "t1", "behind the reservation");
    assert_eq!(
        crate::task_store::readable_through(&dir),
        Some(reserved - 1)
    );
    db.enqueue_ledger_entry(crate::db::task_store::NewLedgerEntry {
        task_id: "t1",
        kind: crate::db::task_store::LedgerEntryKind::Plan,
        operation_id: None,
        source_kind: "test_reserved",
        source_id: "a",
        source_origin: None,
        historical: false,
        recorded_at: None,
        run_id: None,
        declared_role: None,
        channel_identity: &ChannelIdentity::Server,
        body: json!({ "operation": "select" }),
        message: None,
        hold_events_after: None,
        reserved_sequence: Some(reserved),
    })
    .unwrap();
    assert_eq!(
        crate::task_store::readable_through(&dir),
        Some(reserved + 1)
    );
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
}
