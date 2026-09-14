//! "Show this in the desktop" — one acknowledged action.
//!
//! An agent working inside a task can already read its files, its diff and its
//! commit graph; what it could not do was put one of them in front of the
//! person watching that task. `POST /v1/desktop/views/open` closes that gap:
//! it focuses one Kanna window, selects the named task, opens one whitelisted
//! read-only view of it, aims that view at an optional target, and answers
//! only once the window says the view and its target are on screen.
//!
//! Two properties are the whole point of the lane:
//!
//! * **A queued command is not an opened view.** The command is appended to a
//!   bounded in-memory lane the desktop long-polls; a closed desktop loses it.
//!   So the route does not answer when it appends — it registers the request
//!   id, waits for the window's acknowledgement, and reports
//!   `opened: false, code: "desktop_unavailable"` when none arrives. An agent
//!   telling a reviewer "I opened it for you" must be telling the truth.
//! * **Every target is resolved against the task's own current worktree**,
//!   which the database owns. A caller names a task and a repository-relative
//!   path; it never names a filesystem root, and a path that leaves the
//!   worktree — absolute, traversing, or through a symlink — is refused by the
//!   same descriptor-relative resolution the file and browse routes use,
//!   before anything is queued.
//!
//! The action navigates and nothing else. It writes no task state and no
//! `task_input` row: this is not an instruction to the agent, and the durable
//! instruction history must not read as though it were.

use super::lan_trust::{DesktopLocalAccess, PrivilegedTaskAccess};
use super::state::AppState;
use crate::db::Db;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::oneshot;

const DEFAULT_EVENT_LIMIT: usize = 100;
const MAX_EVENT_LIMIT: usize = 500;
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 25;
const MAX_WAIT_TIMEOUT_SECS: u64 = 120;

/// How long the route waits for a window to say the view is on screen.
///
/// Long enough for a cold view to read a file, a diff or a graph and render
/// it; short enough that a desktop which is not running is reported as absent
/// rather than leaving the caller hanging.
pub(crate) const DEFAULT_OPEN_TIMEOUT_MS: u64 = 10_000;

/// The views this action may open.
///
/// A whitelist rather than "any main tab": `shell` runs commands, `image`
/// takes an arbitrary URL, and `preferences` is not a view of a task. Adding a
/// kind here means adding its target shape, its renderer handler and its
/// readiness contract too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DesktopViewKind {
    Agent,
    File,
    Diff,
    Tree,
    Graph,
    Analytics,
}

impl DesktopViewKind {
    fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "agent" => Self::Agent,
            "file" => Self::File,
            "diff" => Self::Diff,
            "tree" => Self::Tree,
            "graph" => Self::Graph,
            "analytics" => Self::Analytics,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::File => "file",
            Self::Diff => "diff",
            Self::Tree => "tree",
            Self::Graph => "graph",
            Self::Analytics => "analytics",
        }
    }
}

/// An expected failure: something the caller or the desktop can act on, told
/// as `opened: false` with a stable code rather than as an HTTP error, so one
/// field — `opened` — is the whole answer to "is it on their screen?".
#[derive(Debug, Clone)]
pub(super) struct OpenViewFailure {
    code: &'static str,
    message: String,
}

impl OpenViewFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn map_task_file_failure(error: crate::task_files::TaskFileError) -> OpenViewFailure {
    use crate::task_files::TaskFileError;
    let code = match &error {
        TaskFileError::InvalidPath(_) => "invalid_path",
        TaskFileError::TaskNotFound => "task_not_found",
        TaskFileError::WorkspaceUnavailable => "workspace_unavailable",
        TaskFileError::FileNotFound => "file_not_found",
        TaskFileError::TooLarge => "file_too_large",
        TaskFileError::UnsupportedContent => "unsupported_content",
        TaskFileError::RequestTooLarge => "invalid_target",
        TaskFileError::Internal(_) => "internal",
    };
    OpenViewFailure::new(code, error.to_string())
}

fn map_browse_failure(error: crate::repo_browser::BrowseError) -> OpenViewFailure {
    use crate::repo_browser::BrowseError;
    let code = match &error {
        BrowseError::InvalidPath | BrowseError::NotFile => "invalid_path",
        BrowseError::RootNotFound => "workspace_unavailable",
        BrowseError::TargetNotFound => "file_not_found",
        BrowseError::Internal(_) => "internal",
    };
    OpenViewFailure::new(code, error.to_string())
}

fn map_diff_failure(error: crate::task_diff::TaskDiffError) -> OpenViewFailure {
    use crate::task_diff::TaskDiffError;
    let code = match &error {
        TaskDiffError::InvalidRequest(_) => "invalid_target",
        TaskDiffError::TaskNotFound => "task_not_found",
        TaskDiffError::WorkspaceUnavailable => "workspace_unavailable",
        TaskDiffError::Internal(_) => "internal",
    };
    OpenViewFailure::new(code, error.to_string())
}

fn map_graph_failure(error: crate::task_graph::TaskGraphError) -> OpenViewFailure {
    use crate::task_graph::TaskGraphError;
    let code = match &error {
        TaskGraphError::TaskNotFound => "task_not_found",
        TaskGraphError::WorkspaceUnavailable => "workspace_unavailable",
        TaskGraphError::Internal(_) => "internal",
    };
    OpenViewFailure::new(code, error.to_string())
}

// ---------------------------------------------------------------------------
// Target shapes
// ---------------------------------------------------------------------------

/// `file`: a worktree-relative path, optionally aimed at a line or a range.
///
/// Coordinates are 1-based and inclusive, counted in Unicode scalar values —
/// the unit the viewer highlights in, so a caller reading a file and pointing
/// at what it read lands on what it meant.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileTarget {
    path: String,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    column: Option<u32>,
    #[serde(default)]
    end_line: Option<u32>,
    #[serde(default)]
    end_column: Option<u32>,
}

