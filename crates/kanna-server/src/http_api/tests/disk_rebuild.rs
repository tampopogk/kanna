//! Offline rebuild from task directories, round-tripped against a migrated
//! fixture database (spec §11, §16.11 — T13 first and second increments).
//!
//! The fixture is a database created with the production migrations and
//! driven through the real endpoints and writers to the representative open
//! task shapes; T0's startup recovery backfills and flushes its ledgers and
//! records; the store is rebuilt into fresh databases; and every durable row
//! of both is compared. Statistics and transient live-session state
//! ([`crate::task_store::rebuild::NOT_REBUILT`]) are not compared; everything
//! else must be identical.
use super::actions::{
    commit_branch_change, ledger_fixture_config, post_json, spawn_recording_daemon,
    wait_for_running_task_stage,
};
use super::*;
use crate::mutation_provenance::ChannelIdentity;
use crate::task_store::rebuild;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn legacy_workflow() -> Value {
    json!({
        "name": "legacy",
        "stages": [
            { "name": "in progress", "transition": "manual" },
            { "name": "review", "transition": "manual", "agent": "review", "prompt": "Review." },
            { "name": "pr", "transition": "manual", "agent": "pr", "prompt": "Open it." }
        ]
    })
}

fn legacy_post_workflow() -> Value {
    json!({
        "name": "legacy-post",
        "stages": [
            { "name": "in progress", "transition": "manual",
              "post": { "name": "commit", "prompt": "Commit $TASK_PROMPT" } },
            { "name": "review", "transition": "manual", "agent": "review", "prompt": "Review." }
        ]
    })
}

fn exits_workflow() -> Value {
    json!({
        "name": "exits-flow",
        "routing": "exits",
        "stages": [
            { "name": "plan", "agent": "plan", "prompt": "$TASK_PROMPT",
              "policy": { "transition": "manual" } },
            { "name": "in progress", "agent": "implement", "prompt": "$TASK_PROMPT",
              "budget": 1,
              "policy": { "transition": "manual", "loop_transition": "auto" } },
            { "name": "review", "agent": "review", "prompt": "Review the branch.",
              "exits": { "revise": "in progress", "replan": "plan" },
              "policy": { "transition": "auto" } },
            { "name": "pr", "agent": "pr", "prompt": "Open the PR.",
              "policy": { "transition": "manual" } }
        ]
    })
}

pub(super) struct Fixture {
    pub(super) app: axum::Router,
    pub(super) state: Arc<AppState>,
    pub(super) db_path: String,
    pub(super) repo_root: PathBuf,
    pub(super) daemon_dir: PathBuf,
    /// Every command the fixture's daemon received (T13c counts them).
    pub(super) daemon_commands: Arc<std::sync::Mutex<Vec<kanna_daemon::protocol::Command>>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

impl Fixture {
    pub(super) fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    pub(super) async fn post(&self, task_id: &str, action: &str, body: Value) -> Value {
        let (status, text) = post_json(
            &self.app,
            &format!("/v1/tasks/{task_id}/actions/{action}"),
            body,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{task_id} {action}: {text}");
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, task_id).await;
        serde_json::from_str(&text).unwrap_or(Value::Null)
    }

