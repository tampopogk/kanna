//! Stage dependency edges (spec §9, §16.4 — component T4).
//!
//! An edge `(upstream task A, stage X) → (dependent task B, stage Y)` says B
//! may not enter Y until A has left X with a `success` result — for A's
//! final stage, until A is closed. Satisfaction is never stored: it is read
//! from A's ledger entries (T0) each time, so it cannot drift from what A
//! actually did. What the edge stores is what B *consumed* (the result id
//! and the SHA it recorded, written when B entered Y) and, afterwards, the
//! newest upstream success that superseded it. A supersession is recorded
//! and announced; the engine never holds, reruns or rebases B because of it.
//!
//! Edges into the stage a task starts in are its *base* edges. The first one
//! (by position) gives the fork point of the task's first workspace — the SHA
//! the upstream result recorded, never the upstream's current branch tip —
//! and the rest are listed to the session to merge itself; the engine merges
//! nothing. Edges into later stages only gate readiness.
//!
//! The pre-T4 `task_blocker` rows are kept as a legacy adapter (see
//! `blockers.rs`): the cycle check here reads them as edges from the
//! blocker's final stage into the blocked task's first stage.

use super::{Db, TaskEventKind};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt;

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS task_stage_edge (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        dependent_task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        dependent_stage TEXT NOT NULL,
        upstream_task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        upstream_stage TEXT NOT NULL,
        position INTEGER NOT NULL,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        consumed_result_id TEXT,
        consumed_sha TEXT,
        consumed_at TEXT,
        superseded_result_id TEXT,
        superseded_sha TEXT,
        superseded_at TEXT,
        UNIQUE (dependent_task_id, dependent_stage, upstream_task_id, upstream_stage)
    );
    CREATE INDEX IF NOT EXISTS idx_task_stage_edge_upstream
        ON task_stage_edge(upstream_task_id, upstream_stage);
    CREATE TABLE IF NOT EXISTS task_dependency_wait (
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        from_stage TEXT NOT NULL,
        to_stage TEXT NOT NULL,
        generation INTEGER NOT NULL,
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
    );
"#;

/// One stored edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageEdge {
    pub id: i64,
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
}

/// An edge a caller asks for. The upstream may be named by id or branch;
/// `dependent_stage` defaults to the stage the dependent starts in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewStageEdge {
    pub upstream_task_id: String,
    pub upstream_stage: String,
    pub dependent_stage: Option<String>,
}

/// The upstream result an edge is satisfied by: its ledger entry id and the
/// commit it recorded (null when the run had no workspace, or for a closed
/// final stage that never recorded a success).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageEdgeInput {
    pub result_id: Option<String>,
    pub committed_sha: Option<String>,
}

/// How a satisfied edge reaches the session of the stage it gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyRole {
    /// The first edge into the starting stage: its SHA is the fork point.
    Base,
    /// A further edge into the starting stage: listed for the session to
    /// merge itself.
    Merge,
    /// An edge into a later stage: it only gated readiness.
    Gate,
}

impl DependencyRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Merge => "merge",
            Self::Gate => "gate",
        }
    }
}

/// A satisfied edge as a stage consumes it, in edge order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumedDependency {
    pub edge: StageEdge,
    pub role: DependencyRole,
    pub input: StageEdgeInput,
}

impl ConsumedDependency {
    pub fn to_ledger_json(&self) -> Value {
        json!({
            "upstream_task_id": self.edge.upstream_task_id,
            "upstream_stage": self.edge.upstream_stage,
            "dependent_stage": self.edge.dependent_stage,
            "position": self.edge.position,
            "role": self.role.as_str(),
            "result_id": self.input.result_id,
            "committed_sha": self.input.committed_sha,
        })
    }

    pub fn to_session_input(&self) -> crate::task_store::DependencyInput {
        crate::task_store::DependencyInput {
            upstream_task_id: self.edge.upstream_task_id.clone(),
            upstream_stage: self.edge.upstream_stage.clone(),
            role: self.role.as_str().to_string(),
            result_id: self.input.result_id.clone(),
            committed_sha: self.input.committed_sha.clone(),
        }
    }
}

/// A completion whose automatic advance is waiting on edges into the next
/// stage. Fenced like a ledger continuation: it is stale once the task is no
/// longer at `from_stage` or a later lifecycle operation started a run.
#[derive(Debug, Clone, PartialEq)]
pub struct DependencyWait {
    pub task_id: String,
    pub from_stage: String,
    pub to_stage: String,
    pub generation: i64,
    pub payload: Value,
}

