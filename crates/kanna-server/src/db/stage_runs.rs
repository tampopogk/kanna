use super::{Db, NewStageRun, StageRun, TaskEventKind};
use crate::mutation_provenance::{ChannelIdentity, MutationProvenance};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The `stage_run.kind` values a task's agent sessions produce.
///
/// The table also carries workspace lifecycle runs (`teardown`), which exist
/// only to give a detached `td-{branch}` cleanup session a durable identity for
/// its terminal archive. Such a run is not the task's latest run, carries no
/// stage verdict, answers no `$PREV_RESULT`, and must never be resolved as the
/// run an agent action applies to — so every query that means "this task's
/// runs" scopes itself with this list.
pub(crate) const AGENT_RUN_KINDS: &str = "('main', 'post')";

/// `stage_run.kind` for a workspace teardown session.
pub(crate) const TEARDOWN_RUN_KIND: &str = "teardown";

/// Where a session's provider transcript lives (spec §6): a reference, never
/// an input. `path` is known only for providers whose transcript location is
/// determined by the session id and working directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptRef {
    pub provider: String,
    pub session_id: String,
    #[serde(default)]
    pub path: Option<String>,
}

/// The identity a stage session records when it starts (spec §6, T2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageRunSession {
    /// The `stage_workspace` row, when the workspace is on record.
    pub workspace_id: Option<String>,
    /// The branch this session checked out in its workspace.
    pub branch: Option<String>,
    pub name: Option<String>,
    pub transcript: Option<TranscriptRef>,
    /// Workspace state the start found and preserved rather than resetting
    /// or merging (uncommitted changes, commits the input lacks).
    pub workspace_report: Option<String>,
}

/// Identity of a run closed by `finish_latest_running_stage_run`.
pub struct FinishedStageRun {
    pub kind: String,
    pub completion_transition: Option<String>,
    pub trigger: String,
}

/// How a stage run was entered. Caller-declared labels are recorded without
/// authentication; only `Auto` is server-owned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageTrigger {
    Auto,
    Operator,
    Manager,
    Unspecified,
}

impl StageTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Operator => "operator",
            Self::Manager => "manager",
            Self::Unspecified => "unspecified",
        }
    }

    pub fn from_caller_declared(value: &str) -> Result<Self, String> {
        match value {
            "operator" => Ok(Self::Operator),
            "manager" => Ok(Self::Manager),
            other => Err(format!(
                "unknown stage advance source: {other}; use \"operator\" or \"manager\", or omit it"
            )),
        }
    }
}

/// Who declared a per-advance provider override. Like [`StageTrigger`], every
/// value here is a caller declaration the server records without
/// authenticating it; unlike a trigger there is no server-owned value, because
/// an override only ever exists because somebody asked for one.
///
/// `Agent` is the value that makes this worth recording separately from the
/// trigger: a plan agent recommends a builder tier and a human accepts it by
/// advancing the stage, so the advance is `operator` while the model was
/// picked by the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOverrideSource {
    Operator,
    Manager,
    Agent,
    Unspecified,
}

impl ProviderOverrideSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Manager => "manager",
            Self::Agent => "agent",
            Self::Unspecified => "unspecified",
        }
    }

    pub fn from_caller_declared(value: &str) -> Result<Self, String> {
        match value {
            "operator" => Ok(Self::Operator),
            "manager" => Ok(Self::Manager),
            "agent" => Ok(Self::Agent),
            other => Err(format!(
                "unknown provider override source: {other}; use \"operator\", \"manager\" or \"agent\", or omit it"
            )),
        }
    }
}

/// A provider/model/effort override carried by one explicit stage advance and
/// applied to the stage that advance enters.
///
/// This is the durable record of *who chose the successor stage's model*, kept
/// beside the run's own resolved `agent_provider`/`model`/`effort` because
/// those alone cannot say whether a value was asked for or merely resolved: an
/// override that names only a provider still lets that provider's own lower
/// layers supply the model.
///
/// Model and effort belong to the provider named here and travel with it as
/// one layer — the AGENTS.md rule that a pair is never composed across layers
/// is why `model` and `effort` are meaningless without `provider`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StageProviderOverride {
    /// `operator` | `manager` | `agent` | `unspecified`.
    pub source: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl StageProviderOverride {
    fn to_column(&self) -> Option<String> {
        match serde_json::to_string(self) {
            Ok(encoded) => Some(encoded),
            Err(error) => {
                // Never fail a spawn over its provenance record; a run with an
                // unreadable override reads as one with none, which is what
                // every pre-upgrade row already does.
                log::warn!("failed to encode a stage provider override: {error}");
                None
            }
        }
    }

    fn from_column(stored: Option<String>) -> Option<Self> {
        let stored = stored?;
        match serde_json::from_str(&stored) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                log::warn!("failed to parse a stored stage provider override: {error}");
                None
            }
        }
    }
}

