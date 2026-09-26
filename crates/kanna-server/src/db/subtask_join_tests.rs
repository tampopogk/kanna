use super::*;
use crate::db::Db;

fn test_db(label: &str) -> (Db, String) {
    let path = Db::test_db_path(label);
    let db = Db::open_for_tests(&path).unwrap();
    db.insert_test_repo("repo-1", "Repo One").unwrap();
    (db, path)
}

fn task(db: &Db, id: &str, parent: Option<&str>) {
    db.insert_test_pipeline_item(id, "repo-1", id, Some(id), "work", "2026-09-23 00:00:00")
        .unwrap();
    db.pin_test_stages(id, &["work"]);
    db.update_pipeline_item_parent(id, parent).unwrap();
}

fn join(db: &Db, id: &str, parent: &str, children: &[&str]) -> TaskJoin {
    db.create_task_join(&NewTaskJoin {
        id: id.to_string(),
        parent_task_id: parent.to_string(),
        parent_stage: Some("work".to_string()),
        parent_run_id: None,
        base_sha: "1111111111111111111111111111111111111111".to_string(),
        base_branch: Some(format!("task-{parent}")),
        members: children
            .iter()
            .map(|child| NewJoinMember {
                child_task_id: child.to_string(),
                spec: serde_json::json!({ "prompt": child }).to_string(),
            })
            .collect(),
    })
    .unwrap()
}

/// Record a result of `task_id` under an explicit source identity, the way
/// a completion does; the same identity again is a replay.
fn result(db: &Db, task_id: &str, source_id: &str, status: &str, message: &str) -> String {
    db.enqueue_ledger_entry(crate::db::task_store::NewLedgerEntry {
        task_id,
        kind: crate::db::task_store::LedgerEntryKind::Result,
        operation_id: None,
        source_kind: "stage_run",
        source_id,
        source_origin: None,
        historical: false,
        recorded_at: None,
        run_id: None,
        declared_role: Some("agent"),
        channel_identity: &crate::mutation_provenance::ChannelIdentity::Unknown,
        body: json!({
            "status": status,
            "stage": "work",
            "run_kind": "main",
            "branch": Value::Null,
            "committed_sha": "2222222222222222222222222222222222222222",
        }),
        message: Some(message),
        hold_events_after: None,
        reserved_sequence: None,
    })
    .unwrap()
    .entry_id
}

fn join_inputs(db: &Db, task_id: &str) -> Vec<crate::db::TaskInputRecord> {
    db.list_task_inputs(task_id, 100)
        .unwrap()
        .into_iter()
        .filter(|input| input.source == SUBTASK_JOIN_INPUT_SOURCE)
        .collect()
}

fn input_ledger_messages(db: &Db, task_id: &str) -> Vec<String> {
    let mut stmt = db
        .conn
        .prepare(
            "SELECT file_name, payload FROM task_ledger_entry
             WHERE task_id = ? AND kind = 'input' ORDER BY sequence",
        )
        .unwrap();
    stmt.query_map([task_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })
    .unwrap()
    .map(|row| {
        let (file_name, payload) = row.unwrap();
        crate::task_store::parse_ledger_file(&file_name, &payload)
            .unwrap()
            .message
            .unwrap_or_default()
    })
    .collect()
}

fn events(db: &Db, task_id: &str, kind: &str) -> Vec<Value> {
    let mut stmt = db
        .conn
        .prepare("SELECT payload FROM task_event WHERE task_id = ? AND type = ? ORDER BY seq")
        .unwrap();
    stmt.query_map(params![task_id, kind], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|payload| serde_json::from_str(&payload.unwrap()).unwrap())
        .collect()
}

