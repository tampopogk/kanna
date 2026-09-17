//! Durable standing supervision constraints, scoped per repository.
//!
//! A supervising manager carries constraints that are not facts about any one
//! task's state: an owner stand-down ("I'm driving that terminal, hands off"),
//! a release gate ("no production publish without an explicit go"), a
//! model-tier policy, a temporary routing decision with a planned revert.
//! Until this record existed they lived in exactly one place — the manager's
//! conversation — and conversation compaction is the one lossy event in this
//! system. It summarizes a carried constraint at the same rate as a stale
//! mailbox page, so the cheapest-looking decisions (the ones whose whole
//! content is "a constraint says don't") are precisely the ones a compaction
//! silently unmakes. A manager that has lost a stand-down does not fail
//! loudly; it intervenes in a session its owner asked it to leave alone.
//!
//! What this is **not**, deliberately:
//!
//! - **Not an enforcement engine.** Constraint text is opaque to the server.
//!   Nothing here refuses a stage advance, an input delivery, or a merge, and
//!   no code path reads `text` to decide anything. A constraint is an advisory
//!   fact a supervisor reads and applies with judgment; the moment the server
//!   started interpreting it, an unparseable sentence would become an outage
//!   and a typo would become policy.
//! - **Not `task_blocker`.** A blocker gates workflow progression and is
//!   derived state with a resolution rule of its own. A constraint gates
//!   nothing and resolves only when somebody clears it.
//! - **Not a key-value store.** Four kinds, one text, one optional subject.
//!   Anything a manager wants durable that is not a standing supervision
//!   constraint belongs in the record that already owns it — the task prompt,
//!   the input ledger, or a stage-run summary.
//!
//! **Nothing is deleted.** Clearing writes a clear timestamp and its own
//! provenance beside the declaration; the row stays readable as history. A
//! manager reconstructing its supervision state after a compaction has to be
//! able to see that a gate was lifted, by whom, and when — an absent row and a
//! lifted gate are the same observation, and only one of them is true.
//!
//! Provenance (`declared_by`, `cleared_by`) is **caller-declared and
//! unverified**, the same model the input ledger and revision origin use: a
//! local agent runs as the same OS user and can reach this API. What a row
//! proves is that *this* constraint, with this text, was recorded at this time
//! by a caller that claimed to be that source.

use super::{Db, TaskEventKind};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Longest constraint text accepted, in Unicode scalar values. A standing
/// constraint is a sentence a supervisor re-reads on every wake, not a
/// document; anything longer belongs in the task it is about.
pub const MAX_CONSTRAINT_TEXT_CHARS: usize = 2_000;

/// Longest note accepted on a clear.
pub const MAX_CONSTRAINT_NOTE_CHARS: usize = 500;

/// Default and maximum number of cleared constraints returned as history.
pub const DEFAULT_CLEARED_CONSTRAINT_TAIL: i64 = 50;
pub const MAX_CLEARED_CONSTRAINT_TAIL: i64 = 500;

/// What kind of supervision judgment a constraint carries.
///
/// A closed vocabulary rather than a free label, because the kinds are what a
/// reader scans for first and an open set degrades into synonyms. It does not
/// change how the server treats the row — every kind is equally advisory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StandingConstraintKind {
    /// Work is paused pending something outside the task.
    Hold,
    /// An action requires an explicit authorization before it happens.
    Gate,
    /// A standing rule about how work is done.
    Policy,
    /// Somebody else is driving; the supervisor does not intervene.
    StandDown,
}

impl StandingConstraintKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Gate => "gate",
            Self::Policy => "policy",
            Self::StandDown => "stand-down",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "hold" => Ok(Self::Hold),
            "gate" => Ok(Self::Gate),
            "policy" => Ok(Self::Policy),
            "stand-down" => Ok(Self::StandDown),
            other => Err(format!(
                "kind must be one of hold, gate, policy, stand-down; got {other}"
            )),
        }
    }

    /// Every kind, for the schema test that proves the CHECK constraint and
    /// this vocabulary cannot drift apart.
    #[cfg(test)]
    pub const ALL: &'static [Self] = &[Self::Hold, Self::Gate, Self::Policy, Self::StandDown];
}

