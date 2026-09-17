//! What the server does when a provider refuses a turn for capacity.
//!
//! The incident this exists for: on 2026-09-16 the Codex CLI answered a turn
//! on the iOS release Ship task with
//! `⚠ Selected model is at capacity. Please try a different model.` and went
//! straight back to its composer. Nothing woke. The runtime channel had
//! nothing to say — the refusal never sustained a busy classification, so no
//! busy→idle edge fired — and the notice channel had nothing to match, because
//! the sentence was not in any measured rule. The owner found the stalled task
//! by looking at it; the manager could not have.
//!
//! What this is *not* is as load-bearing as what it is:
//!
//! - **It is not a quota rejection.** Nothing is spent and nothing has to
//!   reset. So no row is written to `task_provider_rejection`, no candidate is
//!   burned at this stage, no fallback is prepared or spawned, and the task is
//!   never parked as quota-parked. A transient refusal that retired one of a
//!   stage's ordered candidates would be a worse bug than the silence it
//!   replaced.
//! - **It changes nothing about the run.** The session is alive and parked at
//!   its composer, and the run is still running. Finishing it, killing it, or
//!   replacing it would all be lies about a live agent, and the recovery — try
//!   the turn again — is available precisely because none of that happened.
//!
//! What it does is make the fact observable: one durable row, one actionable
//! event a supervisor wakes on, one task-detail field, and the task marked
//! unread, which is what "waiting for a person" already means here.

use super::state::AppState;
use crate::db::{Db, NewProviderCapacityNotice, QuotaRejectionSource, TaskEventKind};

/// One capacity announcement from the daemon, whichever surface stated it.
pub(crate) struct ProviderCapacityNotice {
    pub session_id: String,
    pub provider: String,
    pub scope: Option<String>,
    pub rule_id: String,
    pub text: String,
    pub cli_version: Option<String>,
    pub source: QuotaRejectionSource,
}

/// Record a capacity refusal and announce it.
///
/// Never returns an error to its caller: this rides the terminal-state
/// watcher, and a task whose refusal could not be recorded must not take the
/// watcher's event loop down with it.
pub(crate) fn handle_provider_capacity_notice(state: &AppState, notice: ProviderCapacityNotice) {
    match apply_capacity_notice(state, &notice) {
        Ok(Some(task_id)) => {
            log::warn!(
                "[capacity] {} refused a turn for task {} (model {:?}); the session is alive and \
                 the turn can be retried",
                notice.provider,
                task_id,
                notice.scope,
            );
            state.publish_task_state_changed(&task_id);
        }
        Ok(None) => {}
        Err(error) => log::warn!(
            "[capacity] could not record the capacity refusal from {} for session {}: {error}",
            notice.provider,
            notice.session_id,
        ),
    }
}

fn apply_capacity_notice(
    state: &AppState,
    notice: &ProviderCapacityNotice,
) -> Result<Option<String>, String> {
    let config = state.config();
    let db = Db::open(&config.db_path).map_err(|error| format!("db error: {error}"))?;
    // The same precondition a quota rejection is held to: a refusal against
    // anything but an open task's own still-running attempt on the provider
    // that refused describes a run that no longer exists.
    let Some(attempt) =
        super::quota_recovery::refused_attempt(&db, &notice.session_id, &notice.provider)?
    else {
        return Ok(None);
    };
    // Written before anything is announced, and keyed by the refused run, so a
    // re-announcement from a re-adopted session finds this row rather than
    // waking a supervisor twice for one refusal.
    let Some(_recorded) = db
        .record_provider_capacity_notice(NewProviderCapacityNotice {
            task_id: &attempt.task_id,
            stage_run_id: &attempt.run_id,
            stage: &attempt.stage,
            provider: &notice.provider,
            // The model the run selected. The measured chrome refuses *that*
            // model without naming it, so this is read from the run Kanna
            // started rather than parsed out of the sentence.
            model: attempt.model.as_deref(),
            effort: attempt.effort.as_deref(),
            source: notice.source,
            rule_id: &notice.rule_id,
            matched_text: &notice.text,
            scope: notice.scope.as_deref(),
            cli_version: notice.cli_version.as_deref(),
        })
        .map_err(|error| format!("db error: {error}"))?
    else {
        return Ok(None);
    };

    db.append_task_event(
        &attempt.task_id,
        TaskEventKind::ProviderCapacityRefused,
        serde_json::json!({
            "provider": notice.provider,
            "model": attempt.model,
            "effort": attempt.effort,
            "scope": notice.scope,
            "stage": attempt.stage,
            "stageRunId": attempt.run_id,
            "source": notice.source.as_str(),
            "ruleId": notice.rule_id,
            "matchedText": notice.text,
            "cliVersion": notice.cli_version,
            "action": CAPACITY_ACTION,
        }),
    )
    .map_err(|error| format!("db error: {error}"))?;
    // The task is not failed and not finished — it is waiting for somebody to
    // retry the turn. Unread is the display state that already says that.
    db.update_pipeline_item_activity(&attempt.task_id, "unread")
        .map_err(|error| format!("db error: {error}"))?;
    Ok(Some(attempt.task_id))
}

/// What a person or a manager can actually do, in words.
///
/// Operator instructions are a contract like any other surface: every
/// operation named here has to exist and has to be the right one. Retrying the
/// turn is first because it is what the refusal itself asks for and what the
/// owner did by hand during the incident — the session is still there, so the
/// instruction can simply be sent again.
pub(crate) const CAPACITY_ACTION: &str =
    "The model this run selected is at capacity right now. Nothing is spent, no work was \
     replaced and no provider fallback was started: the session is alive at its composer and the \
     run is still running. Retry the turn by sending the instruction again with \
     kanna_send_task_input. If the model stays at capacity, move the stage onto another model \
     with kanna_replace_task_workflow and rerun it.";
