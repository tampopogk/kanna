//! Feedback reaches the agent's live session when it is free
//! (docs/specs/app-design.md §8).
//!
//! A person's comment, reply or `/agent` message is accepted with its outbox
//! row in one transaction. This worker, running whether or not any window is
//! open, sends the queued rows of each design session to its live agent
//! session as one message per batch, in the order they were accepted, and
//! only when the agent is free:
//!
//! - the daemon's `SubmitDesignInput` checks, at the session itself and
//!   immediately before writing, that the agent is idle with a runtime verdict
//!   and has no attested draft at its composer; otherwise it writes nothing;
//! - after a batch is written, the next waits until the agent has been seen
//!   busy (its turn started) or a grace period passed, so an idle verdict that
//!   lags behind the delivered turn cannot pour further batches into it;
//! - a thread whose anchor is not yet in the server's document waits briefly
//!   for it (§9.3).
//!
//! Delivery is at most once in the terminal. Each batch has a stable id the
//! daemon keeps a receipt for. A lost answer is resolved by asking the daemon;
//! an answer no daemon can give (it crashed mid-write) leaves the batch
//! `uncertain`, shown to the person, and never typed again unless they choose
//! to resend it.

use super::service;
use super::DesignRuntime;
use crate::db::design::{DesignDeliveryRow, DesignSessionRow};
use crate::db::{Db, TaskInputSource};
use crate::http_api::AppState;
use crate::mutation_provenance::ChannelIdentity;
use kanna_daemon::protocol::{
    Command as DaemonCommand, ComposerAttestation, DesignDeliveryOutcome, Event as DaemonEvent,
    SessionKind, SessionState, SessionStatus,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(2);
/// How long a delivered batch holds the next one back while the agent's
/// turn has not visibly started.
const TURN_GRACE: Duration = Duration::from_secs(20);
/// How long a comment waits for its anchor's document update.
const ANCHOR_GRACE: Duration = Duration::from_secs(30);
const MAX_BATCH_ITEMS: usize = 8;
const MAX_BATCH_CHARS: usize = 12_000;

/// After a batch reached a session: when, and whether the agent has since
/// been seen busy.
#[derive(Clone, Copy)]
struct TurnFence {
    delivered_at: Instant,
    saw_busy: bool,
}

/// What the worker remembers between passes, per server: the turn fences and
/// the daemon instance whose design-delivery capability was confirmed. Lost on
/// restart by design: a restart re-reads durable state and asks the daemon.
#[derive(Default)]
pub(crate) struct DeliveryMemory {
    fences: HashMap<String, TurnFence>,
    capable_instance: Option<String>,
}

/// Whether the task's live session can take feedback now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Readiness {
    Free {
        pid: u32,
    },
    Busy {
        reason: String,
    },
    /// No live PTY session for the task.
    Absent,
    /// The daemon could not be asked.
    Unknown {
        reason: String,
    },
}

pub(crate) async fn session_readiness(state: &AppState, task_id: &str) -> Readiness {
    let mut daemon =
        match crate::daemon_client::DaemonClient::connect(&state.config().daemon_dir).await {
            Ok(daemon) => daemon,
            Err(error) => {
                return Readiness::Unknown {
                    reason: format!("daemon unavailable: {error}"),
                }
            }
        };
    let sessions = match daemon.send_command(&DaemonCommand::List).await {
        Ok(DaemonEvent::SessionList { sessions }) => sessions,
        Ok(other) => {
            return Readiness::Unknown {
                reason: format!("unexpected daemon response: {other:?}"),
            }
        }
        Err(error) => {
            return Readiness::Unknown {
                reason: error.to_string(),
            }
        }
    };
    let Some(session) = sessions.into_iter().find(|session| {
        session.session_id == task_id
            && session.kind == SessionKind::Pty
            && matches!(session.state, SessionState::Active)
    }) else {
        return Readiness::Absent;
    };
    if !session.status_observed {
        return Readiness::Busy {
            reason: "the agent has no runtime verdict yet".into(),
        };
    }
    match session.status {
        SessionStatus::Busy => Readiness::Busy {
            reason: "the agent is working".into(),
        },
        SessionStatus::Waiting => Readiness::Busy {
            reason: "the agent is waiting for an answer at a prompt".into(),
        },
        SessionStatus::Idle if session.composer_attestation == ComposerAttestation::Typed => {
            Readiness::Busy {
                reason: "someone has an unsent draft at the agent's composer".into(),
            }
        }
        SessionStatus::Idle => Readiness::Free { pid: session.pid },
    }
}