/// `tree`: reveal one file or directory in the explorer, or open it at the
/// worktree root when no path is given.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TreeTarget {
    #[serde(default)]
    path: Option<String>,
}

/// `diff`: a scope, and optionally one anchored line inside it.
///
/// The anchor is a path plus a side plus a line, never a hunk ordinal: hunk
/// numbering shifts with every commit, so an ordinal recorded by a reviewer is
/// pointing somewhere else by the time a human clicks it. The viewer opens the
/// hunk that contains the anchored line. `excerpt` is optional staleness
/// protection: when present, the anchored line must still contain it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DiffTarget {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    excerpt: Option<String>,
}

/// `graph`: select one commit, named by its full object id.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GraphTarget {
    #[serde(default)]
    commit: Option<String>,
}

fn parse_target<T: serde::de::DeserializeOwned>(
    view: DesktopViewKind,
    target: Value,
) -> Result<T, OpenViewFailure> {
    serde_json::from_value(target).map_err(|error| {
        OpenViewFailure::new(
            "invalid_target",
            format!("{} target is invalid: {error}", view.as_str()),
        )
    })
}

// ---------------------------------------------------------------------------
// Request
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OpenDesktopViewRequest {
    task_id: String,
    /// Kept as a string so an unrecognised view is answered with the
    /// whitelist rather than with a deserialization rejection.
    #[serde(default)]
    view: String,
    #[serde(default)]
    target: Option<Value>,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    window_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    pane_id: Option<String>,
    #[serde(default)]
    tab_id: Option<String>,
    #[serde(default)]
    direction: Option<String>,
}

pub(super) async fn open_desktop_view(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Json(request): Json<OpenDesktopViewRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let prepared = {
        let state = Arc::clone(&state);
        super::blocking::run_handler_blocking("desktop view open", move || {
            let db = Db::open(&state.config().db_path).map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("db error: {error}"),
                )
            })?;
            Ok(prepare_open(&db, request))
        })
        .await?
    };

    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(failure) => return Ok(Json(failure_body(failure))),
    };

    // Only now is a window asked, and only now does the caller start waiting:
    // resolving first is what makes a mistyped path an error the agent can act
    // on rather than a window that quietly opens nothing.
    let acknowledgement = state.desktop_view_acks().register();
    let mut command = json!({
        "type": "desktop_view_open",
        "requestId": acknowledgement.request_id(),
        "taskId": prepared.task_id,
        "view": prepared.view.as_str(),
        "branch": prepared.branch,
        "expiresAt": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default().as_millis() as u64 + state.desktop_view_open_timeout_ms(),
    });
    if let Some(target) = prepared.target.clone() {
        command["target"] = target;
    }
    for (key, value) in &prepared.controls {
        command[key] = value.clone();
    }
    state.desktop_view_commands().append(command);

    let timeout = Duration::from_millis(state.desktop_view_open_timeout_ms());
    match acknowledgement.wait(timeout).await {
        Some(ack) if ack.opened => {
            let requires_identity = prepared.controls.contains_key("operation")
                || prepared.controls.contains_key("paneId")
                || prepared.controls.contains_key("windowId");
            if (requires_identity || ack.workspace.is_some())
                && !confirmed_workspace(&prepared, &ack)
            {
                return Ok(Json(failure_body(OpenViewFailure::new(
                    "renderer_failed",
                    "the desktop did not confirm the requested workspace destination",
                ))));
            }
            // The DB owns stage/workspace identity, even while a renderer was loading.
            let db_path = state.config().db_path.clone();
            let task_id = prepared.task_id.clone();
            let branch = super::blocking::run_handler_blocking(
                "desktop workspace acknowledgement",
                move || {
                    let db = Db::open(&db_path)
                        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
                    Ok(current_branch(&db, &task_id))
                },
            )
            .await?;
            if branch.ok().as_deref() != Some(&prepared.branch) {
                return Ok(Json(failure_body(OpenViewFailure::new(
                    "stale_workspace",
                    "the task changed workspace while the desktop was responding",
                ))));
            }
            let mut body = json!({ "opened": true });
            if !prepared.controls.contains_key("operation") {
                body["view"] = json!(prepared.view.as_str());
            }
            if let Some(target) = prepared.target {
                body["target"] = target;
            }
            if let Some(workspace) = ack.workspace {
                body["workspace"] = workspace;
            }
            if let Some(pane_id) = ack.pane_id {
                body["paneId"] = json!(pane_id);
            }
            if let Some(tab_id) = ack.tab_id {
                body["tabId"] = json!(tab_id);
            }
            if let Some(operation) = prepared.controls.get("operation") {
                body["operation"] = operation.clone();
            }
            Ok(Json(body))
        }
        Some(ack) => Ok(Json(failure_body(OpenViewFailure::new(
            renderer_failure_code(ack.code.as_deref()),
            ack.message
                .unwrap_or_else(|| "the desktop could not show the requested view".to_string()),
        )))),
        None => Ok(Json(failure_body(OpenViewFailure::new(
            "desktop_unavailable",
            "no Kanna window acknowledged the request; the desktop may be closed, \
             on another machine, or busy",
        )))),
    }
}

/// A window's own verdict is its own vocabulary, but an unknown code must not
/// leak through as though the server had classified it.
fn renderer_failure_code(code: Option<&str>) -> &'static str {
    match code {
        Some("task_not_found") => "task_not_found",
        Some("workspace_unavailable") => "workspace_unavailable",
        Some("stale_workspace") => "stale_workspace",
        Some("window_not_found") => "window_not_found",
        Some("pane_not_found") => "pane_not_found",
        Some("tab_not_found") => "tab_not_found",
        Some("request_expired") => "request_expired",
        Some("file_not_found") => "file_not_found",
        Some("invalid_target") => "invalid_target",
        Some("diff_target_not_found") => "diff_target_not_found",
        Some("commit_not_found") => "commit_not_found",
        _ => "renderer_failed",
    }
}

