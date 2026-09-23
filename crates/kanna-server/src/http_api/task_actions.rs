use super::lan_trust::PrivilegedTaskAccess;
use super::state::{db_write_error, AppState};
use super::task_blockers::{
    resolve_existing_task_id, start_dependents_unblocked_by_close_with_daemon,
};
use super::task_input::submit_task_input;
use crate::db::{Db, StageTrigger};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kanna_agent_protocol::StateChangeScope;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn stage_action_error_status(error: &str) -> axum::http::StatusCode {
    if error.starts_with("task is blocked:") || error.starts_with("post is still running") {
        axum::http::StatusCode::CONFLICT
    } else if error.starts_with("cannot apply a provider override") {
        axum::http::StatusCode::BAD_REQUEST
    } else {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn resume_action_error_status(error: &str) -> axum::http::StatusCode {
    if error.starts_with("task is closed:")
        || error.starts_with("task has no stage run to resume:")
        || error.starts_with("latest run is ")
        || error.starts_with("latest interrupted run is not the task's current stage:")
    {
        axum::http::StatusCode::CONFLICT
    } else {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn reject_unprepared_transfer(db: &crate::db::Db, task_id: &str) -> Result<(), String> {
    if let Some((_, _, _, bound_task, state)) = db
        .transferred_task_manifest_for_task(task_id)
        .map_err(|e| format!("db error: {e}"))?
    {
        if bound_task.as_deref() == Some(task_id) && state != "prepared" {
            return Err(format!(
                "transferred task {task_id} has no durable prepared proof"
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AdvanceStageRequest {
    expected_transition_revision: Option<String>,
    /// The pinned workflow the caller inspected before deciding to advance.
    /// A task whose stages can be published while an earlier stage runs can
    /// have a different tail by the time the advance arrives, so a caller that
    /// acted on a displayed stage sequence fences on it rather than accepting
    /// whichever one is pinned now — including one whose final stage would
    /// close the task.
    expected_definition: Option<serde_json::Value>,
    source: Option<String>,
    /// Provider the stage this advance enters must spawn with. It fills the
    /// explicit-override slot of the provider precedence chain, so it outranks
    /// that stage's own `agent_provider` selectors, the repo's
    /// `agentProviders`, the agent definition's frontmatter, and the default.
    #[serde(alias = "nextStageHarness")]
    next_stage_agent_provider: Option<String>,
    /// Model for `next_stage_agent_provider`, passed to that CLI verbatim.
    /// Meaningless — and refused — without the provider it belongs to.
    next_stage_model: Option<String>,
    /// Reasoning effort for `next_stage_agent_provider`, in that provider's
    /// own vocabulary.
    next_stage_effort: Option<String>,
    /// Who chose this override: `operator`, `manager`, or `agent`. Recorded
    /// without authentication, and separate from `source` because the agent
    /// that recommends a builder tier is usually not the one advancing the
    /// stage.
    next_stage_provider_source: Option<String>,
}

pub(super) async fn resolve_task_id_for_mutation(
    state: &Arc<AppState>,
    task_or_branch_id: &str,
) -> Result<String, (axum::http::StatusCode, String)> {
    let state = Arc::clone(state);
    let task_or_branch_id = task_or_branch_id.to_string();
    super::blocking::run_handler_blocking("task mutation identity", move || {
        let db = Db::open(&state.config.db_path).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {}", e),
            )
        })?;
        resolve_existing_task_id(&db, &task_or_branch_id)
    })
    .await
}

pub(super) async fn run_merge_agent(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    #[cfg(test)]
    if let Some(merge_agent_runner) = state.merge_agent_runner.clone() {
        return merge_agent_runner(task_id)
            .map(Json)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e));
    }

    let prepared = {
        let state = Arc::clone(&state);
        let task_id = task_id.clone();
        super::blocking::run_handler_blocking("merge agent prepare", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            crate::task_creator::prepare_merge_agent_for_api(&db, &state.config, &task_id)
                .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))
        })
        .await?
    };
    let mut daemon = crate::daemon_client::DaemonClient::connect(&state.config.daemon_dir)
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("daemon error: {}", e),
            )
        })?;
    let created_task = crate::task_creator::spawn_prepared_task_for_api_recording_stage_run(
        &state.config.db_path,
        &mut daemon,
        prepared,
    )
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(crate::mobile_api::TaskActionResponse {
        task_id: created_task.task_id,
        follow_task: None,
        revision_budget: None,
        workflow_extended: None,
    }))
}

fn parent_chain_reaches(
    db: &Db,
    start: &str,
    target: &str,
) -> Result<bool, (axum::http::StatusCode, String)> {
    let mut current = Some(start.to_string());
    let mut steps = 0usize;
    while let Some(id) = current {
        if id == target {
            return Ok(true);
        }
        steps += 1;
        if steps > 10_000 {
            break;
        }
        current = db
            .pipeline_item_parent(&id)
            .map_err(|e| db_write_error("db error", e))?;
    }
    Ok(false)
}

pub(super) async fn set_task_parent(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    Json(payload): Json<crate::mobile_api::SetTaskParentRequest>,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
    })?;
    let task_id = resolve_existing_task_id(&db, &task_id)?;

    let parent_task_id = match payload.parent_task_id.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => {
            let parent_id = resolve_existing_task_id(&db, raw)?;
            if parent_id == task_id {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    "task cannot be its own parent".to_string(),
                ));
            }
            let task = db
                .get_pipeline_item(&task_id)
                .map_err(|e| db_write_error("db error", e))?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        format!("task not found: {task_id}"),
                    )
                })?;
            let parent = db
                .get_pipeline_item(&parent_id)
                .map_err(|e| db_write_error("db error", e))?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        format!("parent task not found: {parent_id}"),
                    )
                })?;
            if task.repo_id != parent.repo_id {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    "parent task belongs to a different repo".to_string(),
                ));
            }
            if parent_chain_reaches(&db, &parent_id, &task_id)? {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    "cannot set parent because it would create a subtask cycle".to_string(),
                ));
            }
            Some(parent_id)
        }
    };

    db.update_pipeline_item_parent(&task_id, parent_task_id.as_deref())
        .map_err(|e| db_write_error("db error", e))?;
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(crate::mobile_api::TaskActionResponse {
        task_id,
        follow_task: None,
        revision_budget: None,
        workflow_extended: None,
    }))
}

pub(super) async fn set_task_workflow(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    Json(payload): Json<crate::mobile_api::SetTaskWorkflowRequest>,
) -> Result<Json<crate::mobile_api::SetTaskWorkflowResponse>, (axum::http::StatusCode, String)> {
    let workflow_name = payload.workflow_name.trim().to_string();
    if workflow_name.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "workflowName must be non-empty".to_string(),
        ));
    }

    let task_id = resolve_task_id_for_mutation(&state, &task_id).await?;
    let _task_mutation = state.begin_requested_task_mutation(&task_id).await;
    let (response, changed) = {
        let state = Arc::clone(&state);
        super::blocking::run_handler_blocking("task workflow update", move || {
            let db = Db::open(&state.config.db_path).map_err(|error| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?;
            let item = db
                .get_pipeline_item(&task_id)
                .map_err(|error| db_write_error("db error", error))?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        format!("task not found: {task_id}"),
                    )
                })?;
            if item.closed_at.is_some() {
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    format!("cannot change workflow for closed task {task_id}"),
                ));
            }
            let stage = item.stage.clone().ok_or_else(|| {
                (
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "task {task_id} has no current stage to map into workflow {workflow_name}"
                    ),
                )
            })?;
            let repo = db
                .get_repo(&item.repo_id)
                .map_err(|error| db_write_error("db error", error))?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        format!("repo not found for task {task_id}: {}", item.repo_id),
                    )
                })?;
            let snapshot =
                crate::task_creator::resolve_task_workflow_snapshot(&repo, &workflow_name)
                    .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error))?;
            if !snapshot.stage_names.iter().any(|name| name == &stage) {
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "cannot change task {task_id} from workflow {} to {workflow_name}: \
                         current stage '{stage}' is not present in the new workflow (stages: {})",
                        item.pipeline.as_deref().unwrap_or("<none>"),
                        snapshot.stage_names.join(", ")
                    ),
                ));
            }

            let changed = db
                .update_pipeline_item_pipeline(
                    &task_id,
                    &stage,
                    &workflow_name,
                    &snapshot.definition_json,
                    item.revision_rounds,
                    snapshot.revision_limit,
                )
                .map_err(|error| db_write_error("db error", error))?;
            if changed {
                // An owed transition was computed against the old workflow.
                db.clear_ledger_continuation(&task_id)
                    .map_err(|error| db_write_error("db error", error))?;
                crate::task_store::flush_task_best_effort(&db, &state.config.db_path, &task_id);
            }
            Ok((
                crate::mobile_api::SetTaskWorkflowResponse {
                    task_id,
                    legacy_pipeline_name: workflow_name.clone(),
                    workflow_name,
                    stage,
                    revision_rounds: item.revision_rounds,
                    revision_limit: snapshot.revision_limit,
                },
                changed,
            ))
        })
        .await?
    };
    if changed {
        state.publish_state_changed(StateChangeScope::Tasks);
    }
    Ok(Json(response))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReplaceTaskWorkflowRequest {
    workflow_definition: serde_json::Value,
    expected_definition: serde_json::Value,
    source: Option<String>,
}

pub(super) async fn replace_task_workflow(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    axum::extract::Query(local_only): axum::extract::Query<super::task_federation::LocalOnlyQuery>,
    Json(payload): Json<ReplaceTaskWorkflowRequest>,
) -> Result<Response, (axum::http::StatusCode, String)> {
    use axum::http::StatusCode;
    let source = payload.source.as_deref().unwrap_or("unspecified");
    if !["operator", "manager", "agent", "unspecified"].contains(&source) {
        return Err((
            StatusCode::BAD_REQUEST,
            "source must be operator, manager, agent, or unspecified (caller-declared)".into(),
        ));
    }
    let forward_body = serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null);
    let path = super::task_federation::task_path(&task_id, "/actions/replace-workflow");
    let task_id = match super::task_federation::resolve_task_route(
        &state,
        &task_id,
        local_only.local_only,
        "POST",
        &path,
        &forward_body,
    )
    .await?
    {
        super::task_federation::TaskRoute::Local(id) => id,
        super::task_federation::TaskRoute::Remote(response) => return Ok(response),
    };
    let _task_mutation = state.begin_requested_task_mutation(&task_id).await;
    let response = {
        let state = Arc::clone(&state);
        super::blocking::run_handler_blocking("task workflow replacement", move || {
            let db = Db::open(&state.config.db_path)
                .map_err(|error| db_write_error("db error", error))?;
            let item = db
                .get_pipeline_item(&task_id)
                .map_err(|error| db_write_error("db error", error))?
                .ok_or_else(|| (StatusCode::NOT_FOUND, format!("task not found: {task_id}")))?;
            if item.closed_at.is_some() {
                return Err((
                    StatusCode::CONFLICT,
                    "cannot replace a closed task's workflow".into(),
                ));
            }
            let stage = item
                .stage
                .as_deref()
                .ok_or_else(|| (StatusCode::CONFLICT, "task has no current stage".into()))?;
            let previous = item
                .pipeline_def
                .as_deref()
                .ok_or_else(|| (StatusCode::CONFLICT, "task has no pinned workflow".into()))?;
            let before: serde_json::Value = serde_json::from_str(previous).map_err(|error| {
                (
                    StatusCode::CONFLICT,
                    format!("invalid pinned workflow: {error}"),
                )
            })?;
            if before != payload.expected_definition {
                return Err((
                    StatusCode::CONFLICT,
                    "pinned workflow changed; read it again before replacing".into(),
                ));
            }
            let repo = db
                .get_repo(&item.repo_id)
                .map_err(|error| db_write_error("db error", error))?
                .ok_or_else(|| (StatusCode::NOT_FOUND, "task repository not found".into()))?;
            let runs = db
                .list_stage_runs_for_task(&task_id)
                .map_err(|error| db_write_error("db error", error))?;
            let validated = crate::task_creator::validate_task_workflow_replacement(
                &repo,
                &payload.workflow_definition,
                previous,
                stage,
                &runs,
            )
            .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
            let snapshot = validated.snapshot;
            let superseded = validated.superseded_run_ids;
            let changed = db
                .replace_task_workflow(
                    &task_id,
                    stage,
                    item.pipeline.as_deref().unwrap_or("no-review"),
                    &snapshot.definition_json,
                    item.revision_rounds,
                    snapshot.revision_limit,
                    Some(crate::db::WorkflowReplacement {
                        expected_definition: previous,
                        source: payload.source.as_deref().unwrap_or("unspecified"),
                        superseded_run_ids: &superseded,
                        changed_execution_stages: &validated.changed_execution_stages,
                        ledger_result_id: None,
                    }),
                )
                .map_err(|error| db_write_error("db error", error))?;
            if changed {
                // An owed transition was computed against the old workflow.
                db.clear_ledger_continuation(&task_id)
                    .map_err(|error| db_write_error("db error", error))?;
                crate::task_store::flush_task_best_effort(&db, &state.config.db_path, &task_id);
            }
            let definition_value = serde_json::from_str::<serde_json::Value>(
                &snapshot.definition_json,
            )
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("invalid stored workflow: {error}"),
                )
            })?;
            Ok(
                serde_json::json!({"taskId": task_id, "stage": stage, "changed": changed,
                "workflowDefinition": definition_value,
                "revisionLimit": snapshot.revision_limit, "revisionRounds": item.revision_rounds,
                "supersededRunIds": if changed { superseded } else { vec![] }}),
            )
        })
        .await?
    };
    if response["changed"] == true {
        state.publish_state_changed(StateChangeScope::Tasks);
    }
    Ok(Json(response).into_response())
}

/// How a blocker resolved — determines the wording dependents receive.
/// Passed explicitly because the close paths collect instructions before
/// `closed_at` is written, so the row itself cannot be trusted mid-close.
#[derive(Clone, Copy, PartialEq)]
enum BlockerResolution {
    Closed,
    PrCreated,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PinTaskRequest {
    position: Option<i64>,
}

pub(super) async fn pin_task(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    Json(payload): Json<PinTaskRequest>,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
    })?;
    let task_id = resolve_existing_task_id(&db, &task_id)?;
    if let Some(position) = payload.position {
        db.pin_pipeline_item(&task_id, position)
            .map_err(|e| db_write_error("db error", e))?;
    } else {
        let repo_id = db
            .get_pipeline_item(&task_id)
            .map_err(|e| db_write_error("db error", e))?
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::NOT_FOUND,
                    format!("task not found: {task_id}"),
                )
            })?
            .repo_id;
        db.pin_pipeline_item_at_top(&repo_id, &task_id)
            .map_err(|e| db_write_error("db error", e))?;
    }
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(crate::mobile_api::TaskActionResponse {
        task_id,
        follow_task: None,
        revision_budget: None,
        workflow_extended: None,
    }))
}