/// Who a caller declared itself to be when it set or cleared a constraint.
///
/// Deliberately the same vocabulary as the input ledger's
/// [`super::TaskInputSource`], minus its reserved `engine` value: Kanna's own
/// supervisory machinery never declares a standing constraint, because a
/// constraint is somebody's decision and nothing here is authored by the
/// engine. Declared, never authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StandingConstraintSource {
    /// A human, or a human's words relayed by the agent they were said to.
    Operator,
    /// An orchestrating agent acting on its own authority.
    Manager,
    /// The caller declared nothing.
    Unspecified,
}

impl StandingConstraintSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Manager => "manager",
            Self::Unspecified => "unspecified",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "operator" => Ok(Self::Operator),
            "manager" => Ok(Self::Manager),
            "unspecified" => Ok(Self::Unspecified),
            other => Err(format!(
                "source must be one of operator, manager, unspecified; got {other}"
            )),
        }
    }

    /// Every declarable source. The reserved `engine` value of the input
    /// ledger is deliberately absent and must stay refused by the schema.
    #[cfg(test)]
    pub const ALL: &'static [Self] = &[Self::Operator, Self::Manager, Self::Unspecified];
}

/// One recorded standing constraint, live or cleared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandingConstraint {
    pub id: String,
    pub repo_id: String,
    pub kind: StandingConstraintKind,
    /// The constraint itself, verbatim. Never parsed by the server.
    pub text: String,
    /// The task this constraint is about, when it names one. A repository-wide
    /// gate names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_task_id: Option<String>,
    pub declared_by: StandingConstraintSource,
    /// The task session the declaring caller was running in, when it said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_by_task_id: Option<String>,
    pub created_at: String,
    /// `None` while the constraint stands. A cleared constraint keeps every
    /// field above unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_by: Option<StandingConstraintSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_by_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_note: Option<String>,
}

impl StandingConstraint {
    pub fn is_active(&self) -> bool {
        self.cleared_at.is_none()
    }

    /// The task this constraint's whole life is announced on, or `None` when
    /// it names no task at all.
    ///
    /// The task-event log is keyed by task — there is no repository-level feed
    /// — so a repository-wide constraint is announced on the session that
    /// declared it. Fixed at declaration and used for the clear as well, so a
    /// watcher scoped to that task sees both ends of one constraint rather
    /// than a set here and a clear wherever the next session happened to run.
    ///
    /// A constraint that names neither a subject nor a declaring session has
    /// nowhere to be announced. It is still durable and still returned by the
    /// read surface, and the write reports that it announced nothing rather
    /// than pretending otherwise.
    pub fn announcement_task_id(&self) -> Option<&str> {
        self.subject_task_id
            .as_deref()
            .or(self.declared_by_task_id.as_deref())
    }
}

/// A constraint to declare, before it has an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewStandingConstraint<'a> {
    pub repo_id: &'a str,
    pub kind: StandingConstraintKind,
    pub text: &'a str,
    pub subject_task_id: Option<&'a str>,
    pub declared_by: StandingConstraintSource,
    pub declared_by_task_id: Option<&'a str>,
}

/// The provenance a clear carries, which is its own and never the
/// declaration's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StandingConstraintClear<'a> {
    pub cleared_by: StandingConstraintSource,
    pub cleared_by_task_id: Option<&'a str>,
    pub note: Option<&'a str>,
}

/// Normalize constraint text, measured in Unicode scalar values.
pub fn normalize_constraint_text(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("text must not be empty".to_string());
    }
    if text.chars().count() > MAX_CONSTRAINT_TEXT_CHARS {
        return Err(format!(
            "text must contain at most {MAX_CONSTRAINT_TEXT_CHARS} trimmed Unicode characters"
        ));
    }
    Ok(text.to_string())
}

/// Normalize a clear note. An absent or blank note is simply no note.
pub fn normalize_constraint_note(note: Option<&str>) -> Result<Option<String>, String> {
    let Some(note) = note.map(str::trim).filter(|note| !note.is_empty()) else {
        return Ok(None);
    };
    if note.chars().count() > MAX_CONSTRAINT_NOTE_CHARS {
        return Err(format!(
            "note must contain at most {MAX_CONSTRAINT_NOTE_CHARS} trimmed Unicode characters"
        ));
    }
    Ok(Some(note.to_string()))
}

