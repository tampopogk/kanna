//! The standing-constraints surface, driven through the real router.
//!
//! These tests exercise the boundary a supervising agent actually reaches:
//! `kanna_tool_catalog` resolves the request, the axum router serves it, and
//! SQLite stores it. Nothing here is mocked except the daemon-backed input
//! delivery, which is the one thing a test process cannot own.

use super::*;
use crate::db::{StandingConstraintKind, StandingConstraintSource};
use std::sync::Arc;

fn seed_supervised_repo(db: &Db) {
    db.insert_test_repo("repo-sc", "Constraint Repo")
        .expect("repo");
    for (task_id, title) in [
        ("manager-1", "Task manager"),
        ("task-7", "Owner-driven work"),
        ("task-9", "Ordinary work"),
    ] {
        db.insert_test_pipeline_item(
            task_id,
            "repo-sc",
            "Prompt",
            Some(title),
            "in progress",
            "2026-09-17 00:00:00",
        )
        .expect("task");
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(path);
    let request = match body {
        Some(body) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(body.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// Resolve a catalog tool and drive the router with exactly what it produced.
///
/// Neither `kanna-mcp` nor `kanna-cli` hand-writes these routes: both send
/// whatever the shared catalog yields. A path or key that drifts from the
/// router turns a supervisor's "what am I under?" into a 404, which reads
/// exactly like "no constraints" — the one answer this record exists to stop
/// being manufactured.
async fn call_tool(
    app: &axum::Router,
    tool: &str,
    args: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let catalog = kanna_tool_catalog::bundled_catalog();
    let resolved = kanna_tool_catalog::resolve_request(&catalog, tool, &args)
        .unwrap_or_else(|error| panic!("the bundled catalog must expose {tool}: {error}"));
    let method = match resolved.method {
        kanna_tool_catalog::Method::Get => "GET",
        kanna_tool_catalog::Method::Post => "POST",
        kanna_tool_catalog::Method::Patch => "PATCH",
    };
    let body = (resolved.method != kanna_tool_catalog::Method::Get).then_some(resolved.body);
    call(app, method, &resolved.path, body).await
}

#[tokio::test]
async fn catalog_tools_record_read_and_clear_a_standing_constraint() {
    let state = test_state_with_seed("desktop-sc", "Constraint Mac", seed_supervised_repo);
    let app = router(Arc::clone(&state));
    let db = Db::open(&state.config.db_path).unwrap();
    let cursor = db.latest_task_event_seq().unwrap();

    // Nothing recorded yet, and the empty answer says so unambiguously: no
    // active constraints and no cleared history hidden behind a flag.
    let (status, empty) = call_tool(
        &app,
        "kanna_standing_constraints",
        serde_json::json!({ "repo_id": "repo-sc" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["repoId"], "repo-sc");
    assert_eq!(empty["activeCount"], 0);
    assert_eq!(empty["clearedTotal"], 0);
    assert_eq!(empty["constraints"], serde_json::json!([]));
    assert!(empty.get("cleared").is_none());

    let (status, set) = call_tool(
        &app,
        "kanna_set_standing_constraint",
        serde_json::json!({
            "repo_id": "repo-sc",
            "kind": "stand-down",
            "text": "  Owner is driving task-7 directly; stand down until they say otherwise.  ",
            "subject_task_id": "task-7",
            "declared_by": "operator",
            "declared_by_task_id": "manager-1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(set["created"], true);
    assert_eq!(set["announcedOnTaskId"], "task-7");
    let constraint = &set["constraint"];
    let constraint_id = constraint["id"].as_str().expect("id").to_string();
    assert_eq!(constraint["kind"], "stand-down");
    assert_eq!(
        constraint["text"],
        "Owner is driving task-7 directly; stand down until they say otherwise."
    );
    assert_eq!(constraint["subjectTaskId"], "task-7");
    assert_eq!(constraint["declaredBy"], "operator");
    assert_eq!(constraint["declaredByTaskId"], "manager-1");
    assert_eq!(constraint["repoId"], "repo-sc");
    assert!(constraint["createdAt"]
        .as_str()
        .is_some_and(|at| !at.is_empty()));
    assert!(constraint.get("clearedAt").is_none());

    // The keys MCP and CLI consumers read, pinned so a rename is a test
    // failure rather than a field a supervisor silently stops seeing.
    let mut keys = constraint
        .as_object()
        .expect("a constraint is an object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "createdAt",
            "declaredBy",
            "declaredByTaskId",
            "id",
            "kind",
            "repoId",
            "subjectTaskId",
            "text",
        ]
    );

    // Restating what was just read back is recovery, not a second decision.
    let (status, again) = call_tool(
        &app,
        "kanna_set_standing_constraint",
        serde_json::json!({
            "repo_id": "repo-sc",
            "kind": "stand-down",
            "text": "Owner is driving task-7 directly; stand down until they say otherwise.",
            "subject_task_id": "task-7",
            "declared_by": "operator",
            "declared_by_task_id": "manager-1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["created"], false);
    assert_eq!(again["constraint"]["id"], constraint_id.as_str());

    let (status, listed) = call_tool(
        &app,
        "kanna_standing_constraints",
        serde_json::json!({ "repo_id": "repo-sc" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["activeCount"], 1);
    assert_eq!(listed["constraints"][0]["id"], constraint_id.as_str());

    let (status, cleared) = call_tool(
        &app,
        "kanna_clear_standing_constraint",
        serde_json::json!({
            "constraint_id": constraint_id.clone(),
            "cleared_by": "operator",
            "cleared_by_task_id": "manager-1",
            "note": "Owner handed the task back.",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["cleared"], true);
    assert_eq!(cleared["announcedOnTaskId"], "task-7");
    assert_eq!(cleared["constraint"]["clearedBy"], "operator");
    assert_eq!(
        cleared["constraint"]["clearedNote"],
        "Owner handed the task back."
    );

    // A lifted gate is not the same fact as a gate that never existed, so the
    // listing reports the history even when it does not ship it.
    let (status, after) = call_tool(
        &app,
        "kanna_standing_constraints",
        serde_json::json!({ "repo_id": "repo-sc" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after["activeCount"], 0);
    assert_eq!(after["clearedTotal"], 1);
    assert!(after.get("cleared").is_none());

    let (status, history) = call_tool(
        &app,
        "kanna_standing_constraints",
        serde_json::json!({ "repo_id": "repo-sc", "include_cleared": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history["cleared"][0]["id"], constraint_id.as_str());
    assert_eq!(
        history["cleared"][0]["clearedNote"],
        "Owner handed the task back."
    );
    assert!(history["cleared"][0]["clearedAt"]
        .as_str()
        .is_some_and(|at| !at.is_empty()));

    // Clearing again decides nothing new and announces nothing new.
    let (status, noop) = call_tool(
        &app,
        "kanna_clear_standing_constraint",
        serde_json::json!({ "constraint_id": constraint_id.clone(), "cleared_by": "manager" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(noop["cleared"], false);
    assert_eq!(noop["constraint"]["clearedBy"], "operator");

    let head = db.latest_task_event_seq().unwrap();
    let events = db
        .list_task_events(
            &crate::db::TaskEventScope::Repo("repo-sc".to_string()),
            cursor,
            head,
            100,
        )
        .unwrap()
        .into_iter()
        .filter(|event| event.event_type.starts_with("task.standing_constraint"))
        .collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .map(|event| (event.event_type.as_str(), event.task_id.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("task.standing_constraint_set", "task-7"),
            ("task.standing_constraint_cleared", "task-7"),
        ]
    );
    assert_eq!(events[0].payload["constraintId"], constraint_id.as_str());

    let _ = std::fs::remove_file(&state.config.db_path);
}

#[tokio::test]
async fn a_constraint_that_cannot_name_what_it_protects_is_refused() {
    let state = test_state_with_seed("desktop-sc-refuse", "Constraint Mac", seed_supervised_repo);
    let app = router(Arc::clone(&state));

    let base = serde_json::json!({
        "repoId": "repo-sc",
        "kind": "stand-down",
        "text": "Stand down.",
    });

    // A subject that names no task would read as protection and provide none.
    let mut unknown_subject = base.clone();
    unknown_subject["subjectTaskId"] = serde_json::json!("task-does-not-exist");
    let (status, body) = call(
        &app,
        "POST",
        "/v1/standing-constraints",
        Some(unknown_subject),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    let mut unknown_repo = base.clone();
    unknown_repo["repoId"] = serde_json::json!("repo-missing");
    let (status, _) = call(&app, "POST", "/v1/standing-constraints", Some(unknown_repo)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let mut blank = base.clone();
    blank["text"] = serde_json::json!("   ");
    let (status, body) = call(&app, "POST", "/v1/standing-constraints", Some(blank)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let mut bad_kind = base.clone();
    bad_kind["kind"] = serde_json::json!("advice");
    let (status, _) = call(&app, "POST", "/v1/standing-constraints", Some(bad_kind)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The engine authors wakes, never decisions, so its reserved provenance is
    // not something an API caller can claim here.
    let mut engine_source = base.clone();
    engine_source["declaredBy"] = serde_json::json!("engine");
    let (status, _) = call(
        &app,
        "POST",
        "/v1/standing-constraints",
        Some(engine_source),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = call(
        &app,
        "POST",
        "/v1/standing-constraints/sc-missing/clear",
        Some(serde_json::json!({ "clearedBy": "operator" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let db = Db::open(&state.config.db_path).unwrap();
    assert!(db
        .list_active_standing_constraints("repo-sc")
        .unwrap()
        .is_empty());

    let _ = std::fs::remove_file(&state.config.db_path);
}

/// The reason this record exists, tested across the boundary it exists to
/// survive.
///
/// A supervising manager declares a stand-down in one session. That session's
/// conversation then ends — compaction, a stage fork, a restart; the record
/// cannot tell them apart and neither can the manager. A **second** manager,
/// holding nothing at all from the first, reconstructs its supervision state
/// from durable sources only — the task's prompt, its delivered-input ledger,
/// and the constraints record — and must decline an intervention the recorded
/// stand-down forbids while still acting on an unconstrained task.
///
/// Everything here is the real wiring: the shared catalog resolves each
/// request, the real router serves it, and real SQLite stores it. The one
/// stand-in is the manager's judgment, expressed as an explicit gate over the
/// loaded record — an LLM's reasoning is not testable in-process, but whether
/// the record it needs survives the boundary and is sufficient to decide is
/// exactly what this asserts. The gate lives in the test, never in the server:
/// constraints are advisory facts, and Kanna refusing an action because of one
/// would be the enforcement engine this deliberately is not.
#[tokio::test]
async fn a_manager_with_no_prior_conversation_declines_what_a_recorded_stand_down_forbids() {
    let delivered = Arc::new(std::sync::Mutex::new(Vec::new()));
    let delivered_for_sender = Arc::clone(&delivered);
    let state = test_state_with_seed_and_task_input_sender(
        "desktop-sc-recovery",
        "Constraint Mac",
        |db| {
            seed_supervised_repo(db);
            // What the first manager was told, in the durable ledger a later
            // session reads. The ledger records that it was said; only the
            // constraints record says it is still in force.
            db.record_task_input(
                "manager-1",
                crate::db::TaskInputSource::Operator,
                "I'm working with task-7 directly. Stand down on it until I say otherwise.",
            )
            .expect("record operator directive")
            .expect("the seeded manager accepts a recorded input");
        },
        Arc::new(move |task_id, input| {
            delivered_for_sender.lock().unwrap().push((task_id, input));
            Ok(())
        }),
    );
    let app = router(Arc::clone(&state));

    // --- Session one: the manager records the constraint it was given. ------
    let (status, set) = call_tool(
        &app,
        "kanna_set_standing_constraint",
        serde_json::json!({
            "repo_id": "repo-sc",
            "kind": "stand-down",
            "text": "Owner is driving task-7 directly. Do not send it input or advance it until they say otherwise.",
            "subject_task_id": "task-7",
            "declared_by": "operator",
            "declared_by_task_id": "manager-1",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(set["created"], true);

    // --- The boundary. Session two starts holding nothing. -----------------
    // Its whole supervision state is what it can read back.
    let (status, detail) = call_tool(
        &app,
        "kanna_get_task",
        serde_json::json!({ "task_id": "manager-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let repo_id = detail["repoId"].as_str().expect("repo id").to_string();

    let (status, inputs) = call_tool(
        &app,
        "kanna_task_inputs",
        serde_json::json!({ "task_id": "manager-1" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inputs["total"], 1);

    let (status, constraints) = call_tool(
        &app,
        "kanna_standing_constraints",
        serde_json::json!({ "repo_id": repo_id.clone() }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(constraints["activeCount"], 1);

    /// The manager's judgment, made explicit: does anything standing forbid
    /// touching this task? Reading is all the server offers; deciding is the
    /// supervisor's, which is why this predicate is here and not in the
    /// server.
    fn stand_down_in_force(constraints: &serde_json::Value, task_id: &str) -> Option<String> {
        constraints["constraints"]
            .as_array()
            .expect("the active set is an array")
            .iter()
            .find(|constraint| {
                constraint["kind"] == StandingConstraintKind::StandDown.as_str()
                    && constraint["subjectTaskId"] == task_id
            })
            .map(|constraint| constraint["text"].as_str().unwrap_or_default().to_string())
    }

    let forbidden = stand_down_in_force(&constraints, "task-7")
        .expect("the stand-down survived the boundary and is legible");
    assert!(forbidden.contains("Do not send it input"));

    // So the intervention is declined: nothing is sent to task-7.
    assert!(stand_down_in_force(&constraints, "task-9").is_none());
    let (status, sent) = call_tool(
        &app,
        "kanna_send_task_input",
        serde_json::json!({ "task_id": "task-9", "input": "Status?", "source": "manager" }),
    )
    .await;
    assert!(status.is_success(), "{status}: {sent}");
    assert_eq!(
        *delivered.lock().unwrap(),
        vec![("task-9".to_string(), "Status?".to_string())],
        "the unconstrained task is still supervised; only the stood-down one is left alone"
    );

    // Without the record, the same session sees an ordinary idle task and a
    // ledger entry it cannot date against now — which is how a stand-down was
    // lost to compaction before this existed. Prove the record is what makes
    // the difference by clearing it and re-reading: the gate opens only on an
    // explicit clear, with its own provenance, never by forgetting.
    let constraint_id = constraints["constraints"][0]["id"]
        .as_str()
        .expect("constraint id")
        .to_string();
    let (status, cleared) = call_tool(
        &app,
        "kanna_clear_standing_constraint",
        serde_json::json!({
            "constraint_id": constraint_id,
            "cleared_by": "operator",
            "cleared_by_task_id": "manager-1",
            "note": "Owner handed task-7 back.",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["cleared"], true);

    let (status, after) = call_tool(
        &app,
        "kanna_standing_constraints",
        serde_json::json!({ "repo_id": repo_id, "include_cleared": true }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(stand_down_in_force(&after, "task-7").is_none());
    assert_eq!(after["clearedTotal"], 1);
    assert_eq!(
        after["cleared"][0]["clearedBy"],
        StandingConstraintSource::Operator.as_str()
    );
    assert_eq!(
        after["cleared"][0]["clearedNote"],
        "Owner handed task-7 back."
    );

    let _ = std::fs::remove_file(&state.config.db_path);
}
