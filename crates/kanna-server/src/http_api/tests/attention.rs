use super::*;

async fn request(
    app: axum::Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn attention_set_persist_read_transition_clear_and_noop() {
    let state = test_state_with_seed("attention", "Attention", |db| {
        db.insert_test_repo("repo-attention", "Attention").unwrap();
        db.insert_test_pipeline_item(
            "attention-task",
            "repo-attention",
            "Prompt",
            Some("Title"),
            "in progress",
            "2026-09-13 00:00:00",
        )
        .unwrap();
    });
    let app = router(state.clone());
    let mut events = state.subscribe_state_changes();
    let db = Db::open(&state.config.db_path).unwrap();
    let original = serde_json::to_value(db.get_pipeline_item("attention-task").unwrap()).unwrap();
    let cursor = db.latest_task_event_seq().unwrap();
    let (status, value) = request(
        app.clone(),
        "PUT",
        "/v1/tasks/attention-task/attention",
        serde_json::json!({"reason":"  Choose 🦀  "}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        value,
        serde_json::json!({"taskId":"attention-task", "attentionReason":"Choose 🦀", "changed":true})
    );
    assert!(matches!(
        events.try_recv().unwrap(),
        kanna_agent_protocol::ServerFrame::StateChanged {
            scope: kanna_agent_protocol::StateChangeScope::Tasks,
            ..
        }
    ));
    let mut updated =
        serde_json::to_value(db.get_pipeline_item("attention-task").unwrap()).unwrap();
    updated["attention_reason"] = serde_json::Value::Null;
    assert_eq!(
        updated, original,
        "annotation must not reorder or mutate task state"
    );
    assert_eq!(db.latest_task_event_seq().unwrap(), cursor + 1);
    let (_, noop) = request(
        app.clone(),
        "POST",
        "/v1/tasks/attention-task/actions/set-attention",
        serde_json::json!({"reason":"Choose 🦀"}),
    )
    .await;
    assert_eq!(noop["changed"], false);
    assert!(events.try_recv().is_err());
    assert_eq!(db.latest_task_event_seq().unwrap(), cursor + 1);
    for path in [
        "/v1/tasks/attention-task",
        "/v1/tasks/attention-task?brief=true",
    ] {
        let (status, detail) = request(app.clone(), "GET", path, serde_json::Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(detail["attentionReason"], "Choose 🦀");
    }
    drop(db);
    let db = Db::open(&state.config.db_path).unwrap();
    assert_eq!(
        db.ui_snapshot().unwrap().entries[0].items[0]
            .attention_reason
            .as_deref(),
        Some("Choose 🦀")
    );
    db.update_pipeline_item_activity("attention-task", "unread")
        .unwrap();
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/v1/tasks/attention-task/actions/mark-read",
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    db.update_pipeline_item_stage("attention-task", "review")
        .unwrap();
    assert_eq!(
        db.get_pipeline_item("attention-task")
            .unwrap()
            .unwrap()
            .attention_reason
            .as_deref(),
        Some("Choose 🦀")
    );
    let (_, replacement) = request(
        app.clone(),
        "PUT",
        "/v1/tasks/attention-task/attention",
        serde_json::json!({"reason":"Review result"}),
    )
    .await;
    assert_eq!(replacement["attentionReason"], "Review result");
    let (_, clear) = request(
        app.clone(),
        "DELETE",
        "/v1/tasks/attention-task/attention",
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(clear["changed"], true);
    assert!(clear["attentionReason"].is_null());
    let (_, clear) = request(
        app,
        "POST",
        "/v1/tasks/attention-task/actions/clear-attention",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(clear["changed"], false);
    assert!(db.ui_snapshot().unwrap().entries[0].items[0]
        .attention_reason
        .is_none());
}

#[tokio::test]
async fn attention_rejects_invalid_input_and_missing_task() {
    let app = test_router_with_seed("attention-invalid", "Attention", |db| {
        db.insert_test_repo("repo-attention", "Attention").unwrap();
        db.insert_test_pipeline_item(
            "attention-task",
            "repo-attention",
            "Prompt",
            Some("Title"),
            "in progress",
            "2026-09-13 00:00:00",
        )
        .unwrap();
    });
    for reason in ["".to_string(), " \n ".to_string(), "🦀".repeat(241)] {
        assert_eq!(
            request(
                app.clone(),
                "PUT",
                "/v1/tasks/attention-task/attention",
                serde_json::json!({"reason":reason})
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert!(request(
        app.clone(),
        "PUT",
        "/v1/tasks/attention-task/attention",
        serde_json::json!({})
    )
    .await
    .0
    .is_client_error());
    assert_eq!(
        request(
            app.clone(),
            "PUT",
            "/v1/tasks/attention-task/attention",
            serde_json::json!({"reason":"🦀".repeat(240)})
        )
        .await
        .0,
        StatusCode::OK
    );
    for method in ["PUT", "DELETE"] {
        assert_eq!(
            request(
                app.clone(),
                method,
                "/v1/tasks/missing/attention",
                serde_json::json!({"reason":"Choose"})
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
    }
}
