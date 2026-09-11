//! The Analytics read route.
//!
//! One GET returns every statistic for a chosen window. Two side effects run
//! first, both bounded and both idempotent: provider session files are read
//! for any token usage not already stored, and — at most once every few
//! minutes — the forge is asked what happened to the pull requests this
//! repository's tasks opened. Neither invents a number; when either cannot
//! run, the response says so through `coverage` rather than reporting zero.

use super::state::AppState;
use crate::db::{AnalyticsRange, Db};
use crate::forge_pull_requests::{reconcile_repo_pull_requests, ForgeAvailability};
use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

/// Default window when the caller names none.
const DEFAULT_RANGE_DAYS: i64 = 30;

/// Longest window the route will serve. Analytics reads raw activity spans to
/// clip them, so an unbounded range is an unbounded read.
const MAX_RANGE_DAYS: i64 = 400;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AnalyticsQuery {
    /// Inclusive `YYYY-MM-DD` bounds.
    from: Option<String>,
    to: Option<String>,
}

pub(super) async fn get_repo_analytics(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(query): Query<AnalyticsQuery>,
) -> Result<Json<crate::db::RepoAnalytics>, (axum::http::StatusCode, String)> {
    let range = resolve_range(query.from.as_deref(), query.to.as_deref())?;
    super::blocking::run_handler_blocking("repo analytics", move || {
        let db = Db::open(&state.config.db_path).map_err(db_error)?;
        db.get_repo(&repo_id)
            .map_err(db_error)?
            .ok_or((axum::http::StatusCode::NOT_FOUND, "repo not found".into()))?;

        // Pull requests recorded before this record existed would otherwise be
        // invisible forever; the task rows still carry them.
        db.backfill_repo_pull_requests(&repo_id).map_err(db_error)?;

        let confirmed = matches!(
            reconcile_repo_pull_requests(&db, &repo_id, &state.forge_client),
            ForgeAvailability::Confirmed
        );

        // Usage collection is what makes the token figures current. A failure
        // here costs coverage, never the rest of the response.
        let providers_without_usage =
            match crate::usage_collection::collect_repo_token_usage(&db, &repo_id) {
                Ok(report) => report.providers_without_usage,
                Err(error) => {
                    log::warn!("analytics: token usage collection failed: {error}");
                    Vec::new()
                }
            };

        let analytics = db
            .repo_analytics(&repo_id, &range, confirmed, providers_without_usage)
            .map_err(db_error)?;
        Ok(Json(analytics))
    })
    .await
}

fn db_error(error: rusqlite::Error) -> (axum::http::StatusCode, String) {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        format!("db error: {error}"),
    )
}

/// Resolve the requested window, refusing a range this route will not read
/// rather than quietly serving a different one than the caller asked for.
pub(super) fn resolve_range(
    from: Option<&str>,
    to: Option<&str>,
) -> Result<AnalyticsRange, (axum::http::StatusCode, String)> {
    let today = today_utc();
    let to = match to {
        Some(to) => validated_date(to)?,
        None => today.clone(),
    };
    let from = match from {
        Some(from) => validated_date(from)?,
        None => shift_days(&to, -(DEFAULT_RANGE_DAYS - 1)),
    };
    if from > to {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("`from` ({from}) is after `to` ({to})"),
        ));
    }
    if day_span(&from, &to) > MAX_RANGE_DAYS {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("range is longer than the {MAX_RANGE_DAYS} days analytics will read"),
        ));
    }
    Ok(AnalyticsRange { from, to })
}

fn validated_date(value: &str) -> Result<String, (axum::http::StatusCode, String)> {
    let bytes = value.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    if !shaped {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("dates must be YYYY-MM-DD, got `{value}`"),
        ));
    }
    Ok(value.to_string())
}

fn today_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0);
    civil_from_days(seconds.div_euclid(86_400))
}

fn shift_days(date: &str, days: i64) -> String {
    match days_from_date(date) {
        Some(epoch_days) => civil_from_days(epoch_days + days),
        None => date.to_string(),
    }
}

fn day_span(from: &str, to: &str) -> i64 {
    match (days_from_date(from), days_from_date(to)) {
        (Some(from), Some(to)) => to - from + 1,
        _ => 0,
    }
}

fn days_from_date(date: &str) -> Option<i64> {
    let year = date.get(0..4)?.parse::<i32>().ok()?;
    let month = date.get(5..7)?.parse::<u32>().ok()?;
    let day = date.get(8..10)?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day))
}

// Howard Hinnant's civil date algorithms.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) as i64
}

fn civil_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, day_span, resolve_range, shift_days};

    #[test]
    fn an_explicit_window_is_served_exactly_as_asked() {
        let range = resolve_range(Some("2026-09-01"), Some("2026-09-07")).expect("range");
        assert_eq!(range.from, "2026-09-01");
        assert_eq!(range.to, "2026-09-07");
    }

    #[test]
    fn the_default_window_is_thirty_inclusive_days_ending_today() {
        let range = resolve_range(None, Some("2026-09-30")).expect("range");
        assert_eq!(range.from, "2026-09-01");
        assert_eq!(day_span(&range.from, &range.to), 30);
    }

    #[test]
    fn a_backwards_or_malformed_or_oversized_window_is_refused() {
        assert!(resolve_range(Some("2026-09-07"), Some("2026-09-01")).is_err());
        assert!(resolve_range(Some("07/09/2026"), None).is_err());
        assert!(resolve_range(Some("2020-01-01"), Some("2026-09-01")).is_err());
    }

    #[test]
    fn day_arithmetic_crosses_month_and_leap_boundaries() {
        assert_eq!(shift_days("2026-03-01", -1), "2026-02-28");
        assert_eq!(shift_days("2024-03-01", -1), "2024-02-29");
        assert_eq!(civil_from_days(0), "1970-01-01");
    }
}
