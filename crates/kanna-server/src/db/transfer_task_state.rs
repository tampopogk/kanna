//! Task state a transfer carries beyond the task row (spec §11, component T9).
//!
//! A transfer used to carry the task row, its branch, its input ledger and
//! its run history. The structured-workflow components added durable state
//! beside those — T2's branch counter and stage workspaces, T1's exit
//! budgets, T3's commit-step bindings, T4's stage edges and T5's join
//! cohorts — and a task whose state lives there cannot move without it.
//!
//! Two rules decide what moves:
//!
//! - **Task-local rows are rewritten under the destination task id**: the
//!   branch counter, stage budgets and settled commit-step bindings. Nothing
//!   in them names another task, so they are the same facts on either
//!   machine.
//! - **Rows that name another task are carried only when nothing is still
//!   owed across them**, and then as records ([`CarriedTaskLinks`]) kept in
//!   `transferred_task_state.links`: an edge whose other task lives on the
//!   source machine cannot be a foreign-keyed row here, and every machine
//!   acts only on the tasks it owns (spec §11). A task with an open edge or
//!   join is refused ([`Db::transfer_state_blocker`]) rather than moved with
//!   the obligation silently dropped.
//!
//! The same blocker refuses a task whose transition is still pending — an
//! owed ledger continuation, a lifecycle operation across the daemon
//! boundary, a requested commit step, a dependency wait — and while a
//! transfer holds a task's workflow the writers of those states refuse
//! ([`Db::refuse_while_transferring`]), so a source action and a transfer
//! can never both move the task.

use super::Db;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

/// Schema of migration `102_transferred_task_state`.
pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS transferred_task_state (
        pipeline_item_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        transfer_id TEXT NOT NULL,
        source_peer_id TEXT NOT NULL,
        source_task_id TEXT NOT NULL,
        ownership_generation INTEGER NOT NULL,
        state_sha256 TEXT NOT NULL,
        links TEXT NOT NULL,
        session_start TEXT NOT NULL CHECK (session_start IN ('resumed', 'fresh')),
        fresh_start_reason TEXT,
        imported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    );
    CREATE TABLE IF NOT EXISTS transfer_ledger_export (
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        transfer_id TEXT NOT NULL,
        exported_through INTEGER NOT NULL,
        exported_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    );
"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedStageBudget {
    pub stage: String,
    pub spent: i64,
}

/// A settled commit step. `run_id` and `result_id` are the ids it was
/// recorded under where it ran; the destination rewrites `result_id` to the
/// ledger entry that mirrors it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedTransitionCommit {
    pub run_id: String,
    pub stage: String,
    /// The recorded `TransitionExit` JSON, verbatim.
    pub exit: Option<String>,
    pub state: String,
    pub result_id: Option<String>,
    pub committed_sha: Option<String>,
    pub created_at: String,
    pub settled_at: Option<String>,
}

/// A stage workspace as the machine that created it recorded it. Its path
/// names a directory on that machine; the destination holds only the
/// workspace of the stage it starts in (see `transfer_engine::task_state`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedStageWorkspace {
    pub id: String,
    pub stage: String,
    pub path: String,
    pub branch: String,
    pub origin_peer_id: String,
    pub origin_task_id: String,
}

/// A stage edge with nothing left owed across it: consumed, and its other
/// task closed. Task ids are the ones it was recorded under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedStageEdge {
    pub dependent_task_id: String,
    pub dependent_stage: String,
    pub upstream_task_id: String,
    pub upstream_stage: String,
    pub position: i64,
    pub consumed_result_id: Option<String>,
    pub consumed_sha: Option<String>,
    pub consumed_at: Option<String>,
    pub superseded_result_id: Option<String>,
    pub superseded_sha: Option<String>,
    pub superseded_at: Option<String>,
    pub origin_peer_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedJoinMember {
    pub position: i64,
    pub child_task_id: String,
    pub spec: String,
    pub create_error: Option<String>,
    pub resolved_at: Option<String>,
    pub outcome: Option<String>,
    pub result_id: Option<String>,
    pub result_status: Option<String>,
    pub result_stage: Option<String>,
    pub result_sha: Option<String>,
    pub notified_at: Option<String>,
}

