//! Shared task-scoped auto-resolve fallback for callers that address a task
//! by id without knowing which machine owns it.
//!
//! `resolve_task_id_for_mutation` (`task_actions.rs`) already answers "does
//! *this* machine know this task id"; the handlers in scope for auto-resolve
//! (see docs/kanna-server-boundary.md's "Multi-machine Agent Routing"
//! section) instead need "does *some reachable machine* know it", with the
//! caller's exact request forwarded there when so — so a caller no longer
//! has to already know which machine a task lives on before it can act on
//! it. This lives in its own module rather than folded into
//! `resolve_task_id_for_mutation` itself: several call sites of that
//! function (`close_task`, `request_revision`, `complete_stage`,
//! `set_task_workflow`) are out of this feature's scope and must keep their
//! existing flat-404 behavior unchanged.
//!
//! This generalizes the probe `get_task` already used to produce its
//! "found elsewhere, retry with machine_id" hint: the probe itself
//! (`relay_and_lan_desktop_ids` + `invoke_desktop`, `localOnly=true`
//! appended so a remote hop's own local miss never recurses) is unchanged,
//! but the caller's original request is now actually forwarded and its
//! response returned, rather than just naming the owner and asking the
//! caller to repeat itself with `machine_id`.

use super::state::AppState;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kanna_tool_catalog::encode_path_segment;
use std::collections::HashSet;
use std::sync::Arc;

/// Query flag every auto-resolving route now accepts, mirroring
/// `GetTaskQuery::local_only`: set by this server itself on the one
/// federated hop it makes, so a remote server's own local miss can never
/// recurse into federating further.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct LocalOnlyQuery {
    #[serde(default)]
    pub(super) local_only: bool,
}

/// Either the task id resolved against *this* machine's own store
/// (`Local`), or the caller's exact request has already been forwarded to,
/// and answered by, the one reachable sibling that actually owns it
/// (`Remote`) — the handler must return that response as-is and must not
/// proceed with any local mutation using the caller-supplied id.
pub(super) enum TaskRoute {
    Local(String),
    Remote(Response),
}

