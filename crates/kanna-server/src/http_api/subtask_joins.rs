//! Subtask joins through the HTTP API (spec §9, §16.4 — component T5).
//!
//! `POST /v1/tasks/{task_id}/subtasks` creates children in a join: the join
//! and every member are recorded first, pinned to the parent's committed
//! HEAD, and then each child is created with its recorded id, forked from
//! that commit and parented to the task. `GET /v1/tasks/{task_id}/joins`
//! reads where each join stands.
//!
//! Resolution and delivery are decided in SQLite (see `db::subtask_joins`),
//! in the transaction that records the child's result or close. This module
//! adds what cannot happen inside a transaction: creating the children, and
//! typing each delivered outcome into the parent's live session as a notice.
//! The notice is claimed before it is typed and given back only when nothing
//! was written, so a restart or a racing sweep never types one twice; the
//! durable delivery is the parent's input ledger either way.

use super::state::{db_write_error, AppState};
use super::task_input::TaskInputError;
use crate::db::{Db, NewJoinMember, NewTaskJoin, TaskJoin, TaskJoinMember};
use axum::extract::State;
use axum::Json;
use kanna_agent_protocol::StateChangeScope;
use kanna_daemon::protocol::{
    Command as DaemonCommand, ComposerAttestation, Event as DaemonEvent, SessionKind, SessionState,
};
use serde_json::{json, Value};
use std::sync::Arc;

/// Most children one join may create.
const MAX_JOIN_CHILDREN: usize = 32;

/// One child to create. Its repository, parent and fork point are the
/// join's, never the caller's; unknown fields are refused rather than
/// silently dropped.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubtaskSpec {
    prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workflow_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<String>,
    #[serde(default, alias = "harness", skip_serializing_if = "Option::is_none")]
    agent_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    allowed_tools: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateSubtasksRequest {
    children: Vec<SubtaskSpec>,
}

/// The create request a member's spec stands for, bound to its join.
fn member_create_request(
    repo_id: &str,
    parent_task_id: &str,
    base_sha: &str,
    spec: &SubtaskSpec,
) -> Result<crate::mobile_api::CreateTaskRequest, String> {
    let mut request = serde_json::to_value(spec).map_err(|error| error.to_string())?;
    let object = request
        .as_object_mut()
        .ok_or_else(|| "subtask spec is not an object".to_string())?;
    object.insert("repoId".into(), json!(repo_id));
    object.insert("parentTaskId".into(), json!(parent_task_id));
    object.insert("baseRef".into(), json!(base_sha));
    serde_json::from_value(request).map_err(|error| error.to_string())
}

fn conflict(message: String) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::CONFLICT, message)
}

/// Record a join for `parent` pinned to its committed HEAD, with one member
/// per spec. Nothing is created yet.
fn record_join(
    db: &Db,
    parent_ref: &str,
    specs: &[SubtaskSpec],
) -> Result<(TaskJoin, Vec<TaskJoinMember>, String), (axum::http::StatusCode, String)> {
    let parent_task_id = db
        .resolve_pipeline_item_id(parent_ref)
        .map_err(|e| db_write_error("db error", e))?
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                format!("task not found: {parent_ref}"),
            )
        })?;
    let parent = db
        .get_pipeline_item(&parent_task_id)
        .map_err(|e| db_write_error("db error", e))?
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                format!("task not found: {parent_task_id}"),
            )
        })?;
    if parent.closed_at.is_some() {
        return Err(conflict(format!("task is closed: {parent_task_id}")));
    }
    let workspace = db
        .get_task_worktree_path(&parent_task_id)
        .map_err(|e| db_write_error("db error", e))?;
    // Children fork from what the parent has committed, never from its
    // uncommitted edits or a branch tip that moves on after this call.
    let observed = crate::task_store::observe_workspace(workspace.as_deref())
        .map_err(|error| conflict(format!("cannot read the parent's workspace: {error}")))?;
    let Some(base_sha) = observed.committed_sha.clone() else {
        return Err(conflict(format!(
            "task {parent_task_id} has no committed workspace to launch subtasks from ({})",
            observed.provenance
        )));
    };
    let parent_run_id = db
        .latest_stage_run(&parent_task_id)
        .map_err(|e| db_write_error("db error", e))?
        .map(|run| run.id);
    let mut members = Vec::with_capacity(specs.len());
    for spec in specs {
        let child_task_id = crate::task_creator::generate_new_task_id()
            .map_err(|error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error))?;
        member_create_request(&parent.repo_id, &parent_task_id, &base_sha, spec)
            .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error))?;
        members.push(NewJoinMember {
            child_task_id,
            spec: serde_json::to_string(spec)
                .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?,
        });
    }
    let join_id = format!(
        "join-{}",
        crate::task_creator::generate_new_task_id()
            .map_err(|error| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, error))?
    );
    let join = db
        .create_task_join(&NewTaskJoin {
            id: join_id,
            parent_task_id: parent_task_id.clone(),
            parent_stage: parent.stage.clone(),
            parent_run_id,
            base_sha,
            base_branch: observed.branch,
            members,
        })
        .map_err(|e| db_write_error("could not record the subtask join", e))?;
    let members = db
        .list_task_join_members(&join.id)
        .map_err(|e| db_write_error("db error", e))?;
    Ok((join, members, parent.repo_id))
}