pub fn clamp_cleared_constraint_tail(tail: Option<i64>) -> i64 {
    tail.unwrap_or(DEFAULT_CLEARED_CONSTRAINT_TAIL)
        .clamp(1, MAX_CLEARED_CONSTRAINT_TAIL)
}

const CONSTRAINT_COLUMNS: &str = "id, repo_id, kind, text, subject_task_id, declared_by, \
     declared_by_task_id, created_at, cleared_at, cleared_by, cleared_by_task_id, cleared_note";

fn read_constraint(row: &rusqlite::Row<'_>) -> Result<StandingConstraint, rusqlite::Error> {
    let kind: String = row.get(2)?;
    let declared_by: String = row.get(5)?;
    let cleared_by: Option<String> = row.get(9)?;
    Ok(StandingConstraint {
        id: row.get(0)?,
        repo_id: row.get(1)?,
        // The CHECK constraints make these total; an unknown value would mean
        // the row was written around the schema, so fall back rather than
        // failing the whole read a manager depends on.
        kind: StandingConstraintKind::parse(&kind).unwrap_or(StandingConstraintKind::Policy),
        text: row.get(3)?,
        subject_task_id: row.get(4)?,
        declared_by: StandingConstraintSource::parse(&declared_by)
            .unwrap_or(StandingConstraintSource::Unspecified),
        declared_by_task_id: row.get(6)?,
        created_at: row.get(7)?,
        cleared_at: row.get(8)?,
        cleared_by: cleared_by.as_deref().map(|value| {
            StandingConstraintSource::parse(value).unwrap_or(StandingConstraintSource::Unspecified)
        }),
        cleared_by_task_id: row.get(10)?,
        cleared_note: row.get(11)?,
    })
}

