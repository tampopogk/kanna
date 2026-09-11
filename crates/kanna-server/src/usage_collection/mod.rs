//! Token accounting read from the agent CLIs' own session files.
//!
//! Kanna's agents run as real terminal CLIs, so there is no SDK stream to
//! instrument and the terminal itself carries only rendered chrome. What the
//! CLIs do leave behind is a structured local session log — Claude's
//! `~/.claude/projects/<slug>/<session>.jsonl` and Codex's
//! `~/.codex/sessions/**/rollout-*.jsonl` — and each records the usage its own
//! API responses reported. Those files are the source here. Nothing is
//! scraped from a terminal, nothing is inferred from a model name, and no
//! number here is a bill: a subscription does not produce a per-task price,
//! so this module deliberately stops at tokens.
//!
//! Two properties matter more than coverage:
//!
//! * **Never double count.** The same usage record reaches disk repeatedly —
//!   streamed writes of one assistant turn, a resumed session replaying its
//!   history, a fork copying a transcript into a new file, two stages sharing
//!   one provider session, a rescan of an unchanged file. Every record is
//!   therefore keyed by an identity derived from its own content, so seeing it
//!   again writes the same row. Cumulative counters are never summed; Codex's
//!   per-turn `last_token_usage` is used and its running total ignored.
//! * **Count only what was observed.** A provider with no readable usage is
//!   reported as uncovered, never as zero. Attribution that cannot be made
//!   with confidence leaves the record unassigned rather than guessing a task.

mod claude;
mod codex;
mod scan;

#[allow(unused_imports)]
pub use scan::{collect_repo_token_usage, CollectionReport};

/// One usage record as a provider's session file reported it, before it is
/// attributed to a task.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedUsage {
    /// Content-derived identity. Stable across files, rescans and forks.
    pub usage_key: String,
    pub occurred_at: String,
    pub model: Option<String>,
    pub session_id: Option<String>,
    /// Input billed fresh; never includes `cached_input_tokens`.
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_creation_tokens: i64,
    pub output_tokens: i64,
    /// The thinking share of `output_tokens`. A breakdown, not an addend.
    pub reasoning_tokens: i64,
}

impl ParsedUsage {
    /// The non-overlapping parts only. `reasoning_tokens` is deliberately
    /// absent: it is already inside `output_tokens`, and adding it would
    /// inflate every Codex total.
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens
            + self.cached_input_tokens
            + self.cache_creation_tokens
            + self.output_tokens
    }
}

/// What a file's parser learned about the session itself, carried across
/// incremental scans because a session file declares it only once at the top.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionContext {
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
}

impl SessionContext {
    fn absorb(&mut self, other: SessionContext) {
        if other.session_id.is_some() {
            self.session_id = other.session_id;
        }
        if other.cwd.is_some() {
            self.cwd = other.cwd;
        }
        if other.model.is_some() {
            self.model = other.model;
        }
    }
}