/// Why a member was not created now, although nothing failed.
enum MemberSkip {
    /// Its task already exists, or it already resolved: nothing is owed.
    Done,
    /// Its parent closed; no child is created under a closed task.
    ParentClosed,
}

/// Durable state under the member's single-flight guard: a caller's snapshot
/// may be stale (a sweep lists members before creating them in turn), and
/// creating over an existing task would take the create path's repair
/// branch and restart a child whose result was already delivered.
fn member_skip(
    db: &Db,
    join: &TaskJoin,
    member: &TaskJoinMember,
) -> rusqlite::Result<Option<MemberSkip>> {
    let resolved = db
        .task_join_member(&member.child_task_id)?
        .is_none_or(|current| current.resolved_at.is_some());
    if resolved || db.get_pipeline_item(&member.child_task_id)?.is_some() {
        return Ok(Some(MemberSkip::Done));
    }
    let parent_open = db
        .get_pipeline_item(&join.parent_task_id)?
        .is_some_and(|parent| parent.closed_at.is_none());
    Ok((!parent_open).then_some(MemberSkip::ParentClosed))
}

/// Create one member's task from its recorded spec, under the same
/// single-flight guard a `PUT /v1/tasks/{id}` takes. Returns the error when
/// it was not created; a creation that fails before any task row exists
/// resolves the member as `not_created` so its parent is told. A member
/// that is already created or resolved is left alone.
pub(super) async fn create_member(
    state: &Arc<AppState>,
    join: &TaskJoin,
    repo_id: &str,
    member: &TaskJoinMember,
) -> Result<(), String> {
    let spec: SubtaskSpec = serde_json::from_str(&member.spec)
        .map_err(|error| format!("unreadable subtask spec: {error}"))?;
    let request = member_create_request(repo_id, &join.parent_task_id, &join.base_sha, &spec)?;
    let Some(_flight) = state.begin_requested_task_creation(&member.child_task_id) else {
        return Err(format!(
            "task creation already in progress: {}",
            member.child_task_id
        ));
    };
    let skip = {
        let db = Db::open(&state.config.db_path).map_err(|e| format!("db error: {e}"))?;
        member_skip(&db, join, member).map_err(|e| format!("db error: {e}"))?
    };
    match skip {
        Some(MemberSkip::Done) => return Ok(()),
        Some(MemberSkip::ParentClosed) => {
            return Err(format!(
                "parent task {} is closed; subtask {} was not created",
                join.parent_task_id, member.child_task_id
            ))
        }
        None => {}
    }
    let created = super::tasks::create_task_with_requested_id(
        Arc::clone(state),
        request,
        Some(member.child_task_id.clone()),
    )
    .await;
    let Err((_, error)) = created else {
        return Ok(());
    };
    let db = Db::open(&state.config.db_path).map_err(|e| format!("db error: {e}"))?;
    db.resolve_join_member_not_created(&member.child_task_id, &error)
        .map_err(|e| format!("db error: {e}"))?;
    Err(error)
}

