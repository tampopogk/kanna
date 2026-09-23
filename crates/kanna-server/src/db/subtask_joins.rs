//! Engine-owned subtask joins (spec §9, §16.4 — component T5).
//!
//! A parent session creates children in a *join*: a cohort recorded before
//! any child exists, naming each child by the id it will be created with,
//! the commit the parent had when it launched them (every child forks from
//! it), and the create request that is replayed if a restart interrupts the
//! launch. Only the members of a join ever satisfy it: an earlier child, or
//! one attached with a plain `parentTaskId`, is not a member.
//!
//! A member resolves exactly once, in the same transaction as the fact that
//! resolves it — the child's first recorded result (any status), the child
//! closing without one, or a creation that never produced a task — and that
//! transaction also delivers the outcome into the parent's input ledger (a
//! `task_input` row with source `subtask_join` and its `input` ledger entry).
//! A replayed result, a restart or a second close finds the member resolved
//! and delivers nothing. A child whose session died without recording a
//! result stays unresolved and reads as `stalled`, with the actions that can
//! move it; it is never counted.
//!
//! While any member of any of its joins is unresolved the parent reads as
//! blocked on those children (`blockedByTaskIds`, `task.blocked`), and its
//! progression — a successful completion or a manual advance — is refused.
//! Its session is left running. The engine merges and aggregates nothing:
//! the parent combines what it was given.
//!
//! Typing the delivered text into the parent's live session is a separate
//! at-most-once notice (`notified_at`), because the ledger input is the
//! delivery; see `http_api::subtask_joins`.

use super::{Db, TaskEventKind};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS task_join (
        id TEXT PRIMARY KEY,
        parent_task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        parent_stage TEXT,
        parent_run_id TEXT,
        -- The parent's committed HEAD when it launched the children; every
        -- member forks from it.
        base_sha TEXT NOT NULL,
        base_branch TEXT,
        created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        completed_at TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_task_join_parent ON task_join(parent_task_id);
    CREATE TABLE IF NOT EXISTS task_join_member (
        join_id TEXT NOT NULL REFERENCES task_join(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        -- No foreign key: the member is recorded before its task exists.
        child_task_id TEXT NOT NULL UNIQUE,
        -- The create request, replayed if the launch was interrupted.
        spec TEXT NOT NULL,
        create_error TEXT,
        resolved_at TEXT,
        -- result | closed | not_created
        outcome TEXT,
        result_id TEXT,
        result_status TEXT,
        result_stage TEXT,
        result_sha TEXT,
        -- The parent's task_input row that delivered the outcome.
        input_id INTEGER,
        notified_at TEXT,
        PRIMARY KEY (join_id, position)
    );
"#;

/// A join's parent input rows carry this source: the engine delivered them
/// into the ledger, whether or not the notice was typed into a session.
pub const SUBTASK_JOIN_INPUT_SOURCE: &str = "subtask_join";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskJoin {
    pub id: String,
    pub parent_task_id: String,
    pub parent_stage: Option<String>,
    pub parent_run_id: Option<String>,
    pub base_sha: String,
    pub base_branch: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskJoinMember {
    pub join_id: String,
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
    pub input_id: Option<i64>,
    pub notified_at: Option<String>,
}

/// A member to record: the id its task will be created with, and the create
/// request (JSON) that creates it.
#[derive(Debug, Clone)]
pub struct NewJoinMember {
    pub child_task_id: String,
    pub spec: String,
}

/// A join to record, before any of its children exists.
#[derive(Debug, Clone)]
pub struct NewTaskJoin {
    pub id: String,
    pub parent_task_id: String,
    pub parent_stage: Option<String>,
    pub parent_run_id: Option<String>,
    pub base_sha: String,
    pub base_branch: Option<String>,
    pub members: Vec<NewJoinMember>,
}

/// What resolved a member.
enum Resolution<'a> {
    Result {
        result_id: &'a str,
        status: &'a str,
        stage: Option<&'a str>,
        committed_sha: Option<&'a str>,
        message: &'a str,
    },
    Closed,
    NotCreated {
        error: &'a str,
    },
}

impl Resolution<'_> {
    fn outcome(&self) -> &'static str {
        match self {
            Self::Result { .. } => "result",
            Self::Closed => "closed",
            Self::NotCreated { .. } => "not_created",
        }
    }
}

