use super::*;
use crate::db::{Db, NewStageRun};
use std::collections::BTreeSet;

fn migrated(label: &str) -> (Db, String) {
    let path = Db::test_db_path(label);
    let _ = std::fs::remove_file(&path);
    let db = Db::open_migrated(&path).unwrap();
    db.insert_test_repo_with_path("repo-1", "/tmp/repo-one", "Repo One")
        .unwrap();
    (db, path)
}

fn task(db: &Db, id: &str) {
    db.insert_test_pipeline_item(
        id,
        "repo-1",
        &format!("Prompt of {id}"),
        Some(id),
        "in progress",
        "2026-09-23 00:00:00",
    )
    .unwrap();
    db.mark_task_ledger_backfilled(id, 0).unwrap();
}

fn run(db: &Db, id: &str, task_id: &str, kind: &str) {
    db.insert_stage_run(NewStageRun {
        id,
        task_id,
        stage: "in progress",
        kind,
        agent: Some("implement"),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "running",
        result: None,
        feedback: None,
        session_id: Some(task_id),
        provider_session_id: None,
        cwd: None,
        resumed_from_run_id: None,
    })
    .unwrap();
}

fn revision(db: &Db, task_id: &str) -> (i64, i64) {
    db.task_snapshot_revisions(task_id).unwrap().unwrap()
}

fn columns_of(db: &Db, table: &str) -> BTreeSet<String> {
    let mut statement = db
        .conn
        .prepare("SELECT name FROM pragma_table_info(?)")
        .unwrap();
    let rows = statement.query_map([table], |row| row.get(0)).unwrap();
    rows.collect::<Result<_, _>>().unwrap()
}

/// Every table of a migrated database is either carried by task.json's
/// `state` or classified as not carried with a reason, and every column of
/// a carried table is either carried or left out with a reason. A new
/// table or column fails here until someone decides where its authority
/// is.
#[test]
fn every_table_and_column_is_classified() {
    let (db, _) = migrated("task-state-classified");
    let mut statement = db
        .conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap();
    let tables: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for table in &tables {
        let carried = CARRIED_TABLES.iter().any(|carried| carried.table == table);
        let classified = NOT_CARRIED_TABLES
            .iter()
            .any(|(name, _, reason)| name == table && !reason.is_empty());
        assert!(
            carried ^ classified,
            "table {table} must be carried or classified, not both or neither"
        );
    }
    for (name, _, _) in NOT_CARRIED_TABLES {
        assert!(tables.contains(&name.to_string()), "{name} is not a table");
    }
    for table in CARRIED_TABLES {
        let actual = columns_of(&db, table.table);
        let mut classified = BTreeSet::new();
        for column in table
            .columns
            .iter()
            .chain(table.left_out.iter().map(|(column, _)| column))
        {
            assert!(
                classified.insert(column.to_string()),
                "{}.{column} is classified twice",
                table.table
            );
        }
        assert_eq!(actual, classified, "columns of {}", table.table);
        for quiet in table.quiet {
            assert!(table.columns.contains(quiet));
        }
        // Each carried table owes task.json on insert, update and delete
        // (a deleted task row records a removal instead).
        for event in ["insert", "update", "delete"] {
            let name = format!("disk_state_{}_{event}", table.table);
            let present: bool = db
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'trigger' AND name = ?)",
                    [&name],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(present, "{name}");
        }
    }
    assert_eq!(
        columns_of(&db, "repo"),
        REPO_COLUMNS
            .iter()
            .map(|column| column.to_string())
            .collect()
    );
    // Installed exactly as defined, so reopening rewrites nothing.
    let mut desired = disk_state_triggers();
    desired.sort();
    assert_eq!(installed_disk_state_triggers(&db.conn).unwrap(), desired);
}

/// A migration runs without the triggers (SQLite checks them when a table
/// is altered or rebuilt) and they come back afterwards.
#[test]
fn migrations_run_without_the_triggers_and_restore_them() {
    let (db, path) = migrated("task-state-remigrate");
    task(&db, "t-1");
    drop_disk_state_triggers(&db.conn).unwrap();
    db.conn
        .execute_batch(
            "ALTER TABLE pipeline_item DROP COLUMN attention_requested;
             DELETE FROM schema_migrations WHERE id = '094_task_attention_flag';",
        )
        .unwrap();
    drop(db);
    let db = Db::open_migrated(&path).unwrap();
    let mut desired = disk_state_triggers();
    desired.sort();
    assert_eq!(installed_disk_state_triggers(&db.conn).unwrap(), desired);
    let before = revision(&db, "t-1").0;
    db.set_task_attention("t-1", true).unwrap();
    assert_eq!(revision(&db, "t-1").0, before + 1);
}

/// Nothing credential-like is carried to disk.
#[test]
fn no_carried_column_is_a_secret() {
    let secret = [
        "token",
        "secret",
        "password",
        "credential",
        "private_key",
        "api_key",
    ];
    for table in CARRIED_TABLES {
        for column in table.columns {
            assert!(
                !secret.iter().any(|word| column.contains(word)),
                "{}.{column} looks like a secret",
                table.table
            );
        }
    }
    let task_transfer = CARRIED_TABLES
        .iter()
        .find(|table| table.table == "task_transfer")
        .unwrap();
    assert!(task_transfer
        .left_out
        .iter()
        .any(|(column, _)| *column == "claim_owner_token"));
}

