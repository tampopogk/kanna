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
/// retried automatically on this server's behalf. A miss while any known
/// machine could not be asked is not "not found" at all: the owner may be
/// that machine, and no cross-machine action happens while the owner is
/// unreachable, so the answer is an explicit `503 task_owner_unreachable`
/// naming every machine this server could not reach or dispatch to. Only a
/// miss with every known machine asked is a plain 404. Either way nothing is
/// done locally with the caller's id.
///
/// Only same-account machines are candidates: relay presence lists this
/// account's desktops, LAN candidates need a same-account grant or pin, and
/// `invoke_desktop` refuses a pin that does not place the sibling in this
/// account (`crate::account_boundary`). A sibling that refuses this desktop
/// on the account boundary has not answered for the task, so it is a
/// dispatch failure here, never the task's authoritative answer.
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
                FederationOutcome::NotFound {
                    unreachable,
                    dispatch_failures,
                } if !unreachable.is_empty() || !dispatch_failures.is_empty() => Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    format!(
                        "task_owner_unreachable: {}",
                        not_found_message(task_id, &unreachable, &dispatch_failures)
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
    Uncertain {
        machine_id: String,
    },
    NotFound {
        unreachable: Vec<String>,
        dispatch_failures: Vec<(String, String)>,
    },
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
    let mut dispatch_failures: Vec<(String, String)> = Vec::new();
    for machine_id in machine_ids {
        if machine_id == state.config.desktop_id {
            continue;
        }
        let routed = match super::invoke_desktop::invoke_desktop(
            Arc::clone(state),
            machine_id.clone(),
            method.to_string(),
            forward_path.clone(),
            body.clone(),
        )
        .await
        {
            Ok(routed) => routed,
            Err(error) => {
                // A pre-dispatch routing failure (no pairing, no grant, a
                // dial failure): this candidate was discovered but this
                // attempt never actually managed to ask it, so it must NOT
                // be marked `checked` - doing so would silently fold "we
                // found it and could not reach it" into
                // `paired_but_unchecked`'s "never discovered at all" list,
                // erasing the one fact this branch exists to keep. Record
                // the machine and the reason and keep looking; a failed
                // dispatch is not an answer.
                dispatch_failures.push((machine_id, error));
                continue;
            }
        };
        if let Some(refusal) = account_boundary_refusal(&routed.response) {
            dispatch_failures.push((machine_id, refusal));
            continue;
        }
        checked.insert(machine_id.clone());
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
        dispatch_failures,
    }
}

/// The sibling's refusal text when it refused this desktop on the account
/// boundary before routing the request (`crate::account_boundary`).
fn account_boundary_refusal(response: &super::state::HttpInvokeResponse) -> Option<String> {
    let status = response.status;
    if status != StatusCode::FORBIDDEN.as_u16() && status != StatusCode::UNAUTHORIZED.as_u16() {
        return None;
    }
    let text = match &response.body {
        Some(serde_json::Value::String(text)) => text.as_str(),
        _ => response.error.as_deref()?,
    };
    crate::account_boundary::is_refusal_text(text).then(|| text.to_string())
}

/// Builds the final not-found message, keeping two distinct kinds of "this
/// might not mean what a bare 404 means" apart: a paired machine
/// `paired_but_unchecked` says this attempt never even discovered (no relay
/// presence, no cached LAN candidate), and a machine this attempt *did*
/// discover but could not successfully dispatch to (named in
/// `dispatch_failures`, alongside the reason `invoke_desktop` gave). Neither
/// list folds into the other, so a caller reading this message can tell "no
/// such task" apart from "a known machine was unreachable" apart from "a
/// discovered machine refused or failed the dispatch."
fn not_found_message(
    task_id: &str,
    unreachable: &[String],
    dispatch_failures: &[(String, String)],
) -> String {
    let mut message =
        format!("task not found: {task_id} (checked every currently reachable machine");
    if !unreachable.is_empty() {
        message.push_str(&format!(
            "; could not reach {} paired machine(s) to confirm: {}",
            unreachable.len(),
            unreachable.join(", ")
        ));
    }
    if !dispatch_failures.is_empty() {
        let details = dispatch_failures
            .iter()
            .map(|(machine_id, reason)| format!("{machine_id} ({reason})"))
            .collect::<Vec<_>>()
            .join(", ");
        message.push_str(&format!(
            "; discovered but could not dispatch to {} machine(s): {}",
            dispatch_failures.len(),
            details
        ));
    }
    message.push(')');
    message
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

/// Same-account paired machines (any environment) that this federation attempt could not
/// even include in its search - discovered neither on relay presence nor
/// via a cached LAN candidate. Distinct from a candidate that *was* dialed
/// and came back 404: this is "never got the chance to ask."
fn paired_but_unchecked(state: &Arc<AppState>, checked: &HashSet<String>) -> Vec<String> {
    let Ok(store) = state.peer_trust_store() else {
        return Vec::new();
    };
    // A pin that does not place the sibling in this account can never be
    // asked (`invoke_desktop` refuses it), so it can never own a task this
    // desktop may act on; it is not an unreachable owner.
    let current_account_uid = state.authenticated_account_uid();
    let mut unreachable: Vec<String> = store
        .peers
        .into_iter()
        .filter(|peer| {
            crate::account_boundary::peer_standing(peer, current_account_uid.as_deref()).is_ok()
        })
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
