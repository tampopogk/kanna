use super::*;

fn db_with_task() -> (Db, String) {
    let path = Db::test_db_path("design");
    let db = Db::open_for_tests(&path).unwrap();
    db.insert_test_repo("repo-1", "Repo One").unwrap();
    db.insert_test_pipeline_item("task-1", "repo-1", "prompt", None, "design", "2026-09-26 00:00:00")
        .unwrap();
    (db, "task-1".to_string())
}

fn session(db: &Db, task: &str) {
    db.ensure_design_session(task, "design", "static", "schema-1")
        .unwrap();
}

fn thread<'a>(id: &'a str, comment: &'a str, delivery: Option<&'a str>) -> NewDesignThread<'a> {
    NewDesignThread {
        thread_id: id,
        comment_id: comment,
        kind: "comment",
        anchor_block_id: Some("block-1"),
        quoted_text: Some("quoted"),
        anchor_state_vector: Some(&[1, 2, 3]),
        body: "please change this",
        author: "operator",
        client_op_id: Some(comment),
        channel_identity: None,
        delivery_id: delivery,
    }
}

#[test]
fn a_session_is_created_once() {
    let (db, task) = db_with_task();
    let (first, created) = db
        .ensure_design_session(&task, "design", "static", "schema-1")
        .unwrap();
    assert!(created);
    let (second, created) = db
        .ensure_design_session(&task, "design", "prototype", "schema-2")
        .unwrap();
    assert!(!created);
    assert_eq!(first, second);
    assert!(db.set_design_position(&task, "prototype").unwrap());
    assert!(!db.set_design_position(&task, "prototype").unwrap());
    assert_eq!(db.design_session(&task).unwrap().unwrap().position, "prototype");
}

#[test]
fn feedback_and_its_delivery_are_one_write_and_retries_are_the_same_thread() {
    let (db, task) = db_with_task();
    session(&db, &task);
    let created = db
        .create_design_thread(&task, thread("t1", "c1", Some("d1")))
        .unwrap();
    assert!(created.created);
    assert_eq!(created.thread.number, 1);
    let retried = db
        .create_design_thread(&task, thread("t1", "c1", Some("d1")))
        .unwrap();
    assert!(!retried.created);
    assert_eq!(retried.comment.id, "c1");
    let deliveries = db.design_deliveries(&task).unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].comment_id.as_deref(), Some("c1"));
    assert_eq!(deliveries[0].state, DesignDeliveryRow::QUEUED);

    // Numbers follow creation order; replies and resolution never renumber.
    db.create_design_thread(&task, thread("t2", "c2", Some("d2")))
        .unwrap();
    db.add_design_comment(&task, "t1", "c3", "agent", "done", Some("op-1"), None, None)
        .unwrap();
    assert!(db.set_design_thread_resolved(&task, "t1", true, "agent").unwrap());
    assert!(!db.set_design_thread_resolved(&task, "t1", true, "agent").unwrap());
    let numbers: Vec<(String, i64, String)> = db
        .design_threads(&task)
        .unwrap()
        .into_iter()
        .map(|thread| (thread.id, thread.number, thread.status))
        .collect();
    assert_eq!(
        numbers,
        vec![
            ("t1".into(), 1, "resolved".into()),
            ("t2".into(), 2, "open".into())
        ]
    );
    // The agent's reply queued nothing back to the agent.
    assert_eq!(db.design_deliveries(&task).unwrap().len(), 2);
    // A retried reply is the same comment.
    let again = db
        .add_design_comment(&task, "t1", "c4", "agent", "done", Some("op-1"), None, None)
        .unwrap();
    assert!(!again.created);
    assert_eq!(again.comment.id, "c3");
}

#[test]
fn an_attempt_is_reserved_released_or_settled_once() {
    let (db, task) = db_with_task();
    session(&db, &task);
    db.create_design_thread(&task, thread("t1", "c1", Some("d1")))
        .unwrap();
    db.create_design_thread(&task, thread("t2", "c2", Some("d2")))
        .unwrap();
    let ids = vec!["d1".to_string(), "d2".to_string()];
    db.reserve_design_deliveries(&ids, "attempt-1", "daemon-a")
        .unwrap();
    // A second reservation of the same rows is refused.
    assert!(db
        .reserve_design_deliveries(&ids, "attempt-2", "daemon-a")
        .is_err());
    assert_eq!(db.release_design_attempt("attempt-1", "busy").unwrap(), 2);
    db.reserve_design_deliveries(&ids, "attempt-3", "daemon-a")
        .unwrap();
    assert_eq!(db.delivering_design_deliveries().unwrap().len(), 2);
    let settled = db.mark_design_attempt_delivered("attempt-3").unwrap();
    assert_eq!(settled.len(), 2);
    assert!(db.mark_design_attempt_delivered("attempt-3").unwrap().is_empty());
    assert!(db.tasks_with_open_design_deliveries().unwrap().is_empty());
}

