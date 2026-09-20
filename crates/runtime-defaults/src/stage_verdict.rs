//! The words an agent may use to record what happened at a workflow stage.
//!
//! Until 2026-09-19 the vocabulary was two words on the wire — `success` and
//! `failure` — and that is why a task's recorded history was unreadable. An
//! agent with working code and no test run, an agent that finished half the
//! scope, an agent that lacked the information to proceed, and an agent that
//! correctly concluded the work should not be done all had to write
//! `failure`, indistinguishable from a crash. Owner decision, 2026-09-19: the
//! cheap move must not be to build the thing anyway, so being right about
//! "this should not be done" gets its own word.
//!
//! One table, so the MCP schema an agent picks a word out of, the CLI that
//! validates it without a JSON-Schema validator in front of it, and the server
//! that records it all name the same six words. A contract test in
//! `kanna-tool-catalog` holds the advertised enum to this list.
//!
//! # What this is not
//!
//! - It is **not** the `stage_run.status` column, which remains the engine's
//!   own `pending` / `running` / `succeeded` / `failed` / `cancelled`
//!   lifecycle enum. A verdict of `partial` is recorded on a run whose column
//!   says `failed`, exactly as `failure` always was.
//! - `closed` is **not** in it. A closed task is a lifecycle fact — read
//!   `closedAt` or the `task.closed` event — never a verdict about work, and
//!   recording it as one made "somebody stopped this" and "the agent failed"
//!   the same observation.
//! - It does not duplicate the attention badge, `task_blocker`, or the
//!   `waiting` runtime state. Those are live state; a verdict is what one
//!   finished run reported.

/// One recorded stage verdict.
///
/// Ordered as an agent should read them: the two that describe finished work,
/// the two that describe work deliberately left undone, and the one that
/// describes work attempted and lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageVerdict {
    /// Did the work, verified it.
    Success,
    /// Did the work, could not prove it. The summary must say what is
    /// unproven.
    Unverified,
    /// Did some of the scope. The summary must say what remains.
    Partial,
    /// Stopped because the task as specified does not say enough to proceed.
    /// The summary must state the specific question.
    NeedsInput,
    /// Deliberately did not do it — the premise was wrong, it was already
    /// done, or it should not be done. The summary must say which.
    Declined,
    /// Tried, could not.
    Failure,
}

/// Every verdict, in the order the agent-facing surfaces list them.
pub const STAGE_VERDICTS: [StageVerdict; 6] = [
    StageVerdict::Success,
    StageVerdict::Unverified,
    StageVerdict::Partial,
    StageVerdict::NeedsInput,
    StageVerdict::Declined,
    StageVerdict::Failure,
];

impl StageVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Unverified => "unverified",
            Self::Partial => "partial",
            Self::NeedsInput => "needs-input",
            Self::Declined => "declined",
            Self::Failure => "failure",
        }
    }

    /// Whether this verdict is the workflow engine's idea of a completed
    /// stage.
    ///
    /// Only `success` is. Every other word records what happened and stops
    /// advancement, which is precisely what `failure` alone used to do — the
    /// vocabulary got wider, the engine did not change.
    pub fn completes_stage(self) -> bool {
        matches!(self, Self::Success)
    }

    /// The `stage_run.status` a run carrying this verdict is closed with.
    ///
    /// Deliberately still two-valued. Widening the lifecycle column would
    /// change what every existing consumer of `run.finished` and
    /// `latestRun.status` sees; the new information lives in the verdict.
    pub fn run_status(self) -> &'static str {
        if self.completes_stage() {
            "succeeded"
        } else {
            "failed"
        }
    }

    /// Parse a caller-supplied word.
    ///
    /// Strictly closed: an unrecognized value is refused rather than coerced,
    /// because coercion would invent a verdict nobody recorded and the caller
    /// is a live agent that can correct itself.
    pub fn parse(value: &str) -> Result<Self, String> {
        STAGE_VERDICTS
            .into_iter()
            .find(|verdict| verdict.as_str() == value)
            .ok_or_else(|| {
                format!(
                    "status must be one of {}, got {value}",
                    stage_verdict_names().join(", ")
                )
            })
    }
}

/// The advertised vocabulary, as the MCP schema and the CLI spell it.
pub fn stage_verdict_names() -> Vec<String> {
    STAGE_VERDICTS
        .into_iter()
        .map(|verdict| verdict.as_str().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_success_completes_a_stage() {
        for verdict in STAGE_VERDICTS {
            assert_eq!(
                verdict.completes_stage(),
                verdict == StageVerdict::Success,
                "{} must not advance the workflow",
                verdict.as_str()
            );
            assert_eq!(
                verdict.run_status(),
                if verdict == StageVerdict::Success {
                    "succeeded"
                } else {
                    "failed"
                }
            );
        }
    }

    #[test]
    fn the_two_historic_words_still_parse() {
        assert_eq!(StageVerdict::parse("success"), Ok(StageVerdict::Success));
        assert_eq!(StageVerdict::parse("failure"), Ok(StageVerdict::Failure));
    }

    /// `closed` left the vocabulary. This is the inbound half of that rule;
    /// the outbound half — a row that still carries it is reported verbatim
    /// rather than rewritten — is proved at the HTTP surface, in
    /// `a_verdict_outside_the_vocabulary_is_reported_verbatim_rather_than_rewritten`.
    #[test]
    fn closed_is_refused_from_a_caller() {
        assert!(StageVerdict::parse("closed").is_err());
    }

    #[test]
    fn the_refusal_names_every_accepted_word() {
        let error = StageVerdict::parse("maybe").expect_err("maybe is not a verdict");
        for verdict in STAGE_VERDICTS {
            assert!(
                error.contains(verdict.as_str()),
                "{error} omits {verdict:?}"
            );
        }
    }
}