/// Resolves `task_id` locally first (reusing `resolve_task_id_for_mutation`,
/// the same choke point non-federating routes still call directly); on a
/// local miss, with `local_only` false, forwards `method`/`path`/`body` to
/// every currently reachable sibling machine in turn and returns the first
/// one that recognizes the task id. A task id is globally unique, so that
/// answer is authoritative — success or the sibling's own application
/// error alike — and is returned to the caller unchanged rather than
/// re-interpreted here.
///
/// An uncertain delivery from a candidate (`invoke_desktop`'s
/// `delivery_uncertain`, status 0) is terminal, not "keep looking": the
/// caller must never receive a plain 404 for a task that may already have
/// received this exact mutation on another machine, and it is never
/// retried automatically on this server's behalf. A final "not found"
/// additionally names any paired machine this server could not reach to
/// confirm, so "no such task" and "a known machine was unreachable" never
/// read as the same answer.
pub(super) async fn resolve_task_route(
    state: &Arc<AppState>,
    task_id: &str,
    local_only: bool,
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<TaskRoute, (StatusCode, String)> {
    match super::task_actions::resolve_task_id_for_mutation(state, task_id).await {
        Ok(id) => Ok(TaskRoute::Local(id)),
        Err(error) if error.0 == StatusCode::NOT_FOUND && !local_only => {
            match federate(state, method, path, body).await {
                FederationOutcome::Found(response) => Ok(TaskRoute::Remote(response)),
                FederationOutcome::Uncertain { machine_id } => Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!(
                        "task {task_id} may exist on machine {machine_id}, but delivery there \
                         could not be confirmed (delivery_uncertain); check that machine directly \
                         once it is reachable rather than assuming this request never arrived"
                    ),
                )),
                FederationOutcome::NotFound { unreachable } if !unreachable.is_empty() => Err((
                    StatusCode::NOT_FOUND,
                    format!(
                        "task not found: {task_id} (checked every currently reachable machine; \
                         could not reach {} paired machine(s) to confirm: {})",
                        unreachable.len(),
                        unreachable.join(", ")
                    ),
                )),
                FederationOutcome::NotFound { .. } => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

enum FederationOutcome {
    Found(Response),
    Uncertain { machine_id: String },
    NotFound { unreachable: Vec<String> },
}

async fn federate(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
    body: &serde_json::Value,
) -> FederationOutcome {
    let (machine_ids, _relay_error) = super::invoke_desktop::relay_and_lan_desktop_ids(state).await;
    let forward_path = with_local_only(path);
    let mut checked: HashSet<String> = HashSet::new();
    checked.insert(state.config.desktop_id.clone());
    for machine_id in machine_ids {
        if machine_id == state.config.desktop_id {
            continue;
        }
        checked.insert(machine_id.clone());
        let Ok(routed) = super::invoke_desktop::invoke_desktop(
            Arc::clone(state),
            machine_id.clone(),
            method.to_string(),
            forward_path.clone(),
            body.clone(),
        )
        .await
        else {
            // A pre-dispatch routing failure (no pairing, no grant): nothing
            // was sent, so this candidate is exactly as informative as one
            // relay/LAN discovery never surfaced at all - keep looking.
            continue;
        };
        if routed.response.status == StatusCode::NOT_FOUND.as_u16() {
            continue;
        }
        if routed.response.status == 0 {
            return FederationOutcome::Uncertain { machine_id };
        }
        let status =
            StatusCode::from_u16(routed.response.status).unwrap_or(StatusCode::BAD_GATEWAY);
        return FederationOutcome::Found(forwarded_response(status, routed.response.body));
    }
    FederationOutcome::NotFound {
        unreachable: paired_but_unchecked(state, &checked),
    }
}

/// Rebuilds the sibling's response the way it would have rendered locally.
/// `response_to_http_invoke` (the other end of every `invoke_desktop` hop)
/// parses the wire body as JSON and only degrades to a plain
/// `Value::String` when it was not valid JSON - which is exactly what a
/// plain-text route (`task_logs`) always sends, and what a `(StatusCode,
/// String)` error tuple renders as by axum's own default. Reproducing that
/// split on the way back out is what lets one forwarder serve both JSON
/// routes and `task_logs` without either guessing at a route's shape or
/// double-encoding a plain-text body as a JSON string.
fn forwarded_response(status: StatusCode, body: Option<serde_json::Value>) -> Response {
    match body {
        Some(serde_json::Value::String(text)) => (status, text).into_response(),
        Some(value) => (status, axum::Json(value)).into_response(),
        None => status.into_response(),
    }
}

/// Paired machines (any environment) that this federation attempt could not
/// even include in its search - discovered neither on relay presence nor
/// via a cached LAN candidate. Distinct from a candidate that *was* dialed
/// and came back 404: this is "never got the chance to ask."
fn paired_but_unchecked(state: &Arc<AppState>, checked: &HashSet<String>) -> Vec<String> {
    let Ok(store) = state.peer_trust_store() else {
        return Vec::new();
    };
    let mut unreachable: Vec<String> = store
        .peers
        .into_iter()
        .map(|peer| peer.desktop_id)
        .filter(|id| !checked.contains(id))
        .collect();
    unreachable.sort();
    unreachable
}

fn with_local_only(path: &str) -> String {
    if path.contains('?') {
        format!("{path}&localOnly=true")
    } else {
        format!("{path}?localOnly=true")
    }
}

/// Builds the literal `/v1/tasks/{task_id}/...` path a handler's own route
/// corresponds to, for the one caller-supplied `task_id` that just failed to
/// resolve locally - never a canonical id, since resolving one is exactly
/// what failed.
pub(super) fn task_path(task_id: &str, suffix: &str) -> String {
    format!("/v1/tasks/{}{suffix}", encode_path_segment(task_id))
}
