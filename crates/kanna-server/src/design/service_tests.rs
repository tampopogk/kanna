use super::*;
use crate::design::document::{BlockOp, DesignDocument, NewBlock};
use crate::design::DesignRuntime;
use base64::Engine;

const APP_DESIGN: &str = include_str!("../../../../.kanna/workflows/app-design.json");

pub(crate) struct Fixture {
    pub(crate) db: Db,
    pub(crate) db_path: String,
    pub(crate) runtime: DesignRuntime,
    pub(crate) task: String,
}

/// Seed `task-d`, pinned to the bundled `app-design` workflow and in its
/// design stage.
pub(crate) fn seed_design_task(db: &Db) {
    db.insert_test_repo("repo-1", "Repo One").unwrap();
    db.insert_test_pipeline_item(
        "task-d",
        "repo-1",
        "Design the artifact viewer",
        Some("Artifact viewer"),
        "design",
        "2026-09-26T00:00:00Z",
    )
    .unwrap();
    db.execute_test_sql(&format!(
        "UPDATE pipeline_item SET pipeline = 'app-design', pipeline_def = '{}' WHERE id = 'task-d'",
        APP_DESIGN.replace('\'', "''")
    ))
    .unwrap();
}

/// A task pinned to the bundled `app-design` workflow, in its design stage.
pub(crate) fn fixture(label: &str) -> Fixture {
    let db_path = Db::test_db_path(&format!("design-{label}"));
    let db = Db::open_for_tests(&db_path).unwrap();
    seed_design_task(&db);
    Fixture {
        db,
        db_path,
        runtime: DesignRuntime::default(),
        task: "task-d".into(),
    }
}

impl Fixture {
    pub(crate) fn move_to(&self, stage: &str) {
        self.db
            .execute_test_sql(&format!(
                "UPDATE pipeline_item SET stage = '{stage}' WHERE id = 'task-d'"
            ))
            .unwrap();
    }

    pub(crate) fn view(&self) -> DesignView {
        view(&self.db, &self.runtime, &self.db_path, &self.task, true).unwrap()
    }

    pub(crate) fn first_block(&self) -> ProjectedBlock {
        self.view().document.unwrap().blocks[0].clone()
    }