/// A join the task created (`role: parent`, completed) or belongs to
/// (`role: member`, its own membership resolved). A member carries only its
/// own row: its siblings are the parent's business.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedJoin {
    pub id: String,
    pub role: String,
    pub parent_task_id: String,
    pub parent_stage: Option<String>,
    pub parent_run_id: Option<String>,
    pub base_sha: String,
    pub base_branch: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub members: Vec<CarriedJoinMember>,
    pub origin_peer_id: String,
}

/// Records a transfer keeps because they name directories or tasks on
/// another machine. Accumulates across hops: a task transferred again
/// re-exports what it inherited beside its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedTaskLinks {
    #[serde(default)]
    pub stage_workspaces: Vec<CarriedStageWorkspace>,
    #[serde(default)]
    pub stage_edges: Vec<CarriedStageEdge>,
    #[serde(default)]
    pub joins: Vec<CarriedJoin>,
}

/// Everything row-shaped a transfer carries for one task.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedTaskRows {
    /// T2's `task_branch_counter.last_allocated`; `None` when never reserved.
    pub branch_counter: Option<i64>,
    #[serde(default)]
    pub stage_budgets: Vec<CarriedStageBudget>,
    #[serde(default)]
    pub transition_commits: Vec<CarriedTransitionCommit>,
    #[serde(default)]
    pub links: CarriedTaskLinks,
    /// How many times this task has changed owner. A task that was never
    /// transferred is generation 0; each import records the source's
    /// generation plus one.
    pub ownership_generation: i64,
}

/// What a destination recorded when it imported a task's carried state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferredTaskState {
    pub task_id: String,
    pub transfer_id: String,
    pub source_peer_id: String,
    pub source_task_id: String,
    pub ownership_generation: i64,
    pub state_sha256: String,
    pub links: CarriedTaskLinks,
    /// `resumed` or `fresh`.
    pub session_start: String,
    pub fresh_start_reason: Option<String>,
}

pub struct NewTransferredTaskState<'a> {
    pub task_id: &'a str,
    pub transfer_id: &'a str,
    pub source_peer_id: &'a str,
    pub source_task_id: &'a str,
    pub ownership_generation: i64,
    pub state_sha256: &'a str,
    pub links: &'a CarriedTaskLinks,
    pub session_start: &'a str,
    pub fresh_start_reason: Option<&'a str>,
}

/// The key a carried run is recorded under at a destination: namespaced to
/// the task that holds it, so it can never be (or collide with) a local run.
pub fn carried_run_id(task_id: &str, origin_run_id: &str) -> String {
    format!("carried:{task_id}:{origin_run_id}")
}

/// The run id a carried key was recorded from; any other id is returned as is.
pub fn origin_run_id<'a>(task_id: &str, run_id: &'a str) -> &'a str {
    run_id
        .strip_prefix("carried:")
        .and_then(|rest| rest.strip_prefix(task_id))
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(run_id)
}

/// Prefix of the error a refused write carries while a transfer holds the
/// task; matched by callers that turn it into a conflict for their client.
pub const TRANSFER_IN_PROGRESS: &str = "task_transfer_in_progress";