impl Db {
    /// Declare a standing constraint, or resolve to the identical one that is
    /// already standing.
    ///
    /// Idempotent on (repository, kind, subject, exact text) among *active*
    /// rows, for the same reason a human review decision is idempotent on its
    /// reviewed head: the caller most likely to re-declare a constraint is a
    /// manager that just lost its conversation and is restating what it read
    /// back from this record, and turning that into a second row would make
    /// the history it is reading less legible with every recovery. The
    /// returned flag says whether this call created the row.
    ///
    /// A cleared row never absorbs a re-declaration — re-declaring a lifted
    /// gate is a new decision, with its own timestamp and its own provenance.
    pub fn record_standing_constraint(
        &self,
        constraint: NewStandingConstraint<'_>,
    ) -> Result<(StandingConstraint, bool), rusqlite::Error> {
        let text = normalize_constraint_text(constraint.text)
            .map_err(rusqlite::Error::InvalidParameterName)?;
        self.with_immediate_transaction(|db| {
            if let Some(existing) = db.find_active_standing_constraint(
                constraint.repo_id,
                constraint.kind,
                constraint.subject_task_id,
                &text,
            )? {
                return Ok((existing, false));
            }
            let id: String =
                db.conn
                    .query_row("SELECT 'sc-' || lower(hex(randomblob(16)))", [], |row| {
                        row.get(0)
                    })?;
            db.conn.execute(
                "INSERT INTO standing_constraint (
                     id, repo_id, kind, text, subject_task_id, declared_by, declared_by_task_id,
                     created_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, datetime('now'))",
                params![
                    id,
                    constraint.repo_id,
                    constraint.kind.as_str(),
                    text,
                    constraint.subject_task_id,
                    constraint.declared_by.as_str(),
                    constraint.declared_by_task_id,
                ],
            )?;
            let stored = db
                .read_standing_constraint(&id)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            if let Some(task_id) = stored.announcement_task_id() {
                db.append_task_event(
                    task_id,
                    TaskEventKind::StandingConstraintSet,
                    json!({
                        "constraintId": stored.id,
                        "repoId": stored.repo_id,
                        "kind": stored.kind.as_str(),
                        "text": stored.text,
                        "subjectTaskId": stored.subject_task_id,
                        "declaredBy": stored.declared_by.as_str(),
                        "declaredByTaskId": stored.declared_by_task_id,
                    }),
                )?;
            }
            Ok((stored, true))
        })
    }

    /// Clear a standing constraint, recording who cleared it.
    ///
    /// Returns `None` when no such constraint exists. Clearing an already
    /// cleared constraint is a no-op that returns the existing row with
    /// `false`: the clear that matters already happened, its provenance is
    /// already recorded, and a second announcement would read as a second
    /// decision.
    pub fn clear_standing_constraint(
        &self,
        constraint_id: &str,
        clear: StandingConstraintClear<'_>,
    ) -> Result<Option<(StandingConstraint, bool)>, rusqlite::Error> {
        let note =
            normalize_constraint_note(clear.note).map_err(rusqlite::Error::InvalidParameterName)?;
        self.with_immediate_transaction(|db| {
            let Some(existing) = db.read_standing_constraint(constraint_id)? else {
                return Ok(None);
            };
            if !existing.is_active() {
                return Ok(Some((existing, false)));
            }
            db.conn.execute(
                "UPDATE standing_constraint
                 SET cleared_at = datetime('now'),
                     cleared_by = ?,
                     cleared_by_task_id = ?,
                     cleared_note = ?
                 WHERE id = ? AND cleared_at IS NULL",
                params![
                    clear.cleared_by.as_str(),
                    clear.cleared_by_task_id,
                    note,
                    constraint_id,
                ],
            )?;
            let stored = db
                .read_standing_constraint(constraint_id)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            if let Some(task_id) = stored.announcement_task_id() {
                db.append_task_event(
                    task_id,
                    TaskEventKind::StandingConstraintCleared,
                    json!({
                        "constraintId": stored.id,
                        "repoId": stored.repo_id,
                        "kind": stored.kind.as_str(),
                        "text": stored.text,
                        "subjectTaskId": stored.subject_task_id,
                        "clearedBy": clear.cleared_by.as_str(),
                        "clearedByTaskId": clear.cleared_by_task_id,
                        "note": stored.cleared_note,
                    }),
                )?;
            }
            Ok(Some((stored, true)))
        })
    }

    pub fn read_standing_constraint(
        &self,
        constraint_id: &str,
    ) -> Result<Option<StandingConstraint>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {CONSTRAINT_COLUMNS} FROM standing_constraint WHERE id = ?"),
                [constraint_id],
                read_constraint,
            )
            .optional()
    }

    fn find_active_standing_constraint(
        &self,
        repo_id: &str,
        kind: StandingConstraintKind,
        subject_task_id: Option<&str>,
        text: &str,
    ) -> Result<Option<StandingConstraint>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {CONSTRAINT_COLUMNS} FROM standing_constraint
                     WHERE repo_id = ?
                       AND kind = ?
                       AND text = ?
                       AND subject_task_id IS ?
                       AND cleared_at IS NULL
                     ORDER BY created_at, id
                     LIMIT 1"
                ),
                params![repo_id, kind.as_str(), text, subject_task_id],
                read_constraint,
            )
            .optional()
    }

    /// Every constraint still standing in a repository, oldest first.
    ///
    /// Deliberately unbounded: the active set is what a supervisor must hold
    /// in full, and a truncated page of standing constraints is worse than no
    /// page at all — the one it drops is the one it then violates. The set is
    /// small by construction, and history is what gets a tail.
    pub fn list_active_standing_constraints(
        &self,
        repo_id: &str,
    ) -> Result<Vec<StandingConstraint>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {CONSTRAINT_COLUMNS} FROM standing_constraint
             WHERE repo_id = ? AND cleared_at IS NULL
             ORDER BY created_at, id"
        ))?;
        let rows = statement.query_map([repo_id], read_constraint)?;
        rows.collect()
    }

    /// Cleared constraints in a repository, most recently cleared first.
    pub fn list_cleared_standing_constraints(
        &self,
        repo_id: &str,
        tail: i64,
    ) -> Result<Vec<StandingConstraint>, rusqlite::Error> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {CONSTRAINT_COLUMNS} FROM standing_constraint
             WHERE repo_id = ? AND cleared_at IS NOT NULL
             ORDER BY cleared_at DESC, id DESC
             LIMIT ?"
        ))?;
        let rows = statement.query_map(params![repo_id, tail], read_constraint)?;
        rows.collect()
    }

    pub fn count_cleared_standing_constraints(
        &self,
        repo_id: &str,
    ) -> Result<i64, rusqlite::Error> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM standing_constraint
             WHERE repo_id = ? AND cleared_at IS NOT NULL",
            [repo_id],
            |row| row.get(0),
        )
    }
}