pub(super) async fn create_subtasks(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    Json(payload): Json<CreateSubtasksRequest>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    if payload.children.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "children must name at least one subtask".to_string(),
        ));
    }
    if payload.children.len() > MAX_JOIN_CHILDREN {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("a join creates at most {MAX_JOIN_CHILDREN} subtasks"),
        ));
    }
    if let Some(index) = payload
        .children
        .iter()
        .position(|child| child.prompt.trim().is_empty())
    {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("children[{index}].prompt is empty"),
        ));
    }
    let parent_task_id =
        super::task_actions::resolve_task_id_for_mutation(&state, &task_id).await?;
    // Recorded under the parent's lease, which its own completion and
    // advance take, then released: creating children takes as long as
    // their setup does, and the parent must stay reachable meanwhile.
    let (join, members, repo_id) = {
        let _task_mutation = state.begin_requested_task_mutation(&parent_task_id).await;
        let state = Arc::clone(&state);
        let parent_task_id = parent_task_id.clone();
        let specs = payload.children;
        super::blocking::run_handler_blocking("subtask join record", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
            record_join(&db, &parent_task_id, &specs)
        })
        .await?
    };
    state.publish_state_changed(StateChangeScope::Tasks);
    state.publish_state_changed(StateChangeScope::Blockers);
    let mut children = Vec::with_capacity(members.len());
    for member in &members {
        let outcome = create_member(&state, &join, &repo_id, member).await;
        children.push(json!({
            "taskId": member.child_task_id,
            "position": member.position,
            "created": outcome.is_ok(),
            "error": outcome.err(),
        }));
    }
    state.publish_state_changed(StateChangeScope::Tasks);
    spawn_join_notices(&state);
    let view = {
        let state = Arc::clone(&state);
        let join_id = join.id.clone();
        super::blocking::run_handler_blocking("subtask join read", move || {
            let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
            let join = db
                .task_join(&join_id)
                .map_err(|e| db_write_error("db error", e))?
                .ok_or_else(|| conflict(format!("join {join_id} disappeared")))?;
            join_view(&db, &join).map_err(|e| db_write_error("db error", e))
        })
        .await?
    };
    Ok(Json(json!({
        "joinId": join.id,
        "parentTaskId": join.parent_task_id,
        "baseSha": join.base_sha,
        "children": children,
        "join": view,
    })))
}

/// Where one member stands, and what can move it when it is stuck.
fn member_view(db: &Db, member: &TaskJoinMember) -> Result<Value, rusqlite::Error> {
    let item = db.get_pipeline_item(&member.child_task_id)?;
    let latest_run = match item.as_ref() {
        Some(_) => db.latest_stage_run(&member.child_task_id)?,
        None => None,
    };
    let (state, actions, detail): (&str, Vec<&str>, Option<String>) = if member
        .resolved_at
        .is_some()
    {
        ("resolved", Vec::new(), None)
    } else if let Some(item) = item.as_ref() {
        let session_ended = item.runtime_status.as_deref() == Some("exited");
        match latest_run.as_ref() {
            Some(run) if run.status == "running" && !session_ended => ("running", Vec::new(), None),
            // No result and no live session: counted as nothing until a
            // person or manager resumes, reruns or closes it.
            Some(run) => (
                "stalled",
                vec!["kanna_resume_task", "kanna_rerun_stage", "kanna_close_task"],
                Some(format!(
                    "no result recorded; latest run {} is {}{}",
                    run.id,
                    run.status,
                    if session_ended {
                        " and its session has exited"
                    } else {
                        ""
                    }
                )),
            ),
            None => (
                "stalled",
                vec!["kanna_rerun_stage", "kanna_close_task"],
                Some(
                    member
                        .create_error
                        .clone()
                        .unwrap_or_else(|| "no stage run was started".to_string()),
                ),
            ),
        }
    } else {
        (
            "creating",
            Vec::new(),
            member
                .create_error
                .clone()
                .or_else(|| Some("the task has not been created yet".to_string())),
        )
    };
    Ok(json!({
        "taskId": member.child_task_id,
        "position": member.position,
        "state": state,
        "outcome": member.outcome,
        "resultId": member.result_id,
        "status": member.result_status,
        "stage": member.result_stage,
        "committedSha": member.result_sha,
        "inputId": member.input_id,
        "resolvedAt": member.resolved_at,
        "notified": member.notified_at.is_some(),
        "latestRun": latest_run.map(|run| json!({
            "id": run.id,
            "status": run.status,
            "finishedAt": run.finished_at,
        })),
        "runtimeStatus": item.and_then(|item| item.runtime_status),
        "actions": actions,
        "detail": detail,
    }))
}

fn join_view(db: &Db, join: &TaskJoin) -> Result<Value, rusqlite::Error> {
    let members = db.list_task_join_members(&join.id)?;
    let waiting = members
        .iter()
        .filter(|member| member.resolved_at.is_none())
        .map(|member| member.child_task_id.clone())
        .collect::<Vec<_>>();
    Ok(json!({
        "joinId": join.id,
        "parentStage": join.parent_stage,
        "parentRunId": join.parent_run_id,
        "baseSha": join.base_sha,
        "baseBranch": join.base_branch,
        "createdAt": join.created_at,
        "completedAt": join.completed_at,
        "complete": waiting.is_empty(),
        "waitingOn": waiting,
        "members": members
            .iter()
            .map(|member| member_view(db, member))
            .collect::<Result<Vec<_>, _>>()?,
    }))
}

