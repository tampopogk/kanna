//! Confirming pull-request outcomes with the forge.
//!
//! Kanna creates pull requests through an agent running `gh` and records the
//! URL it reports; nothing in the desktop observes what happens to one
//! afterwards. Closing a task is not a merge — a task closes when its work
//! leaves the workflow, which routinely happens before, after, or instead of
//! the PR being merged — so "how many merged" cannot be answered from local
//! state at all. It is asked of the forge here, once per repository per
//! staleness window, and the answer is persisted.
//!
//! Unavailability is a first-class result. Without `gh`, without credentials,
//! or without a recognizable remote, merged counts are reported as unknown
//! rather than as zero.

use crate::db::{Db, ForgePullRequestObservation};
use serde::Deserialize;
use std::process::{Command, Stdio};
use std::time::Duration;

/// How long a confirmed answer stays good enough to reuse. Analytics is a
/// review surface, not a live monitor, and a `gh` call per repaint would make
/// opening the view cost a network round trip every time.
const RECHECK_AFTER: Duration = Duration::from_secs(300);

/// The forge's most recent word on this repository's pull requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeAvailability {
    /// Confirmed facts are stored and current.
    Confirmed,
    /// Stored facts are whatever was confirmed before; nothing new could be
    /// learned this time.
    Unavailable,
}

#[derive(Debug, Deserialize)]
struct GhPullRequest {
    number: i64,
    url: Option<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    #[serde(rename = "mergedAt")]
    merged_at: Option<String>,
    state: Option<String>,
}

/// Bring this repository's pull-request facts up to date, if it is worth
/// asking and the forge can answer.
pub fn reconcile_repo_pull_requests(db: &Db, repo_id: &str, repo_path: &str) -> ForgeAvailability {
    let unresolved = match db.unresolved_repo_pull_request_numbers(repo_id) {
        Ok(unresolved) => unresolved,
        Err(error) => {
            log::warn!("analytics: reading unresolved pull requests failed: {error}");
            return ForgeAvailability::Unavailable;
        }
    };
    if unresolved.is_empty() {
        // Every pull request this repository produced already has a terminal
        // answer stored. Nothing to ask.
        return ForgeAvailability::Confirmed;
    }
    // Analytics is a review surface, not a live monitor. Without this gate
    // every repaint of the view would cost a `gh` round trip.
    let last_check = db.last_pull_request_forge_check(repo_id).unwrap_or(None);
    if recently_reconciled(last_check, now_epoch_seconds()) {
        return ForgeAvailability::Confirmed;
    }

    let observations = match query_pull_requests(repo_path) {
        Some(observations) => observations,
        None => return ForgeAvailability::Unavailable,
    };
    if let Err(error) = db.record_forge_pull_requests(repo_id, &observations) {
        log::warn!("analytics: recording pull request facts failed: {error}");
        return ForgeAvailability::Unavailable;
    }
    ForgeAvailability::Confirmed
}

fn now_epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Whether the stored facts are fresh enough to skip asking again.
fn recently_reconciled(last_checked_epoch_seconds: Option<i64>, now_epoch_seconds: i64) -> bool {
    let Some(last) = last_checked_epoch_seconds else {
        return false;
    };
    now_epoch_seconds.saturating_sub(last) < RECHECK_AFTER.as_secs() as i64
}

/// One `gh` call for the repository's recent pull requests.
///
/// A page rather than a request per pull request: a repository with a hundred
/// open PRs would otherwise cost a hundred subprocesses, and Analytics is
/// looking at a window, not at all history. Pull requests older than the page
/// keep whatever was previously confirmed.
fn query_pull_requests(repo_path: &str) -> Option<Vec<ForgePullRequestObservation>> {
    let output = Command::new("gh")
        .args([
            "pr",
            "list",
            "--state",
            "all",
            "--limit",
            "200",
            "--json",
            "number,url,createdAt,mergedAt,state",
        ])
        .current_dir(repo_path)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        log::info!(
            "analytics: gh could not list pull requests ({}); merged counts stay unconfirmed",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    let parsed: Vec<GhPullRequest> = serde_json::from_slice(&output.stdout).ok()?;
    Some(
        parsed
            .into_iter()
            .map(|pull_request| ForgePullRequestObservation {
                pr_number: pull_request.number,
                url: pull_request.url,
                created_at: pull_request.created_at,
                // `gh` reports `mergedAt` as null for anything not merged;
                // an empty string from an older `gh` means the same thing.
                merged_at: pull_request
                    .merged_at
                    .filter(|merged_at| !merged_at.is_empty()),
                state: pull_request.state,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::{recently_reconciled, GhPullRequest};

    #[test]
    fn a_recent_confirmation_is_reused_and_an_old_one_is_not() {
        assert!(recently_reconciled(Some(1_000), 1_100));
        assert!(!recently_reconciled(Some(1_000), 1_400));
        assert!(!recently_reconciled(None, 1_000));
    }

    #[test]
    fn gh_output_parses_merged_and_unmerged_pull_requests() {
        let parsed: Vec<GhPullRequest> = serde_json::from_str(
            r#"[{"number":1,"url":"https://github.com/o/r/pull/1","createdAt":"2026-09-01T00:00:00Z","mergedAt":"2026-09-02T00:00:00Z","state":"MERGED"},
                {"number":2,"url":"https://github.com/o/r/pull/2","createdAt":"2026-09-03T00:00:00Z","mergedAt":null,"state":"OPEN"}]"#,
        )
        .expect("gh json");
        assert_eq!(parsed[0].merged_at.as_deref(), Some("2026-09-02T00:00:00Z"));
        assert_eq!(parsed[1].merged_at, None);
        assert_eq!(parsed[1].state.as_deref(), Some("OPEN"));
    }
}
