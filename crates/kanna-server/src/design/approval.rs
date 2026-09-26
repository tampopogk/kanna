//! Approve for build and the hand-off to the software factory
//! (docs/specs/app-design.md §6, §7a).
//!
//! 1. **Candidate** (`prepare_candidate`, the desktop): the design's
//!    disposable repository is committed with the exported document and
//!    feedback; that commit, the rendered document and the approval metadata
//!    are published as one immutable snapshot in the artifact store. The
//!    candidate carries a short-lived, single-use confirmation token bound to
//!    exactly that document revision, commit and policy.
//! 2. **Confirmation** (`confirm`, the native control socket only): the
//!    person's click. A document that changed since the candidate refuses it.
//!    The decision "approved for build" is recorded on the snapshot.
//! 3. **Hand-off** (`run_handoffs`, durable phases that resume after a
//!    restart): `approved` → the policy's retained files are written into the
//!    task's worktree (`exported`) → once the live design session is free the
//!    stage's commit step asks it to commit exactly those files
//!    (`committing`) → the commit is verified before the transition may fire
//!    (`committed`) → the factory's first stage starts from it (`entered`).
//!
//! Nothing here claims more than it saw: approval does not claim the commit
//! succeeded, a failed phase stays visible with its error, and the builder
//! never starts before verification.

use super::export;
use super::service::{self, ApprovalView, DesignError, ThreadView};
use super::DesignRuntime;
use crate::artifacts::store::{ArtifactStore, DecisionRequest, PublishLimits, PublishRequest};
use crate::artifacts::types::ArtifactContentKind;
use crate::db::design::{DesignApprovalRow, DesignApprovalUpdate, DesignSessionRow};
use crate::db::Db;
use crate::http_api::AppState;
use crate::task_creator::{DesignRetention, RepoDesignHandoffPolicy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const CONFIRMATION_LIFETIME: Duration = Duration::from_secs(10 * 60);
const SNAPSHOT_SOURCE_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn now() -> String {
    crate::artifacts::rfc3339_utc(SystemTime::now())
}

fn task_title(db: &Db, task_id: &str) -> Result<String, DesignError> {
    let item = db
        .get_pipeline_item(task_id)?
        .ok_or_else(|| DesignError::NotFound {
            message: format!("task not found: {task_id}"),
        })?;
    Ok(item
        .display_name
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            item.prompt
                .as_deref()
                .and_then(|prompt| prompt.lines().find(|line| !line.trim().is_empty()))
                .map(|line| line.chars().take(80).collect())
        })
        .unwrap_or_else(|| format!("Task {task_id}")))
}

/// The retention policy an approval was bound to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BoundPolicy {
    /// `results-and-summary` or `nothing`.
    pub(crate) retain: String,
    /// The repository folder, with `{task}` expanded.
    pub(crate) path: String,
    /// The files the hand-off commits, relative to the repository root.
    pub(crate) files: Vec<String>,
}

/// Expand and check a policy path: relative, inside the repository, no
/// parent components.
fn bind_policy(
    policy: &RepoDesignHandoffPolicy,
    task_id: &str,
) -> Result<BoundPolicy, DesignError> {
    let expanded = policy.path.replace("{task}", task_id);
    let path = expanded.trim_end_matches('/').to_string();
    let valid = !path.is_empty()
        && !path.starts_with('/')
        && !path.starts_with('~')
        && Path::new(&path)
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)));
    if !valid {
        return Err(DesignError::invalid(format!(
            "design.handoff.path '{}' must be a relative folder inside the repository",
            policy.path
        )));
    }
    let (retain, files) = match policy.retain {
        DesignRetention::ResultsAndSummary => (
            "results-and-summary",
            vec![format!("{path}/design.md"), format!("{path}/SUMMARY.md")],
        ),
        DesignRetention::Nothing => ("nothing", Vec::new()),
    };
    Ok(BoundPolicy {
        retain: retain.to_string(),
        path,
        files,
    })
}

fn candidates_dir(db_path: &str, task_id: &str) -> Option<PathBuf> {
    crate::task_store::task_dir_for_db_path(db_path, task_id)
        .map(|dir| dir.join("design").join("candidates"))
}

// ---------------------------------------------------------------------------
// The disposable repository
// ---------------------------------------------------------------------------

fn signature() -> Result<git2::Signature<'static>, DesignError> {
    git2::Signature::now("Kanna", "kanna@localhost").map_err(DesignError::internal)
}

/// Commit everything in the design's disposable repository (the agent's
/// prototype code and the exported design) and return the commit.
fn commit_scratch(repo_dir: &Path, message: &str) -> Result<String, DesignError> {
    let repository = git2::Repository::open(repo_dir).map_err(DesignError::internal)?;
    let mut index = repository.index().map_err(DesignError::internal)?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(DesignError::internal)?;
    index
        .update_all(["*"].iter(), None)
        .map_err(DesignError::internal)?;
    index.write().map_err(DesignError::internal)?;
    let tree_id = index.write_tree().map_err(DesignError::internal)?;
    let tree = repository
        .find_tree(tree_id)
        .map_err(DesignError::internal)?;
    let parent = repository
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok());
    if let Some(parent) = &parent {
        if parent.tree_id() == tree_id {
            return Ok(parent.id().to_string());
        }
    }
    let signature = signature()?;
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    let commit = repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &parents,
        )
        .map_err(DesignError::internal)?;
    Ok(commit.to_string())
}