pub(super) async fn get_task_joins(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    super::blocking::run_handler_blocking("subtask joins read", move || {
        let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
        let parent_task_id = db
            .resolve_pipeline_item_id(&task_id)
            .map_err(|e| db_write_error("db error", e))?
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::NOT_FOUND,
                    format!("task not found: {task_id}"),
                )
            })?;
        let joins = db
            .list_task_joins(&parent_task_id)
            .map_err(|e| db_write_error("db error", e))?
            .iter()
            .map(|join| join_view(&db, join))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| db_write_error("db error", e))?;
        let waiting_on = db
            .unresolved_join_children(&parent_task_id)
            .map_err(|e| db_write_error("db error", e))?;
        Ok(Json(json!({
            "parentTaskId": parent_task_id,
            "blocked": !waiting_on.is_empty(),
            "waitingOn": waiting_on,
            "joins": joins,
        })))
    })
    .await
}

/// Why a notice was not typed, and whether it may be tried again.
enum NoticeOutcome {
    Typed,
    /// Nothing was written; the claim is given back. `transient` when the
    /// cause is expected to clear by itself (a held lease, a daemon handoff,
    /// an unsent draft), so the sweep tries again shortly; otherwise (no live
    /// session) it waits for the next trigger, and a later session reads the
    /// input from its ledger regardless.
    NotWritten {
        reason: String,
        transient: bool,
    },
    /// The write may have reached the session; the claim stands.
    Uncertain(String),
}

/// Sweeps per trigger while transient failures leave notices owed.
const NOTICE_RETRY_ATTEMPTS: u32 = 6;

fn not_written(reason: impl Into<String>, transient: bool) -> NoticeOutcome {
    NoticeOutcome::NotWritten {
        reason: reason.into(),
        transient,
    }
}

/// Type one delivered outcome into `parent_task_id`'s live session.
async fn type_notice(state: &Arc<AppState>, parent_task_id: &str, message: &str) -> NoticeOutcome {
    let Some(_task_mutation) = state.try_begin_requested_task_mutation(parent_task_id) else {
        return not_written("the parent is changing stage or session", true);
    };
    let mut daemon =
        match crate::daemon_client::DaemonClient::connect(&state.config.daemon_dir).await {
            Ok(daemon) => daemon,
            Err(error) => return not_written(format!("daemon error: {error}"), true),
        };
    let sessions = match daemon.send_command(&DaemonCommand::List).await {
        Ok(DaemonEvent::SessionList { sessions }) => sessions,
        Ok(other) => return not_written(format!("unexpected daemon response: {other:?}"), true),
        Err(error) => return not_written(format!("daemon error: {error}"), true),
    };
    let Some((pid, composer)) = sessions
        .iter()
        .find(|session| {
            session.session_id == parent_task_id
                && session.kind == SessionKind::Pty
                && matches!(&session.state, SessionState::Active)
        })
        .map(|session| (session.pid, session.composer_attestation))
    else {
        return not_written("the parent has no live session", false);
    };
    // Like an engine wake, a notice never types into somebody's unsent
    // draft; it waits for the next sweep.
    if composer == ComposerAttestation::Typed {
        return not_written("the parent's composer holds an unsent draft", true);
    }
    match super::task_input::try_submit_task_input_if_session(
        &mut daemon,
        parent_task_id,
        pid,
        message,
    )
    .await
    {
        Ok(()) => NoticeOutcome::Typed,
        Err(TaskInputError::Uncertain(error)) => NoticeOutcome::Uncertain(error),
        Err(TaskInputError::SessionNotFound) => not_written("the parent's session ended", false),
        Err(TaskInputError::Other(error)) => not_written(error, false),
    }
}

/// Type every delivered outcome whose notice is still owed, each at most
/// once. Detached, logged, never returned to the caller whose action
/// resolved the member.
pub(super) fn spawn_join_notices(state: &Arc<AppState>) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        release_parked_completions(&state).await;
        deliver_join_notices_with_retries(&state).await
    });
}

/// A parent whose completion parked on dependency edges before it created a
/// join was held by that join too; once every child has resolved, readiness
/// decides the parked completion again.
async fn release_parked_completions(state: &Arc<AppState>) {
    let parents = {
        let state = Arc::clone(state);
        tokio::task::spawn_blocking(move || {
            Db::open(&state.config.db_path).and_then(|db| db.parents_released_by_joins())
        })
        .await
    };
    let parents = match parents {
        Ok(Ok(parents)) => parents,
        Ok(Err(error)) => {
            log::error!("cannot list parents released by their joins: {error}");
            return;
        }
        Err(error) => {
            log::error!("join release worker failed: {error}");
            return;
        }
    };
    for parent in parents {
        if let Err(error) =
            super::stage_dependencies::ensure_dependencies_ready(state, &parent).await
        {
            log::error!("parked completion of {parent} could not proceed after its join: {error}");
        }
    }
}

