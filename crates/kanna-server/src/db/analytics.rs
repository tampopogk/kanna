//! Analytics statistics over a chosen date range.
//!
//! This view answers a small set of questions an operator asked directly: how
//! long tasks sit waiting for somebody, how many tasks and pull requests the
//! repository produced, how much review churn the work costs, and how many
//! tokens the agent CLIs actually spent. It is statistics, not a monitor —
//! every figure is bounded by an explicit window and an explicit denominator.
//!
//! Three rules shape everything below.
//!
//! **Idle here means "nobody is servicing this task".** That deliberately
//! folds `unread` in with `idle`: a finished task whose output nobody has read
//! is exactly as unserviced as one sitting at a prompt, and this view does not
//! care whether the servicing was owed by a person or an agent. This is an
//! Analytics definition and changes nothing about `runtimeState`, the
//! read/unread dimension, or how tasks are supervised.
//!
//! **A missing record is not a zero.** Every accumulator stamps when it
//! started, and a window that reaches back before that is reported with its
//! coverage boundary rather than as a quiet stretch of nothing.
//!
//! **Every average names its denominator.** "Average revisions per task"
//! counts tasks that actually reached review, including the ones that passed
//! without a single revision; leaving those out would turn a clean week into
//! a bad one.

use super::Db;
use serde::Serialize;
use std::collections::HashMap;

/// Inclusive date window, `YYYY-MM-DD`.
#[derive(Debug, Clone)]
pub struct AnalyticsRange {
    pub from: String,
    pub to: String,
}

impl AnalyticsRange {
    fn start(&self) -> String {
        format!("{} 00:00:00", self.from)
    }

