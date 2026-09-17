//! The durable record of a provider refusing a turn because the model a run
//! selected is at capacity.
//!
//! A separate table from [`crate::db::provider_rejections`] on purpose, and
//! the separation is the point rather than a filing preference. A spent
//! allowance burns a candidate: quota recovery reads
//! `providers_rejected_at_stage` to decide what a stage has left to try, and a
//! rerun walks around whatever is named there. Capacity is transient — nothing
//! is spent, nothing has to reset, and the recovery is to try the turn again —
//! so a capacity refusal that landed in that table would silently retire a
//! provider a stage still has every right to use. Nothing in quota recovery
//! reads this table, and that is what keeps the two claims apart.
//!
//! One row per stage run per provider per stated scope, exactly like a
//! rejection: the daemon latches its own announcement per session incarnation,
//! and a replayed or re-adopted announcement lands on the row that already
//! exists instead of announcing a second refusal.

use rusqlite::{params, OptionalExtension};

use super::{Db, QuotaRejectionSource};

/// One capacity refusal, as it is written.
#[derive(Debug, Clone)]
pub struct NewProviderCapacityNotice<'a> {
    pub task_id: &'a str,
    pub stage_run_id: &'a str,
    pub stage: &'a str,
    pub provider: &'a str,
    /// The model the run selected. The one measured chrome refuses *that*
    /// model without naming it, so this is the identity of what was refused —
    /// read from the run Kanna started, never guessed from the sentence.
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    /// Which surface stated it, `pty` or `sdk`. The vocabulary is shared with
    /// a rejection deliberately — it answers "where was this observed", which
    /// is the same question for both — while nothing else about the two
    /// records is.
    pub source: QuotaRejectionSource,
    pub rule_id: &'a str,
    /// The provider's own wording, so the claim can be checked against the
    /// pattern that produced it rather than believed.
    pub matched_text: &'a str,
    /// The scope the provider itself named, when it named one. `None` is "the
    /// CLI did not say", which is never "this provider is unavailable".
    pub scope: Option<&'a str>,
    pub cli_version: Option<&'a str>,
}

/// One capacity refusal, as it is read back.
#[derive(Debug, Clone)]
pub struct ProviderCapacityNotice {
    pub stage_run_id: String,
    pub stage: String,
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub source: String,
    pub rule_id: String,
    pub matched_text: String,
    pub scope: Option<String>,
    pub cli_version: Option<String>,
    pub observed_at: String,
}

impl Db {
    /// Record one capacity refusal, or report that this one is already
    /// recorded.
    ///
    /// `Ok(None)` means a row for this (run, provider, scope) already exists:
    /// the same refusal announced twice — by a daemon re-adopting the session,
    /// by a reconnecting watcher — must not become two observations, and must
    /// certainly not become two events a supervisor wakes on.
    pub fn record_provider_capacity_notice(
        &self,
        notice: NewProviderCapacityNotice<'_>,
    ) -> Result<Option<ProviderCapacityNotice>, rusqlite::Error> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO task_provider_capacity_notice (
                 task_id, stage_run_id, stage, provider, model, effort, source, rule_id,
                 matched_text, scope, cli_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                notice.task_id,
                notice.stage_run_id,
                notice.stage,
                notice.provider,
                notice.model,
                notice.effort,
                notice.source.as_str(),
                notice.rule_id,
                notice.matched_text,
                // Stored as `''` rather than NULL so the uniqueness constraint
                // can include it: SQLite treats NULLs as distinct, which would
                // let the same unscoped refusal insert twice.
                notice.scope.unwrap_or(""),
                notice.cli_version,
            ],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.provider_capacity_notice(self.conn.last_insert_rowid())
    }

    fn provider_capacity_notice(
        &self,
        id: i64,
    ) -> Result<Option<ProviderCapacityNotice>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT id, task_id, stage_run_id, stage, provider, model, effort, source,
                        rule_id, matched_text, scope, cli_version, observed_at
                 FROM task_provider_capacity_notice WHERE id = ?1",
                params![id],
                map_provider_capacity_notice,
            )
            .optional()
    }

    /// The most recent capacity refusal at one stage of one task.
    ///
    /// Scoped to a stage for the same reason the rejection reader is: a
    /// refusal at a stage the task has already left is history, and reporting
    /// it on detail would read as a live condition.
    pub fn latest_provider_capacity_notice_at_stage(
        &self,
        task_id: &str,
        stage: &str,
    ) -> Result<Option<ProviderCapacityNotice>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT id, task_id, stage_run_id, stage, provider, model, effort, source,
                        rule_id, matched_text, scope, cli_version, observed_at
                 FROM task_provider_capacity_notice WHERE task_id = ?1 AND stage = ?2
                 ORDER BY id DESC LIMIT 1",
                params![task_id, stage],
                map_provider_capacity_notice,
            )
            .optional()
    }

    /// Every capacity refusal recorded for one task, oldest first.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn provider_capacity_notices_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<ProviderCapacityNotice>, rusqlite::Error> {
        let mut statement = self.conn.prepare(
            "SELECT id, task_id, stage_run_id, stage, provider, model, effort, source,
                    rule_id, matched_text, scope, cli_version, observed_at
             FROM task_provider_capacity_notice WHERE task_id = ?1 ORDER BY id ASC",
        )?;
        let rows = statement.query_map(params![task_id], map_provider_capacity_notice)?;
        rows.collect()
    }
}

fn map_provider_capacity_notice(
    row: &rusqlite::Row<'_>,
) -> Result<ProviderCapacityNotice, rusqlite::Error> {
    let scope: String = row.get(10)?;
    Ok(ProviderCapacityNotice {
        stage_run_id: row.get(2)?,
        stage: row.get(3)?,
        provider: row.get(4)?,
        model: row.get(5)?,
        effort: row.get(6)?,
        source: row.get(7)?,
        rule_id: row.get(8)?,
        matched_text: row.get(9)?,
        scope: (!scope.is_empty()).then_some(scope),
        cli_version: row.get(11)?,
        observed_at: row.get(12)?,
    })
}