pub(super) async fn unpin_task(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
    })?;
    let task_id = resolve_existing_task_id(&db, &task_id)?;
    db.unpin_pipeline_item(&task_id)
        .map_err(|e| db_write_error("db error", e))?;
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(crate::mobile_api::TaskActionResponse {
        task_id,
        follow_task: None,
        revision_budget: None,
        workflow_extended: None,
    }))
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderPinnedTasksRequest {
    repo_id: String,
    ordered_ids: Vec<String>,
}

pub(super) async fn reorder_pinned_tasks(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ReorderPinnedTasksRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
    })?;
    db.reorder_pinned_items(&payload.repo_id, &payload.ordered_ids)
        .map_err(|e| db_write_error("db error", e))?;
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(
        serde_json::json!({ "updated": payload.ordered_ids.len() }),
    ))
}

/// Build per-dependent session messages announcing that a blocker at the
/// `pr` stage has resolved — either its PR was just created (optimistic
/// resolution: work committed, reviewed, and pushed, awaiting human merge)
/// or the task closed. Dependents that already have a workspace need to
/// pull the blocker's work in themselves, and the branch they need is the
/// blocker's *current* branch — the PR stage usually renames it away from
/// the stored fork name, so resolve it from the blocker's worktree HEAD.
fn collect_blocker_resolution_instructions(
    db: &Db,
    blocker_task_id: &str,
    resolution: BlockerResolution,
) -> Result<Vec<(String, String)>, (axum::http::StatusCode, String)> {
    let blocker_task_id = resolve_existing_task_id(db, blocker_task_id)?;
    let blocker = db
        .get_pipeline_item(&blocker_task_id)
        .map_err(|e| db_write_error("db error", e))?
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                format!("task not found: {blocker_task_id}"),
            )
        })?;
    if blocker.stage.as_deref() != Some("pr") {
        return Ok(Vec::new());
    }
    let repo = db
        .get_repo(&blocker.repo_id)
        .map_err(|e| db_write_error("db error", e))?
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                format!("repo not found for task: {blocker_task_id}"),
            )
        })?;
    let default_branch = repo.default_branch.unwrap_or_else(|| "main".to_string());
    let blocker_branch = blocker
        .branch
        .as_deref()
        .and_then(|branch| {
            crate::task_creator::resolve_current_source_worktree_branch(&repo.path, Some(branch))
        })
        .or(blocker.branch)
        .unwrap_or_else(|| blocker_task_id.to_string());
    db.update_pipeline_item_pr_branch(&blocker_task_id, &blocker_branch)
        .map_err(|error| db_write_error("db error", error))?;
    let blocker_title = blocker
        .display_name
        .unwrap_or_else(|| blocker_task_id.to_string());
    let pr_reference = blocker
        .pr_url
        .map(|url| format!(" (PR: {url})"))
        .unwrap_or_default();
    let status_sentence = match resolution {
        BlockerResolution::Closed => {
            format!("Blocker task \"{blocker_title}\" has finished its workflow and closed.")
        }
        BlockerResolution::PrCreated => format!(
            "Blocker task \"{blocker_title}\" has completed its work and opened a PR awaiting human review."
        ),
    };

    let mut instructions = Vec::new();
    for dependent_id in db
        .list_tasks_blocked_by(&blocker_task_id)
        .map_err(|e| db_write_error("db error", e))?
    {
        if db
            .get_task_worktree_path(&dependent_id)
            .map_err(|e| db_write_error("db error", e))?
            .is_none()
        {
            continue;
        }
        let Some(session_id) = db
            .resolve_task_terminal_session_id(&dependent_id)
            .map_err(|e| db_write_error("db error", e))?
        else {
            continue;
        };
        let message = format!(
            "{status_sentence} Its work is on branch `{blocker_branch}`{pr_reference}. Bring that work into this branch now: run `git fetch origin`, then rebase (or merge) this branch onto `{blocker_branch}` — or onto `{default_branch}` instead if that PR has already merged. Resolve conflicts if needed and continue your task."
        );
        instructions.push((session_id, message));
    }
    Ok(instructions)
}

/// Closes a task from inside the process, through the same action the route
/// serves.
///
/// The transfer engine closes the source task once the destination has
/// acknowledged the import. That has to be *this* close — WIP snapshotting,
/// session teardown, and blocker instructions
/// — not a second implementation that drifts from it.
pub(crate) async fn close_task_in_process(
    state: Arc<AppState>,
    task_id: String,
) -> Result<(), (axum::http::StatusCode, String)> {
    close_task(
        PrivilegedTaskAccess,
        State(state),
        axum::extract::Path(task_id),
    )
    .await
    .map(|_| ())
}

pub(super) async fn close_task(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    let task_id = resolve_task_id_for_mutation(&state, &task_id).await?;
    let _task_mutation = state.begin_requested_task_mutation(&task_id).await;

    #[cfg(test)]
    if let Some(task_closer) = state.task_closer.clone() {
        return task_closer(task_id)
            .map(|_| axum::http::StatusCode::NO_CONTENT)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e));
    }

    let (pipeline_item_id, blocker_close_instructions, workspace_teardown) = {
        let state = Arc::clone(&state);
        super::blocking::run_handler_blocking("task close prepare", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            let pipeline_item_id = db
                .resolve_pipeline_item_id(&task_id)
                .map_err(|e| {
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("db error: {}", e),
                    )
                })?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        format!("task not found: {}", task_id),
                    )
                })?;

            let open_children = db.count_open_children(&pipeline_item_id).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            if open_children > 0 {
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    "task has open subtasks; close or detach subtasks first".to_string(),
                ));
            }
            let blocker_close_instructions = collect_blocker_resolution_instructions(
                &db,
                &pipeline_item_id,
                BlockerResolution::Closed,
            )?;
            let workspace_teardown = crate::task_creator::prepare_workspace_teardown_for_close(
                &db,
                &state.config,
                &pipeline_item_id,
            );
            Ok((
                pipeline_item_id,
                blocker_close_instructions,
                workspace_teardown,
            ))
        })
        .await?
    };
    let has_workspace_teardown = workspace_teardown.is_some();

    let mut daemon = crate::daemon_client::DaemonClient::connect(&state.config.daemon_dir)
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("daemon error: {}", e),
            )
        })?;

    for (session_id, message) in blocker_close_instructions {
        if let Err((_, error)) = submit_task_input(&mut daemon, &session_id, &message).await {
            log::warn!(
                "failed to deliver blocker-close instructions to dependent session {session_id}: {error}"
            );
        }
    }

    for session_id in [
        pipeline_item_id.to_string(),
        format!("shell-wt-{pipeline_item_id}"),
    ] {
        crate::task_creator::kill_session_replacing(
            &mut daemon,
            &state.session_replacements,
            session_id.as_str(),
        )
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    crate::terminal_editor::close_task_editors(
        &mut daemon,
        &state.session_replacements,
        &pipeline_item_id,
    )
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let teardown_session_id = workspace_teardown
        .as_ref()
        .map(|teardown| teardown.session_id.clone())
        .unwrap_or_else(|| format!("td-{pipeline_item_id}"));
    if let Err(error) = crate::task_creator::kill_session_replacing(
        &mut daemon,
        &state.session_replacements,
        teardown_session_id.as_str(),
    )
    .await
    {
        log::warn!("failed to replace workspace teardown session {teardown_session_id}: {error}");
    }
    {
        // Closing snapshots dirty worktrees into WIP commits and removes
        // them — synchronous git work that must not run on a runtime worker.
        let state = Arc::clone(&state);
        let pipeline_item_id = pipeline_item_id.clone();
        super::blocking::run_handler_blocking("task close finalize", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            db.close_pipeline_item(&pipeline_item_id).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            if !has_workspace_teardown {
                if let Err(error) = crate::worktree_cleanup::cleanup_closed_task_worktrees_by_id(
                    &db,
                    &pipeline_item_id,
                ) {
                    // The close is already committed. Reconciliation retries
                    // leftover worktree cleanup; reporting a 500 here would
                    // falsely tell the caller that the close did not land.
                    log::warn!(
                        "post-close worktree cleanup failed for task {pipeline_item_id}: {error}"
                    );
                }
            }
            Ok(())
        })
        .await?
    };
    if has_workspace_teardown {
        crate::task_creator::spawn_prepared_workspace_teardown_best_effort(
            &mut daemon,
            workspace_teardown,
        )
        .await;
    }
    crate::task_creator::remove_completion_contexts(&state.config.daemon_dir, &pipeline_item_id);
    // Attachments belong to the task, not to a workspace, so they go here with
    // the task's other per-task on-disk artifacts rather than with a worktree.
    crate::task_input_attachments::remove_task_attachments(
        &state.config.db_path,
        &pipeline_item_id,
    );
    state.preview_sessions.revoke_task(&pipeline_item_id).await;
    start_dependents_unblocked_by_close_with_daemon(&state, &mut daemon, &pipeline_item_id).await;
    if let Err(error) =
        super::signal_agent::release_closed_singleton_reservation(&state, &pipeline_item_id).await
    {
        log::warn!(
            "closed task {pipeline_item_id}, but singleton reservation release failed: {error}"
        );
    }
    state.publish_state_changed(StateChangeScope::Tasks);
    state.publish_state_changed(StateChangeScope::Blockers);

    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub(super) async fn abort_task_creation(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    super::tasks::validate_requested_task_id(&task_id)?;
    let _operation = state.begin_requested_task_abort(&task_id).await;

    #[cfg(test)]
    if let Some(task_closer) = state.task_closer.clone() {
        return task_closer(task_id)
            .map(|_| axum::http::StatusCode::NO_CONTENT)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e));
    }

    let task = {
        let state = Arc::clone(&state);
        let lookup_task_id = task_id.clone();
        super::blocking::run_handler_blocking("task creation abort lookup", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            db.get_pipeline_item(&lookup_task_id).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })
        })
        .await?
    };
    if task.is_none() || task.is_some_and(|task| task.closed_at.is_some()) {
        return Ok(axum::http::StatusCode::NO_CONTENT);
    }

    close_task(
        PrivilegedTaskAccess,
        State(state),
        axum::extract::Path(task_id),
    )
    .await
}

pub(super) async fn reopen_task(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    let task_id = {
        let state = Arc::clone(&state);
        super::blocking::run_handler_blocking("task reopen", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            match crate::task_creator::reopen_task_for_api(&db, &task_id) {
                Ok(task_id) => Ok(task_id),
                Err(crate::task_creator::ReopenTaskError::OwnershipConflict) => Err((
                    axum::http::StatusCode::CONFLICT,
                    "cloud task ownership conflicts with an open local task".to_string(),
                )),
                Err(crate::task_creator::ReopenTaskError::Internal(error)) => {
                    Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, error))
                }
            }
        })
        .await?
    };
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(crate::mobile_api::TaskActionResponse {
        task_id,
        follow_task: None,
        revision_budget: None,
        workflow_extended: None,
    }))
}

/// Close a task that advanced past its final workflow stage. Shared by
/// `advance_stage` and `complete_stage`: hands blocker-close instructions to
/// dependents with workspaces, kills the task's daemon sessions, closes the
/// workflow item.
async fn close_task_after_final_stage(
    state: &Arc<AppState>,
    daemon: &mut crate::daemon_client::DaemonClient,
    task_id: String,
    workspace_teardown: Option<crate::task_creator::PreparedWorkspaceTeardown>,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    // Before anything is torn down: a workflow whose final stage declares the
    // merge-signaling approve post must not close leaving an open PR the merge
    // master never heard about. An error here deliberately abandons the close
    // — the task parks at its final stage instead.
    super::signal_agent::ensure_merge_handoff_before_close(state, &task_id).await?;
    let has_workspace_teardown = workspace_teardown.is_some();
    let blocker_close_instructions = {
        let db = Db::open(&state.config.db_path).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {}", e),
            )
        })?;
        collect_blocker_resolution_instructions(&db, &task_id, BlockerResolution::Closed)?
    };
    for (session_id, message) in blocker_close_instructions {
        if let Err((_, error)) = submit_task_input(daemon, &session_id, &message).await {
            log::warn!(
                "failed to deliver blocker-close instructions to dependent session {session_id}: {error}"
            );
        }
    }
    for session_id in [task_id.to_string(), format!("shell-wt-{task_id}")] {
        crate::task_creator::kill_session_replacing(
            daemon,
            &state.session_replacements,
            session_id.as_str(),
        )
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    crate::terminal_editor::close_task_editors(daemon, &state.session_replacements, &task_id)
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let teardown_session_id = workspace_teardown
        .as_ref()
        .map(|teardown| teardown.session_id.clone())
        .unwrap_or_else(|| format!("td-{task_id}"));
    if let Err(error) = crate::task_creator::kill_session_replacing(
        daemon,
        &state.session_replacements,
        &teardown_session_id,
    )
    .await
    {
        log::warn!("failed to replace workspace teardown session {teardown_session_id}: {error}");
    }
    let db = Db::open(&state.config.db_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
    })?;
    db.close_pipeline_item(&task_id).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
    })?;
    crate::task_creator::remove_completion_contexts(&state.config.daemon_dir, &task_id);
    crate::task_input_attachments::remove_task_attachments(&state.config.db_path, &task_id);
    state.preview_sessions.revoke_task(&task_id).await;
    if has_workspace_teardown {
        crate::task_creator::spawn_prepared_workspace_teardown_best_effort(
            daemon,
            workspace_teardown,
        )
        .await;
    } else {
        crate::worktree_cleanup::cleanup_closed_task_worktrees_by_id(&db, &task_id)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    start_dependents_unblocked_by_close_with_daemon(state, daemon, &task_id).await;
    if let Err(error) =
        super::signal_agent::release_closed_singleton_reservation(state, &task_id).await
    {
        log::warn!("closed task {task_id}, but singleton reservation release failed: {error}");
    }
    state.publish_state_changed(StateChangeScope::Tasks);
    state.publish_state_changed(StateChangeScope::Blockers);
    Ok(Json(crate::mobile_api::TaskActionResponse {
        task_id,
        follow_task: Some(false),
        revision_budget: None,
        workflow_extended: None,
    }))
}

