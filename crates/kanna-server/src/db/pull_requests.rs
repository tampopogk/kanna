//! Durable pull-request facts behind the Analytics view.
//!
//! A task's `pr_url` column answers "which PR is this task's", not "how many
//! pull requests did this repository produce" — two tasks can name the same
//! PR (a revision round that re-reports it, a follow-up task pointed at an
//! existing PR), and a task row disappears from the question entirely once
//! somebody counts by task. So the canonical PR identity gets its own row,
//! written where Kanna first observes the URL.
//!
//! Creation and merge are forge facts. Kanna observes the first of them only
//! as "the moment this desktop saw the PR", which is why `first_seen_at` and
//! `forge_created_at` are separate columns, and it cannot observe the second
//! at all without asking the forge — see [`crate::forge_pull_requests`].
//! Closing a task is not a merge and is deliberately never read as one.

use super::Db;
use rusqlite::{Connection, OptionalExtension};

/// A forge-confirmed observation of one pull request.
#[derive(Debug, Clone)]
pub struct ForgePullRequestObservation {
    pub pr_number: i64,
    pub url: Option<String>,
    pub created_at: Option<String>,
    pub merged_at: Option<String>,
    pub state: Option<String>,
}

/// Collapse the many spellings of one PR URL into a single identity.
///
/// `gh` and the agents that report a PR are not consistent about a trailing
/// slash, a `/files` suffix, or the scheme, and every inconsistent spelling
/// would otherwise count as another pull request.
pub fn canonical_pr_key(pr_url: &str, pr_number: Option<i64>) -> String {
    let trimmed = pr_url.trim();
    let without_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed);
    let without_www = without_scheme
        .strip_prefix("www.")
        .unwrap_or(without_scheme);
    // Everything after the PR number is a view of the same pull request.
    let mut segments = without_www.split('/');
    let host = segments.next().unwrap_or_default();
    let owner = segments.next().unwrap_or_default();
    let repo = segments.next().unwrap_or_default();
    let kind = segments.next().unwrap_or_default();
    let number = segments.next().unwrap_or_default();
    if !host.is_empty() && !owner.is_empty() && !repo.is_empty() && !number.is_empty() {
        return format!("{host}/{owner}/{repo}/{kind}/{number}").to_lowercase();
    }
    match pr_number {
        Some(number) => format!("unresolved#{number}"),
        None => without_www.trim_end_matches('/').to_lowercase(),
    }
}

/// Record that this desktop has seen a pull request, without claiming
/// anything about the forge. Re-reporting the same PR is not a new one.
pub(super) fn observe_pull_request(
    conn: &Connection,
    repo_id: &str,
    pr_number: Option<i64>,
    pr_url: &str,
) -> Result<(), rusqlite::Error> {
    let pr_key = canonical_pr_key(pr_url, pr_number);
    conn.execute(
        "INSERT INTO task_pull_request (repo_id, pr_key, pr_number, pr_url, first_seen_at)
         VALUES (?, ?, ?, ?, datetime('now'))
         ON CONFLICT(repo_id, pr_key) DO UPDATE SET
           pr_number = COALESCE(excluded.pr_number, task_pull_request.pr_number),
           pr_url = COALESCE(excluded.pr_url, task_pull_request.pr_url)",
        rusqlite::params![repo_id, pr_key, pr_number, pr_url],
    )?;
    Ok(())
}

