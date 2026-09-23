//! Stage workspaces and session identity (spec §6, §16.2 — component T2).
//!
//! A stage owns a workspace: the directory its sessions run in. Entering a
//! stage for the first time forks a new directory from the committed input
//! the triggering result recorded; re-entering it through a loop reuses the
//! stage's retained directory and checks out a newly allocated branch there,
//! so a provider conversation keyed by working directory can resume. Every
//! session — whether it was started by a stage entry, a revision, a rerun or
//! a recovery — records the same identity on its run: the workspace, the
//! branch it checked out, a stage-scoped name, and where its provider
//! transcript lives.
//!
//! The rules that keep this safe:
//!
//! - Branch numbers come from the task's persisted counter
//!   ([`super::worktree::allocate_task_branch`]) and are spent before any git
//!   work.
//! - A workspace's directory is found from its record, never by appending
//!   the current branch to a path: after a loop the branch and the directory
//!   name differ.
//! - A retained directory is reused only when that moves nothing it holds.
//!   Uncommitted changes or commits the input lacks are preserved, reported
//!   on the session, and the stage gets a fresh directory instead; nothing is
//!   reset and nothing is merged.

use crate::config::Config;
use crate::db::{Db, StageRunSession, TranscriptRef};
use std::path::Path;

use super::worktree::{
    allocate_task_branch, is_ancestor, resolve_commit, workspace_git_state, WorkspaceGitState,
};

/// Stable identity of a stage workspace: its directory name, which is the
/// branch the workspace was created on and unique within the repository.
pub(crate) fn stage_workspace_id(worktree_path: &str) -> String {
    let name = Path::new(worktree_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(worktree_path);
    format!("ws-{name}")
}

/// The directory the task's current workspace lives in: the worktree record.
/// The branch-derived path is only the fallback for a task that has no
/// record at all, which is how every task was addressed before loops could
/// check a new branch out in an old directory.
pub(crate) fn current_workspace_path(
    db: &Db,
    repo_path: &str,
    task_id: &str,
    branch: &str,
) -> String {
    db.get_task_worktree_path(task_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| format!("{repo_path}/.kanna-worktrees/{branch}"))
}

/// Stage-scoped session name (spec §6): the stage, then the task's title.
/// It names the session, never the task.
pub(crate) fn session_name(stage: &str, title: &str) -> String {
    let mut stage_chars = stage.chars();
    let stage = match stage_chars.next() {
        Some(first) => first.to_uppercase().chain(stage_chars).collect::<String>(),
        None => String::new(),
    };
    format!("{stage}: {title}")
}

const SESSION_TITLE_LIMIT: usize = 80;

fn task_title(db: &Db, task_id: &str) -> String {
    let item = db.get_pipeline_item(task_id).ok().flatten();
    let title = item.as_ref().and_then(|item| {
        item.display_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| {
                item.prompt
                    .as_deref()
                    .and_then(|prompt| prompt.lines().find(|line| !line.trim().is_empty()))
                    .map(|line| line.trim().to_string())
            })
    });
    let title = title.unwrap_or_else(|| task_id.to_string());
    if title.chars().count() <= SESSION_TITLE_LIMIT {
        return title;
    }
    let mut bounded = title
        .chars()
        .take(SESSION_TITLE_LIMIT - 1)
        .collect::<String>();
    bounded.push('…');
    bounded
}

/// The identity a session starting in `worktree_path` on `branch` records.
/// The transcript reference is added when the run is recorded, once the
/// provider session id is final.
pub(crate) fn session_identity(
    db: &Db,
    task_id: &str,
    stage: &str,
    worktree_path: &str,
    branch: &str,
    workspace_report: Option<String>,
) -> StageRunSession {
    StageRunSession {
        workspace_id: Some(stage_workspace_id(worktree_path)),
        branch: Some(branch.to_string()),
        name: Some(session_name(stage, &task_title(db, task_id))),
        transcript: None,
        workspace_report,
    }
}