    fn end(&self) -> String {
        format!("{} 23:59:59", self.to)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoAnalytics {
    pub range: RangeReport,
    pub coverage: CoverageReport,
    pub tasks: TaskStats,
    pub pull_requests: PullRequestStats,
    pub idle: IdleStats,
    pub revisions: RevisionStats,
    pub tokens: TokenStats,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeReport {
    pub from: String,
    pub to: String,
}

/// Where each statistic's record actually begins, and whether the forge could
/// be reached. The view draws its own boundaries from this instead of
/// presenting an unrecorded stretch as an empty one.
#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CoverageReport {
    pub idle_since: Option<String>,
    pub revisions_since: Option<String>,
    pub tokens_since: Option<String>,
    /// False when the forge could not confirm merge outcomes this time.
    pub pull_request_state_confirmed: bool,
    /// Providers this repository's runs used whose usage Kanna cannot read.
    pub providers_without_token_usage: Vec<String>,
    /// Runs in the window that have at least one usage record, over all runs
    /// in the window — how much of the work the token figures speak for.
    pub runs_with_token_usage: i64,
    pub runs_in_range: i64,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskStats {
    pub created: i64,
    pub closed: i64,
    pub open_now: i64,
    /// Counted apart from `created` rather than folded into it: a dispatched
    /// specialty review creates real tasks, and silently adding them would
    /// make a repository that reviews thoroughly look more productive.
    pub child_tasks_created: i64,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestStats {
    pub created: i64,
    /// `None` when merge state could not be confirmed with the forge — which
    /// is not the same as no pull request having merged.
    pub merged: Option<i64>,
    pub open_now: Option<i64>,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IdleStats {
    pub total_seconds: i64,
    pub working_seconds: i64,
    /// Denominator for `average_seconds_per_task`: tasks that were alive in
    /// the window, whether or not they spent any of it idle.
    pub task_count: i64,
    pub average_seconds_per_task: f64,
    pub longest_seconds: i64,
    pub contributors: Vec<TaskContribution>,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RevisionStats {
    /// Tasks whose first review began in the window and produced a verdict —
    /// including those that were never revised.
    pub cohort_tasks: i64,
    pub total_revisions: i64,
    pub average_per_task: f64,
    /// Share of the cohort that passed review without a single revision.
    pub clean_pass_rate: Option<f64>,
    /// Revision requests a spent budget parked instead of starting. Reported
    /// separately because they are a verdict, not a round the task spent.
    pub parked_requests: i64,
    pub contributors: Vec<TaskContribution>,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TokenStats {
    pub total: TokenTotals,
    pub by_model: Vec<TokenGroup>,
    pub by_task: Vec<TokenGroup>,
}

#[derive(Debug, Serialize, Default, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    pub input: i64,
    pub cached_input: i64,
    pub cache_creation: i64,
    pub output: i64,
    /// The thinking share of `output`. A breakdown of it, never an addition.
    pub reasoning: i64,
    pub total: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenGroup {
    pub key: String,
    pub label: String,
    pub totals: TokenTotals,
}

/// One task's share of a statistic, so a number can be opened into the rows
/// that produced it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskContribution {
    pub task_id: String,
    pub title: String,
    pub value: i64,
}

/// How many rows a drilldown returns. A statistic is meant to be opened into
/// its largest contributors, not into an unbounded table.
const CONTRIBUTOR_LIMIT: usize = 12;

struct IntervalRow {
    task_id: String,
    activity: String,
    started_at: String,
    ended_at: String,
}

impl Db {
    pub fn repo_analytics(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
        pull_request_state_confirmed: bool,
        providers_without_token_usage: Vec<String>,
    ) -> Result<RepoAnalytics, rusqlite::Error> {
        let titles = self.task_titles(repo_id)?;
        let (runs_with_usage, runs_in_range) = self.token_usage_coverage(repo_id, range)?;
        Ok(RepoAnalytics {
            range: RangeReport {
                from: range.from.clone(),
                to: range.to.clone(),
            },
            coverage: CoverageReport {
                idle_since: self.get_setting(super::ANALYTICS_ACTIVITY_COVERAGE_KEY)?,
                revisions_since: self.get_setting(super::ANALYTICS_REVISION_COVERAGE_KEY)?,
                tokens_since: self.get_setting(super::ANALYTICS_TOKEN_COVERAGE_KEY)?,
                pull_request_state_confirmed,
                providers_without_token_usage,
                runs_with_token_usage: runs_with_usage,
                runs_in_range,
            },
            tasks: self.task_stats(repo_id, range)?,
            pull_requests: self.pull_request_stats(repo_id, range, pull_request_state_confirmed)?,
            idle: self.idle_stats(repo_id, range, &titles)?,
            revisions: self.revision_stats(repo_id, range, &titles)?,
            tokens: self.token_stats(repo_id, range, &titles)?,
        })
    }

    fn task_titles(&self, repo_id: &str) -> Result<HashMap<String, String>, rusqlite::Error> {
        let mut statement = self.conn.prepare(
            "SELECT id, COALESCE(NULLIF(display_name, ''), NULLIF(issue_title, ''),
                                 NULLIF(substr(prompt, 1, 80), ''), id)
             FROM pipeline_item WHERE repo_id = ?",
        )?;
        let titles = statement
            .query_map([repo_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(titles)
    }

    fn task_stats(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
    ) -> Result<TaskStats, rusqlite::Error> {
        let (start, end) = (range.start(), range.end());
        let created = self.conn.query_row(
            "SELECT COUNT(*) FROM pipeline_item
             WHERE repo_id = ? AND parent_task_id IS NULL
               AND created_at >= ? AND created_at <= ?",
            (repo_id, &start, &end),
            |row| row.get(0),
        )?;
        let child_tasks_created = self.conn.query_row(
            "SELECT COUNT(*) FROM pipeline_item
             WHERE repo_id = ? AND parent_task_id IS NOT NULL
               AND created_at >= ? AND created_at <= ?",
            (repo_id, &start, &end),
            |row| row.get(0),
        )?;
        let closed = self.conn.query_row(
            "SELECT COUNT(*) FROM pipeline_item
             WHERE repo_id = ? AND parent_task_id IS NULL
               AND closed_at IS NOT NULL AND closed_at >= ? AND closed_at <= ?",
            (repo_id, &start, &end),
            |row| row.get(0),
        )?;
        // Deliberately "right now" rather than "at the end of the window":
        // an operator reading this wants the backlog they are holding, and
        // reconstructing a past open count needs history this record does not
        // claim to keep.
        let open_now = self.conn.query_row(
            "SELECT COUNT(*) FROM pipeline_item
             WHERE repo_id = ? AND parent_task_id IS NULL AND closed_at IS NULL",
            [repo_id],
            |row| row.get(0),
        )?;
        Ok(TaskStats {
            created,
            closed,
            open_now,
            child_tasks_created,
        })
    }

    fn pull_request_stats(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
        confirmed: bool,
    ) -> Result<PullRequestStats, rusqlite::Error> {
        let (start, end) = (range.start(), range.end());
        // The forge's creation instant when it is known, and the moment this
        // desktop first saw the pull request otherwise. Both are ISO or
        // SQLite-form timestamps, so the window is compared on the date part.
        let created = self.conn.query_row(
            "SELECT COUNT(*) FROM task_pull_request
             WHERE repo_id = ?
               AND substr(COALESCE(forge_created_at, first_seen_at), 1, 10) >= substr(?, 1, 10)
               AND substr(COALESCE(forge_created_at, first_seen_at), 1, 10) <= substr(?, 1, 10)",
            (repo_id, &start, &end),
            |row| row.get(0),
        )?;
        if !confirmed {
            return Ok(PullRequestStats {
                created,
                merged: None,
                open_now: None,
            });
        }
        let merged = self.conn.query_row(
            "SELECT COUNT(*) FROM task_pull_request
             WHERE repo_id = ? AND forge_merged_at IS NOT NULL
               AND substr(forge_merged_at, 1, 10) >= substr(?, 1, 10)
               AND substr(forge_merged_at, 1, 10) <= substr(?, 1, 10)",
            (repo_id, &start, &end),
            |row| row.get(0),
        )?;
        let open_now = self.conn.query_row(
            "SELECT COUNT(*) FROM task_pull_request
             WHERE repo_id = ? AND forge_merged_at IS NULL
               AND (forge_state IS NULL OR forge_state = 'OPEN')",
            [repo_id],
            |row| row.get(0),
        )?;
        Ok(PullRequestStats {
            created,
            merged: Some(merged),
            open_now: Some(open_now),
        })
    }

    /// Read the completed activity spans that overlap the window, plus the one
    /// span still running on each open task.
    ///
    /// The live span is derived from `pipeline_item` rather than stored, which
    /// is exactly why including it cannot double count: a span becomes a row
    /// only at the moment it stops being the live one.
    fn activity_intervals(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
    ) -> Result<Vec<IntervalRow>, rusqlite::Error> {
        let (start, end) = (range.start(), range.end());
        let mut statement = self.conn.prepare(
            "SELECT interval.task_id, interval.activity, interval.started_at, interval.ended_at
             FROM task_activity_interval AS interval
             JOIN pipeline_item ON pipeline_item.id = interval.task_id
             WHERE pipeline_item.repo_id = ?
               AND interval.started_at <= ? AND interval.ended_at >= ?
             UNION ALL
             SELECT id, activity, activity_changed_at, datetime('now')
             FROM pipeline_item
             WHERE repo_id = ? AND closed_at IS NULL
               AND activity IS NOT NULL AND activity_changed_at IS NOT NULL
               AND activity_changed_at <= ?",
        )?;
        let rows = statement
            .query_map((repo_id, &end, &start, repo_id, &end), |row| {
                Ok(IntervalRow {
                    task_id: row.get(0)?,
                    activity: row.get(1)?,
                    started_at: row.get(2)?,
                    ended_at: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn idle_stats(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
        titles: &HashMap<String, String>,
    ) -> Result<IdleStats, rusqlite::Error> {
        let window_start = epoch_seconds(&range.start()).unwrap_or_default();
        let window_end = epoch_seconds(&range.end()).unwrap_or_default();
        let mut idle_by_task: HashMap<String, i64> = HashMap::new();
        let mut working_seconds = 0_i64;

        for interval in self.activity_intervals(repo_id, range)? {
            let Some(seconds) = clipped_seconds(
                &interval.started_at,
                &interval.ended_at,
                window_start,
                window_end,
            ) else {
                continue;
            };
            match interval.activity.as_str() {
                // The Analytics definition: nobody is servicing this task.
                "idle" | "unread" => {
                    *idle_by_task.entry(interval.task_id).or_insert(0) += seconds;
                }
                "working" => working_seconds += seconds,
                _ => {}
            }
        }

        // Every task alive in the window is in the denominator, including the
        // ones that never sat idle — averaging over only the idle ones would
        // report a worse number the better the fleet is doing.
        let task_count = self.tasks_alive_in_range(repo_id, range)?;
        let total_seconds: i64 = idle_by_task.values().sum();
        let longest_seconds = idle_by_task.values().copied().max().unwrap_or(0);
        Ok(IdleStats {
            total_seconds,
            working_seconds,
            task_count,
            average_seconds_per_task: if task_count > 0 {
                total_seconds as f64 / task_count as f64
            } else {
                0.0
            },
            longest_seconds,
            contributors: top_contributions(idle_by_task, titles),
        })
    }

    fn tasks_alive_in_range(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM pipeline_item
             WHERE repo_id = ?
               AND created_at <= ?
               AND (closed_at IS NULL OR closed_at >= ?)",
            (repo_id, range.end(), range.start()),
            |row| row.get(0),
        )
    }

    fn revision_stats(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
        titles: &HashMap<String, String>,
    ) -> Result<RevisionStats, rusqlite::Error> {
        let (start, end) = (range.start(), range.end());
        // Cohort membership is anchored to the first review, then waits for a
        // genuine review verdict. An in-progress or interrupted run has not
        // passed cleanly merely because no revision row exists yet.
        let mut cohort_statement = self.conn.prepare(
            "WITH first_review AS (
               SELECT stage_run.task_id, MIN(stage_run.started_at) AS started_at
               FROM stage_run
               JOIN pipeline_item ON pipeline_item.id = stage_run.task_id
               WHERE pipeline_item.repo_id = ?
                 AND pipeline_item.parent_task_id IS NULL
                 AND stage_run.kind = 'main'
                 AND stage_run.stage = 'review'
               GROUP BY stage_run.task_id
             )
             SELECT first_review.task_id
             FROM first_review
             WHERE first_review.started_at >= ? AND first_review.started_at <= ?
               AND EXISTS (
                 SELECT 1 FROM stage_run AS outcome
                 WHERE outcome.task_id = first_review.task_id
                   AND outcome.kind = 'main' AND outcome.stage = 'review'
                   AND outcome.finished_at IS NOT NULL
                   AND outcome.status IN ('succeeded', 'failed')
                   AND outcome.no_work_termination IS NULL
               )",
        )?;
        let cohort = cohort_statement
            .query_map((repo_id, &start, &end), |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(cohort_statement);

        if cohort.is_empty() {
            return Ok(RevisionStats::default());
        }

        // A dispatched specialty review revises through its parent, and the
        // cohort holds only parents, so one panel verdict is one revision
        // here however many children produced it.
        let mut revisions_by_task: HashMap<String, i64> = HashMap::new();
        let mut parked_requests = 0_i64;
        let mut statement = self.conn.prepare(
            "SELECT task_revision.task_id, task_revision.applied
             FROM task_revision
             JOIN pipeline_item ON pipeline_item.id = task_revision.task_id
             WHERE pipeline_item.repo_id = ?",
        )?;
        let rows = statement
            .query_map([repo_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for (task_id, applied) in rows {
            if !cohort.contains(&task_id) {
                continue;
            }
            if applied == 1 {
                *revisions_by_task.entry(task_id).or_insert(0) += 1;
            } else {
                parked_requests += 1;
            }
        }

        let cohort_tasks = cohort.len() as i64;
        let total_revisions: i64 = revisions_by_task.values().sum();
        let clean = cohort
            .iter()
            .filter(|task_id| !revisions_by_task.contains_key(*task_id))
            .count() as i64;
        Ok(RevisionStats {
            cohort_tasks,
            total_revisions,
            average_per_task: total_revisions as f64 / cohort_tasks as f64,
            clean_pass_rate: Some(clean as f64 / cohort_tasks as f64),
            parked_requests,
            contributors: top_contributions(revisions_by_task, titles),
        })
    }

    /// How many of the window's runs the token figures actually speak for.
    fn token_usage_coverage(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
    ) -> Result<(i64, i64), rusqlite::Error> {
        let (start, end) = (range.start(), range.end());
        let runs_in_range = self.conn.query_row(
            "SELECT COUNT(*) FROM stage_run
             JOIN pipeline_item ON pipeline_item.id = stage_run.task_id
             WHERE pipeline_item.repo_id = ?
               AND stage_run.started_at <= ?
               AND (stage_run.finished_at IS NULL OR stage_run.finished_at >= ?)",
            (repo_id, &end, &start),
            |row| row.get(0),
        )?;
        let runs_with_usage = self.conn.query_row(
            "SELECT COUNT(DISTINCT stage_run.id) FROM stage_run
             JOIN pipeline_item ON pipeline_item.id = stage_run.task_id
             JOIN provider_token_usage ON provider_token_usage.run_id = stage_run.id
             WHERE pipeline_item.repo_id = ?
               AND stage_run.started_at <= ?
               AND (stage_run.finished_at IS NULL OR stage_run.finished_at >= ?)
               AND provider_token_usage.occurred_at >= ?
               AND provider_token_usage.occurred_at <= ?",
            (repo_id, &end, &start, &start, &end),
            |row| row.get(0),
        )?;
        Ok((runs_with_usage, runs_in_range))
    }

    fn token_stats(
        &self,
        repo_id: &str,
        range: &AnalyticsRange,
        titles: &HashMap<String, String>,
    ) -> Result<TokenStats, rusqlite::Error> {
        let (start, end) = (range.start(), range.end());
        let mut statement = self.conn.prepare(
            "SELECT task_id, model,
                    input_tokens, cached_input_tokens, cache_creation_tokens,
                    output_tokens, reasoning_tokens, total_tokens
             FROM provider_token_usage
             WHERE repo_id = ? AND task_id IS NOT NULL
               AND occurred_at >= ? AND occurred_at <= ?",
        )?;
        let rows = statement
            .query_map((repo_id, &start, &end), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    TokenTotals {
                        input: row.get(2)?,
                        cached_input: row.get(3)?,
                        cache_creation: row.get(4)?,
                        output: row.get(5)?,
                        reasoning: row.get(6)?,
                        total: row.get(7)?,
                    },
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        let mut total = TokenTotals::default();
        let mut by_model: HashMap<String, TokenTotals> = HashMap::new();
        let mut by_task: HashMap<String, TokenTotals> = HashMap::new();
        for (task_id, model, totals) in rows {
            total.add(totals);
            by_model
                .entry(model.unwrap_or_else(|| "unknown".to_string()))
                .or_default()
                .add(totals);
            by_task.entry(task_id).or_default().add(totals);
        }

        Ok(TokenStats {
            total,
            by_model: ranked_groups(by_model, |key| key.to_string(), CONTRIBUTOR_LIMIT),
            by_task: ranked_groups(
                by_task,
                |key| titles.get(key).cloned().unwrap_or_else(|| key.to_string()),
                CONTRIBUTOR_LIMIT,
            ),
        })
    }
}

impl TokenTotals {
    fn add(&mut self, other: TokenTotals) {
        self.input += other.input;
        self.cached_input += other.cached_input;
        self.cache_creation += other.cache_creation;
        self.output += other.output;
        self.reasoning += other.reasoning;
        self.total += other.total;
    }
}

fn top_contributions(
    values: HashMap<String, i64>,
    titles: &HashMap<String, String>,
) -> Vec<TaskContribution> {
    let mut contributions = values
        .into_iter()
        .filter(|(_, value)| *value > 0)
        .map(|(task_id, value)| TaskContribution {
            title: titles
                .get(&task_id)
                .cloned()
                .unwrap_or_else(|| task_id.clone()),
            task_id,
            value,
        })
        .collect::<Vec<_>>();
    contributions.sort_by(|left, right| {
        right
            .value
            .cmp(&left.value)
            .then_with(|| left.task_id.cmp(&right.task_id))
    });
    contributions.truncate(CONTRIBUTOR_LIMIT);
    contributions
}

fn ranked_groups(
    values: HashMap<String, TokenTotals>,
    label: impl Fn(&str) -> String,
    limit: usize,
) -> Vec<TokenGroup> {
    let mut groups = values
        .into_iter()
        .map(|(key, totals)| TokenGroup {
            label: label(&key),
            key,
            totals,
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        right
            .totals
            .total
            .cmp(&left.totals.total)
            .then_with(|| left.key.cmp(&right.key))
    });
    groups.truncate(limit);
    groups
}

/// The part of one activity span that falls inside the window, in seconds.
///
/// Clipping is what lets a span that started weeks ago and is still running
/// contribute exactly the window's worth of itself, and what keeps a task
/// counted once when a window boundary falls in the middle of a wait.
fn clipped_seconds(
    started_at: &str,
    ended_at: &str,
    window_start: i64,
    window_end: i64,
) -> Option<i64> {
    let start = epoch_seconds(started_at)?.max(window_start);
    let end = epoch_seconds(ended_at)?.min(window_end);
    (end > start).then_some(end - start)
}

fn epoch_seconds(value: &str) -> Option<i64> {
    parse_datetime(value).map(|parsed| parsed.timestamp_seconds())
}

#[derive(Clone, Copy)]
struct ParsedDateTime {
    year: i32,
    month: u32,
    day: u32,
    hour: i64,
    minute: i64,
    second: i64,
}

impl ParsedDateTime {
    fn timestamp_seconds(self) -> i64 {
        (days_from_civil(self.year, self.month, self.day) * 86_400)
            + (self.hour * 3_600)
            + (self.minute * 60)
            + self.second
    }
}

/// Accepts both timestamp spellings this database holds: SQLite's
/// `YYYY-MM-DD HH:MM:SS` and the ISO `YYYY-MM-DDTHH:MM:SSZ` the provider CLIs
/// write. Both are UTC.
fn parse_datetime(value: &str) -> Option<ParsedDateTime> {
    let date = value.get(0..10)?;
    let year = date.get(0..4)?.parse::<i32>().ok()?;
    let month = date.get(5..7)?.parse::<u32>().ok()?;
    let day = date.get(8..10)?.parse::<u32>().ok()?;
    let time = value.get(11..19).unwrap_or("00:00:00");
    let hour = time.get(0..2)?.parse::<i64>().ok()?;
    let minute = time.get(3..5)?.parse::<i64>().ok()?;
    let second = time.get(6..8)?.parse::<i64>().ok()?;
    Some(ParsedDateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    })
}

// Howard Hinnant's civil date algorithm. Returns days since 1970-01-01.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64
}

#[cfg(test)]
mod tests {
    use super::{clipped_seconds, epoch_seconds, parse_datetime};

    fn window() -> (i64, i64) {
        (
            epoch_seconds("2026-09-02 00:00:00").expect("start"),
            epoch_seconds("2026-09-02 23:59:59").expect("end"),
        )
    }

    #[test]
    fn both_timestamp_spellings_resolve_to_the_same_instant() {
        assert_eq!(
            parse_datetime("2026-09-02 10:00:00")
                .expect("sqlite")
                .timestamp_seconds(),
            parse_datetime("2026-09-02T10:00:00.500Z")
                .expect("iso")
                .timestamp_seconds(),
        );
    }

    #[test]
    fn a_span_inside_the_window_counts_whole() {
        let (start, end) = window();
        assert_eq!(
            clipped_seconds("2026-09-02 10:00:00", "2026-09-02 10:01:00", start, end),
            Some(60)
        );
    }

    #[test]
    fn a_span_crossing_both_boundaries_counts_only_the_window() {
        let (start, end) = window();
        assert_eq!(
            clipped_seconds("2026-08-01 00:00:00", "2026-10-01 00:00:00", start, end),
            Some(86_399)
        );
    }

    #[test]
    fn a_span_that_ends_before_the_window_contributes_nothing() {
        let (start, end) = window();
        assert_eq!(
            clipped_seconds("2026-09-01 00:00:00", "2026-09-01 12:00:00", start, end),
            None
        );
    }

    #[test]
    fn one_span_split_by_a_boundary_is_never_counted_twice() {
        let (day_two_start, day_two_end) = window();
        let day_one_start = epoch_seconds("2026-09-01 00:00:00").expect("start");
        let day_one_end = epoch_seconds("2026-09-01 23:59:59").expect("end");
        let first = clipped_seconds(
            "2026-09-01 23:00:00",
            "2026-09-02 01:00:00",
            day_one_start,
            day_one_end,
        )
        .expect("day one share");
        let second = clipped_seconds(
            "2026-09-01 23:00:00",
            "2026-09-02 01:00:00",
            day_two_start,
            day_two_end,
        )
        .expect("day two share");
        assert_eq!(first + second, 2 * 3_600 - 1);
    }

    #[test]
    fn an_unparseable_timestamp_is_skipped_rather_than_counted_as_the_epoch() {
        let (start, end) = window();
        assert_eq!(
            clipped_seconds("not-a-date", "2026-09-02 10:00:00", start, end),
            None
        );
    }
}
