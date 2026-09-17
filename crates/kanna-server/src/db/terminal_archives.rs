//! Terminal launch bindings and immutable final frames.
//!
//! A row exists for every PTY launch Kanna bound a run id to: a stage's agent
//! (`kind='main'`) and the workspace teardown session that cleans a departed
//! workspace up (`kind='teardown'`). Stage posts are not launches — a post
//! continues the main run's session and has no terminal of its own.
use super::stage_runs::TEARDOWN_RUN_KIND;
use super::Db;
use kanna_daemon::protocol::TerminalAttemptArchive;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTerminalAttempt {
    pub id: String,
    pub stage: String,
    /// `stage_run.kind`: `main` for the stage's agent session, `teardown` for
    /// the workspace cleanup that ran when the task left that workspace. The
    /// label belongs to the workspace/stage the run acted on, never to the
    /// stage being entered.
    pub kind: String,
    pub started_at: String,
    pub cwd: Option<String>,
    pub archived: bool,
    pub recorded_launch: bool,
    pub observed_exit_code: Option<i32>,
}
impl Db {
    pub fn bind_agent_terminal_attempt(&self, run_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO agent_terminal_attempt (run_id) VALUES (?) ON CONFLICT DO NOTHING",
            [run_id],
        )?;
        Ok(())
    }
    pub fn agent_terminal_attempts(
        &self,
        task_id: &str,
    ) -> Result<Vec<AgentTerminalAttempt>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT sr.id, sr.stage, sr.kind, sr.started_at, sr.cwd, a.archive, a.run_id IS NOT NULL
            FROM stage_run sr LEFT JOIN agent_terminal_attempt a ON a.run_id=sr.id
            WHERE sr.task_id=? AND (a.run_id IS NOT NULL OR sr.kind IN ('main', '{TEARDOWN_RUN_KIND}'))
            ORDER BY sr.rowid ASC"
        ))?;
        let rows = stmt
            .query_map([task_id], |row| {
                let payload: Option<String> = row.get(5)?;
                let archive = payload
                    .as_deref()
                    .and_then(|p| serde_json::from_str::<TerminalAttemptArchive>(p).ok());
                Ok(AgentTerminalAttempt {
                    id: row.get(0)?,
                    stage: row.get(1)?,
                    kind: row.get(2)?,
                    started_at: row.get(3)?,
                    cwd: row.get(4)?,
                    archived: archive.as_ref().is_some_and(|a| a.snapshot.is_some()),
                    recorded_launch: row.get(6)?,
                    observed_exit_code: archive.and_then(|a| a.observed_exit_code),
                })
            })?
            .collect();
        rows
    }
    pub fn agent_terminal_archive(
        &self,
        task_id: &str,
        run_id: &str,
    ) -> Result<Option<TerminalAttemptArchive>, String> {
        let payload: Option<String> = self.conn.query_row("SELECT a.archive FROM agent_terminal_attempt a JOIN stage_run sr ON sr.id=a.run_id WHERE sr.task_id=? AND a.run_id=?", params![task_id,run_id], |r| r.get(0)).optional().map_err(|e|e.to_string())?.flatten();
        payload
            .map(|p| serde_json::from_str(&p).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn ingest_agent_terminal_archive(
        &self,
        task_id: &str,
        run_id: &str,
        archive: &TerminalAttemptArchive,
    ) -> Result<(), String> {
        if archive.binding.task_id != task_id || archive.binding.spawned_run_id != run_id {
            return Err("terminal archive binding mismatch".into());
        }
        self.with_immediate_transaction(|db| {
            let binding: Option<(String,Option<String>,Option<String>)> = db.conn.query_row("SELECT sr.task_id,sr.session_id,sr.cwd FROM stage_run sr JOIN agent_terminal_attempt a ON sr.id=a.run_id WHERE sr.id=?", [run_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if !binding.is_some_and(|(owner,session,cwd)| owner==task_id && session.as_deref()==Some(archive.session_id.as_str()) && cwd.as_deref()==Some(archive.cwd.as_str())) { return Err(rusqlite::Error::InvalidQuery); }
            let payload=serde_json::to_string(archive).map_err(|_|rusqlite::Error::InvalidQuery)?;
            let existing: Option<String> = db.conn.query_row("SELECT archive FROM agent_terminal_attempt WHERE run_id=?", [run_id], |r|r.get(0))?;
            if let Some(existing)=existing {
                if existing!=payload { return Err(rusqlite::Error::InvalidQuery); }
            } else {
                db.conn.execute("UPDATE agent_terminal_attempt SET archive=? WHERE run_id=? AND archive IS NULL", params![payload,run_id])?;
            }
            Ok(())
        }).map_err(|e|e.to_string())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use kanna_daemon::protocol::{TerminalAttemptBinding, TerminalSnapshot};
    pub(crate) fn seed(db: &Db) {
        db.insert_test_repo("repo", "Repo").unwrap();
        for task in ["task-a", "task-b"] {
            db.insert_test_pipeline_item(
                task,
                "repo",
                "prompt",
                None,
                "review",
                "2026-09-13 00:00:00",
            )
            .unwrap();
        }
        for (run, kind) in [
            ("run-task-a-1", "main"),
            ("run-task-a-2", "main"),
            ("post-task-a", "post"),
            ("legacy-task-a", "main"),
        ] {
            db.conn.execute("INSERT INTO stage_run (id,task_id,stage,kind,status,session_id,cwd,started_at) VALUES (?,'task-a','review',?,'succeeded','task-a','/work','2026-09-13 00:00:00')",params![run,kind]).unwrap();
        }
        db.bind_agent_terminal_attempt("run-task-a-1").unwrap();
        db.bind_agent_terminal_attempt("run-task-a-2").unwrap();
    }
    pub(crate) fn seed_at(db: &Db, cwd: &str) {
        seed(db);
        db.conn
            .execute("UPDATE stage_run SET cwd=?", [cwd])
            .unwrap();
    }
    pub(crate) fn archive() -> TerminalAttemptArchive {
        TerminalAttemptArchive {
            binding: TerminalAttemptBinding {
                task_id: "task-a".into(),
                spawned_run_id: "run-task-a-1".into(),
            },
            session_id: "task-a".into(),
            cwd: "/work".into(),
            snapshot: Some(TerminalSnapshot {
                version: 1,
                vt: format!(
                    "FIRST\r\n{}LAST",
                    "\u{1b}[32mcafé\u{1b}[0m\r\n".repeat(30000)
                ),
                cols: 120,
                rows: 24,
                cursor_row: 0,
                cursor_col: 0,
                cursor_visible: true,
                saved_at: 1,
                sequence: 2,
            }),
            unavailable_reason: None,
            observed_exit_code: Some(0),
        }
    }
    #[test]
    fn attempt_archives_are_owned_immutable_and_persist_across_reopen() {
        let path = Db::test_db_path("attempt-archives");
        let db = Db::open_for_tests(&path).unwrap();
        seed(&db);
        let a = archive();
        db.ingest_agent_terminal_archive("task-a", "run-task-a-1", &a)
            .unwrap();
        db.ingest_agent_terminal_archive("task-a", "run-task-a-1", &a)
            .unwrap();
        let mut other = a.clone();
        other.observed_exit_code = None;
        assert!(db
            .ingest_agent_terminal_archive("task-a", "run-task-a-1", &other)
            .is_err());
        assert!(db
            .ingest_agent_terminal_archive("task-b", "run-task-a-1", &a)
            .is_err());
        assert!(db
            .agent_terminal_archive("task-b", "run-task-a-1")
            .unwrap()
            .is_none());
        assert!(db
            .agent_terminal_archive("task-a", "run-task-a-2")
            .unwrap()
            .is_none());
        let rows = db.agent_terminal_attempts("task-a").unwrap();
        assert_eq!(rows.len(), 3);
        assert!(!rows[2].recorded_launch);
        drop(db);
        let db = Db::open(&path).unwrap();
        assert_eq!(
            serde_json::to_value(db.agent_terminal_archive("task-a", "run-task-a-1").unwrap())
                .unwrap(),
            serde_json::to_value(Some(a)).unwrap()
        );
        db.conn.execute_batch("CREATE TRIGGER reject_archive BEFORE UPDATE ON agent_terminal_attempt BEGIN SELECT RAISE(ABORT,'archive write failure'); END;").unwrap();
        other.binding.spawned_run_id = "run-task-a-2".into();
        assert!(db
            .ingest_agent_terminal_archive("task-a", "run-task-a-2", &other)
            .is_err());
        assert!(db
            .agent_terminal_archive("task-a", "run-task-a-2")
            .unwrap()
            .is_none());
    }
}