#[test]
fn parent_is_blocked_until_every_member_resolves_and_each_result_is_delivered_once() {
    let (db, _) = test_db("subtask-join-deliver-once");
    task(&db, "parent", None);
    task(&db, "child-a", Some("parent"));
    task(&db, "child-b", Some("parent"));
    join(&db, "join-1", "parent", &["child-a", "child-b"]);

    assert_eq!(
        db.list_blocking_task_ids("parent").unwrap(),
        vec!["child-a", "child-b"]
    );
    assert_eq!(events(&db, "parent", "task.blocked").len(), 1);

    // A success: delivered with its message, the parent still waits on b.
    let a_result = result(&db, "child-a", "run-a", "success", "A found nothing");
    let inputs = join_inputs(&db, "parent");
    assert_eq!(inputs.len(), 1);
    assert!(
        inputs[0].message.contains("child-a"),
        "{}",
        inputs[0].message
    );
    assert!(inputs[0].message.contains("recorded success"));
    assert!(inputs[0].message.contains(&a_result));
    assert!(inputs[0].message.contains("A found nothing"));
    assert!(inputs[0].message.contains("Still waiting on: child-b."));
    assert_eq!(
        db.unresolved_join_children("parent").unwrap(),
        vec!["child-b"]
    );
    assert_eq!(
        db.list_blocking_task_ids("parent").unwrap(),
        vec!["child-b"]
    );

    // A replay of that result and a later result of the same child deliver
    // nothing more.
    result(&db, "child-a", "run-a", "success", "A found nothing");
    result(&db, "child-a", "run-a#2", "failure", "A changed its mind");
    assert_eq!(join_inputs(&db, "parent").len(), 1);

    // A non-success result resolves its member just the same.
    result(&db, "child-b", "run-b", "declined", "B was out of scope");
    let inputs = join_inputs(&db, "parent");
    assert_eq!(inputs.len(), 2);
    assert!(inputs[1].message.contains("recorded declined"));
    assert!(inputs[1].message.contains("B was out of scope"));
    assert!(inputs[1]
        .message
        .contains("Every child in this join has resolved"));
    assert!(db.unresolved_join_children("parent").unwrap().is_empty());
    assert!(db.list_blocking_task_ids("parent").unwrap().is_empty());
    assert!(db
        .task_join("join-1")
        .unwrap()
        .unwrap()
        .completed_at
        .is_some());
    assert_eq!(events(&db, "parent", "task.unblocked").len(), 1);

    // Each input is also an `input` ledger entry of the parent, once.
    let ledger = input_ledger_messages(&db, "parent");
    assert_eq!(ledger.len(), 2);
    assert_eq!(ledger[0], inputs[0].message);
    assert_eq!(ledger[1], inputs[1].message);

    let delivered = events(&db, "parent", "task.subtask_result");
    assert_eq!(delivered.len(), 2);
    assert_eq!(delivered[0]["childTaskId"], "child-a");
    assert_eq!(delivered[0]["resultId"], a_result.as_str());
    assert_eq!(delivered[0]["joinComplete"], false);
    assert_eq!(delivered[1]["status"], "declined");
    assert_eq!(delivered[1]["joinComplete"], true);

    let members = db.list_task_join_members("join-1").unwrap();
    assert_eq!(members[0].outcome.as_deref(), Some("result"));
    assert_eq!(members[0].result_id.as_deref(), Some(a_result.as_str()));
    assert_eq!(members[0].input_id, Some(inputs[0].id));
    assert_eq!(members[1].result_status.as_deref(), Some("declined"));
}

#[test]
fn an_unrelated_or_earlier_child_never_satisfies_a_new_join() {
    let (db, _) = test_db("subtask-join-membership");
    task(&db, "parent", None);
    // An ordinary child, and a child of an earlier, completed join.
    task(&db, "plain-child", Some("parent"));
    task(&db, "earlier-child", Some("parent"));
    join(&db, "join-earlier", "parent", &["earlier-child"]);
    result(
        &db,
        "earlier-child",
        "run-earlier",
        "success",
        "earlier work",
    );
    assert!(db.unresolved_join_children("parent").unwrap().is_empty());
    // An unrelated task that is nobody's child.
    task(&db, "stranger", None);

    task(&db, "new-child", Some("parent"));
    join(&db, "join-new", "parent", &["new-child"]);
    result(&db, "plain-child", "run-plain", "success", "plain work");
    result(
        &db,
        "earlier-child",
        "run-earlier#2",
        "success",
        "more earlier work",
    );
    result(&db, "stranger", "run-stranger", "success", "not yours");
    db.close_pipeline_item("plain-child").unwrap();

    assert_eq!(
        db.unresolved_join_children("parent").unwrap(),
        vec!["new-child"]
    );
    assert!(db
        .task_join("join-new")
        .unwrap()
        .unwrap()
        .completed_at
        .is_none());
    // Only the earlier join's one member was ever delivered.
    assert_eq!(join_inputs(&db, "parent").len(), 1);

    result(&db, "new-child", "run-new", "success", "the new work");
    assert_eq!(join_inputs(&db, "parent").len(), 2);
    assert!(db
        .task_join("join-new")
        .unwrap()
        .unwrap()
        .completed_at
        .is_some());
}

#[test]
fn a_restart_after_partial_delivery_delivers_no_duplicates() {
    let (db, path) = test_db("subtask-join-restart");
    task(&db, "parent", None);
    task(&db, "child-a", Some("parent"));
    task(&db, "child-b", Some("parent"));
    join(&db, "join-1", "parent", &["child-a", "child-b"]);
    result(&db, "child-a", "run-a", "success", "A done");
    // The notice for a was claimed (typed) before the "crash".
    assert!(db.claim_join_notice("child-a").unwrap());
    drop(db);

    // Restart: a new connection replays everything that could re-deliver.
    let db = Db::open(&path).unwrap();
    result(&db, "child-a", "run-a", "success", "A done");
    db.close_pipeline_item("child-a").unwrap();
    assert!(!db
        .resolve_join_member_not_created("child-a", "boom")
        .unwrap());
    assert!(db.list_uncreated_join_members().unwrap().is_empty());
    assert!(db.pending_join_notices().unwrap().is_empty());
    assert!(!db.claim_join_notice("child-a").unwrap());
    assert_eq!(join_inputs(&db, "parent").len(), 1);
    assert_eq!(input_ledger_messages(&db, "parent").len(), 1);
    assert_eq!(
        db.unresolved_join_children("parent").unwrap(),
        vec!["child-b"]
    );

    // The rest of the join still delivers, once, after the restart.
    result(&db, "child-b", "run-b", "failure", "B broke");
    let pending = db.pending_join_notices().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].child_task_id, "child-b");
    assert!(pending[0].message.contains("B broke"));
    assert!(db.claim_join_notice("child-b").unwrap());
    assert!(!db.claim_join_notice("child-b").unwrap());
    // A notice given back after a typing attempt that wrote nothing is owed
    // again, still only once.
    db.release_join_notice("child-b").unwrap();
    assert_eq!(db.pending_join_notices().unwrap().len(), 1);
    assert_eq!(join_inputs(&db, "parent").len(), 2);
}

