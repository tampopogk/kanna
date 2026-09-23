use super::TRANSFER_IN_PROGRESS;
use crate::db::Db;
use serde_json::json;

fn test_db(label: &str) -> (Db, String) {
    let path = Db::test_db_path(label);
    let db = Db::open_for_tests(&path).unwrap();
    db.insert_test_repo("repo-1", "Repo One").unwrap();
    (db, path)
}

fn task(db: &Db, id: &str, stage: &str) {
    db.insert_test_pipeline_item(id, "repo-1", id, Some(id), stage, "2026-09-23 00:00:00")
        .unwrap();
}

fn blocker(db: &Db, task_id: &str) -> Option<String> {
    db.transfer_state_blocker(task_id).unwrap()
}

fn outgoing_transfer(db: &Db, transfer_id: &str, task_id: &str) {
    db.execute_test_sql(&format!(
        "INSERT INTO task_transfer (id, direction, status, source_peer_id, target_peer_id,
                                    source_task_id, local_task_id)
         VALUES ('{transfer_id}', 'outgoing', 'pending', 'peer-src', 'peer-dst',
                 '{task_id}', '{task_id}')"
    ))
    .unwrap();
}

#[test]
fn an_idle_task_with_settled_state_is_transferable() {
    let (db, _) = test_db("t9-blocker-idle");
    task(&db, "task-a", "build");
    task(&db, "task-up", "done");
    db.set_test_pipeline_item_closed_at("task-up", "2026-09-23 01:00:00")
        .unwrap();
    db.execute_test_sql(
        "INSERT INTO task_stage_edge (dependent_task_id, dependent_stage, upstream_task_id,
                                      upstream_stage, position, consumed_result_id, consumed_sha)
         VALUES ('task-a', 'build', 'task-up', 'done', 0, 'task-up-000001', 'abc');
         INSERT INTO transition_commit (run_id, task_id, stage, state)
         VALUES ('run-commit', 'task-a', 'build', 'succeeded');",
    )
    .unwrap();
    assert_eq!(blocker(&db, "task-a"), None);
}

#[test]
fn every_pending_transition_and_open_link_refuses_the_transfer() {
    let cases: [(&str, &str, &str); 8] = [
        (
            "continuation",
            "INSERT INTO task_ledger_continuation (task_id, operation_id, kind, payload)
             VALUES ('task-a', 'op-1', 'stage_completion', '{}')",
            "owes a stage transition",
        ),
        (
            "lifecycle",
            "INSERT INTO lifecycle_operation_intent (id, task_id, kind, phase, payload_json)
             VALUES ('run-1', 'task-a', 'stage_spawn', 'prepared', '{}')",
            "stage_spawn operation in flight",
        ),
        (
            "commit-step",
            "INSERT INTO transition_commit (run_id, task_id, stage)
             VALUES ('run-commit', 'task-a', 'build')",
            "committing its transition out of stage build",
        ),
        (
            "dependency-wait",
            "INSERT INTO task_dependency_wait (task_id, from_stage, to_stage, generation, payload)
             VALUES ('task-a', 'build', 'review', 0, '{}')",
            "waiting on its dependency edges to enter stage review",
        ),
        (
            "unconsumed-edge",
            "INSERT INTO task_stage_edge (dependent_task_id, dependent_stage, upstream_task_id,
                                          upstream_stage, position)
             VALUES ('task-a', 'review', 'task-other', 'plan', 0)",
            "depends on task task-other",
        ),
        (
            "open-dependent",
            "INSERT INTO task_stage_edge (dependent_task_id, dependent_stage, upstream_task_id,
                                          upstream_stage, position, consumed_result_id)
             VALUES ('task-other', 'plan', 'task-a', 'build', 0, 'task-a-000001')",
            "open task task-other depends on task task-a",
        ),
        (
            "open-join",
            "INSERT INTO task_join (id, parent_task_id, base_sha) VALUES ('join-1', 'task-a', 'abc');
             INSERT INTO task_join_member (join_id, position, child_task_id, spec)
             VALUES ('join-1', 1, 'child-1', '{}')",
            "waiting on subtask join join-1",
        ),
        (
            "unresolved-member",
            "INSERT INTO task_join (id, parent_task_id, base_sha) VALUES ('join-2', 'task-other', 'abc');
             INSERT INTO task_join_member (join_id, position, child_task_id, spec)
             VALUES ('join-2', 1, 'task-a', '{}')",
            "unresolved member of a subtask join its parent task-other",
        ),
    ];
    for (label, sql, expected) in cases {
        let (db, _) = test_db(&format!("t9-blocker-{label}"));
        task(&db, "task-a", "build");
        task(&db, "task-other", "plan");
        db.execute_test_sql(sql).unwrap();
        let reason = blocker(&db, "task-a").unwrap_or_else(|| panic!("{label} was not refused"));
        assert!(reason.contains(expected), "{label}: {reason}");
    }
}

