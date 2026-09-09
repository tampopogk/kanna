//! Task terminal sessions, addressed by what they are rather than by the task
//! they belong to.
//!
//! A task used to have exactly one PTY: the startup script and the agent CLI
//! shared its scrollback, and a stage transition respawned the same session id
//! over the top of it. Now a launch owns a *pair* — a plain `setup` shell where
//! the repo's startup commands run visibly, and the `agent` session the server
//! starts once that shell exits cleanly — and every launch gets its own setup
//! terminal, so stage boundaries are terminal boundaries.
//!
//! Rows written before that split carry the role `legacy_agent`: they are one
//! mixed session and are deliberately not split or restarted, they simply keep
//! serving as the task's agent terminal until its next launch.

use super::Db;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

/// What a task terminal is. `agent` is the provider session the rest of the
/// system already addresses by task id; `setup` is the startup shell that runs
/// before it; `teardown` is the departing workspace's best-effort cleanup.
const ROLE_AGENT: &str = "agent";
pub const ROLE_SETUP: &str = "setup";
pub const ROLE_TEARDOWN: &str = "teardown";
const ROLE_LEGACY_AGENT: &str = "legacy_agent";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTerminalSession {
    pub id: String,
    pub task_id: Option<String>,
    pub repo_id: String,
    pub daemon_session_id: Option<String>,
    pub role: String,
    pub stage: Option<String>,
    pub attempt: i64,
    pub state: String,
    pub stage_run_id: Option<String>,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i64>,
    pub created_at: String,
    pub retired_at: Option<String>,
}

pub struct NewTaskTerminalSession<'a> {
    pub id: &'a str,
    pub repo_id: &'a str,
    pub task_id: Option<&'a str>,
    pub daemon_session_id: Option<&'a str>,
    pub role: &'a str,
    pub stage: Option<&'a str>,
    pub attempt: i64,
    pub stage_run_id: Option<&'a str>,
    pub title: Option<&'a str>,
    pub cwd: Option<&'a str>,
}

const SELECT_COLUMNS: &str = "id, pipeline_item_id, repo_id, daemon_session_id, role, stage, \
                              attempt, state, stage_run_id, title, cwd, exit_code, created_at, \
                              retired_at";

fn row_to_session(row: &rusqlite::Row<'_>) -> Result<TaskTerminalSession, rusqlite::Error> {
    Ok(TaskTerminalSession {
        id: row.get(0)?,
        task_id: row.get(1)?,
        repo_id: row.get(2)?,
        daemon_session_id: row.get(3)?,
        role: row.get(4)?,
        stage: row.get(5)?,
        attempt: row.get(6)?,
        state: row.get(7)?,
        stage_run_id: row.get(8)?,
        title: row.get(9)?,
        cwd: row.get(10)?,
        exit_code: row.get(11)?,
        created_at: row.get(12)?,
        retired_at: row.get(13)?,
    })
}

impl Db {
    /// Record a terminal a launch is about to create. Written *before* the
    /// daemon spawn, so a setup shell that emits output immediately already
    /// has a row the desktop can open a tab against.
    pub fn upsert_task_terminal_session(
        &self,
        session: NewTaskTerminalSession<'_>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO terminal_session
               (id, repo_id, pipeline_item_id, label, cwd, daemon_session_id,
                role, stage, attempt, state, stage_run_id, title)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'live', ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
               repo_id = excluded.repo_id,
               pipeline_item_id = excluded.pipeline_item_id,
               label = excluded.label,
               cwd = excluded.cwd,
               daemon_session_id = excluded.daemon_session_id,
               role = excluded.role,
               stage = excluded.stage,
               attempt = excluded.attempt,
               state = 'live',
               stage_run_id = excluded.stage_run_id,
               title = excluded.title,
               exit_code = NULL,
               retired_at = NULL",
            params![
                session.id,
                session.repo_id,
                session.task_id,
                // `label` predates roles and is still what older readers sort
                // on, so it keeps saying the same thing the role does.
                session.role,
                session.cwd,
                session.daemon_session_id,
                session.role,
                session.stage,
                session.attempt,
                session.stage_run_id,
                session.title,
            ],
        )?;
        Ok(())
    }

    /// Every terminal a task owns, oldest first — the order they were launched
    /// in, which is the order a reader expects their tabs in.
    pub fn list_task_terminal_sessions(
        &self,
        task_id: &str,
    ) -> Result<Vec<TaskTerminalSession>, rusqlite::Error> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM terminal_session
             WHERE pipeline_item_id = ?1
             ORDER BY attempt, CASE role WHEN 'setup' THEN 0 WHEN 'agent' THEN 1 ELSE 2 END, created_at, id"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([task_id], row_to_session)?;
        rows.collect()
    }

    /// The role a live daemon session is playing, or `None` when the id is not
    /// a recorded task terminal. Callers that must not treat setup output as
    /// agent output ask this before acting on a daemon event.
    pub fn terminal_session_role(
        &self,
        daemon_session_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT role FROM terminal_session
                 WHERE daemon_session_id = ?1
                 ORDER BY created_at DESC, id
                 LIMIT 1",
                [daemon_session_id],
                |row| row.get(0),
            )
            .optional()
    }

    /// The next launch number for a task. Attempts count launches, not stages:
    /// a rerun, a resumed revision, and a stage advance each open their own
    /// setup terminal and must not land on an existing one's id.
    pub fn next_task_terminal_attempt(&self, task_id: &str) -> Result<i64, rusqlite::Error> {
        let highest: Option<i64> = self.conn.query_row(
            "SELECT MAX(attempt) FROM terminal_session WHERE pipeline_item_id = ?1",
            [task_id],
            |row| row.get(0),
        )?;
        Ok(highest.unwrap_or(0) + 1)
    }

    /// Mark a terminal as finished. The row stays: its scrollback is the
    /// durable record of what that launch's startup did, and a stage that has
    /// moved on is exactly when someone wants to read it.
    pub fn retire_task_terminal_session(
        &self,
        daemon_session_id: &str,
        exit_code: Option<i64>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE terminal_session
             SET state = 'retired', exit_code = ?2, retired_at = datetime('now')
             WHERE daemon_session_id = ?1 AND state != 'retired'",
            params![daemon_session_id, exit_code],
        )?;
        Ok(())
    }

    /// Whether this daemon session is a task's current agent terminal — the
    /// one question every agent-facing surface (completion, composer, waiting
    /// prompts, delivered input) has to answer before acting on an event.
    pub fn is_agent_terminal_session(
        &self,
        daemon_session_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        match self.terminal_session_role(daemon_session_id)? {
            // An id with no row at all is a task session from before roles
            // existed, or one the daemon knows and the DB does not. Treating
            // it as the agent keeps every pre-split task behaving exactly as
            // it did; only a row that positively says `setup` or `teardown`
            // is excluded.
            None => Ok(true),
            Some(role) => Ok(role == ROLE_AGENT || role == ROLE_LEGACY_AGENT),
        }
    }
}