/// The identity of a session that starts in a workspace it does not move:
/// a rerun, a restart, or a recreated creation spawn. Its branch is whatever
/// that directory has checked out.
pub(crate) fn current_session_identity(
    db: &Db,
    task_id: &str,
    stage: &str,
    worktree_path: &str,
    fallback_branch: &str,
) -> StageRunSession {
    let branch =
        super::resume::current_branch(worktree_path).unwrap_or_else(|| fallback_branch.to_string());
    session_identity(db, task_id, stage, worktree_path, &branch, None)
}

/// Where a provider keeps the transcript of `provider_session_id`. The path
/// is filled only where the provider's layout makes it a function of the
/// session id and working directory; the reference is recorded either way.
pub(crate) fn transcript_ref(
    provider: &str,
    provider_session_id: Option<&str>,
    cwd: &str,
) -> Option<TranscriptRef> {
    let session_id = provider_session_id?.to_string();
    let path = (provider == "claude")
        .then(super::resume::claude_projects_dir)
        .flatten()
        .map(|projects| {
            projects
                .join(super::resume::claude_project_slug(cwd))
                .join(format!("{session_id}.jsonl"))
                .to_string_lossy()
                .to_string()
        });
    Some(TranscriptRef {
        provider: provider.to_string(),
        session_id,
        path,
    })
}

/// The one place a starting session is recorded, whichever path started it:
/// its stage workspace row (the directory the stage may be re-entered in)
/// and its identity on the run. Runs inside the caller's transaction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_session_start(
    db: &Db,
    run_id: &str,
    task_id: &str,
    stage: &str,
    cwd: &str,
    agent_provider: &str,
    provider_session_id: Option<&str>,
    session: &StageRunSession,
) -> rusqlite::Result<()> {
    let mut session = session.clone();
    session.transcript = transcript_ref(agent_provider, provider_session_id, cwd);
    if let (Some(workspace_id), Some(branch)) =
        (session.workspace_id.as_deref(), session.branch.as_deref())
    {
        db.upsert_stage_workspace(workspace_id, task_id, stage, cwd, branch)?;
    }
    db.set_stage_run_session(run_id, &session)
}

/// The committed input a new session of `stage` takes: the SHA the
/// triggering result recorded (spec §6), when it names a commit this
/// repository has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StageInput {
    pub(crate) commit: String,
    pub(crate) result_id: String,
}

pub(crate) fn stage_input(
    config: &Config,
    db: &Db,
    repo_path: &str,
    task_id: &str,
    stage: &str,
) -> Option<StageInput> {
    let task_dir = crate::task_store::task_dir_for(db, &config.db_path, task_id)?;
    let trigger = crate::task_store::resolve_trigger(&task_dir, stage)?;
    let recorded = trigger.committed_sha.as_deref()?;
    let commit = resolve_commit(repo_path, recorded)?;
    Some(StageInput {
        commit,
        result_id: trigger.entry_id,
    })
}

