//! The merge singleton's claim identity and handoff message, compared byte for
//! byte against a fixture recorded before the merge master became the merge
//! window of a release workflow (spec §10, T14). What the relay is asked to
//! claim, what the claimed task is called and pinned as, and the line a
//! handoff writes into the merge master must not move.
use super::actions::{ledger_fixture_config, spawn_recording_daemon};
use super::*;

const FIXTURE: &str = include_str!("fixtures/merge_singleton_identity.json");
const REMOTE_URL_HASH: &str = "identity-remote-hash";
const PR_URL: &str = "https://github.com/acme/repo/pull/123";

fn handoff_body() -> serde_json::Value {
    serde_json::json!({
        "branch": "feature/login",
        "target": "main",
        "prUrl": PR_URL,
        "summary": "Add the login form",
    })
}

fn seed_handoff_source(db: &Db, task_id: &str) {
    db.insert_test_pipeline_item(
        task_id,
        "repo-1",
        "Add the login form",
        Some("Add the login form"),
        "pr",
        "2026-09-23T00:00:00Z",
    )
    .unwrap();
    db.update_test_pipeline_item_stage_context(task_id, task_id, "default", Some("main"), "claude")
        .unwrap();
}

/// A handoff that finds no merge master claims one through the relay and
/// starts it with the handoff as its first message.
async fn created_singleton_identity() -> serde_json::Value {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let repo_root = crate::test_paths::unique_test_path("kanna-merge-identity-repo");
    init_test_git_repo(&repo_root);
    publish_test_origin_main(&repo_root);
    let daemon_dir = crate::test_paths::unique_test_path("kanna-merge-identity-d");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let _commands = spawn_recording_daemon(&daemon_dir);
    let config = ledger_fixture_config("merge-identity-created", &daemon_dir);
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    db.patch_repo(
        "repo-1",
        crate::db::RepoPatch {
            remote_url_hash: Some(Some(REMOTE_URL_HASH)),
            ..Default::default()
        },
    )
    .unwrap();
    seed_handoff_source(&db, "handoff-source");
    drop(db);

    let state = Arc::new(super::AppState::new(config.clone()));
    let claims = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let mut requests = state.take_desktop_relay_requests().unwrap();
    state.set_desktop_routing_available(true);
    let recorded_claims = Arc::clone(&claims);
    let desktop_id = config.desktop_id.clone();
    let relay = tokio::spawn(async move {
        while let Some(request) = requests.recv().await {
            match request {
                crate::http_api::DesktopRelayRequest::PublishTaskSnapshot { response, .. } => {
                    let _ = response.send(Ok(()));
                }
                crate::http_api::DesktopRelayRequest::ListRepoSingletons { response, .. } => {
                    let _ = response.send(Ok(Vec::new()));
                }
                crate::http_api::DesktopRelayRequest::ListActive { response, .. } => {
                    let _ = response.send(Ok(Vec::new()));
                }
                crate::http_api::DesktopRelayRequest::ClaimRepoSingleton {
                    remote_url_hash,
                    agent,
                    task_id,
                    response,
                    ..
                } => {
                    recorded_claims.lock().unwrap().push(serde_json::json!({
                        "remoteUrlHash": remote_url_hash,
                        "agent": agent,
                        "taskId": task_id,
                    }));
                    let _ = response.send(Ok(crate::http_api::RemoteSingletonClaim {
                        status: "acquired".into(),
                        machine_id: desktop_id.clone(),
                        task_id,
                        owners: Vec::new(),
                    }));
                }
                _ => {}
            }
        }
    });

    let app = super::router(Arc::clone(&state));
    let (status, text) = super::actions::post_json(
        &app,
        "/v1/tasks/handoff-source/actions/signal-merge-handoff",
        handoff_body(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let body: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["created"], true, "{text}");
    let task_id = body["taskId"].as_str().unwrap().to_string();

    let db = Db::open(&config.db_path).unwrap();
    let mut runs = Vec::new();
    for _ in 0..100 {
        runs = db.list_stage_runs_for_task(&task_id).unwrap();
        if !runs.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let task = db.get_pipeline_item(&task_id).unwrap().unwrap();
    relay.abort();
    let mut claims = claims.lock().unwrap().clone();
    for claim in &mut claims {
        // The claimed id is generated; what is fixed is that the relay was
        // asked to claim exactly the task that was then created.
        assert_eq!(claim["taskId"], task_id.as_str());
        claim["taskId"] = "$CREATED_TASK".into();
    }
    let _ = std::fs::remove_dir_all(&daemon_dir);
    let _ = std::fs::remove_dir_all(&repo_root);
    serde_json::json!({
        "claims": claims,
        "task": {
            "pipeline": task.pipeline,
            "displayName": task.display_name,
            "stage": task.stage,
            "prompt": task.prompt,
            "pinned": task.pinned,
        },
        "runs": runs.iter().map(|run| serde_json::json!({
            "stage": run.stage,
            "kind": run.kind,
            "agent": run.agent,
        })).collect::<Vec<_>>(),
    })
}

/// A handoff to a resident merge master writes one line into its session.
async fn delivered_handoff() -> serde_json::Value {
    let delivered = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let sink = Arc::clone(&delivered);
    let state = super::test_state_with_seed_and_task_input_sender(
        "desktop-merge-identity",
        "Studio Mac",
        |db| {
            db.insert_test_repo("repo-1", "Repo One").unwrap();
            db.insert_test_pipeline_item(
                "resident-master",
                "repo-1",
                "",
                Some("Merge Master"),
                "in progress",
                "2026-09-22T00:00:00Z",
            )
            .unwrap();
            db.update_test_pipeline_item_stage_context(
                "resident-master",
                "resident-master",
                "singleton-merge",
                None,
                "claude",
            )
            .unwrap();
            db.insert_stage_run(crate::db::NewStageRun {
                id: "resident-run",
                task_id: "resident-master",
                stage: "in progress",
                kind: "main",
                agent: Some("merge"),
                agent_provider: Some("claude"),
                model: None,
                effort: None,
                status: "succeeded",
                result: None,
                feedback: None,
                session_id: Some("resident-master"),
                provider_session_id: None,
                cwd: None,
                resumed_from_run_id: None,
            })
            .unwrap();
            seed_handoff_source(db, "handoff-source");
        },
        Arc::new(move |task_id, input| {
            sink.lock().unwrap().push((task_id, input));
            Ok(())
        }),
    );
    let app = super::router(Arc::clone(&state));
    let (status, text) = super::actions::post_json(
        &app,
        "/v1/tasks/handoff-source/actions/signal-merge-handoff",
        handoff_body(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let delivered = delivered.lock().unwrap().clone();
    serde_json::json!({
        "response": serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        "inputs": delivered
            .into_iter()
            .map(|(task_id, input)| serde_json::json!({ "taskId": task_id, "input": input }))
            .collect::<Vec<_>>(),
    })
}

#[tokio::test]
async fn the_merge_singleton_claim_identity_and_handoff_message_match_the_fixture() {
    let observed = serde_json::json!({
        "created": created_singleton_identity().await,
        "delivered": delivered_handoff().await,
    });
    let expected: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(
        observed,
        expected,
        "observed:\n{}",
        serde_json::to_string_pretty(&observed).unwrap()
    );
}