#[test]
fn racing_resolutions_of_one_member_deliver_one_input() {
    let (db, path) = test_db("subtask-join-race");
    task(&db, "parent", None);
    task(&db, "child-a", Some("parent"));
    join(&db, "join-1", "parent", &["child-a"]);
    drop(db);

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let racers = [0, 1].map(|racer| {
        let path = path.clone();
        let barrier = std::sync::Arc::clone(&barrier);
        std::thread::spawn(move || {
            let db = Db::open(&path).unwrap();
            barrier.wait();
            if racer == 0 {
                result(&db, "child-a", "run-a", "success", "A done");
            } else {
                db.close_pipeline_item("child-a").unwrap();
            }
        })
    });
    for racer in racers {
        racer.join().unwrap();
    }
    let db = Db::open(&path).unwrap();
    let inputs = join_inputs(&db, "parent");
    assert_eq!(inputs.len(), 1, "{inputs:?}");
    assert_eq!(events(&db, "parent", "task.subtask_result").len(), 1);
    assert!(db.unresolved_join_children("parent").unwrap().is_empty());
}

#[test]
fn a_crashed_child_stays_unresolved_until_it_is_closed() {
    let (db, _) = test_db("subtask-join-crash");
    task(&db, "parent", None);
    task(&db, "child-a", Some("parent"));
    join(&db, "join-1", "parent", &["child-a"]);
    // Its session died without recording anything: nothing resolves it.
    db.conn
        .execute(
            "UPDATE pipeline_item SET runtime_status = 'exited' WHERE id = 'child-a'",
            [],
        )
        .unwrap();
    assert_eq!(
        db.unresolved_join_children("parent").unwrap(),
        vec!["child-a"]
    );
    assert!(join_inputs(&db, "parent").is_empty());

    // Closing it is an explicit resolution, delivered as such.
    db.close_pipeline_item("child-a").unwrap();
    let inputs = join_inputs(&db, "parent");
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0]
        .message
        .contains("was closed without recording a result"));
    let member = db.task_join_member("child-a").unwrap().unwrap();
    assert_eq!(member.outcome.as_deref(), Some("closed"));
    assert!(member.result_id.is_none());
    assert!(db.unresolved_join_children("parent").unwrap().is_empty());
}

#[test]
fn a_member_that_was_never_created_resolves_as_not_created() {
    let (db, _) = test_db("subtask-join-not-created");
    task(&db, "parent", None);
    task(&db, "child-exists", Some("parent"));
    join(&db, "join-1", "parent", &["child-missing", "child-exists"]);
    assert_eq!(
        db.list_uncreated_join_members()
            .unwrap()
            .into_iter()
            .map(|member| member.child_task_id)
            .collect::<Vec<_>>(),
        vec!["child-missing"]
    );

    assert!(db
        .resolve_join_member_not_created("child-missing", "unknown workflow")
        .unwrap());
    let inputs = join_inputs(&db, "parent");
    assert_eq!(inputs.len(), 1);
    assert!(inputs[0]
        .message
        .contains("could not be created: unknown workflow"));
    assert!(db.list_uncreated_join_members().unwrap().is_empty());

    // A task whose spawn failed after its row was written is a child, not a
    // missing member: it stays unresolved with the error recorded.
    assert!(!db
        .resolve_join_member_not_created("child-exists", "daemon unreachable")
        .unwrap());
    let member = db.task_join_member("child-exists").unwrap().unwrap();
    assert!(member.resolved_at.is_none());
    assert_eq!(member.create_error.as_deref(), Some("daemon unreachable"));
    assert_eq!(
        db.unresolved_join_children("parent").unwrap(),
        vec!["child-exists"]
    );
}

#[test]
fn task_json_links_list_the_parents_joins() {
    let (db, _) = test_db("subtask-join-links");
    task(&db, "parent", None);
    task(&db, "child-a", Some("parent"));
    join(&db, "join-1", "parent", &["child-a"]);
    let entry = result(&db, "child-a", "run-a", "success", "done");
    let links = db.subtask_join_links("parent").unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["join_id"], "join-1");
    assert_eq!(links[0]["members"][0]["child_task_id"], "child-a");
    assert_eq!(links[0]["members"][0]["result_id"], entry.as_str());
    assert!(links[0]["completed_at"].is_string());
}
