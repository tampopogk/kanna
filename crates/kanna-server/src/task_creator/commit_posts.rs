//! Legacy commit posts become commit steps (spec §5, §15, §16.11; T13d).
//!
//! A legacy workflow commits through a post named `commit` on the stage whose
//! work it commits. The spec retires posts as a stage-like concept: the
//! commit is a property of the stage's transition (`exit_commit`, T3), which
//! runs the same post delivery machinery and also records what it committed.
//! An open task's pinned workflow is migrated from the one to the other by
//! the engine, once, at a quiescent boundary, as an ordinary workflow
//! replacement with engine provenance; nothing else about the task changes.
//! Every other post (an `approve` post, a custom one) stays, and runs through
//! the legacy post adapter as before.

use super::definitions::{parse_stored_workflow_definition, WorkflowDefinition};
use crate::db::Db;
use crate::mutation_provenance::{ChannelIdentity, ENGINE_DECLARED_ROLE};

/// The name of the post a legacy workflow commits through.
pub(crate) const LEGACY_COMMIT_POST: &str = "commit";

/// What one look at a task found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommitPostMigration {
    /// Its pinned workflow now commits these stages' work through their
    /// transitions' commit steps.
    Migrated(Vec<String>),
    /// Nothing to do: closed, or its workflow has no commit post.
    NotApplicable,
    /// Keeps the legacy commit post for good: the post has recorded runs,
    /// and a replacement never drops what a recorded run names.
    KeptLegacy(String),
    /// Not at a quiescent boundary now; left exactly as it was. The next
    /// look may migrate it.
    Deferred(String),
}

/// `definition` with every legacy commit post replaced by its stage's
/// `exit_commit`, and the stages that changed; `None` when it has none.
pub(crate) fn with_commit_steps(
    definition: &WorkflowDefinition,
) -> Option<(WorkflowDefinition, Vec<String>)> {
    let mut migrated = definition.clone();
    let mut stages = Vec::new();
    for stage in &mut migrated.stages {
        if stage
            .post
            .as_ref()
            .is_some_and(|post| post.name == LEGACY_COMMIT_POST)
        {
            stage.post = None;
            stage.exit_commit = true;
            stages.push(stage.name.clone());
        }
    }
    (!stages.is_empty()).then_some((migrated, stages))
}

/// Migrate `task_id`'s commit posts to commit steps, if it is at a quiescent
/// boundary. The caller holds the task's mutation lease, so no transition,
/// revision, workflow edit or delivery interleaves. Quiescent means: no run
/// is running, no transition or lifecycle operation is owed, no commit step
/// is in flight, the task is at a stage (not parked at a post), and no
/// transfer holds its workflow (checked in the replacing transaction). The
/// replacement goes through the ordinary validator, so it keeps every
/// recorded run's stage and supersedes no run, and is fenced on the exact
/// definition read. Running it again finds nothing to do.
pub(crate) fn migrate_commit_posts_to_exit_commit(
    db: &Db,
    db_path: &str,
    task_id: &str,
) -> Result<CommitPostMigration, String> {
    let db_error = |error: rusqlite::Error| format!("db error: {error}");
    let Some(item) = db.get_pipeline_item(task_id).map_err(db_error)? else {
        return Ok(CommitPostMigration::NotApplicable);
    };
    if item.closed_at.is_some() {
        return Ok(CommitPostMigration::NotApplicable);
    }
    let (Some(stage), Some(previous)) = (item.stage.as_deref(), item.pipeline_def.as_deref())
    else {
        return Ok(CommitPostMigration::NotApplicable);
    };
    let prior = parse_stored_workflow_definition(previous)?;
    let Some((migrated, stages)) = with_commit_steps(&prior) else {
        return Ok(CommitPostMigration::NotApplicable);
    };
    let runs = db.list_stage_runs_for_task(task_id).map_err(db_error)?;
    if runs.iter().any(|run| run.stage == LEGACY_COMMIT_POST) {
        return Ok(CommitPostMigration::KeptLegacy(
            "its commit post has recorded runs".to_string(),
        ));
    }
    if !prior.stages.iter().any(|candidate| candidate.name == stage) {
        return Ok(CommitPostMigration::Deferred(format!(
            "it is parked at post '{stage}'"
        )));
    }
    if runs.iter().any(|run| run.status == "running") {
        return Ok(CommitPostMigration::Deferred(
            "a stage run is running".to_string(),
        ));
    }
    if db.has_ledger_continuation(task_id).map_err(db_error)? {
        return Ok(CommitPostMigration::Deferred(
            "a transition is still owed".to_string(),
        ));
    }
    if db
        .list_lifecycle_operation_intents()
        .map_err(db_error)?
        .iter()
        .any(|intent| intent.task_id == task_id)
    {
        return Ok(CommitPostMigration::Deferred(
            "a lifecycle operation is in flight".to_string(),
        ));
    }
    for run in &runs {
        if db
            .task_transition_commit(task_id, &run.id)
            .map_err(db_error)?
            .is_some_and(|commit| commit.state == crate::db::TransitionCommit::REQUESTED)
        {
            return Ok(CommitPostMigration::Deferred(
                "a commit step is in flight".to_string(),
            ));
        }
    }
    let repo = db
        .get_repo(&item.repo_id)
        .map_err(db_error)?
        .ok_or_else(|| format!("repo not found for task {task_id}: {}", item.repo_id))?;
    let value = serde_json::to_value(&migrated).map_err(|error| error.to_string())?;
    let validated =
        super::validate_task_workflow_replacement(&repo, &value, previous, stage, &runs)?;
    if !validated.superseded_run_ids.is_empty() {
        return Ok(CommitPostMigration::Deferred(format!(
            "the replacement would supersede runs {:?}",
            validated.superseded_run_ids
        )));
    }
    let snapshot = validated.snapshot;
    let workflow_name = item.pipeline.clone().unwrap_or_default();
    let replaced = db.with_immediate_transaction(|db| {
        if let Err(error) = db.refuse_while_transferring(task_id) {
            return Ok(Err(error.to_string()));
        }
        db.replace_task_workflow(
            task_id,
            stage,
            &workflow_name,
            &snapshot.definition_json,
            item.revision_rounds,
            snapshot.revision_limit,
            Some(crate::db::WorkflowReplacement {
                expected_definition: previous,
                source: ENGINE_DECLARED_ROLE,
                superseded_run_ids: &[],
                changed_execution_stages: &validated.changed_execution_stages,
                ledger_result_id: None,
            }),
            &ChannelIdentity::Server,
        )
        .map(Ok)
    });
    match replaced {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return Ok(CommitPostMigration::NotApplicable),
        Ok(Err(transferring)) => return Ok(CommitPostMigration::Deferred(transferring)),
        Err(error) => {
            return Err(format!(
                "task {task_id}'s commit post was not migrated: {error}"
            ))
        }
    }
    crate::task_store::flush_task_best_effort(db, db_path, task_id);
    Ok(CommitPostMigration::Migrated(stages))
}