#[test]
fn finalization_claim_refuses_a_pending_transition_and_takes_nothing() {
    let (db, _) = test_db("t9-claim-refused");
    task(&db, "task-a", "build");
    outgoing_transfer(&db, "transfer-1", "task-a");
    db.put_ledger_continuation("task-a", "op-1", "stage_completion", &json!({}))
        .unwrap();
    let refused = db
        .claim_task_workflow_for_transfer_finalization("transfer-1", "task-a")
        .unwrap()
        .unwrap_err();
    assert!(refused.contains("owes a stage transition"), "{refused}");
    assert!(refused.contains("source task untouched"), "{refused}");
    assert_eq!(
        db.task_workflow_is_claimed_by_transfer("task-a").unwrap(),
        None
    );
    // The legacy claim (the close path) is unchanged by the new check.
    db.claim_task_workflow_for_transfer("transfer-1", "task-a")
        .unwrap()
        .unwrap();
}

#[test]
fn a_claimed_task_refuses_every_write_that_would_move_it_until_the_transfer_ends() {
    let (db, path) = test_db("t9-claim-fences-writers");
    task(&db, "task-a", "build");
    outgoing_transfer(&db, "transfer-1", "task-a");
    db.claim_task_workflow_for_transfer_finalization("transfer-1", "task-a")
        .unwrap()
        .unwrap();

    let refusals = [
        db.put_ledger_continuation("task-a", "op-1", "stage_completion", &json!({}))
            .unwrap_err(),
        db.insert_lifecycle_operation_intent("run-1", "task-a", "stage_spawn", "prepared", "{}")
            .unwrap_err(),
        db.insert_transition_commit("run-2", "task-a", "build", None)
            .unwrap_err(),
        db.record_dependency_wait("task-a", "build", "review", &json!({}))
            .unwrap_err(),
    ];
    for refusal in refusals {
        assert!(
            refusal.to_string().contains(TRANSFER_IN_PROGRESS),
            "{refusal}"
        );
    }
    assert_eq!(blocker(&db, "task-a"), None, "nothing was recorded");

    // A source that crashes mid-finalization comes back still excluded: the
    // claim is durable and the transfer still owns the source.
    drop(db);
    let db = Db::open(&path).unwrap();
    assert_eq!(
        db.task_workflow_is_claimed_by_transfer("task-a").unwrap(),
        Some("transfer-1".to_string())
    );
    assert!(db
        .put_ledger_continuation("task-a", "op-1", "stage_completion", &json!({}))
        .is_err());

    // A transfer that failed with no source work left releases the task.
    db.fail_outgoing_task_transfer("transfer-1", "destination refused")
        .unwrap();
    db.put_ledger_continuation("task-a", "op-1", "stage_completion", &json!({}))
        .unwrap();
}