/// The worker. Runs for the life of the server.
pub(crate) async fn run(state: Arc<AppState>) {
    reconcile_in_flight(&state).await;
    loop {
        let _ = tokio::time::timeout(TICK, state.design.delivery_woken()).await;
        deliver_all(&state).await;
        super::approval::run_handoffs(&state).await;
    }
}

async fn blocking<T: Send + 'static>(
    state: &AppState,
    work: impl FnOnce(&Db) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let db_path = state.config().db_path.clone();
    tokio::task::spawn_blocking(move || {
        let db = Db::open(&db_path).map_err(|error| format!("db error: {error}"))?;
        work(&db)
    })
    .await
    .map_err(|error| format!("design delivery worker failed: {error}"))?
}

async fn deliver_all(state: &Arc<AppState>) {
    let tasks = match blocking(state, |db| {
        db.tasks_with_open_design_deliveries()
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(tasks) => tasks,
        Err(error) => {
            log::warn!("design delivery scan failed: {error}");
            return;
        }
    };
    for task_id in tasks {
        if let Err(error) = deliver_task(state, &task_id).await {
            log::warn!("design feedback for {task_id} not delivered: {error}");
        }
    }
}

/// What the next batch is, if the design is ready to send one.
struct Batch {
    rows: Vec<DesignDeliveryRow>,
    message: String,
    /// A resend of an uncertain batch: its earlier attempt id, asked about
    /// before anything is typed.
    earlier_attempt: Option<String>,
}

fn next_batch(db: &Db, runtime: &DesignRuntime, task_id: &str) -> Result<Option<Batch>, String> {
    let deliveries = db
        .design_deliveries(task_id)
        .map_err(|error| error.to_string())?;
    if deliveries
        .iter()
        .any(|row| row.state == DesignDeliveryRow::DELIVERING)
    {
        // One batch at a time: the one in flight settles first.
        return Ok(None);
    }
    let Some(session) = db
        .design_session(task_id)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let design = crate::task_creator::task_design_stage(db, task_id)?;
    // The epoch fence: feedback only ever reaches the design session of the
    // current epoch, never a factory stage's agent after the hand-off.
    let designing = design.as_ref().is_some_and(|design| design.is_current())
        && session.status == DesignSessionRow::DESIGNING;
    let queued: Vec<&DesignDeliveryRow> = deliveries
        .iter()
        .filter(|row| row.state == DesignDeliveryRow::QUEUED && row.epoch == session.epoch)
        .collect();
    if queued.is_empty() || !designing {
        return Ok(None);
    }
    let view = service::view(db, runtime, db.db_path(), task_id, false)
        .map_err(|error| error.message().to_string())?;
    let mut comment_to_thread = HashMap::new();
    for thread in &view.threads {
        for comment in &thread.comments {
            comment_to_thread.insert(comment.id.clone(), (thread, comment));
        }
    }
    let now = crate::artifacts::rfc3339_utc(std::time::SystemTime::now());
    let mut rows = Vec::new();
    let mut items = Vec::new();
    let mut chars = 0;
    for row in queued {
        let Some((thread, comment)) = row
            .comment_id
            .as_deref()
            .and_then(|id| comment_to_thread.get(id))
        else {
            continue;
        };
        // A thread whose anchor has not reached the server's document waits
        // a moment for it, and holds everything after it (order is kept).
        if thread
            .anchor
            .as_ref()
            .is_some_and(|anchor| anchor.state == "pending")
            && !older_than(&row.created_at, &now, ANCHOR_GRACE)
        {
            break;
        }
        if rows.len() == MAX_BATCH_ITEMS
            || (chars > 0 && chars + comment.body.len() > MAX_BATCH_CHARS)
        {
            break;
        }
        chars += comment.body.len();
        items.push(render_item(thread, comment));
        rows.push(row.clone());
    }
    if rows.is_empty() {
        return Ok(None);
    }
    let earlier_attempt = rows[0].attempt_id.clone().filter(|attempt| {
        rows.iter()
            .all(|row| row.attempt_id.as_deref() == Some(attempt))
    });
    Ok(Some(Batch {
        message: render_message(task_id, &items),
        rows,
        earlier_attempt,
    }))
}

fn older_than(created_at: &str, now: &str, grace: Duration) -> bool {
    let parse = |value: &str| -> Option<i64> {
        // `YYYY-MM-DDTHH:MM:SS.mmmZ`: compare as seconds within the day and
        // the date, which is all a 30-second grace needs.
        let (date, time) = value.split_once('T')?;
        let time = time.trim_end_matches('Z');
        let mut parts = time.split(':');
        let hours: i64 = parts.next()?.parse().ok()?;
        let minutes: i64 = parts.next()?.parse().ok()?;
        let seconds: f64 = parts.next()?.parse().ok()?;
        let day: i64 = date.replace('-', "").parse().ok()?;
        Some(day * 100_000 + hours * 3600 + minutes * 60 + seconds as i64)
    };
    match (parse(created_at), parse(now)) {
        (Some(created), Some(now)) => now - created >= grace.as_secs() as i64,
        _ => true,
    }
}

fn render_item(thread: &service::ThreadView, comment: &service::CommentView) -> String {
    let is_reply = thread
        .comments
        .first()
        .is_some_and(|first| first.id != comment.id);
    let what = match (&thread.anchor, thread.kind.as_str()) {
        (Some(anchor), _) => format!(
            "{} on \u{201c}{}\u{201d} (block {}{})",
            if is_reply { "reply" } else { "comment" },
            anchor.quoted_text.as_deref().unwrap_or(""),
            anchor.block_id.as_deref().unwrap_or("?"),
            match anchor.state {
                "detached" => ", anchored text since deleted",
                _ => "",
            }
        ),
        (None, _) => {
            if is_reply {
                "reply to your answer".to_string()
            } else {
                "/agent message".to_string()
            }
        }
    };
    let body = comment
        .body
        .lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("#{} (thread {}) {what}:\n{body}", thread.number, thread.id)
}

fn render_message(task_id: &str, items: &[String]) -> String {
    format!(
        "Kanna App Design feedback ({} item{}, queued until you were free). Answer each in its \
         thread with kanna_design_reply {{\"task_id\": \"{task_id}\", \"thread_id\": \"<thread>\", \
         \"op_id\": \"<your id>\", \"body\": \"...\"}}, resolve it with kanna_design_resolve when \
         done, and edit the document with kanna_design_edit after reading it with kanna_design_get.\n\n{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" },
        items.join("\n\n")
    )
}

