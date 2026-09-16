//! Host-owned Copilot extension transport. No daemon input or composer access.
use super::{lan_trust::DesktopLocalAccess, task_input, AppState};
use crate::db::{
    copilot_wake::{Attempt, Registration},
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

pub(super) type Registry = HashMap<String, Connection>;
pub(super) struct Connection {
    binding: Registration,
    tx: mpsc::Sender<Value>,
}
static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(0);
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
    session_id: String,
}

fn command(operation: &str, attempt: &Attempt) -> Value {
    json!({"type":operation, "attempt":attempt})
}

struct StreamGuard {
    state: Arc<AppState>,
    binding: Registration,
}
impl Drop for StreamGuard {
    fn drop(&mut self) {
        let mut registry = self.state.copilot_wakes.lock().unwrap();
        if registry
            .get(&self.binding.run_id)
            .is_some_and(|c| c.binding.connection_id == self.binding.connection_id)
        {
            registry.remove(&self.binding.run_id);
        }
        drop(registry);
        if let Ok(db) = database(&self.state) {
            if let Ok(attempts) = db.pending_copilot_attempts(&self.binding.run_id) {
                for attempt in attempts {
                    if attempt.binding.connection_id == self.binding.connection_id {
                        let _ = db.receipt_copilot_wake(&attempt.id, &self.binding.connection_id, None, None,
                            Some("Copilot extension disconnected; native delivery uncertain, retained for history reconciliation"));
                    }
                }
            }
        }
        self.state.event_subscriptions_changed.notify_waiters();
    }
}

/// A live stream is the capability. Persisted registration alone never enables send.
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
        session_id: query.session_id,
        connection_id: format!(
            "copilot-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed)
        ),
    };
    let (tx, rx) = mpsc::channel(8);
    let pending = {
        let mut registry = state.copilot_wakes.lock().unwrap();
        let pending = database(&state)
            .map_err(storage)?
            .register_copilot_wake(&binding)
            .map_err(storage)?
            .ok_or((
                StatusCode::CONFLICT,
                "extension does not match the live Copilot task/run/native session".into(),
            ))?;
        registry.insert(
            binding.run_id.clone(),
            Connection {
                binding: binding.clone(),
                tx,
            },
        );
        pending
    };
    let initial = std::iter::once(json!({"type":"registered", "protocol":1, "binding":binding}))
        .chain(pending.iter().map(|a| command("inspect", a)))
        .collect::<Vec<_>>();
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
                    .copilot_wakes
                    .lock()
                    .unwrap()
                    .get(&guard.binding.run_id)
                    .is_some_and(|c| c.binding.connection_id == guard.binding.connection_id);
                if !current
                    || !database(&guard.state)
                        .and_then(|db| db.copilot_wake_bound(&guard.binding))
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
    let registry = state.copilot_wakes.lock().unwrap();
    let connection = registry
        .get(&row.run_id)
        .filter(|c| c.binding.task_id == row.task_id && !c.tx.is_closed())
        .ok_or_else(|| {
            failure(
                "Copilot wake extension unavailable; mailbox retained, no composer input sent"
                    .into(),
            )
        })?;
    // Reserve channel capacity before creating a durable uncertain attempt.
    let permit = connection
        .tx
        .try_reserve()
        .map_err(|_| failure("Copilot wake stream occupied; nothing sent".into()))?;
    let db = database(state).map_err(|e| failure(e.to_string()))?;
    let Some((attempt, fresh)) = db
        .prepare_copilot_wake(row, &connection.binding)
        .map_err(|e| failure(e.to_string()))?
    else {
        return Err(failure(
            "Copilot wake binding or batch changed; nothing sent".into(),
        ));
    };
    if attempt.input_id.is_some() {
        return Ok("notified");
    }
    permit.send(command(if fresh { "send" } else { "inspect" }, &attempt));
    Ok("awaiting_receipt")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Receipt {
    run_id: String,
    session_id: String,
    connection_id: String,
    attempt_id: String,
    kind: String,
    message_id: Option<String>,
    event_id: Option<String>,
    content: Option<String>,
    error: Option<String>,
}

pub(super) async fn receipt(
    _access: DesktopLocalAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(body): Json<Receipt>,
) -> Result<Json<Value>, ApiError> {
    let db = database(&state).map_err(storage)?;
    let attempt = db
        .copilot_attempt(&body.attempt_id)
        .map_err(storage)?
        .ok_or((StatusCode::NOT_FOUND, "wake attempt not found".into()))?;
    if attempt.binding.task_id != task_id
        || attempt.binding.run_id != body.run_id
        || attempt.binding.session_id != body.session_id
        || attempt.binding.connection_id != body.connection_id
    {
        return Err((StatusCode::CONFLICT, "receipt binding was replaced".into()));
    }
    let nonempty = |s: &Option<String>| {
        s.as_ref()
            .is_some_and(|s| !s.trim().is_empty() && s.len() <= 256)
    };
    let (message, event, error) = match body.kind.as_str() {
        "accepted" if nonempty(&body.message_id) => (body.message_id.as_deref(), None, None),
        "observed" if nonempty(&body.event_id) && body.content.as_deref() == Some(attempt.message.as_str()) => (None, body.event_id.as_deref(), None),
        "uncertain" => (None, None, Some(body.error.as_deref().unwrap_or("native outcome unknown"))),
        _ => return Err((StatusCode::BAD_REQUEST, "receipt requires a native queue id or matching history event; it cannot acknowledge a mailbox".into())),
    };
    if !db
        .receipt_copilot_wake(&attempt.id, &body.connection_id, message, event, error)
        .map_err(storage)?
    {
        return Err((
            StatusCode::CONFLICT,
            "receipt connection was replaced".into(),
        ));
    }
    state.event_subscriptions_changed.notify_waiters();
    Ok(Json(
        json!({"recorded":message.is_some() || event.is_some(), "acknowledged":false}),
    ))
}
