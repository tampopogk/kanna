use super::{carried_run_id, CarriedTaskRows, CarriedTransitionCommit, TRANSFER_IN_PROGRESS};
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

fn settled_commit(run_id: &str) -> CarriedTransitionCommit {
    CarriedTransitionCommit {
        run_id: run_id.into(),
        stage: "build".into(),
        exit: None,
        state: "succeeded".into(),
        result_id: None,
        committed_sha: Some("abc123".into()),
        created_at: "2026-09-23 02:00:00".into(),
        settled_at: Some("2026-09-23 02:00:00".into()),
    }
}

fn import(
    db: &Db,
    task_id: &str,
    transfer_id: &str,
    rows: &CarriedTaskRows,
) -> Result<(), rusqlite::Error> {
    let links = rows.links.clone();
    db.import_carried_task_rows(
        &super::NewTransferredTaskState {
            task_id,
            transfer_id,
            source_peer_id: "peer",
            source_task_id: "source",
            ownership_generation: rows.ownership_generation + 1,
            state_sha256: transfer_id,
            links: &links,
            session_start: "resumed",
            fresh_start_reason: None,
        },
        rows,
        &Default::default(),
    )
}

#[test]
fn a_task_returning_to_its_first_machine_keeps_its_commit_step_bindings() {
    // Machine A: the original task still holds its binding under run r1.
    let (a, _) = test_db("t9-return-a");
    task(&a, "task-a1", "review");
    a.execute_test_sql(
        "INSERT INTO transition_commit (run_id, task_id, stage, state, committed_sha)
         VALUES ('r1', 'task-a1', 'build', 'succeeded', 'abc123');",
    )
    .unwrap();
    let from_a = a.export_carried_task_rows("task-a1", "peer-a").unwrap();
    assert_eq!(from_a.transition_commits[0].run_id, "r1");

    // Machine B holds it under a key of its own and exports the origin id.
    let (b, _) = test_db("t9-return-b");
    task(&b, "task-b", "review");
    import(&b, "task-b", "transfer-1", &from_a).unwrap();
    assert!(b
        .task_transition_commit("task-b", &carried_run_id("task-b", "r1"))
        .unwrap()
        .is_some());
    let from_b = b.export_carried_task_rows("task-b", "peer-b").unwrap();
    assert_eq!(from_b.transition_commits[0].run_id, "r1");

    // Back on A as a new task: the closed original's row does not swallow it.
    a.set_test_pipeline_item_closed_at("task-a1", "2026-09-24 00:00:00")
        .unwrap();
    task(&a, "task-a2", "review");
    import(&a, "task-a2", "transfer-2", &from_b).unwrap();
    let returned = a.export_carried_task_rows("task-a2", "peer-a").unwrap();
    assert_eq!(returned.transition_commits.len(), 1);
    assert_eq!(returned.transition_commits[0].run_id, "r1");
    assert_eq!(
        returned.transition_commits[0].committed_sha.as_deref(),
        Some("abc123")
    );
    let bound = a
        .task_transition_commit("task-a2", &carried_run_id("task-a2", "r1"))
        .unwrap()
        .expect("binding held by the returned task");
    assert_eq!(bound.state, "succeeded");
    assert_eq!(
        a.task_transition_commit("task-a1", "r1")
            .unwrap()
            .unwrap()
            .task_id,
        "task-a1"
    );
}

#[test]
fn a_carried_binding_never_touches_a_local_run_with_the_same_id() {
    let (db, _) = test_db("t9-binding-collision");
    task(&db, "task-local", "build");
    task(&db, "task-d", "review");
    // A local run r1 whose commit step is still requested.
    db.execute_test_sql(
        "INSERT INTO transition_commit (run_id, task_id, stage) VALUES ('r1', 'task-local', 'build');",
    )
    .unwrap();
    let rows = CarriedTaskRows {
        transition_commits: vec![settled_commit("r1")],
        ..Default::default()
    };
    import(&db, "task-d", "transfer-1", &rows).unwrap();
    let local = db
        .task_transition_commit("task-local", "r1")
        .unwrap()
        .unwrap();
    assert_eq!(local.state, "requested", "the local run is unaffected");
    assert!(db.task_transition_commit("task-d", "r1").unwrap().is_none());
    // A lookup for one task never answers with another task's binding.
    assert!(db
        .task_transition_commit("task-local", &carried_run_id("task-d", "r1"))
        .unwrap()
        .is_none());

    // A key another task already holds is refused, never ignored.
    task(&db, "task-e", "review");
    db.execute_test_sql(&format!(
        "INSERT INTO transition_commit (run_id, task_id, stage, state)
         VALUES ('{}', 'task-local', 'build', 'failed');",
        carried_run_id("task-e", "r2")
    ))
    .unwrap();
    let colliding = CarriedTaskRows {
        transition_commits: vec![settled_commit("r2")],
        ..Default::default()
    };
    let refused = import(&db, "task-e", "transfer-2", &colliding).unwrap_err();
    assert!(refused.to_string().contains("collides"), "{refused}");
    assert!(db.transferred_task_state("task-e").unwrap().is_none());
}