pub(super) async fn advance_stage(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    axum::extract::Query(local_only): axum::extract::Query<super::task_federation::LocalOnlyQuery>,
    payload: Option<Json<AdvanceStageRequest>>,
) -> Result<Response, (axum::http::StatusCode, String)> {
    let payload = payload.map(|Json(payload)| payload);
    let forward_body = payload
        .as_ref()
        .map(|payload| serde_json::to_value(payload).unwrap_or(serde_json::Value::Null))
        .unwrap_or(serde_json::Value::Null);
    let trigger = match payload
        .as_ref()
        .and_then(|payload| payload.source.as_deref())
    {
        Some(source) => StageTrigger::from_caller_declared(source)
            .map_err(|message| (axum::http::StatusCode::BAD_REQUEST, message))?,
        None => StageTrigger::Unspecified,
    };
    // An incoherent pair is refused before anything is scheduled: a stage that
    // fails at spawn leaves the task parked with nothing running.
    let provider_override = crate::task_creator::parse_stage_provider_override(
        payload
            .as_ref()
            .and_then(|payload| payload.next_stage_agent_provider.as_deref()),
        payload
            .as_ref()
            .and_then(|payload| payload.next_stage_model.as_deref()),
        payload
            .as_ref()
            .and_then(|payload| payload.next_stage_effort.as_deref()),
        payload
            .as_ref()
            .and_then(|payload| payload.next_stage_provider_source.as_deref()),
    )
    .map_err(|message| (axum::http::StatusCode::BAD_REQUEST, message))?;
    let path = super::task_federation::task_path(&task_id, "/actions/advance-stage");
    let task_id = match super::task_federation::resolve_task_route(
        &state,
        &task_id,
        local_only.local_only,
        "POST",
        &path,
        &forward_body,
    )
    .await?
    {
        super::task_federation::TaskRoute::Local(id) => id,
        super::task_federation::TaskRoute::Remote(response) => return Ok(response),
    };
    {
        let state = Arc::clone(&state);
        let guarded_task_id = task_id.clone();
        super::blocking::run_handler_blocking("stage advance transfer proof check", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {e}"),
                )
            })?;
            reject_unprepared_transfer(&db, &guarded_task_id)
                .map_err(|e| (axum::http::StatusCode::CONFLICT, e))
        })
        .await?;
    }
    let response = crate::mobile_api::TaskActionResponse {
        task_id: task_id.clone(),
        follow_task: None,
        revision_budget: None,
        workflow_extended: None,
    };
    let Some(stage_advance) = state.begin_requested_stage_advance(&task_id).await else {
        return Ok(Json(response).into_response());
    };

    let (expected_transition_revision, expected_definition) = match payload {
        Some(payload) => (
            payload.expected_transition_revision,
            payload.expected_definition,
        ),
        None => (None, None),
    };
    if let Some(expected_definition) = expected_definition {
        let state = Arc::clone(&state);
        let task_id = task_id.clone();
        super::blocking::run_handler_blocking("stage advance workflow check", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {e}"),
                )
            })?;
            let pinned = db
                .get_pipeline_item(&task_id)
                .map_err(|e| db_write_error("db error", e))?
                .and_then(|item| item.pipeline_def)
                .and_then(|definition| serde_json::from_str::<serde_json::Value>(&definition).ok());
            if pinned.as_ref() != Some(&expected_definition) {
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "stale stage advance for {task_id}: this task's pinned workflow changed; \
                         read it again before advancing"
                    ),
                ));
            }
            Ok(())
        })
        .await?;
    }
    if let Some(expected_transition_revision) = expected_transition_revision {
        let current_transition_revision = {
            let state = Arc::clone(&state);
            let task_id = task_id.clone();
            super::blocking::run_handler_blocking("stage advance revision check", move || {
                let db = Db::open(&state.config.db_path).map_err(|e| {
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("db error: {}", e),
                    )
                })?;
                db.latest_stage_run(&task_id)
                    .map(|run| run.map(|run| run.id))
                    .map_err(|e| {
                        (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!("db error: {}", e),
                        )
                    })
            })
            .await?
        };
        if current_transition_revision.as_deref() != Some(expected_transition_revision.as_str()) {
            return Err((
                axum::http::StatusCode::CONFLICT,
                format!(
                    "stale stage advance for {task_id}: expected transition revision \
                     {expected_transition_revision}"
                ),
            ));
        }
    }

    #[cfg(test)]
    if let Some(stage_advancer) = state.stage_advancer.clone() {
        return stage_advancer(task_id)
            .map(|response| Json(response).into_response())
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e));
    }

    let transition = {
        let state = Arc::clone(&state);
        let task_id = task_id.clone();
        super::blocking::run_handler_blocking("stage advance prepare", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            let latest = db.latest_stage_run(&task_id).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            if latest
                .as_ref()
                .is_some_and(|run| run.kind == "post" && run.status == "running")
            {
                return Ok(None);
            }
            let prepared = crate::task_creator::prepare_advance_stage_for_api_with_intent(
                &db,
                &state.config,
                &task_id,
                crate::task_creator::StageAdvanceIntent {
                    trigger,
                    provider_override,
                },
            )
            .map_err(|e| (stage_action_error_status(&e), e))?;
            // An explicit advance supersedes whatever transition an earlier
            // completion or revision still owed this task.
            db.clear_ledger_continuation(&task_id)
                .map_err(|e| db_write_error("db error", e))?;
            Ok(Some(prepared))
        })
        .await?
    };
    let Some(transition) = transition else {
        return Ok(Json(response).into_response());
    };
    if let crate::task_creator::PreparedStageTransition::Post(prepared) = transition {
        // Unlike a stage swap, a live-session post cannot kill the HTTP
        // caller. Await it so the desktop action receives the daemon's exact
        // held-draft result instead of an unconditional 200 from before the
        // detached worker even attempts delivery.
        let post_state = Arc::clone(&state);
        let handle = tokio::runtime::Handle::current();
        let outcome = tokio::task::spawn_blocking(move || {
            handle.block_on(async move {
                let mut daemon =
                    crate::daemon_client::DaemonClient::connect(&post_state.config.daemon_dir)
                        .await
                        .map_err(|error| format!("daemon error: {error}"))?;
                crate::task_creator::dispatch_prepared_post_for_api(
                    &post_state.config.db_path,
                    &mut daemon,
                    &post_state.session_replacements,
                    *prepared,
                )
                .await
            })
        })
        .await
        .map_err(|error| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("stage post worker failed: {error}"),
            )
        })?
        .map_err(|error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error))?;
        state.publish_state_changed(StateChangeScope::Tasks);
        return Ok(Json(outcome).into_response());
    }
    execute_stage_transition_detached_holding(
        Arc::clone(&state),
        task_id,
        transition,
        StageTransitionOwnership {
            task_mutation: Some(stage_advance),
            requested_operation: None,
        },
    );
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(response).into_response())
}

/// Execute a prepared transition: swap to the next stage's run, dispatch a
/// post into the running session, or close past the final stage. Shared by
/// `advance_stage` and `complete_stage`.
async fn execute_stage_transition(
    state: &Arc<AppState>,
    daemon: &mut crate::daemon_client::DaemonClient,
    task_id: &str,
    transition: crate::task_creator::PreparedStageTransition,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    // Publication barrier: no session is started, and no post or close is
    // dispatched, while an accepted ledger entry of this task (the result
    // that caused the transition, typically) is not yet readable on disk.
    publish_task_ledger_before_dispatch(state, task_id)?;
    match transition {
        crate::task_creator::PreparedStageTransition::Run(prepared) => {
            state.preview_sessions.revoke_task(task_id).await;
            let advanced = crate::task_creator::spawn_prepared_stage_run_for_api(
                &state.config.db_path,
                daemon,
                &state.session_replacements,
                *prepared,
            )
            .await
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
            state.publish_state_changed(StateChangeScope::Tasks);
            Ok(Json(advanced))
        }
        crate::task_creator::PreparedStageTransition::Post(prepared) => {
            // A post never moves the task's stage, so it cannot newly enter
            // `pr`; no dependent start check is needed.
            let dispatched = crate::task_creator::dispatch_prepared_post_for_api(
                &state.config.db_path,
                daemon,
                &state.session_replacements,
                *prepared,
            )
            .await
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
            state.publish_state_changed(StateChangeScope::Tasks);
            Ok(Json(dispatched))
        }
        crate::task_creator::PreparedStageTransition::Close {
            task_id,
            workspace_teardown,
        } => {
            close_task_after_final_stage(
                state,
                daemon,
                task_id,
                workspace_teardown.map(|teardown| *teardown),
            )
            .await
        }
    }
}

fn publish_task_ledger_before_dispatch(
    state: &AppState,
    task_id: &str,
) -> Result<(), (axum::http::StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
    crate::task_store::flush_task(&db, &state.config.db_path, task_id)
        .map(|_| ())
        .map_err(|error| {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!("stage transition held until the task ledger is published: {error}"),
            )
        })
}

/// Run a prepared transition on a detached task, NOT on the HTTP request's
/// future. Transitions kill the session they replace — and when the verdict
/// comes from an agent INSIDE that session (kanna-cli / kanna-mcp riding the
/// task's own process tree), killing it drops the HTTP connection and axum
/// cancels the in-flight handler, silently abandoning the transition between
/// the kill and the respawn. Detaching makes the transition immune to the
/// caller's death; failures are logged since the caller may not outlive the
/// work it triggered.
#[derive(Default)]
struct StageTransitionOwnership {
    task_mutation: Option<super::state::RequestedTaskMutation>,
    requested_operation: Option<super::state::RequestedTaskOperation>,
}

impl StageTransitionOwnership {
    fn release(self) {
        drop(self.task_mutation);
        drop(self.requested_operation);
    }
}

/// Failure is a lifecycle fact, separate from the successful run that asked
/// the engine to advance. Subscription selection must not infer it from idle.
fn record_stage_transition_failure(state: &AppState, task_id: &str, error: &str) {
    let recorded = Db::open(&state.config.db_path).and_then(|db| {
        db.append_task_event(
            task_id,
            crate::db::TaskEventKind::LifecycleFailed,
            serde_json::json!({ "operation": "stage_transition", "error": error }),
        )
    });
    if let Err(record_error) = recorded {
        log::error!("failed to record stage transition failure for {task_id}: {record_error}");
    }
}

/// Same, but the detached worker takes ownership of a per-task operation
/// guard for the whole transition.
///
/// The handler must return before the transition runs — it kills and respawns
/// the caller's own session — so the guard cannot simply live in the handler:
/// dropping it at the 200 would reopen the task between the response and the
/// work landing, and a second request admitted in that window would spend
/// another budget slot and prepare a workspace from task state the in-flight
/// transition is about to change. Held here, it drops when the worker exits,
/// on every path including a daemon that never answers.
fn execute_stage_transition_detached_holding(
    state: Arc<AppState>,
    task_id: String,
    transition: crate::task_creator::PreparedStageTransition,
    ownership: StageTransitionOwnership,
) {
    // Stage execution interleaves async daemon I/O with synchronous git,
    // filesystem, and SQLite work (run records, fork rollback, teardown
    // prep). Drive the whole future from the blocking pool so none of it can
    // occupy a runtime worker and starve the shared KSP terminal transport.
    let worker_task_id = task_id.clone();
    let failure_state = state.clone();
    tokio::spawn(async move {
        // Bound to the worker's own scope: every exit path below — daemon
        // connect failure, transition error, success, join error, or the task
        // being dropped at runtime shutdown — releases the task.
        let handle = tokio::runtime::Handle::current();
        let joined = tokio::task::spawn_blocking(move || {
            handle.block_on(async move {
                let mut daemon =
                    match crate::daemon_client::DaemonClient::connect(&state.config.daemon_dir)
                        .await
                    {
                        Ok(daemon) => daemon,
                        Err(error) => {
                            log::error!(
                                "stage transition for {} failed to reach daemon: {}",
                                task_id,
                                error
                            );
                            record_stage_transition_failure(&state, &task_id, &error.to_string());
                            return;
                        }
                    };
                if let Err((_, message)) =
                    execute_stage_transition(&state, &mut daemon, &task_id, transition).await
                {
                    log::error!("stage transition for {} failed: {}", task_id, message);
                    record_stage_transition_failure(&state, &task_id, &message);
                    state.publish_state_changed(StateChangeScope::Tasks);
                }
            })
        })
        .await;
        ownership.release();
        if let Err(join_error) = joined {
            record_stage_transition_failure(
                &failure_state,
                &worker_task_id,
                &join_error.to_string(),
            );
            log::error!(
                "stage transition worker for {} failed: {}",
                worker_task_id,
                join_error
            );
        }
    });
}

