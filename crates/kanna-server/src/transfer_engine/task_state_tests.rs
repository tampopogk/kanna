use super::*;
use crate::db::task_store::LedgerEntryKind;
use crate::db::Db;
use crate::mutation_provenance::ChannelIdentity;
use serde_json::json;
use std::process::Command;

const ARTIFACT: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn database(label: &str, repo_id: &str) -> (Db, String) {
    let path = Db::test_db_path(label);
    let db = Db::open_for_tests(&path).unwrap();
    db.insert_test_repo(repo_id, repo_id).unwrap();
    (db, path)
}

fn task(db: &Db, repo_id: &str, id: &str, stage: &str) {
    db.insert_test_pipeline_item(id, repo_id, id, Some(id), stage, "2026-09-23 00:00:00")
        .unwrap();
}

fn entry(
    db: &Db,
    task_id: &str,
    kind: LedgerEntryKind,
    source_id: &str,
    body: Value,
    message: Option<&str>,
    artifacts: Option<Value>,
) -> String {
    db.enqueue_ledger_entry_with_artifacts(
        NewLedgerEntry {
            task_id,
            kind,
            operation_id: None,
            source_kind: "test",
            source_id,
            source_origin: None,
            historical: false,
            recorded_at: None,
            run_id: None,
            declared_role: Some("agent"),
            channel_identity: &ChannelIdentity::Server,
            body,
            message,
            hold_events_after: None,
            reserved_sequence: None,
        },
        artifacts.as_ref(),
    )
    .unwrap()
    .entry_id
}

/// A source task in `review` whose ledger holds: a build result, the
/// transition it caused, a review input, and the review result that caused
/// the current session (with a stored artifact).
fn source_task(db: &Db, path: &str) -> (String, String) {
    task(db, "repo-src", "task-src", "review");
    let build = entry(
        db,
        "task-src",
        LedgerEntryKind::Result,
        "run-build",
        json!({"status": "success", "stage": "build", "committed_sha": "a".repeat(40)}),
        Some("built it"),
        None,
    );
    entry(
        db,
        "task-src",
        LedgerEntryKind::Transition,
        "transition-1",
        json!({"from_stage": "build", "to_stage": "review", "trigger": "auto",
               "triggering_result_id": build}),
        None,
        None,
    );
    entry(
        db,
        "task-src",
        LedgerEntryKind::Input,
        "input-1",
        json!({"input_id": 1, "source": "operator", "stage": "review"}),
        Some("please look at the header"),
        None,
    );
    let review = entry(
        db,
        "task-src",
        LedgerEntryKind::Result,
        "run-review",
        json!({"status": "needs_revision", "stage": "review",
               "budget": {"stage": "review", "spent": 1, "limit": 3}}),
        Some("the header overlaps the logo"),
        Some(json!({"mockup": {"type": "stored", "repoId": "repo-src",
                               "artifactId": ARTIFACT, "kind": "document"}})),
    );
    db.reserve_task_branch_number("task-src", 2).unwrap();
    db.execute_test_sql(
        "INSERT INTO task_stage_budget (task_id, stage, spent) VALUES ('task-src', 'review', 1);",
    )
    .unwrap();
    crate::task_store::flush_task(db, path, "task-src").unwrap();
    (build, review)
}

fn carried(db: &Db, path: &str) -> (TaskStateDocument, TransferTaskStatePayload, Vec<u8>) {
    let item = db.get_pipeline_item("task-src").unwrap().unwrap();
    let document = collect(db, path, &item, "peer-src").unwrap();
    let bytes = encode(&document).unwrap();
    let metadata = TransferTaskStatePayload {
        version: crate::transfer_engine::payload::TASK_STATE_VERSION,
        artifact_id: "transfer-9-task-state".into(),
        filename: crate::transfer_engine::payload::TASK_STATE_FILENAME.into(),
        sha256: crate::transfer_engine::payload::sha256_hex(&bytes),
        history_bundle: None,
        artifact_bundle: None,
    };
    (document, metadata, bytes)
}

fn import_into(
    db: &Db,
    path: &str,
    task_id: &str,
    transfer_id: &str,
    document: TaskStateDocument,
    sha256: String,
    session_start: SessionStart,
) -> (ImportedTaskState, HashMap<String, String>) {
    let state = ImportedTaskState {
        transfer_id: transfer_id.into(),
        sha256,
        document,
        destination_repo_id: "repo-dst".into(),
        session_start,
    };
    let ids = record_ledger(db, task_id, &state).unwrap();
    assert!(trigger_matches_prediction(&state, task_id, &ids));
    record_import(db, task_id, Some(&format!("task-{task_id}")), &state, &ids).unwrap();
    let dir = task_store::task_dir_for(db, path, task_id).unwrap();
    archive(&dir, &state).unwrap();
    task_store::flush_task(db, path, task_id).unwrap();
    verify(db, path, task_id, &state).unwrap();
    (state, ids)
}