fn failure_body(failure: OpenViewFailure) -> Value {
    json!({
        "opened": false,
        "code": failure.code,
        "message": failure.message,
    })
}

struct PreparedOpen {
    task_id: String,
    branch: String,
    controls: serde_json::Map<String, Value>,
    view: DesktopViewKind,
    target: Option<Value>,
}

fn prepare_open(db: &Db, request: OpenDesktopViewRequest) -> Result<PreparedOpen, OpenViewFailure> {
    let mut controls = serde_json::Map::new();
    if let Some(operation) = request.operation.as_deref() {
        if !["inspect", "split", "move"].contains(&operation)
            || !request.view.is_empty()
            || request.target.is_some()
        {
            return Err(OpenViewFailure::new(
                "invalid_target",
                "workspace operation must be inspect, split or move and takes no view/target",
            ));
        }
        controls.insert("operation".into(), json!(operation));
    }
    let operation = request.operation.as_deref();
    if request.pane_id.is_some() || matches!(operation, Some("split" | "move")) {
        if request.pane_id.is_none()
            || request.window_id.is_none()
            || request.workspace_id.is_none()
        {
            return Err(OpenViewFailure::new(
                "invalid_target",
                "pane operations require paneId, windowId and workspaceId from inspect",
            ));
        }
    } else if request.workspace_id.is_some() {
        return Err(OpenViewFailure::new(
            "invalid_target",
            "workspaceId requires paneId",
        ));
    }
    if (operation == Some("split")) != request.direction.is_some()
        || request
            .direction
            .as_deref()
            .is_some_and(|direction| !["horizontal", "vertical"].contains(&direction))
        || (operation == Some("move") && request.tab_id.is_none())
        || (!matches!(operation, Some("split" | "move")) && request.tab_id.is_some())
        || (operation == Some("inspect") && request.pane_id.is_some())
    {
        return Err(OpenViewFailure::new("invalid_target", "split needs direction; move needs tabId; inspect takes only taskId and optional windowId"));
    }
    for (key, value) in [
        ("windowId", request.window_id),
        ("workspaceId", request.workspace_id),
        ("paneId", request.pane_id),
        ("tabId", request.tab_id),
        ("direction", request.direction),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty() {
                return Err(OpenViewFailure::new(
                    "invalid_target",
                    format!("{key} must not be empty"),
                ));
            }
            controls.insert(key.into(), json!(value));
        }
    }
    let Some(view) = DesktopViewKind::parse(if operation.is_some() {
        "agent"
    } else {
        request.view.trim()
    }) else {
        return Err(OpenViewFailure::new(
            "unsupported_view",
            format!(
                "unknown view {:?}; open_view shows agent, file, diff, tree, graph or analytics",
                request.view
            ),
        ));
    };

    let task_id = resolve_task(db, &request.task_id)?;
    let branch = current_branch(db, &task_id)?;
    let target = resolve_target(db, &task_id, view, request.target)?;
    Ok(PreparedOpen {
        task_id,
        branch,
        controls,
        view,
        target,
    })
}

fn current_branch(db: &Db, task_id: &str) -> Result<String, OpenViewFailure> {
    let task = db
        .get_pipeline_item(task_id)
        .map_err(|error| OpenViewFailure::new("internal", error.to_string()))?
        .ok_or_else(|| OpenViewFailure::new("task_not_found", "the task no longer exists"))?;
    let root = db
        .get_task_worktree_path(task_id)
        .map_err(|error| OpenViewFailure::new("internal", error.to_string()))?;
    if task.closed_at.is_some() || !root.is_some_and(|root| std::path::Path::new(&root).is_dir()) {
        return Err(OpenViewFailure::new(
            "workspace_unavailable",
            "the task has no current workspace",
        ));
    }
    task.branch.ok_or_else(|| {
        OpenViewFailure::new("workspace_unavailable", "the task has no current branch")
    })
}

