use super::*;
use crate::db::{Db, ReplaceTaskBlockersError};

fn test_db(label: &str) -> Db {
    let db = Db::open_for_tests(&Db::test_db_path(label)).unwrap();
    db.insert_test_repo("repo-1", "Repo One").unwrap();
    db
}

fn task(db: &Db, id: &str, stages: &[&str]) {
    db.insert_test_pipeline_item(id, "repo-1", id, Some(id), stages[0], "2026-09-23 00:00:00")
        .unwrap();
    db.pin_test_stages(id, stages);
}

fn edge(upstream: &str, stage: &str, dependent_stage: Option<&str>) -> NewStageEdge {
    NewStageEdge {
        upstream_task_id: upstream.to_string(),
        upstream_stage: stage.to_string(),
        dependent_stage: dependent_stage.map(str::to_string),
    }
}

/// Leave `from` for `to` the way a completion does: a result, then the
/// transition it triggers.
fn depart(db: &Db, task_id: &str, from: &str, to: &str, status: &str, sha: &str) -> String {
    let result = db.record_test_stage_result(task_id, from, status, Some(sha));
    db.update_pipeline_item_stage(task_id, to).unwrap();
    result
}

fn events(db: &Db, task_id: &str, kind: &str) -> Vec<serde_json::Value> {
    let mut stmt = db
        .conn
        .prepare("SELECT payload FROM task_event WHERE task_id = ? AND type = ? ORDER BY seq")
        .unwrap();
    stmt.query_map(params![task_id, kind], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|payload| serde_json::from_str(&payload.unwrap()).unwrap())
        .collect()
}

fn transition_bodies(db: &Db, task_id: &str) -> Vec<Value> {
    let mut bodies = db.ledger_bodies(task_id, "transition").unwrap();
    bodies.reverse();
    bodies
}

#[test]
fn edge_is_satisfied_only_by_a_forward_departure_with_a_success_result() {
    let db = test_db("stage-edge-forward-success");
    task(&db, "task-a", &["plan", "build", "pr"]);
    task(&db, "task-b", &["work"]);
    let edges = db
        .insert_stage_edges("task-b", &[edge("task-a", "plan", None)])
        .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].dependent_stage, "work");
    let edge = edges[0].clone();
    assert_eq!(
        db.stage_edge_satisfaction(&edge).unwrap(),
        EdgeSatisfaction::Pending
    );

    // Leaving with a non-success result does not satisfy it.
    depart(&db, "task-a", "plan", "build", "partial", "1111111");
    assert_eq!(
        db.stage_edge_satisfaction(&edge).unwrap(),
        EdgeSatisfaction::Pending
    );
    // Neither does a loop back into the stage, nor a success recorded in it
    // while the task has not left it again.
    depart(&db, "task-a", "build", "plan", "success", "2222222");
    db.record_test_stage_result("task-a", "plan", "success", Some("3333333"));
    assert_eq!(
        db.stage_edge_satisfaction(&edge).unwrap(),
        EdgeSatisfaction::Pending
    );

    let result = depart(&db, "task-a", "plan", "build", "success", "4444444");
    assert_eq!(
        db.stage_edge_satisfaction(&edge).unwrap(),
        EdgeSatisfaction::Satisfied(StageEdgeInput {
            result_id: Some(result),
            committed_sha: Some("4444444".to_string()),
        })
    );
}

