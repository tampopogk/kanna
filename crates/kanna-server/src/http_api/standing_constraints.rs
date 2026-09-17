//! Read/write surface for the durable standing-constraints record.
//!
//! Three operations, deliberately: declare one, clear one, and load the active
//! set in a single cheap call. The load is the one a supervising manager makes
//! on every wake and immediately after a compaction, so it answers with the
//! complete active set rather than a page, and reports how much cleared
//! history exists without shipping it unasked.
//!
//! The server stores and returns constraint text; it never interprets it. See
//! [`crate::db::standing_constraints`] for why that boundary is where it is.

use super::state::{db_write_error, AppState};
use super::task_blockers::resolve_existing_task_id;
use crate::db::{
    clamp_cleared_constraint_tail, Db, NewStandingConstraint, StandingConstraint,
    StandingConstraintClear, StandingConstraintKind, StandingConstraintSource,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SetStandingConstraintRequest {
    repo_id: String,
    kind: String,
    text: String,
    #[serde(default)]
    subject_task_id: Option<String>,
    #[serde(default)]
    declared_by: Option<String>,
    #[serde(default)]
    declared_by_task_id: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ClearStandingConstraintRequest {
    #[serde(default)]
    cleared_by: Option<String>,
    #[serde(default)]
    cleared_by_task_id: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StandingConstraintResponse {
    constraint: StandingConstraint,
    /// False when an identical constraint was already standing (set) or the
    /// constraint was already cleared (clear). Either way no second event was
    /// appended, because no second decision was taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    created: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleared: Option<bool>,
    /// Where this constraint's life is announced in the task-event feed, or
    /// null when it names no task and therefore announces nowhere.
    announced_on_task_id: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListStandingConstraintsQuery {
    repo_id: String,
    #[serde(default)]
    include_cleared: Option<bool>,
    #[serde(default)]
    tail: Option<i64>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StandingConstraintsResponse {
    repo_id: String,
    /// Every constraint still standing, oldest first — never a page. A
    /// truncated active set is worse than none: the one dropped is the one
    /// then violated.
    constraints: Vec<StandingConstraint>,
    active_count: usize,
    /// How many cleared constraints exist, whether or not any were returned,
    /// so an absent history reads as "not requested" rather than "none".
    cleared_total: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleared: Option<Vec<StandingConstraint>>,
}

fn parse_kind(value: &str) -> Result<StandingConstraintKind, (StatusCode, String)> {
    StandingConstraintKind::parse(value.trim())
        .map_err(|message| (StatusCode::BAD_REQUEST, message))
}

fn parse_source(value: Option<&String>) -> Result<StandingConstraintSource, (StatusCode, String)> {
    match value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    {
        None => Ok(StandingConstraintSource::Unspecified),
        Some(value) => StandingConstraintSource::parse(value)
            .map_err(|message| (StatusCode::BAD_REQUEST, message)),
    }
}

/// Resolve an optional task reference, refusing one that names no task.
///
/// A constraint whose subject does not exist is a typo, and a typo in a
/// stand-down is the failure this record exists to prevent — it would read as
/// protecting a task while protecting nothing. Cheap to check at write time,
/// impossible to notice later.
fn resolve_optional_task_id(
    db: &Db,
    task_id: Option<&String>,
) -> Result<Option<String>, (StatusCode, String)> {
    let Some(task_id) = task_id
        .map(|task_id| task_id.trim())
        .filter(|task_id| !task_id.is_empty())
    else {
        return Ok(None);
    };
    resolve_existing_task_id(db, task_id).map(Some)
}

pub(super) async fn set_standing_constraint(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SetStandingConstraintRequest>,
) -> Result<Json<StandingConstraintResponse>, (StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
    let repo_id = request.repo_id.trim();
    if repo_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "repoId must not be empty".into()));
    }
    if db
        .get_repo(repo_id)
        .map_err(|e| db_write_error("db error", e))?
        .is_none()
    {
        return Err((
            StatusCode::NOT_FOUND,
            format!("repository not found: {repo_id}"),
        ));
    }
    let kind = parse_kind(&request.kind)?;
    let declared_by = parse_source(request.declared_by.as_ref())?;
    let subject_task_id = resolve_optional_task_id(&db, request.subject_task_id.as_ref())?;
    let declared_by_task_id = resolve_optional_task_id(&db, request.declared_by_task_id.as_ref())?;

    let (constraint, created) = db
        .record_standing_constraint(NewStandingConstraint {
            repo_id,
            kind,
            text: &request.text,
            subject_task_id: subject_task_id.as_deref(),
            declared_by,
            declared_by_task_id: declared_by_task_id.as_deref(),
        })
        .map_err(constraint_write_error)?;
    Ok(Json(StandingConstraintResponse {
        announced_on_task_id: constraint.announcement_task_id().map(str::to_string),
        constraint,
        created: Some(created),
        cleared: None,
    }))
}

pub(super) async fn clear_standing_constraint(
    State(state): State<Arc<AppState>>,
    Path(constraint_id): Path<String>,
    Json(request): Json<ClearStandingConstraintRequest>,
) -> Result<Json<StandingConstraintResponse>, (StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
    let cleared_by = parse_source(request.cleared_by.as_ref())?;
    let cleared_by_task_id = resolve_optional_task_id(&db, request.cleared_by_task_id.as_ref())?;

    let outcome = db
        .clear_standing_constraint(
            constraint_id.trim(),
            StandingConstraintClear {
                cleared_by,
                cleared_by_task_id: cleared_by_task_id.as_deref(),
                note: request.note.as_deref(),
            },
        )
        .map_err(constraint_write_error)?;
    let Some((constraint, cleared)) = outcome else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("standing constraint not found: {constraint_id}"),
        ));
    };
    Ok(Json(StandingConstraintResponse {
        announced_on_task_id: constraint.announcement_task_id().map(str::to_string),
        constraint,
        created: None,
        cleared: Some(cleared),
    }))
}

pub(super) async fn list_standing_constraints(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListStandingConstraintsQuery>,
) -> Result<Json<StandingConstraintsResponse>, (StatusCode, String)> {
    let db = Db::open(&state.config.db_path).map_err(|e| db_write_error("db error", e))?;
    let repo_id = query.repo_id.trim();
    if repo_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "repoId must not be empty".into()));
    }
    let constraints = db
        .list_active_standing_constraints(repo_id)
        .map_err(|e| db_write_error("db error", e))?;
    let cleared_total = db
        .count_cleared_standing_constraints(repo_id)
        .map_err(|e| db_write_error("db error", e))?;
    let cleared = if query.include_cleared.unwrap_or(false) {
        Some(
            db.list_cleared_standing_constraints(
                repo_id,
                clamp_cleared_constraint_tail(query.tail),
            )
            .map_err(|e| db_write_error("db error", e))?,
        )
    } else {
        None
    };
    Ok(Json(StandingConstraintsResponse {
        repo_id: repo_id.to_string(),
        active_count: constraints.len(),
        constraints,
        cleared_total,
        cleared,
    }))
}

/// Normalization refusals reach this layer as `InvalidParameterName`, which is
/// the caller's mistake and not a database fault.
fn constraint_write_error(error: rusqlite::Error) -> (StatusCode, String) {
    match error {
        rusqlite::Error::InvalidParameterName(message) => (StatusCode::BAD_REQUEST, message),
        error => db_write_error("db error", error),
    }
}
