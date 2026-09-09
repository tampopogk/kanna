//! The terminals a task owns.
//!
//! A task used to have exactly one, so "the task's session" and "the task's
//! terminal" were the same sentence and every surface derived one from the
//! task id. A launch now opens a startup terminal of its own before the agent
//! runs in its, and a stage advance opens a new pair, so a client that wants
//! to show a task's terminals has to be told which ones exist rather than
//! guess. This is that list.
//!
//! It is deliberately a read of durable records rather than a live daemon
//! query: a finished startup terminal is still the record of what that
//! launch's setup did, and a stage that has moved on is exactly when someone
//! wants to read it.

use super::lan_trust::PrivilegedTaskAccess;
use super::state::AppState;
use crate::db::{Db, TaskTerminalSession, TerminalSessionArchive};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskTerminalsResponse {
    task_id: String,
    /// The daemon session the task's agent runs in — the one every
    /// agent-facing surface still addresses by task id.
    agent_session_id: Option<String>,
    terminals: Vec<TaskTerminalSession>,
}

pub(super) async fn list_task_terminals(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskTerminalsResponse>, (StatusCode, String)> {
    super::blocking::run_handler_blocking("task terminals", move || {
        let db = Db::open(&state.config().db_path).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?;
        let resolved = db
            .resolve_pipeline_item_id(&task_id)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?
            .ok_or_else(|| (StatusCode::NOT_FOUND, format!("task not found: {task_id}")))?;
        let terminals = db.list_task_terminal_sessions(&resolved).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?;
        let agent_session_id = db
            .resolve_task_terminal_session_id(&resolved)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?;
        Ok(Json(TaskTerminalsResponse {
            task_id: resolved,
            agent_session_id,
            terminals,
        }))
    })
    .await
}

/// The final frame of a terminal that has finished.
///
/// A retired terminal is not attachable — its PTY is gone — so a client that
/// opens its tab reads this instead of looping on an attach that can never
/// succeed. It is the headless terminal's own rendering of the last screen,
/// captured before the daemon dropped the session.
pub(super) async fn read_task_terminal_archive(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((task_id, session_id)): Path<(String, String)>,
) -> Result<Json<TerminalSessionArchive>, (StatusCode, String)> {
    super::blocking::run_handler_blocking("task terminal archive", move || {
        let db = Db::open(&state.config().db_path).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?;
        let resolved = db
            .resolve_pipeline_item_id(&task_id)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?
            .ok_or_else(|| (StatusCode::NOT_FOUND, format!("task not found: {task_id}")))?;
        // The archive is addressed through the task that owns the terminal, so
        // a caller cannot read one terminal's frame by naming another task.
        let terminals = db.list_task_terminal_sessions(&resolved).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?;
        // Addressed by the terminal *record*, which is what names one attempt:
        // a task's agent keeps one daemon session id across every stage, so the
        // session id alone cannot say which attempt's frame is wanted. The
        // record id is accepted first, and a daemon session id still resolves
        // for the terminals that have only ever had one — a setup or teardown
        // shell — so existing callers keep working.
        let record_id = terminals
            .iter()
            .find(|terminal| terminal.id == session_id)
            .or_else(|| {
                terminals
                    .iter()
                    .filter(|terminal| {
                        terminal.daemon_session_id.as_deref() == Some(session_id.as_str())
                    })
                    .max_by_key(|terminal| terminal.attempt)
            })
            .map(|terminal| terminal.id.clone())
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    format!("terminal not found for task {resolved}: {session_id}"),
                )
            })?;
        db.read_terminal_session_archive(&record_id)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?
            .map(Json)
            .ok_or_else(|| {
                (
                    StatusCode::NOT_FOUND,
                    format!("no archived frame for terminal {session_id}"),
                )
            })
    })
    .await
}
