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
    let result: Value = serde_json::from_str(&implement.result).unwrap();
    assert_eq!(
        result,
        json!({ "status": "success", "summary": "Implemented\n\nthe parser", "metadata": { "k": 1 } }),
        "a default exit was not named by the session, so it is not in its result"
    );
    assert_eq!(implement.started_at, "2026-09-23 10:01:00");
    assert_eq!(implement.finished_at, "2026-09-23 10:01:00");
    // Legacy free-form results stay free-form.
    assert_eq!(runs[1].1.result, "free-form text");
    // The correction replaced the first verdict; the start stays the first mention.
    let review = runs[2].1;
    assert_eq!(review.status, "failed");
    let result: Value = serde_json::from_str(&review.result).unwrap();
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
    let result: Value = serde_json::from_str(&run.result).unwrap();
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