/// What a fork from `input` leaves behind in the task's current workspace:
/// commits the recorded input does not contain stay on that workspace's
/// branch and are reported, never silently chosen or dropped.
pub(crate) fn fork_input_report(
    repo_path: &str,
    current_workspace: &str,
    input: &StageInput,
) -> Option<String> {
    let state = workspace_git_state(current_workspace).ok()?;
    if is_ancestor(repo_path, &state.head, &input.commit) {
        return None;
    }
    Some(format!(
        "workspace {current_workspace} ({}) is at {} which the recorded input {} ({}) does \
         not contain; those commits stay there and were not carried into this stage",
        state.branch.as_deref().unwrap_or("detached HEAD"),
        short(&state.head),
        short(&input.commit),
        input.result_id,
    ))
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// A loop back into a stage: reuse its retained directory with a new
/// branch, or say why it cannot be reused.
pub(super) enum RevisitPlan {
    Reuse(super::types::RevisitWorkspaceSpec),
    /// The stage gets a fresh directory. `reason` is recorded as the
    /// resume fallback; `report` names retained state that was preserved.
    Fresh {
        reason: String,
        report: Option<String>,
    },
}

/// The stage's retained directory: its newest recorded workspace still on
/// disk, or, for a task that forked before workspaces were recorded, the
/// directory its latest main run ran in.
fn retained_stage_directory(db: &Db, task_id: &str, stage: &str) -> Result<Option<String>, String> {
    let recorded = db
        .list_stage_workspaces(task_id)
        .map_err(|error| format!("db error: {error}"))?
        .into_iter()
        .rev()
        .find(|workspace| workspace.stage == stage && Path::new(&workspace.path).is_dir())
        .map(|workspace| workspace.path);
    if recorded.is_some() {
        return Ok(recorded);
    }
    Ok(db
        .latest_stage_run_for_stage(task_id, stage, "main")
        .map_err(|error| format!("db error: {error}"))?
        .and_then(|run| run.cwd)
        .filter(|cwd| Path::new(cwd).is_dir()))
}

/// Decide how a loop back into `stage` starts (spec §6 "Loop back").
///
/// The input is the triggering result's recorded commit, else the task's
/// current workspace HEAD. The retained directory is reused when it is at
/// the input (uncommitted changes stay in place and are reported) or clean
/// and behind it. Any other state — uncommitted changes on a different
/// commit, or commits the input lacks — is preserved untouched and reported,
/// and the stage forks a fresh directory from the input instead.
pub(super) fn plan_stage_revisit(
    config: &Config,
    db: &Db,
    repo_path: &str,
    task_id: &str,
    stage: &str,
    current_workspace: &str,
) -> Result<RevisitPlan, String> {
    let Some(directory) = retained_stage_directory(db, task_id, stage)? else {
        return Ok(RevisitPlan::Fresh {
            reason: format!("no retained workspace for stage '{stage}'"),
            report: None,
        });
    };
    let state = match workspace_git_state(&directory) {
        Ok(state) => state,
        Err(error) => {
            return Ok(RevisitPlan::Fresh {
                reason: format!("retained workspace {directory} is unreadable: {error}"),
                report: None,
            })
        }
    };
    let input = stage_input(config, db, repo_path, task_id, stage)
        .map(|input| input.commit)
        .or_else(|| {
            workspace_git_state(current_workspace)
                .ok()
                .map(|current| current.head)
        })
        .unwrap_or_else(|| state.head.clone());
    let WorkspaceGitState {
        head,
        branch: previous_branch,
        dirty,
    } = state;
    let preserved = |what: &str| {
        format!(
            "retained workspace {directory} ({}) {what}; it was preserved untouched",
            previous_branch.as_deref().unwrap_or("detached HEAD"),
        )
    };
    let report = if head == input {
        dirty.then(|| {
            format!(
                "retained workspace {directory} had uncommitted changes; they were kept in place \
                 on the new branch"
            )
        })
    } else if dirty {
        let report = preserved(&format!(
            "has uncommitted changes and is at {} rather than the input {}",
            short(&head),
            short(&input)
        ));
        return Ok(RevisitPlan::Fresh {
            reason: report.clone(),
            report: Some(report),
        });
    } else if is_ancestor(repo_path, &head, &input) {
        None
    } else {
        let report = preserved(&format!(
            "holds commits the input {} does not contain (HEAD {})",
            short(&input),
            short(&head)
        ));
        return Ok(RevisitPlan::Fresh {
            reason: report.clone(),
            report: Some(report),
        });
    };
    let branch = allocate_task_branch(db, repo_path, task_id)?;
    Ok(RevisitPlan::Reuse(super::types::RevisitWorkspaceSpec {
        worktree_path: directory,
        branch,
        start_point: input,
        previous_branch,
        previous_head: head,
        observed_dirty: dirty,
        report,
        resume: None,
    }))
}