#[test]
fn carried_rows_round_trip_under_a_new_task_id() {
    let (source, _) = test_db("t9-rows-source");
    task(&source, "task-a", "review");
    task(&source, "task-up", "done");
    source
        .set_test_pipeline_item_closed_at("task-up", "2026-09-23 01:00:00")
        .unwrap();
    source.reserve_task_branch_number("task-a", 3).unwrap();
    source
        .upsert_stage_workspace(
            "ws-task-task-a",
            "task-a",
            "build",
            "/src/task-task-a",
            "task-task-a",
        )
        .unwrap();
    source
        .execute_test_sql(
            "INSERT INTO task_stage_budget (task_id, stage, spent) VALUES ('task-a', 'review', 2);
             INSERT INTO transition_commit (run_id, task_id, stage, exit, state, result_id,
                                            committed_sha, settled_at)
             VALUES ('run-commit', 'task-a', 'build', '{\"name\":\"advance\"}', 'succeeded',
                     'task-a-000002', 'abc123', '2026-09-23 02:00:00');
             INSERT INTO task_stage_edge (dependent_task_id, dependent_stage, upstream_task_id,
                                          upstream_stage, position, consumed_result_id,
                                          consumed_sha, consumed_at)
             VALUES ('task-a', 'build', 'task-up', 'done', 0, 'task-up-000004', 'def456',
                     '2026-09-23T01:00:00Z');
             INSERT INTO task_join (id, parent_task_id, parent_stage, base_sha, completed_at)
             VALUES ('join-1', 'task-a', 'build', 'abc123', '2026-09-23T03:00:00Z');
             INSERT INTO task_join_member (join_id, position, child_task_id, spec, resolved_at,
                                           outcome, result_id, result_status)
             VALUES ('join-1', 1, 'child-1', '{}', '2026-09-23T03:00:00Z', 'result',
                     'child-1-000001', 'success');",
        )
        .unwrap();
    let rows = source
        .export_carried_task_rows("task-a", "peer-src")
        .unwrap();
    assert_eq!(rows.branch_counter, Some(4));
    assert_eq!(rows.stage_budgets.len(), 1);
    assert_eq!(rows.transition_commits.len(), 1);
    assert_eq!(rows.links.stage_workspaces.len(), 1);
    assert_eq!(rows.links.stage_edges.len(), 1);
    assert_eq!(rows.links.joins.len(), 1);
    assert_eq!(rows.links.joins[0].members.len(), 1);
    assert_eq!(rows.ownership_generation, 0);

    let (destination, _) = test_db("t9-rows-destination");
    task(&destination, "task-b", "review");
    let ids = std::collections::HashMap::from([(
        "task-a-000002".to_string(),
        "task-b-000001".to_string(),
    )]);
    let links = rows.links.clone();
    let state = super::NewTransferredTaskState {
        task_id: "task-b",
        transfer_id: "transfer-1",
        source_peer_id: "peer-src",
        source_task_id: "task-a",
        ownership_generation: 1,
        state_sha256: "digest",
        links: &links,
        session_start: "fresh",
        fresh_start_reason: Some("no transcript"),
    };
    destination
        .import_carried_task_rows(&state, &rows, &ids)
        .unwrap();
    // A retried import writes the same facts once.
    destination
        .import_carried_task_rows(&state, &rows, &ids)
        .unwrap();
    let imported = destination
        .export_carried_task_rows("task-b", "peer-dst")
        .unwrap();
    assert_eq!(imported.branch_counter, Some(4));
    assert_eq!(imported.stage_budgets, rows.stage_budgets);
    assert_eq!(imported.transition_commits.len(), 1);
    let commit = &imported.transition_commits[0];
    assert_eq!(commit.result_id.as_deref(), Some("task-b-000001"));
    assert_eq!(commit.committed_sha.as_deref(), Some("abc123"));
    assert_eq!(commit.exit, rows.transition_commits[0].exit);
    assert_eq!(
        imported.links, rows.links,
        "inherited links re-export verbatim"
    );
    assert_eq!(imported.ownership_generation, 1);
    assert_eq!(
        destination.stage_budget_spent("task-b", "review").unwrap(),
        2
    );
    let recorded = destination
        .transferred_task_state("task-b")
        .unwrap()
        .unwrap();
    assert_eq!(recorded.session_start, "fresh");
    assert_eq!(
        recorded.fresh_start_reason.as_deref(),
        Some("no transcript")
    );

    // Different state for the same task is refused, never merged.
    let other = super::NewTransferredTaskState {
        state_sha256: "other",
        ..state
    };
    assert!(destination
        .import_carried_task_rows(&other, &rows, &ids)
        .is_err());
}
