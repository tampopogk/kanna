//! Approve for build against a real git repository and artifact store.

use super::*;
use crate::db::design::DesignApprovalRow;
use crate::design::document::BlockOp;
use crate::design::service::tests::seed_design_task;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

struct Setup {
    state: Arc<AppState>,
    repo: PathBuf,
}

/// A repository whose committed policy keeps results and a summary, the
/// task's worktree at that repository, and an isolated artifact home.
fn setup(label: &str, retain: &str) -> Setup {
    let repo = crate::test_paths::unique_test_path(&format!("design-approval-{label}"));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join(".kanna")).unwrap();
    std::fs::write(repo.join("README.md"), "app\n").unwrap();
    std::fs::write(
        repo.join(".kanna/config.json"),
        serde_json::json!({ "design": { "handoff": { "retain": retain, "path": "docs/design-results/{task}" } } })
            .to_string(),
    )
    .unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["update-ref", "refs/remotes/origin/main", &head]);
    let home = crate::test_paths::unique_test_path(&format!("design-artifacts-{label}"));
    let repo_path = repo.to_string_lossy().to_string();
    let state = crate::http_api::test_support::test_state_with_artifact_home(
        &format!("design-approval-{label}"),
        &home,
        |db| {
            seed_design_task(db);
            db.execute_test_sql(&format!(
                "UPDATE repo SET path = '{repo_path}', default_branch = 'main' WHERE id = 'repo-1'"
            ))
            .unwrap();
            db.upsert_worktree("wt-1", "task-d", &repo_path, "task-d").unwrap();
        },
    );
    Setup { state, repo }
}

impl Setup {
    fn db(&self) -> Db {
        Db::open(&self.state.config().db_path).unwrap()
    }

    fn db_path(&self) -> String {
        self.state.config().db_path.clone()
    }

    fn candidate(&self) -> CandidateView {
        prepare_candidate(&self.state, &self.db(), &self.state.design, &self.db_path(), "task-d").unwrap()
    }

    fn confirm(&self, candidate: &CandidateView, token: &str) -> Result<ApprovalView, DesignError> {
        confirm(
            &self.state,
            &self.db(),
            &self.state.design,
            &self.db_path(),
            "task-d",
            &candidate.approval.id,
            token,
            "person (test)",
        )
    }

    fn write_document(&self, text: &str) {
        let db = self.db();
        let view = service::view(&db, &self.state.design, &self.db_path(), "task-d", true).unwrap();
        let block = &view.document.unwrap().blocks[0];
        service::agent_edit(
            &db,
            &self.state.design,
            &self.db_path(),
            "task-d",
            &format!("op-{}", text.len()),
            &[BlockOp::ReplaceText {
                block_id: block.id.clone(),
                expected_text: block.text.clone(),
                text: text.into(),
            }],
        )
        .unwrap();
    }

    fn approval(&self) -> DesignApprovalRow {
        self.db().current_design_approval("task-d").unwrap().unwrap()
    }

    fn export(&self) {
        export_retained(&self.state, &self.approval()).unwrap();
    }

    fn begin_commit_step(&self) {
        let head = git(&self.repo, &["rev-parse", "HEAD"]);
        assert!(self
            .db()
            .advance_design_approval(
                &self.approval().id,
                &[DesignApprovalRow::EXPORTED],
                DesignApprovalRow::COMMITTING,
                &DesignApprovalUpdate {
                    handoff_base_sha: Some(&head),
                    ..Default::default()
                },
            )
            .unwrap());
    }
}

#[test]
fn a_candidate_is_an_immutable_snapshot_of_the_committed_disposable_repository() {
    let setup = setup("candidate", "results-and-summary");
    setup.write_document("The approved design");
    let candidate = setup.candidate();
    assert_eq!(candidate.approval.phase, "candidate");
    assert_eq!(
        candidate.policy.files,
        vec!["docs/design-results/task-d/design.md", "docs/design-results/task-d/SUMMARY.md"]
    );
    assert_eq!(candidate.next_stage.as_deref(), Some("plan"));
    let artifact = candidate.approval.artifact_id.clone().unwrap();
    // The disposable repository's commit is what is being approved.
    let view = service::view(&setup.db(), &setup.state.design, &setup.db_path(), "task-d", false).unwrap();
    let scratch = PathBuf::from(view.scratch_repository.unwrap());
    assert_eq!(git(&scratch, &["rev-parse", "HEAD"]), candidate.approval.source_commit.clone().unwrap());
    assert!(std::fs::read_to_string(scratch.join(".kanna-design/design.md")).unwrap().contains("The approved design"));
    // The snapshot holds the rendered document, the export and the metadata.
    let store = artifact_store(&setup.state, &setup.db(), "repo-1").unwrap();
    let detail = serde_json::to_value(store.detail(&artifact).unwrap()).unwrap();
    assert!(detail.to_string().contains("index.html"));
    let index = String::from_utf8(store.read_file(&artifact, "index.html").unwrap().bytes).unwrap();
    assert!(index.contains("The approved design"));
    let metadata: serde_json::Value =
        serde_json::from_slice(&store.read_file(&artifact, "approval.json").unwrap().bytes).unwrap();
    assert_eq!(metadata["sourceCommit"], candidate.approval.source_commit.clone().unwrap());
    assert_eq!(metadata["documentRevision"], candidate.approval.doc_revision);
    // The committed prototype source travels with the snapshot.
    assert!(store.read_file(&artifact, "source/.kanna-design/design.md").is_ok());
    // The token is never stored in the clear.
    assert_ne!(setup.approval().confirmation_hash.as_deref(), Some(candidate.confirmation_token.as_str()));
}