pub(super) async fn resume_task(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    axum::extract::Query(local_only): axum::extract::Query<super::task_federation::LocalOnlyQuery>,
) -> Result<Response, (axum::http::StatusCode, String)> {
    let path = super::task_federation::task_path(&task_id, "/actions/resume");
    let task_id = match super::task_federation::resolve_task_route(
        &state,
        &task_id,
        local_only.local_only,
        "POST",
        &path,
        &serde_json::Value::Null,
    )
    .await?
    {
        super::task_federation::TaskRoute::Local(id) => id,
        super::task_federation::TaskRoute::Remote(response) => return Ok(response),
    };
    let task_mutation = state.begin_requested_task_mutation(&task_id).await;
    let (latest_run_status, daemon_session_id) = {
        let state = Arc::clone(&state);
        let task_id = task_id.clone();
        super::blocking::run_handler_blocking("task resume inspect", move || {
            let db = Db::open(&state.config.db_path).map_err(|error| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?;
            let run = db
                .latest_stage_run(&task_id)
                .map_err(|error| {
                    (
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                        format!("db error: {error}"),
                    )
                })?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::CONFLICT,
                        format!("task has no stage run to resume: {task_id}"),
                    )
                })?;
            // `succeeded` belongs here because a recorded verdict does not keep
            // a session alive. A manual stage's agent finishes its turn, parks
            // at its composer, and waits — and when the machine reboots, that
            // PTY dies with a `succeeded` run behind it. Refusing those was how
            // restart recovery stopped working for every singleton and every
            // manual stage: the desktop's attach-failure path calls this route,
            // so a task nobody could resume was a task whose terminal never
            // came back. The live-session case is still rejected below, after
            // the daemon has been asked; this only decides what is eligible.
            if !matches!(
                run.status.as_str(),
                "running" | "cancelled" | "failed" | "succeeded"
            ) {
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "latest run is {}, not running, cancelled, failed or succeeded: {}",
                        run.status, task_id
                    ),
                ));
            }
            let daemon_session_id = run.session_id.unwrap_or_else(|| task_id.clone());
            Ok((run.status, daemon_session_id))
        })
        .await?
    };
    match crate::task_creator::daemon_session_presence(&state.config.daemon_dir, &daemon_session_id)
        .await
    {
        crate::task_creator::DaemonSessionPresence::Present => {
            if latest_run_status == "running" {
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    format!("task session is still alive: {task_id}"),
                ));
            }
            let db_path = state.config.db_path.clone();
            let restore_task_id = task_id.clone();
            let restored = super::blocking::run_handler_blocking(
                "task resume live-session reconciliation",
                move || {
                    crate::http_api::restore_task_run_for_live_session(&db_path, &restore_task_id)
                        .map_err(|error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error))
                },
            )
            .await?;
            if restored {
                log::warn!(
                    "resume request found live daemon session for {task_id}; restored interrupted run instead of spawning"
                );
                state.publish_state_changed(StateChangeScope::Tasks);
                return Ok(Json(crate::mobile_api::TaskActionResponse {
                    task_id,
                    follow_task: None,
                    revision_budget: None,
                    workflow_extended: None,
                })
                .into_response());
            }
            return Err((
                axum::http::StatusCode::CONFLICT,
                format!("task session is still alive: {task_id}"),
            ));
        }
        crate::task_creator::DaemonSessionPresence::Unknown => {
            return Err((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!("could not verify that task session is dead: {task_id}"),
            ));
        }
        crate::task_creator::DaemonSessionPresence::Absent => {}
    }
    if latest_run_status == "running" {
        let db_path = state.config.db_path.clone();
        let interrupted_task_id = task_id.clone();
        super::blocking::run_handler_blocking("task resume mark missing session", move || {
            let summary = "task session was missing when the resume action began automatic provider-context recovery";
            let result = serde_json::json!({
                "status": "failure",
                "summary": summary,
            })
            .to_string();
            crate::http_api::mark_task_session_interrupted_for_recovery(
                &db_path,
                &interrupted_task_id,
                "failed",
                &result,
            )
            .map_err(|error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error))?
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::NOT_FOUND,
                    format!("task not found: {interrupted_task_id}"),
                )
            })?;
            Ok(())
        })
        .await?;
        state.publish_state_changed(StateChangeScope::Tasks);
    }
    let prepared = {
        let state = Arc::clone(&state);
        let task_id = task_id.clone();
        super::blocking::run_handler_blocking("task resume prepare", move || {
            let db = Db::open(&state.config.db_path).map_err(|error| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?;
            reject_unprepared_transfer(&db, &task_id)
                .map_err(|error| (axum::http::StatusCode::CONFLICT, error))?;
            crate::task_creator::prepare_resume_task_for_api(&db, &state.config, &task_id)
                .map_err(|error| (resume_action_error_status(&error), error))
        })
        .await?
    };
    execute_stage_transition_detached_holding(
        Arc::clone(&state),
        task_id.clone(),
        crate::task_creator::PreparedStageTransition::Run(Box::new(prepared)),
        StageTransitionOwnership {
            task_mutation: Some(task_mutation),
            requested_operation: None,
        },
    );
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(crate::mobile_api::TaskActionResponse {
        task_id,
        follow_task: None,
        revision_budget: None,
        workflow_extended: None,
    })
    .into_response())
}

pub(super) async fn rerun_stage(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    axum::extract::Query(local_only): axum::extract::Query<super::task_federation::LocalOnlyQuery>,
) -> Result<Response, (axum::http::StatusCode, String)> {
    let path = super::task_federation::task_path(&task_id, "/actions/rerun-stage");
    let task_id = match super::task_federation::resolve_task_route(
        &state,
        &task_id,
        local_only.local_only,
        "POST",
        &path,
        &serde_json::Value::Null,
    )
    .await?
    {
        super::task_federation::TaskRoute::Local(id) => id,
        super::task_federation::TaskRoute::Remote(response) => return Ok(response),
    };
    let task_mutation = state.begin_requested_task_mutation(&task_id).await;

    #[cfg(test)]
    if let Some(stage_rerunner) = state.stage_rerunner.clone() {
        return stage_rerunner(task_id)
            .map(|response| Json(response).into_response())
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e));
    }

    let prepared = {
        let state = Arc::clone(&state);
        let task_id = task_id.clone();
        super::blocking::run_handler_blocking("stage rerun prepare", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            reject_unprepared_transfer(&db, &task_id)
                .map_err(|error| (axum::http::StatusCode::CONFLICT, error))?;
            let prepared =
                crate::task_creator::prepare_rerun_stage_for_api(&db, &state.config, &task_id)
                    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
            // A rerun supersedes any transition still owed to the run it replaces.
            db.clear_ledger_continuation(&task_id)
                .map_err(|e| db_write_error("db error", e))?;
            Ok(prepared)
        })
        .await?
    };
    // Detached for the same reason as execute_stage_transition_detached: the
    // rerun kills the session that may carry this very request. Driven from
    // the blocking pool for the same reason as well — the rerun records runs
    // and rolls back forks with synchronous git/SQLite work.
    let rerun_state = Arc::clone(&state);
    let rerun_task_id = task_id.clone();
    tokio::spawn(async move {
        let _task_mutation = task_mutation;
        let handle = tokio::runtime::Handle::current();
        let worker_task_id = rerun_task_id.clone();
        let joined = tokio::task::spawn_blocking(move || {
            handle.block_on(async move {
                let mut daemon = match crate::daemon_client::DaemonClient::connect(
                    &rerun_state.config.daemon_dir,
                )
                .await
                {
                    Ok(daemon) => daemon,
                    Err(error) => {
                        log::error!(
                            "stage rerun for {} failed to reach daemon: {}",
                            rerun_task_id,
                            error
                        );
                        return;
                    }
                };
                rerun_state
                    .preview_sessions
                    .revoke_task(&rerun_task_id)
                    .await;
                if let Err(error) = crate::task_creator::rerun_prepared_stage_for_api(
                    &rerun_state.config.db_path,
                    &mut daemon,
                    &rerun_state.session_replacements,
                    prepared,
                )
                .await
                {
                    log::error!("stage rerun for {} failed: {}", rerun_task_id, error);
                    rerun_state.publish_state_changed(StateChangeScope::Tasks);
                    return;
                }
                rerun_state.publish_state_changed(StateChangeScope::Tasks);
            })
        })
        .await;
        if let Err(join_error) = joined {
            log::error!(
                "stage rerun worker for {} failed: {}",
                worker_task_id,
                join_error
            );
        }
    });
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(Json(crate::mobile_api::TaskActionResponse {
        task_id,
        follow_task: None,
        revision_budget: None,
        workflow_extended: None,
    })
    .into_response())
}

/// A pull-request URL carried by a stage-complete verdict: explicitly via
/// `metadata.pr_url`, or the first `…/pull/<n>` link in the summary (agents
/// reporting through plain `kanna-cli` tend to put it there).
fn pr_url_from_verdict(metadata: Option<&serde_json::Value>, summary: &str) -> Option<String> {
    if let Some(url) = metadata
        .and_then(|metadata| metadata.get("pr_url"))
        .and_then(|value| value.as_str())
        .filter(|url| !url.trim().is_empty())
    {
        return Some(url.trim().to_string());
    }
    summary
        .split_whitespace()
        .map(|token| token.trim_end_matches(['.', ',', ')', ']', ';']))
        .find(|token| {
            token.starts_with("https://")
                && token.rsplit_once("/pull/").is_some_and(|(_, number)| {
                    !number.is_empty() && number.chars().all(|c| c.is_ascii_digit())
                })
        })
        .map(str::to_string)
}

/// A pull-request review context carried by a stage-complete verdict.
///
/// Accepts both `reviewContext` and `review_context` because agents reach this
/// through the MCP catalog and through plain `kanna-cli` JSON, and a
/// convention mismatch here would silently drop the whole identity.
fn review_context_from_verdict(
    metadata: Option<&serde_json::Value>,
) -> Result<Option<crate::db::ReviewContextInput>, (axum::http::StatusCode, String)> {
    let Some(raw) = metadata
        .and_then(|metadata| {
            metadata
                .get("reviewContext")
                .or_else(|| metadata.get("review_context"))
        })
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    // Deliberately not `.ok()`: a context whose shape is wrong must say so.
    // Swallowing it would leave the agent believing it published a PR identity
    // and the operator with a review it cannot queue, for no visible reason.
    serde_json::from_value(raw.clone())
        .map(Some)
        .map_err(|error| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("invalid reviewContext metadata: {error}"),
            )
        })
}

fn pr_number_from_url(pr_url: &str) -> Option<i64> {
    pr_url
        .rsplit_once("/pull/")
        .and_then(|(_, number)| number.parse::<i64>().ok())
}

/// Fingerprint of one exact combined plan completion.
///
/// The recorded stage result cannot identify the operation on its own: the
/// same plan summary can be submitted with a different suffix — another review
/// depth, another provider, another revision budget — and each is a different
/// publication. Both definitions are canonicalized through `serde_json::Value`
/// first, so a retry that only reorders keys or changes whitespace is still
/// recognized as the same request.
fn plan_publication_digest(
    stage_result: &str,
    expected: &serde_json::Value,
    definition: &serde_json::Value,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in [stage_result, &expected.to_string(), &definition.to_string()] {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// What the task's pinned workflow records about a plan publication, if any.
struct RecordedPlanPublication {
    source_run_id: String,
    request_digest: Option<String>,
}

fn recorded_plan_publication(db: &Db, task_id: &str) -> Option<RecordedPlanPublication> {
    let definition = db.get_pipeline_item(task_id).ok().flatten()?.pipeline_def?;
    let plan_context = serde_json::from_str::<serde_json::Value>(&definition)
        .ok()?
        .get("plan_context")?
        .clone();
    Some(RecordedPlanPublication {
        source_run_id: plan_context.get("source_run_id")?.as_str()?.to_string(),
        request_digest: plan_context
            .get("request_digest")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    })
}

/// Stage whose completion may publish the rest of its own task's workflow.
///
/// A single reserved name, not a policy flag: the journey this serves is
/// research -> appended planning -> the stages planning chose, and the
/// planning agent is the one that has read the objective.
const PLAN_STAGE_NAME: &str = "plan";

/// Carries an HTTP error out of a DB transaction closure, which must name a
/// type convertible from `rusqlite::Error`.
struct PlanExtensionTxError((axum::http::StatusCode, String));

impl From<rusqlite::Error> for PlanExtensionTxError {
    fn from(error: rusqlite::Error) -> Self {
        Self(db_write_error("db error", error))
    }
}

struct PreparedPlanWorkflowExtension {
    stage: String,
    workflow_name: String,
    previous_definition: String,
    revision_rounds: i64,
    validated: crate::task_creator::ValidatedWorkflowReplacement,
}

/// Validate the stages a planning run publishes for its own task.
///
/// Everything here is refused *before* the verdict is recorded, so a planner
/// that composed an unsupported suffix can correct it and complete again
/// rather than discovering its plan was recorded without them.
fn prepare_plan_workflow_extension(
    db: &Db,
    task_id: &str,
    current_run: &crate::db::StageRun,
    stage_result: &str,
    definition: &serde_json::Value,
    expected: &serde_json::Value,
) -> Result<PreparedPlanWorkflowExtension, (axum::http::StatusCode, String)> {
    use axum::http::StatusCode;
    let item = db
        .get_pipeline_item(task_id)
        .map_err(|error| db_write_error("db error", error))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("task not found: {task_id}")))?;
    let stage = item
        .stage
        .clone()
        .ok_or_else(|| (StatusCode::CONFLICT, "task has no current stage".into()))?;
    if current_run.kind != "main" || current_run.stage != stage {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "only the task's current main stage run may publish its remaining stages; \
                 this run is the {} run of stage '{}'",
                current_run.kind, current_run.stage
            ),
        ));
    }
    if stage != PLAN_STAGE_NAME {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "only a stage named '{PLAN_STAGE_NAME}' may publish a task's remaining stages; \
                 this task is at '{stage}'"
            ),
        ));
    }
    let previous = item
        .pipeline_def
        .clone()
        .ok_or_else(|| (StatusCode::CONFLICT, "task has no pinned workflow".into()))?;
    let before: serde_json::Value = serde_json::from_str(&previous).map_err(|error| {
        (
            StatusCode::CONFLICT,
            format!("invalid pinned workflow: {error}"),
        )
    })?;
    if &before != expected {
        return Err((
            StatusCode::CONFLICT,
            "pinned workflow changed; read it again before publishing the remaining stages".into(),
        ));
    }
    crate::task_creator::validate_plan_workflow_extension(&previous, definition, &stage)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    // A planning stage that advances on its own would start the stages it just
    // published before anybody read the plan, which is the gate this journey
    // exists to keep.
    if plan_stage_transition(&before, &stage).as_deref() != Some("manual") {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "stage '{stage}' must declare policy.transition \"manual\" to publish the \
                    remaining stages"
            ),
        ));
    }
    if before["stages"]
        .as_array()
        .and_then(|stages| stages.iter().find(|entry| entry["name"] == stage.as_str()))
        .is_some_and(|entry| entry.get("post").is_some())
    {
        return Err((
            StatusCode::CONFLICT,
            format!(
                "stage '{stage}' declares a post; publish its remaining stages from a \
                    planning stage that has none"
            ),
        ));
    }
    let repo = db
        .get_repo(&item.repo_id)
        .map_err(|error| db_write_error("db error", error))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "task repository not found".into()))?;
    let runs = db
        .list_stage_runs_for_task(task_id)
        .map_err(|error| db_write_error("db error", error))?;
    let stamp = crate::task_creator::WorkflowPlanContext {
        source_run_id: current_run.id.clone(),
        stage: stage.clone(),
        result: stage_result.to_string(),
        request_digest: Some(plan_publication_digest(stage_result, expected, definition)),
    };
    let validated = crate::task_creator::validate_task_workflow_replacement_with_plan_context(
        &repo,
        definition,
        &previous,
        &stage,
        &runs,
        crate::task_creator::PlanContextPolicy::Stamp(&stamp),
    )
    .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    Ok(PreparedPlanWorkflowExtension {
        stage,
        workflow_name: item
            .pipeline
            .clone()
            .unwrap_or_else(|| "no-review".to_string()),
        previous_definition: previous,
        revision_rounds: item.revision_rounds,
        validated,
    })
}