#[test]
fn an_uncertain_delivery_waits_for_a_person_to_retry_it() {
    let (db, task) = db_with_task();
    session(&db, &task);
    db.create_design_thread(&task, thread("t1", "c1", Some("d1")))
        .unwrap();
    db.reserve_design_deliveries(&["d1".into()], "attempt-1", "daemon-a")
        .unwrap();
    db.mark_design_attempt_uncertain("attempt-1", "response lost")
        .unwrap();
    assert!(db.tasks_with_open_design_deliveries().unwrap().is_empty());
    assert!(db.retry_design_delivery(&task, "d1").unwrap());
    assert!(!db.retry_design_delivery(&task, "d1").unwrap());
    assert_eq!(db.design_delivery("d1").unwrap().unwrap().state, "queued");
}

#[test]
fn a_new_epoch_invalidates_unfinished_approvals_and_keeps_queued_feedback() {
    let (db, task) = db_with_task();
    session(&db, &task);
    db.create_design_thread(&task, thread("t1", "c1", Some("d1")))
        .unwrap();
    db.insert_design_candidate(
        "a1", &task, 1, 3, "sha", "commit", None, None, "{}", "hash", "2099-01-01T00:00:00Z",
    )
    .unwrap();
    assert!(db
        .advance_design_approval(
            "a1",
            &[DesignApprovalRow::CANDIDATE],
            DesignApprovalRow::APPROVED,
            &DesignApprovalUpdate {
                consume_confirmation: true,
                ..Default::default()
            },
        )
        .unwrap());
    // Advancing from a phase it has left is refused.
    assert!(!db
        .advance_design_approval(
            "a1",
            &[DesignApprovalRow::CANDIDATE],
            DesignApprovalRow::APPROVED,
            &DesignApprovalUpdate::default(),
        )
        .unwrap());
    assert!(db.design_approval("a1").unwrap().unwrap().confirmation_hash.is_none());
    let epoch = db.begin_design_epoch(&task, "design").unwrap();
    assert_eq!(epoch, 2);
    assert_eq!(db.design_approval("a1").unwrap().unwrap().phase, "invalidated");
    assert!(db.current_design_approval(&task).unwrap().is_none());
    assert_eq!(db.design_delivery("d1").unwrap().unwrap().epoch, 2);
}

#[test]
fn document_updates_advance_the_revision_and_compact() {
    let (db, task) = db_with_task();
    session(&db, &task);
    assert_eq!(db.append_design_doc_update(&task, &[1], "client").unwrap(), 1);
    assert_eq!(db.append_design_doc_update(&task, &[2], "agent").unwrap(), 2);
    assert_eq!(db.design_doc_updates(&task).unwrap().len(), 2);
    db.compact_design_doc(&task, &[9, 9]).unwrap();
    assert_eq!(db.design_doc_updates(&task).unwrap(), vec![(2, vec![9, 9])]);
    assert_eq!(db.design_session(&task).unwrap().unwrap().doc_revision, 2);
    // Restoring over existing updates is a no-op.
    db.restore_design_doc(&task, &[7], 10).unwrap();
    assert_eq!(db.design_doc_updates(&task).unwrap(), vec![(2, vec![9, 9])]);
}

#[test]
fn a_task_with_a_live_design_is_not_transferred_until_handed_off() {
    let (db, task) = db_with_task();
    session(&db, &task);
    let transfer = |id: &str| crate::db::NewTaskTransfer {
        id: id.to_string(),
        direction: "outgoing".into(),
        status: "pending".into(),
        source_peer_id: Some("peer-a".into()),
        target_peer_id: Some("peer-b".into()),
        source_desktop_id: None,
        target_desktop_id: None,
        source_task_id: Some(task.clone()),
        local_task_id: Some(task.clone()),
        error: None,
        payload_json: None,
    };
    let refused = db.insert_task_transfer(&transfer("tr-1")).unwrap_err();
    assert!(crate::db::is_live_design_transfer_refusal(&refused), "{refused}");
    db.set_design_session_status(&task, DesignSessionRow::HANDED_OFF)
        .unwrap();
    db.insert_task_transfer(&transfer("tr-2")).unwrap();
}