/// Copy the committed tree of the disposable repository into `target`
/// (source files only; oversized files are left out and listed).
fn copy_committed_source(
    repo_dir: &Path,
    commit: &str,
    target: &Path,
) -> Result<Vec<String>, DesignError> {
    let repository = git2::Repository::open(repo_dir).map_err(DesignError::internal)?;
    let commit = repository
        .find_commit(git2::Oid::from_str(commit).map_err(DesignError::internal)?)
        .map_err(DesignError::internal)?;
    let tree = commit.tree().map_err(DesignError::internal)?;
    let mut skipped = Vec::new();
    let mut failure = None;
    tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
        if entry.kind() != Some(git2::ObjectType::Blob) {
            return git2::TreeWalkResult::Ok;
        }
        let name = format!("{root}{}", entry.name().unwrap_or(""));
        let Ok(blob) = repository.find_blob(entry.id()) else {
            return git2::TreeWalkResult::Ok;
        };
        if blob.size() as u64 > SNAPSHOT_SOURCE_MAX_FILE_BYTES {
            skipped.push(name);
            return git2::TreeWalkResult::Ok;
        }
        let path = target.join(&name);
        if let Err(error) = path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .and_then(|_| std::fs::write(&path, blob.content()))
        {
            failure = Some(error.to_string());
            return git2::TreeWalkResult::Abort;
        }
        git2::TreeWalkResult::Ok
    })
    .map_err(DesignError::internal)?;
    if let Some(error) = failure {
        return Err(DesignError::internal(format!(
            "copying the prototype source: {error}"
        )));
    }
    Ok(skipped)
}