#[test]
fn confirmation_needs_the_shown_token_the_same_document_and_happens_once() {
    let setup = setup("confirm", "results-and-summary");
    setup.write_document("Version one");
    let candidate = setup.candidate();
    assert!(matches!(setup.confirm(&candidate, "forged"), Err(DesignError::Invalid { .. })));

    // The document changed after the candidate: it is refused and retired.
    setup.write_document("Version two, changed");
    assert!(matches!(
        setup.confirm(&candidate, &candidate.confirmation_token),
        Err(DesignError::Conflict { .. })
    ));
    assert_eq!(setup.db().design_approval(&candidate.approval.id).unwrap().unwrap().phase, "invalidated");

    let fresh = setup.candidate();
    let approved = setup.confirm(&fresh, &fresh.confirmation_token).unwrap();
    assert_eq!(approved.phase, "approved");
    let session = setup.db().design_session("task-d").unwrap().unwrap();
    assert_eq!(session.status, "handing_off");
    // The decision is on the exact snapshot.
    let store = artifact_store(&setup.state, &setup.db(), "repo-1").unwrap();
    let detail = serde_json::to_value(store.detail(fresh.approval.artifact_id.as_deref().unwrap()).unwrap()).unwrap();
    assert!(detail.to_string().contains("approved for build"));
    // A double click is refused, and the token is spent.
    assert!(setup.confirm(&fresh, &fresh.confirmation_token).is_err());
    assert!(setup.approval().confirmation_hash.is_none());
    // The document is read-only while it is handed off.
    let db = setup.db();
    let block = service::view(&db, &setup.state.design, &setup.db_path(), "task-d", true)
        .unwrap()
        .document
        .unwrap()
        .blocks[0]
        .clone();
    assert!(matches!(
        service::agent_edit(
            &db,
            &setup.state.design,
            &setup.db_path(),
            "task-d",
            "op-late",
            &[BlockOp::ReplaceText {
                block_id: block.id,
                expected_text: block.text,
                text: "late".into(),
            }],
        ),
        Err(DesignError::NotDesigning { .. })
    ));
}

#[test]
fn the_hand_off_commits_exactly_the_retained_files_and_is_verified() {
    let setup = setup("verify", "results-and-summary");
    setup.write_document("Ship this design");
    let candidate = setup.candidate();
    setup.confirm(&candidate, &candidate.confirmation_token).unwrap();
    // No plain advance may leave the design stage before the export.
    assert!(guard_design_exit(&setup.db(), "task-d", "design").is_err());
    setup.export();
    let approval = setup.approval();
    assert_eq!(approval.phase, "exported");
    let design_md = std::fs::read_to_string(setup.repo.join("docs/design-results/task-d/design.md")).unwrap();
    assert!(design_md.contains("Ship this design"));
    let summary = std::fs::read_to_string(setup.repo.join("docs/design-results/task-d/SUMMARY.md")).unwrap();
    assert!(summary.contains(candidate.approval.artifact_id.as_deref().unwrap()));
    assert!(summary.contains(candidate.approval.source_commit.as_deref().unwrap()));
    assert!(guard_design_exit(&setup.db(), "task-d", "design").is_ok());
    let instruction = commit_step_instruction(&setup.db(), "task-d").unwrap();
    assert!(instruction.contains("- docs/design-results/task-d/design.md"));
    assert!(instruction.contains("Do not commit anything else"));

    setup.begin_commit_step();
    // The agent commits exactly the retained files.
    git(&setup.repo, &["add", "docs/design-results/task-d"]);
    git(&setup.repo, &["commit", "-m", "docs(design): approved design results"]);
    let head = git(&setup.repo, &["rev-parse", "HEAD"]);
    assert_eq!(verify_handoff_commit(&setup.db(), "task-d").unwrap(), head);
    let approval = setup.approval();
    assert_eq!(approval.phase, "committed");
    assert_eq!(approval.committed_sha.as_deref(), Some(head.as_str()));
}