#[derive(Debug)]
pub enum StageEdgeError {
    Database(rusqlite::Error),
    TaskNotFound(String),
    UpstreamNotFound(String),
    StageNotFound { task_id: String, stage: String },
    SelfDependency,
    CircularDependency,
}

impl fmt::Display for StageEdgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "{error}"),
            Self::TaskNotFound(task_id) => write!(formatter, "task not found: {task_id}"),
            Self::UpstreamNotFound(task_id) => {
                write!(formatter, "dependency task not found: {task_id}")
            }
            Self::StageNotFound { task_id, stage } => {
                write!(
                    formatter,
                    "stage not found in the workflow of {task_id}: {stage}"
                )
            }
            Self::SelfDependency => write!(formatter, "task cannot depend on itself"),
            Self::CircularDependency => write!(
                formatter,
                "cannot add dependency because it would create a circular dependency"
            ),
        }
    }
}

impl std::error::Error for StageEdgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for StageEdgeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error)
    }
}

const EDGE_COLUMNS: &str = "id, dependent_task_id, dependent_stage, upstream_task_id, \
    upstream_stage, position, consumed_result_id, consumed_sha, consumed_at, \
    superseded_result_id, superseded_sha, superseded_at";

fn edge_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StageEdge> {
    Ok(StageEdge {
        id: row.get(0)?,
        dependent_task_id: row.get(1)?,
        dependent_stage: row.get(2)?,
        upstream_task_id: row.get(3)?,
        upstream_stage: row.get(4)?,
        position: row.get(5)?,
        consumed_result_id: row.get(6)?,
        consumed_sha: row.get(7)?,
        consumed_at: row.get(8)?,
        superseded_result_id: row.get(9)?,
        superseded_sha: row.get(10)?,
        superseded_at: row.get(11)?,
    })
}

/// Whether an edge is satisfied now, and by which upstream result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeSatisfaction {
    Pending,
    Satisfied(StageEdgeInput),
}

impl Db {
    /// Install edges into `dependent_task_id`, validated and cycle-checked in
    /// one immediate transaction (joining the caller's when there is one, so
    /// task creation installs them atomically with the task row). Edge order
    /// is kept: positions continue after the task's existing edges. A
    /// duplicate of an existing edge is ignored.
    pub fn insert_stage_edges(
        &self,
        dependent_task_id: &str,
        edges: &[NewStageEdge],
    ) -> Result<Vec<StageEdge>, StageEdgeError> {
        self.in_immediate_transaction_if_needed(|db| {
            let dependent = db
                .get_pipeline_item(dependent_task_id)?
                .ok_or_else(|| StageEdgeError::TaskNotFound(dependent_task_id.to_string()))?;
            let dependent_stages = pinned_stage_names(dependent.pipeline_def.as_deref());
            let starting_stage = dependent.stage.clone().unwrap_or_default();
            for edge in edges {
                let upstream_id = db
                    .resolve_pipeline_item_id(&edge.upstream_task_id)?
                    .ok_or_else(|| StageEdgeError::UpstreamNotFound(edge.upstream_task_id.clone()))?;
                if upstream_id == dependent.id {
                    return Err(StageEdgeError::SelfDependency);
                }
                let upstream = db
                    .get_pipeline_item(&upstream_id)?
                    .ok_or_else(|| StageEdgeError::UpstreamNotFound(upstream_id.clone()))?;
                if !pinned_stage_names(upstream.pipeline_def.as_deref())
                    .contains(&edge.upstream_stage)
                {
                    return Err(StageEdgeError::StageNotFound {
                        task_id: upstream_id,
                        stage: edge.upstream_stage.clone(),
                    });
                }
                let dependent_stage = edge
                    .dependent_stage
                    .clone()
                    .unwrap_or_else(|| starting_stage.clone());
                if !dependent_stages.contains(&dependent_stage) {
                    return Err(StageEdgeError::StageNotFound {
                        task_id: dependent.id.clone(),
                        stage: dependent_stage,
                    });
                }
                let position: i64 = db.conn.query_row(
                    "SELECT COALESCE(MAX(position), 0) + 1 FROM task_stage_edge
                     WHERE dependent_task_id = ?",
                    [&dependent.id],
                    |row| row.get(0),
                )?;
                db.conn.execute(
                    "INSERT OR IGNORE INTO task_stage_edge
                     (dependent_task_id, dependent_stage, upstream_task_id, upstream_stage, position)
                     VALUES (?, ?, ?, ?, ?)",
                    params![
                        dependent.id,
                        dependent_stage,
                        upstream_id,
                        edge.upstream_stage,
                        position
                    ],
                )?;
                // Checked with the edge in place, against every edge and
                // legacy blocker committed before this transaction began.
                if dependency_graph_has_cycle_through(
                    db,
                    &upstream_id,
                    &edge.upstream_stage,
                    &dependent.id,
                    &dependent_stage,
                )? {
                    return Err(StageEdgeError::CircularDependency);
                }
            }
            db.mark_task_snapshot_dirty(&dependent.id)?;
            db.sync_blocked_event(&dependent.id)?;
            Ok(db.list_stage_edges_into(&dependent.id)?)
        })
    }