fn plan_stage_transition(definition: &serde_json::Value, stage: &str) -> Option<String> {
    definition["stages"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"] == stage)?
        .get("policy")?
        .get("transition")?
        .as_str()
        .map(str::to_string)
}

pub(super) async fn complete_stage(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    Json(payload): Json<crate::mobile_api::CompleteStageRequest>,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    let task_id = resolve_task_id_for_mutation(&state, &task_id).await?;
    let task_mutation = state.begin_requested_task_mutation(&task_id).await;

    #[cfg(test)]
    if let Some(stage_completer) = state.stage_completer.clone() {
        return stage_completer(task_id, payload)
            .map(Json)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e));
    }

    // Refused rather than coerced. An unrecognized word is an agent that has
    // not read the vocabulary, and guessing which of the six it meant would
    // invent a verdict nobody recorded; the refusal names all of them so the
    // caller can correct itself.
    let verdict = kanna_runtime_defaults::stage_verdict::StageVerdict::parse(&payload.status)
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error))?;
    // The result's message is what the next session receives and what the
    // ledger keeps (spec §7). An empty one is refused before anything is
    // recorded, so the caller can say what happened and record again.
    if payload.summary.trim().is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "the result message (`summary`) is empty; nothing was recorded. Its first line is \
             the one-line summary surfaces show; the rest is what was done and what the next \
             stage must know. Record the result again with a message."
                .to_string(),
        ));
    }
    // A plan publishes the stages it chose in the same call that records the
    // plan itself. The two arguments are one operation: a plan visible without
    // its stages, or stages published under a plan that failed, are both
    // states nothing downstream could interpret.
    let workflow_extension = match (
        payload.workflow_definition.clone(),
        payload.expected_definition.clone(),
    ) {
        (Some(definition), Some(expected)) => {
            if !verdict.completes_stage() {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    "workflowDefinition may only accompany a successful stage completion"
                        .to_string(),
                ));
            }
            Some((definition, expected))
        }
        (None, None) => None,
        _ => {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                "workflowDefinition and expectedDefinition must be provided together".to_string(),
            ))
        }
    };
    // A review agent working without the PR review manager publishes the PR
    // identity here, because there is no `kanna_create_task` call it could
    // have carried it on. Validated before anything is recorded so a
    // malformed context is a refused argument the agent can correct, not a
    // completion that silently dropped it.
    let review_context = match review_context_from_verdict(payload.metadata.as_ref())? {
        Some(context) => Some(
            context
                .validated()
                .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?,
        ),
        None => None,
    };
    let should_auto_advance = verdict.completes_stage();
    let stage_result_value = serde_json::json!({
        // The canonical spelling from the shared table, so the durable record
        // never carries a word the vocabulary does not contain.
        "status": verdict.as_str(),
        "summary": payload.summary,
        "metadata": payload.metadata,
    });
    let stage_result = serde_json::to_string(&stage_result_value).map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            format!("invalid stage result: {}", e),
        )
    })?;

    // The identity of a combined completion is its result *and* the stages it
    // asked to publish. Computed once, before anything is compared, so every
    // replay decision below asks the same question.
    let requested_digest = workflow_extension
        .as_ref()
        .map(|(definition, expected)| plan_publication_digest(&stage_result, expected, definition));
    let completion_attempt_key = payload.completion_attempt_key.clone();
    let completion_attempt_key_for_record = completion_attempt_key.clone();
    let completion_run_id = payload.run_id.clone();
    let (task_id, _finished_run, already_closed, replayed, workflow_extended) = {
        let state = Arc::clone(&state);
        let payload_verdict = verdict;
        let payload_summary = payload.summary;
        let payload_metadata = payload.metadata;
        let payload_run_id = payload.run_id;
        let workflow_extension = workflow_extension.clone();
        let requested_digest = requested_digest.clone();
        super::blocking::run_handler_blocking("stage completion record", move || {
            /// Is a recorded completion of `run_id` the same operation as this
            /// request?
            ///
            /// For an ordinary completion the recorded result answers it, as
            /// it always has. For a combined one it cannot: the same plan
            /// summary can be submitted with a different suffix, and each is a
            /// different publication. So a combined request is a replay only
            /// when this task's pinned workflow records a publication by that
            /// run carrying exactly this request's fingerprint.
            fn same_operation(
                db: &Db,
                task_id: &str,
                run_id: &str,
                requested_digest: Option<&str>,
            ) -> bool {
                let Some(digest) = requested_digest else {
                    return true;
                };
                recorded_plan_publication(db, task_id).is_some_and(|record| {
                    record.source_run_id == run_id
                        && record.request_digest.as_deref() == Some(digest)
                })
            }
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            let task_id = db
                .resolve_pipeline_item_id(&task_id)
                .map_err(|e| db_write_error("db error", e))?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::NOT_FOUND,
                        format!("task not found: {}", task_id),
                    )
                })?;
            // Resolve retries before looking at live runs (or closed state).
            // The task mutation guard serializes lookup, recording and transition.
            let contextless_key = if payload_run_id.is_none() {
                completion_attempt_key_for_record.as_deref()
            } else {
                None
            };
            if let Some(key) = contextless_key {
                if let Some((original_run_id, original_result)) = db
                    .contextless_completion_attempt(&task_id, key)
                    .map_err(|e| db_write_error("db error", e))?
                {
                    if original_result != stage_result {
                        return Err((axum::http::StatusCode::CONFLICT, format!(
                            "completionAttemptKey already recorded a different verdict for run {original_run_id}"
                        )));
                    }
                    if !same_operation(
                        &db,
                        &task_id,
                        &original_run_id,
                        requested_digest.as_deref(),
                    ) {
                        return Err((axum::http::StatusCode::CONFLICT, format!(
                            "completionAttemptKey already recorded a completion for run \
                             {original_run_id} that did not publish these stages; read the task's \
                             workflow again before retrying"
                        )));
                    }
                    return Ok((task_id, None, false, true, false));
                }
            }
            if db
                .get_pipeline_item(&task_id)
                .map_err(|e| db_write_error("db error", e))?
                .is_some_and(|item| item.closed_at.is_some())
            {
                return Ok((task_id, None, true, false, false));
            }
            // The lifecycle column stays two-valued: the six-word verdict is
            // recorded in the run's result, and widening this enum would change
            // what every existing reader of `run.finished` and
            // `latestRun.status` already means by it.
            let run_status = payload_verdict.run_status();
            let current_run = resolve_action_run(&db, &task_id, payload_run_id.as_deref())?
                .ok_or_else(|| {
                    (
                        axum::http::StatusCode::CONFLICT,
                        format!("task has no stage run to complete: {task_id}"),
                    )
                })?;
            let mut payload_run_id = payload_run_id.unwrap_or_else(|| current_run.id.clone());
            if let Some(attempt_key) = completion_attempt_key_for_record.as_deref() {
                if let Some(original_run_id) =
                    crate::task_creator::resolve_legacy_completion_retry_run(
                        &state.config.daemon_dir,
                        &db,
                        &task_id,
                        &payload_run_id,
                        attempt_key,
                    )
                {
                    payload_run_id = original_run_id;
                }
            }
            if current_run.id != payload_run_id {
                let prior = db
                    .stage_run(&payload_run_id)
                    .map_err(|e| db_write_error("db error", e))?;
                if prior.as_ref().is_some_and(|run| {
                    run.task_id == task_id
                        && run.status == run_status
                        && run.result.as_deref() == Some(stage_result.as_str())
                }) && same_operation(&db, &task_id, &payload_run_id, requested_digest.as_deref())
                {
                    if let Some(key) = contextless_key {
                        db.record_contextless_completion_attempt(key, &payload_run_id, &stage_result)
                            .map_err(|e| db_write_error("db error", e))?;
                    }
                    return Ok((task_id, None, false, true, false));
                }
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "stale stage completion for run {payload_run_id}; current run is {}",
                        current_run.id
                    ),
                ));
            }
            if current_run.status == run_status
                && current_run.result.as_deref() == Some(stage_result.as_str())
                && same_operation(&db, &task_id, &payload_run_id, requested_digest.as_deref())
            {
                if let Some(key) = contextless_key {
                    db.record_contextless_completion_attempt(key, &payload_run_id, &stage_result)
                        .map_err(|e| db_write_error("db error", e))?;
                }
                return Ok((task_id, None, false, true, false));
            }
            // The plan a task's later stages were published under is not a
            // draft: once stamped, a differing retry of that same run would
            // leave the recorded plan and the executing stages describing
            // different work.
            if recorded_plan_publication(&db, &task_id)
                .is_some_and(|record| record.source_run_id == payload_run_id)
            {
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "run {payload_run_id} already published this task's remaining stages; \
                         its recorded plan cannot be replaced"
                    ),
                ));
            }
            if !matches!(
                current_run.status.as_str(),
                "running" | "succeeded" | "failed"
            ) {
                return Err((
                    axum::http::StatusCode::CONFLICT,
                    format!(
                        "stage run {payload_run_id} cannot be completed from status {}",
                        current_run.status
                    ),
                ));
            }
            let extension = match workflow_extension.as_ref() {
                None => None,
                Some((definition, expected)) => Some(prepare_plan_workflow_extension(
                    &db,
                    &task_id,
                    &current_run,
                    &stage_result,
                    definition,
                    expected,
                )?),
            };
            let finished_run = Some(crate::db::FinishedStageRun {
                kind: current_run.kind.clone(),
                completion_transition: current_run.completion_transition.clone(),
                trigger: current_run.trigger.clone(),
            });
            // The engine attaches the branch and commit it observes in the
            // run's own workspace now, at acceptance (read-only; unknown is
            // recorded as unknown). A later HEAD is not evidence of this
            // result, so this is the only time it is read.
            let observed = crate::task_store::observe_workspace(current_run.cwd.as_deref())
                .map_err(|error| (axum::http::StatusCode::CONFLICT, error))?;
            // One transaction, so a plan is never durably successful without
            // the stages it published, and a rejected extension leaves no
            // recorded verdict for the planner to discover later. The ledger
            // entry and the continuation that will dispatch the transition
            // commit in it too.
            let record = |db: &Db| -> Result<(), (axum::http::StatusCode, String)> {
                let event_floor = db
                    .ledger_event_floor()
                    .map_err(|e| db_write_error("db error", e))?;
                // Asked inside the write transaction, not before it. A transfer
                // finalizing this task claims its workflow in a transaction of
                // its own, and SQLite's single writer is what makes the two
                // mutually exclusive: whichever commits first wins, and the
                // loser writes nothing. Committing the plan under a live
                // transfer would hand an older destination a suffix whose plan
                // it silently drops — and the source has already been asked to
                // quit by then.
                if extension.is_some() {
                    if let Some(transfer_id) = db
                        .task_workflow_is_claimed_by_transfer(&task_id)
                        .map_err(|error| db_write_error("db error", error))?
                    {
                        return Err((
                            axum::http::StatusCode::CONFLICT,
                            format!(
                                "transfer {transfer_id} is finalizing this task and owns its \
                                 workflow; nothing was recorded. Wait for the transfer to settle, \
                                 then complete the plan again."
                            ),
                        ));
                    }
                }
                if let Some(key) = contextless_key {
                    db.finish_contextless_stage_run(
                        key, &payload_run_id, run_status, &stage_result, &payload_summary,
                    )
                } else {
                    db.finish_stage_run(
                        &payload_run_id, run_status, Some(&stage_result), Some(&payload_summary),
                    )
                }
                .map_err(|e| db_write_error("db error", e))?;
                let result_entry = enqueue_completion_result(
                    db,
                    &task_id,
                    &current_run,
                    &observed,
                    payload_verdict.as_str(),
                    &payload_summary,
                    payload_metadata.as_ref(),
                    &stage_result,
                    requested_digest.as_deref(),
                    event_floor,
                )
                .map_err(|e| db_write_error("db error", e))?;
                // A corrected verdict replaces whatever the earlier one asked
                // the engine to do next.
                if payload_verdict.completes_stage() {
                    // The task's stage, not the run's: a post run is named
                    // after its post, while the task stays at the owning stage.
                    let task_stage = db
                        .get_pipeline_item(&task_id)
                        .map_err(|e| db_write_error("db error", e))?
                        .and_then(|item| item.stage);
                    db.put_ledger_continuation(
                        &task_id,
                        &result_entry.operation_id,
                        crate::db::task_store::STAGE_COMPLETION_CONTINUATION,
                        &serde_json::json!({
                            "kind": current_run.kind,
                            "completionTransition": current_run.completion_transition,
                            "trigger": current_run.trigger,
                            "resultId": result_entry.entry_id,
                            // The fence: this transition is owed only while
                            // the task is still at this stage with this run
                            // as its latest.
                            "runId": current_run.id,
                            "stage": task_stage,
                        }),
                    )
                } else {
                    db.clear_ledger_continuation(&task_id)
                }
                .map_err(|e| db_write_error("db error", e))?;
                if let Some(extension) = extension.as_ref() {
                    db.replace_task_workflow(
                        &task_id,
                        &extension.stage,
                        &extension.workflow_name,
                        &extension.validated.snapshot.definition_json,
                        extension.revision_rounds,
                        extension.validated.snapshot.revision_limit,
                        Some(crate::db::WorkflowReplacement {
                            expected_definition: &extension.previous_definition,
                            source: "agent",
                            superseded_run_ids: &extension.validated.superseded_run_ids,
                            changed_execution_stages: &extension
                                .validated
                                .changed_execution_stages,
                            ledger_result_id: Some(&result_entry.entry_id),
                        }),
                    )
                    .map_err(|e| db_write_error("db error", e))?;
                }
                Ok(())
            };
            db.with_immediate_transaction(|db| record(db).map_err(PlanExtensionTxError))
                .map_err(|error| error.0)?;
            if payload_verdict.completes_stage() {
                if let Some(pr_url) =
                    pr_url_from_verdict(payload_metadata.as_ref(), &payload_summary)
                {
                    db.update_pipeline_item_pr(&task_id, pr_number_from_url(&pr_url), &pr_url)
                        .map_err(|e| db_write_error("db error", e))?;
                }
            }
            // Recorded on failure too: a reviewer that could not finish its
            // brief may still have resolved which PR it was looking at, and
            // the operator's control needs that identity regardless of the
            // verdict. Refreshing deliberately bumps the context version, so
            // an earlier decision taken against the old version reads as
            // stale rather than being carried forward onto a new head.
            if let Some(context) = review_context.as_ref() {
                db.upsert_task_review_context(&task_id, context)
                    .map_err(|error| {
                        (
                            axum::http::StatusCode::BAD_REQUEST,
                            format!("invalid review context: {error}"),
                        )
                    })?;
            }
            Ok((task_id, finished_run, false, false, extension.is_some()))
        })
        .await?
    };

    if let (Some(run_id), Some(attempt_key)) = (
        completion_run_id.as_deref(),
        completion_attempt_key.as_deref(),
    ) {
        mark_completion_context_succeeded(&state.config.daemon_dir, &task_id, run_id, attempt_key);
    }

    let mut workflow_extended = workflow_extended.then_some(true);
    if already_closed || replayed {
        // A replay of the exact completion that published the stages writes
        // nothing, but it must still answer that they are published: the
        // caller reads a missing flag as "the stages were NOT published",
        // which is the honest answer only for a server that ignored the
        // arguments.
        if workflow_extension.is_some() && replayed {
            let state = Arc::clone(&state);
            let task_id = task_id.clone();
            workflow_extended = super::blocking::run_handler_blocking(
                "stage completion extension check",
                move || {
                    let db = Db::open(&state.config.db_path)
                        .map_err(|error| db_write_error("db error", error))?;
                    // Confirm the publication this request asked for, not
                    // merely that some plan was once published here.
                    Ok(recorded_plan_publication(&db, &task_id)
                        .is_some_and(|record| record.request_digest == requested_digest)
                        .then_some(true))
                },
            )
            .await?;
        }
        // A retry is how a caller finishes a completion whose ledger entry
        // could not be published the first time: it publishes what is still
        // pending and dispatches the transition the original still owes,
        // exactly once. It never answers success ahead of the file.
        if let Some(owed) = settle_ledger_continuation(&state, &task_id, replayed).await? {
            dispatch_owed_transition(Arc::clone(&state), task_id.clone(), owed, task_mutation)
                .await?;
        }
        return Ok(Json(crate::mobile_api::TaskActionResponse {
            task_id,
            follow_task: None,
            revision_budget: None,
            workflow_extended,
        }));
    }

    // Durable completion is announced, and its transition dispatched, only
    // once the result's ledger entry is on disk. A failure here leaves the
    // accepted result and its continuation pending; nothing is re-recorded.
    let owed = settle_ledger_continuation(&state, &task_id, true).await?;
    let response = crate::mobile_api::TaskActionResponse {
        task_id: task_id.clone(),
        follow_task: None,
        revision_budget: None,
        workflow_extended,
    };
    let Some(owed) = owed.filter(|_| should_auto_advance) else {
        state.publish_state_changed(StateChangeScope::Tasks);
        return Ok(Json(response));
    };
    dispatch_owed_transition(Arc::clone(&state), task_id, owed, task_mutation).await?;
    Ok(Json(response))
}