/// One sweep, then — while transient failures left notices owed — a few
/// more, 1 s doubling, before waiting for the next trigger.
async fn deliver_join_notices_with_retries(state: &Arc<AppState>) {
    for attempt in 0..NOTICE_RETRY_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(1 << (attempt - 1))).await;
        }
        if !deliver_join_notices(state).await {
            return;
        }
    }
}

/// Type every owed notice once; `true` when one failed transiently.
async fn deliver_join_notices(state: &Arc<AppState>) -> bool {
    let pending = {
        let state = Arc::clone(state);
        tokio::task::spawn_blocking(move || {
            Db::open(&state.config.db_path).and_then(|db| db.pending_join_notices())
        })
        .await
    };
    let pending = match pending {
        Ok(Ok(pending)) => pending,
        Ok(Err(error)) => {
            log::error!("cannot list pending subtask join notices: {error}");
            return false;
        }
        Err(error) => {
            log::error!("subtask join notice worker failed: {error}");
            return false;
        }
    };
    let mut retry = false;
    for notice in pending {
        let claimed = {
            let state = Arc::clone(state);
            let child = notice.child_task_id.clone();
            let parent = notice.parent_task_id.clone();
            tokio::task::spawn_blocking(move || {
                let db = Db::open(&state.config.db_path)?;
                // The session is told of an input whose ledger file it can
                // already read; a publish failure leaves it to the publisher.
                if let Err(error) =
                    crate::task_store::flush_task(&db, &state.config.db_path, &parent)
                {
                    log::warn!("ledger of {parent} not yet published before a notice: {error}");
                }
                db.claim_join_notice(&child)
            })
            .await
        };
        if !matches!(claimed, Ok(Ok(true))) {
            continue;
        }
        let release = match type_notice(state, &notice.parent_task_id, &notice.message).await {
            NoticeOutcome::Typed => false,
            NoticeOutcome::Uncertain(error) => {
                log::warn!(
                    "subtask result of {} may not have reached {}: {error}; it is in the \
                     parent's inputs and is not typed again",
                    notice.child_task_id,
                    notice.parent_task_id
                );
                false
            }
            NoticeOutcome::NotWritten { reason, transient } => {
                log::info!(
                    "subtask result of {} not typed into {} yet: {reason}",
                    notice.child_task_id,
                    notice.parent_task_id
                );
                retry |= transient;
                true
            }
        };
        if release {
            let state = Arc::clone(state);
            let child = notice.child_task_id.clone();
            let released = tokio::task::spawn_blocking(move || {
                Db::open(&state.config.db_path).and_then(|db| db.release_join_notice(&child))
            })
            .await;
            if !matches!(released, Ok(Ok(()))) {
                log::error!(
                    "could not give back the notice claim of {}",
                    notice.child_task_id
                );
            }
        }
    }
    retry
}

/// Startup: create the members a launch interrupted before reaching them
/// (from their recorded spec and id, so each is created once), then type
/// the notices a restart left owed.
pub(crate) async fn resume_subtask_joins(state: Arc<AppState>) {
    let uncreated = {
        let state = Arc::clone(&state);
        tokio::task::spawn_blocking(move || {
            let db = Db::open(&state.config.db_path)?;
            let mut work = Vec::new();
            for member in db.list_uncreated_join_members()? {
                let Some(join) = db.task_join(&member.join_id)? else {
                    continue;
                };
                let Some(parent) = db.get_pipeline_item(&join.parent_task_id)? else {
                    continue;
                };
                work.push((join, parent.repo_id, member));
            }
            Ok::<_, rusqlite::Error>(work)
        })
        .await
    };
    match uncreated {
        Ok(Ok(work)) => {
            for (join, repo_id, member) in work {
                if let Err(error) = create_member(&state, &join, &repo_id, &member).await {
                    log::error!(
                        "subtask {} of join {} could not be created on resume: {error}",
                        member.child_task_id,
                        join.id
                    );
                }
            }
        }
        Ok(Err(error)) => log::error!("cannot list uncreated subtask join members: {error}"),
        Err(error) => log::error!("subtask join resume worker failed: {error}"),
    }
    deliver_join_notices_with_retries(&state).await;
}
