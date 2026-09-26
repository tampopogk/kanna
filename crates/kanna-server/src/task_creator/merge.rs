use super::definitions::{
    parse_stored_workflow_definition, RepoDefinitions, WorkflowRouting, RELEASE_WORKFLOW_NAME,
};
use super::lifecycle::spawn_prepared_task_for_api_recording_stage_run;
use super::types::{PreparedTaskSpawn, TaskCreationRequest};
use crate::config::Config;
use crate::daemon_client::DaemonClient;
use crate::db::{Db, Repo};
use crate::mutation_provenance::ChannelIdentity;

pub async fn run_merge_agent(
    db: &Db,
    daemon: &mut DaemonClient,
    config: &Config,
    source_task_id: &str,
) -> Result<String, String> {
    let prepared = prepare_merge_agent_for_api(db, config, source_task_id)?;
    spawn_merge_agent_task(&config.db_path, daemon, prepared).await
}

pub(crate) fn prepare_merge_agent_for_api(
    db: &Db,
    config: &Config,
    source_task_id: &str,
) -> Result<PreparedTaskSpawn, String> {
    let repo = load_merge_source_repo(db, source_task_id)?;
    let request = build_merge_task_request(&repo)?;
    super::prepare_task_spawn(db, config, &repo, request)
}

async fn spawn_merge_agent_task(
    db_path: &str,
    daemon: &mut DaemonClient,
    prepared: PreparedTaskSpawn,
) -> Result<String, String> {
    let created =
        spawn_prepared_task_for_api_recording_stage_run(db_path, daemon, prepared).await?;
    Ok(created.task_id)
}

fn load_merge_source_repo(db: &Db, source_task_id: &str) -> Result<Repo, String> {
    let source_task = db
        .get_pipeline_item(source_task_id)
        .map_err(|e| format!("db error: {}", e))?
        .ok_or_else(|| format!("task not found: {}", source_task_id))?;
    db.get_repo(&source_task.repo_id)
        .map_err(|e| format!("db error: {}", e))?
        .ok_or_else(|| format!("repo not found for task: {}", source_task_id))
}

/// The merge master's pinned workflow name. It is the durable marker of the
/// account-wide merge singleton (`directory_singleton_agent`): it travels with
/// the task row, and every surface that tells a singleton apart reads it. The
/// definition pinned under it is the repository's release workflow.
pub(crate) const MERGE_SINGLETON_WORKFLOW: &str = "singleton-merge";
pub(super) const MERGE_AGENT: &str = "merge";

/// The definition a merge master is claimed onto: the repository's release
/// workflow (spec §10), whose first stage is the merge window. A definition
/// whose first stage does not run the merge agent is refused rather than
/// pinned, because the merge singleton is found by that stage's agent.
pub(super) fn merge_singleton_workflow_definition(repo: &Repo) -> Result<String, String> {
    let workflow = RepoDefinitions::resolve(repo)?.workflow(RELEASE_WORKFLOW_NAME)?;
    let first = workflow
        .stages
        .first()
        .ok_or_else(|| format!("the {RELEASE_WORKFLOW_NAME} workflow has no stages"))?;
    if first.agent.as_deref() != Some(MERGE_AGENT) {
        return Err(format!(
            ".kanna/workflows/{RELEASE_WORKFLOW_NAME}.json must open with the merge window: its \
             first stage '{}' runs {} instead of the {MERGE_AGENT} agent; no merge master was \
             created",
            first.name,
            first.agent.as_deref().unwrap_or("no agent"),
        ));
    }
    serde_json::to_string(&workflow).map_err(|e| format!("serialize error: {}", e))
}

fn build_merge_task_request(repo: &Repo) -> Result<TaskCreationRequest, String> {
    let workflow_name = MERGE_SINGLETON_WORKFLOW.to_string();
    let workflow_def = merge_singleton_workflow_definition(repo)?;
    Ok(TaskCreationRequest {
        requested_task_id: None,
        create_intent_json: None,
        task_prompt: String::new(),
        display_name: Some("Merge Master".to_string()),
        workflow_name: Some(workflow_name),
        workflow_def: Some(workflow_def),
        base_ref: None,
        stored_base_ref: None,
        stage_override: None,
        agent: None,
        explicit_provider: None,
        default_provider: None,
        agent_type: None,
        initial_terminal_geometry: None,
        model: None,
        effort: None,
        permission_mode: None,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        max_turns: None,
        max_budget_usd: None,
        setup_cmds: Vec::new(),
        task_template: None,
        resume_session_id: None,
        recovery_snapshot: None,
        transfer_import: None,
        notify_task_id: None,
        parent_task_id: None,
    })
}

/// What one look at a merge master found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MergeSingletonMigration {
    /// Its pinned workflow is now the release workflow.
    Migrated,
    /// Nothing to do: closed, not a merge master, or already migrated.
    NotApplicable,
    /// Not at a quiescent boundary, or the release workflow would change what
    /// its session was started with. Left exactly as it was; a later look may
    /// migrate it.
    Deferred(String),
}