    fn running_run(&self, task_id: &str, stage: &str) -> String {
        self.db()
            .list_stage_runs_for_task(task_id)
            .unwrap()
            .into_iter()
            .find(|run| run.stage == stage && run.status == "running")
            .unwrap_or_else(|| panic!("{task_id} has no running {stage} run"))
            .id
    }
}

/// Insert a task on `workflow` with its own worktree, created after the
/// ledger bridge exists (so it has no history to backfill).
fn insert_task(db: &Db, repo_root: &Path, id: &str, stage: &str, workflow: &Value) -> PathBuf {
    let branch = format!("task-{id}");
    let worktree = commit_branch_change(repo_root, &branch, &format!("{id}.txt"), id);
    db.insert_test_pipeline_item(
        id,
        "repo-1",
        &format!("Prompt of {id}"),
        Some(&format!("Title of {id}")),
        stage,
        "2026-09-23 09:00:00",
    )
    .unwrap();
    db.update_test_pipeline_item_stage_context(
        id,
        &branch,
        workflow["name"].as_str().unwrap(),
        None,
        "claude",
    )
    .unwrap();
    db.update_test_pipeline_item_pipeline_def(id, &workflow.to_string())
        .unwrap();
    db.upsert_worktree(
        &format!("wt-{id}"),
        id,
        &worktree.to_string_lossy(),
        &branch,
    )
    .unwrap();
    db.mark_task_ledger_backfilled(id, 0).unwrap();
    worktree
}

fn insert_run(db: &Db, id: &str, task_id: &str, stage: &str, agent: &str, cwd: &Path) {
    db.insert_stage_run(crate::db::NewStageRun {
        id,
        task_id,
        stage,
        kind: "main",
        agent: Some(agent),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "running",
        result: None,
        feedback: None,
        session_id: Some(task_id),
        provider_session_id: None,
        cwd: Some(&cwd.to_string_lossy()),
        resumed_from_run_id: None,
    })
    .unwrap();
}

fn git_head(dir: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

pub(super) async fn build_fixture() -> Fixture {
    let repo_root = crate::test_paths::unique_test_path("kanna-disk-rebuild");
    init_test_git_repo(&repo_root);
    let daemon_dir = crate::test_paths::unique_test_path("kanna-disk-rebuild-d");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    let daemon_commands = spawn_recording_daemon(&daemon_dir);
    let config = ledger_fixture_config("disk-rebuild", &daemon_dir);
    // The production migrations, not the test schema: the rebuild must
    // match what a real installation holds, foreign keys included.
    let _ = std::fs::remove_file(&config.db_path);
    let db = Db::open_migrated(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();

    let gate = insert_task(&db, &repo_root, "gate", "in progress", &legacy_workflow());
    insert_run(&db, "gate-run", "gate", "in progress", "implement", &gate);
    let active = insert_task(&db, &repo_root, "active", "in progress", &legacy_workflow());
    insert_run(
        &db,
        "active-run",
        "active",
        "in progress",
        "implement",
        &active,
    );
    insert_task(
        &db,
        &repo_root,
        "blocked",
        "in progress",
        &legacy_workflow(),
    );
    let revision = insert_task(&db, &repo_root, "revision", "review", &legacy_workflow());
    insert_run(
        &db,
        "revision-review",
        "revision",
        "review",
        "review",
        &revision,
    );
    let post = insert_task(
        &db,
        &repo_root,
        "post",
        "in progress",
        &legacy_post_workflow(),
    );
    insert_run(&db, "post-main", "post", "in progress", "implement", &post);
    let exits = insert_task(&db, &repo_root, "exits", "review", &exits_workflow());
    insert_run(&db, "exits-review-1", "exits", "review", "review", &exits);
    let artifact = insert_task(
        &db,
        &repo_root,
        "artifact",
        "in progress",
        &legacy_workflow(),
    );
    insert_run(
        &db,
        "artifact-run",
        "artifact",
        "in progress",
        "implement",
        &artifact,
    );

    // Pre-ledger history: a finished run with a verdict, a delivered input
    // and a stage change, whose ledger T0's startup backfill must import.
    let history = insert_task(
        &db,
        &repo_root,
        "history",
        "in progress",
        &legacy_workflow(),
    );
    db.insert_stage_run(crate::db::NewStageRun {
        id: "history-run",
        task_id: "history",
        stage: "in progress",
        kind: "main",
        agent: Some("implement"),
        agent_provider: Some("claude"),
        model: None,
        effort: None,
        status: "succeeded",
        result: Some(r#"{"status":"success","summary":"Old work","metadata":null}"#),
        feedback: Some("Old work"),
        session_id: Some("history"),
        provider_session_id: None,
        cwd: Some(&history.to_string_lossy()),
        resumed_from_run_id: None,
    })
    .unwrap();
    db.record_task_input(
        "history",
        crate::db::TaskInputSource::Operator,
        &ChannelIdentity::Unknown,
        "an old instruction",
    )
    .unwrap();
    db.update_pipeline_item_stage("history", "review").unwrap();
    db.delete_ledger_entries_for_tests("history").unwrap();

    // The running task receives tool input; the blocked one waits on it.
    db.record_task_input(
        "active",
        crate::db::TaskInputSource::Operator,
        &ChannelIdentity::Unknown,
        "please also cover the retry",
    )
    .unwrap();
    db.record_task_input(
        "active",
        crate::db::TaskInputSource::Manager,
        &ChannelIdentity::Server,
        "status?",
    )
    .unwrap();
    db.insert_task_blocker("blocked", "active").unwrap();
    let artifact_sha = git_head(&artifact);
    drop(db);

    let state = Arc::new(super::AppState::new(config.clone()));
    let app = super::router(Arc::clone(&state));
    let fixture = Fixture {
        app,
        state,
        db_path: config.db_path.clone(),
        repo_root,
        daemon_dir,
        daemon_commands,
    };

    // Parked manual gate.
    fixture
        .post(
            "gate",
            "complete-stage",
            json!({ "runId": "gate-run", "status": "success",
                    "summary": "Implemented the parser\n\nRan the unit tests." }),
        )
        .await;

    // Artifact-referencing result, also parked at its gate.
    fixture
        .post(
            "artifact",
            "complete-stage",
            json!({
                "runId": "artifact-run", "status": "success", "summary": "Mockup and diff",
                "artifacts": {
                    "diff": { "type": "commit", "repoId": "repo-1", "sha": artifact_sha },
                    "pr": { "type": "pr", "url": "https://example.test/pull/7", "headSha": artifact_sha },
                },
            }),
        )
        .await;

    // Legacy revision: the reviewer sends the task back.
    fixture
        .post(
            "revision",
            "request-revision",
            json!({ "runId": "revision-review", "targetStage": "in progress",
                    "summary": "Two defects", "prompt": "Fix the parser and the retry",
                    "origin": "agent" }),
        )
        .await;
    wait_for_running_task_stage(&fixture.db(), "revision", "in progress").await;

    // Legacy custom post: the main run parks, the operator advances, the post
    // runs and its verdict performs the deferred transition.
    fixture
        .post(
            "post",
            "complete-stage",
            json!({ "runId": "post-main", "status": "success", "summary": "Implemented" }),
        )
        .await;
    fixture
        .post("post", "advance-stage", json!({ "source": "operator" }))
        .await;
    let post_run = {
        let db = fixture.db();
        let mut found = None;
        for _ in 0..100 {
            found = db
                .list_stage_runs_for_task("post")
                .unwrap()
                .into_iter()
                .find(|run| run.kind == "post" && run.status == "running");
            if found.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        found.expect("post run started").id
    };
    fixture
        .post(
            "post",
            "complete-stage",
            json!({ "runId": post_run, "status": "success", "summary": "Committed" }),
        )
        .await;
    wait_for_running_task_stage(&fixture.db(), "post", "review").await;

    // Named-exit loop until its destination budget is spent.
    fixture
        .post(
            "exits",
            "complete-stage",
            json!({ "runId": "exits-review-1", "status": "success",
                    "summary": "One defect", "exit": "revise" }),
        )
        .await;
    wait_for_running_task_stage(&fixture.db(), "exits", "in progress").await;
    let reviser = fixture.running_run("exits", "in progress");
    fixture
        .post(
            "exits",
            "complete-stage",
            json!({ "runId": reviser, "status": "success", "summary": "Fixed it" }),
        )
        .await;
    wait_for_running_task_stage(&fixture.db(), "exits", "review").await;
    let second_review = fixture.running_run("exits", "review");
    let parked = fixture
        .post(
            "exits",
            "complete-stage",
            json!({ "runId": second_review, "status": "success",
                    "summary": "Still one defect", "exit": "revise" }),
        )
        .await;
    assert_eq!(parked["routing"]["outcome"], "parked", "{parked}");

    // State the ledger does not carry, set the way its writers leave it.
    let db = fixture.db();
    let conn = db.connection_for_e2e_tests();
    conn.execute(
        "UPDATE pipeline_item SET agent_provider = 'codex', pinned = 1, pin_order = 3,
                initial_pipeline = 'first-flow', issue_number = 42, issue_title = 'Issue',
                pr_branch = 'feature/x', merge_signaled_at = '2026-09-23 12:00:00'
         WHERE id = 'active'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE pipeline_item SET attention_requested = 1 WHERE id = 'gate'",
        [],
    )
    .unwrap();
    // The repository's registration and sidebar order.
    conn.execute(
        "UPDATE repo SET remote_url = 'git@example.test:o/r.git', remote_url_hash = 'hash-1',
                default_branch_source = 'remote', sort_order = 2
         WHERE id = 'repo-1'",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO repo_sidebar_order (remote_url_hash, sort_order) VALUES ('hash-1', 5)",
        [],
    )
    .unwrap();

    // Active/recovered session: the session is lost (the engine records the
    // ending), then the daemon proves it alive and the run is restored.
    db.finish_latest_running_stage_run("active", "failed", None, Some("session lost"))
        .unwrap()
        .expect("active had a running run");
    assert!(db
        .restore_latest_interrupted_stage_run("active", "session lost")
        .unwrap());
    // Its session identity, workspace, branch number, prompt and setup.
    let active_dir = db
        .get_task_worktree_path("active")
        .unwrap()
        .expect("active has a worktree");
    let number = db.reserve_task_branch_number("active", 0).unwrap();
    let session_branch = format!("task-active-{number}");
    db.upsert_stage_workspace(
        "ws-active",
        "active",
        "in progress",
        &active_dir,
        &session_branch,
    )
    .unwrap();
    db.set_stage_run_session(
        "active-run",
        &crate::db::StageRunSession {
            workspace_id: Some("ws-active".into()),
            branch: Some(session_branch.clone()),
            name: Some("Implement: Title of active".into()),
            transcript: Some(crate::db::TranscriptRef {
                provider: "claude".into(),
                session_id: "sess-active".into(),
                path: None,
            }),
            workspace_report: Some("kept 1 uncommitted file".into()),
        },
    )
    .unwrap();
    db.record_stage_run_prompt("active-run", "Implement: Prompt of active")
        .unwrap();
    db.record_workspace_setup_run(
        "active-run",
        &crate::db::WorkspaceSetupOutcome {
            exit_code: Some(0),
            timed_out: false,
            truncated: false,
            commands: vec!["pnpm install".into()],
            output: "ok".into(),
            duration_ms: 1200,
        },
    )
    .unwrap();
    conn.execute(
        "INSERT INTO task_provider_rejection
            (task_id, stage_run_id, stage, provider, source, rule_id, matched_text, scope, recovery)
         VALUES ('active', 'active-run', 'in progress', 'claude', 'pty', 'quota-1',
                 'usage limit reached', 'account', 'none')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO task_provider_capacity_notice
            (task_id, stage_run_id, stage, provider, source, rule_id, matched_text, scope)
         VALUES ('active', 'active-run', 'in progress', 'claude', 'pty', 'cap-1',
                 'capacity', 'model')",
        [],
    )
    .unwrap();
    // A completion retry key on the parked gate, and the PR it reviews.
    db.record_contextless_completion_attempt("attempt-1", "gate-run", "success")
        .unwrap();
    db.upsert_task_review_context(
        "gate",
        &crate::db::ReviewContextInput {
            pr_url: "https://example.test/pull/9".into(),
            head_sha: "a".repeat(40),
            base_ref: "main".into(),
            ..Default::default()
        },
    )
    .unwrap();
    conn.execute(
        "INSERT INTO human_review_decision
            (id, task_id, review_context_version, pr_url, head_sha, base_ref, action_text, origin)
         VALUES ('decision-1', 'gate', 1, 'https://example.test/pull/9', ?, 'main',
                 'approve', 'desktop')",
        [&"a".repeat(40)],
    )
    .unwrap();
    db.insert_create_task_intent(
        "blocked",
        r#"{"repoId":"repo-1","prompt":"Prompt of blocked"}"#,
    )
    .unwrap();

    // Blocked first stage: an edge into the dependent's first stage, before
    // it started. Blocked later stage: an edge into review, and the
    // completion parked on it.
    insert_task(
        &db,
        &fixture.repo_root,
        "downstream",
        "in progress",
        &legacy_workflow(),
    );
    db.insert_stage_edges(
        "downstream",
        &[crate::db::NewStageEdge {
            upstream_task_id: "active".into(),
            upstream_stage: "in progress".into(),
            dependent_stage: None,
        }],
    )
    .unwrap();
    insert_task(
        &db,
        &fixture.repo_root,
        "waits",
        "in progress",
        &legacy_workflow(),
    );
    db.insert_stage_edges(
        "waits",
        &[crate::db::NewStageEdge {
            upstream_task_id: "gate".into(),
            upstream_stage: "review".into(),
            dependent_stage: Some("review".into()),
        }],
    )
    .unwrap();
    db.record_dependency_wait("waits", "in progress", "review", &json!({ "kind": "main" }))
        .unwrap();

    // Subtask join: a member not created yet holds the parent's owed
    // transition.
    let joiner = insert_task(
        &db,
        &fixture.repo_root,
        "joiner",
        "in progress",
        &legacy_workflow(),
    );
    insert_run(
        &db,
        "joiner-run",
        "joiner",
        "in progress",
        "implement",
        &joiner,
    );
    db.create_task_join(&crate::db::NewTaskJoin {
        id: "join-1".into(),
        parent_task_id: "joiner".into(),
        parent_stage: Some("in progress".into()),
        parent_run_id: Some("joiner-run".into()),
        base_sha: git_head(&joiner),
        base_branch: Some("task-joiner".into()),
        members: vec![crate::db::NewJoinMember {
            child_task_id: "c0ffee99".into(),
            spec: json!({ "prompt": "child" }).to_string(),
        }],
    })
    .unwrap();
    db.put_ledger_continuation(
        "joiner",
        "op-joiner",
        crate::db::task_store::STAGE_COMPLETION_CONTINUATION,
        &json!({ "runId": "joiner-run", "generation": 1 }),
    )
    .unwrap();

    // Pending commit step: requested, with its delivery in flight.
    let commit = insert_task(
        &db,
        &fixture.repo_root,
        "commit",
        "in progress",
        &legacy_workflow(),
    );
    insert_run(
        &db,
        "commit-run",
        "commit",
        "in progress",
        "implement",
        &commit,
    );
    db.insert_transition_commit("commit-run", "commit", "in progress", None)
        .unwrap();
    db.insert_lifecycle_operation_intent(
        "op-commit",
        "commit",
        "post",
        "submitted",
        &json!({ "version": 1, "task_id": "commit", "run_id": "commit-run" }).to_string(),
    )
    .unwrap();

    // Pending transfer claim: an incoming transfer claimed for this task.
    insert_task(
        &db,
        &fixture.repo_root,
        "incoming",
        "in progress",
        &legacy_workflow(),
    );
    db.insert_task_transfer(&crate::db::NewTaskTransfer {
        id: "xfer-in".into(),
        direction: "incoming".into(),
        status: "pending".into(),
        source_peer_id: Some("peer-b".into()),
        target_peer_id: None,
        source_desktop_id: None,
        target_desktop_id: None,
        source_task_id: Some("remote-1".into()),
        local_task_id: Some("incoming".into()),
        error: None,
        payload_json: Some("{}".into()),
    })
    .unwrap();
    assert!(db
        .claim_pending_incoming_transfer("xfer-in", CLAIM_TOKEN, false)
        .unwrap());
    db.insert_task_transfer_provenance(&crate::db::NewTaskTransferProvenance {
        pipeline_item_id: "incoming".into(),
        source_peer_id: "peer-b".into(),
        source_task_id: "remote-1".into(),
        source_machine_task_label: Some("remote task".into()),
    })
    .unwrap();
    conn.execute_batch(
        "INSERT INTO transferred_task_context (task_id, transfer_id, workflow_definition)
         VALUES ('incoming', 'xfer-in', '{}');
         INSERT INTO transferred_task_manifest
            (transfer_id, repo_id, local_task_id, head_oid, base_oid, state)
         VALUES ('xfer-in', 'repo-1', 'incoming', 'h', 'b', 'prepared');
         INSERT INTO transferred_task_history
            (task_id, sequence, origin_peer_id, origin_task_id, origin_run_id, stage, kind)
         VALUES ('incoming', 1, 'peer-b', 'remote-1', 'remote-run', 'in progress', 'main');
         INSERT INTO transferred_task_state
            (pipeline_item_id, transfer_id, source_peer_id, source_task_id,
             ownership_generation, state_sha256, links, session_start, fresh_start_reason)
         VALUES ('incoming', 'xfer-in', 'peer-b', 'remote-1', 1, 'sha', '{}', 'fresh',
                 'no transcript');",
    )
    .unwrap();
    // An outgoing transfer holding a task's workflow, its ledger fenced.
    insert_task(
        &db,
        &fixture.repo_root,
        "outgoing",
        "in progress",
        &legacy_workflow(),
    );
    db.insert_task_transfer(&crate::db::NewTaskTransfer {
        id: "xfer-out".into(),
        direction: "outgoing".into(),
        status: "streaming".into(),
        source_peer_id: None,
        target_peer_id: Some("peer-c".into()),
        source_desktop_id: None,
        target_desktop_id: None,
        source_task_id: Some("outgoing".into()),
        local_task_id: Some("outgoing".into()),
        error: None,
        payload_json: None,
    })
    .unwrap();
    db.claim_task_workflow_for_transfer("xfer-out", "outgoing")
        .unwrap()
        .unwrap();
    db.fence_ledger_for_transfer_export("outgoing", "xfer-out")
        .unwrap()
        .unwrap();

    // A task whose creation was rolled back after its task.json was written.
    insert_task(
        &db,
        &fixture.repo_root,
        "doomed",
        "in progress",
        &legacy_workflow(),
    );
    crate::task_store::flush_task(&db, &fixture.db_path, "doomed").unwrap();
    db.delete_task_creation_artifacts("doomed").unwrap();

    crate::task_store::recover_on_startup(&db, &fixture.db_path);
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
    assert!(db.repos_with_pending_disk_record().unwrap().is_empty());
    assert!(db.pending_disk_removals().unwrap().is_empty());
    fixture
}

/// The claim token of the pending incoming transfer: a capability that must
/// never reach disk.
pub(super) const CLAIM_TOKEN: &str = "claim-capability-7f3a";

/// Every durable row of a database, keyed `<table>|<identity>`, with the
/// columns a rebuild restores. Statistics and transient live-session state
/// (`rebuild::NOT_REBUILT`) are left out, and so is `pipeline_item.updated_at`,
/// which unrelated live writes move without owing a new task.json.
pub(super) fn durable_state(db: &Db) -> BTreeMap<String, Value> {
    let conn = db.connection_for_e2e_tests();
    let rows = |sql: &str| -> Vec<Vec<Value>> {
        let mut statement = conn.prepare(sql).unwrap();
        let columns = statement.column_count();
        statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get_ref(index).map(crate::db::task_state::sql_to_json))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let id = |value: &Value| {
        value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_string)
    };
    let mut state = BTreeMap::new();
    for table in crate::db::task_state::CARRIED_TABLES {
        let columns: Vec<&str> = table
            .columns
            .iter()
            .copied()
            .filter(|column| !(table.table == "pipeline_item" && *column == "updated_at"))
            .collect();
        let quoted: Vec<String> = columns
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect();
        for row in rows(&format!(
            "SELECT rowid, {} FROM {}",
            quoted.join(", "),
            table.table
        )) {
            let object: serde_json::Map<String, Value> = columns
                .iter()
                .zip(&row[1..])
                .map(|(column, value)| (column.to_string(), value.clone()))
                .collect();
            state.insert(format!("{}|{}", table.table, row[0]), Value::Object(object));
        }
    }
    let repo_columns = crate::db::task_state::REPO_COLUMNS.join(", ");
    for row in rows(&format!("SELECT {repo_columns} FROM repo")) {
        state.insert(format!("repo|{}", id(&row[0])), json!(row));
    }
    for row in rows("SELECT remote_url_hash, sort_order FROM repo_sidebar_order") {
        state.insert(
            format!("repo_sidebar_order|{}", id(&row[0])),
            row[1].clone(),
        );
    }
    for row in rows("SELECT blocked_item_id, blocker_item_id FROM task_blocker") {
        state.insert(
            format!("task_blocker|{}->{}", id(&row[0]), id(&row[1])),
            json!(true),
        );
    }
    for row in rows(
        "SELECT id, task_id, run_id, stage, source, message, delivered_at, origin_peer_id,
                origin_task_id, origin_input_id, origin_run_id, channel_identity
         FROM task_input",
    ) {
        state.insert(format!("task_input|{}", id(&row[0])), json!(row));
    }
    for row in rows(
        "SELECT entry_id, kind, file_name, operation_id, source_kind, source_id, payload
         FROM task_ledger_entry WHERE published_at IS NOT NULL",
    ) {
        state.insert(format!("task_ledger_entry|{}", id(&row[0])), json!(row));
    }
    state
}

pub(super) fn differences(
    source: &BTreeMap<String, Value>,
    rebuilt: &BTreeMap<String, Value>,
) -> Vec<String> {
    let keys: BTreeSet<&String> = source.keys().chain(rebuilt.keys()).collect();
    keys.into_iter()
        .filter(|key| source.get(*key) != rebuilt.get(*key))
        .map(|key| {
            format!(
                "{key}:\n  source:  {}\n  rebuilt: {}",
                source.get(key).unwrap_or(&Value::Null),
                rebuilt.get(key).unwrap_or(&Value::Null)
            )
        })
        .collect()
}

fn assert_same_rows(left: &[String], right: &[String]) {
    let left_only: Vec<&String> = left.iter().filter(|row| !right.contains(row)).collect();
    let right_only: Vec<&String> = right.iter().filter(|row| !left.contains(row)).collect();
    assert!(
        left_only.is_empty() && right_only.is_empty() && left.len() == right.len(),
        "only in the first: {left_only:#?}\nonly in the second: {right_only:#?}"
    );
}

pub(super) fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[tokio::test]
async fn a_migrated_fixture_round_trips_through_its_task_directories() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = build_fixture().await;
    let source = fixture.db();
    let root = crate::task_store::root_for_db(&fixture.db_path);

    // Every carried table is exercised by the fixture.
    let source_state = durable_state(&source);
    for table in crate::db::task_state::CARRIED_TABLES {
        assert!(
            source_state
                .keys()
                .any(|key| key.starts_with(&format!("{}|", table.table))),
            "the fixture holds no {} row",
            table.table
        );
    }
    // The engine recorded the lost session's ending, and no secret reached
    // disk.
    let endings: i64 = source
        .connection_for_e2e_tests()
        .query_row(
            "SELECT COUNT(*) FROM task_ledger_entry
             WHERE task_id = 'active' AND source_kind = 'stage_run_ending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(endings, 1);
    // A revision request records its summary and findings apart.
    let revision_dir = crate::task_store::task_dir(&root, "repo-1", "revision");
    let revision_result = crate::task_store::read_ledger(&revision_dir)
        .unwrap()
        .into_iter()
        .find(|file| file.body()["request"]["kind"] == "revision_request")
        .expect("the revision request's result entry");
    assert_eq!(revision_result.body()["request"]["summary"], "Two defects");
    assert_eq!(
        revision_result.body()["request"]["findings"],
        "Fix the parser and the retry"
    );
    let on_disk = files_under(&root);
    assert!(on_disk
        .iter()
        .all(|(_, bytes)| !String::from_utf8_lossy(bytes).contains(CLAIM_TOKEN)));

    let first = PathBuf::from(Db::test_db_path("disk-rebuild-first"));
    let second = PathBuf::from(Db::test_db_path("disk-rebuild-second"));
    let report = rebuild::rebuild_into_new_database(&root, &first).unwrap();
    assert!(report.unreadable.is_empty(), "{:?}", report.unreadable);
    assert_eq!(report.tasks, 14);
    assert_eq!(report.removed.len(), 1, "{:?}", report.removed);
    assert!(report.removed[0].ends_with("doomed"));
    for note in &report.diagnostics {
        eprintln!("rebuild diagnostic: {note}");
    }
    rebuild::rebuild_into_new_database(&root, &second).unwrap();
    let rebuilt = Db::open(first.to_str().unwrap()).unwrap();

    // Idempotent: two rebuilds are identical, and re-applying changes nothing.
    let dump = rebuilt.disk_rebuild_dump_for_tests();
    assert_same_rows(
        &dump,
        &Db::open(second.to_str().unwrap())
            .unwrap()
            .disk_rebuild_dump_for_tests(),
    );
    let scan = rebuild::scan_store_records(&root).unwrap();
    rebuilt
        .apply_disk_projection(&rebuild::project_store(&scan))
        .unwrap();
    assert_same_rows(&dump, &rebuilt.disk_rebuild_dump_for_tests());

    // Every durable row rebuilds exactly.
    let differing = differences(&source_state, &durable_state(&rebuilt));
    assert!(differing.is_empty(), "{}", differing.join("\n"));
    assert!(rebuild::NOT_REBUILT.iter().all(|gap| matches!(
        gap.gap,
        rebuild::Gap::Statistics | rebuild::Gap::Transient | rebuild::Gap::Unrecoverable
    )));

    // Pending effects are reconstructed, and nothing is re-done: the
    // rebuilt database owes nothing to publish, a flush writes nothing, the
    // owed transition is still held by its join, and the commit step and
    // its delivery wait for restart reconciliation exactly as before.
    let rebuilt_path = first.to_str().unwrap();
    assert!(rebuilt.ledger_tasks_with_pending_work().unwrap().is_empty());
    assert!(rebuilt.repos_with_pending_disk_record().unwrap().is_empty());
    assert!(rebuilt.pending_disk_removals().unwrap().is_empty());
    assert!(rebuilt.tasks_needing_ledger_backfill().unwrap().is_empty());
    assert!(crate::task_store::flush_all(&rebuilt, rebuilt_path).is_empty());
    assert!(!crate::task_store::root_for_db(rebuilt_path).exists());
    assert!(rebuilt
        .claim_ledger_continuation("joiner")
        .unwrap()
        .is_none());
    assert!(rebuilt.has_ledger_continuation("joiner").unwrap());
    assert_eq!(rebuilt.disk_rebuild_dump_for_tests(), dump);
    assert_eq!(files_under(&root), on_disk);

    // The facts the scenarios exist for, stated directly.
    let state = durable_state(&rebuilt);
    let task = |id: &str, column: &str| {
        state
            .iter()
            .find(|(key, value)| key.starts_with("pipeline_item|") && value["id"] == id)
            .map(|(_, value)| value[column].clone())
            .unwrap_or(Value::Null)
    };
    let run = |id: &str, column: &str| {
        state
            .iter()
            .find(|(key, value)| key.starts_with("stage_run|") && value["id"] == id)
            .map(|(_, value)| value[column].clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(task("gate", "stage"), "in progress");
    assert_eq!(task("gate", "attention_requested"), 1);
    assert_eq!(task("active", "agent_provider"), "codex");
    assert_eq!(task("active", "initial_pipeline"), "first-flow");
    assert_eq!(run("gate-run", "status"), "succeeded");
    assert_eq!(task("revision", "stage"), "in progress");
    assert_eq!(task("revision", "revision_rounds"), 1);
    assert_eq!(task("post", "stage"), "review");
    assert_eq!(task("exits", "stage"), "review");
    assert_eq!(run("active-run", "status"), "running");
    assert_eq!(run("active-run", "session_branch"), "task-active-1");
    assert_eq!(task("doomed", "id"), Value::Null);
    let stored = |key: &str| state.get(key).cloned().unwrap_or(Value::Null);
    assert_eq!(stored("task_blocker|blocked->active"), true);
    assert!(state
        .iter()
        .any(|(key, value)| key.starts_with("task_stage_budget|")
            && value["task_id"] == "exits"
            && value["stage"] == "in progress"
            && value["spent"] == 1));
    assert!(state
        .iter()
        .any(|(key, value)| key.starts_with("transition_commit|")
            && value["run_id"] == "commit-run"
            && value["state"] == "requested"));
    assert!(state
        .iter()
        .any(|(key, value)| key.starts_with("task_transfer|")
            && value["id"] == "xfer-in"
            && value["status"] == "claimed"));
    assert!(state
        .get("repo|repo-1")
        .is_some_and(|row| row[1] == fixture.repo_root.to_string_lossy().as_ref()));
}