/// A carried change owes a new task.json in the writer's own statement; a
/// live-only change does not; a rolled-back change owes nothing.
#[test]
fn carried_changes_owe_task_json_in_the_same_transaction() {
    let (db, _) = migrated("task-state-triggers");
    task(&db, "t-1");
    let start = revision(&db, "t-1").0;

    db.conn
        .execute(
            "UPDATE pipeline_item SET activity = 'working', last_output_preview = 'x',
                    updated_at = datetime('now', '+1 minute')
             WHERE id = 't-1'",
            [],
        )
        .unwrap();
    assert_eq!(revision(&db, "t-1").0, start, "live columns owe nothing");

    db.conn
        .execute(
            "UPDATE pipeline_item SET attention_requested = 1 WHERE id = 't-1'",
            [],
        )
        .unwrap();
    assert_eq!(revision(&db, "t-1").0, start + 1);

    run(&db, "run-1", "t-1", "main");
    assert_eq!(revision(&db, "t-1").0, start + 2);
    db.record_stage_run_prompt("run-1", "Implement it").unwrap();
    assert_eq!(revision(&db, "t-1").0, start + 3);

    db.conn
        .execute_batch(
            "BEGIN IMMEDIATE;
             INSERT INTO task_branch_counter (task_id, last_allocated) VALUES ('t-1', 1);
             ROLLBACK;",
        )
        .unwrap();
    let after_rollback = revision(&db, "t-1").0;
    assert!(db.task_branch_counter("t-1").unwrap().is_none());
    assert_eq!(after_rollback, start + 3);

    // The recorded state is what task.json carries.
    let facts = db.task_snapshot_facts("t-1").unwrap().unwrap();
    let tables = &facts["state"]["tables"];
    assert_eq!(facts["state"]["version"], DISK_STATE_VERSION);
    assert_eq!(tables["pipeline_item"][0]["attention_requested"], 1);
    assert!(tables["pipeline_item"][0].get("activity").is_none());
    assert_eq!(tables["stage_run"][0]["id"], "run-1");
    assert_eq!(
        tables["stage_run_prompt"][0]["resolved_prompt"],
        "Implement it"
    );
}

/// The repository record is owed when its registration or sidebar order
/// changes, and a removed task or repository is owed a tombstone.
#[test]
fn repositories_and_removals_are_owed_their_records() {
    let (db, path) = migrated("task-state-repo");
    let root = crate::task_store::root_for_db(&path);
    task(&db, "t-1");
    assert!(crate::task_store::flush_all(&db, &path).is_empty());
    let repo_json = root.join("repos").join("repo-1").join("repo.json");
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&repo_json).unwrap()).unwrap();
    assert_eq!(record["registration"]["path"], "/tmp/repo-one");

    db.conn
        .execute_batch(
            "UPDATE repo SET remote_url_hash = 'h' WHERE id = 'repo-1';
             INSERT INTO repo_sidebar_order (remote_url_hash, sort_order) VALUES ('h', 7);",
        )
        .unwrap();
    assert_eq!(db.repos_with_pending_disk_record().unwrap(), vec!["repo-1"]);
    assert!(crate::task_store::flush_all(&db, &path).is_empty());
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&repo_json).unwrap()).unwrap();
    assert_eq!(record["sidebar_order"], 7);

    let task_json = crate::task_store::task_dir(&root, "repo-1", "t-1").join("task.json");
    assert!(task_json.exists());
    db.delete_task_creation_artifacts("t-1").unwrap();
    assert!(crate::task_store::flush_all(&db, &path).is_empty());
    let tombstone: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&task_json).unwrap()).unwrap();
    assert_eq!(tombstone[REMOVED_KEY], true);
    assert!(db.pending_disk_removals().unwrap().is_empty());

    db.delete_repo("repo-1").unwrap();
    assert!(crate::task_store::flush_all(&db, &path).is_empty());
    let tombstone: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&repo_json).unwrap()).unwrap();
    assert_eq!(tombstone[REMOVED_KEY], true);
}