#[test]
fn the_ledger_crosses_under_the_new_task_id_and_a_fresh_session_is_told_its_trigger() {
    let (source, source_path) = database("t9-ledger-source", "repo-src");
    let (build, review) = source_task(&source, &source_path);
    let (document, metadata, bytes) = carried(&source, &source_path);
    assert_eq!(document.ledger.len(), 4);
    assert_eq!(document.rows.ownership_generation, 1);
    assert_eq!(
        document.artifacts,
        vec![CarriedArtifact {
            artifact_id: ARTIFACT.into(),
            kind: "document".into()
        }]
    );

    // The document is bound to its payload and to its task.
    let decoded = decode(&bytes, &metadata, "peer-src", "task-src").unwrap();
    assert_eq!(decoded, document);
    let mut tampered = metadata.clone();
    tampered.sha256 = "0".repeat(64);
    assert!(decode(&bytes, &tampered, "peer-src", "task-src").is_err());
    assert!(decode(&bytes, &metadata, "peer-src", "task-other").is_err());

    let (destination, destination_path) = database("t9-ledger-destination", "repo-dst");
    task(&destination, "repo-dst", "task-dst", "review");
    let fresh = SessionStart::Fresh("the source shipped no transcript".into());
    let predicted = predicted_trigger(
        &ImportedTaskState {
            transfer_id: "transfer-9".into(),
            sha256: metadata.sha256.clone(),
            document: decoded.clone(),
            destination_repo_id: "repo-dst".into(),
            session_start: fresh.clone(),
        },
        "task-dst",
    )
    .unwrap()
    .unwrap();
    let (state, ids) = import_into(
        &destination,
        &destination_path,
        "task-dst",
        "transfer-9",
        decoded,
        metadata.sha256.clone(),
        fresh,
    );
    assert_eq!(ids[&build], "task-dst-000001");
    assert_eq!(ids[&review], "task-dst-000003");
    assert_eq!(predicted.entry_id, "task-dst-000003");
    assert_eq!(predicted.message, "the header overlaps the logo");

    let dir = task_store::task_dir_for(&destination, &destination_path, "task-dst").unwrap();
    let files = task_store::read_ledger(&dir).unwrap();
    // Result, transition, result mirrored (the input crosses in the input
    // ledger), then the import's own transition.
    assert_eq!(files.len(), 4);
    assert!(files
        .iter()
        .all(|file| file.envelope["task_id"] == "task-dst"));
    assert_eq!(files[1].body()["triggering_result_id"], "task-dst-000001");
    assert_eq!(
        files[2].envelope["source"]["origin"]["ledger_entry"],
        review
    );
    assert_eq!(files[2].envelope["source"]["origin"]["peer_id"], "peer-src");
    assert_eq!(files[2].envelope["historical"], true);
    assert_eq!(
        files[2].envelope["channel_identity"],
        json!({"kind": "unknown"})
    );
    assert_eq!(
        files[2].envelope["artifacts"]["mockup"]["repoId"],
        "repo-dst"
    );
    let import = &files[3];
    assert_eq!(import.kind, LedgerEntryKind::Transition);
    assert_eq!(import.body()["operation"], "transfer_import");
    assert_eq!(import.body()["triggering_result_id"], "task-dst-000003");
    assert_eq!(import.body()["transfer"]["session"], "fresh");
    assert_eq!(
        import.body()["transfer"]["fresh_start_reason"],
        "the source shipped no transcript"
    );
    assert_eq!(import.body()["transfer"]["ownership_generation"], 1);

    // A new session of the stage reads the same result the source session was
    // caused by, from the destination's own ledger.
    let trigger = task_store::resolve_trigger(&dir, "review").unwrap();
    assert_eq!(trigger.entry_id, "task-dst-000003");
    assert_eq!(trigger.message, "the header overlaps the logo");

    // The source's bytes are kept verbatim.
    for file in &state.document.ledger {
        let kept = std::fs::read_to_string(
            archive_dir(&dir, "transfer-9")
                .join("ledger")
                .join(&file.file_name),
        )
        .unwrap();
        assert_eq!(kept, file.content);
    }

    // A retried import records nothing twice.
    let again = record_ledger(&destination, "task-dst", &state).unwrap();
    assert_eq!(again, ids);
    record_import(
        &destination,
        "task-dst",
        Some("task-task-dst"),
        &state,
        &ids,
    )
    .unwrap();
    task_store::flush_task(&destination, &destination_path, "task-dst").unwrap();
    assert_eq!(task_store::read_ledger(&dir).unwrap().len(), 4);

    let rows = destination
        .export_carried_task_rows("task-dst", "peer-dst")
        .unwrap();
    assert_eq!(rows.branch_counter, Some(3));
    assert_eq!(rows.stage_budgets, state.document.rows.stage_budgets);
}