impl Db {
    /// Why this task cannot be transferred right now, if it cannot.
    ///
    /// Asked before a push reserves anything and again, authoritatively,
    /// inside the transaction that claims the task's workflow for a
    /// finalizing transfer (`claim_task_workflow_for_transfer_finalization`).
    /// Each refusal names what is owed so the operator can finish it here or
    /// wait for it; none of them is discarded to make a transfer possible.
    pub fn transfer_state_blocker(&self, task_id: &str) -> Result<Option<String>, rusqlite::Error> {
        let one = |sql: &str| -> Result<Option<String>, rusqlite::Error> {
            self.conn
                .query_row(sql, [task_id], |row| row.get::<_, String>(0))
                .optional()
        };
        if let Some(kind) = one("SELECT kind FROM task_ledger_continuation WHERE task_id = ?")? {
            return Ok(Some(format!(
                "task {task_id} owes a stage transition that has not been dispatched yet \
                 ({kind} continuation); transfer it once the transition has happened"
            )));
        }
        if let Some(kind) = one("SELECT kind FROM lifecycle_operation_intent WHERE task_id = ?")? {
            return Ok(Some(format!(
                "task {task_id} has a {kind} operation in flight; transfer it once the operation \
                 has finished"
            )));
        }
        if let Some(stage) =
            one("SELECT stage FROM transition_commit WHERE task_id = ? AND state = 'requested'")?
        {
            return Ok(Some(format!(
                "task {task_id} is committing its transition out of stage {stage}; transfer it \
                 once the commit step has settled"
            )));
        }
        if let Some(stage) = one("SELECT to_stage FROM task_dependency_wait WHERE task_id = ?")? {
            return Ok(Some(format!(
                "task {task_id} is waiting on its dependency edges to enter stage {stage}; a \
                 transfer cannot carry an edge to a task on this machine, so finish the wait here"
            )));
        }
        if let Some(upstream) = one("SELECT edge.upstream_task_id FROM task_stage_edge AS edge
             LEFT JOIN pipeline_item AS upstream ON upstream.id = edge.upstream_task_id
             WHERE edge.dependent_task_id = ?
               AND (edge.consumed_result_id IS NULL
                    OR (upstream.id IS NOT NULL AND upstream.closed_at IS NULL))
             ORDER BY edge.id LIMIT 1")?
        {
            return Ok(Some(format!(
                "task {task_id} depends on task {upstream} through a stage edge that is still \
                 open; a transfer cannot carry an edge to a task on this machine, so finish or \
                 close that dependency here first"
            )));
        }
        if let Some(dependent) = one("SELECT edge.dependent_task_id FROM task_stage_edge AS edge
             JOIN pipeline_item AS dependent ON dependent.id = edge.dependent_task_id
             WHERE edge.upstream_task_id = ? AND dependent.closed_at IS NULL
             ORDER BY edge.id LIMIT 1")?
        {
            return Ok(Some(format!(
                "open task {dependent} depends on task {task_id} through a stage edge; moving \
                 {task_id} would strand that dependency, so finish or close {dependent} first"
            )));
        }
        if let Some(join) = one(
            "SELECT id FROM task_join WHERE parent_task_id = ? AND completed_at IS NULL
             ORDER BY created_at LIMIT 1",
        )? {
            return Ok(Some(format!(
                "task {task_id} is waiting on subtask join {join}; its subtasks live on this \
                 machine, so transfer the task once the join has resolved"
            )));
        }
        if let Some(parent) = one(
            "SELECT join_row.parent_task_id FROM task_join_member AS member
             JOIN task_join AS join_row ON join_row.id = member.join_id
             WHERE member.child_task_id = ? AND member.resolved_at IS NULL",
        )? {
            return Ok(Some(format!(
                "task {task_id} is an unresolved member of a subtask join its parent {parent} is \
                 waiting on; transfer it once it has recorded its result"
            )));
        }
        Ok(None)
    }

    /// Refuse a write that would move `task_id` while a transfer holds its
    /// workflow. Called inside the writer's own statement sequence, so under
    /// SQLite's single writer it and the finalization's claim are mutually
    /// exclusive: either the write lands first and the claim refuses the
    /// transfer, or the claim lands first and the write is refused here.
    pub(crate) fn refuse_while_transferring(&self, task_id: &str) -> Result<(), rusqlite::Error> {
        let claimed = self.task_workflow_is_claimed_by_transfer(task_id)?;
        #[cfg(test)]
        after_transfer_guard::run(task_id);
        match claimed {
            None => Ok(()),
            Some(transfer_id) => Err(rusqlite::Error::InvalidParameterName(format!(
                "{TRANSFER_IN_PROGRESS}: task {task_id} is being transferred to another machine \
                 (transfer {transfer_id}); it cannot change stage here"
            ))),
        }
    }

    /// Fence the task's ledger for the final export of `transfer_id`: from
    /// here on no entry can be appended to it while that transfer holds the
    /// task ([`Db::refuse_ledger_after_transfer_export`]), so the exported
    /// task-state.json is the whole ledger the source ever had. Only the
    /// transfer holding the task's workflow claim may fence it. Returns the
    /// fenced sequence.
    pub fn fence_ledger_for_transfer_export(
        &self,
        task_id: &str,
        transfer_id: &str,
    ) -> Result<Result<i64, String>, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            if db.task_workflow_is_claimed_by_transfer(task_id)?.as_deref() != Some(transfer_id) {
                return Ok(Err(format!(
                    "transfer {transfer_id} does not hold task {task_id}'s workflow, so it cannot \
                     export its final ledger"
                )));
            }
            let through = db.last_ledger_sequence(task_id)?;
            db.conn.execute(
                "INSERT INTO transfer_ledger_export (task_id, transfer_id, exported_through)
                 VALUES (?, ?, ?)
                 ON CONFLICT(task_id) DO UPDATE SET
                    transfer_id = excluded.transfer_id,
                    exported_through = excluded.exported_through,
                    exported_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                rusqlite::params![task_id, transfer_id, through],
            )?;
            Ok(Ok(through))
        })
    }

    /// Refuse a ledger append to a task whose final ledger a transfer that
    /// still holds it has exported: the entry would not reach the
    /// destination, and the source closes once the destination acknowledges
    /// what it received. A fence left by a transfer that no longer holds the
    /// task refuses nothing.
    pub(crate) fn refuse_ledger_after_transfer_export(
        &self,
        task_id: &str,
    ) -> Result<(), rusqlite::Error> {
        let fenced = self
            .conn
            .query_row(
                "SELECT transfer_id, exported_through FROM transfer_ledger_export
                 WHERE task_id = ?",
                [task_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((transfer_id, through)) = fenced else {
            return Ok(());
        };
        if self
            .task_workflow_is_claimed_by_transfer(task_id)?
            .as_deref()
            != Some(&transfer_id)
        {
            return Ok(());
        }
        Err(rusqlite::Error::InvalidParameterName(format!(
            "{TRANSFER_IN_PROGRESS}: task {task_id} is being transferred to another machine \
             (transfer {transfer_id}), whose ledger was exported through entry {through}; \
             nothing more can be recorded here"
        )))
    }

    /// Everything row-shaped the transfer of `task_id` carries, read in one
    /// snapshot. Links it inherited from an earlier transfer are re-exported
    /// beside its own.
    pub fn export_carried_task_rows(
        &self,
        task_id: &str,
        source_peer_id: &str,
    ) -> Result<CarriedTaskRows, rusqlite::Error> {
        self.in_transaction_snapshot(|db| {
            db.export_carried_task_rows_inner(task_id, source_peer_id)
        })
    }

    fn in_transaction_snapshot<T>(
        &self,
        read: impl FnOnce(&Self) -> Result<T, rusqlite::Error>,
    ) -> Result<T, rusqlite::Error> {
        if !self.conn.is_autocommit() {
            return read(self);
        }
        self.conn.execute_batch("BEGIN")?;
        let outcome = read(self);
        let _ = self.conn.execute_batch("COMMIT");
        outcome
    }

    fn export_carried_task_rows_inner(
        &self,
        task_id: &str,
        source_peer_id: &str,
    ) -> Result<CarriedTaskRows, rusqlite::Error> {
        let branch_counter = self
            .conn
            .query_row(
                "SELECT last_allocated FROM task_branch_counter WHERE task_id = ?",
                [task_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let stage_budgets = self
            .conn
            .prepare("SELECT stage, spent FROM task_stage_budget WHERE task_id = ? ORDER BY stage")?
            .query_map([task_id], |row| {
                Ok(CarriedStageBudget {
                    stage: row.get(0)?,
                    spent: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let transition_commits = self
            .conn
            .prepare(
                "SELECT run_id, stage, exit, state, result_id, committed_sha, created_at, settled_at
                 FROM transition_commit WHERE task_id = ? ORDER BY created_at, run_id",
            )?
            .query_map([task_id], |row| {
                let run_id: String = row.get(0)?;
                Ok(CarriedTransitionCommit {
                    // The id the run had where it ran, not this machine's key.
                    run_id: origin_run_id(task_id, &run_id).to_string(),
                    stage: row.get(1)?,
                    exit: row.get(2)?,
                    state: row.get(3)?,
                    result_id: row.get(4)?,
                    committed_sha: row.get(5)?,
                    created_at: row.get(6)?,
                    settled_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let inherited = self.transferred_task_state(task_id)?;
        let mut links = inherited
            .as_ref()
            .map(|state| state.links.clone())
            .unwrap_or_default();
        for workspace in self.list_stage_workspaces(task_id)? {
            links.stage_workspaces.push(CarriedStageWorkspace {
                id: workspace.id,
                stage: workspace.stage,
                path: workspace.path,
                branch: workspace.branch,
                origin_peer_id: source_peer_id.to_string(),
                origin_task_id: task_id.to_string(),
            });
        }
        let edges = self
            .conn
            .prepare(
                "SELECT dependent_task_id, dependent_stage, upstream_task_id, upstream_stage,
                        position, consumed_result_id, consumed_sha, consumed_at,
                        superseded_result_id, superseded_sha, superseded_at
                 FROM task_stage_edge
                 WHERE dependent_task_id = ?1 OR upstream_task_id = ?1
                 ORDER BY id",
            )?
            .query_map([task_id], |row| {
                Ok(CarriedStageEdge {
                    dependent_task_id: row.get(0)?,
                    dependent_stage: row.get(1)?,
                    upstream_task_id: row.get(2)?,
                    upstream_stage: row.get(3)?,
                    position: row.get(4)?,
                    consumed_result_id: row.get(5)?,
                    consumed_sha: row.get(6)?,
                    consumed_at: row.get(7)?,
                    superseded_result_id: row.get(8)?,
                    superseded_sha: row.get(9)?,
                    superseded_at: row.get(10)?,
                    origin_peer_id: source_peer_id.to_string(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        links.stage_edges.extend(edges);

        let join_row = |row: &rusqlite::Row<'_>, role: &str| -> rusqlite::Result<CarriedJoin> {
            Ok(CarriedJoin {
                id: row.get(0)?,
                role: role.to_string(),
                parent_task_id: row.get(1)?,
                parent_stage: row.get(2)?,
                parent_run_id: row.get(3)?,
                base_sha: row.get(4)?,
                base_branch: row.get(5)?,
                created_at: row.get(6)?,
                completed_at: row.get(7)?,
                members: Vec::new(),
                origin_peer_id: source_peer_id.to_string(),
            })
        };
        const JOIN_COLUMNS: &str = "join_row.id, join_row.parent_task_id, join_row.parent_stage,
             join_row.parent_run_id, join_row.base_sha, join_row.base_branch,
             join_row.created_at, join_row.completed_at";
        let mut joins = self
            .conn
            .prepare(&format!(
                "SELECT {JOIN_COLUMNS} FROM task_join AS join_row
                 WHERE join_row.parent_task_id = ? ORDER BY join_row.created_at, join_row.id"
            ))?
            .query_map([task_id], |row| join_row(row, "parent"))?
            .collect::<Result<Vec<_>, _>>()?;
        for join in &mut joins {
            join.members = self.carried_join_members(&join.id, None)?;
        }
        let mut memberships = self
            .conn
            .prepare(&format!(
                "SELECT {JOIN_COLUMNS} FROM task_join AS join_row
                 JOIN task_join_member AS member ON member.join_id = join_row.id
                 WHERE member.child_task_id = ?"
            ))?
            .query_map([task_id], |row| join_row(row, "member"))?
            .collect::<Result<Vec<_>, _>>()?;
        for join in &mut memberships {
            join.members = self.carried_join_members(&join.id, Some(task_id))?;
        }
        links.joins.extend(joins);
        links.joins.extend(memberships);

        Ok(CarriedTaskRows {
            branch_counter,
            stage_budgets,
            transition_commits,
            links,
            ownership_generation: inherited.map_or(0, |state| state.ownership_generation),
        })
    }

    fn carried_join_members(
        &self,
        join_id: &str,
        only_child: Option<&str>,
    ) -> Result<Vec<CarriedJoinMember>, rusqlite::Error> {
        self.conn
            .prepare(
                "SELECT position, child_task_id, spec, create_error, resolved_at, outcome,
                        result_id, result_status, result_stage, result_sha, notified_at
                 FROM task_join_member
                 WHERE join_id = ?1 AND (?2 IS NULL OR child_task_id = ?2)
                 ORDER BY position",
            )?
            .query_map(rusqlite::params![join_id, only_child], |row| {
                Ok(CarriedJoinMember {
                    position: row.get(0)?,
                    child_task_id: row.get(1)?,
                    spec: row.get(2)?,
                    create_error: row.get(3)?,
                    resolved_at: row.get(4)?,
                    outcome: row.get(5)?,
                    result_id: row.get(6)?,
                    result_status: row.get(7)?,
                    result_stage: row.get(8)?,
                    result_sha: row.get(9)?,
                    notified_at: row.get(10)?,
                })
            })?
            .collect()
    }

    /// Write the carried rows under the destination task id and record the
    /// import, in one transaction. Idempotent: a retried import rewrites the
    /// same values, and a second import of different state for the same task
    /// is refused. `result_ids` maps each carried result id to the entry that
    /// mirrors it here.
    pub fn import_carried_task_rows(
        &self,
        state: &NewTransferredTaskState<'_>,
        rows: &CarriedTaskRows,
        result_ids: &std::collections::HashMap<String, String>,
    ) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            if let Some(existing) = db.transferred_task_state(state.task_id)? {
                if existing.transfer_id != state.transfer_id
                    || existing.state_sha256 != state.state_sha256
                {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "task {} already imported different transferred state (transfer {})",
                        state.task_id, existing.transfer_id
                    )));
                }
            }
            if let Some(counter) = rows.branch_counter {
                db.conn.execute(
                    "INSERT INTO task_branch_counter (task_id, last_allocated) VALUES (?, ?)
                     ON CONFLICT(task_id) DO UPDATE SET
                        last_allocated = MAX(last_allocated, excluded.last_allocated),
                        updated_at = datetime('now')",
                    rusqlite::params![state.task_id, counter],
                )?;
            }
            for budget in &rows.stage_budgets {
                db.conn.execute(
                    "INSERT INTO task_stage_budget (task_id, stage, spent) VALUES (?, ?, ?)
                     ON CONFLICT(task_id, stage) DO UPDATE SET spent = excluded.spent",
                    rusqlite::params![state.task_id, budget.stage, budget.spent],
                )?;
            }
            for commit in &rows.transition_commits {
                if commit.state == "requested" {
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "transferred commit step {} is still requested",
                        commit.run_id
                    )));
                }
                let result_id = commit
                    .result_id
                    .as_ref()
                    .map(|id| result_ids.get(id).cloned().unwrap_or_else(|| id.clone()));
                // Keyed to this task, never under the bare origin run id: that
                // id is the table's global key, and a task returning to a
                // machine that still holds its closed original (A -> B -> A),
                // or an unrelated local run with the same id, already owns it.
                let run_id = carried_run_id(state.task_id, &commit.run_id);
                let existing = db
                    .conn
                    .query_row(
                        "SELECT task_id, state, result_id FROM transition_commit
                         WHERE run_id = ?",
                        [&run_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                            ))
                        },
                    )
                    .optional()?;
                if let Some((owner, recorded_state, recorded_result)) = existing {
                    if owner == state.task_id
                        && recorded_state == commit.state
                        && recorded_result == result_id
                    {
                        continue;
                    }
                    return Err(rusqlite::Error::InvalidParameterName(format!(
                        "transferred commit step {run_id} collides with a binding task {owner} \
                         already holds; nothing was imported"
                    )));
                }
                db.conn.execute(
                    "INSERT INTO transition_commit
                     (run_id, task_id, stage, exit, state, result_id, committed_sha,
                      created_at, settled_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        run_id,
                        state.task_id,
                        commit.stage,
                        commit.exit,
                        commit.state,
                        result_id,
                        commit.committed_sha,
                        commit.created_at,
                        commit.settled_at,
                    ],
                )?;
            }
            let links = serde_json::to_string(state.links)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            db.conn.execute(
                "INSERT INTO transferred_task_state
                 (pipeline_item_id, transfer_id, source_peer_id, source_task_id,
                  ownership_generation, state_sha256, links, session_start, fresh_start_reason)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(pipeline_item_id) DO NOTHING",
                rusqlite::params![
                    state.task_id,
                    state.transfer_id,
                    state.source_peer_id,
                    state.source_task_id,
                    state.ownership_generation,
                    state.state_sha256,
                    links,
                    state.session_start,
                    state.fresh_start_reason,
                ],
            )?;
            Ok(())
        })
    }

    /// `(sequence, file name)` of every entry of the task already on disk,
    /// in order.
    pub fn published_ledger_files(
        &self,
        task_id: &str,
    ) -> Result<Vec<(i64, String)>, rusqlite::Error> {
        self.conn
            .prepare(
                "SELECT sequence, file_name FROM task_ledger_entry
                 WHERE task_id = ? AND kind IS NOT NULL AND published_at IS NOT NULL
                 ORDER BY sequence",
            )?
            .query_map([task_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    /// The task's highest ledger sequence ever allocated (T13's high-water
    /// mark, which a released reservation leaves above every row),
    /// reservations included; 0 for none. The next allocation is this + 1.
    pub fn last_ledger_sequence(&self, task_id: &str) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT MAX(
                 COALESCE((SELECT MAX(sequence) FROM task_ledger_entry WHERE task_id = ?1), 0),
                 COALESCE((SELECT high_water FROM task_ledger_sequence WHERE task_id = ?1), 0)
             )",
            [task_id],
            |row| row.get(0),
        )
    }

    /// Entries of the task recorded but not yet on disk (reservations
    /// included).
    pub fn unpublished_ledger_entry_count(&self, task_id: &str) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM task_ledger_entry WHERE task_id = ? AND published_at IS NULL",
            [task_id],
            |row| row.get(0),
        )
    }

    pub fn transferred_task_state(
        &self,
        task_id: &str,
    ) -> Result<Option<TransferredTaskState>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT pipeline_item_id, transfer_id, source_peer_id, source_task_id,
                        ownership_generation, state_sha256, links, session_start,
                        fresh_start_reason
                 FROM transferred_task_state WHERE pipeline_item_id = ?",
                [task_id],
                |row| {
                    let links: String = row.get(6)?;
                    Ok(TransferredTaskState {
                        task_id: row.get(0)?,
                        transfer_id: row.get(1)?,
                        source_peer_id: row.get(2)?,
                        source_task_id: row.get(3)?,
                        ownership_generation: row.get(4)?,
                        state_sha256: row.get(5)?,
                        links: serde_json::from_str(&links).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                6,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                        session_start: row.get(7)?,
                        fresh_start_reason: row.get(8)?,
                    })
                },
            )
            .optional()
    }
}

/// A test hook run between the transfer guard's read and the caller's write,
/// to prove no claim can commit in that gap.
#[cfg(test)]
pub(crate) mod after_transfer_guard {
    use std::sync::Mutex;

    type Hook = Box<dyn FnOnce() + Send>;
    static HOOKS: Mutex<Vec<(String, Hook)>> = Mutex::new(Vec::new());

    /// Run `hook` once, the next time the guard is read for `task_id`.
    pub(crate) fn set(task_id: &str, hook: impl FnOnce() + Send + 'static) {
        HOOKS
            .lock()
            .unwrap()
            .push((task_id.to_string(), Box::new(hook)));
    }

    pub(super) fn run(task_id: &str) {
        let hook = {
            let mut hooks = HOOKS.lock().unwrap();
            hooks
                .iter()
                .position(|(id, _)| id == task_id)
                .map(|index| hooks.remove(index).1)
        };
        if let Some(hook) = hook {
            hook();
        }
    }
}

#[cfg(test)]
impl Db {
    /// Seed rows the transfer tests need in states only a long sequence of
    /// real operations would otherwise reach (a consumed edge, a resolved
    /// join, a settled commit step).
    pub(crate) fn execute_test_sql(&self, sql: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute_batch(sql)
    }

    pub(crate) fn query_test_i64(&self, sql: &str) -> i64 {
        self.conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    /// Make this connection fail at once on a held lock instead of waiting,
    /// so a test can observe that another connection holds the write lock.
    pub(crate) fn set_test_busy_timeout(&self, timeout: std::time::Duration) {
        self.conn.busy_timeout(timeout).unwrap();
    }
}

#[cfg(test)]
#[path = "transfer_task_state_tests.rs"]
mod tests;