impl Db {
    pub fn has_durable_running_task_session(&self, task_id: &str) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            &format!(
                "SELECT EXISTS(
                SELECT 1
                FROM stage_run sr
                JOIN terminal_session ts
                  ON ts.pipeline_item_id = sr.task_id
                 AND ts.daemon_session_id = sr.session_id
                WHERE sr.task_id = ?
                  AND sr.kind IN {AGENT_RUN_KINDS}
                  AND sr.status = 'running'
                  AND sr.session_id IS NOT NULL
                  AND sr.session_id != ''
            )"
            ),
            [task_id],
            |row| row.get(0),
        )
    }

    pub fn insert_stage_run(&self, run: NewStageRun<'_>) -> Result<(), rusqlite::Error> {
        self.insert_stage_run_with_completion_transition(run, None)
    }

    pub fn insert_stage_run_with_completion_transition(
        &self,
        run: NewStageRun<'_>,
        completion_transition: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.insert_stage_run_with_completion_binding(run, completion_transition, false)
    }

    /// Insert a run whose agent process was spawned with this exact run id in
    /// its completion context. Legacy/pre-upgrade runs deliberately leave the
    /// bit clear so surviving old clients may omit `runId`; a newly spawned
    /// run never takes that compatibility path.
    pub fn insert_stage_run_with_completion_binding(
        &self,
        run: NewStageRun<'_>,
        completion_transition: Option<&str>,
        completion_bound: bool,
    ) -> Result<(), rusqlite::Error> {
        self.insert_stage_run_with_completion_binding_and_trigger(
            run,
            completion_transition,
            completion_bound,
            None,
        )
    }

    pub fn insert_stage_run_with_completion_binding_and_trigger(
        &self,
        run: NewStageRun<'_>,
        completion_transition: Option<&str>,
        completion_bound: bool,
        trigger: Option<StageTrigger>,
    ) -> Result<(), rusqlite::Error> {
        self.insert_stage_run_with_provenance(
            run,
            completion_transition,
            completion_bound,
            trigger,
            None,
            None,
            None,
        )
    }

    /// Insert a run together with the full provenance of how it was started:
    /// its trigger (the declared role of the entry), the channel the entry
    /// arrived on, and the per-advance provider override that picked its
    /// model, if any. `entry_channel` is `None` only for writers that predate
    /// channel identity; it is stored as NULL and reads as unknown.
    ///
    /// A run inserted already carrying a result (a failed spawn, an orphaned
    /// workspace) records that result as this server's own observation.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_stage_run_with_provenance(
        &self,
        run: NewStageRun<'_>,
        completion_transition: Option<&str>,
        completion_bound: bool,
        trigger: Option<StageTrigger>,
        provider_override: Option<&StageProviderOverride>,
        replaces_run_id: Option<&str>,
        entry_channel: Option<&ChannelIdentity>,
    ) -> Result<(), rusqlite::Error> {
        let result_provenance = run.result.map(|_| MutationProvenance::engine());
        self.conn.execute(
            "INSERT INTO stage_run
             (id, task_id, stage, kind, agent, agent_provider, model, effort, status, result, feedback,
              session_id, provider_session_id, cwd, resumed_from_run_id, completion_transition,
              completion_bound, trigger, provider_override, replaces_run_id,
              entry_channel_identity, result_declared_role, result_channel_identity)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                run.id,
                run.task_id,
                run.stage,
                run.kind,
                run.agent,
                run.agent_provider,
                run.model,
                run.effort,
                run.status,
                run.result,
                run.feedback,
                run.session_id,
                run.provider_session_id,
                run.cwd,
                run.resumed_from_run_id,
                completion_transition,
                completion_bound,
                trigger.map(StageTrigger::as_str),
                provider_override.and_then(StageProviderOverride::to_column),
                replaces_run_id,
                entry_channel.map(ChannelIdentity::to_column),
                result_provenance
                    .as_ref()
                    .map(|provenance| provenance.declared_role.as_str()),
                result_provenance
                    .as_ref()
                    .map(|provenance| provenance.channel_identity.to_column()),
            ],
        )?;
        // A pending run has not started anything yet; the watcher wants the
        // moment an agent is actually working. A workspace teardown is not an
        // agent: announcing `run.started` for one would report work nobody is
        // doing, and clearing the task's runtime verdict below would erase the
        // `exited` its agent session just earned — a teardown is spawned
        // immediately after that session was killed.
        if run.status == "running" && run.kind != TEARDOWN_RUN_KIND {
            // A previous session's `exited` verdict describes a session that
            // no longer exists, and a run that is starting proves this task has
            // one again. A post run is injected into the same live session,
            // whose `busy` verdict is still current, so only the terminal value
            // is cleared. The live-session restore path clears it the same way
            // — see `restore_task_run_for_live_session`.
            self.clear_exited_runtime_status(run.task_id)?;
            self.append_task_event(
                run.task_id,
                TaskEventKind::RunStarted,
                json!({
                    "runId": run.id,
                    "stage": run.stage,
                    "kind": run.kind,
                    "agent": run.agent,
                    "agentProvider": run.agent_provider,
                    "declaredRole": trigger.unwrap_or(StageTrigger::Unspecified).as_str(),
                    "channelIdentity": entry_channel.cloned().unwrap_or_default().to_json(),
                }),
            )?;
        }
        Ok(())
    }

    /// A stage that never spawned has no run stamp to supersede. An explicit
    /// execution edit also releases its creation request as a spawn template.
    pub fn workflow_stage_execution_edited(
        &self,
        task_id: &str,
        stage: &str,
    ) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_event, json_each(task_event.payload, '$.changedExecutionStages') AS stage
             WHERE task_event.task_id = ? AND task_event.type = 'task.workflow_changed' AND stage.value = ?)",
            (task_id, stage), |row| row.get(0))
    }

    /// A workflow replacement explicitly invalidates only the old executions
    /// whose stage binding changed. RunStarted and WorkflowChanged are durable
    /// transactionally ordered events; no wall-clock comparison is involved.
    pub fn stage_run_workflow_superseded(
        &self,
        task_id: &str,
        run_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_event, json_each(task_event.payload, '$.supersededRunIds') AS run
             WHERE task_event.task_id = ? AND task_event.type = 'task.workflow_changed' AND run.value = ?)",
            (task_id, run_id), |row| row.get(0))
    }

    #[allow(dead_code)]
    pub fn list_stage_runs_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<StageRun>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, task_id, stage, kind, agent, agent_provider, model, effort, status, result, feedback,
                    session_id, provider_session_id, cwd, resumed_from_run_id,
                    resume_fallback_reason, completion_transition,
                    COALESCE(trigger, 'unspecified'), provider_override, started_at, finished_at, replaces_run_id, no_work_termination,
                    entry_channel_identity, result_declared_role, result_channel_identity
             FROM stage_run
             WHERE task_id = ? AND kind IN {AGENT_RUN_KINDS}
             ORDER BY rowid ASC"
        ))?;
        let rows = stmt.query_map([task_id], stage_run_from_row)?;
        rows.collect()
    }

    pub fn running_stage_runs_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<StageRun>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, task_id, stage, kind, agent, agent_provider, model, effort, status, result, feedback,
                    session_id, provider_session_id, cwd, resumed_from_run_id,
                    resume_fallback_reason, completion_transition,
                    COALESCE(trigger, 'unspecified'), provider_override, started_at, finished_at, replaces_run_id, no_work_termination,
                    entry_channel_identity, result_declared_role, result_channel_identity
             FROM stage_run
             WHERE task_id = ? AND kind IN {AGENT_RUN_KINDS} AND status = 'running'
             ORDER BY rowid ASC"
        ))?;
        let rows = stmt.query_map([task_id], stage_run_from_row)?;
        rows.collect()
    }

    /// Every distinct worktree a task's runs have been recorded in, most
    /// recently used first. A task's work does not stay on one branch —
    /// each stage transition forks a new workspace — so this is how
    /// `task_creator::work_tip` enumerates the branches that might hold the
    /// task's committed tip.
    pub fn task_stage_run_cwds(&self, task_id: &str) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = match self.conn.prepare(&format!(
            "SELECT cwd FROM stage_run
             WHERE task_id = ? AND kind IN {AGENT_RUN_KINDS} AND cwd IS NOT NULL
             GROUP BY cwd
             ORDER BY MAX(rowid) DESC"
        )) {
            Ok(stmt) => stmt,
            Err(err) if is_missing_stage_run_table(&err) => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let rows = stmt.query_map([task_id], |row| row.get::<_, String>(0))?;
        rows.collect()
    }

    /// The most recently started run for a task, regardless of status.
    pub fn latest_stage_run(&self, task_id: &str) -> Result<Option<StageRun>, rusqlite::Error> {
        let run = self
            .conn
            .query_row(
                &format!(
                "SELECT id, task_id, stage, kind, agent, agent_provider, model, effort, status, result,
                        feedback, session_id, provider_session_id, cwd, resumed_from_run_id,
                        resume_fallback_reason, completion_transition,
                        COALESCE(trigger, 'unspecified'), provider_override, started_at, finished_at, replaces_run_id, no_work_termination,
                    entry_channel_identity, result_declared_role, result_channel_identity
                 FROM stage_run
                 WHERE task_id = ? AND kind IN {AGENT_RUN_KINDS}
                 ORDER BY rowid DESC
                 LIMIT 1"
                ),
                [task_id],
                stage_run_from_row,
            )
            .optional();
        match run {
            Ok(run) => Ok(run),
            Err(err) if is_missing_stage_run_table(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The most recently started run of one stage and kind. A rerun uses this
    /// to reproduce the run it is replacing (its provider, model, and effort)
    /// instead of re-deriving the stage's defaults.
    pub fn latest_stage_run_for_stage(
        &self,
        task_id: &str,
        stage: &str,
        kind: &str,
    ) -> Result<Option<StageRun>, rusqlite::Error> {
        let run = self
            .conn
            .query_row(
                "SELECT id, task_id, stage, kind, agent, agent_provider, model, effort, status, result,
                        feedback, session_id, provider_session_id, cwd, resumed_from_run_id,
                        resume_fallback_reason, completion_transition,
                        COALESCE(trigger, 'unspecified'), provider_override, started_at, finished_at, replaces_run_id, no_work_termination,
                    entry_channel_identity, result_declared_role, result_channel_identity
                 FROM stage_run
                 WHERE task_id = ? AND stage = ? AND kind = ?
                 ORDER BY rowid DESC
                 LIMIT 1",
                [task_id, stage, kind],
                stage_run_from_row,
            )
            .optional();
        match run {
            Ok(run) => Ok(run),
            Err(err) if is_missing_stage_run_table(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn stage_run(&self, run_id: &str) -> Result<Option<StageRun>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT id, task_id, stage, kind, agent, agent_provider, model, effort, status, result,
                        feedback, session_id, provider_session_id, cwd, resumed_from_run_id,
                        resume_fallback_reason, completion_transition,
                        COALESCE(trigger, 'unspecified'), provider_override, started_at, finished_at, replaces_run_id, no_work_termination,
                    entry_channel_identity, result_declared_role, result_channel_identity
                 FROM stage_run WHERE id = ?",
                [run_id],
                stage_run_from_row,
            )
            .optional()
    }

    /// The most recent main run of `stage` whose provider session could be
    /// resumed: it recorded both the agent CLI's own session id and the
    /// worktree it ran in. Whether resumption is actually possible (worktree
    /// still on disk, transcript present, tips match) is the caller's check.
    pub fn latest_resumable_stage_run(
        &self,
        task_id: &str,
        stage: &str,
    ) -> Result<Option<StageRun>, rusqlite::Error> {
        let run = self
            .conn
            .query_row(
                "SELECT id, task_id, stage, kind, agent, agent_provider, model, effort, status, result,
                        feedback, session_id, provider_session_id, cwd, resumed_from_run_id,
                        resume_fallback_reason, completion_transition,
                        COALESCE(trigger, 'unspecified'), provider_override, started_at, finished_at, replaces_run_id, no_work_termination,
                    entry_channel_identity, result_declared_role, result_channel_identity
                 FROM stage_run
                 WHERE task_id = ? AND stage = ? AND kind = 'main'
                   AND provider_session_id IS NOT NULL AND cwd IS NOT NULL
                 ORDER BY rowid DESC
                 LIMIT 1",
                [task_id, stage],
                stage_run_from_row,
            )
            .optional();
        match run {
            Ok(run) => Ok(run),
            Err(err) if is_missing_stage_run_table(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Context-less keys belong to the task, not whichever run is live on retry.
    pub(crate) fn contextless_completion_attempt(
        &self,
        task_id: &str,
        attempt_key: &str,
    ) -> Result<Option<(String, String)>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT run_id, result FROM contextless_completion_attempt
             WHERE task_id = ? AND attempt_key = ?",
                (task_id, attempt_key),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    pub(crate) fn record_contextless_completion_attempt(
        &self,
        attempt_key: &str,
        run_id: &str,
        result: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT INTO contextless_completion_attempt (task_id, attempt_key, run_id, result)
             SELECT task_id, ?, id, ? FROM stage_run WHERE id = ?",
            (attempt_key, result, run_id),
        )?;
        Ok(())
    }

    /// Commit the retry identity and verdict together, including run.finished.
    pub(crate) fn finish_contextless_stage_run(
        &self,
        attempt_key: &str,
        run_id: &str,
        status: &str,
        result: &str,
        summary: &str,
        provenance: &MutationProvenance,
    ) -> Result<(), rusqlite::Error> {
        self.in_immediate_transaction_if_needed(|db| {
            db.finish_stage_run_with_provenance(
                run_id,
                status,
                Some(result),
                Some(summary),
                provenance,
            )?;
            db.record_contextless_completion_attempt(attempt_key, run_id, result)
        })
    }

    /// Close a run on a verdict this server reached itself (a live session
    /// handing its run to a post, a test fixture). A verdict a caller
    /// submitted goes through [`Self::finish_stage_run_with_provenance`].
    pub fn finish_stage_run(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
        feedback: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        self.finish_stage_run_inner(
            id,
            status,
            result,
            feedback,
            None,
            &MutationProvenance::engine(),
        )
    }

    /// Close a run on a verdict a caller submitted, recording who declared it
    /// and the channel it arrived on beside the result. The run's entry
    /// provenance is a different mutation and is left untouched.
    pub fn finish_stage_run_with_provenance(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
        feedback: Option<&str>,
        provenance: &MutationProvenance,
    ) -> Result<(), rusqlite::Error> {
        self.finish_stage_run_inner(id, status, result, feedback, None, provenance)
    }

    /// Close a run that recorded no agent or task verdict, declaring why.
    ///
    /// `kind` comes from [`super::no_work_termination`]. It is written to its
    /// own column rather than to `feedback`, which the rejected-resume and
    /// quota producers must leave carrying a resumed revision's requested
    /// changes.
    pub fn finish_stage_run_without_work(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
        feedback: Option<&str>,
        kind: &str,
    ) -> Result<(), rusqlite::Error> {
        self.finish_stage_run_inner(
            id,
            status,
            result,
            feedback,
            Some(kind),
            &MutationProvenance::engine(),
        )
    }

    fn finish_stage_run_inner(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
        feedback: Option<&str>,
        no_work_termination: Option<&str>,
        provenance: &MutationProvenance,
    ) -> Result<(), rusqlite::Error> {
        // Provenance describes the result, so a close that records none
        // leaves the columns empty rather than attributing nothing to someone.
        let result_provenance = result.map(|_| provenance);
        let identity = self
            .conn
            .query_row(
                "SELECT task_id, stage, kind FROM stage_run WHERE id = ?",
                [id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let rows_affected = self.conn.execute(
            "UPDATE stage_run
             SET status = ?, result = ?, feedback = ?, no_work_termination = ?,
                 result_declared_role = ?, result_channel_identity = ?,
                 finished_at = datetime('now')
             WHERE id = ?",
            params![
                status,
                result,
                feedback,
                no_work_termination,
                result_provenance.map(|provenance| provenance.declared_role.as_str()),
                result_provenance.map(|provenance| provenance.channel_identity.to_column()),
                id,
            ],
        )?;
        if rows_affected == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        // `run.finished` is a fact about a task's agent: subscribers treat a
        // non-succeeded one as urgent and enrich it with the task's latest
        // run. A workspace teardown has neither an agent nor a verdict, so
        // closing one publishes nothing — its record is the run row and the
        // terminal archive bound to it.
        if let Some((task_id, stage, kind)) =
            identity.filter(|(_, _, kind)| kind != TEARDOWN_RUN_KIND)
        {
            self.append_task_event(
                &task_id,
                TaskEventKind::RunFinished,
                json!({
                    "runId": id,
                    "stage": stage,
                    "kind": kind,
                    "status": status,
                    "result": result,
                    "declaredRole": result_provenance.map(|provenance| provenance.declared_role.as_str()),
                    "channelIdentity": result_provenance.map(|provenance| provenance.channel_identity.to_json()),
                }),
            )?;
        }
        Ok(())
    }

    /// Daemon session ids of every session this task's records place in a
    /// workspace directory — agent, post and teardown runs recorded there,
    /// runs whose session named the directory's workspace, and terminal
    /// sessions opened in it — whatever branch the directory is on now.
    pub fn task_session_ids_in_directory(
        &self,
        task_id: &str,
        directory: &str,
        workspace_id: &str,
    ) -> Result<Vec<String>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM stage_run
             WHERE task_id = ?1 AND session_id IS NOT NULL
               AND (cwd = ?2 OR workspace_id = ?3)
             UNION
             SELECT daemon_session_id FROM terminal_session
             WHERE pipeline_item_id = ?1 AND cwd = ?2 AND daemon_session_id IS NOT NULL",
        )?;
        let rows = stmt.query_map(rusqlite::params![task_id, directory, workspace_id], |row| {
            row.get::<_, String>(0)
        })?;
        rows.collect()
    }

    /// Teardown runs of a task still recorded as running in a workspace
    /// directory, with their session ids.
    pub fn running_teardown_runs_in_directory(
        &self,
        task_id: &str,
        directory: &str,
    ) -> Result<Vec<(String, Option<String>)>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id, session_id FROM stage_run
             WHERE task_id = ?1 AND kind = '{TEARDOWN_RUN_KIND}' AND cwd = ?2
               AND status = 'running'"
        ))?;
        let rows = stmt.query_map(rusqlite::params![task_id, directory], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        rows.collect()
    }

    /// Record the identity a session started with (spec §6): the stage
    /// workspace it runs in, the branch it checked out there, its name, where
    /// its provider transcript lives, and any workspace state the start
    /// preserved rather than touched. Written once, beside the run row.
    pub fn set_stage_run_session(
        &self,
        run_id: &str,
        session: &StageRunSession,
    ) -> Result<(), rusqlite::Error> {
        let rows_affected = self.conn.execute(
            "UPDATE stage_run
             SET workspace_id = ?, session_branch = ?, session_name = ?,
                 transcript_ref = ?, workspace_report = ?
             WHERE id = ?",
            rusqlite::params![
                session.workspace_id,
                session.branch,
                session.name,
                session
                    .transcript
                    .as_ref()
                    .map(|transcript| serde_json::to_string(transcript).unwrap_or_default()),
                session.workspace_report,
                run_id,
            ],
        )?;
        if rows_affected == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// The session identity recorded on a run, or `None` for a run that
    /// predates it (or a teardown run, which is not a session).
    pub fn stage_run_session(
        &self,
        run_id: &str,
    ) -> Result<Option<StageRunSession>, rusqlite::Error> {
        let row = self
            .conn
            .query_row(
                "SELECT workspace_id, session_branch, session_name, transcript_ref, workspace_report
                 FROM stage_run WHERE id = ?",
                [run_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional();
        let row = match row {
            Ok(row) => row,
            Err(err) if is_missing_stage_run_table(&err) => return Ok(None),
            Err(err) => return Err(err),
        };
        Ok(row.and_then(
            |(workspace_id, branch, name, transcript, workspace_report)| {
                if workspace_id.is_none() && branch.is_none() && name.is_none() {
                    return None;
                }
                Some(StageRunSession {
                    workspace_id,
                    branch,
                    name,
                    transcript: transcript
                        .as_deref()
                        .and_then(|value| serde_json::from_str(value).ok()),
                    workspace_report,
                })
            },
        ))
    }

    pub fn set_stage_run_resume_fallback_reason(
        &self,
        run_id: &str,
        reason: &str,
    ) -> Result<(), rusqlite::Error> {
        let rows_affected = self.conn.execute(
            "UPDATE stage_run SET resume_fallback_reason = ? WHERE id = ?",
            (reason, run_id),
        )?;
        if rows_affected == 0 {
            return Err(rusqlite::Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// Persist a provider session id learned after spawn (for example Codex's
    /// terminal footer) on both the task and its latest run. The run record is
    /// the durable resume source; the `pipeline_item` field remains the legacy
    /// current-session mirror.
    pub fn update_latest_stage_run_provider_session_id(
        &self,
        task_id: &str,
        provider_session_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            &format!(
                "UPDATE stage_run
             SET provider_session_id = ?
             WHERE id = (
               SELECT id FROM stage_run
               WHERE task_id = ? AND kind IN {AGENT_RUN_KINDS}
               ORDER BY rowid DESC
               LIMIT 1
             )"
            ),
            (provider_session_id, task_id),
        )?;
        self.update_pipeline_item_agent_session_id(task_id, Some(provider_session_id))
    }

    /// The most recent `main` run a daemon session served for a task. A stage
    /// transition respawns the same session id for the next stage, so the run
    /// — not the session — is the identity a provider session belongs to, and
    /// a killer must resolve it before its replacement run is inserted. The
    /// stage's post shares the session id but is never the run a revision
    /// reopens, so it is deliberately skipped.
    pub fn latest_main_stage_run_id_for_session(
        &self,
        task_id: &str,
        session_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        let run_id = self
            .conn
            .query_row(
                "SELECT id FROM stage_run
                 WHERE task_id = ? AND session_id = ? AND kind = 'main'
                 ORDER BY rowid DESC
                 LIMIT 1",
                (task_id, session_id),
                |row| row.get::<_, String>(0),
            )
            .optional();
        match run_id {
            Ok(run_id) => Ok(run_id),
            Err(err) if is_missing_stage_run_table(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Record a provider session id discovered as a session ended, on the exact
    /// run that session was serving. Returns whether it was recorded.
    ///
    /// The write is fenced on the run still naming that session and on having
    /// no provider session of its own: a delayed `Exit` from a replaced
    /// incarnation must never overwrite the id a later session recorded for
    /// the same run. Unlike the natural-exit path this deliberately leaves
    /// `pipeline_item.agent_session_id` alone — an orchestrated kill retires
    /// the outgoing session, and the task's current session is its
    /// replacement, which sets that mirror itself when it spawns.
    pub fn record_stage_run_provider_session_id(
        &self,
        run_id: &str,
        session_id: &str,
        provider_session_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        let updated = self.conn.execute(
            "UPDATE stage_run
             SET provider_session_id = ?
             WHERE id = ? AND session_id = ? AND provider_session_id IS NULL",
            (provider_session_id, run_id, session_id),
        )?;
        Ok(updated > 0)
    }

    /// Restore a run that the terminal-loss path marked interrupted when the
    /// daemon proves the original session is still alive. The feedback marker
    /// identifies current no-verdict interruptions; legacy bare cancellations
    /// are also recoverable. A live-but-failed agent verdict is never reopened.
    pub fn restore_latest_interrupted_stage_run(
        &self,
        task_id: &str,
        interruption_feedback: &str,
    ) -> Result<bool, rusqlite::Error> {
        let transaction = self.conn.unchecked_transaction()?;
        let run_id = transaction
            .query_row(
                &format!(
                    "SELECT sr.id
                 FROM stage_run sr
                 JOIN pipeline_item p ON p.id = sr.task_id
                 WHERE sr.task_id = ?
                   AND p.closed_at IS NULL
                   AND sr.kind IN {AGENT_RUN_KINDS}
                   AND sr.id = (
                     SELECT latest.id
                     FROM stage_run latest
                     WHERE latest.task_id = sr.task_id
                       AND latest.kind IN {AGENT_RUN_KINDS}
                     ORDER BY latest.rowid DESC
                     LIMIT 1
                   )
                   AND sr.status IN ('cancelled', 'failed')
                   AND (
                     sr.no_work_termination = ?
                     OR
                     sr.feedback = ?
                     OR (sr.status = 'cancelled' AND sr.result IS NULL AND sr.feedback IS NULL)
                   )
                 ORDER BY sr.rowid DESC
                 LIMIT 1"
                ),
                (
                    task_id,
                    super::no_work_termination::SESSION_INTERRUPTED,
                    interruption_feedback,
                ),
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(run_id) = run_id else {
            transaction.commit()?;
            return Ok(false);
        };
        let rows_affected = transaction.execute(
            "UPDATE stage_run
             SET status = 'running', result = NULL,
                 result_declared_role = NULL, result_channel_identity = NULL,
                 feedback = CASE WHEN feedback = ? THEN NULL ELSE feedback END,
                 no_work_termination = NULL, finished_at = NULL
             WHERE id = ?
               AND status IN ('cancelled', 'failed')
               AND (
                 no_work_termination = ?
                 OR
                 feedback = ?
                 OR (status = 'cancelled' AND result IS NULL AND feedback IS NULL)
               )",
            (
                interruption_feedback,
                &run_id,
                super::no_work_termination::SESSION_INTERRUPTED,
                interruption_feedback,
            ),
        )?;
        transaction.commit()?;
        Ok(rows_affected > 0)
    }

    /// The running workspace-teardown run a daemon session is serving, if any.
    ///
    /// A `td-{branch}` session id resolves to no task, so the exit handler
    /// cannot reach this run the way it reaches an agent's. Looked up by
    /// session id and kind, which together name exactly one live cleanup.
    pub fn running_teardown_stage_run_for_session(
        &self,
        session_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT id FROM stage_run
                 WHERE session_id = ? AND kind = ? AND status = 'running'
                 ORDER BY rowid DESC
                 LIMIT 1",
                (session_id, TEARDOWN_RUN_KIND),
                |row| row.get(0),
            )
            .optional()
    }

    pub fn stage_run_completion_bound(&self, run_id: &str) -> Result<bool, rusqlite::Error> {
        self.conn.query_row(
            "SELECT completion_bound != 0 FROM stage_run WHERE id = ?",
            [run_id],
            |row| row.get(0),
        )
    }

    /// Finish the task's most recent `running` run, returning its kind so
    /// callers can tell whether a main run or a post completed.
    /// Returns `Ok(None)` without writing when no run is running.
    pub fn finish_latest_running_stage_run(
        &self,
        task_id: &str,
        status: &str,
        result: Option<&str>,
        feedback: Option<&str>,
    ) -> Result<Option<FinishedStageRun>, rusqlite::Error> {
        let run_result = self
            .conn
            .query_row(
                &format!(
                    "SELECT id, kind, completion_transition, COALESCE(trigger, 'unspecified'), feedback
                 FROM stage_run
                 WHERE task_id = ? AND kind IN {AGENT_RUN_KINDS} AND status = 'running'
                 ORDER BY rowid DESC
                 LIMIT 1"
                ),
                [task_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional();
        let run = match run_result {
            Ok(run) => run,
            Err(err) if is_missing_stage_run_table(&err) => return Ok(None),
            Err(err) => return Err(err),
        };
        let Some((run_id, kind, completion_transition, trigger, existing_feedback)) = run else {
            return Ok(None);
        };
        // The session died mid-turn: no verdict was recorded here. A run with
        // no pre-existing feedback keeps the legacy marker, while the
        // dedicated producer provenance lets restore and lineage code find
        // either shape without overloading the directive itself.
        // A revision run already carries the reviewer's requested changes in
        // `feedback`. Session loss is bookkeeping, not a new instruction, so
        // retain that producer-authored directive and put the interruption
        // provenance in its dedicated column. Runs without an existing
        // directive keep the legacy marker for old readers and diagnostics.
        let feedback = existing_feedback.as_deref().or(feedback);
        self.finish_stage_run_without_work(
            &run_id,
            status,
            result,
            feedback,
            super::no_work_termination::SESSION_INTERRUPTED,
        )?;
        Ok(Some(FinishedStageRun {
            kind,
            completion_transition,
            trigger,
        }))
    }

    /// Set a run's replacement lineage directly. Tests need to build the
    /// shapes production writes across several runs without driving every
    /// producer that would have written them.
    #[cfg(test)]
    pub fn set_test_stage_run_replaces_run_id(
        &self,
        run_id: &str,
        replaces_run_id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE stage_run SET replaces_run_id = ? WHERE id = ?",
            (replaces_run_id, run_id),
        )?;
        Ok(())
    }

    pub fn cancel_running_stage_runs(&self, task_id: &str) -> Result<(), rusqlite::Error> {
        match self.conn.execute(
            "UPDATE stage_run
             SET status = 'cancelled', finished_at = COALESCE(finished_at, datetime('now'))
             WHERE task_id = ? AND status IN ('pending', 'running')",
            [task_id],
        ) {
            Ok(_) => {}
            Err(err) if is_missing_stage_run_table(&err) => return Ok(()),
            Err(err) => return Err(err),
        }
        Ok(())
    }

    /// The task's most recently finished run result, whatever its kind. This
    /// is what `$PREV_RESULT` binds to, so for a stage whose predecessor
    /// declares a post it is the *post's* result (e.g. the commit agent's),
    /// not the stage agent's.
    pub fn latest_finished_stage_run_result(
        &self,
        task_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.latest_finished_stage_run_result_of_kind(task_id, None)
    }

    /// The task's most recently finished **main** run result, skipping posts.
    /// A stage that needs what the previous stage's own agent reported — the
    /// implementer's summary, including work it declined — must use this:
    /// `latest_finished_stage_run_result` would hand it the commit post's
    /// result instead, silently losing that report.
    pub fn latest_finished_main_stage_run_result(
        &self,
        task_id: &str,
    ) -> Result<Option<String>, rusqlite::Error> {
        self.latest_finished_stage_run_result_of_kind(task_id, Some("main"))
    }

    /// Every finished run this task has recorded locally, oldest first. A
    /// transfer export uses this to carry the task's own contribution to the
    /// ordered stage/main/post/revision history, appended after whatever it
    /// itself inherited from an earlier hop.
    pub fn finished_stage_runs(&self, task_id: &str) -> Result<Vec<StageRun>, rusqlite::Error> {
        let mut stmt = match self.conn.prepare(&format!(
            "SELECT id, task_id, stage, kind, agent, agent_provider, model, effort, status, result,
                    feedback, session_id, provider_session_id, cwd, resumed_from_run_id,
                    resume_fallback_reason, completion_transition,
                    COALESCE(trigger, 'unspecified'), provider_override, started_at, finished_at,
                    replaces_run_id, no_work_termination,
                    entry_channel_identity, result_declared_role, result_channel_identity
             FROM stage_run
             WHERE task_id = ? AND kind IN {AGENT_RUN_KINDS}
               AND status IN ('succeeded', 'failed')
             ORDER BY rowid ASC"
        )) {
            Ok(stmt) => stmt,
            Err(err) if is_missing_stage_run_table(&err) => return Ok(Vec::new()),
            Err(err) => return Err(err),
        };
        let rows = stmt.query_map([task_id], stage_run_from_row)?;
        rows.collect()
    }

    fn latest_finished_stage_run_result_of_kind(
        &self,
        task_id: &str,
        kind: Option<&str>,
    ) -> Result<Option<String>, rusqlite::Error> {
        let result = self
            .conn
            .query_row(
                &format!(
                    "SELECT result
                 FROM stage_run
                 WHERE task_id = ?
                   AND kind IN {AGENT_RUN_KINDS}
                   AND status IN ('succeeded', 'failed')
                   AND result IS NOT NULL
                   AND (?2 IS NULL OR kind = ?2)
                 ORDER BY rowid DESC
                 LIMIT 1"
                ),
                rusqlite::params![task_id, kind],
                |row| row.get(0),
            )
            .optional();
        match result {
            Ok(result) => Ok(result),
            Err(err) if is_missing_stage_run_table(&err) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

fn is_missing_stage_run_table(err: &rusqlite::Error) -> bool {
    matches!(err, rusqlite::Error::SqliteFailure(_, Some(message)) if message.contains("no such table: stage_run"))
}

fn stage_run_from_row(row: &rusqlite::Row<'_>) -> Result<StageRun, rusqlite::Error> {
    let result: Option<String> = row.get(9)?;
    Ok(StageRun {
        id: row.get(0)?,
        task_id: row.get(1)?,
        stage: row.get(2)?,
        kind: row.get(3)?,
        agent: row.get(4)?,
        agent_provider: row.get(5)?,
        model: row.get(6)?,
        effort: row.get(7)?,
        status: row.get(8)?,
        result: result.clone(),
        feedback: row.get(10)?,
        session_id: row.get(11)?,
        provider_session_id: row.get(12)?,
        cwd: row.get(13)?,
        resumed_from_run_id: row.get(14)?,
        resume_fallback_reason: row.get(15)?,
        completion_transition: row.get(16)?,
        trigger: row.get(17)?,
        provider_override: StageProviderOverride::from_column(row.get(18)?),
        started_at: row.get(19)?,
        finished_at: row.get(20)?,
        replaces_run_id: row.get(21)?,
        no_work_termination: row.get(22)?,
        entry_channel_identity: ChannelIdentity::from_column(
            row.get::<_, Option<String>>(23)?.as_deref(),
        ),
        result_provenance: result_provenance_from_columns(
            result.is_some(),
            row.get(24)?,
            row.get::<_, Option<String>>(25)?.as_deref(),
        ),
    })
}

/// A run's result provenance, present exactly when it recorded a result. A
/// result written before provenance existed keeps no label to recover, so it
/// reads as an undeclared role on an unknown channel.
fn result_provenance_from_columns(
    has_result: bool,
    declared_role: Option<String>,
    channel_identity: Option<&str>,
) -> Option<MutationProvenance> {
    has_result.then(|| {
        MutationProvenance::new(
            declared_role.unwrap_or_else(|| "unspecified".to_string()),
            ChannelIdentity::from_column(channel_identity),
        )
    })
}
