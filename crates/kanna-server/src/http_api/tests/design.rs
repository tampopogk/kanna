//! Who may do what to an App Design session over HTTP: the task's agent
//! reads and edits through typed routes, only the person writes feedback and
//! syncs the document, only the desktop prepares an approval, and nothing
//! over HTTP confirms one.

use super::*;
use crate::design::service::tests::seed_design_task;
use serde_json::{json, Value};

fn request(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(
            body.map(|body| body.to_string()).unwrap_or_default(),
        ))
        .unwrap();
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            50_000,
        ))));
    request
}

/// The task's agent: a local process without the desktop's credential.
fn as_agent(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    request(method, path, body)
}

/// The desktop app: the same socket, plus its local control credential.
fn as_desktop(state: &AppState, method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let mut request = request(method, path, body);
    let token = state
        .local_task_events_token
        .clone()
        .expect("test credential");
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    request
}

/// A paired phone over the LAN.
fn as_phone(method: &str, path: &str, body: Option<Value>) -> Request<Body> {
    let mut request = request(method, path, body);
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
            [192, 168, 1, 30],
            50_000,
        ))));
    request
        .extensions_mut()
        .insert(super::super::lan_trust::TrustedLanDeviceAccess::new(
            "phone-1".into(),
        ));
    request
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    // A plain-text refusal reads as a JSON string.
    let body = serde_json::from_slice(&body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()));
    (status, body)
}

fn setup(label: &str) -> (Arc<AppState>, axum::Router) {
    let state = test_state_with_seed(&format!("design-http-{label}"), "Design", seed_design_task);
    let app = router(Arc::clone(&state));
    (state, app)
}

fn thread(id: &str) -> Value {
    json!({ "threadId": id, "commentId": format!("{id}-c"), "kind": "message", "body": "please change it" })
}