/// Enqueue the ledger entry for a result `complete_stage` accepted.
///
/// A run's first result is sourced by the run id itself; a corrected verdict
/// on the same run (which completion has always accepted) is a new fact with
/// its own source identity, never a rewrite of the first.
#[allow(clippy::too_many_arguments)]
fn enqueue_completion_result(
    db: &Db,
    task_id: &str,
    run: &crate::db::StageRun,
    observed: &crate::task_store::WorkspaceObservation,
    status: &str,
    message: &str,
    metadata: Option<&serde_json::Value>,
    stage_result: &str,
    requested_digest: Option<&str>,
    event_floor: i64,
) -> rusqlite::Result<crate::db::task_store::LedgerEntryRef> {
    use sha2::{Digest, Sha256};
    let prior = db.ledger_result_count_for_run(task_id, &run.id)?;
    let source_id = if prior == 0 {
        run.id.clone()
    } else {
        format!("{}#{}", run.id, prior + 1)
    };
    let mut hasher = Sha256::new();
    hasher.update(stage_result.as_bytes());
    hasher.update(requested_digest.unwrap_or("").as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let operation_id = format!("complete-stage:{source_id}:{}", &digest[..16]);
    db.enqueue_ledger_entry(crate::db::task_store::NewLedgerEntry {
        task_id,
        kind: crate::db::task_store::LedgerEntryKind::Result,
        operation_id: Some(&operation_id),
        source_kind: "stage_run",
        source_id: &source_id,
        source_origin: None,
        historical: false,
        recorded_at: None,
        run_id: Some(&run.id),
        declared_role: None,
        body: crate::task_store::result_body(
            status,
            run,
            observed,
            metadata,
            serde_json::json!({
                "kind": "complete_stage",
                "publishesWorkflow": requested_digest.is_some(),
            }),
        ),
        message: Some(message),
        hold_events_after: Some(event_floor),
        reserved_sequence: None,
    })
}

/// A transition an accepted operation still owes, taken from its durable
/// ledger continuation once that operation's entries are published.
enum OwedTransition {
    Completion(crate::db::FinishedStageRun),
    Revision {
        target_stage: String,
        prompt: String,
        round: Option<crate::task_creator::RevisionRound>,
    },
}

/// Publish the task's pending ledger entries and, when `claim` is set, take
/// the transition an accepted completion or revision still owes.
///
/// A continuation is fenced to the stage and run that recorded it. If the
/// task has since moved (an operator advanced it, a rerun or another
/// transition replaced the run) the continuation is stale: it is consumed
/// and discarded, never dispatched against the task's current stage.
async fn settle_ledger_continuation(
    state: &Arc<AppState>,
    task_id: &str,
    claim: bool,
) -> Result<Option<OwedTransition>, (axum::http::StatusCode, String)> {
    let state = Arc::clone(state);
    let task_id = task_id.to_string();
    super::blocking::run_handler_blocking("ledger continuation publication", move || {
        let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
        crate::task_store::flush_task(&db, &state.config.db_path, &task_id).map_err(|error| {
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "the result was recorded, but its ledger entry is not yet published, so \
                     nothing downstream has been told and no transition was dispatched: {error}. \
                     Retry the same completion; it is recognized as a retry, records nothing \
                     new, and finishes the publication. Kanna also retries on its own."
                ),
            )
        })?;
        if !claim {
            return Ok(None);
        }
        let Some(continuation) = db
            .claim_ledger_continuation(&task_id)
            .map_err(|e| db_write_error("db error", e))?
        else {
            return Ok(None);
        };
        if !continuation_still_owed(&db, &continuation)
            .map_err(|e| db_write_error("db error", e))?
        {
            log::warn!(
                "discarding stale {} continuation {} for task {task_id}: the task moved on",
                continuation.kind,
                continuation.operation_id
            );
            return Ok(None);
        }
        Ok(owed_transition(&continuation))
    })
    .await
}

/// Is the task still where the continuation's operation left it?
fn continuation_still_owed(
    db: &Db,
    continuation: &crate::db::task_store::LedgerContinuation,
) -> rusqlite::Result<bool> {
    let Some(item) = db.get_pipeline_item(&continuation.task_id)? else {
        return Ok(false);
    };
    if item.closed_at.is_some() {
        return Ok(false);
    }
    let payload = &continuation.payload;
    // Every continuation is written with its fence; one without it cannot be
    // proven current.
    let Some(stage) = payload.get("stage").and_then(serde_json::Value::as_str) else {
        return Ok(false);
    };
    if item.stage.as_deref() != Some(stage) {
        return Ok(false);
    }
    match payload.get("runId").and_then(serde_json::Value::as_str) {
        Some(run_id) => Ok(db
            .latest_stage_run(&continuation.task_id)?
            .is_some_and(|latest| latest.id == run_id)),
        None => Ok(true),
    }
}

fn owed_transition(
    continuation: &crate::db::task_store::LedgerContinuation,
) -> Option<OwedTransition> {
    let payload = &continuation.payload;
    let text = |key: &str| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    match continuation.kind.as_str() {
        crate::db::task_store::STAGE_COMPLETION_CONTINUATION => {
            Some(OwedTransition::Completion(crate::db::FinishedStageRun {
                kind: text("kind")?,
                completion_transition: text("completionTransition"),
                trigger: text("trigger").unwrap_or_else(|| "unspecified".to_string()),
            }))
        }
        crate::db::task_store::REVISION_CONTINUATION => Some(OwedTransition::Revision {
            target_stage: text("targetStage")?,
            prompt: text("prompt")?,
            round: payload.get("round").and_then(|round| {
                Some(crate::task_creator::RevisionRound {
                    number: round.get("number")?.as_i64()?,
                    limit: round.get("limit")?.as_i64()?,
                })
            }),
        }),
        other => {
            log::error!(
                "dropping unknown ledger continuation {other} for task {}",
                continuation.task_id
            );
            None
        }
    }
}

/// Dispatch an owed transition under the caller's task mutation lease.
async fn dispatch_owed_transition(
    state: Arc<AppState>,
    task_id: String,
    owed: OwedTransition,
    task_mutation: super::state::RequestedTaskMutation,
) -> Result<(), (axum::http::StatusCode, String)> {
    match owed {
        OwedTransition::Completion(finished_run) => {
            dispatch_completion_transition(state, task_id, finished_run, task_mutation).await
        }
        OwedTransition::Revision {
            target_stage,
            prompt,
            round,
        } => {
            // The round was spent and the reviewer's run finished when the
            // revision was accepted; only the reviser's spawn is still owed.
            let prepared = {
                let state = Arc::clone(&state);
                let task_id = task_id.clone();
                super::blocking::run_handler_blocking("owed revision prepare", move || {
                    let db = Db::open(&state.config.db_path)
                        .map_err(|e| db_write_error("db error", e))?;
                    crate::task_creator::prepare_revision_task_for_api(
                        &db,
                        &state.config,
                        &task_id,
                        &target_stage,
                        &prompt,
                        round,
                    )
                    .map_err(|e| {
                        record_stage_transition_failure(&state, &task_id, &e);
                        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e)
                    })
                })
                .await?
            };
            execute_stage_transition_detached_holding(
                Arc::clone(&state),
                task_id,
                crate::task_creator::PreparedStageTransition::Run(Box::new(prepared)),
                StageTransitionOwnership {
                    task_mutation: Some(task_mutation),
                    requested_operation: None,
                },
            );
            state.publish_state_changed(StateChangeScope::Tasks);
            Ok(())
        }
    }
}

/// Prepare and dispatch the transition a published completion asked for.
async fn dispatch_completion_transition(
    state: Arc<AppState>,
    task_id: String,
    finished_run: crate::db::FinishedStageRun,
    task_mutation: super::state::RequestedTaskMutation,
) -> Result<(), (axum::http::StatusCode, String)> {
    let transition = {
        let state = Arc::clone(&state);
        let task_id = task_id.clone();
        super::blocking::run_handler_blocking("stage completion prepare", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            crate::task_creator::prepare_stage_completion_for_api_with_trigger(
                &db,
                &state.config,
                &task_id,
                Some(finished_run.kind.as_str()),
                finished_run.completion_transition.as_deref(),
                Some(finished_run.trigger.as_str()),
            )
            .map_err(|e| {
                record_stage_transition_failure(&state, &task_id, &e);
                (stage_action_error_status(&e), e)
            })
        })
        .await?
    };
    let Some(transition) = transition else {
        state.publish_state_changed(StateChangeScope::Tasks);
        // Parked at a manual-transition stage. When that stage is `pr` and
        // the PR exists, the task's work is final enough for dependents:
        // resolve blockers optimistically instead of waiting for the human
        // review/merge loop to close the task. Best-effort — a failure here
        // must not fail the recorded completion.
        unblock_dependents_of_pr_resolved_blocker(&state, &task_id).await;
        return Ok(());
    };
    execute_stage_transition_detached_holding(
        Arc::clone(&state),
        task_id,
        transition,
        StageTransitionOwnership {
            task_mutation: Some(task_mutation),
            requested_operation: None,
        },
    );
    state.publish_state_changed(StateChangeScope::Tasks);
    Ok(())
}

/// Finish completions whose ledger publication failed earlier: publish what
/// is pending and dispatch the owed transition, each under the task's
/// mutation lease so it cannot race the request that recorded it. Run at
/// startup before anything else can dispatch, and by the publisher service.
pub(crate) async fn resume_ledger_continuations(state: Arc<AppState>) {
    let task_ids = {
        let state = Arc::clone(&state);
        tokio::task::spawn_blocking(move || {
            Db::open(&state.config.db_path).and_then(|db| db.ledger_continuation_task_ids())
        })
        .await
    };
    let task_ids = match task_ids {
        Ok(Ok(task_ids)) => task_ids,
        Ok(Err(error)) => {
            log::error!("failed to list ledger continuations: {error}");
            return;
        }
        Err(error) => {
            log::error!("ledger continuation worker failed: {error}");
            return;
        }
    };
    for task_id in task_ids {
        let task_mutation = state.begin_requested_task_mutation(&task_id).await;
        match settle_ledger_continuation(&state, &task_id, true).await {
            Ok(Some(owed)) => {
                if let Err((_, error)) = dispatch_owed_transition(
                    Arc::clone(&state),
                    task_id.clone(),
                    owed,
                    task_mutation,
                )
                .await
                {
                    log::error!("owed stage transition for {task_id} failed: {error}");
                }
            }
            Ok(None) => {}
            Err((_, error)) => {
                log::warn!("ledger continuation for {task_id} still pending: {error}")
            }
        }
    }
}

fn mark_completion_context_succeeded(
    daemon_dir: &str,
    task_id: &str,
    run_id: &str,
    attempt_key: &str,
) {
    let directory = std::path::Path::new(daemon_dir)
        .join("runtime")
        .join("completion");
    let task_path = directory.join(format!("task-{task_id}.json"));
    let legacy_path = directory.join(format!("{run_id}.json"));
    let legacy_shared_path =
        kanna_runtime_defaults::socket_path(&std::path::Path::new(daemon_dir).join("pipeline"))
            .parent()
            .unwrap_or(std::path::Path::new(daemon_dir))
            .join("runtime")
            .join("completion")
            .join(format!("{run_id}.json"));
    let path = if legacy_path.exists() {
        Some(legacy_path)
    } else if task_path.exists() {
        Some(task_path)
    } else if legacy_shared_path.exists() {
        Some(legacy_shared_path)
    } else {
        find_rebound_completion_context(&directory, task_id, run_id).or_else(|| {
            legacy_shared_path
                .parent()
                .and_then(|directory| find_rebound_completion_context(directory, task_id, run_id))
        })
    };
    let Some(path) = path else {
        return;
    };
    if let Err(error) = kanna_tool_catalog::mutate_completion_context(&path, |current| {
        let mut context =
            current.ok_or_else(|| format!("completion context {} disappeared", path.display()))?;
        context.record_completed_attempt(run_id, attempt_key);
        Ok(context)
    }) {
        log::warn!("failed to persist completion retry binding for {run_id}: {error}");
    }
}

fn find_rebound_completion_context(
    directory: &std::path::Path,
    task_id: &str,
    run_id: &str,
) -> Option<std::path::PathBuf> {
    let prefix = format!("run-{task_id}-");
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().and_then(|extension| extension.to_str()) == Some("json")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&prefix))
                && kanna_tool_catalog::read_completion_context(path)
                    .is_ok_and(|context| context.run_id == run_id)
        })
}