fn artifact_store(state: &AppState, db: &Db, repo_id: &str) -> Result<ArtifactStore, DesignError> {
    let repo = db.get_repo(repo_id)?.ok_or_else(|| DesignError::NotFound {
        message: format!("repository {repo_id} not found"),
    })?;
    let policy = crate::task_creator::load_repo_artifact_policy(state.repo_definitions(), &repo)
        .map_err(|error| DesignError::Unavailable {
            message: format!("repository configuration could not be resolved: {error}"),
        })?;
    let path = crate::artifacts::resolve_repository_path(
        state.artifact_storage(),
        &repo.id,
        Path::new(&repo.path),
        policy.repository_path.as_deref(),
    )
    .map_err(|error| DesignError::Unavailable {
        message: error.to_string(),
    })?;
    ArtifactStore::open_or_create(&path, &repo.id).map_err(|error| DesignError::Unavailable {
        message: error.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Candidate
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CandidateView {
    pub(crate) approval: ApprovalView,
    /// Shown once, to the desktop that asked; confirming needs it back.
    pub(crate) confirmation_token: String,
    pub(crate) policy: BoundPolicy,
    pub(crate) next_stage: Option<String>,
    pub(crate) open_threads: usize,
    pub(crate) undelivered_feedback: usize,
    pub(crate) skipped_source_files: Vec<String>,
}

pub(crate) fn prepare_candidate(
    state: &AppState,
    db: &Db,
    runtime: &DesignRuntime,
    db_path: &str,
    task_id: &str,
) -> Result<CandidateView, DesignError> {
    let (stage, session) = service::ensure_session(db, task_id)?;
    if !stage.is_current() || session.status != DesignSessionRow::DESIGNING {
        return Err(DesignError::NotDesigning {
            message: "the design is not in its design stage; there is nothing to approve".into(),
        });
    }
    if let Some(current) = db.current_design_approval(task_id)? {
        if current.is_handing_off() {
            return Err(DesignError::Conflict {
                message: format!(
                    "the design was already approved and is being handed off ({})",
                    current.phase
                ),
            });
        }
    }
    let item = db
        .get_pipeline_item(task_id)?
        .ok_or_else(|| DesignError::NotFound {
            message: format!("task not found: {task_id}"),
        })?;
    let repo = db
        .get_repo(&item.repo_id)?
        .ok_or_else(|| DesignError::NotFound {
            message: format!("repository {} not found", item.repo_id),
        })?;
    let policy = crate::task_creator::load_repo_design_policy(state.repo_definitions(), &repo)
        .map_err(|error| DesignError::Unavailable {
            message: format!("the repository's design policy could not be read: {error}"),
        })?;
    let policy = bind_policy(&policy, task_id)?;
    let (blocks, state_bytes, revision) =
        runtime
            .documents
            .read(db, db_path, task_id, |document, revision| {
                Ok((document.project(), document.encode_state(), revision))
            })?;
    let blocks = blocks.map_err(|error| DesignError::Schema {
        message: format!("the document cannot be exported: {error}"),
    })?;
    let view = service::view(db, runtime, db_path, task_id, false)?;
    let title = task_title(db, task_id)?;
    let doc_sha256 = super::live::state_digest(&state_bytes);
    let design_md = export::document_markdown(&title, &blocks);
    let feedback_md = export::feedback_markdown(&view.threads);

    // 1. Commit the disposable repository, with the exported design in it.
    let scratch =
        service::scratch_repository(db_path, task_id, session.epoch).ok_or_else(|| {
            DesignError::Unavailable {
                message: "the design's disposable repository could not be created".into(),
            }
        })?;
    let record = scratch.join(".kanna-design");
    std::fs::create_dir_all(&record).map_err(DesignError::internal)?;
    std::fs::write(record.join("design.md"), &design_md).map_err(DesignError::internal)?;
    std::fs::write(record.join("feedback.md"), &feedback_md).map_err(DesignError::internal)?;
    let source_commit = commit_scratch(
        &scratch,
        &format!("Design candidate: {title} (document revision {revision})"),
    )?;

    // 2. Publish the snapshot: rendered document, committed source, metadata.
    let approval_id = format!(
        "ap-{}",
        crate::artifacts::random_hex(12).map_err(DesignError::internal)?
    );
    let candidates = candidates_dir(db_path, task_id).ok_or_else(|| DesignError::Unavailable {
        message: "the task directory is unavailable".into(),
    })?;
    let snapshot = candidates.join(&approval_id);
    std::fs::create_dir_all(&snapshot).map_err(DesignError::internal)?;
    std::fs::write(
        snapshot.join("index.html"),
        export::document_html(&title, &blocks, &view.threads),
    )
    .map_err(DesignError::internal)?;
    std::fs::write(snapshot.join("design.md"), &design_md).map_err(DesignError::internal)?;
    std::fs::write(snapshot.join("feedback.md"), &feedback_md).map_err(DesignError::internal)?;
    let skipped = copy_committed_source(&scratch, &source_commit, &snapshot.join("source"))?;
    let metadata = json!({
        "schemaVersion": 1,
        "taskId": task_id,
        "title": title,
        "stage": stage.stage,
        "position": session.position,
        "epoch": session.epoch,
        "documentRevision": revision,
        "documentSha256": doc_sha256,
        "documentSchema": session.schema_version,
        "sourceCommit": source_commit,
        "policy": policy,
        "nextStage": stage.next_stage,
        "skippedSourceFiles": skipped,
    });
    std::fs::write(
        snapshot.join("approval.json"),
        serde_json::to_vec_pretty(&metadata).map_err(DesignError::internal)?,
    )
    .map_err(DesignError::internal)?;
    let store = artifact_store(state, db, &repo.id)?;
    let artifact_policy =
        crate::task_creator::load_repo_artifact_policy(state.repo_definitions(), &repo).map_err(
            |error| DesignError::Unavailable {
                message: error.to_string(),
            },
        )?;
    let published = store
        .publish(PublishRequest {
            task_id,
            workspace_root: &candidates,
            source_path: &approval_id,
            kind: ArtifactContentKind::Document,
            entrypoint: Some("index.html"),
            previous: None,
            retention: artifact_policy.retention,
            limits: PublishLimits::default(),
        })
        .map_err(|error| DesignError::Unavailable {
            message: format!("the snapshot could not be published: {error}"),
        })?;

    // 3. The candidate and its single-use confirmation.
    let token = crate::artifacts::random_hex(32).map_err(DesignError::internal)?;
    let expires = crate::artifacts::rfc3339_utc(SystemTime::now() + CONFIRMATION_LIFETIME);
    let row = db.insert_design_candidate(
        &approval_id,
        task_id,
        session.epoch,
        revision,
        &doc_sha256,
        &source_commit,
        Some(&repo.id),
        Some(&published.artifact_id),
        &serde_json::to_string(&policy).map_err(DesignError::internal)?,
        &sha256_hex(token.as_bytes()),
        &expires,
    )?;
    runtime.feed_changed(task_id);
    Ok(CandidateView {
        approval: ApprovalView::from_row(&row, revision),
        confirmation_token: token,
        policy,
        next_stage: stage.next_stage,
        open_threads: view
            .threads
            .iter()
            .filter(|thread| thread.status == "open")
            .count(),
        undelivered_feedback: view
            .threads
            .iter()
            .filter(|thread| {
                matches!(
                    thread.delivery_status.as_str(),
                    "queued" | "delivering" | "uncertain"
                )
            })
            .count(),
        skipped_source_files: skipped,
    })
}

// ---------------------------------------------------------------------------
// Confirmation (native control socket only)
// ---------------------------------------------------------------------------

/// The person's confirmation of a candidate. Reached only through
/// `crate::human_control`, whose peer the kernel identifies as the desktop.
#[allow(clippy::too_many_arguments)]
pub(crate) fn confirm(
    state: &AppState,
    db: &Db,
    runtime: &DesignRuntime,
    db_path: &str,
    task_id: &str,
    approval_id: &str,
    token: &str,
    approved_by: &str,
) -> Result<ApprovalView, DesignError> {
    let approval = db
        .design_approval(approval_id)?
        .filter(|approval| approval.task_id == task_id)
        .ok_or_else(|| DesignError::NotFound {
            message: format!("approval {approval_id} is not in task {task_id}"),
        })?;
    let (stage, session) = service::ensure_session(db, task_id)?;
    let stale = |reason: &str| -> Result<ApprovalView, DesignError> {
        db.advance_design_approval(
            approval_id,
            &[DesignApprovalRow::CANDIDATE],
            DesignApprovalRow::INVALIDATED,
            &DesignApprovalUpdate {
                consume_confirmation: true,
                error: Some(reason),
                ..Default::default()
            },
        )?;
        runtime.feed_changed(task_id);
        Err(DesignError::Conflict {
            message: format!("{reason}; review the design again and approve the current version"),
        })
    };
    if approval.phase != DesignApprovalRow::CANDIDATE {
        return Err(DesignError::Conflict {
            message: format!(
                "approval {approval_id} is {}, not awaiting confirmation",
                approval.phase
            ),
        });
    }
    let expected = approval.confirmation_hash.as_deref().unwrap_or("");
    if expected.is_empty() || sha256_hex(token.as_bytes()) != expected {
        return Err(DesignError::Invalid {
            message: "this confirmation does not belong to the candidate shown".into(),
        });
    }
    if approval
        .confirmation_expires_at
        .as_deref()
        .is_none_or(|expires| expires < now().as_str())
    {
        return stale("the confirmation expired");
    }
    if !stage.is_current()
        || session.status != DesignSessionRow::DESIGNING
        || approval.epoch != session.epoch
    {
        return stale("the design moved on since this candidate was prepared");
    }
    let current_revision = runtime
        .documents
        .read(db, db_path, task_id, |_, revision| Ok(revision))?;
    if current_revision != approval.doc_revision {
        return stale("the document changed after this candidate was prepared");
    }
    let policy: BoundPolicy =
        serde_json::from_str(&approval.policy_json).map_err(DesignError::internal)?;
    let item = db
        .get_pipeline_item(task_id)?
        .ok_or_else(|| DesignError::NotFound {
            message: format!("task not found: {task_id}"),
        })?;
    let repo = db
        .get_repo(&item.repo_id)?
        .ok_or_else(|| DesignError::NotFound {
            message: format!("repository {} not found", item.repo_id),
        })?;
    let current_policy =
        crate::task_creator::load_repo_design_policy(state.repo_definitions(), &repo)
            .map_err(|error| DesignError::Unavailable { message: error })?;
    if bind_policy(&current_policy, task_id)? != policy {
        return stale("the repository's design policy changed after this candidate was prepared");
    }

    // The decision is recorded on the exact snapshot before the phase moves:
    // a failure here leaves the candidate confirmable again.
    if let (Some(repo_id), Some(artifact_id)) = (&approval.artifact_repo_id, &approval.artifact_id)
    {
        artifact_store(state, db, repo_id)?
            .record_decision(
                artifact_id,
                DecisionRequest {
                    who: "person (Kanna desktop)",
                    what: "approved for build",
                },
            )
            .map_err(|error| DesignError::Unavailable {
                message: format!("the decision could not be recorded on the snapshot: {error}"),
            })?;
    }
    let approved_at = now();
    let moved = db.in_immediate_transaction_if_needed(|db| {
        let moved = db.advance_design_approval(
            approval_id,
            &[DesignApprovalRow::CANDIDATE],
            DesignApprovalRow::APPROVED,
            &DesignApprovalUpdate {
                approved_at: Some(&approved_at),
                approved_by: Some(approved_by),
                consume_confirmation: true,
                ..Default::default()
            },
        )?;
        if moved {
            db.set_design_session_status(task_id, DesignSessionRow::HANDING_OFF)?;
        }
        Ok::<_, rusqlite::Error>(moved)
    })?;
    if !moved {
        // A double click: the first confirmation already moved it.
        return Err(DesignError::Conflict {
            message: "this approval was already confirmed".into(),
        });
    }
    runtime.feed_changed(task_id);
    runtime.wake_delivery();
    let row = db.design_approval(approval_id)?.expect("approval exists");
    Ok(ApprovalView::from_row(&row, current_revision))
}

// ---------------------------------------------------------------------------
// Reopen and retry
// ---------------------------------------------------------------------------

/// Return to designing before the software factory started: the pending
/// approval (or candidate) no longer hands anything off, and files it
/// exported into the worktree are removed. After the factory started, the
/// task is sent back to its design stage instead, which begins a new epoch.
pub(crate) fn reopen(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
) -> Result<Value, DesignError> {
    let (stage, session) = service::ensure_session(db, task_id)?;
    if !stage.is_current() {
        return Err(DesignError::NotDesigning {
            message: format!(
                "the design was handed to the software factory (the task is at {}); send the task \
                 back to its {} stage to reopen it",
                stage.current_stage.as_deref().unwrap_or("another stage"),
                stage.stage
            ),
        });
    }
    let Some(approval) = db.current_design_approval(task_id)? else {
        return Ok(json!({ "reopened": false, "status": session.status }));
    };
    if matches!(
        approval.phase.as_str(),
        DesignApprovalRow::COMMITTING | DesignApprovalRow::COMMITTED
    ) {
        return Err(DesignError::Conflict {
            message: "the hand-off's commit step is running; wait for it to finish or fail".into(),
        });
    }
    if approval.phase == DesignApprovalRow::ENTERED {
        return Err(DesignError::NotDesigning {
            message: "the design was handed off".into(),
        });
    }
    remove_exported(db, task_id, &approval);
    db.in_immediate_transaction_if_needed(|db| {
        db.advance_design_approval(
            &approval.id,
            &[
                DesignApprovalRow::CANDIDATE,
                DesignApprovalRow::APPROVED,
                DesignApprovalRow::EXPORTED,
                DesignApprovalRow::FAILED,
            ],
            DesignApprovalRow::INVALIDATED,
            &DesignApprovalUpdate {
                consume_confirmation: true,
                error: Some("reopened for more design"),
                ..Default::default()
            },
        )?;
        db.set_design_session_status(task_id, DesignSessionRow::DESIGNING)
    })?;
    runtime.feed_changed(task_id);
    runtime.wake_delivery();
    Ok(json!({ "reopened": true, "status": DesignSessionRow::DESIGNING }))
}

fn remove_exported(db: &Db, task_id: &str, approval: &DesignApprovalRow) {
    let (Some(retained), Ok(Some(worktree))) = (
        approval
            .retained_json
            .as_deref()
            .and_then(|json| serde_json::from_str::<Vec<RetainedFile>>(json).ok()),
        db.get_task_worktree_path(task_id),
    ) else {
        return;
    };
    for file in retained {
        let _ = std::fs::remove_file(Path::new(&worktree).join(&file.path));
    }
}

/// Resume a failed hand-off from its last good phase.
pub(crate) fn retry_handoff(
    db: &Db,
    runtime: &DesignRuntime,
    task_id: &str,
) -> Result<Value, DesignError> {
    let approval = db
        .current_design_approval(task_id)?
        .filter(|approval| approval.phase == DesignApprovalRow::FAILED)
        .ok_or_else(|| DesignError::Conflict {
            message: "there is no failed hand-off to retry".into(),
        })?;
    let to = if approval.retained_json.is_some() {
        DesignApprovalRow::EXPORTED
    } else {
        DesignApprovalRow::APPROVED
    };
    db.advance_design_approval(
        &approval.id,
        &[DesignApprovalRow::FAILED],
        to,
        &DesignApprovalUpdate::default(),
    )?;
    runtime.feed_changed(task_id);
    runtime.wake_delivery();
    Ok(json!({ "phase": to }))
}

// ---------------------------------------------------------------------------
// The commit step: guard, instruction, verification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RetainedFile {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

fn handing_off_approval(db: &Db, task_id: &str) -> Result<Option<DesignApprovalRow>, String> {
    db.current_design_approval(task_id)
        .map_err(|error| format!("db error: {error}"))
}

/// An App Design stage leaves only through Approve for build: the explicit
/// advance that dispatches its commit step is the hand-off's own. A commit
/// step that already ran (a retried hand-off) is verified again here, since
/// the advance then transitions without another one.
pub(crate) fn guard_design_exit(db: &Db, task_id: &str, stage: &str) -> Result<(), String> {
    let refusal = || {
        format!(
            "stage '{stage}' is an App Design stage: it leaves only through Approve for build in \
             the Kanna desktop app, which commits the approved results and then starts the next stage"
        )
    };
    let Some(approval) = handing_off_approval(db, task_id)? else {
        return Err(refusal());
    };
    if !matches!(
        approval.phase.as_str(),
        DesignApprovalRow::EXPORTED | DesignApprovalRow::COMMITTING | DesignApprovalRow::COMMITTED
    ) {
        return Err(refusal());
    }
    let commit_already_ran = db
        .latest_stage_run(task_id)
        .map_err(|error| format!("db error: {error}"))?
        .is_some_and(|run| {
            run.kind == "post"
                && run.stage == format!("{stage} commit")
                && run.status == "succeeded"
        });
    if commit_already_ran && approval.phase != DesignApprovalRow::COMMITTED {
        verify_handoff_commit(db, task_id)?;
    }
    Ok(())
}

/// What the live design session is told when the commit step runs: the
/// approval, and exactly which files to commit.
pub(crate) fn commit_step_instruction(db: &Db, task_id: &str) -> Result<String, String> {
    let approval = handing_off_approval(db, task_id)?
        .filter(|approval| approval.is_handing_off())
        .ok_or_else(|| "no approved design is being handed off".to_string())?;
    let retained: Vec<RetainedFile> = approval
        .retained_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| format!("retained files unreadable: {error}"))?
        .unwrap_or_default();
    let snapshot = match (&approval.artifact_repo_id, &approval.artifact_id) {
        (Some(repo), Some(artifact)) => {
            format!("artifact {artifact} in repository {repo}'s artifact store")
        }
        _ => "the approved snapshot".to_string(),
    };
    let files = if retained.is_empty() {
        "This repository's design policy keeps nothing in the repository: commit nothing, and do \
         not commit anything else."
            .to_string()
    } else {
        format!(
            "Kanna has written the results its design policy keeps. Commit exactly these files, \
             unchanged, in one commit (`git add` each path, then `git commit -m \"docs(design): \
             approved design results\"`):\n{}\nDo not commit anything else: the prototype is \
             throwaway code in the design's disposable repository and must not enter this \
             repository. Kanna verifies the commit contains only these files, byte for byte, \
             before the next stage starts.",
            retained
                .iter()
                .map(|file| format!("- {}", file.path))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    Ok(format!(
        "The person approved this design for build ({snapshot}, disposable repository commit {}). \
         {files} Then record your result: say what was approved, what you committed, and what the \
         planning stage should know.",
        approval.source_commit.as_deref().unwrap_or("unknown"),
    ))
}

/// Check the commit step's work before the transition fires: HEAD holds
/// every retained file byte for byte, and nothing outside the retained files
/// changed since the commit step was requested. Records the verified commit
/// or the reason it failed on the approval.
pub(crate) fn verify_handoff_commit(db: &Db, task_id: &str) -> Result<String, String> {
    let approval = handing_off_approval(db, task_id)?
        .filter(|approval| {
            matches!(
                approval.phase.as_str(),
                DesignApprovalRow::COMMITTING
                    | DesignApprovalRow::COMMITTED
                    | DesignApprovalRow::EXPORTED
            )
        })
        .ok_or_else(|| "no approved design is being committed for this stage".to_string())?;
    match check_commit(db, task_id, &approval) {
        Ok(sha) => {
            db.advance_design_approval(
                &approval.id,
                &[
                    DesignApprovalRow::COMMITTING,
                    DesignApprovalRow::EXPORTED,
                    DesignApprovalRow::COMMITTED,
                ],
                DesignApprovalRow::COMMITTED,
                &DesignApprovalUpdate {
                    committed_sha: Some(&sha),
                    ..Default::default()
                },
            )
            .map_err(|error| format!("db error: {error}"))?;
            Ok(sha)
        }
        Err(reason) => {
            let message = format!("the hand-off commit was not accepted: {reason}");
            let _ = db.advance_design_approval(
                &approval.id,
                &[
                    DesignApprovalRow::COMMITTING,
                    DesignApprovalRow::EXPORTED,
                    DesignApprovalRow::COMMITTED,
                ],
                DesignApprovalRow::FAILED,
                &DesignApprovalUpdate {
                    error: Some(&message),
                    ..Default::default()
                },
            );
            Err(message)
        }
    }
}

fn check_commit(db: &Db, task_id: &str, approval: &DesignApprovalRow) -> Result<String, String> {
    let worktree = db
        .get_task_worktree_path(task_id)
        .map_err(|error| format!("db error: {error}"))?
        .ok_or_else(|| "the task has no worktree".to_string())?;
    let repository = git2::Repository::open(&worktree).map_err(|error| error.to_string())?;
    let head = repository
        .head()
        .and_then(|head| head.peel_to_commit())
        .map_err(|error| format!("no commit at HEAD: {error}"))?;
    let tree = head.tree().map_err(|error| error.to_string())?;
    let retained: Vec<RetainedFile> = approval
        .retained_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    for file in &retained {
        let entry = tree
            .get_path(Path::new(&file.path))
            .map_err(|_| format!("{} is not in the commit", file.path))?;
        let blob = repository
            .find_blob(entry.id())
            .map_err(|_| format!("{} is not a file in the commit", file.path))?;
        if sha256_hex(blob.content()) != file.sha256 {
            return Err(format!(
                "{} was committed with different content",
                file.path
            ));
        }
    }
    if let Some(base) = approval.handoff_base_sha.as_deref() {
        let base = repository
            .find_commit(git2::Oid::from_str(base).map_err(|error| error.to_string())?)
            .map_err(|error| format!("the pre-commit HEAD {base} is gone: {error}"))?;
        let diff = repository
            .diff_tree_to_tree(
                Some(&base.tree().map_err(|e| e.to_string())?),
                Some(&tree),
                None,
            )
            .map_err(|error| error.to_string())?;
        let allowed: std::collections::BTreeSet<&str> =
            retained.iter().map(|file| file.path.as_str()).collect();
        for delta in diff.deltas() {
            for path in [delta.old_file().path(), delta.new_file().path()]
                .into_iter()
                .flatten()
            {
                let path = path.to_string_lossy();
                if !allowed.contains(path.as_ref()) {
                    return Err(format!(
                        "the commit also changed {path}; only the retained design results may be committed"
                    ));
                }
            }
        }
    }
    Ok(head.id().to_string())
}

// ---------------------------------------------------------------------------
// The hand-off worker
// ---------------------------------------------------------------------------

/// Advance every hand-off one phase where it can. Called by the design
/// worker on each wake-up; every phase is re-derived from durable state, so
/// a restart resumes where it stopped.
pub(crate) async fn run_handoffs(state: &Arc<AppState>) {
    let db_path = state.config().db_path.clone();
    let pending = tokio::task::spawn_blocking(move || -> Result<Vec<DesignApprovalRow>, String> {
        let db = Db::open(&db_path).map_err(|error| error.to_string())?;
        db.handing_off_design_approvals()
            .map_err(|error| error.to_string())
    })
    .await;
    let pending = match pending {
        Ok(Ok(pending)) => pending,
        Ok(Err(error)) => {
            log::warn!("design hand-off scan failed: {error}");
            return;
        }
        Err(error) => {
            log::warn!("design hand-off scan failed: {error}");
            return;
        }
    };
    for approval in pending {
        if let Err(error) = advance_one(state, &approval).await {
            log::warn!(
                "design hand-off for {} did not advance: {error}",
                approval.task_id
            );
        }
    }
}

async fn advance_one(state: &Arc<AppState>, approval: &DesignApprovalRow) -> Result<(), String> {
    let task_id = approval.task_id.clone();
    match approval.phase.as_str() {
        DesignApprovalRow::APPROVED => {
            let state = Arc::clone(state);
            let approval = approval.clone();
            tokio::task::spawn_blocking(move || export_retained(&state, &approval))
                .await
                .map_err(|error| error.to_string())?
        }
        DesignApprovalRow::EXPORTED => {
            // The commit step goes to the live design session only when it
            // is free; it never starts a second agent.
            match super::delivery::session_readiness(state, &task_id).await {
                super::delivery::Readiness::Free { .. } => {}
                super::delivery::Readiness::Absent => {
                    return record_waiting(
                        state,
                        approval,
                        "the design session is not running; resume the task to finish the hand-off",
                    )
                    .await
                }
                _ => return Ok(()),
            }
            let db_path = state.config().db_path.clone();
            let id = approval.id.clone();
            let task = task_id.clone();
            let moved = tokio::task::spawn_blocking(move || -> Result<bool, String> {
                let db = Db::open(&db_path).map_err(|error| error.to_string())?;
                let worktree = db
                    .get_task_worktree_path(&task)
                    .map_err(|error| error.to_string())?
                    .ok_or("the task has no worktree")?;
                let repository =
                    git2::Repository::open(&worktree).map_err(|error| error.to_string())?;
                let head = repository
                    .head()
                    .and_then(|head| head.peel_to_commit())
                    .map_err(|error| error.to_string())?
                    .id()
                    .to_string();
                db.advance_design_approval(
                    &id,
                    &[DesignApprovalRow::EXPORTED],
                    DesignApprovalRow::COMMITTING,
                    &DesignApprovalUpdate {
                        handoff_base_sha: Some(&head),
                        ..Default::default()
                    },
                )
                .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())??;
            if !moved {
                return Ok(());
            }
            state.design.feed_changed(&task_id);
            if let Err(error) =
                crate::http_api::advance_design_stage(Arc::clone(state), &task_id).await
            {
                fail(
                    state,
                    approval,
                    &format!("the commit step could not start: {error}"),
                )
                .await;
            }
            Ok(())
        }
        DesignApprovalRow::COMMITTING | DesignApprovalRow::COMMITTED => {
            let db_path = state.config().db_path.clone();
            let approval = approval.clone();
            let state_for_work = Arc::clone(state);
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let db = Db::open(&db_path).map_err(|error| error.to_string())?;
                let design = crate::task_creator::task_design_stage(&db, &approval.task_id)?
                    .ok_or("the task has no design stage")?;
                if !design.is_current() {
                    // The transition fired: the factory started from the
                    // verified commit.
                    db.in_immediate_transaction_if_needed(|db| {
                        db.advance_design_approval(
                            &approval.id,
                            &[DesignApprovalRow::COMMITTED, DesignApprovalRow::COMMITTING],
                            DesignApprovalRow::ENTERED,
                            &DesignApprovalUpdate::default(),
                        )?;
                        db.set_design_session_status(
                            &approval.task_id,
                            DesignSessionRow::HANDED_OFF,
                        )
                    })
                    .map_err(|error| error.to_string())?;
                    state_for_work.design.feed_changed(&approval.task_id);
                    return Ok(());
                }
                let latest = db
                    .latest_stage_run(&approval.task_id)
                    .map_err(|error| error.to_string())?;
                if let Some(run) = latest.filter(|run| {
                    run.kind == "post"
                        && run.stage == format!("{} commit", design.stage)
                        && matches!(run.status.as_str(), "failed" | "cancelled")
                }) {
                    let message = format!(
                        "the commit step ended {}: {}",
                        run.status,
                        run.feedback.or(run.result).unwrap_or_default()
                    );
                    db.advance_design_approval(
                        &approval.id,
                        &[DesignApprovalRow::COMMITTING],
                        DesignApprovalRow::FAILED,
                        &DesignApprovalUpdate {
                            error: Some(&message),
                            ..Default::default()
                        },
                    )
                    .map_err(|error| error.to_string())?;
                    state_for_work.design.feed_changed(&approval.task_id);
                }
                Ok(())
            })
            .await
            .map_err(|error| error.to_string())?
        }
        _ => Ok(()),
    }
}

async fn record_waiting(
    state: &Arc<AppState>,
    approval: &DesignApprovalRow,
    reason: &str,
) -> Result<(), String> {
    if approval.error.as_deref() == Some(reason) {
        return Ok(());
    }
    let db_path = state.config().db_path.clone();
    let id = approval.id.clone();
    let phase = approval.phase.clone();
    let reason = reason.to_string();
    tokio::task::spawn_blocking(move || {
        let db = Db::open(&db_path).map_err(|error| error.to_string())?;
        db.advance_design_approval(
            &id,
            &[phase.as_str()],
            &phase,
            &DesignApprovalUpdate {
                error: Some(&reason),
                ..Default::default()
            },
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())??;
    state.design.feed_changed(&approval.task_id);
    Ok(())
}

async fn fail(state: &Arc<AppState>, approval: &DesignApprovalRow, message: &str) {
    let db_path = state.config().db_path.clone();
    let id = approval.id.clone();
    let message = message.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        Db::open(&db_path).and_then(|db| {
            db.advance_design_approval(
                &id,
                &[DesignApprovalRow::COMMITTING, DesignApprovalRow::EXPORTED],
                DesignApprovalRow::FAILED,
                &DesignApprovalUpdate {
                    error: Some(&message),
                    ..Default::default()
                },
            )
        })
    })
    .await;
    state.design.feed_changed(&approval.task_id);
}

/// Write the files the policy keeps into the task's worktree, with the
/// snapshot's identity now known, and record their digests.
fn export_retained(state: &AppState, approval: &DesignApprovalRow) -> Result<(), String> {
    let db_path = state.config().db_path.clone();
    let db = Db::open(&db_path).map_err(|error| error.to_string())?;
    let task_id = &approval.task_id;
    let policy: BoundPolicy =
        serde_json::from_str(&approval.policy_json).map_err(|error| error.to_string())?;
    let mut retained = Vec::new();
    if !policy.files.is_empty() {
        let worktree = db
            .get_task_worktree_path(task_id)
            .map_err(|error| error.to_string())?
            .ok_or("the task has no worktree")?;
        let snapshot = candidates_dir(&db_path, task_id)
            .ok_or("the task directory is unavailable")?
            .join(&approval.id);
        let design_md =
            std::fs::read(snapshot.join("design.md")).map_err(|error| error.to_string())?;
        let view = service::view(&db, &state.design, &db_path, task_id, false)
            .map_err(|error| error.message().to_string())?;
        let title = task_title(&db, task_id).map_err(|error| error.message().to_string())?;
        let summary = export::summary_markdown(
            &export::SummaryFacts {
                title: &title,
                task_id,
                workflow_stage: &view.stage,
                position: &view.position,
                epoch: approval.epoch,
                doc_revision: approval.doc_revision,
                doc_sha256: &approval.doc_sha256,
                source_commit: approval.source_commit.as_deref().unwrap_or("unknown"),
                artifact: approval
                    .artifact_repo_id
                    .as_deref()
                    .zip(approval.artifact_id.as_deref()),
                approved_at: approval.approved_at.as_deref(),
                next_stage: view.next_stage.as_deref(),
            },
            &approved_threads(&view.threads),
        );
        for (path, bytes) in [
            (format!("{}/design.md", policy.path), design_md),
            (format!("{}/SUMMARY.md", policy.path), summary.into_bytes()),
        ] {
            let target = Path::new(&worktree).join(&path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::write(&target, &bytes).map_err(|error| format!("writing {path}: {error}"))?;
            retained.push(RetainedFile {
                sha256: sha256_hex(&bytes),
                path,
            });
        }
    }
    let retained_json = serde_json::to_string(&retained).map_err(|error| error.to_string())?;
    db.advance_design_approval(
        &approval.id,
        &[DesignApprovalRow::APPROVED],
        DesignApprovalRow::EXPORTED,
        &DesignApprovalUpdate {
            retained_json: Some(&retained_json),
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())?;
    state.design.feed_changed(task_id);
    state.design.wake_delivery();
    Ok(())
}

/// The threads as the approved record keeps them (all of them: unresolved
/// feedback stays attached to what was approved, never silently dropped).
fn approved_threads(threads: &[ThreadView]) -> Vec<ThreadView> {
    threads.to_vec()
}

#[cfg(test)]
#[path = "approval_tests.rs"]
mod tests;