/// A delivered outcome whose notice has not been typed into the parent yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingJoinNotice {
    pub parent_task_id: String,
    pub child_task_id: String,
    pub message: String,
}

const JOIN_COLUMNS: &str = "id, parent_task_id, parent_stage, parent_run_id, base_sha, \
    base_branch, created_at, completed_at";

const MEMBER_COLUMNS: &str = "join_id, position, child_task_id, spec, create_error, \
    resolved_at, outcome, result_id, result_status, result_stage, result_sha, input_id, \
    notified_at";

fn join_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskJoin> {
    Ok(TaskJoin {
        id: row.get(0)?,
        parent_task_id: row.get(1)?,
        parent_stage: row.get(2)?,
        parent_run_id: row.get(3)?,
        base_sha: row.get(4)?,
        base_branch: row.get(5)?,
        created_at: row.get(6)?,
        completed_at: row.get(7)?,
    })
}

fn member_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskJoinMember> {
    Ok(TaskJoinMember {
        join_id: row.get(0)?,
        position: row.get(1)?,
        child_task_id: row.get(2)?,
        spec: row.get(3)?,
        create_error: row.get(4)?,
        resolved_at: row.get(5)?,
        outcome: row.get(6)?,
        result_id: row.get(7)?,
        result_status: row.get(8)?,
        result_stage: row.get(9)?,
        result_sha: row.get(10)?,
        input_id: row.get(11)?,
        notified_at: row.get(12)?,
    })
}