/// Every guarded writer holds its transaction from the guard's read to its
/// write: a finalization claim attempted in between cannot take the write
/// lock, and attempted afterwards is refused by what the writer left pending
/// — never both.
#[test]
fn a_claim_attempted_between_the_guard_and_the_write_cannot_land_beside_it() {
    type Write = fn(&Db, &str) -> Result<(), rusqlite::Error>;
    let writers: [(&str, Write); 3] = [
        ("continuation", |db, task| {
            db.put_ledger_continuation(task, "op-1", "stage_completion", &json!({}))
        }),
        ("lifecycle", |db, task| {
            db.insert_lifecycle_operation_intent("run-1", task, "stage_spawn", "prepared", "{}")
        }),
        ("commit-step", |db, task| {
            db.insert_transition_commit("run-2", task, "build", None)
        }),
    ];
    for (label, write) in writers {
        let (db, path) = test_db(&format!("t9-guard-gap-{label}"));
        let task_id = format!("task-gap-{label}");
        task(&db, &task_id, "build");
        outgoing_transfer(&db, "transfer-1", &task_id);
        let (claim_path, claim_task) = (path.clone(), task_id.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        super::after_transfer_guard::set(&task_id, move || {
            // In the gap: a claim that does not wait for a lock must fail to
            // start, because the writer already holds it. (Unguarded, it
            // would commit here and the write would land beside it.)
            let claimant = Db::open(&claim_path).unwrap();
            claimant.set_test_busy_timeout(std::time::Duration::ZERO);
            let in_gap =
                claimant.claim_task_workflow_for_transfer_finalization("transfer-1", &claim_task);
            sender.send(in_gap.map(|claim| claim.is_ok())).unwrap();
        });
        write(&db, &task_id).unwrap_or_else(|error| panic!("{label}: {error}"));
        let in_gap = receiver.recv().unwrap();
        assert!(
            matches!(&in_gap, Err(error) if error.to_string().contains("locked")),
            "{label}: a claim got in between the guard and the write: {in_gap:?}"
        );
        let claimed = Db::open(&path)
            .unwrap()
            .claim_task_workflow_for_transfer_finalization("transfer-1", &task_id)
            .unwrap();
        let refusal = claimed.expect_err(label);
        assert!(
            refusal.contains("source task untouched"),
            "{label}: {refusal}"
        );
        assert_eq!(
            db.task_workflow_is_claimed_by_transfer(&task_id).unwrap(),
            None
        );
    }
}

#[test]
fn a_claimed_task_gains_no_join_and_no_edge_at_either_end() {
    let (db, _) = test_db("t9-claim-fences-links");
    task(&db, "task-a", "build");
    task(&db, "task-b", "build");
    db.pin_test_stages("task-a", &["build"]);
    db.pin_test_stages("task-b", &["build"]);
    outgoing_transfer(&db, "transfer-1", "task-a");
    db.claim_task_workflow_for_transfer_finalization("transfer-1", "task-a")
        .unwrap()
        .unwrap();

    let join = db
        .create_task_join(&crate::db::NewTaskJoin {
            id: "join-1".into(),
            parent_task_id: "task-a".into(),
            parent_stage: Some("build".into()),
            parent_run_id: None,
            base_sha: "abc".into(),
            base_branch: None,
            members: vec![crate::db::NewJoinMember {
                child_task_id: "child-1".into(),
                spec: "{}".into(),
            }],
        })
        .unwrap_err();
    assert!(join.to_string().contains(TRANSFER_IN_PROGRESS), "{join}");
    let edge = |dependent: &str, upstream: &str| {
        db.insert_stage_edges(
            dependent,
            &[crate::db::NewStageEdge {
                upstream_task_id: upstream.into(),
                upstream_stage: "build".into(),
                dependent_stage: None,
            }],
        )
        .unwrap_err()
        .to_string()
    };
    let onto_upstream = edge("task-b", "task-a");
    assert!(
        onto_upstream.contains(TRANSFER_IN_PROGRESS),
        "{onto_upstream}"
    );
    let from_dependent = edge("task-a", "task-b");
    assert!(
        from_dependent.contains(TRANSFER_IN_PROGRESS),
        "{from_dependent}"
    );
    assert_eq!(blocker(&db, "task-a"), None, "nothing was recorded");
}

#[test]
fn nothing_is_appended_to_a_ledger_after_its_final_export() {
    let (db, _) = test_db("t9-ledger-export-fence");
    task(&db, "task-a", "build");
    let append = |db: &Db, source: &str| {
        db.enqueue_ledger_entry(crate::db::task_store::NewLedgerEntry {
            task_id: "task-a",
            kind: crate::db::task_store::LedgerEntryKind::Result,
            operation_id: None,
            source_kind: "test",
            source_id: source,
            source_origin: None,
            historical: false,
            recorded_at: None,
            run_id: None,
            declared_role: None,
            channel_identity: &crate::mutation_provenance::ChannelIdentity::Unknown,
            body: json!({"status": "blocked", "stage": "build"}),
            message: Some("late"),
            hold_events_after: None,
            reserved_sequence: None,
        })
    };
    append(&db, "before").unwrap();
    outgoing_transfer(&db, "transfer-1", "task-a");
    // Only the transfer holding the task may fence its ledger.
    assert!(db
        .fence_ledger_for_transfer_export("task-a", "transfer-1")
        .unwrap()
        .is_err());
    db.claim_task_workflow_for_transfer_finalization("transfer-1", "task-a")
        .unwrap()
        .unwrap();
    // Entries recorded while the agent wraps up, before the final export,
    // are still accepted and exported.
    append(&db, "wrap-up").unwrap();
    assert_eq!(
        db.fence_ledger_for_transfer_export("task-a", "transfer-1")
            .unwrap()
            .unwrap(),
        2
    );
    let refused = append(&db, "after-export").unwrap_err();
    assert!(
        refused.to_string().contains(TRANSFER_IN_PROGRESS),
        "{refused}"
    );
    assert_eq!(
        db.last_ledger_sequence("task-a").unwrap(),
        2,
        "nothing was lost or added"
    );
    // A replay of an entry already recorded is still answered.
    append(&db, "wrap-up").unwrap();

    // A transfer that ended releases the ledger.
    db.fail_outgoing_task_transfer("transfer-1", "destination refused")
        .unwrap();
    append(&db, "after-failure").unwrap();
}

/// Migration 102 applies after the parent's 101 on a fresh database, and
/// creates everything a transfer writes, including the final-export fence.
#[test]
fn a_migrated_database_holds_the_transfer_tables_and_can_fence_a_ledger() {
    let path = Db::test_db_path("t9-migrated");
    let db = Db::open_migrated(&path).unwrap();
    let position = |id: &str| {
        db.query_test_i64(&format!(
            "SELECT rowid FROM schema_migrations WHERE id = '{id}'"
        ))
    };
    assert!(position("101_subtask_joins") < position("102_transferred_task_state"));
    for table in ["transferred_task_state", "transfer_ledger_export"] {
        assert_eq!(
            db.query_test_i64(&format!(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '{table}'"
            )),
            1,
            "{table}"
        );
    }
    db.insert_test_repo("repo-1", "Repo One").unwrap();
    task(&db, "task-a", "build");
    outgoing_transfer(&db, "transfer-1", "task-a");
    db.claim_task_workflow_for_transfer_finalization("transfer-1", "task-a")
        .unwrap()
        .unwrap();
    assert_eq!(
        db.fence_ledger_for_transfer_export("task-a", "transfer-1")
            .unwrap()
            .unwrap(),
        0
    );
}