/// A run that ends without a verdict leaves an engine-observed ending in
/// its ledger, and nothing reads that ending as a verdict.
#[test]
fn engine_observed_endings_are_recorded_and_never_read_as_verdicts() {
    let (db, path) = migrated("task-state-endings");
    task(&db, "t-1");
    run(&db, "run-1", "t-1", "main");
    db.finish_latest_running_stage_run("t-1", "failed", None, Some("lost"))
        .unwrap()
        .unwrap();
    run(&db, "run-2", "t-1", "main");
    run(&db, "teardown-1", "t-1", "teardown");
    db.cancel_running_stage_runs("t-1").unwrap();
    assert!(crate::task_store::flush_all(&db, &path).is_empty());

    let dir = crate::task_store::task_dir(&crate::task_store::root_for_db(&path), "repo-1", "t-1");
    let files = crate::task_store::read_ledger(&dir).unwrap();
    let endings: Vec<(String, serde_json::Value)> = files
        .iter()
        .map(|file| {
            (
                file.envelope["run_id"].as_str().unwrap_or("").to_string(),
                file.body().clone(),
            )
        })
        .collect();
    // One per session that ended; none for the teardown run.
    assert_eq!(
        endings
            .iter()
            .map(|(run, _)| run.as_str())
            .collect::<Vec<_>>(),
        vec!["run-1", "run-2"]
    );
    let (_, first) = &endings[0];
    assert_eq!(first["observed_by"], "engine");
    assert_eq!(first["status"], serde_json::Value::Null);
    assert_eq!(
        first["ending"]["no_work_termination"],
        "session_interrupted"
    );
    assert_eq!(endings[1].1["ending"]["run_status"], "cancelled");
    assert!(files
        .iter()
        .all(|file| file.envelope["declared_role"].is_null()));

    // Not a session's trigger, and not a transition's.
    assert_eq!(
        crate::task_store::resolve_trigger(&dir, "in progress"),
        None
    );
    assert_eq!(db.ledger_transition_trigger("t-1").unwrap(), None);
    assert_eq!(db.ledger_result_count_for_run("t-1", "run-1").unwrap(), 0);
}

/// Migration `103_disk_state_records` backfills every open task's
/// task.json. Interrupted part way and resumed, it leaves exactly what an
/// uninterrupted backfill leaves; running it again changes nothing but the
/// snapshot revision.
#[test]
fn an_interrupted_backfill_resumes_to_the_same_records() {
    let (db, path) = migrated("task-state-backfill");
    let root = crate::task_store::root_for_db(&path);
    for id in ["t-1", "t-2", "t-3"] {
        task(&db, id);
        run(&db, &format!("{id}-run"), id, "main");
    }
    db.conn
        .execute(
            "UPDATE pipeline_item SET closed_at = '2026-09-23 01:00:00' WHERE id = 't-3'",
            [],
        )
        .unwrap();
    assert!(crate::task_store::flush_all(&db, &path).is_empty());
    let task_json = |id: &str| crate::task_store::task_dir(&root, "repo-1", id).join("task.json");
    let read = |id: &str| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(task_json(id)).unwrap()).unwrap()
    };
    let without_revision = |mut value: serde_json::Value| {
        value.as_object_mut().unwrap().remove("snapshot_revision");
        value
    };
    let expected: Vec<serde_json::Value> = ["t-1", "t-2"]
        .iter()
        .map(|id| without_revision(read(id)))
        .collect();

    // A database as the previous build left it: no migration 103, and
    // task.json files without `state`.
    let downgrade = |db: &Db| {
        let mut statement = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' AND name LIKE 'disk_state_%'")
            .unwrap();
        let triggers: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        for trigger in triggers {
            db.conn
                .execute_batch(&format!("DROP TRIGGER {trigger}"))
                .unwrap();
        }
        db.conn
            .execute_batch(
                "DROP TABLE repo_disk_snapshot;
                 DROP TABLE disk_record_removal;
                 DELETE FROM schema_migrations WHERE id = '103_disk_state_records';
                 UPDATE task_ledger_snapshot SET published_revision = revision;",
            )
            .unwrap();
    };
    downgrade(&db);
    for id in ["t-1", "t-2", "t-3"] {
        let mut old = read(id);
        old.as_object_mut().unwrap().remove("state");
        std::fs::write(task_json(id), old.to_string()).unwrap();
    }
    drop(db);

    let db = Db::open_migrated(&path).unwrap();
    let mut owed = db.ledger_tasks_with_pending_work().unwrap();
    owed.sort();
    assert_eq!(owed, vec!["t-1", "t-2"], "only open tasks are backfilled");
    assert_eq!(db.repos_with_pending_disk_record().unwrap(), vec!["repo-1"]);

    // Interrupted: the first task.json fails to publish.
    crate::task_store::inject_fault(&root, crate::task_store::FlushFault::BeforeSnapshot);
    let failures = crate::task_store::flush_all(&db, &path);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(db.ledger_tasks_with_pending_work().unwrap().len(), 1);
    // Resumed.
    assert!(crate::task_store::flush_all(&db, &path).is_empty());
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
    let resumed: Vec<serde_json::Value> = ["t-1", "t-2"]
        .iter()
        .map(|id| without_revision(read(id)))
        .collect();
    assert_eq!(resumed, expected);
    assert!(
        read("t-3").get("state").is_none(),
        "a closed task is left alone"
    );

    // Run again from the start: the same records.
    downgrade(&db);
    drop(db);
    let db = Db::open_migrated(&path).unwrap();
    assert!(crate::task_store::flush_all(&db, &path).is_empty());
    let again: Vec<serde_json::Value> = ["t-1", "t-2"]
        .iter()
        .map(|id| without_revision(read(id)))
        .collect();
    assert_eq!(again, expected);
}