#[test]
fn a_second_hop_keeps_every_entrys_first_origin_and_counts_ownership() {
    let (source, source_path) = database("t9-hop-source", "repo-src");
    let (_, review) = source_task(&source, &source_path);
    let (document, metadata, _) = carried(&source, &source_path);
    let (middle, middle_path) = database("t9-hop-middle", "repo-dst");
    task(&middle, "repo-dst", "task-mid", "review");
    import_into(
        &middle,
        &middle_path,
        "task-mid",
        "transfer-1",
        document,
        metadata.sha256,
        SessionStart::Resumed,
    );

    let item = middle.get_pipeline_item("task-mid").unwrap().unwrap();
    let second = collect(&middle, &middle_path, &item, "peer-mid").unwrap();
    assert_eq!(second.rows.ownership_generation, 2);
    let bytes = encode(&second).unwrap();
    let (last, last_path) = database("t9-hop-last", "repo-dst");
    task(&last, "repo-dst", "task-last", "review");
    let (_, ids) = import_into(
        &last,
        &last_path,
        "task-last",
        "transfer-2",
        second,
        crate::transfer_engine::payload::sha256_hex(&bytes),
        SessionStart::Resumed,
    );
    let dir = task_store::task_dir_for(&last, &last_path, "task-last").unwrap();
    let files = task_store::read_ledger(&dir).unwrap();
    // The review result names where it was first recorded, not the middle hop.
    let mirrored_review = files
        .iter()
        .find(|file| file.message.as_deref() == Some("the header overlaps the logo"))
        .unwrap();
    assert_eq!(
        mirrored_review.envelope["source"]["origin"]["ledger_entry"],
        review
    );
    // The middle hop's own import transition crossed too, as history.
    assert!(files
        .iter()
        .any(|file| file.body()["operation"] == "transfer_import"
            && file.body()["transfer"]["transfer_id"] == "transfer-1"));
    assert_eq!(ids.len(), 4);
    assert_eq!(
        last.transferred_task_state("task-last")
            .unwrap()
            .unwrap()
            .ownership_generation,
        2
    );
}

