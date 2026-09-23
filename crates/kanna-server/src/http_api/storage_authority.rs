//! Disk authority at runtime (spec §11, §16.11 — T13c): a task the
//! publisher found the disk ahead of is reconciled from its directory under
//! the task's mutation lease, so no mutation of the task runs meanwhile.

use super::AppState;
use crate::db::Db;
use crate::task_store::authority::{self, Mode};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Reconcile every task flagged as diverged. A task with an operation in
/// flight (a ledger reservation) waits for the publisher's next pass.
pub(crate) async fn reconcile_diverged_tasks(state: Arc<AppState>) {
    let db_path = state.config().db_path.clone();
    let root = crate::task_store::root_for_db(&db_path);
    if authority::mode_for_root(&root) != Mode::Disk {
        return;
    }
    for task_id in authority::diverged_tasks(&root) {
        let _lease = state.begin_requested_task_mutation(&task_id).await;
        let db_path = db_path.clone();
        let task = task_id.clone();
        let reconciled = tokio::task::spawn_blocking(move || {
            let db = Db::open(&db_path).map_err(|error| format!("open database: {error}"))?;
            if db
                .has_ledger_reservation(&task)
                .map_err(|error| format!("db error: {error}"))?
            {
                return Ok(None);
            }
            authority::reconcile_from_disk(&db, &db_path, Some(&BTreeSet::from([task]))).map(Some)
        })
        .await;
        match reconciled {
            Ok(Ok(Some(report))) => {
                for (task, reasons) in &report.reconciled {
                    log::warn!("task {task} reconciled from disk: {}", reasons.join("; "));
                }
                for (task, error) in &report.failed {
                    log::error!("task {task} could not be reconciled from disk: {error}");
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                log::error!("task {task_id} could not be reconciled from disk: {error}")
            }
            Err(error) => log::error!("disk reconciliation worker for {task_id} failed: {error}"),
        }
    }
}