fn confirmed_workspace(prepared: &PreparedOpen, ack: &DesktopViewAck) -> bool {
    let Some(workspace) = &ack.workspace else {
        return false;
    };
    if workspace["taskId"] != prepared.task_id
        || workspace["branch"] != prepared.branch
        || !workspace["windowId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        || !workspace["workspaceId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        || !workspace["panes"]
            .as_array()
            .is_some_and(|panes| !panes.is_empty())
    {
        return false;
    }
    for key in ["windowId", "workspaceId"] {
        if prepared
            .controls
            .get(key)
            .is_some_and(|value| workspace[key] != *value)
        {
            return false;
        }
    }
    let operation = prepared.controls.get("operation").and_then(Value::as_str);
    if operation == Some("inspect") {
        return true;
    }
    let Some(pane_id) = ack.pane_id.as_deref() else {
        return false;
    };
    if operation != Some("split")
        && prepared
            .controls
            .get("paneId")
            .is_some_and(|value| value != pane_id)
    {
        return false;
    }
    let Some(pane) = workspace["panes"]
        .as_array()
        .and_then(|panes| panes.iter().find(|pane| pane["id"] == pane_id))
    else {
        return false;
    };
    if operation == Some("split") {
        if prepared
            .controls
            .get("paneId")
            .is_some_and(|source| source == pane_id)
        {
            return false;
        }
        if ack.tab_id.is_none() {
            return !prepared.controls.contains_key("tabId") && pane["activeTabId"] == "";
        }
    }
    let Some(tab_id) = ack.tab_id.as_deref() else {
        return false;
    };
    if prepared
        .controls
        .get("tabId")
        .is_some_and(|value| value != tab_id)
    {
        return false;
    }
    let Some(tab) = pane["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["id"] == tab_id))
    else {
        return false;
    };
    if operation.is_none()
        && (tab["kind"] != prepared.view.as_str()
            || (prepared.view == DesktopViewKind::File
                && prepared
                    .target
                    .as_ref()
                    .is_none_or(|target| tab["filePath"] != target["path"])))
    {
        return false;
    }
    workspace["displayedPanes"].as_array().is_some_and(|panes| {
        panes
            .iter()
            .any(|pane| pane["id"] == pane_id && pane["activeTabId"] == tab_id)
    }) && pane["activeTabId"] == tab_id
}

/// Resolve a task id or its *current* branch name.
///
/// A task's work moves between workspaces, so an older branch name is not a
/// second name for the task — it names a workspace the task has left. Saying
/// so is worth a code of its own: "not found" would send a caller looking for
/// a task that is right there under a newer branch.
fn resolve_task(db: &Db, task_or_branch_id: &str) -> Result<String, OpenViewFailure> {
    let requested = task_or_branch_id.trim();
    if requested.is_empty() {
        return Err(OpenViewFailure::new(
            "invalid_target",
            "task_id must not be empty",
        ));
    }
    if let Some(task_id) = db
        .resolve_pipeline_item_id(requested)
        .map_err(|error| OpenViewFailure::new("internal", format!("db error: {error}")))?
    {
        return Ok(task_id);
    }
    if let Some(task_id) = db
        .resolve_task_by_workspace_branch(requested)
        .map_err(|error| OpenViewFailure::new("internal", format!("db error: {error}")))?
    {
        return Err(OpenViewFailure::new(
            "stale_branch_alias",
            format!(
                "branch {requested} is a workspace task {task_id} has left; pass the task id, \
                 which is stable across stages"
            ),
        ));
    }
    Err(OpenViewFailure::new(
        "task_not_found",
        format!("no task matches {requested}"),
    ))
}

fn resolve_target(
    db: &Db,
    task_id: &str,
    view: DesktopViewKind,
    target: Option<Value>,
) -> Result<Option<Value>, OpenViewFailure> {
    let target = match target {
        Some(Value::Null) | None => None,
        Some(value) if !value.is_object() => {
            return Err(OpenViewFailure::new(
                "invalid_target",
                "target must be an object",
            ))
        }
        Some(value) => Some(value),
    };

    match (view, target) {
        (DesktopViewKind::Agent | DesktopViewKind::Analytics, Some(_)) => {
            Err(OpenViewFailure::new(
                "unsupported_target",
                format!("the {} view takes no target", view.as_str()),
            ))
        }
        (DesktopViewKind::Agent | DesktopViewKind::Analytics, None) => Ok(None),
        (DesktopViewKind::File, None) => Err(OpenViewFailure::new(
            "invalid_target",
            "the file view needs a target naming the path to open",
        )),
        (DesktopViewKind::File, Some(value)) => {
            resolve_file_target(db, task_id, parse_target(view, value)?).map(Some)
        }
        (DesktopViewKind::Tree, value) => {
            let target = match value {
                Some(value) => parse_target::<TreeTarget>(view, value)?,
                None => TreeTarget { path: None },
            };
            resolve_tree_target(db, task_id, target)
        }
        (DesktopViewKind::Diff, value) => {
            let target = match value {
                Some(value) => parse_target::<DiffTarget>(view, value)?,
                None => DiffTarget {
                    scope: None,
                    path: None,
                    side: None,
                    line: None,
                    excerpt: None,
                },
            };
            resolve_diff_target(db, task_id, target).map(Some)
        }
        (DesktopViewKind::Graph, value) => {
            let target = match value {
                Some(value) => parse_target::<GraphTarget>(view, value)?,
                None => GraphTarget { commit: None },
            };
            resolve_graph_target(db, task_id, target)
        }
    }
}

fn resolve_file_target(
    db: &Db,
    task_id: &str,
    target: FileTarget,
) -> Result<Value, OpenViewFailure> {
    // The same resolution `/v1/tasks/{id}/files/content` performs, so an
    // absolute path, a traversal, a symlinked escape, a missing file, an
    // oversized one or one the viewer cannot render is refused here. The
    // content it reads on the way is what the range is checked against; the
    // desktop reads the file itself.
    let file = crate::task_files::read_task_file(db, task_id, &target.path)
        .map_err(map_task_file_failure)?;

    let mut resolved = json!({ "path": file.path });
    let Some(line) = positive("line", target.line)? else {
        if target.column.is_some() || target.end_line.is_some() || target.end_column.is_some() {
            return Err(OpenViewFailure::new(
                "invalid_target",
                "a column or end of range needs a line to start from",
            ));
        }
        return Ok(resolved);
    };

    let lines: Vec<&str> = file.content.split('\n').collect();
    let total = lines.len() as u32;
    if line > total {
        return Err(OpenViewFailure::new(
            "invalid_range",
            format!(
                "{} has {total} lines; line {line} is past its end",
                file.path
            ),
        ));
    }
    let end_line = positive("endLine", target.end_line)?.unwrap_or(line);
    if end_line < line {
        return Err(OpenViewFailure::new(
            "invalid_range",
            "endLine must not be before line",
        ));
    }
    if end_line > total {
        return Err(OpenViewFailure::new(
            "invalid_range",
            format!(
                "{} has {total} lines; endLine {end_line} is past its end",
                file.path
            ),
        ));
    }
    let column = positive("column", target.column)?;
    let end_column = positive("endColumn", target.end_column)?;
    check_column(&file.path, &lines, line, "column", column)?;
    check_column(&file.path, &lines, end_line, "endColumn", end_column)?;
    if line == end_line {
        if let (Some(column), Some(end_column)) = (column, end_column) {
            if end_column < column {
                return Err(OpenViewFailure::new(
                    "invalid_range",
                    "endColumn must not be before column on the same line",
                ));
            }
        }
    }

    resolved["line"] = json!(line);
    if let Some(column) = column {
        resolved["column"] = json!(column);
    }
    if end_line != line || end_column.is_some() {
        resolved["endLine"] = json!(end_line);
    }
    if let Some(end_column) = end_column {
        resolved["endColumn"] = json!(end_column);
    }
    Ok(resolved)
}

/// A column may sit one past the last character — that is the end of the line,
/// which is what an exclusive-feeling caller most often means by "to here".
fn check_column(
    path: &str,
    lines: &[&str],
    line: u32,
    field: &str,
    column: Option<u32>,
) -> Result<(), OpenViewFailure> {
    let Some(column) = column else { return Ok(()) };
    let text = lines[(line - 1) as usize].trim_end_matches('\r');
    let width = text.chars().count() as u32;
    if column > width.saturating_add(1) {
        return Err(OpenViewFailure::new(
            "invalid_range",
            format!("{path} line {line} is {width} characters; {field} {column} is past its end"),
        ));
    }
    Ok(())
}

fn positive(field: &str, value: Option<u32>) -> Result<Option<u32>, OpenViewFailure> {
    match value {
        Some(0) => Err(OpenViewFailure::new(
            "invalid_range",
            format!("{field} is 1-based, so 0 is not a position"),
        )),
        other => Ok(other),
    }
}

fn resolve_tree_target(
    db: &Db,
    task_id: &str,
    target: TreeTarget,
) -> Result<Option<Value>, OpenViewFailure> {
    let Some(path) = target.path else {
        return Ok(None);
    };
    let root = crate::repo_browser::task_root(db, task_id).map_err(map_browse_failure)?;
    // A tree target may be either kind of thing to reveal, so ask the browser
    // for it as a directory first and as a file second. Both resolutions are
    // descriptor-relative to the worktree root, so an escape fails both.
    match crate::repo_browser::list_directory(&root, &path, true, 0, 1, None) {
        Ok(listing) => Ok(Some(json!({ "path": listing.path, "kind": "directory" }))),
        Err(crate::repo_browser::BrowseError::Internal(message)) => {
            Err(OpenViewFailure::new("internal", message))
        }
        Err(directory_error) => {
            match crate::repo_browser::read_file_range(&root, &path, 0, 0, 1, true) {
                Ok(file) => Ok(Some(json!({ "path": file.path, "kind": "file" }))),
                // The directory attempt is the one that saw the path as a
                // path; report a traversal as a traversal rather than as the
                // file reader's second opinion.
                Err(crate::repo_browser::BrowseError::TargetNotFound) => {
                    Err(map_browse_failure(directory_error))
                }
                Err(file_error) => Err(map_browse_failure(file_error)),
            }
        }
    }
}

fn resolve_diff_target(
    db: &Db,
    task_id: &str,
    target: DiffTarget,
) -> Result<Value, OpenViewFailure> {
    let scope = match target.scope.as_deref() {
        None => "branch",
        Some("branch") => "branch",
        Some("working") => "working",
        Some(other) => {
            return Err(OpenViewFailure::new(
                "invalid_target",
                format!("unknown diff scope {other:?}; it is branch or working"),
            ))
        }
    };

    let anchored = target.path.is_some() || target.side.is_some() || target.line.is_some();
    if !anchored {
        if target.excerpt.is_some() {
            return Err(OpenViewFailure::new(
                "invalid_target",
                "an excerpt only guards an anchored line, so it needs path, side and line",
            ));
        }
        return Ok(json!({ "scope": scope }));
    }
    let (Some(path), Some(side), Some(line)) = (
        target.path.as_deref(),
        target.side.as_deref(),
        positive("line", target.line)?,
    ) else {
        return Err(OpenViewFailure::new(
            "invalid_target",
            "a diff line target needs path, side and line together",
        ));
    };
    if side != "old" && side != "new" {
        return Err(OpenViewFailure::new(
            "invalid_target",
            format!("unknown diff side {side:?}; it is old or new"),
        ));
    }

    let request =
        crate::task_diff::TaskDiffRequest::parse(Some(scope), None).map_err(map_diff_failure)?;
    let diff = crate::task_diff::read_task_diff(db, task_id, request).map_err(map_diff_failure)?;
    let anchor = locate_diff_line(&diff.patch, path, side, line)?;
    if let Some(excerpt) = target.excerpt.as_deref() {
        let excerpt = excerpt.trim();
        if !excerpt.is_empty() && !anchor.text.contains(excerpt) {
            return Err(OpenViewFailure::new(
                "diff_target_stale",
                format!(
                    "{path} {side} line {line} reads {:?}, which no longer contains the excerpt",
                    anchor.text
                ),
            ));
        }
    }

    // The anchor carries both sides' numbering and what kind of line it is,
    // because the rendered diff numbers each row by its own side: without
    // that, a context line's old and new numbers are indistinguishable in the
    // DOM and the view would scroll to whichever it found first.
    let mut resolved = json!({
        "scope": scope,
        "path": path,
        "side": side,
        "line": line,
        "anchorKind": anchor.kind,
    });
    if let Some(old_line) = anchor.old_line {
        resolved["oldLine"] = json!(old_line);
    }
    if let Some(new_line) = anchor.new_line {
        resolved["newLine"] = json!(new_line);
    }
    if let Some(excerpt) = target.excerpt {
        resolved["excerpt"] = json!(excerpt);
    }
    Ok(resolved)
}

#[derive(Debug)]
struct DiffAnchor {
    text: String,
    /// Which side(s) of the diff number this line. A deletion has no new-side
    /// number and an addition has no old-side one; a context line has both.
    kind: &'static str,
    old_line: Option<u32>,
    new_line: Option<u32>,
}

/// Find one line of one side of one file in a unified patch.
///
/// Walking the patch is what turns "line 40 of the new side" into a claim the
/// server has checked: the diff the window will render is the diff this read,
/// so an anchor that is not in it is refused here instead of arriving as a
/// window that scrolled nowhere. A path that appears on the requested side
/// more than once is ambiguous rather than resolved to the first one.
fn locate_diff_line(
    patch: &str,
    path: &str,
    side: &str,
    line: u32,
) -> Result<DiffAnchor, OpenViewFailure> {
    let wanted_old = side == "old";
    let mut matching_sections = 0usize;
    let mut found: Option<DiffAnchor> = None;
    let mut in_target_file = false;
    let mut old_line = 0u32;
    let mut new_line = 0u32;
    // How many lines of the current hunk's body are still owed on each side.
    // While either is outstanding the line is content, whatever it starts
    // with — which is what keeps a deleted `-- title` (spelled `--- title`)
    // from being read as the next file's header.
    let mut old_remaining = 0u32;
    let mut new_remaining = 0u32;

    for raw in patch.lines() {
        let in_hunk_body = old_remaining > 0 || new_remaining > 0;

        if !in_hunk_body {
            if raw.starts_with("diff --git ") {
                in_target_file = false;
                continue;
            }
            if let Some(header_path) = raw.strip_prefix("--- ") {
                if wanted_old {
                    in_target_file = patch_path_matches(header_path, path);
                    if in_target_file {
                        matching_sections += 1;
                    }
                }
                continue;
            }
            if let Some(header_path) = raw.strip_prefix("+++ ") {
                if !wanted_old {
                    in_target_file = patch_path_matches(header_path, path);
                    if in_target_file {
                        matching_sections += 1;
                    }
                }
                continue;
            }
            if raw.starts_with("@@ ") {
                if let Some(header) = parse_hunk_header(raw) {
                    old_line = header.old_start;
                    new_line = header.new_start;
                    old_remaining = header.old_count;
                    new_remaining = header.new_count;
                }
                continue;
            }
            // Anything else outside a hunk — `index`, `similarity`, `Binary
            // files differ`, a commit message in `git log -p` — numbers
            // nothing.
            continue;
        }

        // Inside a hunk body: the first character classifies the line, and a
        // side that has run out of promised lines stops consuming.
        let marker = raw.chars().next().unwrap_or(' ');
        let body = if raw.is_empty() { "" } else { &raw[1..] };
        match marker {
            '-' if old_remaining > 0 => {
                if wanted_old && in_target_file && old_line == line {
                    found.get_or_insert(DiffAnchor {
                        text: body.to_string(),
                        kind: "deletion",
                        old_line: Some(old_line),
                        new_line: None,
                    });
                }
                old_line += 1;
                old_remaining -= 1;
            }
            '+' if new_remaining > 0 => {
                if !wanted_old && in_target_file && new_line == line {
                    found.get_or_insert(DiffAnchor {
                        text: body.to_string(),
                        kind: "addition",
                        old_line: None,
                        new_line: Some(new_line),
                    });
                }
                new_line += 1;
                new_remaining -= 1;
            }
            // A context line, including the bare empty line some generators
            // emit for an unchanged blank line.
            ' ' if old_remaining > 0 && new_remaining > 0 => {
                if in_target_file
                    && ((wanted_old && old_line == line) || (!wanted_old && new_line == line))
                {
                    found.get_or_insert(DiffAnchor {
                        text: body.to_string(),
                        kind: "context",
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                    });
                }
                old_line += 1;
                new_line += 1;
                old_remaining -= 1;
                new_remaining -= 1;
            }
            // `\ No newline at end of file` belongs to neither side's count,
            // and anything else here is a malformed body line we do not
            // number rather than guess at.
            _ => {}
        }
    }

    if matching_sections > 1 {
        return Err(OpenViewFailure::new(
            "diff_target_ambiguous",
            format!("{path} appears more than once on the {side} side of this diff"),
        ));
    }
    found.ok_or_else(|| {
        OpenViewFailure::new(
            "diff_target_not_found",
            if matching_sections == 0 {
                format!("{path} is not part of this diff on the {side} side")
            } else {
                format!("{side} line {line} of {path} is not inside any hunk of this diff")
            },
        )
    })
}

/// `--- a/src/main.rs` / `+++ b/src/main.rs`, with the tab-separated timestamp
/// some generators append, and `/dev/null` for an added or deleted file.
fn patch_path_matches(header: &str, path: &str) -> bool {
    let header = header.split('\t').next().unwrap_or(header).trim();
    if header == "/dev/null" {
        return false;
    }
    let stripped = header
        .strip_prefix("a/")
        .or_else(|| header.strip_prefix("b/"))
        .unwrap_or(header);
    stripped == path
}

/// One hunk's starting line and line count on each side.
///
/// The counts are what tell a hunk's *body* from the next file's headers. A
/// deletion of `-- old title` is spelled `--- old title`, and an addition of
/// `++ new title` is spelled `+++ new title`: read as headers, those end the
/// hunk and rename the file being read. Counting the lines the header
/// promised is the only way to know that they are content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HunkHeader {
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
}

fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let inner = line.strip_prefix("@@ ")?;
    let inner = inner.split(" @@").next()?;
    let mut parts = inner.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    // `@@ -1 +1 @@` omits a count of one.
    let side = |raw: &str| -> Option<(u32, u32)> {
        let mut fields = raw.split(',');
        let start = fields.next()?.parse::<u32>().ok()?;
        let count = match fields.next() {
            Some(value) => value.parse::<u32>().ok()?,
            None => 1,
        };
        Some((start, count))
    };
    let (old_start, old_count) = side(old)?;
    let (new_start, new_count) = side(new)?;
    Some(HunkHeader {
        old_start,
        old_count,
        new_start,
        new_count,
    })
}