#[test]
fn a_commit_that_sweeps_in_other_files_or_changes_the_results_is_refused() {
    let setup = setup("refuse", "results-and-summary");
    setup.write_document("Design");
    let candidate = setup.candidate();
    setup.confirm(&candidate, &candidate.confirmation_token).unwrap();
    setup.export();
    setup.begin_commit_step();
    std::fs::write(setup.repo.join("prototype.js"), "throwaway").unwrap();
    git(&setup.repo, &["add", "."]);
    git(&setup.repo, &["commit", "-m", "everything"]);
    let error = verify_handoff_commit(&setup.db(), "task-d").unwrap_err();
    assert!(error.contains("prototype.js"), "{error}");
    let approval = setup.approval();
    assert_eq!(approval.phase, "failed");
    assert!(approval.error.unwrap().contains("prototype.js"));

    // Retrying resumes from the export; a commit with edited results is refused too.
    retry_handoff(&setup.db(), &setup.state.design, "task-d").unwrap();
    assert_eq!(setup.approval().phase, "exported");
    git(&setup.repo, &["reset", "--hard", "HEAD~1"]);
    setup.export_again();
    setup.begin_commit_step();
    std::fs::write(setup.repo.join("docs/design-results/task-d/design.md"), "edited by hand").unwrap();
    git(&setup.repo, &["add", "docs"]);
    git(&setup.repo, &["commit", "-m", "results"]);
    let error = verify_handoff_commit(&setup.db(), "task-d").unwrap_err();
    assert!(error.contains("different content"), "{error}");
}

impl Setup {
    /// After a reset the exported files are gone; write them back as the
    /// resumed hand-off would have left them.
    fn export_again(&self) {
        let approval = self.approval();
        let retained: Vec<RetainedFile> = serde_json::from_str(approval.retained_json.as_deref().unwrap()).unwrap();
        let snapshot = candidates_dir(&self.db_path(), "task-d").unwrap().join(&approval.id);
        for file in retained {
            let target = self.repo.join(&file.path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            if file.path.ends_with("design.md") {
                std::fs::copy(snapshot.join("design.md"), &target).unwrap();
            }
        }
    }
}

#[test]
fn a_policy_that_keeps_nothing_commits_nothing() {
    let setup = setup("nothing", "nothing");
    setup.write_document("Design");
    let candidate = setup.candidate();
    assert!(candidate.policy.files.is_empty());
    setup.confirm(&candidate, &candidate.confirmation_token).unwrap();
    setup.export();
    assert!(!setup.repo.join("docs").exists());
    assert!(commit_step_instruction(&setup.db(), "task-d").unwrap().contains("commit nothing"));
    setup.begin_commit_step();
    let head = git(&setup.repo, &["rev-parse", "HEAD"]);
    assert_eq!(verify_handoff_commit(&setup.db(), "task-d").unwrap(), head);
}

#[test]
fn reopening_before_the_factory_starts_withdraws_the_hand_off() {
    let setup = setup("reopen", "results-and-summary");
    setup.write_document("Design");
    let candidate = setup.candidate();
    setup.confirm(&candidate, &candidate.confirmation_token).unwrap();
    setup.export();
    assert!(setup.repo.join("docs/design-results/task-d/design.md").exists());
    reopen(&setup.db(), &setup.state.design, "task-d").unwrap();
    assert_eq!(setup.db().design_approval(&candidate.approval.id).unwrap().unwrap().phase, "invalidated");
    assert!(!setup.repo.join("docs/design-results/task-d/design.md").exists());
    assert_eq!(setup.db().design_session("task-d").unwrap().unwrap().status, "designing");
    assert!(guard_design_exit(&setup.db(), "task-d", "design").is_err());
    // Designing again works, and a new approval is needed.
    setup.write_document("Design, revised");
    assert_eq!(setup.candidate().approval.phase, "candidate");
}

#[test]
fn a_plain_advance_of_the_design_stage_is_refused() {
    let setup = setup("advance", "results-and-summary");
    let error = crate::task_creator::prepare_advance_stage_for_api(&setup.db(), setup.state.config(), "task-d")
        .err()
        .expect("advance refused");
    assert!(error.contains("Approve for build"), "{error}");
}

#[test]
fn policy_paths_stay_inside_the_repository() {
    for bad in ["../outside", "/abs/{task}", "~/home", "a/../../b"] {
        let policy = RepoDesignHandoffPolicy {
            retain: DesignRetention::ResultsAndSummary,
            path: bad.into(),
        };
        assert!(bind_policy(&policy, "t").is_err(), "{bad}");
    }
    let bound = bind_policy(
        &RepoDesignHandoffPolicy {
            retain: DesignRetention::ResultsAndSummary,
            path: "docs/design/{task}/".into(),
        },
        "t1",
    )
    .unwrap();
    assert_eq!(bound.path, "docs/design/t1");
}
