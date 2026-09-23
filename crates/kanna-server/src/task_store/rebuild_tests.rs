//! Unit tests for the task-directory reader and the projector. The fixture
//! round trip through real endpoints is `http_api::tests::disk_rebuild`.

use super::rebuild::*;
use crate::db::task_store::{ledger_file_name, render_ledger_entry, LedgerEntryKind};
use crate::db::Db;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const REPO: &str = "repo-1";

fn store_root(label: &str) -> PathBuf {
    let root = crate::test_paths::unique_test_path(&format!("kanna-rebuild-{label}"));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn task_json(task_id: &str, stage: &str) -> Value {
    json!({
        "schema_version": 1,
        "task_id": task_id,
        "repo_id": REPO,
        "title": "Build it",
        "origin_prompt": "Build the thing",
        "workflow": { "name": "flow", "definition": { "name": "flow", "stages": [] } },
        "links": { "parent": null, "dependencies": [], "pr": null },
        "stage": stage,
        "branch": format!("task-{task_id}"),
        "base_ref": "main",
        "owning_machine": null,
        "created_at": "2026-09-23 10:00:00",
        "updated_at": "2026-09-23 11:00:00",
        "closed_at": null,
        "snapshot_revision": 4,
        "ledger": { "published_through": 0 },
    })
}

/// A task directory holding `snapshot` and one file per entry.
struct TaskFiles {
    dir: PathBuf,
    task_id: String,
    next: i64,
}

impl TaskFiles {
    fn new(root: &Path, snapshot: Value) -> Self {
        let task_id = snapshot["task_id"].as_str().unwrap().to_string();
        let dir = crate::task_store::task_dir(root, REPO, &task_id);
        std::fs::create_dir_all(dir.join("ledger")).unwrap();
        std::fs::write(dir.join("task.json"), snapshot.to_string()).unwrap();
        Self {
            dir,
            task_id,
            next: 1,
        }
    }

    /// Write the next entry; `extra` is merged into the envelope.
    fn entry(&mut self, kind: LedgerEntryKind, body: Value, extra: Value, message: Option<&str>) {
        let sequence = self.next;
        self.next += 1;
        let entry_id = crate::db::task_store::ledger_entry_id(&self.task_id, sequence);
        let mut envelope = json!({
            "schema_version": 1,
            "entry_id": entry_id,
            "task_id": self.task_id,
            "sequence": sequence,
            "kind": kind.as_str(),
            "operation_id": format!("op-{entry_id}"),
            "source": { "kind": "test", "id": entry_id, "origin": null },
            "recorded_at": format!("2026-09-23T10:{:02}:00.500Z", sequence),
            "historical": false,
            "run_id": null,
            "session_ref": null,
            "declared_role": null,
            "channel_identity": { "kind": "unknown" },
            "artifacts": {},
            kind.as_str(): body,
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            envelope[key] = value;
        }
        let bytes =
            render_ledger_entry(&envelope, message.or(kind.has_message_body().then_some("")));
        std::fs::write(
            self.dir
                .join("ledger")
                .join(ledger_file_name(sequence, kind)),
            bytes,
        )
        .unwrap();
    }

    fn result(&mut self, run_id: &str, body: Value, message: &str) {
        self.entry(
            LedgerEntryKind::Result,
            body,
            json!({ "run_id": run_id, "session_ref": { "kind": "stage_run", "id": run_id } }),
            Some(message),
        );
    }

    fn transition(&mut self, body: Value) {
        self.entry(LedgerEntryKind::Transition, body, json!({}), None);
    }
}

fn read(dir: &Path) -> TaskDirectory {
    read_task_directory(dir).unwrap()
}

#[test]
fn the_reader_refuses_versions_it_does_not_understand() {
    let root = store_root("versions");
    let mut snapshot = task_json("t-1", "review");
    snapshot["schema_version"] = json!(2);
    let files = TaskFiles::new(&root, snapshot);
    let error = read_task_directory(&files.dir).unwrap_err();
    assert!(error.contains("schema_version 2"), "{error}");

    let mut files = TaskFiles::new(&root, task_json("t-2", "review"));
    files.entry(
        LedgerEntryKind::Transition,
        json!({ "from_stage": "a", "to_stage": "b" }),
        json!({ "schema_version": 7 }),
        None,
    );
    let error = read_task_directory(&files.dir).unwrap_err();
    assert!(error.contains("schema_version 7"), "{error}");
}

#[test]
fn the_reader_refuses_entries_that_disagree_with_their_place() {
    let root = store_root("identity");
    let mut files = TaskFiles::new(&root, task_json("t-1", "review"));
    files.entry(
        LedgerEntryKind::Transition,
        json!({}),
        json!({ "sequence": 9 }),
        None,
    );
    assert!(read_task_directory(&files.dir)
        .unwrap_err()
        .contains("sequence does not match"));

    let mut files = TaskFiles::new(&root, task_json("t-2", "review"));
    files.entry(
        LedgerEntryKind::Transition,
        json!({}),
        json!({ "task_id": "t-other" }),
        None,
    );
    assert!(read_task_directory(&files.dir)
        .unwrap_err()
        .contains("another task"));

    // Two files claiming one sequence.
    let mut files = TaskFiles::new(&root, task_json("t-3", "review"));
    files.transition(json!({}));
    files.next = 1;
    files.entry(LedgerEntryKind::Plan, json!({}), json!({}), None);
    assert!(read_task_directory(&files.dir)
        .unwrap_err()
        .contains("appears twice"));

    // task.json naming a task other than its directory.
    let files = TaskFiles::new(&root, task_json("t-4", "review"));
    std::fs::write(
        files.dir.join("task.json"),
        task_json("t-5", "review").to_string(),
    )
    .unwrap();
    assert!(read_task_directory(&files.dir)
        .unwrap_err()
        .contains("holds task.json for task t-5"));
}

#[test]
fn the_reader_tolerates_additive_fields_and_ignores_temporary_files() {
    let root = store_root("additive");
    let mut snapshot = task_json("t-1", "review");
    snapshot["a_later_field"] = json!({ "added": "by T2" });
    let mut files = TaskFiles::new(&root, snapshot);
    files.entry(
        LedgerEntryKind::Transition,
        json!({ "from_stage": "in progress", "to_stage": "review", "workspace": "w-1" }),
        json!({ "session_ref": { "kind": "workspace_session", "id": "s-1" } }),
        None,
    );
    std::fs::write(
        files.dir.join("ledger/.000002-result.md.tmp-1-0"),
        "half-written",
    )
    .unwrap();
    let directory = read(&files.dir);
    assert_eq!(directory.entries.len(), 1);
    assert_eq!(directory.snapshot.stage.as_deref(), Some("review"));
}

#[test]
fn results_project_one_stage_run_each_and_a_correction_wins() {
    let root = store_root("runs");
    let mut files = TaskFiles::new(&root, task_json("t-1", "review"));
    files.result(
        "run-impl",
        json!({ "status": "success", "stage": "in progress", "run_kind": "main",
                "metadata": { "k": 1 }, "exit": "advance", "exit_source": "default" }),
        "Implemented\n\nthe parser",
    );
    files.result(
        "run-review",
        json!({ "status": "success", "stage": "review", "run_kind": "main", "metadata": null,
                "exit": "revise", "exit_source": "explicit" }),
        "first verdict",
    );
    files.result(
        "run-review",
        json!({ "status": "partial", "stage": "review", "run_kind": "main", "metadata": null }),
        "corrected verdict",
    );
    files.entry(
        LedgerEntryKind::Result,
        json!({ "status": "failure", "stage": "in progress", "run_kind": "main",
                "legacy_format": true }),
        json!({ "run_id": "run-old", "historical": true }),
        Some("free-form text"),
    );
    let projection = project(&[read(&files.dir)]);
    let runs: Vec<_> = projection
        .stage_runs
        .iter()
        .map(|run| (run.id.as_str(), run))
        .collect();
    assert_eq!(
        runs.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        ["run-impl", "run-old", "run-review"]
    );
    let implement = runs[0].1;
    assert_eq!(implement.status, "succeeded");
    assert_eq!(implement.kind, "main");
    assert_eq!(
        implement.feedback.as_deref(),
        Some("Implemented\n\nthe parser")
    );
    let result: Value = serde_json::from_str(implement.result.as_deref().unwrap()).unwrap();
    assert_eq!(
        result,
        json!({ "status": "success", "summary": "Implemented\n\nthe parser", "metadata": { "k": 1 } }),
        "a default exit was not named by the session, so it is not in its result"
    );
    assert_eq!(implement.started_at, "2026-09-23 10:01:00");
    assert_eq!(implement.finished_at, "2026-09-23 10:01:00");
    // Legacy free-form results stay free-form.
    assert_eq!(runs[1].1.result.as_deref(), Some("free-form text"));
    // The correction replaced the first verdict; the start stays the first mention.
    let review = runs[2].1;
    assert_eq!(review.status, "failed");
    let result: Value = serde_json::from_str(review.result.as_deref().unwrap()).unwrap();
    assert_eq!(result["summary"], "corrected verdict");
    assert!(result.get("exit").is_none());
    assert_eq!(review.started_at, "2026-09-23 10:02:00");
    assert_eq!(review.finished_at, "2026-09-23 10:03:00");
}

#[test]
fn an_explicit_exit_and_artifacts_are_part_of_the_projected_result() {
    let root = store_root("artifacts");
    let mut files = TaskFiles::new(&root, task_json("t-1", "review"));
    files.entry(
        LedgerEntryKind::Result,
        json!({ "status": "success", "stage": "review", "run_kind": "main",
                "exit": "revise", "exit_source": "explicit" }),
        json!({
            "run_id": "run-1",
            "declared_role": "agent",
            "channel_identity": { "kind": "localProcess", "evidence": "loopbackSocket", "future": 1 },
            "artifacts": { "diff": { "type": "commit", "repoId": REPO, "sha": "abc" } },
        }),
        Some("look"),
    );
    let projection = project(&[read(&files.dir)]);
    let run = &projection.stage_runs[0];
    let result: Value = serde_json::from_str(run.result.as_deref().unwrap()).unwrap();
    assert_eq!(result["exit"], "revise");
    assert_eq!(result["artifacts"]["diff"]["sha"], "abc");
    assert_eq!(run.result_declared_role.as_deref(), Some("agent"));
    // Written verbatim, so evidence this build does not model is kept.
    let channel: Value =
        serde_json::from_str(run.result_channel_identity.as_deref().unwrap()).unwrap();
    assert_eq!(channel["future"], 1);
}

#[test]
fn inputs_keep_their_row_ids_origin_and_delivery_time() {
    let root = store_root("inputs");
    let mut files = TaskFiles::new(&root, task_json("t-1", "in progress"));
    files.entry(
        LedgerEntryKind::Input,
        json!({ "input_id": 42, "source": "manager", "stage": "in progress",
                "delivered_at": "2026-09-23T10:05:07Z" }),
        json!({
            "run_id": "run-live",
            "source": { "kind": "task_input", "id": "42",
                        "origin": { "peer_id": "peer-a", "task_id": "t-9", "input_id": 3, "run_id": "r-9" } },
        }),
        Some("please also fix the retry"),
    );
    let projection = project(&[read(&files.dir)]);
    let input = &projection.inputs[0];
    assert_eq!(input.id, 42);
    assert_eq!(input.source, "manager");
    assert_eq!(input.delivered_at, "2026-09-23 10:05:07");
    assert_eq!(input.message, "please also fix the retry");
    assert_eq!(input.origin_peer_id.as_deref(), Some("peer-a"));
    assert_eq!(input.origin_input_id, Some(3));
    // The run it went to recorded nothing, so it is known only by reference,
    // and the schema's foreign key forbids keeping a dangling one.
    assert_eq!(input.run_id, None);
    assert!(projection.stage_runs.is_empty());
    assert!(projection.diagnostics.iter().any(
        |note| note.contains("run run-live is named by") && note.contains("recorded no result")
    ));
}

#[test]
fn budgets_replay_spends_and_reset_only_on_a_send_back() {
    let root = store_root("budgets");
    let mut files = TaskFiles::new(&root, task_json("t-1", "review"));
    let loop_result = |spent: i64, exhausted: bool, stage: &str| {
        json!({ "status": "success", "stage": "review", "run_kind": "main",
                "exit": "revise", "exit_source": "explicit",
                "budget": { "stage": stage, "spent": spent, "limit": 2, "exhausted": exhausted } })
    };
    files.result("r-1", loop_result(1, false, "in progress"), "loop one");
    files.transition(json!({ "from_stage": "review", "to_stage": "in progress",
                             "exit": "revise", "exit_source": "explicit",
                             "budget": { "stage": "in progress", "spent": 1, "limit": 2 } }));
    files.result("r-2", loop_result(1, false, "plan"), "replan");
    files.result("r-3", loop_result(2, false, "in progress"), "loop two");
    files.result("r-4", loop_result(2, true, "in progress"), "parked");
    // Operating the gate does not refund anything.
    files.transition(json!({ "from_stage": "review", "to_stage": "plan",
                             "exit": "advance", "exit_source": "operator" }));
    let projection = project(&[read(&files.dir)]);
    let spent = |projection: &Projection| {
        projection
            .budgets
            .iter()
            .map(|budget| (budget.stage.clone(), budget.spent))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        spent(&projection),
        [("in progress".to_string(), 2), ("plan".to_string(), 1)]
    );
    // A person sending it back to `in progress` resets that stage only.
    files.transition(json!({ "from_stage": "plan", "to_stage": "in progress",
                             "exit": "revise", "exit_source": "operator" }));
    let projection = project(&[read(&files.dir)]);
    assert_eq!(spent(&projection), [("plan".to_string(), 1)]);
}

#[test]
fn dependencies_and_links_come_from_task_json() {
    let root = store_root("links");
    let mut snapshot = task_json("t-1", "in progress");
    snapshot["links"] = json!({
        "parent": "t-parent",
        "dependencies": ["t-b", "t-a", "t-b", "t-gone"],
        "pr": { "url": "https://example.test/pr/7", "number": 7, "head_sha": null },
    });
    let files = TaskFiles::new(&root, snapshot);
    let a = TaskFiles::new(&root, task_json("t-a", "pr"));
    let b = TaskFiles::new(&root, task_json("t-b", "pr"));
    let projection = project(&[read(&files.dir), read(&a.dir), read(&b.dir)]);
    // A dependency on a task with no directory is reported, not invented.
    assert!(projection
        .diagnostics
        .iter()
        .any(|note| note.contains("dependency on t-gone")));
    assert_eq!(
        projection.blockers,
        [
            ("t-1".to_string(), "t-a".to_string()),
            ("t-1".to_string(), "t-b".to_string())
        ]
    );
    let task = projection
        .tasks
        .iter()
        .find(|task| task.id == "t-1")
        .unwrap();
    assert_eq!(task.parent_task_id.as_deref(), Some("t-parent"));
    assert_eq!(task.pr_number, Some(7));
    assert_eq!(task.workflow_name.as_deref(), Some("flow"));
    assert_eq!(
        serde_json::from_str::<Value>(task.workflow_definition.as_deref().unwrap()).unwrap(),
        json!({ "name": "flow", "stages": [] })
    );
}

#[test]
fn a_stale_task_json_is_reported_not_corrected() {
    let root = store_root("stale");
    let mut files = TaskFiles::new(&root, task_json("t-1", "in progress"));
    files.transition(json!({ "from_stage": "in progress", "to_stage": "review" }));
    let projection = project(&[read(&files.dir)]);
    assert_eq!(projection.tasks[0].stage.as_deref(), Some("in progress"));
    assert!(projection
        .diagnostics
        .iter()
        .any(|note| note.contains("differs from the last transition")));
    assert!(projection
        .diagnostics
        .iter()
        .any(|note| note.contains("written through sequence 0 but the ledger reaches 1")));
}

#[test]
fn projection_is_independent_of_directory_order() {
    let root = store_root("order");
    let mut a = TaskFiles::new(&root, task_json("t-a", "review"));
    a.result(
        "r-a",
        json!({ "status": "success", "stage": "in progress" }),
        "a",
    );
    let mut b = TaskFiles::new(&root, task_json("t-b", "review"));
    b.result(
        "r-b",
        json!({ "status": "success", "stage": "in progress" }),
        "b",
    );
    let forward = project(&[read(&a.dir), read(&b.dir)]);
    let backward = project(&[read(&b.dir), read(&a.dir)]);
    assert_eq!(forward, backward);
    let (scanned, unreadable) = scan_store(&root).unwrap();
    assert!(unreadable.is_empty());
    assert_eq!(project(&scanned), forward);
}

#[test]
fn scanning_reports_unreadable_directories_instead_of_dropping_them() {
    let root = store_root("unreadable");
    TaskFiles::new(&root, task_json("t-good", "review"));
    let broken = crate::task_store::task_dir(&root, REPO, "t-broken");
    std::fs::create_dir_all(broken.join("ledger")).unwrap();
    let (read, unreadable) = scan_store(&root).unwrap();
    assert_eq!(read.len(), 1);
    assert_eq!(unreadable.len(), 1);
    assert_eq!(unreadable[0].0, broken);
}

#[test]
fn a_rebuild_writes_only_a_new_database_and_reapplying_changes_nothing() {
    let root = store_root("apply");
    let mut files = TaskFiles::new(&root, task_json("t-1", "review"));
    files.result(
        "run-1",
        json!({ "status": "success", "stage": "in progress", "run_kind": "main",
                "budget": { "stage": "in progress", "spent": 1, "limit": 2, "exhausted": false } }),
        "done",
    );
    files.entry(
        LedgerEntryKind::Input,
        json!({ "input_id": 5, "source": "operator", "stage": "review",
                "delivered_at": "2026-09-23T10:09:00Z" }),
        json!({}),
        Some("hello"),
    );
    let target = PathBuf::from(Db::test_db_path("rebuild-apply"));
    let _ = std::fs::remove_file(&target);
    let report = rebuild_into_new_database(&root, &target).unwrap();
    assert_eq!(
        (
            report.tasks,
            report.stage_runs,
            report.inputs,
            report.budgets,
            report.ledger_entries
        ),
        (1, 1, 1, 1, 2)
    );
    let error = rebuild_into_new_database(&root, &target).unwrap_err();
    assert!(error.contains("already exists"), "{error}");

    let db = Db::open(target.to_str().unwrap()).unwrap();
    let dump = |db: &Db| db.disk_rebuild_dump_for_tests();
    let before = dump(&db);
    let (directories, _) = scan_store(&root).unwrap();
    db.apply_disk_projection(&project(&directories)).unwrap();
    assert_eq!(dump(&db), before);
    // The rebuilt outbox continues the sequence rather than reusing it, and
    // owes nothing to publish or backfill.
    assert!(db.ledger_tasks_with_pending_work().unwrap().is_empty());
    assert!(db.tasks_needing_ledger_backfill().unwrap().is_empty());
    assert_eq!(db.ledger_published_through("t-1").unwrap(), 2);
    assert_eq!(db.stage_budget_spent("t-1", "in progress").unwrap(), 1);
}

#[test]
fn a_session_identity_in_session_ref_projects_onto_its_run() {
    let root = store_root("session");
    let mut files = TaskFiles::new(&root, task_json("t-1", "review"));
    let session_ref = json!({
        "kind": "stage_run", "id": "run-1",
        "workspace_id": "ws-1", "branch": "task-t-1-2", "name": "t-1 review",
        "transcript": { "provider": "claude", "session_id": "sess-9", "path": null },
    });
    files.entry(
        LedgerEntryKind::Input,
        json!({ "input_id": 1, "source": "operator", "stage": "review",
                "delivered_at": "2026-09-23T10:01:00Z" }),
        json!({ "run_id": "run-1", "session_ref": session_ref }),
        Some("look again"),
    );
    files.result(
        "run-other",
        json!({ "status": "success", "stage": "in progress" }),
        "T0-shaped reference",
    );
    files.entry(
        LedgerEntryKind::Result,
        json!({ "status": "success", "stage": "review", "run_kind": "main" }),
        json!({ "run_id": "run-1", "session_ref": session_ref }),
        Some("fine"),
    );
    let projection = project(&[read(&files.dir)]);
    let run = projection
        .stage_runs
        .iter()
        .find(|run| run.id == "run-1")
        .unwrap();
    assert_eq!(run.workspace_id.as_deref(), Some("ws-1"));
    assert_eq!(run.session_branch.as_deref(), Some("task-t-1-2"));
    assert_eq!(run.session_name.as_deref(), Some("t-1 review"));
    let transcript: crate::db::TranscriptRef =
        serde_json::from_str(run.transcript_ref.as_deref().unwrap()).unwrap();
    assert_eq!(transcript.session_id, "sess-9");
    // A run from before session identity keeps nothing invented.
    let other = projection
        .stage_runs
        .iter()
        .find(|run| run.id == "run-other")
        .unwrap();
    assert_eq!(
        (
            &other.workspace_id,
            &other.session_branch,
            &other.transcript_ref
        ),
        (&None, &None, &None)
    );
}

fn with_state(mut snapshot: Value, tables: Value) -> Value {
    snapshot["state"] = json!({ "version": 1, "tables": tables });
    snapshot
}

fn task_row(task_id: &str, branch: &str) -> Value {
    json!({ "rowid": 1, "id": task_id, "repo_id": REPO, "stage": "review",
            "pipeline": "flow", "branch": branch, "agent_provider": "claude",
            "created_at": "2026-09-23 10:00:00", "updated_at": "2026-09-23 11:00:00" })
}

#[test]
fn the_reader_refuses_state_it_does_not_understand() {
    let root = store_root("state-versions");
    let cases = [
        (json!({ "version": 2, "tables": {} }), "version Some(2)"),
        (
            json!({ "version": 1, "tables": { "mystery": [] } }),
            "unknown table mystery",
        ),
        (
            json!({ "version": 1, "tables": { "pipeline_item": [{ "rowid": 1, "shiny": 1 }] } }),
            "unknown column shiny",
        ),
        (
            json!({ "version": 1, "tables": { "pipeline_item": [{ "id": "t-1" }] } }),
            "has no rowid",
        ),
    ];
    for (index, (state, expected)) in cases.into_iter().enumerate() {
        let mut snapshot = task_json(&format!("t-{index}"), "review");
        snapshot["state"] = state;
        let files = TaskFiles::new(&root, snapshot);
        let error = read_task_directory(&files.dir).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn state_rows_are_projected_instead_of_what_the_ledger_implies() {
    let root = store_root("state-rows");
    let mut snapshot = with_state(
        task_json("t-1", "review"),
        json!({
            "pipeline_item": [task_row("t-1", "task-t-1")],
            "stage_run": [{ "rowid": 7, "id": "run-live", "task_id": "t-1", "stage": "review",
                            "kind": "main", "status": "running", "completion_bound": 0,
                            "started_at": "2026-09-23 10:00:00" }],
            "task_stage_budget": [{ "rowid": 1, "task_id": "t-1", "stage": "review",
                                    "spent": 3, "updated_at": "2026-09-23 10:00:00" }],
        }),
    );
    // The entry below was published before task.json was written.
    snapshot["ledger"]["published_through"] = json!(1);
    let mut files = TaskFiles::new(&root, snapshot);
    files.result(
        "run-old",
        json!({ "status": "success", "stage": "in progress", "run_kind": "main",
                "budget": { "stage": "review", "spent": 1, "exhausted": false } }),
        "done",
    );
    let projection = project(&[read(&files.dir)]);
    // Runs and budgets are the rows, not a replay of the ledger.
    assert!(projection.stage_runs.is_empty());
    assert!(projection.budgets.is_empty());
    let tables: Vec<(&str, Option<i64>)> = projection
        .carried
        .iter()
        .map(|row| (row.table, row.row.get("rowid").and_then(Value::as_i64)))
        .collect();
    assert_eq!(
        tables,
        vec![
            ("pipeline_item", Some(1)),
            ("stage_run", Some(7)),
            ("task_stage_budget", Some(1)),
        ]
    );
    assert!(projection.tasks[0].from_state);
    assert_eq!(projection.ledger.len(), 1);

    let target = PathBuf::from(Db::test_db_path("rebuild-state"));
    let _ = std::fs::remove_file(&target);
    rebuild_into_new_database(&root, &target).unwrap();
    let db = Db::open(target.to_str().unwrap()).unwrap();
    let run = db.stage_run("run-live").unwrap().unwrap();
    assert_eq!(run.status, "running");
    assert!(db.stage_run("run-old").unwrap().is_none());
    assert_eq!(db.stage_budget_spent("t-1", "review").unwrap(), 3);
    let rowid: i64 = db
        .connection_for_e2e_tests()
        .query_row(
            "SELECT rowid FROM stage_run WHERE id = 'run-live'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rowid, 7);
}

#[test]
fn an_owed_transition_paid_after_task_json_is_not_restored() {
    let root = store_root("state-continuation");
    let continuation = json!({
        "task_ledger_continuation": [{ "rowid": 1, "task_id": "t-1", "operation_id": "op-1",
                                       "kind": "stage_completion", "payload": "{}",
                                       "created_at": "2026-09-23 10:00:00" }],
        "pipeline_item": [task_row("t-1", "task-t-1")],
    });
    // Current: task.json was written through the ledger's last entry.
    let mut snapshot = with_state(task_json("t-1", "review"), continuation.clone());
    snapshot["ledger"]["published_through"] = json!(1);
    let mut files = TaskFiles::new(&root, snapshot);
    files.result(
        "run-1",
        json!({ "status": "success", "stage": "review" }),
        "done",
    );
    let projection = project(&[read(&files.dir)]);
    assert!(projection
        .carried
        .iter()
        .any(|row| row.table == "task_ledger_continuation"));

    // Stale: the transition it owed was recorded after task.json.
    files.transition(json!({ "from_stage": "review", "to_stage": "pr" }));
    let projection = project(&[read(&files.dir)]);
    assert!(!projection
        .carried
        .iter()
        .any(|row| row.table == "task_ledger_continuation"));
    assert!(projection
        .diagnostics
        .iter()
        .any(|note| note.contains("owed transition op-1 is not restored")));
}

/// A completion held by a child join is snapshotted, then an unrelated
/// operator input is published and the process dies before task.json is
/// rewritten: the transition is still owed and must survive the rebuild.
#[test]
fn an_owed_transition_survives_an_unrelated_newer_entry() {
    let root = store_root("state-continuation-unrelated");
    let mut snapshot = with_state(
        task_json("t-1", "review"),
        json!({
            "pipeline_item": [task_row("t-1", "task-t-1")],
            "task_ledger_continuation": [{ "rowid": 1, "task_id": "t-1",
                                           "operation_id": "op-t-1-000001",
                                           "kind": "stage_completion", "payload": "{}",
                                           "created_at": "2026-09-23 10:00:00" }],
        }),
    );
    snapshot["ledger"]["published_through"] = json!(1);
    let mut files = TaskFiles::new(&root, snapshot);
    // The completion that created it (its own operation), then the input.
    files.result(
        "run-1",
        json!({ "status": "success", "stage": "review" }),
        "done",
    );
    files.entry(
        LedgerEntryKind::Input,
        json!({ "input_id": 9, "source": "operator", "stage": "review",
                "delivered_at": "2026-09-23T10:02:00Z" }),
        json!({}),
        Some("while you wait"),
    );
    let projection = project(&[read(&files.dir)]);
    assert!(
        projection
            .carried
            .iter()
            .any(|row| row.table == "task_ledger_continuation"),
        "{:?}",
        projection.diagnostics
    );
    assert!(projection
        .diagnostics
        .iter()
        .any(|note| note.contains("owed transition op-t-1-000001 is restored")));

    // A corrected verdict under another operation replaces it.
    files.result(
        "run-1",
        json!({ "status": "failure", "stage": "review" }),
        "on second thought",
    );
    let projection = project(&[read(&files.dir)]);
    assert!(!projection
        .carried
        .iter()
        .any(|row| row.table == "task_ledger_continuation"));
}

/// The publisher writes a completion (or an engine-observed ending) to the
/// ledger and dies before task.json is rewritten: the ledger's newer facts
/// win over the stale rows.
#[test]
fn ledger_entries_newer_than_task_json_apply_on_top_of_its_state() {
    let root = store_root("state-crash-window");
    let running = |id: &str, rowid: i64| {
        json!({ "rowid": rowid, "id": id, "task_id": "t-1", "stage": "review", "kind": "main",
                "status": "running", "completion_bound": 0,
                "started_at": "2026-09-23 10:00:00", "feedback": "keep me" })
    };
    let mut snapshot = with_state(
        task_json("t-1", "review"),
        json!({
            "pipeline_item": [task_row("t-1", "task-t-1")],
            "stage_run": [running("run-done", 1), running("run-lost", 2)],
            "task_stage_budget": [{ "rowid": 1, "task_id": "t-1", "stage": "in progress",
                                    "spent": 1, "updated_at": "2026-09-23 10:00:00" }],
        }),
    );
    snapshot["ledger"]["published_through"] = json!(0);
    let mut files = TaskFiles::new(&root, snapshot);
    files.entry(
        LedgerEntryKind::Result,
        json!({ "status": "success", "stage": "review", "run_kind": "main",
                "budget": { "stage": "in progress", "spent": 2, "exhausted": false } }),
        json!({ "run_id": "run-done", "declared_role": "agent" }),
        Some("reviewed"),
    );
    files.entry(
        LedgerEntryKind::Result,
        json!({ "status": null, "observed_by": "engine", "stage": "review", "run_kind": "main",
                "ending": { "run_status": "failed",
                            "no_work_termination": "session_interrupted" } }),
        json!({ "run_id": "run-lost" }),
        Some("ended"),
    );
    files.result(
        "run-new",
        json!({ "status": "success", "stage": "pr" }),
        "opened",
    );

    let target = PathBuf::from(Db::test_db_path("rebuild-crash-window"));
    let _ = std::fs::remove_file(&target);
    rebuild_into_new_database(&root, &target).unwrap();
    let db = Db::open(target.to_str().unwrap()).unwrap();
    let done = db.stage_run("run-done").unwrap().unwrap();
    assert_eq!(done.status, "succeeded");
    assert_eq!(done.feedback.as_deref(), Some("reviewed"));
    let result: Value = serde_json::from_str(done.result.as_deref().unwrap()).unwrap();
    assert_eq!(result["summary"], "reviewed");
    let lost = db.stage_run("run-lost").unwrap().unwrap();
    assert_eq!(lost.status, "failed");
    assert_eq!(
        lost.no_work_termination.as_deref(),
        Some("session_interrupted")
    );
    assert_eq!(lost.feedback.as_deref(), Some("keep me"));
    assert_eq!(
        db.stage_run("run-new").unwrap().unwrap().status,
        "succeeded"
    );
    assert_eq!(db.stage_budget_spent("t-1", "in progress").unwrap(), 2);
}

/// Tasks whose task.json predates `state` get rowids SQLite allocates;
/// carried rows keep theirs. The two must never collide, whatever order the
/// task ids sort in, and a re-application must still change nothing.
#[test]
fn legacy_and_state_tasks_with_colliding_rowids_both_rebuild() {
    let root = store_root("state-rowids");
    // "a-legacy" sorts first and has no `state`.
    let mut legacy = TaskFiles::new(&root, task_json("a-legacy", "review"));
    legacy.result(
        "legacy-run",
        json!({ "status": "success", "stage": "review" }),
        "old",
    );
    let snapshot = with_state(
        task_json("b-state", "review"),
        json!({
            "pipeline_item": [task_row("b-state", "task-b-state")],
            "stage_run": [{ "rowid": 1, "id": "state-run", "task_id": "b-state",
                            "stage": "review", "kind": "main", "status": "running",
                            "completion_bound": 0, "started_at": "2026-09-23 10:00:00" }],
            "task_stage_budget": [{ "rowid": 1, "task_id": "b-state", "stage": "review",
                                    "spent": 1, "updated_at": "2026-09-23 10:00:00" }],
        }),
    );
    let mut snapshot = snapshot;
    snapshot["task_id"] = json!("b-state");
    TaskFiles::new(&root, snapshot);

    let target = PathBuf::from(Db::test_db_path("rebuild-rowids"));
    let _ = std::fs::remove_file(&target);
    let report = rebuild_into_new_database(&root, &target).unwrap();
    assert_eq!(report.tasks, 2);
    let db = Db::open(target.to_str().unwrap()).unwrap();
    let rowid = |table: &str, id: &str| -> i64 {
        db.connection_for_e2e_tests()
            .query_row(
                &format!("SELECT rowid FROM {table} WHERE id = ?"),
                [id],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(rowid("pipeline_item", "b-state"), 1);
    assert_ne!(rowid("pipeline_item", "a-legacy"), 1);
    assert_eq!(rowid("stage_run", "state-run"), 1);
    assert!(db.stage_run("legacy-run").unwrap().is_some());
    assert_eq!(db.stage_budget_spent("b-state", "review").unwrap(), 1);

    let before = db.disk_rebuild_dump_for_tests();
    let scan = scan_store_records(&root).unwrap();
    db.apply_disk_projection(&project_store(&scan)).unwrap();
    assert_eq!(db.disk_rebuild_dump_for_tests(), before);
}

/// A carried row whose rowid holds a different row is a collision, refused
/// rather than skipped as if it were already applied.
#[test]
fn a_carried_row_colliding_with_another_row_refuses_the_rebuild() {
    let root = store_root("state-collision");
    TaskFiles::new(
        &root,
        with_state(
            task_json("t-1", "review"),
            json!({ "pipeline_item": [task_row("t-1", "task-t-1")] }),
        ),
    );
    let target = PathBuf::from(Db::test_db_path("rebuild-collision"));
    let _ = std::fs::remove_file(&target);
    rebuild_into_new_database(&root, &target).unwrap();
    let db = Db::open(target.to_str().unwrap()).unwrap();
    let scan = scan_store_records(&root).unwrap();
    let mut projection = project_store(&scan);
    let task = projection
        .carried
        .iter_mut()
        .find(|row| row.table == "pipeline_item")
        .unwrap();
    task.row.insert("id".into(), json!("t-other"));
    let error = db.apply_disk_projection(&projection).unwrap_err();
    assert!(error.to_string().contains("collides"), "{error}");
}

#[test]
fn the_branch_counter_is_never_below_a_recorded_branch() {
    let root = store_root("state-counter");
    let snapshot = with_state(
        task_json("t-1", "review"),
        json!({
            "pipeline_item": [task_row("t-1", "task-t-1-2")],
            "task_branch_counter": [{ "rowid": 1, "task_id": "t-1", "last_allocated": 1,
                                      "updated_at": "2026-09-23 10:00:00" }],
        }),
    );
    let mut files = TaskFiles::new(&root, snapshot);
    files.result(
        "run-1",
        json!({ "status": "success", "stage": "review", "branch": "task-t-1-4" }),
        "done",
    );
    let projection = project(&[read(&files.dir)]);
    let counter = projection
        .carried
        .iter()
        .find(|row| row.table == "task_branch_counter")
        .unwrap();
    assert_eq!(counter.row["last_allocated"], 4);
}

#[test]
fn tombstones_and_repository_records_are_read() {
    let root = store_root("state-repos");
    TaskFiles::new(&root, task_json("t-live", "review"));
    let removed = TaskFiles::new(&root, task_json("t-gone", "review"));
    std::fs::write(
        removed.dir.join("task.json"),
        json!({ "schema_version": 1, "task_id": "t-gone", "repo_id": REPO, "removed": true })
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("repos").join(REPO).join("repo.json"),
        json!({
            "schema_version": 1, "repo_id": REPO, "snapshot_revision": 3, "sidebar_order": 4,
            "registration": { "id": REPO, "path": "/work/repo", "name": "Repo",
                              "default_branch": "main", "remote_url_hash": "h",
                              "sort_order": 0, "created_at": "c", "last_opened_at": "o",
                              "hidden": 0 },
        })
        .to_string(),
    )
    .unwrap();
    let scan = scan_store_records(&root).unwrap();
    assert_eq!(scan.tasks.len(), 1);
    assert_eq!(scan.removed, vec![removed.dir.clone()]);
    assert_eq!(scan.repos.len(), 1);

    let target = PathBuf::from(Db::test_db_path("rebuild-repos"));
    let _ = std::fs::remove_file(&target);
    let report = rebuild_into_new_database(&root, &target).unwrap();
    assert_eq!(report.tasks, 1);
    let db = Db::open(target.to_str().unwrap()).unwrap();
    let repo = db.get_repo(REPO).unwrap().unwrap();
    assert_eq!(repo.path, "/work/repo");
    assert!(db.get_pipeline_item("t-gone").unwrap().is_none());
    assert!(db.repos_with_pending_disk_record().unwrap().is_empty());
}

/// The rows task.json carries can already reflect entries that are not
/// published yet (committed behind a reservation, or after the publisher
/// read its pending list). Here the engine records a lost session's ending,
/// the daemon proves the session alive and the run is restored, and a
/// transition is owed — all while publication is held at a reservation, so
/// task.json is written with state newer than its publication watermark.
/// The next flush publishes the ending but leaves task.json as it was.
/// The rebuild must neither replay the ending over the restored run nor
/// take anything it already reflects as payment for the owed transition.
#[test]
fn entries_the_state_already_reflects_are_not_replayed() {
    let path = Db::test_db_path("rebuild-reflected");
    let _ = std::fs::remove_file(&path);
    let db = Db::open_migrated(&path).unwrap();
    let root = super::root_for_db(&path);
    db.insert_test_repo_with_path(REPO, "/tmp/repo-one", "Repo One")
        .unwrap();
    db.insert_test_pipeline_item(
        "t-1",
        REPO,
        "Prompt",
        Some("t-1"),
        "review",
        "2026-09-23 00:00:00",
    )
    .unwrap();
    db.mark_task_ledger_backfilled("t-1", 0).unwrap();
    fn run(id: &str) -> crate::db::NewStageRun<'_> {
        crate::db::NewStageRun {
            id,
            task_id: "t-1",
            stage: "review",
            kind: "main",
            agent: Some("review"),
            agent_provider: Some("claude"),
            model: None,
            effort: None,
            status: "running",
            result: None,
            feedback: None,
            session_id: Some("t-1"),
            provider_session_id: None,
            cwd: None,
            resumed_from_run_id: None,
        }
    }
    // An earlier run with a verdict under its own operation, published.
    db.insert_stage_run(run("run-0")).unwrap();
    db.finish_stage_run(
        "run-0",
        "succeeded",
        Some(r#"{"status":"success","summary":"ok","metadata":null}"#),
        None,
    )
    .unwrap();
    db.enqueue_ledger_entry(crate::db::task_store::NewLedgerEntry {
        task_id: "t-1",
        kind: LedgerEntryKind::Result,
        operation_id: Some("op-verdict-0"),
        source_kind: "stage_run",
        source_id: "run-0",
        source_origin: None,
        historical: false,
        recorded_at: None,
        run_id: Some("run-0"),
        declared_role: Some("agent"),
        channel_identity: &crate::mutation_provenance::ChannelIdentity::Server,
        body: json!({ "status": "success", "stage": "review", "run_kind": "main" }),
        message: Some("ok"),
        hold_events_after: None,
        reserved_sequence: None,
    })
    .unwrap();
    db.insert_stage_run(run("run-1")).unwrap();
    super::flush_task(&db, &path, "t-1").unwrap();

    // Publication is held at a reservation while the mutations commit.
    let reserved = db.reserve_ledger_sequence("t-1").unwrap();
    db.finish_latest_running_stage_run("t-1", "failed", None, Some("lost"))
        .unwrap()
        .unwrap();
    assert!(db
        .restore_latest_interrupted_stage_run("t-1", "lost")
        .unwrap());
    db.put_ledger_continuation(
        "t-1",
        "op-owed",
        "stage_completion",
        &json!({ "runId": "run-1" }),
    )
    .unwrap();
    let outcome = super::flush_task(&db, &path, "t-1").unwrap();
    assert!(outcome.waiting_on_reservation);
    let dir = super::task_dir(&root, REPO, "t-1");
    let written: Value =
        serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
    assert_eq!(written["ledger"]["published_through"], json!(reserved - 1));
    assert_eq!(written["state"]["reflects_through"], json!(reserved + 1));
    assert_eq!(
        written["state"]["unreflected_reservations"],
        json!([reserved])
    );

    // The reservation is given up; the next flush publishes the ending.
    // Nothing it publishes owes a new task.json, so the one on disk keeps
    // the older watermark with state that already reflects the ending, as a
    // crash before the rewrite would leave it too.
    db.release_ledger_reservation("t-1", reserved).unwrap();
    super::flush_task(&db, &path, "t-1").unwrap();
    let after: Value =
        serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
    assert_eq!(after, written);
    assert!(dir
        .join("ledger")
        .join(ledger_file_name(reserved + 1, LedgerEntryKind::Result))
        .exists());

    let target = PathBuf::from(Db::test_db_path("rebuild-reflected-target"));
    let _ = std::fs::remove_file(&target);
    let report = rebuild_into_new_database(&root, &target).unwrap();
    let rebuilt = Db::open(target.to_str().unwrap()).unwrap();
    let restored = rebuilt.stage_run("run-1").unwrap().unwrap();
    assert_eq!(restored.status, "running", "{:?}", report.diagnostics);
    assert_eq!(restored.no_work_termination, None);
    assert_eq!(
        rebuilt.stage_run("run-0").unwrap().unwrap().status,
        "succeeded"
    );
    assert!(rebuilt.has_ledger_continuation("t-1").unwrap());
    let continuations: i64 = rebuilt
        .connection_for_e2e_tests()
        .query_row("SELECT COUNT(*) FROM task_ledger_continuation", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(continuations, 1);
    // The ledger itself is complete in the rebuilt outbox.
    assert_eq!(
        rebuilt.ledger_published_through("t-1").unwrap(),
        reserved + 1
    );
}

/// A `state` from before `reflects_through` existed falls back to the
/// publication watermark; with the field, an entry at or below it is not
/// replayed and does not pay a carried continuation.
#[test]
fn the_state_boundary_decides_which_entries_are_newer() {
    let root = store_root("state-boundary");
    let tables = json!({
        "pipeline_item": [task_row("t-1", "task-t-1")],
        "stage_run": [{ "rowid": 1, "id": "run-1", "task_id": "t-1", "stage": "review",
                        "kind": "main", "status": "running", "completion_bound": 0,
                        "started_at": "2026-09-23 10:00:00" }],
        "task_ledger_continuation": [{ "rowid": 1, "task_id": "t-1", "operation_id": "op-owed",
                                       "kind": "stage_completion", "payload": "{}",
                                       "created_at": "2026-09-23 10:00:00" }],
    });
    let mut snapshot = with_state(task_json("t-1", "review"), tables);
    snapshot["state"]["reflects_through"] = json!(1);
    let mut files = TaskFiles::new(&root, snapshot);
    files.result(
        "run-1",
        json!({ "status": "failure", "stage": "review" }),
        "reflected",
    );
    let projection = project(&[read(&files.dir)]);
    let run = projection
        .carried
        .iter()
        .find(|row| row.table == "stage_run")
        .unwrap();
    assert_eq!(run.row["status"], "running");
    assert!(projection
        .carried
        .iter()
        .any(|row| row.table == "task_ledger_continuation"));

    // Without the field the watermark (0) decides, as before.
    let mut directory = read(&files.dir);
    directory.snapshot.state_reflects_through = None;
    let projection = project(&[directory]);
    let run = projection
        .carried
        .iter()
        .find(|row| row.table == "stage_run")
        .unwrap();
    assert_eq!(run.row["status"], "failed");
    assert!(!projection
        .carried
        .iter()
        .any(|row| row.table == "task_ledger_continuation"));
}