async fn daemon_instance(
    runtime: &DesignRuntime,
    daemon: &mut crate::daemon_client::DaemonClient,
) -> Result<String, String> {
    if let Some(instance) = runtime.delivery.lock().unwrap().capable_instance.clone() {
        return Ok(instance);
    }
    match daemon
        .send_command(&DaemonCommand::NegotiateDesignDelivery {
            version: kanna_daemon::protocol::DESIGN_DELIVERY_PROTOCOL_VERSION,
        })
        .await
    {
        Ok(DaemonEvent::DesignDeliveryReady { instance, .. }) => {
            runtime.delivery.lock().unwrap().capable_instance = Some(instance.clone());
            Ok(instance)
        }
        // Never a silent downgrade to immediate delivery: an older daemon
        // cannot check that the agent is free, so nothing is sent to it.
        Ok(other) => Err(format!(
            "the terminal daemon does not support queued design feedback ({other:?}); restart Kanna to update it"
        )),
        Err(error) => Err(format!(
            "the terminal daemon does not support queued design feedback ({error}); restart Kanna to update it"
        )),
    }
}

fn forget_capability(runtime: &DesignRuntime) {
    runtime.delivery.lock().unwrap().capable_instance = None;
}

async fn deliver_task(state: &Arc<AppState>, task_id: &str) -> Result<(), String> {
    let runtime = state.design.clone();
    let task = task_id.to_string();
    let Some(batch) = blocking(state, move |db| next_batch(db, &runtime, &task)).await? else {
        return Ok(());
    };
    // The turn fence: after a batch, wait for the agent to start working on
    // it (or for the grace period) before judging it free again.
    let readiness = session_readiness(state, task_id).await;
    let fence = state
        .design
        .delivery
        .lock()
        .unwrap()
        .fences
        .get(task_id)
        .copied();
    if let Some(mut fence) = fence {
        if matches!(readiness, Readiness::Busy { .. }) {
            fence.saw_busy = true;
            state
                .design
                .delivery
                .lock()
                .unwrap()
                .fences
                .insert(task_id.to_string(), fence);
        }
        if !fence.saw_busy && fence.delivered_at.elapsed() < TURN_GRACE {
            return Ok(());
        }
    }
    let pid = match readiness {
        Readiness::Free { pid } => pid,
        Readiness::Busy { reason } | Readiness::Unknown { reason } => {
            return note(state, &batch, &format!("waiting: {reason}")).await
        }
        Readiness::Absent => {
            return note(
                state,
                &batch,
                "waiting: the design session is not running; resume the task to deliver",
            )
            .await
        }
    };
    let mut daemon = crate::daemon_client::DaemonClient::connect(&state.config().daemon_dir)
        .await
        .map_err(|error| format!("daemon unavailable: {error}"))?;
    let instance = match daemon_instance(&state.design, &mut daemon).await {
        Ok(instance) => instance,
        Err(reason) => return note(state, &batch, &reason).await,
    };

    // A person's resend of an uncertain batch: ask first whether the daemon
    // already wrote it.
    if let Some(earlier) = batch.earlier_attempt.clone() {
        match query(&mut daemon, &earlier).await {
            Ok((DesignDeliveryOutcome::Delivered, _)) => {
                return settle_delivered(state, task_id, &earlier, &batch.message, false).await;
            }
            Ok((DesignDeliveryOutcome::Accepted, _)) => return Ok(()),
            _ => {}
        }
    }

    let attempt_id = format!(
        "da-{}",
        crate::artifacts::random_hex(12).map_err(|error| error.to_string())?
    );
    let ids: Vec<String> = batch.rows.iter().map(|row| row.id.clone()).collect();
    {
        let attempt_id = attempt_id.clone();
        let instance = instance.clone();
        blocking(state, move |db| {
            db.reserve_design_deliveries(&ids, &attempt_id, &instance)
                .map_err(|error| error.to_string())
        })
        .await?;
    }
    state.design.feed_changed(task_id);
    let answer = daemon
        .send_command(&DaemonCommand::SubmitDesignInput {
            session_id: task_id.to_string(),
            expected_pid: pid,
            delivery_id: attempt_id.clone(),
            data: batch.message.as_bytes().to_vec(),
        })
        .await
        .map_err(|error| error.to_string());
    match answer {
        Ok(DaemonEvent::DesignDelivery { outcome, .. }) => {
            settle_outcome(
                state,
                task_id,
                &attempt_id,
                &batch.message,
                outcome,
                &instance,
                &[],
            )
            .await
        }
        Ok(DaemonEvent::Error { code, message }) => {
            use kanna_daemon::protocol::ErrorCode;
            // Every daemon refusal is answered before a byte is written.
            let detail = match code {
                Some(ErrorCode::SessionNotFound | ErrorCode::SessionIncarnationMismatch) => {
                    "waiting: the design session changed; it will be delivered to the live session"
                        .to_string()
                }
                Some(ErrorCode::RetryOnSuccessor) => {
                    forget_capability(&state.design);
                    "waiting: the terminal daemon is being upgraded".to_string()
                }
                _ => format!("waiting: {message}"),
            };
            release(state, task_id, &attempt_id, &detail).await
        }
        Ok(other) => {
            mark_uncertain(
                state,
                task_id,
                &attempt_id,
                &format!("unexpected daemon answer: {other:?}"),
            )
            .await
        }
        Err(error) => {
            // The answer was lost; the daemon may or may not have written.
            forget_capability(&state.design);
            match reconnect_and_query(state, &attempt_id).await {
                Ok((outcome, known)) => {
                    settle_outcome(state, task_id, &attempt_id, &batch.message, outcome, &instance, &known).await
                }
                Err(query_error) => {
                    mark_uncertain(
                        state,
                        task_id,
                        &attempt_id,
                        &format!("the daemon's answer was lost ({error}) and it could not be asked ({query_error})"),
                    )
                    .await
                }
            }
        }
    }
}

