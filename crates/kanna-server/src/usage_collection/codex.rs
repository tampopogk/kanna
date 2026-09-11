//! Codex rollout usage.
//!
//! Codex writes a `token_count` event after each turn carrying two blocks:
//! `total_token_usage`, a running cumulative counter for the session, and
//! `last_token_usage`, that turn's own consumption. Only the per-turn block is
//! read. Summing the cumulative one would count every earlier turn again on
//! each event, and it also resets on compaction, so it cannot be differenced
//! reliably either.
//!
//! Codex counts `cached_input_tokens` *inside* `input_tokens`, unlike Claude,
//! so the cached share is subtracted out here to give every provider's rows
//! the same non-overlapping meaning.

use super::{ParsedUsage, SessionContext};
use serde_json::Value;

pub(super) fn parse_line(line: &str) -> (Option<ParsedUsage>, SessionContext) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return (None, SessionContext::default());
    };
    let payload = value.get("payload");
    let record_type = value.get("type").and_then(Value::as_str);

    match record_type {
        Some("session_meta") => {
            let context = payload
                .map(|payload| SessionContext {
                    session_id: string_at(payload, "session_id")
                        .or_else(|| string_at(payload, "id")),
                    cwd: string_at(payload, "cwd"),
                    model: None,
                })
                .unwrap_or_default();
            (None, context)
        }
        Some("turn_context") => {
            let context = payload
                .map(|payload| SessionContext {
                    session_id: None,
                    cwd: string_at(payload, "cwd"),
                    model: string_at(payload, "model"),
                })
                .unwrap_or_default();
            (None, context)
        }
        Some("event_msg") => {
            let Some(payload) = payload else {
                return (None, SessionContext::default());
            };
            if payload.get("type").and_then(Value::as_str) != Some("token_count") {
                return (None, SessionContext::default());
            }
            // A rate-limit-only `token_count` carries `info: null`. It reports
            // an allowance, not consumption, and is not usage.
            let Some(info) = payload.get("info").filter(|info| !info.is_null()) else {
                return (None, SessionContext::default());
            };
            let Some(last) = info.get("last_token_usage") else {
                return (None, SessionContext::default());
            };
            let Some(occurred_at) = string_at(&value, "timestamp") else {
                return (None, SessionContext::default());
            };

            let cached_input_tokens = integer_at(last, "cached_input_tokens");
            // Codex's `input_tokens` is inclusive of the cached share.
            let input_tokens = (integer_at(last, "input_tokens") - cached_input_tokens).max(0);
            let output_tokens = integer_at(last, "output_tokens");
            let reasoning_tokens = integer_at(last, "reasoning_output_tokens").min(output_tokens);

            // Identity comes from the record's own content, so history copied
            // into a resumed session's file — which keeps the original
            // timestamps and counts — resolves to the record already stored
            // rather than to a second one.
            let usage_key = format!(
                "codex:{occurred_at}:{cumulative}:{turn}",
                cumulative = info
                    .get("total_token_usage")
                    .map(|total| integer_at(total, "total_tokens"))
                    .unwrap_or_default(),
                turn = integer_at(last, "total_tokens"),
            );

            let parsed = ParsedUsage {
                usage_key,
                occurred_at,
                model: None,
                session_id: None,
                input_tokens,
                cached_input_tokens,
                cache_creation_tokens: integer_at(last, "cache_write_input_tokens"),
                output_tokens,
                reasoning_tokens,
            };
            (Some(parsed), SessionContext::default())
        }
        _ => (None, SessionContext::default()),
    }
}

fn string_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn integer_at(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0).max(0)
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    fn token_count(timestamp: &str, cumulative: i64, turn_input: i64, turn_cached: i64) -> String {
        format!(
            r#"{{"timestamp":"{timestamp}","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{cumulative},"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0,"total_tokens":{cumulative}}},"last_token_usage":{{"input_tokens":{turn_input},"cached_input_tokens":{turn_cached},"cache_write_input_tokens":3,"output_tokens":50,"reasoning_output_tokens":7,"total_tokens":{turn_total}}}}}}}}}"#,
            turn_total = turn_input + 50,
        )
    }

    #[test]
    fn a_turn_reports_only_its_own_consumption_with_cache_split_out() {
        let (usage, _) = parse_line(&token_count("2026-09-01T10:00:00.000Z", 9_000, 1_000, 400));
        let usage = usage.expect("token_count carries usage");
        // 1000 input of which 400 came from cache: 600 billed fresh.
        assert_eq!(usage.input_tokens, 600);
        assert_eq!(usage.cached_input_tokens, 400);
        assert_eq!(usage.cache_creation_tokens, 3);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.reasoning_tokens, 7);
        assert_eq!(usage.total_tokens(), 600 + 400 + 3 + 50);
    }

    #[test]
    fn the_running_total_is_never_what_gets_counted() {
        let first = parse_line(&token_count("2026-09-01T10:00:00.000Z", 1_000, 1_000, 0))
            .0
            .expect("first");
        let second = parse_line(&token_count("2026-09-01T10:05:00.000Z", 9_000, 500, 0))
            .0
            .expect("second");
        assert_eq!(first.input_tokens, 1_000);
        assert_eq!(
            second.input_tokens, 500,
            "the second turn must not re-count the first turn's input"
        );
    }

    #[test]
    fn copied_history_resolves_to_the_record_it_copied() {
        let line = token_count("2026-09-01T10:00:00.000Z", 1_000, 1_000, 0);
        assert_eq!(
            parse_line(&line).0.expect("original").usage_key,
            parse_line(&line).0.expect("copy").usage_key,
        );
    }

    #[test]
    fn two_turns_that_happen_to_match_in_size_stay_distinct() {
        let first = parse_line(&token_count("2026-09-01T10:00:00.000Z", 1_000, 1_000, 0))
            .0
            .expect("first");
        let second = parse_line(&token_count("2026-09-01T10:05:00.000Z", 2_000, 1_000, 0))
            .0
            .expect("second");
        assert_ne!(first.usage_key, second.usage_key);
    }

    #[test]
    fn a_rate_limit_only_event_is_not_usage() {
        let (usage, _) = parse_line(
            r#"{"timestamp":"2026-09-01T10:00:00.000Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":6.0}}}}"#,
        );
        assert!(usage.is_none());
    }

    #[test]
    fn session_and_turn_headers_supply_attribution_context() {
        let (_, meta) = parse_line(
            r#"{"type":"session_meta","payload":{"session_id":"s-1","cwd":"/w","cli_version":"0.117.0"}}"#,
        );
        assert_eq!(meta.session_id.as_deref(), Some("s-1"));
        assert_eq!(meta.cwd.as_deref(), Some("/w"));
        let (_, turn) =
            parse_line(r#"{"type":"turn_context","payload":{"cwd":"/w","model":"gpt-5.4"}}"#);
        assert_eq!(turn.model.as_deref(), Some("gpt-5.4"));
    }
}