#[test]
fn final_stage_edge_waits_for_upstream_closure() {
    let db = test_db("stage-edge-final-closure");
    task(&db, "task-a", &["build", "pr"]);
    task(&db, "task-b", &["work"]);
    let edge = db
        .insert_stage_edges("task-b", &[edge("task-a", "pr", None)])
        .unwrap()
        .remove(0);
    depart(&db, "task-a", "build", "pr", "success", "aaaaaaa");
    let pr_result = db.record_test_stage_result("task-a", "pr", "success", Some("bbbbbbb"));
    // A success in the final stage is not enough: the task must close.
    assert_eq!(
        db.stage_edge_satisfaction(&edge).unwrap(),
        EdgeSatisfaction::Pending
    );
    assert_eq!(
        db.list_waiting_stage_edge_upstreams("task-b").unwrap(),
        vec!["task-a".to_string()]
    );

    db.close_pipeline_item("task-a").unwrap();
    assert_eq!(
        db.stage_edge_satisfaction(&edge).unwrap(),
        EdgeSatisfaction::Satisfied(StageEdgeInput {
            result_id: Some(pr_result),
            committed_sha: Some("bbbbbbb".to_string()),
        })
    );
    assert!(db
        .list_waiting_stage_edge_upstreams("task-b")
        .unwrap()
        .is_empty());
    // The derived blocked state followed: blocked on install, unblocked by
    // the close.
    let blocked = events(&db, "task-b", "task.blocked");
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["blockerTaskIds"], serde_json::json!(["task-a"]));
    assert_eq!(events(&db, "task-b", "task.unblocked").len(), 1);
}

#[test]
fn later_stage_edge_is_consumed_on_entry_and_recorded_as_a_gate() {
    let db = test_db("stage-edge-later-gate");
    task(&db, "task-a", &["plan", "build"]);
    task(&db, "task-b", &["work", "review"]);
    db.insert_stage_edges("task-b", &[edge("task-a", "plan", Some("review"))])
        .unwrap();
    // The starting stage has no edge: nothing waits there.
    assert_eq!(
        db.stage_edge_inputs("task-b", "work", true).unwrap(),
        Some(Vec::new())
    );
    assert_eq!(
        db.stage_edge_inputs("task-b", "review", false).unwrap(),
        None
    );
    assert_eq!(
        db.unsatisfied_stage_edges_into("task-b", "review")
            .unwrap()
            .len(),
        1
    );

    let result = depart(&db, "task-a", "plan", "build", "success", "c0ffee1");
    db.update_pipeline_item_stage("task-b", "review").unwrap();

    let stored = db.list_stage_edges_into("task-b").unwrap().remove(0);
    assert_eq!(stored.consumed_result_id.as_deref(), Some(result.as_str()));
    assert_eq!(stored.consumed_sha.as_deref(), Some("c0ffee1"));
    let entry = transition_bodies(&db, "task-b").pop().unwrap();
    assert_eq!(entry["to_stage"], "review");
    assert_eq!(
        entry["dependencies"],
        serde_json::json!([{
            "upstream_task_id": "task-a",
            "upstream_stage": "plan",
            "dependent_stage": "review",
            "position": 1,
            "role": "gate",
            "result_id": result,
            "committed_sha": "c0ffee1",
        }])
    );
    // task.json lists the edge with what it consumed.
    let facts = db.task_snapshot_facts("task-b").unwrap().unwrap();
    assert_eq!(
        facts["links"]["stage_dependencies"][0]["consumed_sha"],
        "c0ffee1"
    );
}

