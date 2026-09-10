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
    /// Whether a launch could still produce this task's agent session.
    ///
    /// A client watching an empty agent view cannot tell "the startup terminal
    /// is still running" from "this launch failed and nothing will start" —
    /// the daemon refuses the attach identically for both — and a missing PTY
    /// alone says neither. The server holds the facts that do: a closed task
    /// has no launch left, and a launch that ended without an agent recorded a
    /// failed run when it did.
    agent_launch_pending: bool,
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
        let item = db.get_pipeline_item(&resolved).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?;
        let latest_main = db
            .list_stage_runs_for_task(&resolved)
            .map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?
            .into_iter()
            .rfind(|run| run.kind == "main");
        let agent_launch_pending = item.is_some_and(|item| item.closed_at.is_none())
            && !latest_main
                .is_some_and(|run| matches!(run.status.as_str(), "failed" | "cancelled"));

        Ok(Json(TaskTerminalsResponse {
            task_id: resolved,
            agent_session_id,
            terminals,
            agent_launch_pending,
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

/// One thing that happened to a task's workspace, in the order it happened.
///
/// The owner's shape is a single read-only log of workspace operations —
/// creation, the startup script, the agent starting and finishing, teardown,
/// then the next stage's startup — rather than a permanent terminal tab per
/// stage. Nothing new is stored for it: a task's terminals and its runs
/// already record all of this, and this is the chronological reading of them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskActivityEntry {
    /// `terminal` for a startup or teardown shell, `agent` for a run.
    kind: &'static str,
    at: String,
    title: String,
    stage: Option<String>,
    attempt: Option<i64>,
    /// Present for a terminal that has finished.
    exit_code: Option<i64>,
    /// The terminal record this entry can be reopened from, when it has one.
    terminal_session_id: Option<String>,
    /// Whether that terminal's output was kept.
    archived: bool,
    /// For an agent run: how it ended, once it has.
    status: Option<String>,
    result: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskActivityResponse {
    task_id: String,
    entries: Vec<TaskActivityEntry>,
}

pub(super) async fn read_task_activity(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskActivityResponse>, (StatusCode, String)> {
    super::blocking::run_handler_blocking("task activity", move || {
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
        let runs = db.list_stage_runs_for_task(&resolved).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?;

        // A retained agent attempt is reopened from here, so its run entry
        // carries the record that holds it. Keyed by the run the attempt
        // served, which is what the transition records when it retains one.
        let retained_agents: std::collections::HashMap<&str, &TaskTerminalSession> = terminals
            .iter()
            .filter(|terminal| terminal.role == crate::db::ROLE_AGENT)
            .filter_map(|terminal| {
                terminal
                    .stage_run_id
                    .as_deref()
                    .map(|run_id| (run_id, terminal))
            })
            .collect();

        let mut entries: Vec<TaskActivityEntry> = Vec::new();
        for terminal in &terminals {
            // An agent attempt is not a log line of its own: the log is the
            // workspace around the agent, and the attempt is reported on the
            // run it served, below.
            if terminal.role != crate::db::ROLE_SETUP && terminal.role != crate::db::ROLE_TEARDOWN {
                continue;
            }
            entries.push(TaskActivityEntry {
                kind: "terminal",
                at: terminal.created_at.clone(),
                title: terminal
                    .title
                    .clone()
                    .unwrap_or_else(|| terminal.role.clone()),
                stage: terminal.stage.clone(),
                attempt: Some(terminal.attempt),
                exit_code: terminal.exit_code,
                terminal_session_id: Some(terminal.id.clone()),
                archived: terminal.archived,
                status: Some(terminal.state.clone()),
                result: None,
            });
        }
        for run in &runs {
            if run.kind != "main" {
                continue;
            }
            let retained = retained_agents.get(run.id.as_str()).copied();
            entries.push(TaskActivityEntry {
                kind: "agent",
                at: run.started_at.clone(),
                title: retained
                    .and_then(|terminal| terminal.title.clone())
                    .unwrap_or_else(|| match run.agent.as_deref() {
                        Some(agent) => format!("{agent} · {}", run.stage),
                        None => format!("Agent · {}", run.stage),
                    }),
                stage: Some(run.stage.clone()),
                attempt: retained.map(|terminal| terminal.attempt),
                exit_code: retained.and_then(|terminal| terminal.exit_code),
                // The handle a reader reopens this attempt's output with.
                terminal_session_id: retained.map(|terminal| terminal.id.clone()),
                archived: retained.is_some_and(|terminal| terminal.archived),
                status: Some(run.status.clone()),
                result: run.result.clone(),
            });
        }
        // One order, by when each thing began: a stage's startup, then its
        // agent, then the next stage's startup.
        entries.sort_by(|left, right| left.at.cmp(&right.at));

        Ok(Json(TaskActivityResponse {
            task_id: resolved,
            entries,
        }))
    })
    .await
}
