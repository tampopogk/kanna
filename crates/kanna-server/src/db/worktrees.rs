use super::{pipeline_items::update_open_pipeline_item_activity, Db, WorktreeRecord};
use rusqlite::OptionalExtension;

/// Schema of migration `097_stage_workspaces` (spec §6, component T2).
///
/// `task_branch_counter.last_allocated` is the highest `task-<id>-<n>` number
/// ever handed out for the task. It only grows: a number is spent when it is
/// reserved, before any git work, so neither a failed attempt nor a deleted
/// branch can make it available again.
///
/// `stage_workspace` names the directory each stage ran in and the branch its
/// latest session checked out there. A stage re-entered by a loop reuses its
/// row's directory; a stage whose directory could not be reused gets a new row
/// and the old one stays, so the retained directory is still on record.
pub(super) const STAGE_WORKSPACE_SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS task_branch_counter (
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        last_allocated INTEGER NOT NULL,
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE TABLE IF NOT EXISTS stage_workspace (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL REFERENCES pipeline_item(id) ON DELETE CASCADE,
        stage TEXT NOT NULL,
        path TEXT NOT NULL,
        branch TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_stage_workspace_task_stage
        ON stage_workspace(task_id, stage);
"#;

/// One stage's workspace: the directory, and the branch its latest session
/// checked out there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageWorkspaceRecord {
    pub id: String,
    pub task_id: String,
    pub stage: String,
    pub path: String,
    pub branch: String,
}

impl Db {
    pub fn list_open_task_worktree_paths(&self) -> Result<Vec<(String, String)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT worktree.pipeline_item_id, worktree.path
             FROM worktree
             JOIN pipeline_item ON pipeline_item.id = worktree.pipeline_item_id
             WHERE pipeline_item.closed_at IS NULL
             ORDER BY worktree.created_at DESC, worktree.id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let pipeline_item_id: String = row.get(0)?;
            let path: String = row.get(1)?;
            Ok((pipeline_item_id, path))
        })?;

        let mut paths = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let (pipeline_item_id, path) = row?;
            if seen.insert(pipeline_item_id.clone()) {
                paths.push((pipeline_item_id, path));
            }
        }
        Ok(paths)
    }

    pub fn get_task_worktree_path(
        &self,
        pipeline_item_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT path FROM worktree
                 WHERE pipeline_item_id = ?
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT 1",
                [pipeline_item_id],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn list_worktrees_for_task(
        &self,
        pipeline_item_id: &str,
    ) -> Result<Vec<WorktreeRecord>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT pipeline_item_id, path, branch
             FROM worktree
             WHERE pipeline_item_id = ?
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([pipeline_item_id], |row| {
            Ok(WorktreeRecord {
                pipeline_item_id: row.get(0)?,
                path: row.get(1)?,
                branch: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn list_worktrees_for_repo(
        &self,
        repo_id: &str,
    ) -> Result<Vec<WorktreeRecord>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT worktree.pipeline_item_id, worktree.path, worktree.branch
             FROM worktree
             JOIN pipeline_item ON pipeline_item.id = worktree.pipeline_item_id
             WHERE pipeline_item.repo_id = ?
             ORDER BY worktree.created_at ASC, worktree.id ASC",
        )?;
        let rows = stmt.query_map([repo_id], |row| {
            Ok(WorktreeRecord {
                pipeline_item_id: row.get(0)?,
                path: row.get(1)?,
                branch: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_worktree_rows_for_task(
        &self,
        pipeline_item_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM worktree WHERE pipeline_item_id = ?",
            [pipeline_item_id],
        )?;
        Ok(())
    }

    pub fn delete_worktree_row_for_path(&self, path: &str) -> Result<(), rusqlite::Error> {
        self.conn
            .execute("DELETE FROM worktree WHERE path = ?", [path])?;
        Ok(())
    }

    pub fn upsert_worktree(
        &self,
        id: &str,
        pipeline_item_id: &str,
        path: &str,
        branch: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO worktree (id, pipeline_item_id, path, branch)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               pipeline_item_id = excluded.pipeline_item_id,
               path = excluded.path,
               branch = excluded.branch",
            (id, pipeline_item_id, path, branch),
        )?;
        Ok(())
    }

    /// Record a recreated checkout whose repository setup has not completed.
    /// Ordinary worktree upserts deliberately do not change this bit: only the
    /// recovery lifecycle that created the checkout may settle it.
    pub fn upsert_worktree_with_setup_pending(
        &self,
        id: &str,
        pipeline_item_id: &str,
        path: &str,
        branch: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO worktree (id, pipeline_item_id, path, branch, setup_pending)
             VALUES (?, ?, ?, ?, 1)
             ON CONFLICT(id) DO UPDATE SET
               pipeline_item_id = excluded.pipeline_item_id,
               path = excluded.path,
               branch = excluded.branch,
               setup_pending = 1",
            (id, pipeline_item_id, path, branch),
        )?;
        Ok(())
    }

    pub fn task_worktree_setup_pending(
        &self,
        pipeline_item_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM worktree
                WHERE pipeline_item_id = ? AND setup_pending = 1
             )",
            [pipeline_item_id],
            |row| row.get(0),
        )
    }

    pub fn mark_task_worktree_setup_complete(
        &self,
        pipeline_item_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE worktree SET setup_pending = 0 WHERE pipeline_item_id = ?",
            [pipeline_item_id],
        )?;
        Ok(())
    }

    pub fn upsert_terminal_session(
        &self,
        id: &str,
        repo_id: &str,
        pipeline_item_id: Option<&str>,
        label: Option<&str>,
        cwd: Option<&str>,
        daemon_session_id: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO terminal_session (id, repo_id, pipeline_item_id, label, cwd, daemon_session_id)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               repo_id = excluded.repo_id,
               pipeline_item_id = excluded.pipeline_item_id,
               label = excluded.label,
               cwd = excluded.cwd,
               daemon_session_id = excluded.daemon_session_id",
            (id, repo_id, pipeline_item_id, label, cwd, daemon_session_id),
        )?;
        Ok(())
    }

    pub fn delete_task_creation_artifacts(&self, item_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM terminal_session WHERE pipeline_item_id = ?",
            [item_id],
        )?;
        self.conn
            .execute("DELETE FROM worktree WHERE pipeline_item_id = ?", [item_id])?;
        self.conn.execute(
            "DELETE FROM task_blocker WHERE blocked_item_id = ? OR blocker_item_id = ?",
            (item_id, item_id),
        )?;
        self.conn.execute(
            "DELETE FROM task_port WHERE pipeline_item_id = ?",
            [item_id],
        )?;
        self.conn
            .execute("DELETE FROM pipeline_item WHERE id = ?", [item_id])?;
        Ok(())
    }

    pub fn delete_dormant_task_start_artifacts(
        &self,
        item_id: &str,
        previous_base_ref: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            db.conn.execute(
                "DELETE FROM terminal_session WHERE pipeline_item_id = ?",
                [item_id],
            )?;
            db.conn
                .execute("DELETE FROM worktree WHERE pipeline_item_id = ?", [item_id])?;
            db.conn.execute(
                "DELETE FROM task_port WHERE pipeline_item_id = ?",
                [item_id],
            )?;
            db.conn.execute(
                "UPDATE pipeline_item
                 SET base_ref = ?, port_offset = NULL, port_env = NULL,
                     updated_at = datetime('now')
                 WHERE id = ? AND closed_at IS NULL",
                (previous_base_ref, item_id),
            )?;
            update_open_pipeline_item_activity(&db.conn, item_id, "idle", None, None)?;
            Ok(())
        })
    }

    /// Reserve the task's next branch number. `floor` is the highest number
    /// the caller found already in use (refs, directories, recorded rows);
    /// the reservation is strictly above both it and every number this task
    /// has ever been given, and is durable when this returns.
    pub fn reserve_task_branch_number(
        &self,
        task_id: &str,
        floor: i64,
    ) -> Result<i64, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            let last: Option<i64> = db
                .conn
                .query_row(
                    "SELECT last_allocated FROM task_branch_counter WHERE task_id = ?",
                    [task_id],
                    |row| row.get(0),
                )
                .optional()?;
            let next = last.unwrap_or(0).max(floor) + 1;
            db.conn.execute(
                "INSERT INTO task_branch_counter (task_id, last_allocated)
                 VALUES (?1, ?2)
                 ON CONFLICT(task_id) DO UPDATE SET
                   last_allocated = excluded.last_allocated,
                   updated_at = datetime('now')",
                rusqlite::params![task_id, next],
            )?;
            Ok(next)
        })
    }

    pub fn task_branch_counter(&self, task_id: &str) -> Result<Option<i64>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT last_allocated FROM task_branch_counter WHERE task_id = ?",
                [task_id],
                |row| row.get(0),
            )
            .optional()
    }

    /// Every branch name the task's own records mention: the current branch,
    /// worktree rows, stage workspaces, session branches and the directory
    /// names of recorded run cwds. The counter floor is taken over these and
    /// the repository's refs.
    pub fn task_recorded_branch_names(
        &self,
        task_id: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT branch FROM pipeline_item WHERE id = ?1 AND branch IS NOT NULL
             UNION SELECT branch FROM worktree WHERE pipeline_item_id = ?1
             UNION SELECT path FROM worktree WHERE pipeline_item_id = ?1
             UNION SELECT branch FROM stage_workspace WHERE task_id = ?1
             UNION SELECT path FROM stage_workspace WHERE task_id = ?1
             UNION SELECT session_branch FROM stage_run
                   WHERE task_id = ?1 AND session_branch IS NOT NULL
             UNION SELECT cwd FROM stage_run WHERE task_id = ?1 AND cwd IS NOT NULL",
        )?;
        let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
        rows.map(|row| {
            row.map(|value| {
                // Paths contribute their directory name, which is the branch
                // the workspace was created on.
                std::path::Path::new(&value)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
                    .unwrap_or(value)
            })
        })
        .collect()
    }

    /// Record the workspace a stage session starts in. Called in the same
    /// transaction that moves the task onto it.
    pub fn upsert_stage_workspace(
        &self,
        id: &str,
        task_id: &str,
        stage: &str,
        path: &str,
        branch: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO stage_workspace (id, task_id, stage, path, branch)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               stage = excluded.stage,
               path = excluded.path,
               branch = excluded.branch,
               updated_at = datetime('now')",
            (id, task_id, stage, path, branch),
        )?;
        Ok(())
    }

    /// Every workspace recorded for a task, oldest first.
    pub fn list_stage_workspaces(
        &self,
        task_id: &str,
    ) -> Result<Vec<StageWorkspaceRecord>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, task_id, stage, path, branch FROM stage_workspace
             WHERE task_id = ?
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map([task_id], stage_workspace_from_row)?;
        rows.collect()
    }
}

fn stage_workspace_from_row(
    row: &rusqlite::Row<'_>,
) -> Result<StageWorkspaceRecord, rusqlite::Error> {
    Ok(StageWorkspaceRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        stage: row.get(2)?,
        path: row.get(3)?,
        branch: row.get(4)?,
    })
}