#[tokio::test]
async fn only_the_person_writes_feedback_and_the_agent_reads_it() {
    let (state, app) = setup("feedback");
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/design/threads",
            Some(thread("t-a")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["reason"], "operator_only");

    let (status, body) = send(
        &app,
        as_desktop(
            &state,
            "POST",
            "/v1/tasks/task-d/design/threads",
            Some(thread("t-d")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["number"], 1);
    assert_eq!(body["deliveryStatus"], "queued");

    let (status, body) = send(
        &app,
        as_phone(
            "POST",
            "/v1/tasks/task-d/design/threads",
            Some(thread("t-p")),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["number"], 2);

    let (status, body) = send(&app, as_agent("GET", "/v1/tasks/task-d/design/agent", None)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["threads"].as_array().unwrap().len(), 2);
    assert_eq!(body["document"].as_array().unwrap().len(), 1);

    // The agent answers through its own route; the answer is not queued back.
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/design/agent/threads/t-d/replies",
            Some(json!({ "opId": "reply-1", "body": "Done." })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deliveryStatus"], "agent_replied");
    let deliveries = Db::open(&state.config.db_path)
        .unwrap()
        .design_deliveries("task-d")
        .unwrap();
    assert_eq!(deliveries.len(), 2);
}

#[tokio::test]
async fn the_agent_edits_through_typed_operations_never_raw_sync() {
    let (state, app) = setup("sync");
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/design/document/sync",
            Some(json!({ "schemaVersion": crate::design::document::SCHEMA_VERSION, "stateVector": "", "update": "AAA=" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    // Reading the raw document is fine for any client with task access.
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/design/document/read",
            Some(json!({ "schemaVersion": crate::design::document::SCHEMA_VERSION, "stateVector": "" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["update"]
        .as_str()
        .is_some_and(|update| !update.is_empty()));
    // A client on another schema is refused before touching anything.
    let (status, body) = send(
        &app,
        as_desktop(
            &state,
            "POST",
            "/v1/tasks/task-d/design/document/sync",
            Some(json!({ "schemaVersion": "blocknote@0.1", "stateVector": "" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");

    let (_, view) = send(&app, as_agent("GET", "/v1/tasks/task-d/design/agent", None)).await;
    let block = view["document"][0]["id"].as_str().unwrap().to_string();
    let edit = json!({
        "opId": "edit-1",
        "ops": [{ "op": "replace_text", "block_id": block, "expected_text": "", "text": "Agent wrote this" }],
    });
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/design/agent/edits",
            Some(edit.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["status"], "applied");
    let (_, replay) = send(
        &app,
        as_agent("POST", "/v1/tasks/task-d/design/agent/edits", Some(edit)),
    )
    .await;
    assert_eq!(replay["replayed"], true);
    let (_, view) = send(&app, as_agent("GET", "/v1/tasks/task-d/design/agent", None)).await;
    assert_eq!(view["document"][0]["text"], "Agent wrote this");
}

#[tokio::test]
async fn positions_move_without_a_transition() {
    let (_state, app) = setup("position");
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/design/position",
            Some(json!({ "position": "interactive" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, view) = send(&app, as_agent("GET", "/v1/tasks/task-d/design", None)).await;
    assert_eq!(view["position"], "interactive");
    assert_eq!(view["currentStage"], "design");
    let (status, _) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/design/position",
            Some(json!({ "position": "nowhere" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn only_the_desktop_prepares_an_approval_and_no_route_confirms_one() {
    let (_state, app) = setup("approval");
    for request in [
        as_agent("POST", "/v1/tasks/task-d/design/approval/candidate", None),
        as_phone("POST", "/v1/tasks/task-d/design/approval/candidate", None),
        as_agent("POST", "/v1/tasks/task-d/design/approval/reopen", None),
    ] {
        let (status, body) = send(&app, request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["reason"], "desktop_only");
    }
    for path in [
        "/v1/tasks/task-d/design/approval/confirm",
        "/v1/tasks/task-d/design/approval/ap-1/confirm",
    ] {
        let (status, _) = send(&app, as_agent("POST", path, Some(json!({ "token": "x" })))).await;
        assert!(
            matches!(
                status,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{path}: {status}"
        );
    }
}

#[tokio::test]
async fn the_change_feed_answers_when_feedback_arrives() {
    let (state, app) = setup("changes");
    let (_, view) = send(&app, as_agent("GET", "/v1/tasks/task-d/design", None)).await;
    let feed = view["feedRevision"].as_u64().unwrap();
    let doc = view["docRevision"].as_i64().unwrap();
    let waiting = {
        let app = app.clone();
        tokio::spawn(async move {
            send(
                &app,
                as_agent(
                    "GET",
                    &format!(
                        "/v1/tasks/task-d/design/changes?doc={doc}&feed={feed}&timeoutMs=10000"
                    ),
                    None,
                ),
            )
            .await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    send(
        &app,
        as_desktop(
            &state,
            "POST",
            "/v1/tasks/task-d/design/threads",
            Some(thread("t-w")),
        ),
    )
    .await;
    let (status, changed) = tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
        .await
        .expect("the long poll answers on the change")
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(changed["feedRevision"].as_u64().unwrap() > feed);
}

/// A design task whose repository is a real git repository with an origin,
/// which workflow replacement resolves the repository's definitions from.
fn setup_with_repository(label: &str) -> (Arc<AppState>, axum::Router) {
    let repo = crate::test_paths::unique_test_path(&format!("design-http-{label}-repo"));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("README.md"), "app\n").unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=T",
            "add",
            ".",
        ],
        vec![
            "-c",
            "user.email=t@example.com",
            "-c",
            "user.name=T",
            "commit",
            "-m",
            "init",
        ],
        vec!["update-ref", "refs/remotes/origin/main", "HEAD"],
    ] {
        assert!(std::process::Command::new("git")
            .args(&args)
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
    }
    let repo_path = repo.to_string_lossy().to_string();
    let state = test_state_with_seed(&format!("design-http-{label}"), "Design", move |db| {
        seed_design_task(db);
        db.execute_test_sql(&format!(
            "UPDATE repo SET path = '{repo_path}', default_branch = 'main' WHERE id = 'repo-1'"
        ))
        .unwrap();
    });
    let app = router(Arc::clone(&state));
    (state, app)
}

fn pinned_definition(state: &AppState) -> Value {
    let db = Db::open(&state.config().db_path).unwrap();
    serde_json::from_str(
        &db.get_pipeline_item("task-d")
            .unwrap()
            .unwrap()
            .pipeline_def
            .unwrap(),
    )
    .unwrap()
}

/// Review round 1, finding 1: the agent cannot drop the design stage's
/// `design` or `exit_commit` to walk past Approve for build.
#[tokio::test]
async fn a_live_design_stage_keeps_its_hand_off_through_any_workflow_change() {
    let (state, app) = setup_with_repository("workflow-guard");
    let (status, _) = send(&app, as_agent("GET", "/v1/tasks/task-d/design/agent", None)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the design session is live once opened"
    );
    let pinned = pinned_definition(&state);

    let mut without_design = pinned.clone();
    without_design["stages"][0]
        .as_object_mut()
        .unwrap()
        .remove("design");
    let mut without_commit = pinned.clone();
    without_commit["stages"][0]["exit_commit"] = json!(false);
    // Dropping `design` is this guard's to refuse; turning off exit_commit on
    // a design stage is already an invalid workflow.
    for (changed, expected, reason) in [
        (
            without_design,
            StatusCode::CONFLICT,
            "live App Design stage",
        ),
        (
            without_commit,
            StatusCode::BAD_REQUEST,
            "a design stage needs exit_commit",
        ),
    ] {
        let (status, body) = send(
            &app,
            as_agent(
                "POST",
                "/v1/tasks/task-d/actions/replace-workflow",
                Some(json!({ "workflowDefinition": changed, "expectedDefinition": pinned, "source": "agent" })),
            ),
        )
        .await;
        assert_eq!(status, expected, "{body}");
        assert!(body.to_string().contains(reason), "{body}");
    }
    assert_eq!(pinned_definition(&state), pinned, "nothing changed");

    // Switching to another workflow is refused as well.
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/actions/set-workflow",
            Some(json!({ "workflowName": "no-review" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(pinned_definition(&state), pinned, "nothing changed");

    // And the stage still leaves only through its hand-off.
    let error = crate::task_creator::prepare_advance_stage_for_api(
        &Db::open(&state.config().db_path).unwrap(),
        state.config(),
        "task-d",
    )
    .err()
    .expect("advance refused");
    assert!(error.contains("Approve for build"), "{error}");
}

/// Review round 2: before the agent has made any design call there is no
/// design session yet, and the stage is still live: its design cannot be
/// replaced away, the workflow cannot be switched, and it cannot be advanced.
#[tokio::test]
async fn a_design_stage_is_guarded_before_any_design_call() {
    let (state, app) = setup_with_repository("workflow-guard-no-session");
    let db = || Db::open(&state.config().db_path).unwrap();
    assert!(db().design_session("task-d").unwrap().is_none());
    let pinned = pinned_definition(&state);
    let mut without_design = pinned.clone();
    without_design["stages"][0]
        .as_object_mut()
        .unwrap()
        .remove("design");
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/actions/replace-workflow",
            Some(json!({ "workflowDefinition": without_design, "expectedDefinition": pinned, "source": "agent" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.to_string().contains("live App Design stage"), "{body}");
    let (status, body) = send(
        &app,
        as_agent(
            "POST",
            "/v1/tasks/task-d/actions/set-workflow",
            Some(json!({ "workflowName": "no-review" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(pinned_definition(&state), pinned, "nothing changed");
    let error = crate::task_creator::prepare_advance_stage_for_api(&db(), state.config(), "task-d")
        .err()
        .expect("advance refused");
    assert!(error.contains("Approve for build"), "{error}");
    assert!(
        db().design_session("task-d").unwrap().is_none(),
        "nothing here needed a session"
    );
}