fn resolve_graph_target(
    db: &Db,
    task_id: &str,
    target: GraphTarget,
) -> Result<Option<Value>, OpenViewFailure> {
    let Some(commit) = target.commit else {
        return Ok(None);
    };
    let commit = commit.trim().to_lowercase();
    if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(OpenViewFailure::new(
            "invalid_target",
            "commit must be a full 40-character object id; abbreviations are ambiguous",
        ));
    }
    let graph = crate::task_graph::read_task_graph(db, task_id, None).map_err(map_graph_failure)?;
    if !graph
        .commits
        .iter()
        .any(|entry| entry.hash.eq_ignore_ascii_case(&commit))
    {
        return Err(OpenViewFailure::new(
            "commit_not_found",
            format!("commit {commit} is not in this task's graph"),
        ));
    }
    Ok(Some(json!({ "commit": commit })))
}

// ---------------------------------------------------------------------------
// Acknowledgement
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(super) struct DesktopViewAck {
    opened: bool,
    code: Option<String>,
    message: Option<String>,
    workspace: Option<Value>,
    pane_id: Option<String>,
    tab_id: Option<String>,
}

/// The in-flight opens, keyed by request id.
///
/// In memory and per process on purpose: a request only means anything while
/// the caller is still waiting for it, and the server that issued the id is
/// the only one that can answer it.
#[derive(Default)]
pub(crate) struct DesktopViewAcks {
    pending: StdMutex<HashMap<String, oneshot::Sender<DesktopViewAck>>>,
}