/// Optimistic blocker resolution: a task parked at the `pr` stage with a
/// created PR has committed, reviewed, rebased, and pushed work — dependents
/// can start stacking on its branch now rather than waiting for the PR to
/// merge and the task to close. Dormant dependents whose blockers are all
/// resolved get started; dependents with a live workspace get a session
/// message naming the resolved branch. The close path later delivers the
/// "closed" variant of the same message as a catch-up.
async fn unblock_dependents_of_pr_resolved_blocker(state: &Arc<AppState>, task_id: &str) {
    let instructions = {
        let state = Arc::clone(state);
        let task_id = task_id.to_string();
        let gathered = tokio::task::spawn_blocking(move || {
            let db = match Db::open(&state.config.db_path) {
                Ok(db) => db,
                Err(error) => {
                    log::error!("optimistic unblock for {task_id}: db error: {error}");
                    return None;
                }
            };
            let item = match db.get_pipeline_item(&task_id) {
                Ok(Some(item)) => item,
                Ok(None) => return None,
                Err(error) => {
                    log::error!("optimistic unblock for {task_id}: db error: {error}");
                    return None;
                }
            };
            let parked_at_pr_with_pr = item.closed_at.is_none()
                && item.stage.as_deref() == Some("pr")
                && item.pr_url.is_some();
            if !parked_at_pr_with_pr {
                return None;
            }
            match db.list_tasks_blocked_by(&task_id) {
                Ok(dependents) if dependents.is_empty() => return None,
                Ok(_) => {}
                Err(error) => {
                    log::error!("optimistic unblock for {task_id}: db error: {error}");
                    return None;
                }
            }
            match collect_blocker_resolution_instructions(
                &db,
                &task_id,
                BlockerResolution::PrCreated,
            ) {
                Ok(instructions) => Some(instructions),
                Err((_, error)) => {
                    log::error!("optimistic unblock for {task_id}: {error}");
                    Some(Vec::new())
                }
            }
        })
        .await;
        match gathered {
            Ok(Some(instructions)) => instructions,
            Ok(None) => return,
            Err(join_error) => {
                log::error!("optimistic unblock worker failed: {join_error}");
                return;
            }
        }
    };

    let mut daemon =
        match crate::daemon_client::DaemonClient::connect(&state.config.daemon_dir).await {
            Ok(daemon) => daemon,
            Err(error) => {
                log::error!("optimistic unblock for {task_id}: daemon error: {error}");
                return;
            }
        };
    for (session_id, message) in instructions {
        if let Err((_, error)) = submit_task_input(&mut daemon, &session_id, &message).await {
            log::warn!(
                "failed to deliver blocker-resolution instructions to dependent session {session_id}: {error}"
            );
        }
    }
    start_dependents_unblocked_by_close_with_daemon(state, &mut daemon, task_id).await;
    state.publish_state_changed(StateChangeScope::Tasks);
    state.publish_state_changed(StateChangeScope::Blockers);
}

pub(super) async fn request_revision(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    Json(payload): Json<crate::mobile_api::RequestRevisionRequest>,
) -> Result<Json<crate::mobile_api::TaskActionResponse>, (axum::http::StatusCode, String)> {
    let task_id = resolve_task_id_for_mutation(&state, &task_id).await?;
    let Some(revision_in_flight) = state.begin_requested_task_revision(&task_id) else {
        return Err((
            axum::http::StatusCode::CONFLICT,
            format!("a revision is already in progress for task {task_id}"),
        ));
    };
    let task_mutation = state.begin_requested_task_mutation(&task_id).await;

    #[cfg(test)]
    if let Some(revision_requester) = state.revision_requester.clone() {
        return revision_requester(task_id, payload)
            .map(Json)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e));
    }

    let origin = payload.origin.unwrap_or_default();
    // The reviewer's findings live in `prompt`; `summary` is only the headline
    // shown to the user. An agent that sends an empty `prompt` starts an agent
    // with nothing to act on and burns a budgeted round proving it, so the
    // request is refused here — before the round is claimed and before the
    // review run is closed — leaving the reviewer able to retry with the
    // findings. A human request is never refused: the compose path falls back
    // to the terminating run's verdict for it.
    if origin.is_agent() && payload.prompt.trim().is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "revision request carried no reviewer feedback: `prompt` must contain the findings \
             the revising agent has to act on (what is wrong, where, and what must change). \
             `summary` is only the one-line headline shown to the user. No revision was started \
             and no revision round was spent — retry this request with the findings in `prompt`."
                .to_string(),
        ));
    }
    // `resolve_task_id_for_mutation` returned the durable id before either
    // ownership guard was acquired. The nonblocking revision guard is taken
    // before waiting for the broader mutation lease so a duplicate revision
    // is refused immediately instead of sleeping until the first transition
    // lands and then spending another round.
    let source_task_id = task_id;

    let outcome = {
        let state = Arc::clone(&state);
        let source_task_id = source_task_id.clone();
        super::blocking::run_handler_blocking("revision prepare", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {}", e),
                )
            })?;
            let mut payload = payload;
            payload.run_id = validate_revision_run_binding(
                &db,
                &source_task_id,
                payload.run_id.as_deref(),
                origin,
            )?;
            let budget = match crate::task_creator::resolve_revision_budget(&db, &source_task_id) {
                Ok(budget) => budget,
                Err(error) => return Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, error)),
            };

            if origin.is_agent() && budget.limit > 0 && budget.rounds >= budget.limit {
                return park_exhausted_revision(&db, source_task_id, &payload, budget);
            }

            // Only a capped agent round is announced to the revising agent as
            // a round; a human-requested revision is the human's own call.
            let round = (origin.is_agent() && budget.limit > 0).then_some(
                crate::task_creator::RevisionRound {
                    number: budget.rounds + 1,
                    limit: budget.limit,
                },
            );
            // Historically an empty explicit prompt fell back to the verdict
            // after that verdict had already been written to the current run.
            // Resolve the same effective feedback before preparation so a
            // preparation error cannot leave that run falsely terminated.
            let revision_prompt = if payload.prompt.trim().is_empty() {
                &payload.summary
            } else {
                &payload.prompt
            };
            // The reviewer's verdict is the result that causes the revision,
            // so the reviser's session is prepared naming it. Its ledger
            // sequence is reserved now and filled by the transaction below;
            // the session starts only after that entry is published.
            let review_result =
                reserve_revision_result(&db, &source_task_id, &payload, revision_prompt)?;
            let prepare = || {
                crate::task_creator::prepare_revision_task_for_api(
                    &db,
                    &state.config,
                    &source_task_id,
                    &payload.target_stage,
                    revision_prompt,
                    round,
                )
            };
            let prepared = match review_result.as_ref() {
                Some(review) => crate::task_store::with_pending_trigger(
                    review.trigger(&source_task_id),
                    prepare,
                ),
                None => prepare(),
            };
            let prepared = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    release_revision_result(&db, &source_task_id, review_result.as_ref());
                    return Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, error));
                }
            };
            let stage_result = revision_stage_result(&payload.summary, &payload.metadata)?;
            // The event reports the budget this revision leaves behind: an
            // agent round consumes one, a human request resets the counter,
            // which is the same write the transaction below makes. Reporting
            // the pre-reset count made a human revision announce a spent
            // budget (`rounds == limit`) alongside `exhausted: false`.
            let revision_event_payload = revision_requested_event_payload(
                &payload,
                if origin.is_agent() {
                    budget.rounds + 1
                } else {
                    0
                },
                budget.limit,
                false,
            );
            let finalized = db.with_immediate_transaction(|db| -> rusqlite::Result<i64> {
                let event_floor = db.ledger_event_floor()?;
                let rounds = if origin.is_agent() {
                    db.claim_agent_revision_round_in_transaction(&source_task_id, budget.limit)?
                        .ok_or(rusqlite::Error::QueryReturnedNoRows)?
                } else {
                    db.reset_task_revision_rounds(&source_task_id)?;
                    0
                };
                if let Some(run_id) = payload.run_id.as_deref() {
                    db.finish_stage_run(
                        run_id,
                        "failed",
                        Some(&stage_result),
                        Some(&payload.summary),
                    )?;
                }
                db.append_task_event(
                    &source_task_id,
                    crate::db::TaskEventKind::RevisionRequested,
                    revision_event_payload,
                )?;
                // The budget counter above is not a history — a human request
                // resets it — and the event is pruned after 14 days. Analytics
                // reads this row instead, written in the same transaction so
                // the record and the revision cannot disagree.
                db.record_revision_request_in_transaction(
                    &source_task_id,
                    recorded_revision_origin(origin),
                    Some(payload.target_stage.as_str()),
                    true,
                )?;
                if let Some(review) = review_result.as_ref() {
                    review.enqueue(db, &source_task_id, &payload, origin, event_floor)?;
                }
                // The reviser's spawn is owed from this commit on, whatever
                // happens to the detached worker: the round is spent and the
                // reviewer's run is finished. The continuation is what lets
                // the publisher start the reviser if publication has to wait,
                // fenced to the stage and reviewer run this request saw. It
                // supersedes any transition an earlier completion still owed.
                let source_stage = db
                    .get_pipeline_item(&source_task_id)?
                    .and_then(|item| item.stage);
                db.put_ledger_continuation(
                    &source_task_id,
                    &format!("revision:{source_task_id}:{event_floor}"),
                    crate::db::task_store::REVISION_CONTINUATION,
                    &serde_json::json!({
                        "stage": source_stage,
                        "runId": payload.run_id,
                        "targetStage": payload.target_stage,
                        "prompt": revision_prompt,
                        "round": round.map(|round| serde_json::json!({
                            "number": round.number,
                            "limit": round.limit,
                        })),
                    }),
                )?;
                Ok(rounds)
            });
            let rounds = match finalized {
                Ok(rounds) => rounds,
                Err(error) => {
                    release_revision_result(&db, &source_task_id, review_result.as_ref());
                    let error = crate::task_creator::rollback_prepared_stage_run_for_api(
                        &prepared,
                        format!("db error: {error}"),
                    );
                    return Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, error));
                }
            };
            let budget = crate::task_creator::RevisionBudget {
                rounds,
                limit: budget.limit,
            };
            Ok(RevisionOutcome::Started {
                source_task_id,
                prepared: Box::new(prepared),
                budget,
            })
        })
        .await?
    };

    match outcome {
        RevisionOutcome::Parked {
            source_task_id,
            budget,
        } => {
            {
                let state = Arc::clone(&state);
                let task_id = source_task_id.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Ok(db) = Db::open(&state.config.db_path) {
                        crate::task_store::flush_task_best_effort(
                            &db,
                            &state.config.db_path,
                            &task_id,
                        );
                    }
                })
                .await;
            }
            state.publish_state_changed(StateChangeScope::Tasks);
            Ok(Json(crate::mobile_api::TaskActionResponse {
                task_id: source_task_id,
                follow_task: None,
                revision_budget: Some(crate::mobile_api::RevisionBudgetStatus {
                    rounds: budget.rounds,
                    limit: budget.limit,
                    exhausted: true,
                    message: format!(
                        "No revision was started: this task has already used its \
                         {limit} automatic revision round(s). The task is parked at its current \
                         stage for its human, who decides whether to revise again. Ask the human \
                         to explicitly authorize continuation in the agent terminal, then stop. \
                         Do not retry unless that instruction is given; relay it once with \
                         origin 'human'.",
                        limit = budget.limit,
                    ),
                }),
                workflow_extended: None,
            }))
        }
        RevisionOutcome::Started {
            source_task_id,
            prepared,
            budget,
        } => {
            // The reviser starts only once the reviewer's result is on disk,
            // and only by whoever claims the revision's continuation. Holding
            // the lease, this request normally does both; if publication has
            // to wait, the prepared workspace is rolled back and the publisher
            // starts the reviser from the continuation later — the round was
            // spent once and is not spent again.
            let owned = match settle_ledger_continuation(&state, &source_task_id, true).await {
                Ok(Some(OwedTransition::Revision { .. })) => true,
                Ok(_) => false,
                Err((_, error)) => {
                    log::warn!("revision of {source_task_id} waits for its ledger: {error}");
                    false
                }
            };
            if owned {
                // Ownership moves into the worker: the task stays claimed until
                // the revision has actually landed, not just until this response.
                execute_stage_transition_detached_holding(
                    Arc::clone(&state),
                    source_task_id.clone(),
                    crate::task_creator::PreparedStageTransition::Run(prepared),
                    StageTransitionOwnership {
                        task_mutation: Some(task_mutation),
                        requested_operation: Some(revision_in_flight),
                    },
                );
            } else {
                let _ = crate::task_creator::rollback_prepared_stage_run_for_api(
                    &prepared,
                    "revision start deferred until its ledger entry is published".to_string(),
                );
            }
            state.publish_state_changed(StateChangeScope::Tasks);

            let message = if !owned {
                "Revision accepted; the revising session starts once the reviewer's result is \
                 published to the task ledger. Do not request it again."
                    .to_string()
            } else if budget.limit > 0 && origin.is_agent() {
                format!(
                    "Revision round {rounds} of {limit} started.",
                    rounds = budget.rounds,
                    limit = budget.limit,
                )
            } else if budget.limit > 0 {
                format!(
                    "Revision started; the automatic revision budget was reset to 0 of {limit} \
                     round(s).",
                    limit = budget.limit,
                )
            } else {
                "Revision started; this workflow sets no revision-round limit.".to_string()
            };
            Ok(Json(crate::mobile_api::TaskActionResponse {
                task_id: source_task_id,
                follow_task: None,
                revision_budget: Some(crate::mobile_api::RevisionBudgetStatus {
                    rounds: budget.rounds,
                    limit: budget.limit,
                    exhausted: false,
                    message,
                }),
                workflow_extended: None,
            }))
        }
    }
}

/// Either a revision run to dispatch, or a parked task whose revision budget
/// is spent.
enum RevisionOutcome {
    Parked {
        source_task_id: String,
        budget: crate::task_creator::RevisionBudget,
    },
    Started {
        source_task_id: String,
        prepared: Box<crate::task_creator::PreparedStageRunSpawn>,
        budget: crate::task_creator::RevisionBudget,
    },
}

/// Record the review verdict, park the task at its current stage for its
/// human, and start nothing. Used when the revision-round budget is spent.
fn recorded_revision_origin(
    origin: crate::mobile_api::RevisionOrigin,
) -> crate::db::RecordedRevisionOrigin {
    if origin.is_agent() {
        crate::db::RecordedRevisionOrigin::Agent
    } else {
        crate::db::RecordedRevisionOrigin::Human
    }
}

fn park_exhausted_revision(
    db: &Db,
    source_task_id: String,
    payload: &crate::mobile_api::RequestRevisionRequest,
    budget: crate::task_creator::RevisionBudget,
) -> Result<RevisionOutcome, (axum::http::StatusCode, String)> {
    let parked_summary = format!(
        "Parked for human review: this task's automatic revision budget \
         ({limit} round(s)) is spent, so Kanna did not start another revision. \
         Review verdict: {summary}",
        limit = budget.limit,
        summary = payload.summary,
    );
    let parked_result = revision_stage_result(&parked_summary, &payload.metadata)?;
    let review_run = match payload.run_id.as_deref() {
        Some(run_id) => db
            .stage_run(run_id)
            .map_err(|error| db_write_error("db error", error))?,
        None => None,
    };
    let observed = match review_run.as_ref() {
        Some(run) => crate::task_store::observe_workspace(run.cwd.as_deref())
            .map_err(|error| (axum::http::StatusCode::CONFLICT, error))?,
        None => crate::task_store::WorkspaceObservation::none(),
    };
    db.with_immediate_transaction(|db| {
        park_exhausted_revision_in_transaction(
            db,
            &source_task_id,
            payload,
            &parked_result,
            &parked_summary,
            &budget,
            review_run.as_ref(),
            &observed,
        )
        .map_err(PlanExtensionTxError)
    })
    .map_err(|error| error.0)?;
    Ok(RevisionOutcome::Parked {
        source_task_id,
        budget,
    })
}

