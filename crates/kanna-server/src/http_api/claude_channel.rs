//! Claude native MCP-channel transport. No daemon input and no composer access.
//!
//! The wake leaves this server as a frame on an SSE stream held by the task's
//! own `kanna-mcp` child, which turns it into a `notifications/claude/channel`
//! on the MCP stdio it already owns. Nothing is typed anywhere, so an unsent
//! human draft — the thing that broke on 2026-09-17 — is never touched.
//!
//! Two facts from the live experiment are load-bearing here:
//!
//! * **Confirmation is not a startup fact.** A probe emitted during MCP
//!   initialization was dropped before the CLI could listen, and nothing ever
//!   retried it. So every admitted wake re-probes an unconfirmed channel
//!   rather than assuming one initialization-time probe was enough.
//! * **Written is not read.** A notice written into a running turn was
//!   absorbed by that turn with no mailbox read. The transport receipt says
//!   only that bytes reached the CLI; `event_subscriptions` asks this module
//!   whether the turn since ended without a read, and repeats the notice a
//!   bounded number of times if so.
use super::{lan_trust::DesktopLocalAccess, task_input, AppState};
use crate::db::{
    claude_channel::{Attempt, Registration},
    Db, EventSubscription,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use futures_util::{stream, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    convert::Infallible,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

pub(super) const UNCONFIRMED: &str = "Claude channel is not confirmed; enable it at launch and \
     confirm its probe. Mailbox remains pending; no terminal input sent";

pub(super) type Registry = HashMap<String, Connection>;
pub(super) struct Connection {
    binding: Registration,
    tx: mpsc::Sender<Value>,
}
static NEXT_CHANNEL: AtomicU64 = AtomicU64::new(0);
type ApiError = (StatusCode, String);
fn storage(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn database(state: &AppState) -> Result<Db, rusqlite::Error> {
    Db::open(&state.config().db_path)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Binding {
    run_id: String,
}

fn probe(binding: &Registration) -> Value {
    json!({"type": "probe", "channelId": binding.channel_id, "taskId": binding.task_id})
}

struct StreamGuard {
    state: Arc<AppState>,
    binding: Registration,
}
impl Drop for StreamGuard {
    fn drop(&mut self) {
        let mut registry = self.state.claude_channels.lock().unwrap();
        if registry
            .get(&self.binding.run_id)
            .is_some_and(|c| c.binding.channel_id == self.binding.channel_id)
        {
            registry.remove(&self.binding.run_id);
        }
        drop(registry);
        self.state.event_subscriptions_changed.notify_waiters();
    }
}

/// A live MCP stream is the capability. A persisted registration is identity
/// only, and a reconnect starts unconfirmed however recently the last one was
/// confirmed — the new channel has not been probed by anybody yet.
pub(super) async fn connect(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Query(query): Query<Binding>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let _mutation = state.try_begin_requested_task_mutation(&task_id).ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "subscriber is changing sessions; retry registration".into(),
    ))?;
    let binding = Registration {
        task_id,
        run_id: query.run_id,
        channel_id: format!(
            "channel-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT_CHANNEL.fetch_add(1, Ordering::Relaxed)
        ),
        confirmed: false,
    };
    let (tx, rx) = mpsc::channel(8);
    {
        let mut registry = state.claude_channels.lock().unwrap();
        if !database(&state)
            .map_err(storage)?
            .register_claude_channel(&binding)
            .map_err(storage)?
        {
            return Err((
                StatusCode::CONFLICT,
                "channel does not match the live Claude task/run".into(),
            ));
        }
        registry.insert(
            binding.run_id.clone(),
            Connection {
                binding: binding.clone(),
                tx,
            },
        );
    }
    let initial = vec![
        json!({"type": "registered", "protocol": 1, "binding": binding}),
        probe(&binding),
    ];
    let changes = state.subscribe_state_changes();
    let guard = StreamGuard {
        state: state.clone(),
        binding,
    };
    let live = stream::unfold(
        (rx, changes, guard),
        |(mut rx, mut changes, guard)| async move {
            loop {
                let notifier = guard.state.event_subscriptions_changed.clone();
                let changed = notifier.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let current = guard
                    .state
                    .claude_channels
                    .lock()
                    .unwrap()
                    .get(&guard.binding.run_id)
                    .is_some_and(|c| c.binding.channel_id == guard.binding.channel_id);
                if !current
                    || !database(&guard.state)
                        .and_then(|db| db.claude_channel_bound(&guard.binding))
                        .unwrap_or(false)
                {
                    return None;
                }
                let frame = tokio::select! {
                    frame = rx.recv() => frame,
                    _ = changes.recv() => continue,
                    _ = changed.as_mut() => continue,
                };
                return frame.map(|frame| (frame, (rx, changes, guard)));
            }
        },
    );
    state.event_subscriptions_changed.notify_waiters();
    Ok(Sse::new(
        stream::iter(initial)
            .chain(live)
            .map(|frame| Ok(Event::default().data(frame.to_string()))),
    )
    .keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// Whether this run's own MCP child is holding a live channel stream.
///
/// This — not a provider name, a historical session id, or a cwd — is the
/// measured capability the transport choice is made on. Deliberately *not*
/// "confirmed": an attached but unconfirmed channel still owns the wake, and
/// [`deliver`] re-probes it and keeps the batch pending. Falling back to the
/// composer because a probe has not been answered yet is exactly the mistake
/// that put engine text into somebody's draft.
pub(super) fn attached_channel(state: &AppState, run_id: &str, task_id: &str) -> bool {
    state
        .claude_channels
        .lock()
        .unwrap()
        .get(run_id)
        .is_some_and(|c| c.binding.task_id == task_id && !c.tx.is_closed())
}

pub(super) fn deliver(
    state: &AppState,
    row: &EventSubscription,
) -> Result<&'static str, task_input::EngineWakeFailure> {
    let failure = |message: String| task_input::EngineWakeFailure {
        retry: task_input::DeliveryRetry::Transient,
        message,
    };
    let _mutation = state
        .try_begin_requested_task_mutation(&row.task_id)
        .ok_or_else(|| failure("subscriber is changing sessions; nothing sent".into()))?;
    let registry = state.claude_channels.lock().unwrap();
    let connection = registry
        .get(&row.run_id)
        .filter(|c| c.binding.task_id == row.task_id && !c.tx.is_closed())
        .ok_or_else(|| {
            failure(
                "Claude channel host is not attached; mailbox retained, no terminal input sent"
                    .into(),
            )
        })?;
    let db = database(state).map_err(|e| failure(e.to_string()))?;
    let registration = db
        .claude_channel_registration(&row.run_id)
        .map_err(|e| failure(e.to_string()))?
        .filter(|r| r.channel_id == connection.binding.channel_id)
        .ok_or_else(|| failure("Claude channel registration was replaced; nothing sent".into()))?;
    // Re-probe from the admission itself. The experiment proved a single
    // initialization-time probe can be lost before the CLI is listening, and
    // nothing else would ever ask again.
    if !registration.confirmed {
        if let Ok(permit) = connection.tx.try_reserve() {
            permit.send(probe(&registration));
        }
        return Err(failure(UNCONFIRMED.into()));
    }
    // Reserve channel capacity before creating a durable uncertain attempt.
    let permit = connection
        .tx
        .try_reserve()
        .map_err(|_| failure("Claude channel stream occupied; nothing sent".into()))?;
    let Some((attempt, fresh)) = db
        .prepare_claude_channel_wake(row, &registration)
        .map_err(|e| failure(e.to_string()))?
    else {
        return Err(failure(
            "Claude channel binding or batch changed; nothing sent".into(),
        ));
    };
    // A repeat of an already-written notice is the bounded post-turn
    // follow-up, counted so it can never become a loop.
    if attempt.written_at.is_some()
        && !db
            .record_claude_channel_follow_up(&attempt.id)
            .map_err(|e| failure(e.to_string()))?
    {
        return Ok("notified");
    }
    permit.send(json!({"type": "wake", "fresh": fresh, "attempt": attempt}));
    Ok("awaiting_receipt")
}

/// Whether a written notice was absorbed by the turn it landed in and the turn
/// has since ended without the subscriber reading its batch. The follow-up is
/// event-driven — the caller is already awake on a task state change — and is
/// bounded by `MAX_FOLLOW_UPS`, because the mailbox is durable and a wake that
/// repeats forever is a scheduler.
pub(super) fn needs_post_turn_follow_up(state: &AppState, row: &EventSubscription) -> bool {
    let Ok(db) = database(state) else {
        return false;
    };
    let Ok(Some(attempt)) = db.claude_channel_attempt(&format!("{}-{}", row.id, row.batch_id))
    else {
        return false;
    };
    if !attempt.absorbed_unread() {
        return false;
    }
    // A recorded non-busy verdict, never an absent one: "the daemon has not
    // classified this session yet" is not evidence that a turn ended, and
    // repeating a notice on no evidence is how a wake becomes a nag.
    db.get_pipeline_item(&row.task_id)
        .ok()
        .flatten()
        .and_then(|task| task.runtime_status)
        .is_some_and(|status| status != "busy")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Confirm {
    channel_id: String,
}

/// The subscriber's own confirmation of a probe, relayed by `kanna-mcp` from
/// the `kanna_confirm_event_channel` tool call. It proves the CLI is listening
/// on this channel; it says nothing about any batch and acknowledges nothing.
pub(super) async fn confirm(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(body): Json<Confirm>,
) -> Result<Json<Value>, ApiError> {
    let registration = database(&state)
        .map_err(storage)?
        .confirm_claude_channel(&task_id, &body.channel_id)
        .map_err(storage)?
        .ok_or((
            StatusCode::CONFLICT,
            "channel id does not match this task's live Claude channel".into(),
        ))?;
    state.event_subscriptions_changed.notify_waiters();
    Ok(Json(json!({
        "confirmed": true,
        "channelId": registration.channel_id,
        "acknowledged": false,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Receipt {
    run_id: String,
    channel_id: String,
    attempt_id: String,
    /// `written` once the notification reached the MCP transport; `uncertain`
    /// when the write failed or its outcome is unknown. There is no third
    /// value: this host cannot see whether the model read anything.
    kind: String,
    error: Option<String>,
}

pub(super) async fn receipt(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(body): Json<Receipt>,
) -> Result<Json<Value>, ApiError> {
    let db = database(&state).map_err(storage)?;
    let attempt: Attempt = db
        .claude_channel_attempt(&body.attempt_id)
        .map_err(storage)?
        .ok_or((StatusCode::NOT_FOUND, "channel attempt not found".into()))?;
    if attempt.binding.task_id != task_id
        || attempt.binding.run_id != body.run_id
        || attempt.binding.channel_id != body.channel_id
    {
        return Err((StatusCode::CONFLICT, "receipt binding was replaced".into()));
    }
    let written = match body.kind.as_str() {
        "written" => true,
        "uncertain" => false,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "receipt reports a transport write outcome; it cannot report a mailbox read or acknowledgement"
                    .into(),
            ))
        }
    };
    let error = body
        .error
        .as_deref()
        .filter(|e| !e.trim().is_empty() && e.len() <= 256);
    if !db
        .receipt_claude_channel_wake(&attempt.id, &body.channel_id, written, error)
        .map_err(storage)?
    {
        return Err((StatusCode::CONFLICT, "receipt channel was replaced".into()));
    }
    state.event_subscriptions_changed.notify_waiters();
    Ok(Json(json!({"recorded": written, "acknowledged": false})))
}
