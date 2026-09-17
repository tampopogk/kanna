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

fn work_set_ids(value: &serde_json::Value) -> Vec<String> {
    value["tasks"]
        .as_array()
        .expect("tasks array")
        .iter()
        .map(|task| task["id"].as_str().expect("task id").to_string())
        .collect()
}

fn seed_idle_tasks(db: &Db) {
    db.insert_test_repo("repo-serviced", "Serviced").unwrap();
    for (id, created_at) in [
        ("serviced-a", "2026-09-17 09:00:00"),
        ("serviced-b", "2026-09-17 09:01:00"),
    ] {
        db.insert_test_pipeline_item(
            id,
            "repo-serviced",
            "Prompt",
            Some(id),
            "in progress",
            created_at,
        )
        .unwrap();
        db.update_pipeline_item_runtime_status(id, "idle", None)
            .unwrap();
    }
}

#[tokio::test]
async fn recording_servicing_filters_the_work_set_without_touching_task_state() {
    let state = test_state_with_seed("serviced", "Serviced", seed_idle_tasks);
    let app = router(state.clone());
    let mut events = state.subscribe_state_changes();
    let db = Db::open(&state.config.db_path).unwrap();
    let before = serde_json::to_value(db.get_pipeline_item("serviced-a").unwrap()).unwrap();
    let cursor = db.latest_task_event_seq().unwrap();

    let query = "/v1/tasks?repoId=repo-serviced&runtimeState=idle&sortBy=createdAt&order=asc";
    let unserviced = format!("{query}&unservicedOnly=true");
    let (status, all) = request(app.clone(), "GET", query, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all["unservicedOnly"], false);
    assert_eq!(work_set_ids(&all), vec!["serviced-a", "serviced-b"]);
    let (_, work_set) = request(app.clone(), "GET", &unserviced, serde_json::Value::Null).await;
    assert_eq!(
        work_set["unservicedOnly"], true,
        "the response echoes the filter so an older peer's unfiltered page is detectable"
    );
    assert_eq!(work_set_ids(&work_set), vec!["serviced-a", "serviced-b"]);

    let (status, watermark) = request(
        app.clone(),
        "POST",
        "/v1/tasks/serviced-a/actions/record-serviced",
        serde_json::json!({"runId": "manager-run-1", "observedEventSeq": cursor}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(watermark["taskId"], "serviced-a");
    assert_eq!(watermark["servicedRunId"], "manager-run-1");
    assert_eq!(watermark["servicedEventSeq"], cursor);
    assert!(watermark["servicedAt"].is_string());

    // Servicing is a manager's private bookkeeping: no task event, no state
    // change broadcast, and nothing on the task row moves - otherwise the write
    // would put the task straight back into the work set it just left.
    assert_eq!(db.latest_task_event_seq().unwrap(), cursor);
    assert!(events.try_recv().is_err());
    assert_eq!(
        serde_json::to_value(db.get_pipeline_item("serviced-a").unwrap()).unwrap(),
        before
    );

    let (_, work_set) = request(app.clone(), "GET", &unserviced, serde_json::Value::Null).await;
    assert_eq!(work_set_ids(&work_set), vec!["serviced-b"]);
    let (_, all) = request(app.clone(), "GET", query, serde_json::Value::Null).await;
    assert_eq!(
        work_set_ids(&all),
        vec!["serviced-a", "serviced-b"],
        "the unfiltered listing is unchanged by servicing"
    );

    // Omitting the cursor records the log head; the body itself is optional.
    let (status, watermark) = request(
        app.clone(),
        "POST",
        "/v1/tasks/serviced-b/actions/record-serviced",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(watermark["servicedEventSeq"], cursor);
    assert!(watermark["servicedRunId"].is_null());
    let (_, work_set) = request(app.clone(), "GET", &unserviced, serde_json::Value::Null).await;
    assert!(work_set_ids(&work_set).is_empty());

    // The task moves again, so it is the manager's work again.
    db.update_pipeline_item_runtime_status("serviced-b", "busy", None)
        .unwrap();
    db.update_pipeline_item_runtime_status("serviced-b", "idle", None)
        .unwrap();
    let (_, work_set) = request(app, "GET", &unserviced, serde_json::Value::Null).await;
    assert_eq!(work_set_ids(&work_set), vec!["serviced-b"]);
}

#[tokio::test]
async fn record_serviced_rejects_an_unreadable_mark_and_a_missing_task() {
    let state = test_state_with_seed("serviced-invalid", "Serviced", seed_idle_tasks);
    let app = router(state.clone());
    let head = Db::open(&state.config.db_path)
        .unwrap()
        .latest_task_event_seq()
        .unwrap();

    // A mark past the head would suppress events nobody has read yet.
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/v1/tasks/serviced-a/actions/record-serviced",
            serde_json::json!({"observedEventSeq": head + 1})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            app.clone(),
            "POST",
            "/v1/tasks/serviced-a/actions/record-serviced",
            serde_json::json!({"observedEventSeq": -1})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert!(request(
        app.clone(),
        "POST",
        "/v1/tasks/serviced-a/actions/record-serviced",
        serde_json::json!({"unknown": true})
    )
    .await
    .0
    .is_client_error());
    assert_eq!(
        request(
            app,
            "POST",
            "/v1/tasks/missing/actions/record-serviced",
            serde_json::json!({})
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
