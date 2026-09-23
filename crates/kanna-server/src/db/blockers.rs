//! Legacy task-level blockers (`task_blocker`). Kept as an explicit adapter
//! during the migration to stage dependency edges (T4, `stage_edges.rs`):
//! existing tasks keep exactly the readiness, dormant start and branch
//! inheritance they had. New dependencies are stage edges; the two share one
//! cycle check and one derived blocked state.

use super::{pipeline_items::update_open_pipeline_item_activity, Db, TaskEventKind};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::json;
use std::fmt;

#[derive(Debug)]
pub enum ReplaceTaskBlockersError {
    Database(rusqlite::Error),
    TaskNotFound(String),
    BlockerNotFound(String),
    SelfDependency,
    CircularDependency,
}

impl fmt::Display for ReplaceTaskBlockersError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "{error}"),
            Self::TaskNotFound(task_id) => write!(formatter, "task not found: {task_id}"),
            Self::BlockerNotFound(task_id) => write!(formatter, "task not found: {task_id}"),
            Self::SelfDependency => write!(formatter, "task cannot block itself"),
            Self::CircularDependency => {
                write!(
                    formatter,
                    "cannot add blocker because it would create a circular dependency"
                )
            }
        }
    }
}

impl std::error::Error for ReplaceTaskBlockersError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for ReplaceTaskBlockersError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