#[allow(clippy::too_many_arguments)]
fn park_exhausted_revision_in_transaction(
    db: &Db,
    source_task_id: &str,
    payload: &crate::mobile_api::RequestRevisionRequest,
    parked_result: &str,
    parked_summary: &str,
    budget: &crate::task_creator::RevisionBudget,
    review_run: Option<&crate::db::StageRun>,
    observed: &crate::task_store::WorkspaceObservation,
) -> Result<(), (axum::http::StatusCode, String)> {
    let source_task_id = source_task_id.to_string();
    let parked_result = parked_result.to_string();
    let event_floor = db
        .ledger_event_floor()
        .map_err(|error| db_write_error("db error", error))?;
    // The requested changes stay on the run as feedback so nothing the
    // reviewer found is lost when the loop stops.
    if let Some(run_id) = payload.run_id.as_deref() {
        db.finish_stage_run(
            run_id,
            "failed",
            Some(&parked_result),
            Some(&payload.prompt),
        )
        .map_err(|error| db_write_error("db error", error))?;
    }
    db.update_pipeline_item_activity(&source_task_id, "unread")
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {}", e),
            )
        })?;
    append_revision_requested_event(db, &source_task_id, payload, budget, true)?;
    // Recorded as a review verdict that started nothing (`applied = false`):
    // a parked request is churn the reviewer asked for but the task never
    // spent, and averaging it in with real rounds would overstate revisions.
    db.record_revision_request_in_transaction(
        &source_task_id,
        recorded_revision_origin(payload.origin.unwrap_or_default()),
        Some(payload.target_stage.as_str()),
        false,
    )
    .map_err(|error| db_write_error("db error", error))?;
    // The parked verdict is the result a person reads at this gate.
    if let Some(run) = review_run {
        let message = revision_ledger_message(parked_summary, &payload.prompt);
        let review = RevisionResult {
            run: run.clone(),
            observed: observed.clone(),
            message,
            reserved_sequence: None,
        };
        review
            .enqueue(
                db,
                &source_task_id,
                payload,
                payload.origin.unwrap_or_default(),
                event_floor,
            )
            .map_err(|error| db_write_error("db error", error))?;
    }
    Ok(())
}

/// A reviewer's verdict as the ledger result that causes (or parks) a
/// revision.
struct RevisionResult {
    run: crate::db::StageRun,
    observed: crate::task_store::WorkspaceObservation,
    message: String,
    reserved_sequence: Option<i64>,
}

impl RevisionResult {
    /// The triggering result the reviser's preamble names, before its entry
    /// is written.
    fn trigger(&self, task_id: &str) -> crate::task_store::TriggeringResult {
        let sequence = self.reserved_sequence.unwrap_or_default();
        crate::task_store::TriggeringResult {
            entry_id: crate::db::task_store::ledger_entry_id(task_id, sequence),
            file: format!(
                "ledger/{}",
                crate::db::task_store::ledger_file_name(
                    sequence,
                    crate::db::task_store::LedgerEntryKind::Result
                )
            ),
            status: "failure".to_string(),
            stage: Some(self.run.stage.clone()),
            run_id: Some(self.run.id.clone()),
            branch: self.observed.branch.clone(),
            committed_sha: self.observed.committed_sha.clone(),
            message: self.message.clone(),
        }
    }

    fn enqueue(
        &self,
        db: &Db,
        task_id: &str,
        payload: &crate::mobile_api::RequestRevisionRequest,
        origin: crate::mobile_api::RevisionOrigin,
        event_floor: i64,
    ) -> rusqlite::Result<crate::db::task_store::LedgerEntryRef> {
        let prior = db.ledger_result_count_for_run(task_id, &self.run.id)?;
        let source_id = if prior == 0 {
            self.run.id.clone()
        } else {
            format!("{}#{}", self.run.id, prior + 1)
        };
        let operation_id = format!("revision:{source_id}");
        db.enqueue_ledger_entry(crate::db::task_store::NewLedgerEntry {
            task_id,
            kind: crate::db::task_store::LedgerEntryKind::Result,
            operation_id: Some(&operation_id),
            source_kind: "stage_run",
            source_id: &source_id,
            source_origin: None,
            historical: false,
            recorded_at: None,
            run_id: Some(&self.run.id),
            declared_role: None,
            // The verdict a revision request records on the run it closes;
            // routing by exit instead of status is T1's.
            body: crate::task_store::result_body(
                "failure",
                &self.run,
                &self.observed,
                payload.metadata.as_ref(),
                serde_json::json!({
                    "kind": "revision_request",
                    "targetStage": payload.target_stage,
                    "origin": if origin.is_agent() { "agent" } else { "human" },
                }),
            ),
            message: Some(&self.message),
            hold_events_after: Some(event_floor),
            reserved_sequence: self.reserved_sequence,
        })
    }
}

/// The headline, then the findings the reviser must act on.
fn revision_ledger_message(summary: &str, findings: &str) -> String {
    let summary = summary.trim_end();
    let findings = findings.trim_end();
    if summary.trim().is_empty() {
        findings.to_string()
    } else if findings.trim().is_empty() || findings == summary {
        summary.to_string()
    } else {
        format!("{summary}\n\n{findings}")
    }
}

/// Resolve the reviewer's run and reserve its result's ledger sequence.
///
/// `None` when the request binds no run (a person asking for a revision is
/// not a session recording a result) or carries no text at all: an empty
/// message is never written to the ledger.
fn reserve_revision_result(
    db: &Db,
    task_id: &str,
    payload: &crate::mobile_api::RequestRevisionRequest,
    findings: &str,
) -> Result<Option<RevisionResult>, (axum::http::StatusCode, String)> {
    let Some(run_id) = payload.run_id.as_deref() else {
        return Ok(None);
    };
    let Some(run) = db
        .stage_run(run_id)
        .map_err(|error| db_write_error("db error", error))?
    else {
        return Ok(None);
    };
    let message = revision_ledger_message(&payload.summary, findings);
    if message.trim().is_empty() {
        return Ok(None);
    }
    let observed = crate::task_store::observe_workspace(run.cwd.as_deref())
        .map_err(|error| (axum::http::StatusCode::CONFLICT, error))?;
    let sequence = db
        .reserve_ledger_sequence(task_id)
        .map_err(|error| db_write_error("db error", error))?;
    Ok(Some(RevisionResult {
        run,
        observed,
        message,
        reserved_sequence: Some(sequence),
    }))
}

fn release_revision_result(db: &Db, task_id: &str, review: Option<&RevisionResult>) {
    if let Some(sequence) = review.and_then(|review| review.reserved_sequence) {
        if let Err(error) = db.release_ledger_reservation(task_id, sequence) {
            log::error!("failed to release ledger reservation {sequence} of {task_id}: {error}");
        }
    }
}

/// A revision request is a state change a watcher cares about whether or not it
/// started anything: an exhausted budget parks the task for its human, which is
/// exactly when an orchestrator must stop waiting for a fix and report.
fn append_revision_requested_event(
    db: &Db,
    task_id: &str,
    payload: &crate::mobile_api::RequestRevisionRequest,
    budget: &crate::task_creator::RevisionBudget,
    exhausted: bool,
) -> Result<(), (axum::http::StatusCode, String)> {
    db.append_task_event(
        task_id,
        crate::db::TaskEventKind::RevisionRequested,
        revision_requested_event_payload(payload, budget.rounds, budget.limit, exhausted),
    )
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("db error: {}", e),
        )
    })
}

fn revision_requested_event_payload(
    payload: &crate::mobile_api::RequestRevisionRequest,
    rounds: i64,
    limit: i64,
    exhausted: bool,
) -> serde_json::Value {
    serde_json::json!({
        "targetStage": payload.target_stage,
        "summary": payload.summary,
        "origin": if payload.origin.unwrap_or_default().is_agent() { "agent" } else { "human" },
        "rounds": rounds,
        "limit": limit,
        "exhausted": exhausted,
    })
}

/// The `{status, summary, metadata}` verdict JSON a revision request records
/// on the run it closes.
fn revision_stage_result(
    summary: &str,
    metadata: &Option<serde_json::Value>,
) -> Result<String, (axum::http::StatusCode, String)> {
    serde_json::to_string(&serde_json::json!({
        "status": "failure",
        "summary": summary,
        "metadata": metadata,
    }))
    .map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            format!("invalid revision result: {}", e),
        )
    })
}

/// Resolve an omitted identity from durable running state, never from the
/// adapter's lifetime. Explicit identities disambiguate multiple running runs;
/// callers retain their own mismatch and identical-retry checks. With no live
/// run, preserve the latest-run recovery/replay behavior.
fn resolve_action_run(
    db: &Db,
    task_id: &str,
    requested_run_id: Option<&str>,
) -> Result<Option<crate::db::StageRun>, (axum::http::StatusCode, String)> {
    if requested_run_id.is_some_and(|id| id.trim().is_empty()) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "runId must be non-empty when provided".into(),
        ));
    }
    let mut running = db
        .running_stage_runs_for_task(task_id)
        .map_err(|error| db_write_error("db error", error))?;
    match running.len() {
        0 => db
            .latest_stage_run(task_id)
            .map_err(|error| db_write_error("db error", error)),
        1 => Ok(running.pop()),
        _ => {
            if let Some(index) = running
                .iter()
                .position(|run| Some(run.id.as_str()) == requested_run_id)
            {
                return Ok(Some(running.remove(index)));
            }
            let ids = running
                .iter()
                .map(|run| run.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let received = requested_run_id.unwrap_or("omitted");
            Err((axum::http::StatusCode::CONFLICT, format!(
                "ambiguous stage action for task {task_id}: running runs are {ids}; supply runId matching one of them (received {received})"
            )))
        }
    }
}

/// Apply the same run resolution to revision verdicts, retaining the main-run
/// and running-status requirements for agents. Humans may revise parked runs.
fn validate_revision_run_binding(
    db: &Db,
    task_id: &str,
    requested_run_id: Option<&str>,
    origin: crate::mobile_api::RevisionOrigin,
) -> Result<Option<String>, (axum::http::StatusCode, String)> {
    let current_run = resolve_action_run(db, task_id, requested_run_id)?;
    if origin.is_agent() && requested_run_id.is_none() {
        if let Some(run) = current_run.as_ref().filter(|run| run.status != "running") {
            if db
                .stage_run_completion_bound(&run.id)
                .map_err(|error| db_write_error("db error", error))?
            {
                return Err((axum::http::StatusCode::CONFLICT, format!(
                    "no running review run for task {task_id}; current run {} is {}. No revision was started and no revision round was spent.",
                    run.id, run.status
                )));
            }
        }
    }
    let inferred_run_id = current_run
        .as_ref()
        .filter(|run| run.status == "running")
        .map(|run| run.id.clone());
    let requested_run_id = requested_run_id.or(inferred_run_id.as_deref());
    if let Some(requested_run_id) = requested_run_id {
        let Some(current_run) = current_run else {
            return Err((
                axum::http::StatusCode::CONFLICT,
                format!(
                    "review run {requested_run_id} cannot conclude task {task_id}: the task has no stage run"
                ),
            ));
        };
        if current_run.id != requested_run_id || current_run.task_id != task_id {
            let owner = db
                .stage_run(requested_run_id)
                .map_err(|error| db_write_error("db error", error))?
                .map(|run| run.task_id)
                .unwrap_or_else(|| "no task".to_string());
            return Err((
                axum::http::StatusCode::CONFLICT,
                format!(
                    "review run {requested_run_id} belongs to {owner} and cannot conclude task \
                     {task_id}; that task's current run is {}. No revision was started and no \
                     revision round was spent.",
                    current_run.id
                ),
            ));
        }
        if origin.is_agent() && (current_run.kind != "main" || current_run.status != "running") {
            return Err((
                axum::http::StatusCode::CONFLICT,
                format!(
                    "review run {requested_run_id} is {} {} and cannot issue a new revision. No \
                     revision was started and no revision round was spent.",
                    current_run.status, current_run.kind
                ),
            ));
        }
        return Ok(inferred_run_id);
    }

    Ok(None)
}

#[cfg(test)]
mod notification_failure_tests {
    use super::*;

    #[tokio::test]
    async fn detached_transition_without_daemon_publishes_actionable_failure() {
        let state = super::super::test_support::test_state_with_seed(
            "transition-notification",
            "Transition",
            |db| {
                db.insert_test_repo("repo", "Repo").unwrap();
                db.insert_test_pipeline_item(
                    "task",
                    "repo",
                    "work",
                    None,
                    "pr",
                    "2026-09-09 00:00:00",
                )
                .unwrap();
            },
        );
        // The fixture owns an isolated daemon directory with no daemon.
        execute_stage_transition_detached_holding(
            state.clone(),
            "task".into(),
            crate::task_creator::PreparedStageTransition::Close {
                task_id: "task".into(),
                workspace_teardown: None,
            },
            StageTransitionOwnership::default(),
        );
        let page = super::super::task_events::wait_subscription_events(
            state.clone(),
            serde_json::json!({"taskIds":"task", "localOnly":true,
                "includeCurrentActivity":false, "timeoutSecs":5}),
            std::sync::Arc::new(std::sync::Mutex::new(
                super::super::subscription_timing::Collection::default(),
            )),
        )
        .await
        .unwrap();
        assert_eq!(page["events"].as_array().unwrap().len(), 1, "{page}");
        assert_eq!(page["events"][0]["type"], "task.lifecycle_failed");
        assert_eq!(
            page["events"][0]["payload"]["operation"],
            "stage_transition"
        );
        assert!(page["events"][0]["payload"]["error"].is_string());
        assert!(Db::open(&state.config.db_path)
            .unwrap()
            .get_pipeline_item("task")
            .unwrap()
            .unwrap()
            .closed_at
            .is_none());
    }
}

#[cfg(test)]
mod harness_request_tests {
    #[test]
    fn advance_harness_alias_refuses_both_spellings() {
        let request: super::AdvanceStageRequest =
            serde_json::from_value(serde_json::json!({"nextStageHarness":"codex"})).unwrap();
        assert_eq!(request.next_stage_agent_provider.as_deref(), Some("codex"));
        assert!(serde_json::from_value::<super::AdvanceStageRequest>(
            serde_json::json!({"nextStageHarness":"codex", "nextStageAgentProvider":"codex"})
        )
        .is_err());
    }
}