#[test]
fn a_task_with_no_ledger_and_no_rows_needs_no_carry() {
    let (db, _) = database("t9-legacy-task", "repo-src");
    task(&db, "repo-src", "task-legacy", "build");
    assert!(!task_requires_carry(&db, "task-legacy").unwrap());
    db.reserve_task_branch_number("task-legacy", 1).unwrap();
    assert!(task_requires_carry(&db, "task-legacy").unwrap());
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn scratch(label: &str) -> PathBuf {
    let root = crate::test_paths::unique_test_dir(label);
    std::fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn history_commits_off_the_task_head_cross_in_their_own_bundle() {
    let root = scratch("t9-history-bundle");
    let source = root.join("source");
    std::fs::create_dir_all(&source).unwrap();
    git(&source, &["init", "--quiet", "-b", "main"]);
    git(
        &source,
        &["commit", "--quiet", "--allow-empty", "-m", "base"],
    );
    // A review stage forked `task-x-2` and committed there; the task head
    // later moved on from the base without it.
    git(&source, &["checkout", "--quiet", "-b", "task-x-2"]);
    git(
        &source,
        &["commit", "--quiet", "--allow-empty", "-m", "review fix"],
    );
    let side = git(&source, &["rev-parse", "HEAD"]);
    git(&source, &["checkout", "--quiet", "main"]);
    git(
        &source,
        &["commit", "--quiet", "--allow-empty", "-m", "head"],
    );
    let head = git(&source, &["rev-parse", "HEAD"]);

    let mut document = TaskStateDocument {
        version: 1,
        source_peer_id: "peer-src".into(),
        source_task_id: "x".into(),
        source_repo_id: "repo".into(),
        stage: "review".into(),
        task_json: None,
        ledger: vec![CarriedLedgerFile {
            file_name: "000001-result.md".into(),
            content: format!(
                "---\n{}\n---\n\nfixed",
                json!({"entry_id": "x-000001", "task_id": "x", "kind": "result",
                       "result": {"status": "success", "committed_sha": side}})
            ),
        }],
        rows: Default::default(),
        artifacts: Vec::new(),
        history_refs: Vec::new(),
    };
    document.rows.links.stage_workspaces.push(
        crate::db::transfer_task_state::CarriedStageWorkspace {
            id: "ws-task-x-2".into(),
            stage: "review".into(),
            path: "/src/task-x-2".into(),
            branch: "task-x-2".into(),
            origin_peer_id: "peer-src".into(),
            origin_task_id: "x".into(),
        },
    );
    let bundle = root.join("history.bundle");
    assert!(stage_history_bundle(&source, &mut document, &head, "t-1", &bundle).unwrap());
    assert_eq!(
        document.history_refs,
        vec![
            CarriedRef {
                name: format!("commits/{side}"),
                oid: side.clone()
            },
            CarriedRef {
                name: "branches/task-x-2".into(),
                oid: side.clone()
            },
        ]
    );
    // Nothing is left behind in the source repository.
    assert_eq!(git(&source, &["for-each-ref", "refs/kanna/"]), "");

    // The destination has the task head (from the repository bundle), not the
    // side commit.
    let destination = root.join("destination");
    git(
        &root,
        &[
            "clone",
            "--quiet",
            "--no-local",
            "--single-branch",
            "-b",
            "main",
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
        ],
    );
    assert!(Command::new("git")
        .args(["cat-file", "-e", &format!("{side}^{{commit}}")])
        .current_dir(&destination)
        .status()
        .map(|status| !status.success())
        .unwrap());
    import_history_refs(&destination, "t-1", &document, Some(&bundle)).unwrap();
    assert_eq!(
        git(
            &destination,
            &[
                "rev-parse",
                &format!("refs/kanna/transfers/t-1/history/commits/{side}")
            ]
        ),
        side
    );
    assert_eq!(
        git(
            &destination,
            &[
                "rev-parse",
                "refs/kanna/transfers/t-1/history/branches/task-x-2"
            ]
        ),
        side
    );
    // Idempotent, and a commit already under the head needs no bundle.
    import_history_refs(&destination, "t-1", &document, Some(&bundle)).unwrap();
    let mut on_head = document.clone();
    on_head.ledger[0].content = on_head.ledger[0].content.replace(&side, &head);
    on_head.rows.links.stage_workspaces.clear();
    assert!(!stage_history_bundle(
        &source,
        &mut on_head,
        &head,
        "t-2",
        &root.join("none.bundle")
    )
    .unwrap());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn referenced_artifacts_cross_with_their_records_and_arrive_retained() {
    use crate::artifacts::store::{ArtifactStore, PublishLimits, PublishRequest};
    use crate::artifacts::types::{ArtifactContentKind, ArtifactRetention};

    let root = scratch("t9-artifact-bundle");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(workspace.join("mockup")).unwrap();
    std::fs::write(workspace.join("mockup/index.html"), "<h1>header</h1>").unwrap();
    let source_store_path = root.join("source/artifacts.git");
    let source_store = ArtifactStore::open_or_create(&source_store_path, "repo-src").unwrap();
    let published = source_store
        .publish(PublishRequest {
            task_id: "task-src",
            workspace_root: &workspace,
            source_path: "mockup",
            kind: ArtifactContentKind::Document,
            entrypoint: None,
            previous: None,
            retention: ArtifactRetention::ThirtyDays,
            limits: PublishLimits::default(),
        })
        .unwrap();
    let artifacts = vec![CarriedArtifact {
        artifact_id: published.artifact_id.clone(),
        kind: "document".into(),
    }];
    let bundle = root.join("artifacts.bundle");
    stage_artifact_bundle(
        &source_store_path,
        &root,
        "repo-src",
        &artifacts,
        &root.join("source-scratch.git"),
        &bundle,
    )
    .unwrap();
    assert!(!root.join("source-scratch.git").exists());

    let destination_store_path = root.join("destination/artifacts.git");
    assert!(verify_artifacts(&destination_store_path, "repo-dst", &artifacts).is_err());
    import_artifact_bundle(
        &destination_store_path,
        &root,
        "repo-dst",
        &artifacts,
        &bundle,
        &root.join("destination-scratch.git"),
    )
    .unwrap();
    verify_artifacts(&destination_store_path, "repo-dst", &artifacts).unwrap();
    let detail = ArtifactStore::open_existing(&destination_store_path, "repo-dst")
        .unwrap()
        .unwrap()
        .detail(&published.artifact_id)
        .unwrap();
    assert!(detail.retained);
    assert_eq!(
        detail.versions.len(),
        1,
        "the version record crossed with it"
    );

    // An artifact the source no longer has fails the transfer at the source.
    let missing = vec![CarriedArtifact {
        artifact_id: ARTIFACT.into(),
        kind: "document".into(),
    }];
    assert!(stage_artifact_bundle(
        &source_store_path,
        &root,
        "repo-src",
        &missing,
        &root.join("source-scratch.git"),
        &root.join("missing.bundle"),
    )
    .is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// What the ledger alone rebuilds: the directory read as a reader of a
/// task.json without `state` (T13) reads it. With `state`, the rows are
/// copied as the destination holds them ([`carried_rows_rebuild`]).
fn projected(db: &Db, path: &str, task_id: &str) -> crate::task_store::rebuild::Projection {
    let dir = task_store::task_dir_for(db, path, task_id).unwrap();
    let mut directory = crate::task_store::rebuild::read_task_directory(&dir).unwrap();
    carried_rows_rebuild(&directory, task_id);
    directory.snapshot.state = None;
    crate::task_store::rebuild::project(&[directory])
}

/// With `state`, a transferred task rebuilds its carried budget as a row,
/// and no local run for a result that ran on another machine.
fn carried_rows_rebuild(directory: &crate::task_store::rebuild::TaskDirectory, task_id: &str) {
    let projection = crate::task_store::rebuild::project(std::slice::from_ref(directory));
    assert!(projection
        .carried
        .iter()
        .any(|row| row.table == "task_stage_budget"
            && row.row["task_id"] == task_id
            && row.row["stage"] == "review"
            && row.row["spent"] == 1));
    assert!(
        !projection.carried.iter().any(|row| row.table == "stage_run"
            && row.row["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("carried:")))
    );
}

/// Rebuilding a transferred task directory from disk projects every carried
/// result and the budget it recorded — after one hop and after a second.
#[test]
fn a_transferred_directory_rebuilds_its_carried_results_and_budgets() {
    let (source, source_path) = database("t9-rebuild-source", "repo-src");
    let (build, review) = source_task(&source, &source_path);
    let (document, metadata, _) = carried(&source, &source_path);
    let (middle, middle_path) = database("t9-rebuild-middle", "repo-dst");
    task(&middle, "repo-dst", "task-mid", "review");
    import_into(
        &middle,
        &middle_path,
        "task-mid",
        "transfer-1",
        document,
        metadata.sha256,
        SessionStart::Resumed,
    );

    let check = |projection: &crate::task_store::rebuild::Projection, task_id: &str| {
        let run = |origin: &str| {
            projection
                .stage_runs
                .iter()
                .find(|run| {
                    run.id == crate::db::transfer_task_state::carried_run_id(task_id, origin)
                })
                .unwrap_or_else(|| panic!("no run for {origin}: {:?}", projection.diagnostics))
        };
        assert_eq!(run(&build).stage, "build");
        assert_eq!(run(&build).status, "succeeded");
        assert_eq!(run(&review).stage, "review");
        assert_eq!(run(&review).status, "failed");
        assert_eq!(
            run(&review).feedback.as_deref(),
            Some("the header overlaps the logo")
        );
        assert!(
            projection
                .budgets
                .iter()
                .any(|budget| budget.task_id == task_id
                    && budget.stage == "review"
                    && budget.spent == 1),
            "{:?}",
            projection.budgets
        );
        assert!(
            !projection
                .diagnostics
                .iter()
                .any(|note| note.contains("names no run")),
            "{:?}",
            projection.diagnostics
        );
    };
    check(&projected(&middle, &middle_path, "task-mid"), "task-mid");

    let item = middle.get_pipeline_item("task-mid").unwrap().unwrap();
    let second = collect(&middle, &middle_path, &item, "peer-mid").unwrap();
    let bytes = encode(&second).unwrap();
    let (last, last_path) = database("t9-rebuild-last", "repo-dst");
    task(&last, "repo-dst", "task-last", "review");
    import_into(
        &last,
        &last_path,
        "task-last",
        "transfer-2",
        second,
        crate::transfer_engine::payload::sha256_hex(&bytes),
        SessionStart::Resumed,
    );
    check(&projected(&last, &last_path, "task-last"), "task-last");
}