pub(super) struct PendingDesktopViewOpen {
    request_id: String,
    receiver: oneshot::Receiver<DesktopViewAck>,
    acks: Arc<DesktopViewAcks>,
}

impl PendingDesktopViewOpen {
    fn request_id(&self) -> &str {
        &self.request_id
    }

    async fn wait(self, timeout: Duration) -> Option<DesktopViewAck> {
        let PendingDesktopViewOpen {
            request_id,
            receiver,
            acks,
        } = self;
        let result = tokio::time::timeout(timeout, receiver).await;
        // A timed-out or dropped request must not sit in the map: the window
        // may answer late, and the entry would otherwise be leaked.
        acks.forget(&request_id);
        match result {
            Ok(Ok(ack)) => Some(ack),
            _ => None,
        }
    }
}

impl DesktopViewAcks {
    pub(super) fn register(self: &Arc<Self>) -> PendingDesktopViewOpen {
        let request_id = format!(
            "view-{}",
            crate::transfer_engine::queue::unique_work_nonce()
        );
        let (sender, receiver) = oneshot::channel();
        self.lock().insert(request_id.clone(), sender);
        PendingDesktopViewOpen {
            request_id,
            receiver,
            acks: Arc::clone(self),
        }
    }

    /// Answer one in-flight open. False when nothing was waiting — the caller
    /// gave up, or this is a second acknowledgement of the same request.
    fn resolve(&self, request_id: &str, ack: DesktopViewAck) -> bool {
        let Some(sender) = self.lock().remove(request_id) else {
            return false;
        };
        sender.send(ack).is_ok()
    }