impl Db {
    /// How many distinct pull requests this repository has produced.
    #[cfg(test)]
    pub fn count_test_repo_pull_requests(&self, repo_id: &str) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM task_pull_request WHERE repo_id = ?",
            [repo_id],
            |row| row.get(0),
        )
    }

    /// When the forge confirmed this pull request merged, if it ever did.
    #[cfg(test)]
    pub fn test_pull_request_merged_at(
        &self,
        repo_id: &str,
        pr_number: i64,
    ) -> Result<Option<String>, rusqlite::Error> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row(
                "SELECT forge_merged_at FROM task_pull_request
                 WHERE repo_id = ? AND pr_number = ?",
                (repo_id, pr_number),
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
    }

    /// When the forge was last asked about this repository, as epoch seconds.
    /// `None` means it never was.
    pub fn last_pull_request_forge_check(
        &self,
        repo_id: &str,
    ) -> Result<Option<i64>, rusqlite::Error> {
        self.conn.query_row(
            "SELECT MAX(strftime('%s', forge_checked_at))
             FROM task_pull_request WHERE repo_id = ?",
            [repo_id],
            |row| row.get(0),
        )
    }

    /// PR numbers this repo knows about whose forge state is still open or
    /// never checked. A merged or closed pull request is terminal, so it is
    /// never asked about again.
    pub fn unresolved_repo_pull_request_numbers(
        &self,
        repo_id: &str,
    ) -> Result<Vec<i64>, rusqlite::Error> {
        let mut statement = self.conn.prepare(
            "SELECT pr_number
             FROM task_pull_request
             WHERE repo_id = ?
               AND pr_number IS NOT NULL
               AND forge_merged_at IS NULL
               AND (forge_state IS NULL OR forge_state NOT IN ('CLOSED', 'MERGED'))
             ORDER BY pr_number DESC",
        )?;
        let numbers = statement
            .query_map([repo_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(numbers)
    }

    /// Persist what the forge actually said. Facts only: a pull request the
    /// forge did not mention keeps whatever was previously confirmed rather
    /// than being downgraded to "not merged".
    pub fn record_forge_pull_requests(
        &self,
        repo_id: &str,
        observations: &[ForgePullRequestObservation],
    ) -> Result<usize, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            let mut recorded = 0;
            for observation in observations {
                let Some(url) = observation.url.as_deref() else {
                    continue;
                };
                let pr_key = canonical_pr_key(url, Some(observation.pr_number));
                let existing: Option<String> = db
                    .conn
                    .query_row(
                        "SELECT pr_key FROM task_pull_request WHERE repo_id = ? AND pr_key = ?",
                        (repo_id, &pr_key),
                        |row| row.get(0),
                    )
                    .optional()?;
                if existing.is_none() {
                    // Only pull requests this desktop's tasks produced are
                    // this repository's Analytics subject; a PR somebody else
                    // opened is not counted just because `gh` listed it.
                    continue;
                }
                db.conn.execute(
                    "UPDATE task_pull_request
                     SET pr_number = ?,
                         forge_created_at = ?,
                         forge_merged_at = ?,
                         forge_state = ?,
                         forge_checked_at = datetime('now')
                     WHERE repo_id = ? AND pr_key = ?",
                    rusqlite::params![
                        observation.pr_number,
                        observation.created_at,
                        observation.merged_at,
                        observation.state,
                        repo_id,
                        pr_key,
                    ],
                )?;
                recorded += 1;
            }
            Ok(recorded)
        })
    }

    /// Backfill PR identities from task rows that predate this record, so an
    /// upgrading operator sees the pull requests their tasks already carry.
    pub fn backfill_repo_pull_requests(&self, repo_id: &str) -> Result<usize, rusqlite::Error> {
        self.with_immediate_transaction(|db| {
            let mut statement = db.conn.prepare(
                "SELECT pr_number, pr_url
                 FROM pipeline_item
                 WHERE repo_id = ? AND pr_url IS NOT NULL AND pr_url <> ''",
            )?;
            let rows = statement
                .query_map([repo_id], |row| {
                    Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            let mut written = 0;
            for (pr_number, pr_url) in rows {
                observe_pull_request(&db.conn, repo_id, pr_number, &pr_url)?;
                written += 1;
            }
            Ok(written)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_pr_key;

    #[test]
    fn one_pull_request_keeps_one_identity_across_its_spellings() {
        let canonical = canonical_pr_key("https://github.com/owner/repo/pull/12", Some(12));
        for spelling in [
            "http://github.com/owner/repo/pull/12",
            "https://www.github.com/owner/repo/pull/12",
            "https://github.com/owner/repo/pull/12/files",
            "https://github.com/Owner/Repo/pull/12",
        ] {
            assert_eq!(
                canonical_pr_key(spelling, Some(12)),
                canonical,
                "{spelling} should collapse onto the same pull request"
            );
        }
    }

    #[test]
    fn different_pull_requests_keep_different_identities() {
        assert_ne!(
            canonical_pr_key("https://github.com/owner/repo/pull/12", Some(12)),
            canonical_pr_key("https://github.com/owner/repo/pull/13", Some(13)),
        );
        assert_ne!(
            canonical_pr_key("https://github.com/owner/other/pull/12", Some(12)),
            canonical_pr_key("https://github.com/owner/repo/pull/12", Some(12)),
        );
    }

    #[test]
    fn an_unparseable_url_falls_back_to_the_pull_request_number() {
        assert_eq!(canonical_pr_key("not a url", Some(7)), "unresolved#7");
    }
}