impl Db {
    pub fn insert_task_blocker(
        &self,
        blocked_item_id: &str,
        blocker_item_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            db.conn.execute(
                "INSERT OR IGNORE INTO task_blocker (blocked_item_id, blocker_item_id) VALUES (?, ?)",
                (blocked_item_id, blocker_item_id),
            )?;
            db.sync_blocked_event(blocked_item_id)
        })
    }

    pub fn remove_all_task_blockers(&self, blocked_item_id: &str) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            db.conn.execute(
                "DELETE FROM task_blocker WHERE blocked_item_id = ?",
                [blocked_item_id],
            )?;
            db.sync_blocked_event(blocked_item_id)
        })
    }

    /// Publish the derived blocked state of `task_id` if it moved since the
    /// last time managers were told, inside the caller's transaction.
    ///
    /// Blocked is not a stored flag: it is `count_open_task_blockers > 0`, so
    /// it flips both when the task's own `task_blocker` rows are rewritten and
    /// when a blocker resolves underneath it. `blocked_event_baseline` is what
    /// makes an *edge* out of that predicate; without it the only alternative
    /// is a watcher diffing snapshots, which is exactly what the event feed
    /// exists to replace. Closed tasks are skipped — nothing depends on the
    /// blocked state of work that is over.
    pub(crate) fn sync_blocked_event(&self, task_id: &str) -> Result<(), rusqlite::Error> {
        // Every rewrite of a task's own blocker set passes through here, and
        // its dependency links are part of `task.json`.
        self.mark_task_snapshot_dirty(task_id)?;
        let baseline: Option<i64> = self
            .conn
            .query_row(
                "SELECT blocked_event_baseline FROM pipeline_item
                 WHERE id = ? AND closed_at IS NULL",
                [task_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(baseline) = baseline else {
            return Ok(());
        };
        let blocker_task_ids = self.list_blocking_task_ids(task_id)?;
        let blocked = !blocker_task_ids.is_empty();
        if baseline == i64::from(blocked) {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE pipeline_item SET blocked_event_baseline = ? WHERE id = ?",
            (i64::from(blocked), task_id),
        )?;
        self.append_task_event(
            task_id,
            if blocked {
                TaskEventKind::TaskBlocked
            } else {
                TaskEventKind::TaskUnblocked
            },
            json!({
                "blocked": blocked,
                "blockerTaskIds": blocker_task_ids,
            }),
        )
    }

    /// Republish the blocked state of every task that depends on
    /// `blocker_item_id`, after a write that can change whether this blocker
    /// counts as resolved. The write sites are exactly the ones the
    /// `task_blocker_resolution_revision` trigger watches — `closed_at`,
    /// `stage`, and `pr_url` — so the two stay in step by construction.
    pub(crate) fn sync_blocked_events_for_dependents(
        &self,
        blocker_item_id: &str,
    ) -> Result<(), rusqlite::Error> {
        let mut dependents = self.list_tasks_blocked_by(blocker_item_id)?;
        for dependent in self.list_stage_edge_dependents(blocker_item_id)? {
            if !dependents.contains(&dependent) {
                dependents.push(dependent);
            }
        }
        for blocked_item_id in dependents {
            self.sync_blocked_event(&blocked_item_id)?;
        }
        Ok(())
    }

    /// Resolve, validate, and replace an existing task's full blocker set in
    /// one serialized transaction. `BEGIN IMMEDIATE` makes cycle checks
    /// authoritative against every blocker writer that committed first.
    pub fn replace_task_blockers_atomically(
        &self,
        task_or_branch_id: &str,
        blocker_task_ids: &[String],
    ) -> Result<String, ReplaceTaskBlockersError> {
        self.replace_task_blockers_atomically_impl(task_or_branch_id, blocker_task_ids, || {})
    }

    #[cfg(test)]
    pub(crate) fn replace_task_blockers_atomically_with_hook(
        &self,
        task_or_branch_id: &str,
        blocker_task_ids: &[String],
        after_delete: impl FnOnce(),
    ) -> Result<String, ReplaceTaskBlockersError> {
        self.replace_task_blockers_atomically_impl(
            task_or_branch_id,
            blocker_task_ids,
            after_delete,
        )
    }

    fn replace_task_blockers_atomically_impl(
        &self,
        task_or_branch_id: &str,
        blocker_task_ids: &[String],
        after_delete: impl FnOnce(),
    ) -> Result<String, ReplaceTaskBlockersError> {
        let transaction = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let task_id = resolve_pipeline_item_id(&transaction, task_or_branch_id)?
            .ok_or_else(|| ReplaceTaskBlockersError::TaskNotFound(task_or_branch_id.to_string()))?;

        let mut resolved_blocker_ids = Vec::new();
        for blocker_task_id in blocker_task_ids {
            let blocker_id =
                resolve_pipeline_item_id(&transaction, blocker_task_id)?.ok_or_else(|| {
                    ReplaceTaskBlockersError::BlockerNotFound(blocker_task_id.to_string())
                })?;
            if blocker_id == task_id {
                return Err(ReplaceTaskBlockersError::SelfDependency);
            }
            if !resolved_blocker_ids.contains(&blocker_id) {
                resolved_blocker_ids.push(blocker_id);
            }
        }
        // The legacy rows share one dependency graph with stage edges (T4):
        // a blocker cycle through any stage path is refused as before.
        for blocker_id in &resolved_blocker_ids {
            if super::stage_edges::legacy_blocker_would_cycle(self, &task_id, blocker_id)? {
                return Err(ReplaceTaskBlockersError::CircularDependency);
            }
        }

        transaction.execute(
            "DELETE FROM task_blocker WHERE blocked_item_id = ?",
            [&task_id],
        )?;
        after_delete();
        for blocker_id in &resolved_blocker_ids {
            transaction.execute(
                "INSERT OR IGNORE INTO task_blocker (blocked_item_id, blocker_item_id)
                 VALUES (?, ?)",
                (&task_id, blocker_id),
            )?;
        }
        update_open_pipeline_item_activity(&transaction, &task_id, "idle", None, None)?;
        self.sync_blocked_event(&task_id)?;
        transaction.commit()?;
        Ok(task_id)
    }

    pub fn list_task_blocker_ids(
        &self,
        blocked_item_id: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT blocker_item_id FROM task_blocker WHERE blocked_item_id = ? ORDER BY blocker_item_id",
            )?;
        let rows = stmt.query_map([blocked_item_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Count blockers that are still unresolved. A blocker resolves
    /// optimistically: either it closed, or it is parked at the `pr` stage
    /// with a PR created (work committed, reviewed, rebased, renamed, and
    /// pushed — stable enough for dependents to stack on without waiting
    /// for the human review/merge loop). Dependents started at that point
    /// inherit same-repo blocker branches before falling back to their
    /// normal base. Keep this predicate in sync with `isBlockerResolved`
    /// in packages/db/src/queries.ts.
    pub fn count_open_task_blockers(&self, blocked_item_id: &str) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*)
             FROM task_blocker blocker
             JOIN pipeline_item blocker_item ON blocker_item.id = blocker.blocker_item_id
             WHERE blocker.blocked_item_id = ?
               AND blocker_item.closed_at IS NULL
               AND NOT (blocker_item.stage = 'pr' AND blocker_item.pr_url IS NOT NULL)",
            [blocked_item_id],
            |row| row.get(0),
        )
    }

    /// Every task holding `task_id` right now: its unresolved legacy
    /// blockers, then the upstreams of stage edges (T4) holding it at the
    /// stage it is waiting to enter, then the children of its subtask joins
    /// (T5) that have not resolved. The one set behind `task.blocked` and
    /// every `blockedByTaskIds` a client reads.
    pub fn list_blocking_task_ids(&self, task_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        let mut blocker_task_ids = self.list_open_task_blocker_ids(task_id)?;
        for upstream in self
            .list_waiting_stage_edge_upstreams(task_id)?
            .into_iter()
            .chain(self.unresolved_join_children(task_id)?)
        {
            if !blocker_task_ids.contains(&upstream) {
                blocker_task_ids.push(upstream);
            }
        }
        Ok(blocker_task_ids)
    }

    /// Ids of blockers that are still unresolved, for surfacing why a task
    /// is blocked. Keep the resolution predicate in sync with
    /// `count_open_task_blockers` above.
    pub fn list_open_task_blocker_ids(
        &self,
        blocked_item_id: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT blocker.blocker_item_id
             FROM task_blocker blocker
             JOIN pipeline_item blocker_item ON blocker_item.id = blocker.blocker_item_id
             WHERE blocker.blocked_item_id = ?
               AND blocker_item.closed_at IS NULL
               AND NOT (blocker_item.stage = 'pr' AND blocker_item.pr_url IS NOT NULL)
             ORDER BY blocker.blocker_item_id",
        )?;
        let rows = stmt.query_map([blocked_item_id], |row| row.get(0))?;
        rows.collect()
    }

    pub fn list_tasks_blocked_by(
        &self,
        blocker_item_id: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT blocked_item_id FROM task_blocker WHERE blocker_item_id = ? ORDER BY blocked_item_id",
        )?;
        let rows = stmt.query_map([blocker_item_id], |row| row.get(0))?;
        rows.collect()
    }
}

fn resolve_pipeline_item_id(
    connection: &Connection,
    task_or_branch_id: &str,
) -> Result<Option<String>, rusqlite::Error> {
    let exact = connection
        .query_row(
            "SELECT id FROM pipeline_item WHERE id = ?",
            [task_or_branch_id],
            |row| row.get(0),
        )
        .optional()?;
    if exact.is_some() {
        return Ok(exact);
    }
    connection
        .query_row(
            "SELECT id FROM pipeline_item WHERE branch = ?",
            [task_or_branch_id],
            |row| row.get(0),
        )
        .optional()
}
