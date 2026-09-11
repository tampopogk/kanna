//! Durable storage for observed agent-CLI token usage.
//!
//! Every row is one *usage record* the provider's own session file reported,
//! keyed by an identity derived from that record's content rather than by
//! where it was found. That is what makes collection idempotent across the
//! several ways the same record reaches disk twice: a resumed session whose
//! file replays earlier turns, a fork that copies history into a new file, a
//! rescan of a file that has not changed, and two stages sharing one provider
//! session. A rescan re-inserts the same key and changes nothing.
//!
//! Fields are a breakdown, never a sum of overlapping parts: `reasoning_tokens`
//! is the share of `output_tokens` spent thinking and `cached_input_tokens` is
//! input served from cache rather than extra input. `total_tokens` is written
//! by the collector from the non-overlapping parts, so summing a column here
//! never double counts.

use super::{AnalyticsRange, Db};

/// One task run's worktree and time window — what attribution needs to decide
/// which task a provider's usage record belongs to.
#[derive(Debug, Clone)]
pub struct RepoRunWindow {
    pub task_id: String,
    pub run_id: String,
    pub provider: Option<String>,
    pub provider_session_id: Option<String>,
    pub cwd: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

/// Cached result of looking for one recorded run's provider files in its
/// bounded candidate directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageDiscoveryState {
    pub directory_path: String,
    pub directory_modified_ns: i64,
    pub candidate_paths: Vec<String>,
}

/// One provider-reported usage record, normalized across providers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsageRecord {
    pub usage_key: String,
    pub provider: String,
    pub provider_session_id: Option<String>,
    pub repo_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub model: Option<String>,
    pub occurred_at: String,
    /// Input tokens billed fresh — never includes `cached_input_tokens`.
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub output_tokens: i64,
    /// The thinking share of `output_tokens`. A breakdown, not an addend.
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
}

/// How far a provider session file has been read, so growth is appended
/// rather than the whole file being parsed again.
///
/// `session_id`, `cwd` and `model` are carried because they are declared once
/// at the top of a session file: an incremental scan that resumes mid-file
/// never sees them again and would otherwise attribute the appended records
/// to nothing.
#[derive(Debug, Clone, Default)]
pub struct UsageScanState {
    pub file_size: i64,
    pub byte_offset: i64,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

/// The scan position to persist alongside a batch of records.
#[derive(Debug, Clone)]
pub struct UsageScanCheckpoint<'a> {
    pub file_path: &'a str,
    pub provider: &'a str,
    pub file_size: i64,
    pub byte_offset: i64,
    pub session_id: Option<&'a str>,
    pub cwd: Option<&'a str>,
    pub model: Option<&'a str>,
}

impl Db {
    /// Every recorded run of this repository's tasks that has a working
    /// directory, oldest first.
    pub fn list_repo_run_windows(
        &self,
        repo_id: &str,
    ) -> Result<Vec<RepoRunWindow>, rusqlite::Error> {
        let mut statement = self.conn.prepare(
            "SELECT stage_run.task_id, stage_run.id, stage_run.agent_provider,
                    stage_run.provider_session_id, stage_run.cwd,
                    stage_run.started_at, stage_run.finished_at
             FROM stage_run
             JOIN pipeline_item ON pipeline_item.id = stage_run.task_id
             WHERE pipeline_item.repo_id = ? AND stage_run.cwd IS NOT NULL
             ORDER BY stage_run.started_at ASC",
        )?;
        let runs = statement
            .query_map([repo_id], |row| {
                Ok(RepoRunWindow {
                    task_id: row.get(0)?,
                    run_id: row.get(1)?,
                    provider: row.get(2)?,
                    provider_session_id: row.get(3)?,
                    cwd: row.get(4)?,
                    started_at: row.get(5)?,
                    finished_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(runs)
    }

    pub fn usage_scan_state(
        &self,
        file_path: &str,
    ) -> Result<Option<UsageScanState>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT file_size, byte_offset, session_id, cwd, model
                 FROM provider_usage_scan WHERE file_path = ?",
                [file_path],
                |row| {
                    Ok(UsageScanState {
                        file_size: row.get(0)?,
                        byte_offset: row.get(1)?,
                        session_id: row.get(2)?,
                        cwd: row.get(3)?,
                        model: row.get(4)?,
                    })
                },
            )
            .optional()
    }

    pub fn usage_discovery_state(
        &self,
        discovery_key: &str,
    ) -> Result<Option<UsageDiscoveryState>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT directory_path, directory_modified_ns, candidate_paths
                 FROM provider_usage_discovery WHERE discovery_key = ?",
                [discovery_key],
                |row| {
                    let encoded: String = row.get(2)?;
                    Ok(UsageDiscoveryState {
                        directory_path: row.get(0)?,
                        directory_modified_ns: row.get(1)?,
                        candidate_paths: serde_json::from_str(&encoded).unwrap_or_default(),
                    })
                },
            )
            .optional()
    }