/// Move a merge master claimed under the synthetic one-stage workflow onto
/// the repository's release workflow, keeping its claim, stage, runs and
/// conversation.
///
/// Only the pinned definition changes, through the ordinary replacement
/// boundary: fenced on the exact definition read (a concurrent change wins and
/// this does nothing), recorded as a workflow change with the server's
/// channel, and validated so the current stage keeps its name and role. It
/// happens at a quiescent boundary of the task's *workflow*, or not at all:
/// the caller holds the task-mutation lease (so no transition, revision,
/// workflow edit or handoff delivery interleaves), no stage run is running
/// (the replacement validator refuses a routing switch under a running run),
/// no ledger continuation is owed, no lifecycle operation is in flight, and
/// the merge window's agent, prompt, provider and environment are unchanged,
/// so no run is superseded. A turn in progress in the merge master's session
/// is deliberately not excluded -- a handoff starts one without reopening the
/// run, and nothing the replacement changes reaches the session: its stage,
/// run, conversation and claim stay as they are, and a result it records
/// afterwards parks at the manual merge window exactly as before.
/// Running it again finds the release workflow pinned and does nothing.
pub(crate) fn migrate_merge_singleton_to_release_workflow(
    db: &Db,
    db_path: &str,
    task_id: &str,
) -> Result<MergeSingletonMigration, String> {
    let db_error = |error: rusqlite::Error| format!("db error: {error}");
    let Some(item) = db.get_pipeline_item(task_id).map_err(db_error)? else {
        return Ok(MergeSingletonMigration::NotApplicable);
    };
    if item.closed_at.is_some() || item.pipeline.as_deref() != Some(MERGE_SINGLETON_WORKFLOW) {
        return Ok(MergeSingletonMigration::NotApplicable);
    }
    let (Some(stage), Some(previous)) = (item.stage.as_deref(), item.pipeline_def.as_deref())
    else {
        return Ok(MergeSingletonMigration::NotApplicable);
    };
    let prior = parse_stored_workflow_definition(previous)?;
    // The synthetic workflow every merge master was claimed under until now:
    // one legacy stage running the merge agent. Anything else was already
    // migrated, or was pinned by hand and is not this migration's to touch.
    let synthetic = prior.routing == WorkflowRouting::Legacy
        && prior.stages.len() == 1
        && prior.stages[0].name == stage
        && prior.stages[0].agent.as_deref() == Some(MERGE_AGENT)
        && prior.stages[0].post.is_none();
    if !synthetic {
        return Ok(MergeSingletonMigration::NotApplicable);
    }
    let runs = db.list_stage_runs_for_task(task_id).map_err(db_error)?;
    if runs.iter().any(|run| run.status == "running") {
        return Ok(MergeSingletonMigration::Deferred(
            "a stage run is running".to_string(),
        ));
    }
    if db.has_ledger_continuation(task_id).map_err(db_error)? {
        return Ok(MergeSingletonMigration::Deferred(
            "a transition is still owed".to_string(),
        ));
    }
    if db
        .list_lifecycle_operation_intents()
        .map_err(db_error)?
        .iter()
        .any(|intent| intent.task_id == task_id)
    {
        return Ok(MergeSingletonMigration::Deferred(
            "a lifecycle operation is in flight".to_string(),
        ));
    }
    let repo = db
        .get_repo(&item.repo_id)
        .map_err(db_error)?
        .ok_or_else(|| format!("repo not found for task {task_id}: {}", item.repo_id))?;
    let release: serde_json::Value =
        serde_json::from_str(&merge_singleton_workflow_definition(&repo)?)
            .map_err(|e| format!("invalid release workflow: {e}"))?;
    let validated =
        super::validate_task_workflow_replacement(&repo, &release, previous, stage, &runs)?;
    if !validated.superseded_run_ids.is_empty()
        || validated
            .changed_execution_stages
            .iter()
            .any(|changed| changed == stage)
    {
        return Ok(MergeSingletonMigration::Deferred(format!(
            "the release workflow's stage '{stage}' changes what this merge master's session \
             was started with"
        )));
    }
    let snapshot = validated.snapshot;
    let changed = db
        .replace_task_workflow(
            task_id,
            stage,
            MERGE_SINGLETON_WORKFLOW,
            &snapshot.definition_json,
            item.revision_rounds,
            snapshot.revision_limit,
            Some(crate::db::WorkflowReplacement {
                expected_definition: previous,
                source: "unspecified",
                superseded_run_ids: &[],
                changed_execution_stages: &validated.changed_execution_stages,
                ledger_result_id: None,
            }),
            &ChannelIdentity::Server,
        )
        .map_err(|error| format!("merge master {task_id} was not migrated: {error}"))?;
    if !changed {
        return Ok(MergeSingletonMigration::NotApplicable);
    }
    crate::task_store::flush_task_best_effort(db, db_path, task_id);
    Ok(MergeSingletonMigration::Migrated)
}
