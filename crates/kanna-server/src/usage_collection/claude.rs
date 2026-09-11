//! Claude CLI transcript usage.
//!
//! Each assistant line carries the `message.usage` block the API returned.
//! A streamed turn is written more than once with the same `message.id`, and
//! a forked or resumed conversation copies earlier lines verbatim into a new
//! transcript — so the message id, not the file or the line number, is the
//! record's identity.

use super::{ParsedUsage, SessionContext};
use serde_json::Value;

/// Parse one transcript line. Returns nothing for lines that carry no usage,
/// which is most of them.
pub(super) fn parse_line(line: &str) -> (Option<ParsedUsage>, SessionContext) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return (None, SessionContext::default());
    };
    let context = SessionContext {
        session_id: string_at(&value, "sessionId"),
        cwd: string_at(&value, "cwd"),
        model: value
            .get("message")
            .and_then(|message| string_at(message, "model")),
    };
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return (None, context);
    }
    let Some(message) = value.get("message") else {
        return (None, context);
    };
    let Some(usage) = message.get("usage") else {
        return (None, context);
    };

    // A message id is globally unique and survives being copied into another
    // transcript; a request id is the fallback for older transcripts that
    // predate it. Without either the record cannot be deduplicated, so it is
    // dropped rather than risk being counted twice.
    let identity = string_at(message, "id")
        .map(|id| format!("claude:message:{id}"))
        .or_else(|| string_at(&value, "requestId").map(|id| format!("claude:request:{id}")));
    let Some(usage_key) = identity else {
        return (None, context);
    };
    let Some(occurred_at) = string_at(&value, "timestamp") else {
        return (None, context);
    };

    let output_tokens = integer_at(usage, "output_tokens");
    let reasoning_tokens = usage
        .get("output_tokens_details")
        .map(|details| integer_at(details, "thinking_tokens"))
        .unwrap_or_default()
        // Never let a breakdown exceed the whole it is part of.
        .min(output_tokens);

    let parsed = ParsedUsage {
        usage_key,
        occurred_at,
        model: context.model.clone(),
        session_id: context.session_id.clone(),
        // Claude reports uncached input separately from cache reads already,
        // so `input_tokens` needs no subtraction.
        input_tokens: integer_at(usage, "input_tokens"),
        cached_input_tokens: integer_at(usage, "cache_read_input_tokens"),
        cache_creation_tokens: integer_at(usage, "cache_creation_input_tokens"),
        output_tokens,
        reasoning_tokens,
    };
    (Some(parsed), context)
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

    const ASSISTANT_LINE: &str = r#"{"type":"assistant","requestId":"req_1","sessionId":"sess-1","cwd":"/w","timestamp":"2026-09-01T10:00:00.000Z","message":{"id":"msg_1","model":"claude-opus-5","usage":{"input_tokens":11,"cache_creation_input_tokens":700,"cache_read_input_tokens":900,"output_tokens":40,"output_tokens_details":{"thinking_tokens":12}}}}"#;

    #[test]
    fn an_assistant_turn_reports_every_usage_field_without_overlap() {
        let (usage, context) = parse_line(ASSISTANT_LINE);
        let usage = usage.expect("assistant line carries usage");
        assert_eq!(usage.usage_key, "claude:message:msg_1");
        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.cache_creation_tokens, 700);
        assert_eq!(usage.cached_input_tokens, 900);
        assert_eq!(usage.output_tokens, 40);
        assert_eq!(usage.reasoning_tokens, 12);
        // Thinking is inside output, so it must not be added again.
        assert_eq!(usage.total_tokens(), 11 + 700 + 900 + 40);
        assert_eq!(context.cwd.as_deref(), Some("/w"));
        assert_eq!(context.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn a_copied_transcript_line_keeps_the_same_identity() {
        let copied = ASSISTANT_LINE.replace("\"sess-1\"", "\"sess-2\"");
        let (original, _) = parse_line(ASSISTANT_LINE);
        let (copy, _) = parse_line(&copied);
        assert_eq!(
            original.expect("original").usage_key,
            copy.expect("copy").usage_key,
            "a fork copying history must not create a second usage record"
        );
    }

    #[test]
    fn lines_without_usage_are_ignored_but_still_carry_session_context() {
        let (usage, context) = parse_line(
            r#"{"type":"user","sessionId":"sess-1","cwd":"/w","message":{"content":"hi"}}"#,
        );
        assert!(usage.is_none());
        assert_eq!(context.cwd.as_deref(), Some("/w"));
    }

    #[test]
    fn a_truncated_line_is_skipped_rather_than_failing_the_scan() {
        let (usage, context) = parse_line("{\"type\":\"assist");
        assert!(usage.is_none());
        assert_eq!(context, Default::default());
    }

    #[test]
    fn a_thinking_count_can_never_exceed_the_output_it_belongs_to() {
        let line = ASSISTANT_LINE.replace("\"thinking_tokens\":12", "\"thinking_tokens\":9999");
        let (usage, _) = parse_line(&line);
        let usage = usage.expect("usage");
        assert_eq!(usage.reasoning_tokens, usage.output_tokens);
    }
}