    pub(crate) fn comment(&self, thread: &str, anchor: Option<&str>) -> ThreadView {
        create_thread(
            &self.db,
            &self.runtime,
            &self.task,
            CreateThreadRequest {
                thread_id: thread.into(),
                comment_id: format!("{thread}-c1"),
                kind: if anchor.is_some() {
                    "comment"
                } else {
                    "message"
                }
                .into(),
                body: format!("feedback on {thread}"),
                anchor: anchor.map(|block| AnchorRequest {
                    block_id: block.into(),
                    quoted_text: "quoted".into(),
                    state_vector: None,
                }),
            },
            Some("{\"kind\":\"test\"}"),
        )
        .unwrap()
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn a_task_in_its_design_stage_gets_one_session_at_the_first_position() {
    let fixture = fixture("session");
    let view = fixture.view();
    assert_eq!(view.stage, "design");
    assert!(view.in_design_stage);
    assert_eq!(view.position, "static");
    assert_eq!(
        view.positions
            .iter()
            .map(|position| position.name.as_str())
            .collect::<Vec<_>>(),
        vec!["static", "interactive", "prototype"]
    );
    assert_eq!(
        view.stage_chain,
        vec!["design", "plan", "in progress", "review", "pr"]
    );
    assert_eq!(view.next_stage.as_deref(), Some("plan"));
    assert_eq!(view.epoch, 1);
    // A new document is one empty paragraph, as BlockNote starts.
    let document = view.document.unwrap();
    assert_eq!(document.blocks.len(), 1);
    assert_eq!(document.blocks[0].kind, "paragraph");
    // The disposable repository lives outside the worktree, in the task's directory.
    let scratch = view.scratch_repository.unwrap();
    assert!(std::path::Path::new(&scratch).join(".git").exists());
    // Seeing it again reuses the session.
    assert_eq!(fixture.view().epoch, 1);
}

#[test]
fn a_position_is_state_never_a_transition() {
    let fixture = fixture("position");
    fixture.view();
    let runs_before = fixture.db.latest_stage_run(&fixture.task).unwrap();
    assert_eq!(
        set_position(&fixture.db, &fixture.runtime, &fixture.task, "prototype").unwrap(),
        "prototype"
    );
    assert_eq!(fixture.view().position, "prototype");
    // Same stage, no new run, no budget, no workspace: nothing but the position moved.
    let item = fixture
        .db
        .get_pipeline_item(&fixture.task)
        .unwrap()
        .unwrap();
    assert_eq!(item.stage.as_deref(), Some("design"));
    assert_eq!(
        fixture
            .db
            .latest_stage_run(&fixture.task)
            .unwrap()
            .map(|run| run.id),
        runs_before.map(|run| run.id)
    );
    assert!(matches!(
        set_position(&fixture.db, &fixture.runtime, &fixture.task, "shipping"),
        Err(DesignError::Invalid { .. })
    ));
}

#[test]
fn feedback_is_numbered_by_creation_and_queued_while_agent_replies_are_not() {
    let fixture = fixture("threads");
    let block = fixture.first_block().id;
    let first = fixture.comment("t-one", Some(&block));
    let second = fixture.comment("t-two", None);
    assert_eq!((first.number, second.number), (1, 2));
    assert_eq!(first.delivery_status, "queued");
    assert_eq!(second.kind, "message");
    assert!(second.anchor.is_none());

    let replied = reply_as_agent(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        "t-one",
        "op-1",
        "Done: tightened it.",
    )
    .unwrap();
    assert_eq!(replied.delivery_status, "agent_replied");
    // Idempotent: the same op id is the same reply.
    let again = reply_as_agent(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        "t-one",
        "op-1",
        "Done: tightened it.",
    )
    .unwrap();
    assert_eq!(again.comments.len(), 2);
    // The agent's reply queued nothing back to the agent.
    assert_eq!(
        fixture.db.design_deliveries(&fixture.task).unwrap().len(),
        2
    );

    // The person answers back: queued like the first comment.
    let back = reply_as_operator(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        "t-one",
        ReplyRequest {
            comment_id: "t-one-c3".into(),
            body: "Better, but shorter still".into(),
        },
        None,
    )
    .unwrap();
    assert_eq!(back.delivery_status, "queued");
    assert_eq!(
        fixture.db.design_deliveries(&fixture.task).unwrap().len(),
        3
    );

    // Resolve and reopen never renumber.
    let resolved = set_resolved(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        "t-one",
        true,
        "operator",
    )
    .unwrap();
    assert_eq!((resolved.number, resolved.status.as_str()), (1, "resolved"));
    let reopened = set_resolved(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        "t-one",
        false,
        "agent",
    )
    .unwrap();
    assert_eq!((reopened.number, reopened.status.as_str()), (1, "open"));
    let numbers: Vec<i64> = fixture
        .view()
        .threads
        .iter()
        .map(|thread| thread.number)
        .collect();
    assert_eq!(numbers, vec![1, 2]);
}

#[test]
fn feedback_is_refused_empty_or_unanchored_and_retries_are_the_same_thread() {
    let fixture = fixture("refusals");
    let request = |kind: &str, body: &str, anchor: Option<AnchorRequest>| CreateThreadRequest {
        thread_id: "t-x".into(),
        comment_id: "t-x-c1".into(),
        kind: kind.into(),
        body: body.into(),
        anchor,
    };
    assert!(create_thread(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        request("message", "  ", None),
        None
    )
    .is_err());
    assert!(create_thread(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        request("comment", "x", None),
        None
    )
    .is_err());
    let created = create_thread(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        request("message", "hello", None),
        None,
    )
    .unwrap();
    let retried = create_thread(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        request("message", "hello", None),
        None,
    )
    .unwrap();
    assert_eq!(created.id, retried.id);
    assert_eq!(
        fixture.db.design_deliveries(&fixture.task).unwrap().len(),
        1
    );
}

#[test]
fn agent_edits_apply_once_and_conflicts_record_nothing() {
    let fixture = fixture("edits");
    let block = fixture.first_block();
    let insert = [BlockOp::InsertBlock {
        after: Some(block.id.clone()),
        parent: None,
        block: NewBlock {
            kind: "heading".into(),
            text: "Artifact viewer".into(),
            props: serde_json::Map::new(),
        },
    }];
    let first = agent_edit(
        &fixture.db,
        &fixture.runtime,
        &fixture.db_path,
        &fixture.task,
        "edit-1",
        &insert,
    )
    .unwrap();
    assert_eq!(first["status"], "applied");
    let revision = first["revision"].as_i64().unwrap();
    // Retrying the same op id replays its result and inserts nothing twice.
    let replay = agent_edit(
        &fixture.db,
        &fixture.runtime,
        &fixture.db_path,
        &fixture.task,
        "edit-1",
        &insert,
    )
    .unwrap();
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["blockIds"], first["blockIds"]);
    assert_eq!(fixture.view().document.unwrap().blocks.len(), 2);
    assert_eq!(fixture.view().doc_revision, revision);

    let stale = [BlockOp::ReplaceText {
        block_id: block.id.clone(),
        expected_text: "not what is there".into(),
        text: "x".into(),
    }];
    let conflict = agent_edit(
        &fixture.db,
        &fixture.runtime,
        &fixture.db_path,
        &fixture.task,
        "edit-2",
        &stale,
    )
    .unwrap();
    assert_eq!(conflict["status"], "conflict");
    assert_eq!(conflict["conflicts"][0]["current"]["text"], "");
    // Not recorded: the corrected edit may reuse the op id.
    let fixed = [BlockOp::ReplaceText {
        block_id: block.id.clone(),
        expected_text: String::new(),
        text: "Intro".into(),
    }];
    let applied = agent_edit(
        &fixture.db,
        &fixture.runtime,
        &fixture.db_path,
        &fixture.task,
        "edit-2",
        &fixed,
    )
    .unwrap();
    assert_eq!(applied["status"], "applied");
}

#[test]
fn the_persons_editor_syncs_by_state_vector_in_the_one_schema() {
    let fixture = fixture("sync");
    fixture.view();
    // A client on another schema is refused before it can touch anything.
    assert!(matches!(
        sync_document(
            &fixture.db,
            &fixture.runtime,
            &fixture.db_path,
            &fixture.task,
            "blocknote@0.1",
            &[],
            None
        ),
        Err(DesignError::Schema { .. })
    ));
    // A new client gets the whole document.
    let full = sync_document(
        &fixture.db,
        &fixture.runtime,
        &fixture.db_path,
        &fixture.task,
        document::SCHEMA_VERSION,
        &[],
        None,
    )
    .unwrap();
    let mut client = DesignDocument::from_state(&full.update).unwrap();
    let block = client.project().unwrap()[0].id.clone();
    // The client edits locally and sends its update.
    let before = client.state_vector();
    let document::EditOutcome::Applied(edit) = client
        .apply_ops(&[BlockOp::ReplaceText {
            block_id: block.clone(),
            expected_text: String::new(),
            text: "typed by the person".into(),
        }])
        .unwrap()
    else {
        panic!("edit applies")
    };
    let _ = before;
    let synced = sync_document(
        &fixture.db,
        &fixture.runtime,
        &fixture.db_path,
        &fixture.task,
        document::SCHEMA_VERSION,
        &client.state_vector(),
        Some(&edit.update),
    )
    .unwrap();
    assert!(synced.revision > full.revision);
    assert_eq!(fixture.first_block().text, "typed by the person");
    // Once the design is handed off, the document is read-only.
    fixture.move_to("plan");
    assert!(matches!(
        sync_document(
            &fixture.db,
            &fixture.runtime,
            &fixture.db_path,
            &fixture.task,
            document::SCHEMA_VERSION,
            &[],
            Some(&edit.update),
        ),
        Err(DesignError::NotDesigning { .. })
    ));
    let _ = b64(&[]);
}

#[test]
fn a_restart_and_a_rebuild_from_disk_keep_every_acknowledged_edit() {
    let fixture = fixture("rebuild");
    let block = fixture.first_block();
    agent_edit(
        &fixture.db,
        &fixture.runtime,
        &fixture.db_path,
        &fixture.task,
        "edit-r",
        &[BlockOp::ReplaceText {
            block_id: block.id.clone(),
            expected_text: String::new(),
            text: "kept across restarts".into(),
        }],
    )
    .unwrap();
    // A restart: a fresh process cache reloads from the database.
    let restarted = DesignRuntime::default();
    let view = view(
        &fixture.db,
        &restarted,
        &fixture.db_path,
        &fixture.task,
        true,
    )
    .unwrap();
    assert_eq!(
        view.document.unwrap().blocks[0].text,
        "kept across restarts"
    );
    // A database rebuilt from disk has the carried rows but no stored
    // updates: the document comes back from the task directory's file.
    fixture
        .db
        .execute_test_sql("DELETE FROM design_doc_update")
        .unwrap();
    let rebuilt = DesignRuntime::default();
    let view = super::view(&fixture.db, &rebuilt, &fixture.db_path, &fixture.task, true).unwrap();
    assert_eq!(
        view.document.unwrap().blocks[0].text,
        "kept across restarts"
    );
}

#[test]
fn a_design_handed_off_and_sent_back_starts_a_new_epoch() {
    let fixture = fixture("epoch");
    fixture.view();
    fixture.comment("t-held", None);
    fixture
        .db
        .set_design_session_status(&fixture.task, DesignSessionRow::HANDED_OFF)
        .unwrap();
    fixture.move_to("plan");
    let handed_off = fixture.view();
    assert!(!handed_off.in_design_stage);
    // Feedback accepted before the hand-off is held, never lost.
    assert_eq!(handed_off.threads[0].delivery_status, "held");
    assert!(create_thread(
        &fixture.db,
        &fixture.runtime,
        &fixture.task,
        CreateThreadRequest {
            thread_id: "t-late".into(),
            comment_id: "t-late-c1".into(),
            kind: "message".into(),
            body: "too late".into(),
            anchor: None,
        },
        None,
    )
    .is_err());
    fixture.move_to("design");
    let reopened = fixture.view();
    assert_eq!(reopened.epoch, 2);
    assert_eq!(reopened.status, DesignSessionRow::DESIGNING);
    // The held feedback moves to the new epoch and is queued again.
    assert_eq!(reopened.threads[0].delivery_status, "queued");
    assert_eq!(
        fixture.db.design_deliveries(&fixture.task).unwrap()[0].epoch,
        2
    );
}

#[test]
fn a_task_without_a_design_stage_has_no_design() {
    let fixture = fixture("no-design");
    fixture
        .db
        .execute_test_sql("UPDATE pipeline_item SET pipeline_def = NULL, pipeline = 'no-review' WHERE id = 'task-d'")
        .unwrap();
    assert!(matches!(
        view(
            &fixture.db,
            &fixture.runtime,
            &fixture.db_path,
            &fixture.task,
            false
        ),
        Err(DesignError::NotDesigning { .. }) | Err(DesignError::Internal { .. })
    ));
}