#[test]
fn newer_upstream_success_records_supersession_and_changes_nothing_downstream() {
    let db = test_db("stage-edge-supersession");
    task(&db, "task-a", &["plan", "build"]);
    task(&db, "task-b", &["work", "review"]);
    db.insert_stage_edges("task-b", &[edge("task-a", "plan", Some("review"))])
        .unwrap();
    let first = depart(&db, "task-a", "plan", "build", "success", "1000001");

    // Before the dependent consumed anything, a newer departure is simply
    // what it will consume: no supersession.
    depart(&db, "task-a", "build", "plan", "success", "1000002");
    let second = depart(&db, "task-a", "plan", "build", "success", "1000003");
    assert!(events(&db, "task-b", "task.dependency_superseded").is_empty());
    assert_ne!(first, second);

    db.update_pipeline_item_stage("task-b", "review").unwrap();
    let consumed = db.list_stage_edges_into("task-b").unwrap().remove(0);
    assert_eq!(
        consumed.consumed_result_id.as_deref(),
        Some(second.as_str())
    );
    let b_before = db.get_pipeline_item("task-b").unwrap().unwrap();
    let b_transitions = transition_bodies(&db, "task-b").len();

    depart(&db, "task-a", "build", "plan", "success", "1000004");
    let third = depart(&db, "task-a", "plan", "build", "success", "1000005");

    let edge = db.list_stage_edges_into("task-b").unwrap().remove(0);
    assert_eq!(edge.consumed_result_id.as_deref(), Some(second.as_str()));
    assert_eq!(edge.consumed_sha.as_deref(), Some("1000003"));
    assert_eq!(edge.superseded_result_id.as_deref(), Some(third.as_str()));
    assert_eq!(edge.superseded_sha.as_deref(), Some("1000005"));
    let superseded = events(&db, "task-b", "task.dependency_superseded");
    assert_eq!(superseded.len(), 1);
    assert_eq!(superseded[0]["upstreamTaskId"], "task-a");
    assert_eq!(superseded[0]["consumedResultId"], second);
    assert_eq!(superseded[0]["supersedingResultId"], third);
    assert_eq!(superseded[0]["supersedingSha"], "1000005");

    // No downstream effect: same stage, branch and base, no transition, no
    // wait, no run.
    let b_after = db.get_pipeline_item("task-b").unwrap().unwrap();
    assert_eq!(b_after.stage, b_before.stage);
    assert_eq!(b_after.branch, b_before.branch);
    assert_eq!(b_after.base_ref, b_before.base_ref);
    assert_eq!(transition_bodies(&db, "task-b").len(), b_transitions);
    assert!(db.dependency_wait("task-b").unwrap().is_none());
    assert!(db.latest_stage_run("task-b").unwrap().is_none());

    // The same departure seen again announces nothing new.
    db.record_stage_edge_departure("task-a", "plan", "build", Some(&third))
        .unwrap();
    assert_eq!(events(&db, "task-b", "task.dependency_superseded").len(), 1);
}

#[test]
fn cycles_are_refused_through_stage_paths_and_legacy_blockers() {
    let db = test_db("stage-edge-cycles");
    for id in ["task-a", "task-b", "task-c"] {
        task(&db, id, &["s1", "s2"]);
    }
    assert!(matches!(
        db.insert_stage_edges("task-a", &[edge("task-a", "s1", Some("s2"))]),
        Err(StageEdgeError::SelfDependency)
    ));
    // A.s2 → B.s1, B.s2 → C.s1; C.s1 → A.s1 would close A→B→C→A.
    db.insert_stage_edges("task-b", &[edge("task-a", "s2", Some("s1"))])
        .unwrap();
    db.insert_stage_edges("task-c", &[edge("task-b", "s2", Some("s1"))])
        .unwrap();
    assert!(matches!(
        db.insert_stage_edges("task-a", &[edge("task-c", "s1", Some("s1"))]),
        Err(StageEdgeError::CircularDependency)
    ));
    // The refused edge was not kept.
    assert!(db.list_stage_edges_into("task-a").unwrap().is_empty());
    // A legacy blocker closing the same loop is refused too.
    assert!(matches!(
        db.replace_task_blockers_atomically("task-a", &["task-c".to_string()]),
        Err(ReplaceTaskBlockersError::CircularDependency)
    ));
    assert!(db.list_task_blocker_ids("task-a").unwrap().is_empty());
    // And a stage edge closing a loop made of a legacy blocker.
    task(&db, "task-d", &["s1", "s2"]);
    db.replace_task_blockers_atomically("task-d", &["task-c".to_string()])
        .unwrap();
    assert!(matches!(
        db.insert_stage_edges("task-a", &[edge("task-d", "s2", Some("s1"))]),
        Err(StageEdgeError::CircularDependency)
    ));
}