    /// Every edge into `dependent_task_id`, in edge order.
    pub fn list_stage_edges_into(
        &self,
        dependent_task_id: &str,
    ) -> Result<Vec<StageEdge>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EDGE_COLUMNS} FROM task_stage_edge
             WHERE dependent_task_id = ? ORDER BY position, id"
        ))?;
        let rows = stmt.query_map([dependent_task_id], edge_from_row)?;
        rows.collect()
    }

    /// Every edge out of `upstream_task_id`, optionally only out of one stage.
    pub fn list_stage_edges_from(
        &self,
        upstream_task_id: &str,
        upstream_stage: Option<&str>,
    ) -> Result<Vec<StageEdge>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EDGE_COLUMNS} FROM task_stage_edge
             WHERE upstream_task_id = ?1 AND (?2 IS NULL OR upstream_stage = ?2)
             ORDER BY dependent_task_id, position, id"
        ))?;
        let rows = stmt.query_map(params![upstream_task_id, upstream_stage], edge_from_row)?;
        rows.collect()
    }

    /// Open tasks that depend on some stage of `upstream_task_id`.
    pub fn list_stage_edge_dependents(
        &self,
        upstream_task_id: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT edge.dependent_task_id
             FROM task_stage_edge edge
             JOIN pipeline_item item ON item.id = edge.dependent_task_id
             WHERE edge.upstream_task_id = ? AND item.closed_at IS NULL
             ORDER BY edge.dependent_task_id",
        )?;
        let rows = stmt.query_map([upstream_task_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Open tasks that could be waiting on an edge: those with edges into
    /// them, and those holding a dependency wait. For the startup sweep.
    pub fn list_open_stage_edge_dependents(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id FROM (
                 SELECT dependent_task_id AS task_id FROM task_stage_edge
                 UNION SELECT task_id FROM task_dependency_wait
             ) waiting
             JOIN pipeline_item item ON item.id = waiting.task_id
             WHERE item.closed_at IS NULL
             ORDER BY task_id",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Is the edge satisfied, and by which upstream result?
    ///
    /// Non-final stage: the newest transition out of `upstream_stage` to a
    /// later stage whose triggering result has status `success`. Loop exits
    /// back to earlier stages and reruns are not departures. Final stage:
    /// the upstream task is closed; the input is its newest success result
    /// recorded at that stage, if any.
    pub fn stage_edge_satisfaction(
        &self,
        edge: &StageEdge,
    ) -> Result<EdgeSatisfaction, rusqlite::Error> {
        let Some(upstream) = self.get_pipeline_item(&edge.upstream_task_id)? else {
            return Ok(EdgeSatisfaction::Pending);
        };
        let stages = pinned_stage_names(upstream.pipeline_def.as_deref());
        if stages.last() == Some(&edge.upstream_stage) {
            if upstream.closed_at.is_none() {
                return Ok(EdgeSatisfaction::Pending);
            }
            let result =
                self.latest_success_result_at_stage(&edge.upstream_task_id, &edge.upstream_stage)?;
            return Ok(EdgeSatisfaction::Satisfied(result.unwrap_or(
                StageEdgeInput {
                    result_id: None,
                    committed_sha: None,
                },
            )));
        }
        Ok(
            match self.latest_success_departure(
                &edge.upstream_task_id,
                &edge.upstream_stage,
                &stages,
            )? {
                Some(input) => EdgeSatisfaction::Satisfied(input),
                None => EdgeSatisfaction::Pending,
            },
        )
    }

    /// Edges into `stage` of `task_id` that are not satisfied yet.
    pub fn unsatisfied_stage_edges_into(
        &self,
        task_id: &str,
        stage: &str,
    ) -> Result<Vec<StageEdge>, rusqlite::Error> {
        let mut pending = Vec::new();
        for edge in self.list_stage_edges_into(task_id)? {
            if edge.dependent_stage == stage
                && self.stage_edge_satisfaction(&edge)? == EdgeSatisfaction::Pending
            {
                pending.push(edge);
            }
        }
        Ok(pending)
    }

    /// The inputs a session of `stage` takes from its edges, in edge order,
    /// or `None` when some edge into it is not satisfied. `starting` says
    /// whether `stage` is the one the task starts in (its edges are base
    /// edges); otherwise they only gate.
    pub fn stage_edge_inputs(
        &self,
        task_id: &str,
        stage: &str,
        starting: bool,
    ) -> Result<Option<Vec<ConsumedDependency>>, rusqlite::Error> {
        let mut inputs = Vec::new();
        for edge in self.list_stage_edges_into(task_id)? {
            if edge.dependent_stage != stage {
                continue;
            }
            let EdgeSatisfaction::Satisfied(input) = self.stage_edge_satisfaction(&edge)? else {
                return Ok(None);
            };
            let role = match (starting, inputs.is_empty()) {
                (true, true) => DependencyRole::Base,
                (true, false) => DependencyRole::Merge,
                (false, _) => DependencyRole::Gate,
            };
            inputs.push(ConsumedDependency { edge, role, input });
        }
        Ok(Some(inputs))
    }

    /// Record, inside the caller's transaction, that `task_id` entered
    /// `stage` taking these inputs: each edge not consumed before gets the
    /// result id and SHA it consumed. Edges consumed on an earlier entry
    /// (a loop back) keep what they first consumed. Returns what was newly
    /// consumed, for the entry's ledger record.
    pub(crate) fn record_stage_edge_consumption(
        &self,
        inputs: &[ConsumedDependency],
    ) -> Result<Vec<ConsumedDependency>, rusqlite::Error> {
        let mut consumed = Vec::new();
        for dependency in inputs {
            let changed = self.conn.execute(
                "UPDATE task_stage_edge
                 SET consumed_result_id = ?, consumed_sha = ?,
                     consumed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ? AND consumed_at IS NULL",
                params![
                    dependency.input.result_id,
                    dependency.input.committed_sha,
                    dependency.edge.id
                ],
            )?;
            if changed > 0 {
                self.mark_task_snapshot_dirty(&dependency.edge.dependent_task_id)?;
                consumed.push(dependency.clone());
            }
        }
        Ok(consumed)
    }

    /// The entry side of a real transition, inside its transaction: record
    /// what `task_id` consumes on entering `to_stage` (later-stage edges
    /// gate only) and return it for the transition's ledger entry.
    pub(super) fn consume_stage_edges_on_entry(
        &self,
        task_id: &str,
        to_stage: &str,
    ) -> Result<Vec<ConsumedDependency>, rusqlite::Error> {
        let Some(inputs) = self.stage_edge_inputs(task_id, to_stage, false)? else {
            // Entered with an edge still pending (a person moved the task
            // there another way): nothing was consumed, and the edge keeps
            // waiting for a result it can record.
            return Ok(Vec::new());
        };
        self.record_stage_edge_consumption(&inputs)
    }

    /// The departure side of a real transition of `upstream_task_id` out of
    /// `from_stage`, inside its transaction. When it leaves forward with a
    /// success result that differs from what a dependent already consumed,
    /// the edge records the newer result as superseding and the dependent
    /// hears `task.dependency_superseded`. Nothing else happens to the
    /// dependent: no hold, no rerun, no rebase.
    pub(super) fn record_stage_edge_departure(
        &self,
        upstream_task_id: &str,
        from_stage: &str,
        to_stage: &str,
        triggering_result_id: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let edges = self.list_stage_edges_from(upstream_task_id, Some(from_stage))?;
        if edges.is_empty() {
            return Ok(());
        }
        let Some(trigger) = triggering_result_id else {
            return Ok(());
        };
        let Some(upstream) = self.get_pipeline_item(upstream_task_id)? else {
            return Ok(());
        };
        let stages = pinned_stage_names(upstream.pipeline_def.as_deref());
        if !is_forward(&stages, from_stage, to_stage) {
            return Ok(());
        }
        let Some(result) = self.success_result(upstream_task_id, trigger)? else {
            return Ok(());
        };
        for edge in edges {
            if edge.consumed_at.is_none()
                || edge.consumed_result_id.as_deref() == result.result_id.as_deref()
                || edge.superseded_result_id.as_deref() == result.result_id.as_deref()
            {
                continue;
            }
            self.conn.execute(
                "UPDATE task_stage_edge
                 SET superseded_result_id = ?, superseded_sha = ?,
                     superseded_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ?",
                params![result.result_id, result.committed_sha, edge.id],
            )?;
            self.mark_task_snapshot_dirty(&edge.dependent_task_id)?;
            self.append_task_event(
                &edge.dependent_task_id,
                TaskEventKind::DependencySuperseded,
                json!({
                    "upstreamTaskId": edge.upstream_task_id,
                    "upstreamStage": edge.upstream_stage,
                    "dependentStage": edge.dependent_stage,
                    "consumedResultId": edge.consumed_result_id,
                    "consumedSha": edge.consumed_sha,
                    "supersedingResultId": result.result_id,
                    "supersedingSha": result.committed_sha,
                }),
            )?;
        }
        Ok(())
    }

    /// Upstream ids whose edges hold `task_id` right now: edges into the
    /// stage a not-yet-started task starts in, or into the stage a parked
    /// completion is waiting to enter.
    pub(crate) fn list_waiting_stage_edge_upstreams(
        &self,
        task_id: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let has_edges: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_stage_edge WHERE dependent_task_id = ?)",
            [task_id],
            |row| row.get(0),
        )?;
        if !has_edges {
            return Ok(Vec::new());
        }
        let waiting_stage = match self.dependency_wait(task_id)? {
            Some(wait) => Some(wait.to_stage),
            None if self.get_task_worktree_path(task_id)?.is_none() => {
                self.pipeline_item_stage(task_id)?
            }
            None => None,
        };
        let Some(stage) = waiting_stage else {
            return Ok(Vec::new());
        };
        let mut upstreams = Vec::new();
        for edge in self.unsatisfied_stage_edges_into(task_id, &stage)? {
            if !upstreams.contains(&edge.upstream_task_id) {
                upstreams.push(edge.upstream_task_id);
            }
        }
        Ok(upstreams)
    }

    /// Park an automatic completion until the edges into `to_stage` are
    /// satisfied. `payload` is what replays the completion.
    pub(crate) fn record_dependency_wait(
        &self,
        task_id: &str,
        from_stage: &str,
        to_stage: &str,
        payload: &Value,
    ) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            let generation = db.task_run_generation(task_id)?;
            db.conn.execute(
                "INSERT INTO task_dependency_wait (task_id, from_stage, to_stage, generation, payload)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(task_id) DO UPDATE SET
                     from_stage = excluded.from_stage, to_stage = excluded.to_stage,
                     generation = excluded.generation, payload = excluded.payload,
                     created_at = excluded.created_at",
                params![task_id, from_stage, to_stage, generation, payload.to_string()],
            )?;
            db.sync_blocked_event(task_id)
        })
    }

    pub(crate) fn dependency_wait(
        &self,
        task_id: &str,
    ) -> Result<Option<DependencyWait>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT task_id, from_stage, to_stage, generation, payload
                 FROM task_dependency_wait WHERE task_id = ?",
                [task_id],
                |row| {
                    let payload: String = row.get(4)?;
                    Ok(DependencyWait {
                        task_id: row.get(0)?,
                        from_stage: row.get(1)?,
                        to_stage: row.get(2)?,
                        generation: row.get(3)?,
                        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                    })
                },
            )
            .optional()
    }

    /// Is the wait still where its completion left the task?
    pub(crate) fn dependency_wait_is_current(
        &self,
        wait: &DependencyWait,
    ) -> Result<bool, rusqlite::Error> {
        let Some(item) = self.get_pipeline_item(&wait.task_id)? else {
            return Ok(false);
        };
        Ok(item.closed_at.is_none()
            && item.stage.as_deref() == Some(wait.from_stage.as_str())
            && self.task_run_generation(&wait.task_id)? == wait.generation)
    }

    /// Drop the task's wait; `true` when there was one.
    pub(crate) fn clear_dependency_wait(&self, task_id: &str) -> Result<bool, rusqlite::Error> {
        Ok(self.conn.execute(
            "DELETE FROM task_dependency_wait WHERE task_id = ?",
            [task_id],
        )? > 0)
    }

    /// A dependent entering the stage it starts in, once its base edges are
    /// satisfied: record what each edge consumed and mirror the entry as a
    /// `transition` ledger entry (`operation: "dependency_start"`, no
    /// `from_stage`) listing the inputs in edge order — the first is the
    /// fork point, the rest were handed to the session to merge. One
    /// transaction; the dormant start calls it once its workspace exists.
    pub(crate) fn record_dependency_start(
        &self,
        task_id: &str,
        stage: &str,
        branch: &str,
        inputs: &[ConsumedDependency],
    ) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            db.record_stage_edge_consumption(inputs)?;
            let source_id = format!("{task_id}:dependency_start");
            db.enqueue_ledger_entry(super::task_store::NewLedgerEntry {
                task_id,
                kind: super::task_store::LedgerEntryKind::Transition,
                operation_id: None,
                source_kind: "stage_edge",
                source_id: &source_id,
                source_origin: None,
                historical: false,
                recorded_at: None,
                run_id: None,
                declared_role: None,
                channel_identity: &crate::mutation_provenance::ChannelIdentity::Server,
                body: json!({
                    "from_stage": Value::Null,
                    "to_stage": stage,
                    "branch": branch,
                    "trigger": "auto",
                    "operation": "dependency_start",
                    "triggering_result_id": Value::Null,
                    "exit": Value::Null,
                    "exit_source": Value::Null,
                    "dependencies": inputs
                        .iter()
                        .map(ConsumedDependency::to_ledger_json)
                        .collect::<Vec<_>>(),
                }),
                message: None,
                hold_events_after: None,
                reserved_sequence: None,
            })?;
            db.sync_blocked_event(task_id)
        })
    }

    /// Every stored edge, dependent side, for `task.json`.
    pub(crate) fn stage_edge_links(&self, task_id: &str) -> Result<Vec<Value>, rusqlite::Error> {
        Ok(self
            .list_stage_edges_into(task_id)?
            .iter()
            .map(|edge| {
                json!({
                    "upstream_task_id": edge.upstream_task_id,
                    "upstream_stage": edge.upstream_stage,
                    "dependent_stage": edge.dependent_stage,
                    "position": edge.position,
                    "consumed_result_id": edge.consumed_result_id,
                    "consumed_sha": edge.consumed_sha,
                    "consumed_at": edge.consumed_at,
                    "superseded_result_id": edge.superseded_result_id,
                    "superseded_sha": edge.superseded_sha,
                    "superseded_at": edge.superseded_at,
                })
            })
            .collect())
    }

    /// The newest forward departure from `stage` caused by a success result.
    fn latest_success_departure(
        &self,
        task_id: &str,
        stage: &str,
        stages: &[String],
    ) -> Result<Option<StageEdgeInput>, rusqlite::Error> {
        for transition in self.ledger_bodies(task_id, "transition")? {
            if transition.get("from_stage").and_then(Value::as_str) != Some(stage) {
                continue;
            }
            let Some(to_stage) = transition.get("to_stage").and_then(Value::as_str) else {
                continue;
            };
            if !is_forward(stages, stage, to_stage) {
                continue;
            }
            let Some(trigger) = transition
                .get("triggering_result_id")
                .and_then(Value::as_str)
            else {
                continue;
            };
            if let Some(result) = self.success_result(task_id, trigger)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    fn latest_success_result_at_stage(
        &self,
        task_id: &str,
        stage: &str,
    ) -> Result<Option<StageEdgeInput>, rusqlite::Error> {
        for result in self.ledger_bodies(task_id, "result")? {
            if result.get("stage").and_then(Value::as_str) == Some(stage)
                && result.get("status").and_then(Value::as_str) == Some("success")
            {
                return Ok(Some(result_input(&result)));
            }
        }
        Ok(None)
    }

    /// The result entry `entry_id` of `task_id`, when its status is success.
    fn success_result(
        &self,
        task_id: &str,
        entry_id: &str,
    ) -> Result<Option<StageEdgeInput>, rusqlite::Error> {
        let row = self
            .conn
            .query_row(
                "SELECT file_name, payload FROM task_ledger_entry
                 WHERE task_id = ? AND entry_id = ? AND kind = 'result'",
                params![task_id, entry_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?;
        let Some((file_name, payload)) = row else {
            return Ok(None);
        };
        let Ok(file) = crate::task_store::parse_ledger_file(&file_name, &payload) else {
            return Ok(None);
        };
        let body = file.body();
        if body.get("status").and_then(Value::as_str) != Some("success") {
            return Ok(None);
        }
        Ok(Some(result_input(body)))
    }

    /// Bodies of one kind of `task_id`'s ledger entries, newest first. Read
    /// from the SQL outbox, which is authoritative during the bridge.
    fn ledger_bodies(&self, task_id: &str, kind: &str) -> Result<Vec<Value>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT file_name, payload FROM task_ledger_entry
             WHERE task_id = ? AND kind = ? ORDER BY sequence DESC",
        )?;
        let rows = stmt.query_map(params![task_id, kind], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut bodies = Vec::new();
        for row in rows {
            let (file_name, payload) = row?;
            if let Ok(file) = crate::task_store::parse_ledger_file(&file_name, &payload) {
                bodies.push(file.body().clone());
            }
        }
        Ok(bodies)
    }
}

fn result_input(body: &Value) -> StageEdgeInput {
    StageEdgeInput {
        result_id: body
            .get("result_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        committed_sha: body
            .get("committed_sha")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Stage names of a pinned workflow definition, in order.
pub(crate) fn pinned_stage_names(definition: Option<&str>) -> Vec<String> {
    definition
        .and_then(|definition| serde_json::from_str::<Value>(definition).ok())
        .and_then(|definition| {
            definition
                .get("stages")
                .and_then(Value::as_array)
                .map(|stages| {
                    stages
                        .iter()
                        .filter_map(|stage| stage.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
        })
        .unwrap_or_default()
}

/// A move from `from` to `to` is a departure when it goes to a later stage,
/// or to a stage the (replaced) plan no longer names.
fn is_forward(stages: &[String], from: &str, to: &str) -> bool {
    match (
        stages.iter().position(|stage| stage == from),
        stages.iter().position(|stage| stage == to),
    ) {
        (Some(from), Some(to)) => to > from,
        (_, None) => from != to,
        (None, Some(_)) => false,
    }
}

/// Would `(upstream, upstream_stage) → (dependent, dependent_stage)` close a
/// cycle? Nodes are "task T has left stage i" (for T's final stage: T is
/// closed). Leaving stage i requires leaving every earlier stage; entering
/// (and so leaving) stage Y of a dependent requires each edge into any stage
/// up to Y. A legacy `task_blocker` row gates the blocked task's first stage
/// on the blocker's closure. The new edge closes a cycle exactly when leaving
/// `upstream_stage` already requires the dependent to leave `dependent_stage`
/// or a later stage — so an edge between two tasks that already depend on
/// each other at unrelated stages is accepted, and an indirect loop through
/// any number of tasks and stages is refused.
fn dependency_graph_has_cycle_through(
    db: &Db,
    upstream_task_id: &str,
    upstream_stage: &str,
    dependent_task_id: &str,
    dependent_stage: &str,
) -> Result<bool, rusqlite::Error> {
    let mut stage_lists: HashMap<String, Vec<String>> = HashMap::new();
    let mut stages_of = |task_id: &str| -> Result<Vec<String>, rusqlite::Error> {
        if let Some(stages) = stage_lists.get(task_id) {
            return Ok(stages.clone());
        }
        let stages = db
            .get_pipeline_item(task_id)?
            .map(|item| pinned_stage_names(item.pipeline_def.as_deref()))
            .unwrap_or_default();
        stage_lists.insert(task_id.to_string(), stages.clone());
        Ok(stages)
    };
    // An unknown upstream stage is read as the last one and an unknown
    // dependent stage as the first: both over-approximate what is required,
    // so a cycle is never missed.
    let upstream_index = |stages: &[String], stage: &str| {
        stages
            .iter()
            .position(|name| name == stage)
            .unwrap_or(stages.len().saturating_sub(1))
    };
    let dependent_index =
        |stages: &[String], stage: &str| stages.iter().position(|name| name == stage).unwrap_or(0);

    let target_index = dependent_index(&stages_of(dependent_task_id)?, dependent_stage);
    // Highest stage index already required of each task: requiring stage i
    // implies requiring every earlier stage, so a lower index adds nothing.
    let mut required: HashMap<String, usize> = HashMap::new();
    let start_index = upstream_index(&stages_of(upstream_task_id)?, upstream_stage);
    let mut stack = vec![(upstream_task_id.to_string(), start_index)];
    while let Some((task_id, index)) = stack.pop() {
        if task_id == dependent_task_id && index >= target_index {
            return Ok(true);
        }
        if required.get(&task_id).is_some_and(|seen| *seen >= index) {
            continue;
        }
        required.insert(task_id.clone(), index);
        let task_stages = stages_of(&task_id)?;
        let mut edges = db.conn.prepare(
            "SELECT upstream_task_id, upstream_stage, dependent_stage
             FROM task_stage_edge WHERE dependent_task_id = ?",
        )?;
        let incoming = edges
            .query_map([&task_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (upstream, up_stage, down_stage) in incoming {
            if dependent_index(&task_stages, &down_stage) <= index {
                let up_index = upstream_index(&stages_of(&upstream)?, &up_stage);
                stack.push((upstream, up_index));
            }
        }
        let mut blockers = db
            .conn
            .prepare("SELECT blocker_item_id FROM task_blocker WHERE blocked_item_id = ?")?;
        let legacy = blockers
            .query_map([&task_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for blocker in legacy {
            let final_index = stages_of(&blocker)?.len().saturating_sub(1);
            stack.push((blocker, final_index));
        }
    }
    Ok(false)
}

/// Would making `blocked_task_id` wait on the closure of `blocker_task_id`
/// (a legacy blocker row) close a cycle through stage edges or other
/// blockers? The legacy row is the edge (blocker, final stage) → (blocked,
/// first stage).
pub(crate) fn legacy_blocker_would_cycle(
    db: &Db,
    blocked_task_id: &str,
    blocker_task_id: &str,
) -> Result<bool, rusqlite::Error> {
    let final_stage = db
        .get_pipeline_item(blocker_task_id)?
        .map(|item| pinned_stage_names(item.pipeline_def.as_deref()))
        .and_then(|stages| stages.last().cloned())
        .unwrap_or_default();
    let first_stage = db
        .get_pipeline_item(blocked_task_id)?
        .map(|item| pinned_stage_names(item.pipeline_def.as_deref()))
        .and_then(|stages| stages.first().cloned())
        .unwrap_or_default();
    dependency_graph_has_cycle_through(
        db,
        blocker_task_id,
        &final_stage,
        blocked_task_id,
        &first_stage,
    )
}

#[cfg(test)]
impl Db {
    /// Record a result ledger entry for `task_id` as a completion would:
    /// `status` at `stage`, observed at `committed_sha`. Returns its id.
    pub(crate) fn record_test_stage_result(
        &self,
        task_id: &str,
        stage: &str,
        status: &str,
        committed_sha: Option<&str>,
    ) -> String {
        let entries: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM task_ledger_entry WHERE task_id = ?",
                [task_id],
                |row| row.get(0),
            )
            .unwrap();
        let source_id = format!("{task_id}:test-result:{entries}");
        self.enqueue_ledger_entry(super::task_store::NewLedgerEntry {
            task_id,
            kind: super::task_store::LedgerEntryKind::Result,
            operation_id: None,
            source_kind: "stage_run",
            source_id: &source_id,
            source_origin: None,
            historical: false,
            recorded_at: None,
            run_id: None,
            declared_role: Some("agent"),
            channel_identity: &crate::mutation_provenance::ChannelIdentity::Unknown,
            body: json!({
                "status": status,
                "stage": stage,
                "run_kind": "main",
                "branch": Value::Null,
                "committed_sha": committed_sha,
            }),
            message: Some("test result"),
            hold_events_after: None,
            reserved_sequence: None,
        })
        .unwrap()
        .entry_id
    }

    /// Pin `stages` as `task_id`'s workflow.
    pub(crate) fn pin_test_stages(&self, task_id: &str, stages: &[&str]) {
        let definition = json!({
            "name": "test",
            "stages": stages.iter().map(|name| json!({"name": name})).collect::<Vec<_>>(),
        });
        self.conn
            .execute(
                "UPDATE pipeline_item SET pipeline_def = ? WHERE id = ?",
                params![definition.to_string(), task_id],
            )
            .unwrap();
    }
}

#[cfg(test)]
#[path = "stage_edge_tests.rs"]
mod tests;