    pub fn record_usage_discovery(
        &self,
        discovery_key: &str,
        provider: &str,
        directory_path: &str,
        directory_modified_ns: i64,
        candidate_paths: &[String],
    ) -> Result<(), rusqlite::Error> {
        let encoded = serde_json::to_string(candidate_paths).unwrap_or_else(|_| "[]".to_string());
        self.conn.execute(
            "INSERT INTO provider_usage_discovery
               (discovery_key, provider, directory_path, directory_modified_ns,
                candidate_paths, checked_at)
             VALUES (?, ?, ?, ?, ?, datetime('now'))
             ON CONFLICT(discovery_key) DO UPDATE SET
               provider = excluded.provider,
               directory_path = excluded.directory_path,
               directory_modified_ns = excluded.directory_modified_ns,
               candidate_paths = excluded.candidate_paths,
               checked_at = excluded.checked_at",
            (
                discovery_key,
                provider,
                directory_path,
                directory_modified_ns,
                encoded,
            ),
        )?;
        Ok(())
    }

    pub fn repo_provider_run_ids_with_usage(
        &self,
        repo_id: &str,
        provider: &str,
        range: &AnalyticsRange,
    ) -> Result<std::collections::HashSet<String>, rusqlite::Error> {
        let start = format!("{} 00:00:00", range.from);
        let end = format!("{} 23:59:59", range.to);
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT run_id FROM provider_token_usage
             WHERE repo_id = ? AND provider = ? AND run_id IS NOT NULL
               AND occurred_at >= ? AND occurred_at <= ?",
        )?;
        let run_ids = statement
            .query_map((repo_id, provider, start, end), |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(run_ids)
    }

    /// Store a batch of usage records and the scan position that produced
    /// them in one transaction, so a crash mid-scan never advances the offset
    /// past records it did not store.
    pub fn record_token_usage(
        &self,
        records: &[TokenUsageRecord],
        scan: Option<UsageScanCheckpoint<'_>>,
    ) -> Result<usize, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            let mut inserted = 0;
            {
                let mut statement = db.conn.prepare(
                    "INSERT INTO provider_token_usage (
                       usage_key, provider, provider_session_id, repo_id, task_id, run_id,
                       model, occurred_at, input_tokens, cached_input_tokens,
                       cache_creation_tokens, output_tokens, reasoning_tokens, total_tokens
                     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                     ON CONFLICT(usage_key) DO UPDATE SET
                       repo_id = COALESCE(provider_token_usage.repo_id, excluded.repo_id),
                       task_id = COALESCE(provider_token_usage.task_id, excluded.task_id),
                       run_id = COALESCE(provider_token_usage.run_id, excluded.run_id),
                       model = COALESCE(provider_token_usage.model, excluded.model),
                       -- A streamed assistant turn is written more than once,
                       -- each write carrying a more complete usage block for
                       -- the same message. Per-field MAX keeps the most
                       -- complete report of one record without ever summing
                       -- the two reports of it.
                       input_tokens = MAX(provider_token_usage.input_tokens, excluded.input_tokens),
                       cached_input_tokens =
                         MAX(provider_token_usage.cached_input_tokens, excluded.cached_input_tokens),
                       cache_creation_tokens =
                         MAX(provider_token_usage.cache_creation_tokens, excluded.cache_creation_tokens),
                       output_tokens =
                         MAX(provider_token_usage.output_tokens, excluded.output_tokens),
                       reasoning_tokens =
                         MAX(provider_token_usage.reasoning_tokens, excluded.reasoning_tokens),
                       total_tokens = MAX(provider_token_usage.total_tokens, excluded.total_tokens)",
                )?;
                for record in records {
                    let changed = statement.execute(rusqlite::params![
                        record.usage_key,
                        record.provider,
                        record.provider_session_id,
                        record.repo_id,
                        record.task_id,
                        record.run_id,
                        record.model,
                        record.occurred_at,
                        record.input_tokens,
                        record.cached_input_tokens,
                        record.cache_creation_tokens,
                        record.output_tokens,
                        record.reasoning_tokens,
                        record.total_tokens,
                    ])?;
                    inserted += changed;
                }
            }
            if let Some(checkpoint) = scan {
                db.conn.execute(
                    "INSERT INTO provider_usage_scan
                       (file_path, provider, file_size, byte_offset, session_id, cwd, model, scanned_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'))
                     ON CONFLICT(file_path) DO UPDATE SET
                       file_size = excluded.file_size,
                       byte_offset = excluded.byte_offset,
                       session_id = COALESCE(excluded.session_id, provider_usage_scan.session_id),
                       cwd = COALESCE(excluded.cwd, provider_usage_scan.cwd),
                       model = COALESCE(excluded.model, provider_usage_scan.model),
                       scanned_at = excluded.scanned_at",
                    rusqlite::params![
                        checkpoint.file_path,
                        checkpoint.provider,
                        checkpoint.file_size,
                        checkpoint.byte_offset,
                        checkpoint.session_id,
                        checkpoint.cwd,
                        checkpoint.model,
                    ],
                )?;
            }
            Ok(inserted)
        })
    }

    #[cfg(test)]
    pub fn count_test_token_usage_rows(&self) -> Result<i64, rusqlite::Error> {
        self.conn
            .query_row("SELECT COUNT(*) FROM provider_token_usage", [], |row| {
                row.get(0)
            })
    }
}