#[test]
fn tasks_gating_each_other_at_unrelated_stages_are_not_a_cycle() {
    let db = test_db("stage-edge-mutual-stages");
    task(&db, "task-a", &["plan", "build"]);
    task(&db, "task-b", &["plan", "build"]);
    // B builds on A's plan and A builds on B's plan: both plans can finish
    // first, so nothing waits on itself.
    db.insert_stage_edges("task-b", &[edge("task-a", "plan", Some("build"))])
        .unwrap();
    db.insert_stage_edges("task-a", &[edge("task-b", "plan", Some("build"))])
        .unwrap();
    // B's plan waiting on A's build would: A's build needs B's plan.
    task(&db, "task-c", &["plan", "build"]);
    db.insert_stage_edges("task-c", &[edge("task-a", "plan", Some("build"))])
        .unwrap();
    assert!(matches!(
        db.insert_stage_edges("task-b", &[edge("task-a", "build", Some("plan"))]),
        Err(StageEdgeError::CircularDependency)
    ));
}

#[test]
fn edges_keep_their_order_and_validate_stages() {
    let db = test_db("stage-edge-order");
    task(&db, "task-a", &["plan", "build"]);
    task(&db, "task-b", &["plan", "build"]);
    task(&db, "task-c", &["work"]);
    let edges = db
        .insert_stage_edges(
            "task-c",
            &[edge("task-b", "build", None), edge("task-a", "plan", None)],
        )
        .unwrap();
    assert_eq!(
        edges
            .iter()
            .map(|edge| (edge.upstream_task_id.as_str(), edge.position))
            .collect::<Vec<_>>(),
        vec![("task-b", 1), ("task-a", 2)]
    );
    assert!(matches!(
        db.insert_stage_edges("task-c", &[edge("task-a", "deploy", None)]),
        Err(StageEdgeError::StageNotFound { .. })
    ));
    assert!(matches!(
        db.insert_stage_edges("task-c", &[edge("task-a", "plan", Some("review"))]),
        Err(StageEdgeError::StageNotFound { .. })
    ));
    assert!(matches!(
        db.insert_stage_edges("task-c", &[edge("task-missing", "plan", None)]),
        Err(StageEdgeError::UpstreamNotFound(_))
    ));

    depart(&db, "task-b", "plan", "build", "success", "b0b0b0b");
    db.close_pipeline_item("task-b").unwrap();
    depart(&db, "task-a", "plan", "build", "success", "a0a0a0a");
    let inputs = db
        .stage_edge_inputs("task-c", "work", true)
        .unwrap()
        .unwrap();
    assert_eq!(
        inputs
            .iter()
            .map(|input| (input.role, input.input.committed_sha.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            (DependencyRole::Base, None),
            (DependencyRole::Merge, Some("a0a0a0a")),
        ]
    );
}

#[test]
fn a_transition_settles_a_parked_completion_and_its_blocked_state() {
    let db = test_db("stage-edge-wait");
    task(&db, "task-a", &["plan", "build"]);
    task(&db, "task-b", &["work", "review"]);
    db.upsert_worktree("wt-task-b", "task-b", "/tmp/task-b", "branch-task-b")
        .unwrap();
    db.insert_stage_edges("task-b", &[edge("task-a", "plan", Some("review"))])
        .unwrap();
    // Started and not waiting: an edge into a later stage blocks nothing yet.
    assert!(db
        .list_waiting_stage_edge_upstreams("task-b")
        .unwrap()
        .is_empty());
    db.record_dependency_wait(
        "task-b",
        "work",
        "review",
        &serde_json::json!({"kind": "main"}),
    )
    .unwrap();
    assert_eq!(
        db.list_waiting_stage_edge_upstreams("task-b").unwrap(),
        vec!["task-a".to_string()]
    );
    let wait = db.dependency_wait("task-b").unwrap().unwrap();
    assert!(db.dependency_wait_is_current(&wait).unwrap());
    assert_eq!(events(&db, "task-b", "task.blocked").len(), 1);

    depart(&db, "task-a", "plan", "build", "success", "d00d00d");
    assert_eq!(events(&db, "task-b", "task.unblocked").len(), 1);
    db.update_pipeline_item_stage("task-b", "review").unwrap();
    assert!(db.dependency_wait("task-b").unwrap().is_none());
}