async fn query(
    daemon: &mut crate::daemon_client::DaemonClient,
    delivery_id: &str,
) -> Result<(DesignDeliveryOutcome, Vec<String>), String> {
    match daemon
        .send_command(&DaemonCommand::QueryDesignDelivery {
            delivery_id: delivery_id.to_string(),
        })
        .await
    {
        Ok(DaemonEvent::DesignDelivery {
            outcome,
            known_instances,
            ..
        }) => Ok((outcome, known_instances)),
        Ok(other) => Err(format!("unexpected daemon answer: {other:?}")),
        Err(error) => Err(error.to_string()),
    }
}

async fn reconnect_and_query(
    state: &AppState,
    delivery_id: &str,
) -> Result<(DesignDeliveryOutcome, Vec<String>), String> {
    let mut daemon = crate::daemon_client::DaemonClient::connect(&state.config().daemon_dir)
        .await
        .map_err(|error| error.to_string())?;
    query(&mut daemon, delivery_id).await
}

#[allow(clippy::too_many_arguments)]
async fn settle_outcome(
    state: &Arc<AppState>,
    task_id: &str,
    attempt_id: &str,
    message: &str,
    outcome: DesignDeliveryOutcome,
    reserved_instance: &str,
    known_instances: &[String],
) -> Result<(), String> {
    match outcome {
        DesignDeliveryOutcome::Delivered => {
            settle_delivered(state, task_id, attempt_id, message, true).await
        }
        DesignDeliveryOutcome::NotFree { reason } => {
            release(state, task_id, attempt_id, &format!("waiting: {reason}")).await
        }
        // Still being written by the daemon: settle on a later pass.
        DesignDeliveryOutcome::Accepted => Ok(()),
        DesignDeliveryOutcome::WriteFailed { message } => {
            mark_uncertain(
                state,
                task_id,
                attempt_id,
                &format!("the terminal writer failed partway ({message}); check the agent's terminal before resending"),
            )
            .await
        }
        DesignDeliveryOutcome::Unknown
            if known_instances.is_empty()
                || known_instances.iter().any(|instance| instance == reserved_instance) =>
        {
            // The daemon that would have received it never did.
            release(state, task_id, attempt_id, "waiting: the daemon never received it").await
        }
        DesignDeliveryOutcome::Unknown => {
            mark_uncertain(
                state,
                task_id,
                attempt_id,
                "the terminal daemon restarted without its delivery records; check the agent's terminal before resending",
            )
            .await
        }
    }
}

