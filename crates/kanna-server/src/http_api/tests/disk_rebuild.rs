//! Offline rebuild from task directories, round-tripped against a migrated
//! fixture database (spec §16.11, T13 first increment).
//!
//! The fixture is a database created with the production migrations and
//! driven through the real endpoints and writers to the representative open
//! task shapes; T0's startup recovery backfills and flushes its ledgers; the
//! store is rebuilt into fresh databases; and the stable semantic state of
//! both is compared. Statistics and transient live-session state are not
//! compared. What differs must be exactly the facts
//! [`crate::task_store::rebuild::NOT_REBUILT`] declares.
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

struct Fixture {
    app: axum::Router,
    state: Arc<AppState>,
    db_path: String,
    repo_root: PathBuf,
    daemon_dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

impl Fixture {
    fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    async fn post(&self, task_id: &str, action: &str, body: Value) -> Value {
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

async fn build_fixture() -> Fixture {
    let repo_root = crate::test_paths::unique_test_path("kanna-disk-rebuild");
    init_test_git_repo(&repo_root);
    let daemon_dir = crate::test_paths::unique_test_path("kanna-disk-rebuild-d");
    std::fs::create_dir_all(&daemon_dir).unwrap();
    spawn_recording_daemon(&daemon_dir);
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
        "UPDATE pipeline_item SET agent_provider = 'codex', pinned = 1 WHERE id = 'active'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE pipeline_item SET attention_requested = 1 WHERE id = 'gate'",
        [],
    )
    .unwrap();
    // Every creation path marks task.json; the fixture's direct inserts do
    // not, so mark them before T0's startup recovery backfills and flushes.
    for task in [
        "gate", "active", "blocked", "revision", "post", "exits", "artifact", "history",
    ] {
        db.mark_task_snapshot_dirty(task).unwrap();
    }
    crate::task_store::recover_on_startup(&db, &fixture.db_path);
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
    fixture
}

/// The stable semantic state of a database, keyed `<fact class>|<instance>`.
/// Statistics, timestamps of live bookkeeping and transient session state
/// are left out.
fn semantic_state(db: &Db) -> BTreeMap<String, Value> {
    use rusqlite::types::Value as Sql;
    let conn = db.connection_for_e2e_tests();
    let rows = |sql: &str| -> Vec<Vec<Value>> {
        let mut statement = conn.prepare(sql).unwrap();
        let columns = statement.column_count();
        statement
            .query_map([], |row| {
                (0..columns)
                    .map(|index| {
                        Ok(match row.get::<_, Sql>(index)? {
                            Sql::Null => Value::Null,
                            Sql::Integer(value) => json!(value),
                            Sql::Real(value) => json!(value),
                            Sql::Text(value) => json!(value),
                            Sql::Blob(value) => json!(String::from_utf8_lossy(&value)),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let parse = |value: &Value| {
        value
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .unwrap_or(Value::Null)
    };
    let channel = |value: &Value| ChannelIdentity::from_column(value.as_str()).to_json();
    let mut state = BTreeMap::new();
    let mut put = |class: &str, instance: &str, value: Value| {
        state.insert(format!("{class}|{instance}"), value);
    };
    for row in rows("SELECT id, path, name, default_branch, remote_url FROM repo") {
        put(
            "repo.registration",
            row[0].as_str().unwrap(),
            json!([row[1], row[2], row[3], row[4]]),
        );
    }
    let task_columns = [
        "repo_id",
        "prompt",
        "display_name",
        "stage",
        "branch",
        "base_ref",
        "parent_task_id",
        "pr_url",
        "pr_number",
        "created_at",
        "closed_at",
        "agent_type",
        "agent_provider",
        "initial_pipeline",
        "revision_rounds",
        "attention_requested",
        "pinned",
    ];
    for row in rows(&format!(
        "SELECT id, pipeline, pipeline_def, {} FROM pipeline_item",
        task_columns.join(", ")
    )) {
        let id = row[0].as_str().unwrap().to_string();
        put("pin.name", &id, row[1].clone());
        put("pin.definition", &id, parse(&row[2]));
        for (column, value) in task_columns.iter().zip(&row[3..]) {
            put(&format!("task.{column}"), &id, value.clone());
        }
    }
    for row in rows("SELECT pipeline_item_id, path, branch FROM worktree ORDER BY id") {
        put(
            "task.worktree",
            &format!("{}/{}", row[0].as_str().unwrap(), row[2].as_str().unwrap()),
            row[1].clone(),
        );
    }
    for row in rows("SELECT id, task_id, stage, path, branch FROM stage_workspace") {
        put(
            "task.stage_workspace",
            row[0].as_str().unwrap(),
            json!([row[1], row[2], row[3], row[4]]),
        );
    }
    for row in rows("SELECT task_id, last_allocated FROM task_branch_counter") {
        put(
            "task.branch_counter",
            row[0].as_str().unwrap(),
            row[1].clone(),
        );
    }
    for row in rows("SELECT blocked_item_id, blocker_item_id FROM task_blocker") {
        put(
            "blocker",
            &format!("{}->{}", row[0].as_str().unwrap(), row[1].as_str().unwrap()),
            json!(true),
        );
    }
    for row in rows("SELECT task_id, stage, spent FROM task_stage_budget") {
        put(
            "budget",
            &format!("{}/{}", row[0].as_str().unwrap(), row[1].as_str().unwrap()),
            row[2].clone(),
        );
    }
    let runs = rows(
        "SELECT stage_run.id, stage, kind, status, result, feedback, result_declared_role,
                result_channel_identity, workspace_id, session_branch, session_name,
                transcript_ref, agent, agent_provider, model, effort, session_id,
                provider_session_id, cwd, resumed_from_run_id, replaces_run_id, trigger,
                entry_channel_identity, no_work_termination, workspace_report,
                completion_transition, completion_bound, stage_run_prompt.resolved_prompt
         FROM stage_run LEFT JOIN stage_run_prompt ON stage_run_prompt.run_id = stage_run.id
         WHERE kind IN ('main', 'post')",
    );
    for row in runs {
        let id = row[0].as_str().unwrap().to_string();
        put("run.exists", &id, json!(true));
        let (verdict, summary, metadata, _) = crate::db::task_store::normalize_recorded_result(
            row[3].as_str().unwrap(),
            row[4].as_str().unwrap_or(""),
        );
        let result = parse(&row[4]);
        let recorded = |value: Value| if row[4].is_null() { Value::Null } else { value };
        let artifact_names: Vec<String> = result
            .get("artifacts")
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        let fields = [
            ("run.stage", row[1].clone()),
            ("run.kind", row[2].clone()),
            ("run.status", row[3].clone()),
            ("run.verdict", recorded(json!(verdict))),
            ("run.summary", recorded(json!(summary))),
            ("run.metadata", recorded(metadata)),
            (
                "run.exit",
                result.get("exit").cloned().unwrap_or(Value::Null),
            ),
            ("run.artifacts", json!(artifact_names)),
            ("run.feedback", row[5].clone()),
            ("run.result_declared_role", row[6].clone()),
            ("run.result_channel", recorded(channel(&row[7]))),
            ("run.workspace_id", row[8].clone()),
            ("run.session_branch", row[9].clone()),
            ("run.session_name", row[10].clone()),
            ("run.transcript", parse(&row[11])),
            (
                "run.session",
                json!([
                    row[12],
                    row[13],
                    row[14],
                    row[15],
                    row[16],
                    row[17],
                    row[18],
                    row[19],
                    row[20],
                    row[21],
                    channel(&row[22]),
                    row[23],
                    row[24]
                ]),
            ),
            ("run.completion", json!([row[25], row[26]])),
            ("run.resolved_prompt", row[27].clone()),
        ];
        for (class, value) in fields {
            put(class, &id, value);
        }
    }
    for row in rows(
        "SELECT id, run_id, stage, source, message, delivered_at, origin_peer_id,
                origin_task_id, origin_input_id, origin_run_id, channel_identity
         FROM task_input",
    ) {
        let id = row[0].to_string();
        put("input.run_id", &id, row[1].clone());
        put("input.stage", &id, row[2].clone());
        put("input.source", &id, row[3].clone());
        put("input.message", &id, row[4].clone());
        put("input.delivered_at", &id, row[5].clone());
        put("input.origin", &id, json!([row[6], row[7], row[8], row[9]]));
        put("input.channel", &id, channel(&row[10]));
    }
    for row in rows(
        "SELECT entry_id, kind, file_name, operation_id, source_kind, source_id, payload
         FROM task_ledger_entry WHERE published_at IS NOT NULL",
    ) {
        let entry_id = row[0].as_str().unwrap().to_string();
        put("ledger.entry", &entry_id, json!(row[1..].to_vec()));
        if row[1] == "transition" {
            let file = crate::task_store::parse_ledger_file(
                row[2].as_str().unwrap(),
                row[6].as_str().unwrap().as_bytes(),
            )
            .unwrap();
            put("transition", &entry_id, file.body().clone());
        }
    }
    for row in rows("SELECT task_id, kind, payload FROM task_ledger_continuation") {
        put(
            "ledger.continuation",
            row[0].as_str().unwrap(),
            json!([row[1], row[2]]),
        );
    }
    state
}

fn differing_classes(
    source: &BTreeMap<String, Value>,
    rebuilt: &BTreeMap<String, Value>,
) -> BTreeMap<String, Vec<String>> {
    let mut classes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let keys: BTreeSet<&String> = source.keys().chain(rebuilt.keys()).collect();
    for key in keys {
        if source.get(key) != rebuilt.get(key) {
            let (class, instance) = key.split_once('|').unwrap();
            // A run missing on one side is one fact (`run.exists`), not one
            // per column.
            let run_exists = format!("run.exists|{instance}");
            if class.starts_with("run.")
                && class != "run.exists"
                && source.get(&run_exists) != rebuilt.get(&run_exists)
            {
                continue;
            }
            classes.entry(class.to_string()).or_default().push(format!(
                "{instance}: {} -> {}",
                source.get(key).unwrap_or(&Value::Null),
                rebuilt.get(key).unwrap_or(&Value::Null)
            ));
        }
    }
    classes
}

fn assert_same_rows(left: &[String], right: &[String]) {
    let left_only: Vec<&String> = left.iter().filter(|row| !right.contains(row)).collect();
    let right_only: Vec<&String> = right.iter().filter(|row| !left.contains(row)).collect();
    assert!(
        left_only.is_empty() && right_only.is_empty() && left.len() == right.len(),
        "only in the first: {left_only:#?}\nonly in the second: {right_only:#?}"
    );
}

#[tokio::test]
async fn a_migrated_fixture_round_trips_through_its_task_directories() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = build_fixture().await;
    let source = fixture.db();
    let root = crate::task_store::root_for_db(&fixture.db_path);

    let first = PathBuf::from(Db::test_db_path("disk-rebuild-first"));
    let second = PathBuf::from(Db::test_db_path("disk-rebuild-second"));
    let report = rebuild::rebuild_into_new_database(&root, &first).unwrap();
    assert!(report.unreadable.is_empty(), "{:?}", report.unreadable);
    assert_eq!(report.tasks, 8);
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
    let (directories, _) = rebuild::scan_store(&root).unwrap();
    rebuilt
        .apply_disk_projection(&rebuild::project(&directories))
        .unwrap();
    assert_same_rows(&dump, &rebuilt.disk_rebuild_dump_for_tests());

    // Everything the fixture exercises either rebuilds exactly or is a
    // declared gap, and every declared (compared) gap is exercised here.
    let differences = differing_classes(&semantic_state(&source), &semantic_state(&rebuilt));
    for (class, instances) in &differences {
        eprintln!("not rebuilt: {class}\n  {}", instances.join("\n  "));
    }
    let expected: BTreeSet<&str> = rebuild::NOT_REBUILT
        .iter()
        .filter(|fact| fact.compared)
        .map(|fact| fact.fact)
        .collect();
    let actual: BTreeSet<&str> = differences.keys().map(String::as_str).collect();
    assert_eq!(actual, expected, "{differences:#?}");

    // The facts the scenarios exist for, stated directly.
    let state = semantic_state(&rebuilt);
    let at = |key: &str| state.get(key).cloned().unwrap_or(Value::Null);
    assert_eq!(at("task.stage|gate"), "in progress");
    assert_eq!(at("run.status|gate-run"), "succeeded");
    assert_eq!(at("task.stage|revision"), "in progress");
    assert_eq!(at("task.stage|post"), "review");
    assert_eq!(at("task.stage|exits"), "review");
    assert_eq!(at("budget|exits/in progress"), 1);
    assert_eq!(at("run.exit|exits-review-1"), "revise");
    assert_eq!(at("blocker|blocked->active"), true);
    assert_eq!(at("run.artifacts|artifact-run"), json!(["diff", "pr"]));
    assert_eq!(at("run.summary|history-run"), "Old work");
    assert_eq!(at("run.exists|active-run"), Value::Null);
}
