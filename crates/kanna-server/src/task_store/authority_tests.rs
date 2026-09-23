//! Focused tests for storage authority: the record, installation scoping,
//! the divergence verdicts, and a refused switch. The fixture installation
//! round trips (switch, restart at every checkpoint, rollback, rebuild,
//! recovery without replay) are in `http_api::tests::disk_authority`.

use super::*;
use crate::mutation_provenance::ChannelIdentity;
use crate::task_store::task_dir;

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