impl Db {
    /// Record a join and its members in one immediate transaction, before
    /// any child is created, and mark the parent blocked on them.
    pub(crate) fn create_task_join(&self, join: &NewTaskJoin) -> Result<TaskJoin, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            db.conn.execute(
                "INSERT INTO task_join
                 (id, parent_task_id, parent_stage, parent_run_id, base_sha, base_branch)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    join.id,
                    join.parent_task_id,
                    join.parent_stage,
                    join.parent_run_id,
                    join.base_sha,
                    join.base_branch
                ],
            )?;
            for (index, member) in join.members.iter().enumerate() {
                db.conn.execute(
                    "INSERT INTO task_join_member (join_id, position, child_task_id, spec)
                     VALUES (?, ?, ?, ?)",
                    params![join.id, index as i64 + 1, member.child_task_id, member.spec],
                )?;
            }
            db.mark_task_snapshot_dirty(&join.parent_task_id)?;
            db.sync_blocked_event(&join.parent_task_id)?;
            db.task_join(&join.id)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)
        })
    }

    pub(crate) fn task_join(&self, join_id: &str) -> Result<Option<TaskJoin>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {JOIN_COLUMNS} FROM task_join WHERE id = ?"),
                [join_id],
                join_from_row,
            )
            .optional()
    }

    /// Every join `parent_task_id` created, oldest first.
    pub(crate) fn list_task_joins(
        &self,
        parent_task_id: &str,
    ) -> Result<Vec<TaskJoin>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {JOIN_COLUMNS} FROM task_join WHERE parent_task_id = ?
             ORDER BY created_at, rowid"
        ))?;
        let rows = stmt.query_map([parent_task_id], join_from_row)?;
        rows.collect()
    }

    pub(crate) fn list_task_join_members(
        &self,
        join_id: &str,
    ) -> Result<Vec<TaskJoinMember>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MEMBER_COLUMNS} FROM task_join_member WHERE join_id = ? ORDER BY position"
        ))?;
        let rows = stmt.query_map([join_id], member_from_row)?;
        rows.collect()
    }

    pub(crate) fn task_join_member(
        &self,
        child_task_id: &str,
    ) -> Result<Option<TaskJoinMember>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {MEMBER_COLUMNS} FROM task_join_member WHERE child_task_id = ?"),
                [child_task_id],
                member_from_row,
            )
            .optional()
    }

    /// Children `parent_task_id` is waiting on: the unresolved members of
    /// its joins, in join and member order. Part of its blocked state.
    pub(crate) fn unresolved_join_children(
        &self,
        parent_task_id: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT member.child_task_id FROM task_join_member member
             JOIN task_join ON task_join.id = member.join_id
             WHERE task_join.parent_task_id = ? AND member.resolved_at IS NULL
             ORDER BY task_join.created_at, task_join.rowid, member.position",
        )?;
        let rows = stmt.query_map([parent_task_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Members whose task was never created — a launch interrupted before
    /// it reached them — for the startup sweep to create from their spec.
    pub(crate) fn list_uncreated_join_members(
        &self,
    ) -> Result<Vec<TaskJoinMember>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM task_join_member member
             WHERE member.resolved_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM pipeline_item item
                               WHERE item.id = member.child_task_id)
             ORDER BY member.join_id, member.position",
            MEMBER_COLUMNS
                .split(", ")
                .map(|column| format!("member.{}", column.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        let rows = stmt.query_map([], member_from_row)?;
        rows.collect()
    }

    pub(crate) fn record_join_member_create_error(
        &self,
        child_task_id: &str,
        error: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE task_join_member SET create_error = ?
             WHERE child_task_id = ? AND resolved_at IS NULL",
            params![error, child_task_id],
        )?;
        Ok(())
    }

    /// Inside the transaction that records a result entry of `task_id`: if
    /// the task is an unresolved join member, this result resolves it and is
    /// delivered to its parent. A later result of the same child, or a
    /// replay, changes nothing.
    pub(super) fn resolve_join_member_on_result(
        &self,
        task_id: &str,
        result_id: &str,
        body: &Value,
        message: &str,
    ) -> Result<(), rusqlite::Error> {
        let text = |key: &str| body.get(key).and_then(Value::as_str);
        self.resolve_join_member(
            task_id,
            Resolution::Result {
                result_id,
                status: text("status").unwrap_or("unknown"),
                stage: text("stage"),
                committed_sha: text("committed_sha"),
                message,
            },
        )
    }

    /// Inside the close of `task_id`: a member that closes without a result
    /// resolves as `closed`, and its parent is told so. Closing is one of
    /// the actions a stalled member offers; it is never inferred.
    pub(super) fn resolve_join_member_on_close(
        &self,
        task_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.resolve_join_member(task_id, Resolution::Closed)
    }

    /// A member whose task could not be created (none exists): resolved as
    /// `not_created` with the error, so the parent is told and not left
    /// waiting on a task that will never report.
    pub(crate) fn resolve_join_member_not_created(
        &self,
        child_task_id: &str,
        error: &str,
    ) -> Result<bool, rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            let exists: bool = db.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pipeline_item WHERE id = ?)",
                [child_task_id],
                |row| row.get(0),
            )?;
            if exists {
                // The task exists after all (a spawn failed after its row was
                // written): it is a child with a stalled run, not a member
                // that never existed.
                db.record_join_member_create_error(child_task_id, error)?;
                return Ok(false);
            }
            db.resolve_join_member(child_task_id, Resolution::NotCreated { error })?;
            Ok(true)
        })
    }

    fn resolve_join_member(
        &self,
        child_task_id: &str,
        resolution: Resolution<'_>,
    ) -> Result<(), rusqlite::Error> {
        let Some(member) = self.task_join_member(child_task_id)? else {
            return Ok(());
        };
        if member.resolved_at.is_some() {
            return Ok(());
        }
        let Some(join) = self.task_join(&member.join_id)? else {
            return Ok(());
        };
        let members = self.list_task_join_members(&join.id)?;
        let total = members.len();
        let resolved = members
            .iter()
            .filter(|other| other.resolved_at.is_some())
            .count()
            + 1;
        let waiting: Vec<&str> = members
            .iter()
            .filter(|other| {
                other.resolved_at.is_none() && other.child_task_id != member.child_task_id
            })
            .map(|other| other.child_task_id.as_str())
            .collect();
        let title = self
            .get_pipeline_item(child_task_id)?
            .and_then(|item| item.display_name.or(item.prompt))
            .map(|title| title.lines().next().unwrap_or_default().to_string());
        let message = join_input_message(
            &join,
            child_task_id,
            title.as_deref(),
            &resolution,
            resolved,
            total,
            &waiting,
        );
        let Some(input) = self.insert_delivered_task_input(
            &join.parent_task_id,
            SUBTASK_JOIN_INPUT_SOURCE,
            &crate::mutation_provenance::ChannelIdentity::Server,
            &message,
        )?
        else {
            return Ok(());
        };
        let (result_id, status, stage, sha) = match &resolution {
            Resolution::Result {
                result_id,
                status,
                stage,
                committed_sha,
                ..
            } => (Some(*result_id), Some(*status), *stage, *committed_sha),
            Resolution::Closed | Resolution::NotCreated { .. } => (None, None, None, None),
        };
        let changed = self.conn.execute(
            "UPDATE task_join_member
             SET resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), outcome = ?,
                 result_id = ?, result_status = ?, result_stage = ?, result_sha = ?,
                 input_id = ?,
                 create_error = COALESCE(?, create_error)
             WHERE child_task_id = ? AND resolved_at IS NULL",
            params![
                resolution.outcome(),
                result_id,
                status,
                stage,
                sha,
                input.id,
                match &resolution {
                    Resolution::NotCreated { error } => Some(*error),
                    _ => None,
                },
                child_task_id
            ],
        )?;
        if changed != 1 {
            // Checked above inside the same transaction; a second writer
            // cannot have resolved it in between.
            return Err(rusqlite::Error::StatementChangedRows(changed));
        }
        let complete = waiting.is_empty();
        if complete {
            self.conn.execute(
                "UPDATE task_join SET completed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE id = ? AND completed_at IS NULL",
                [&join.id],
            )?;
        }
        self.append_task_event(
            &join.parent_task_id,
            TaskEventKind::SubtaskResultDelivered,
            json!({
                "joinId": join.id,
                "childTaskId": child_task_id,
                "outcome": resolution.outcome(),
                "resultId": result_id,
                "status": status,
                "stage": stage,
                "committedSha": sha,
                "inputId": input.id,
                "resolved": resolved,
                "total": total,
                "joinComplete": complete,
            }),
        )?;
        self.mark_task_snapshot_dirty(&join.parent_task_id)?;
        self.sync_blocked_event(&join.parent_task_id)
    }

    /// Open parents with a completion parked on dependency edges (T4) whose
    /// joins have all resolved: the join no longer holds that completion,
    /// so readiness decides it again.
    pub(crate) fn parents_released_by_joins(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT task_join.parent_task_id FROM task_join
             JOIN task_dependency_wait wait ON wait.task_id = task_join.parent_task_id
             JOIN pipeline_item parent ON parent.id = task_join.parent_task_id
             WHERE parent.closed_at IS NULL
               AND NOT EXISTS (
                   SELECT 1 FROM task_join_member member
                   JOIN task_join other ON other.id = member.join_id
                   WHERE other.parent_task_id = task_join.parent_task_id
                     AND member.resolved_at IS NULL)
             ORDER BY task_join.parent_task_id",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Delivered outcomes whose notice has not been typed into the parent's
    /// session, for open parents, oldest first.
    pub(crate) fn pending_join_notices(&self) -> Result<Vec<PendingJoinNotice>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT task_join.parent_task_id, member.child_task_id, input.message
             FROM task_join_member member
             JOIN task_join ON task_join.id = member.join_id
             JOIN task_input input ON input.id = member.input_id
             JOIN pipeline_item parent ON parent.id = task_join.parent_task_id
             WHERE member.resolved_at IS NOT NULL AND member.notified_at IS NULL
               AND parent.closed_at IS NULL
             ORDER BY member.input_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PendingJoinNotice {
                parent_task_id: row.get(0)?,
                child_task_id: row.get(1)?,
                message: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Claim the notice of `child_task_id`'s outcome for typing. `false` when
    /// another sweep already claimed it: a notice is typed at most once.
    pub(crate) fn claim_join_notice(&self, child_task_id: &str) -> Result<bool, rusqlite::Error> {
        Ok(self.conn.execute(
            "UPDATE task_join_member SET notified_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE child_task_id = ? AND resolved_at IS NOT NULL AND notified_at IS NULL",
            [child_task_id],
        )? == 1)
    }

    /// Give a claimed notice back after a typing attempt that wrote nothing.
    pub(crate) fn release_join_notice(&self, child_task_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE task_join_member SET notified_at = NULL WHERE child_task_id = ?",
            [child_task_id],
        )?;
        Ok(())
    }

    /// The joins of `task_id`, parent side, for `task.json`.
    pub(crate) fn subtask_join_links(&self, task_id: &str) -> Result<Vec<Value>, rusqlite::Error> {
        let mut links = Vec::new();
        for join in self.list_task_joins(task_id)? {
            let members = self
                .list_task_join_members(&join.id)?
                .into_iter()
                .map(|member| {
                    json!({
                        "child_task_id": member.child_task_id,
                        "position": member.position,
                        "outcome": member.outcome,
                        "result_id": member.result_id,
                        "input_id": member.input_id,
                        "resolved_at": member.resolved_at,
                    })
                })
                .collect::<Vec<_>>();
            links.push(json!({
                "join_id": join.id,
                "parent_stage": join.parent_stage,
                "base_sha": join.base_sha,
                "created_at": join.created_at,
                "completed_at": join.completed_at,
                "members": members,
            }));
        }
        Ok(links)
    }
}