    fn forget(&self, request_id: &str) {
        self.lock().remove(request_id);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<DesktopViewAck>>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DesktopViewAckRequest {
    request_id: String,
    opened: bool,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    workspace: Option<Value>,
    #[serde(default)]
    pane_id: Option<String>,
    #[serde(default)]
    tab_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DesktopViewAckResponse {
    /// False when no caller was still waiting: the open timed out, or this
    /// request was already answered.
    acknowledged: bool,
}

/// The window's answer. Loopback-only, like the command lane it answers: only
/// this machine's desktop can say what is on this machine's screen.
pub(super) async fn acknowledge_desktop_view(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Json(request): Json<DesktopViewAckRequest>,
) -> Json<DesktopViewAckResponse> {
    let acknowledged = state.desktop_view_acks().resolve(
        &request.request_id,
        DesktopViewAck {
            opened: request.opened,
            code: request.code,
            message: request.message,
            workspace: request.workspace,
            pane_id: request.pane_id,
            tab_id: request.tab_id,
        },
    );
    Json(DesktopViewAckResponse { acknowledged })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DesktopViewCommandsQuery {
    cursor: Option<u64>,
    stream_id: Option<String>,
    limit: Option<usize>,
    timeout_secs: Option<u64>,
}

/// Long-poll the desktop view command lane. Same cursor/streamId contract as
/// the transfer advisory lanes: a cursor is only meaningful inside the server
/// incarnation that issued it, and reading through one prunes it.
pub(super) async fn wait_desktop_view_commands(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Query(query): Query<DesktopViewCommandsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let limit = query
        .limit
        .unwrap_or(DEFAULT_EVENT_LIMIT)
        .clamp(1, MAX_EVENT_LIMIT);
    let timeout_secs = query
        .timeout_secs
        .unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS)
        .clamp(1, MAX_WAIT_TIMEOUT_SECS);
    let batch = state
        .desktop_view_commands()
        .wait_for_events(
            query.cursor,
            query.stream_id.as_deref(),
            limit,
            Duration::from_secs(timeout_secs),
        )
        .await;
    Ok(Json(json!({
        "waitOutcome": if batch.events.is_empty() { "timeout" } else { "events" },
        "cursor": batch.cursor,
        "streamId": batch.stream_id,
        "events": batch.events,
        "hasMore": batch.has_more,
        "missedEvents": batch.missed_events,
    })))
}

#[cfg(test)]
mod tests {
    use super::{locate_diff_line, parse_hunk_header, patch_path_matches};

    // Written as joined lines rather than with `\`-continuations: a
    // continuation eats the next line's leading whitespace, which is exactly
    // the character that marks a context line in a patch.
    const PATCH: &str = concat!(
        "diff --git a/src/main.rs b/src/main.rs\n",
        "--- a/src/main.rs\n",
        "+++ b/src/main.rs\n",
        "@@ -10,4 +10,5 @@ fn main() {\n",
        " let kept = 1;\n",
        "-let removed = 2;\n",
        "+let added = 3;\n",
        "+let also_added = 4;\n",
        " let tail = 5;\n",
    );

    #[test]
    fn hunk_headers_give_both_starting_lines() {
        let header = parse_hunk_header("@@ -10,4 +12,5 @@ ctx").unwrap();
        assert_eq!((header.old_start, header.new_start), (10, 12));
        assert_eq!((header.old_count, header.new_count), (4, 5));
        assert_eq!(parse_hunk_header("@@ nonsense"), None);
    }

    #[test]
    fn patch_paths_ignore_the_a_and_b_prefixes_and_dev_null() {
        assert!(patch_path_matches("a/src/main.rs", "src/main.rs"));
        assert!(patch_path_matches(
            "b/src/main.rs\t2026-01-01",
            "src/main.rs"
        ));
        assert!(!patch_path_matches("/dev/null", "src/main.rs"));
        assert!(!patch_path_matches("a/src/other.rs", "src/main.rs"));
    }

    #[test]
    fn each_side_counts_only_the_lines_that_side_has() {
        // New side: 10 kept, 11 added, 12 also_added, 13 tail.
        assert_eq!(
            locate_diff_line(PATCH, "src/main.rs", "new", 12)
                .unwrap()
                .text,
            "let also_added = 4;"
        );
        let context = locate_diff_line(PATCH, "src/main.rs", "new", 10).unwrap();
        assert_eq!(context.kind, "context");
        assert_eq!((context.old_line, context.new_line), (Some(10), Some(10)));
        // Old side: 10 kept, 11 removed, 12 tail.
        assert_eq!(
            locate_diff_line(PATCH, "src/main.rs", "old", 11)
                .unwrap()
                .text,
            "let removed = 2;"
        );
    }

    /// A change from `-- old title` to `++ new title`. Git spells those body
    /// lines `--- old title` and `+++ new title`, which are shaped exactly
    /// like file headers.
    const HEADER_SHAPED_PATCH: &str = concat!(
        "diff --git a/doc.md b/doc.md\n",
        "--- a/doc.md\n",
        "+++ b/doc.md\n",
        "@@ -1,3 +1,3 @@\n",
        "-- old title\n",
        "++ new title\n",
        " body stays\n",
    );

    #[test]
    fn header_shaped_body_lines_are_content_not_file_headers() {
        // The deletion, on the old side.
        let removed = locate_diff_line(HEADER_SHAPED_PATCH, "doc.md", "old", 1).unwrap();
        assert_eq!(removed.kind, "deletion");
        assert_eq!(removed.text, "- old title");

        // The addition, on the new side.
        let added = locate_diff_line(HEADER_SHAPED_PATCH, "doc.md", "new", 1).unwrap();
        assert_eq!(added.kind, "addition");
        assert_eq!(added.text, "+ new title");

        // And the context line after them, which the old reader lost along
        // with the file identity those two lines reset.
        let context = locate_diff_line(HEADER_SHAPED_PATCH, "doc.md", "new", 2).unwrap();
        assert_eq!(context.kind, "context");
        assert_eq!(context.text, "body stays");
        assert_eq!((context.old_line, context.new_line), (Some(2), Some(2)));
    }

    #[test]
    fn a_hunk_body_ends_where_its_counts_run_out() {
        // Two files, the first ending in a header-shaped addition. Without
        // counts the second file's headers would be read as more body.
        let patch = concat!(
            "diff --git a/first.md b/first.md\n",
            "--- a/first.md\n",
            "+++ b/first.md\n",
            "@@ -1 +1,2 @@\n",
            " keep\n",
            "+++ trailing\n",
            "diff --git a/second.md b/second.md\n",
            "--- a/second.md\n",
            "+++ b/second.md\n",
            "@@ -5,1 +5,1 @@\n",
            "-gone\n",
            "+here\n",
        );
        assert_eq!(
            locate_diff_line(patch, "first.md", "new", 2).unwrap().text,
            "++ trailing"
        );
        // The second file is still found, and numbered from its own header.
        let second = locate_diff_line(patch, "second.md", "new", 5).unwrap();
        assert_eq!(second.kind, "addition");
        assert_eq!(second.text, "here");
    }

    #[test]
    fn an_omitted_hunk_count_means_one_line() {
        assert_eq!(
            parse_hunk_header("@@ -1 +1 @@"),
            Some(super::HunkHeader {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
            })
        );
    }

    #[test]
    fn a_line_outside_every_hunk_is_refused_rather_than_guessed_at() {
        let error = locate_diff_line(PATCH, "src/main.rs", "new", 900).unwrap_err();
        assert_eq!(error.code, "diff_target_not_found");
        let missing = locate_diff_line(PATCH, "src/other.rs", "new", 10).unwrap_err();
        assert_eq!(missing.code, "diff_target_not_found");
    }
}