async fn settle_delivered(
    state: &Arc<AppState>,
    task_id: &str,
    attempt_id: &str,
    message: &str,
    fence: bool,
) -> Result<(), String> {
    let task = task_id.to_string();
    let attempt = attempt_id.to_string();
    let message = message.to_string();
    let db_path = state.config().db_path.clone();
    blocking(state, move |db| {
        // The outbox rows and the task-input record are one transaction, and
        // settling twice records nothing twice.
        db.in_immediate_transaction_if_needed(|db| {
            let settled = db.mark_design_attempt_delivered(&attempt)?;
            if !settled.is_empty() {
                db.record_task_input(
                    &task,
                    TaskInputSource::Operator,
                    &ChannelIdentity::Server,
                    &message,
                )?;
            }
            Ok::<_, rusqlite::Error>(())
        })
        .map_err(|error| error.to_string())?;
        crate::task_store::flush_task_best_effort(db, &db_path, &task);
        Ok(())
    })
    .await?;
    if fence {
        state.design.delivery.lock().unwrap().fences.insert(
            task_id.to_string(),
            TurnFence {
                delivered_at: Instant::now(),
                saw_busy: false,
            },
        );
    }
    state.design.feed_changed(task_id);
    state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Tasks);
    Ok(())
}

async fn release(
    state: &Arc<AppState>,
    task_id: &str,
    attempt_id: &str,
    detail: &str,
) -> Result<(), String> {
    let attempt = attempt_id.to_string();
    let detail = detail.to_string();
    blocking(state, move |db| {
        db.release_design_attempt(&attempt, &detail)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await?;
    state.design.feed_changed(task_id);
    Ok(())
}

async fn mark_uncertain(
    state: &Arc<AppState>,
    task_id: &str,
    attempt_id: &str,
    detail: &str,
) -> Result<(), String> {
    let attempt = attempt_id.to_string();
    let detail = detail.to_string();
    blocking(state, move |db| {
        db.mark_design_attempt_uncertain(&attempt, &detail)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
    .await?;
    state.design.feed_changed(task_id);
    state.publish_state_changed(kanna_agent_protocol::StateChangeScope::Tasks);
    Ok(())
}

/// Say why queued feedback is waiting, on its rows, without changing them.
async fn note(state: &Arc<AppState>, batch: &Batch, detail: &str) -> Result<(), String> {
    if batch
        .rows
        .iter()
        .all(|row| row.detail.as_deref() == Some(detail))
    {
        return Ok(());
    }
    let ids: Vec<String> = batch.rows.iter().map(|row| row.id.clone()).collect();
    let detail = detail.to_string();
    let task_id = batch.rows[0].task_id.clone();
    blocking(state, move |db| {
        db.note_design_deliveries(&ids, &detail)
            .map_err(|error| error.to_string())
    })
    .await?;
    state.design.feed_changed(&task_id);
    Ok(())
}

/// After a restart: every batch recorded as being written is asked about
/// before anything else is sent. A batch no daemon remembers, sent to a
/// daemon that is gone, is uncertain.
async fn reconcile_in_flight(state: &Arc<AppState>) {
    let rows = match blocking(state, |db| {
        db.delivering_design_deliveries()
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            log::warn!("design delivery reconciliation skipped: {error}");
            return;
        }
    };
    let mut attempts: Vec<(String, String, String)> = rows
        .iter()
        .filter_map(|row| {
            Some((
                row.task_id.clone(),
                row.attempt_id.clone()?,
                row.daemon_instance.clone().unwrap_or_default(),
            ))
        })
        .collect();
    attempts.sort();
    attempts.dedup();
    for (task_id, attempt_id, instance) in attempts {
        let outcome = reconnect_and_query(state, &attempt_id).await;
        let message = blocking(state, {
            let task_id = task_id.clone();
            let attempt_id = attempt_id.clone();
            move |db| delivered_message(db, &task_id, &attempt_id)
        })
        .await
        .unwrap_or_default();
        let result = match outcome {
            Ok((outcome, known)) => {
                settle_outcome(
                    state,
                    &task_id,
                    &attempt_id,
                    &message,
                    outcome,
                    &instance,
                    &known,
                )
                .await
            }
            Err(error) => {
                mark_uncertain(
                    state,
                    &task_id,
                    &attempt_id,
                    &format!("could not ask the terminal daemon after a restart ({error})"),
                )
                .await
            }
        };
        if let Err(error) = result {
            log::warn!("design delivery reconciliation for {task_id}: {error}");
        }
    }
}

/// The text a reconciled batch would have delivered, for its task-input
/// record: rebuilt from the rows, since only the daemon kept the bytes.
fn delivered_message(db: &Db, task_id: &str, attempt_id: &str) -> Result<String, String> {
    let rows: Vec<DesignDeliveryRow> = db
        .design_deliveries(task_id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|row| row.attempt_id.as_deref() == Some(attempt_id))
        .collect();
    let comments: Vec<String> = rows
        .iter()
        .filter_map(|row| row.comment_id.as_deref())
        .filter_map(|id| db.design_comment(id).ok().flatten())
        .map(|comment| comment.body)
        .collect();
    Ok(render_message(task_id, &comments))
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod tests;