/// The text the parent receives for one resolved member: which child, what it
/// recorded, its message verbatim, and where the join stands.
fn join_input_message(
    join: &TaskJoin,
    child_task_id: &str,
    title: Option<&str>,
    resolution: &Resolution<'_>,
    resolved: usize,
    total: usize,
    waiting: &[&str],
) -> String {
    let child = match title.filter(|title| !title.trim().is_empty()) {
        Some(title) => format!("{child_task_id} ({title})"),
        None => child_task_id.to_string(),
    };
    let mut text = format!(
        "[kanna] Subtask {resolved} of {total} resolved in join {}: ",
        join.id
    );
    match resolution {
        Resolution::Result {
            result_id,
            status,
            stage,
            committed_sha,
            message,
        } => {
            text.push_str(&format!(
                "child {child} recorded {status} at stage '{}' (result {result_id}, commit {}).\n\n{}",
                stage.unwrap_or("unknown"),
                committed_sha.unwrap_or("none"),
                message.trim_end(),
            ));
        }
        Resolution::Closed => {
            text.push_str(&format!(
                "child {child} was closed without recording a result."
            ));
        }
        Resolution::NotCreated { error } => {
            text.push_str(&format!("child {child} could not be created: {error}"));
        }
    }
    text.push_str("\n\n");
    if waiting.is_empty() {
        text.push_str(
            "Every child in this join has resolved. Combine their results yourself — the engine \
             merged nothing — and continue.",
        );
    } else {
        text.push_str(&format!("Still waiting on: {}.", waiting.join(", ")));
    }
    text
}

#[cfg(test)]
#[path = "subtask_join_tests.rs"]
mod tests;
