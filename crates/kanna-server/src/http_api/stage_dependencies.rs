//! Readiness of stage dependency edges (spec §9, T4).
//!
//! Satisfaction is decided in SQLite, inside the upstream's own transition
//! or close (see `db::stage_edges`). This module does what follows once a
//! dependent may move: a task that has not started yet enters the stage it
//! starts in, forked from its first base edge's recorded commit; a task whose
//! automatic completion parked on edges into its next stage replays that
//! completion. Each dependent is handled under its own mutation lease and
//! re-decided from durable state there, so a restart, a startup sweep and a
//! late departure racing each other start or advance it once.
//!
//! The engine merges nothing here and never creates integration tasks; the
//! legacy blocker path in `task_blockers.rs` keeps doing so for pre-T4
//! tasks only.

use super::state::AppState;
use crate::db::Db;
use crate::mutation_provenance::ChannelIdentity;
use kanna_agent_protocol::StateChangeScope;
use std::sync::Arc;

enum Readiness {
    Waiting,
    Start,
    Advance(Box<crate::task_creator::PreparedStageTransition>),
}

/// What `task_id`'s edges allow now, read from durable state.
fn decide(db: &Db, config: &crate::config::Config, task_id: &str) -> Result<Readiness, String> {
    let db_error = |error: rusqlite::Error| format!("db error: {error}");
    let Some(item) = db.get_pipeline_item(task_id).map_err(db_error)? else {
        return Ok(Readiness::Waiting);
    };
    if item.closed_at.is_some() {
        return Ok(Readiness::Waiting);
    }
    let has_stage_edges = !db
        .list_stage_edges_into(task_id)
        .map_err(db_error)?
        .is_empty();
    if has_stage_edges
        && db
            .get_task_worktree_path(task_id)
            .map_err(db_error)?
            .is_some()
        && db.latest_stage_run(task_id).map_err(db_error)?.is_none()
    {
        // A start interrupted between recording its workspace and recording
        // its first run (a restart, or a daemon that could not be reached).
        // The run is written before any spawn, so no session exists: undo
        // the partial start and decide again as an unstarted task. Its edges
        // keep their reserved inputs, so it forks from the same commit.
        log::warn!("rolling back the interrupted dependency start of {task_id}");
        crate::task_creator::rollback_interrupted_dependency_start(db, task_id)?;
    }
    if db
        .get_task_worktree_path(task_id)
        .map_err(db_error)?
        .is_none()
    {
        // Not started. Only tasks with stage edges start here; a legacy
        // dormant task keeps its own blocker path.
        if !has_stage_edges || db.count_open_task_blockers(task_id).map_err(db_error)? > 0 {
            return Ok(Readiness::Waiting);
        }
        let stage = item.stage.clone().unwrap_or_default();
        return Ok(
            match db
                .stage_edge_inputs(task_id, &stage, true)
                .map_err(db_error)?
            {
                Some(_) => Readiness::Start,
                None => Readiness::Waiting,
            },
        );
    }
    let Some(wait) = db.dependency_wait(task_id).map_err(db_error)? else {
        return Ok(Readiness::Waiting);
    };
    if !db.dependency_wait_is_current(&wait).map_err(db_error)? {
        // The task moved on (a person advanced, reran or resumed it): the
        // parked completion is no longer owed.
        db.clear_dependency_wait(task_id).map_err(db_error)?;
        db.sync_blocked_event(task_id).map_err(db_error)?;
        return Ok(Readiness::Waiting);
    }
    if !db
        .unsatisfied_stage_edges_into(task_id, &wait.to_stage)
        .map_err(db_error)?
        .is_empty()
    {
        return Ok(Readiness::Waiting);
    }
    let text = |key: &str| {
        wait.payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let exit = wait
        .payload
        .get("exit")
        .filter(|exit| !exit.is_null())
        .and_then(|exit| serde_json::from_value::<crate::db::TransitionExit>(exit.clone()).ok());
    let prepared = crate::task_creator::prepare_stage_completion_for_api_with_trigger(
        db,
        config,
        task_id,
        text("kind").as_deref(),
        text("completionTransition").as_deref(),
        text("trigger").as_deref(),
        exit.as_ref(),
    )?;
    Ok(match prepared {
        Some(mut transition) => {
            // The engine applying the stage's policy once its edges allow it.
            transition.set_entry_channel(ChannelIdentity::Server);
            transition.set_entry_exit(exit);
            Readiness::Advance(Box::new(transition))
        }
        None => Readiness::Waiting,
    })
}

/// Start or advance `task_id` if its stage dependency edges now allow it.
/// `Ok(true)` when it started or advanced.
pub(super) async fn ensure_dependencies_ready(
    state: &Arc<AppState>,
    task_id: &str,
) -> Result<bool, String> {
    let _task_mutation = state.begin_requested_task_mutation(task_id).await;
    let readiness = {
        let state = Arc::clone(state);
        let task_id = task_id.to_string();
        tokio::task::spawn_blocking(move || {
            let db =
                Db::open(&state.config.db_path).map_err(|error| format!("db error: {error}"))?;
            decide(&db, &state.config, &task_id)
        })
        .await
        .map_err(|error| format!("dependency readiness worker failed: {error}"))??
    };
    match readiness {
        Readiness::Waiting => Ok(false),
        Readiness::Start => {
            let started =
                super::task_blockers::start_dormant_task_if_ready(state, task_id, Vec::new())
                    .await
                    .map_err(|(_, error)| error)?;
            if started {
                state.publish_state_changed(StateChangeScope::Blockers);
                state.publish_state_changed(StateChangeScope::Tasks);
            }
            Ok(started)
        }
        Readiness::Advance(transition) => {
            let mut daemon = crate::daemon_client::DaemonClient::connect(&state.config.daemon_dir)
                .await
                .map_err(|error| format!("daemon error: {error}"))?;
            if let Err((_, error)) = super::task_actions::execute_stage_transition(
                state,
                &mut daemon,
                task_id,
                *transition,
            )
            .await
            {
                super::task_actions::record_stage_transition_failure(state, task_id, &error);
                return Err(error);
            }
            state.publish_state_changed(StateChangeScope::Blockers);
            Ok(true)
        }
    }
}

/// After `upstream_task_id` left a stage or closed: bring forward each open
/// dependent its edges now allow. Detached — it never holds the caller's
/// lease while taking a dependent's, so two tasks gating each other at
/// different stages cannot deadlock — and every failure is logged, never
/// returned to the upstream's transition.
pub(super) fn spawn_dependents_readiness(state: &Arc<AppState>, upstream_task_id: &str) {
    let state = Arc::clone(state);
    let upstream_task_id = upstream_task_id.to_string();
    tokio::spawn(async move {
        let dependents = {
            let state = Arc::clone(&state);
            let upstream_task_id = upstream_task_id.clone();
            tokio::task::spawn_blocking(move || {
                Db::open(&state.config.db_path)
                    .and_then(|db| db.list_stage_edge_dependents(&upstream_task_id))
            })
            .await
        };
        let dependents = match dependents {
            Ok(Ok(dependents)) => dependents,
            Ok(Err(error)) => {
                log::error!("cannot list stage dependents of {upstream_task_id}: {error}");
                return;
            }
            Err(error) => {
                log::error!("stage dependents worker failed for {upstream_task_id}: {error}");
                return;
            }
        };
        for dependent in dependents {
            match ensure_dependencies_ready(&state, &dependent).await {
                Ok(true) => log::info!(
                    "dependency edges of {dependent} satisfied after {upstream_task_id} moved; \
                     it started or advanced"
                ),
                Ok(false) => {}
                Err(error) => log::error!(
                    "failed to bring dependent {dependent} forward after {upstream_task_id} \
                     moved: {error}"
                ),
            }
        }
    });
}

/// Startup: a departure or close that committed before a restart may never
/// have reached its dependents. Re-decide every open task that has edges or
/// a parked completion.
pub(crate) async fn resume_stage_dependency_readiness(state: Arc<AppState>) {
    let task_ids = {
        let state = Arc::clone(&state);
        tokio::task::spawn_blocking(move || {
            Db::open(&state.config.db_path).and_then(|db| db.list_open_stage_edge_dependents())
        })
        .await
    };
    let task_ids = match task_ids {
        Ok(Ok(task_ids)) => task_ids,
        Ok(Err(error)) => {
            log::error!("failed to list stage dependents: {error}");
            return;
        }
        Err(error) => {
            log::error!("stage dependency sweep worker failed: {error}");
            return;
        }
    };
    for task_id in task_ids {
        if let Err(error) = ensure_dependencies_ready(&state, &task_id).await {
            log::error!("stage dependency sweep could not bring {task_id} forward: {error}");
        }
    }
}
