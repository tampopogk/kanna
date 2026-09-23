//! The durable facts a task directory carries beside its ledger (spec §11,
//! §16.11 — component T13, second increment).
//!
//! T0's `task.json` names a task; its ledger records what happened. Neither
//! held the rest of a task's durable state — its runs and their sessions,
//! workspaces, branch counter, commit steps, edges, joins, owed work,
//! transfer and review records — so a database rebuilt from disk lost them.
//! This module is the list of those facts and their disk form:
//!
//! - [`CARRIED_TABLES`]: every table holding durable task state, with each
//!   column either carried or left out for a stated reason. `task.json`
//!   gains a `state` object (see [`Db::task_state_record`]) holding the
//!   task's rows of each table, verbatim, keyed by table name, with their
//!   `rowid` (several readers order runs and intents by it).
//! - [`NOT_CARRIED_TABLES`]: every other table, and why a database rebuilt
//!   from disk starts it empty or restores it from another record.
//! - `repos/<repo-id>/repo.json`: the repository's registration and sidebar
//!   order ([`Db::repo_disk_record`]).
//!
//! **Same transaction.** Triggers ([`sync_disk_state_triggers`], installed
//! once migration `103_disk_state_records` has run) bump T0's
//! `task_ledger_snapshot.revision` (or `repo_disk_snapshot.revision`) in the
//! statement that changes a carried row, so no writer can change carried
//! state without owing a new `task.json`; the publisher rewrites it from the
//! current rows. Every migration that runs does so without them (SQLite
//! checks triggers when a table is altered or rebuilt), and they are
//! re-installed, to this build's definition, after the last one.
//!
//! **Removal.** A task or repository deleted from the database (a failed
//! creation rolled back, a repository unregistered) leaves a row in
//! `disk_record_removal`, in the deleting statement, and the publisher
//! replaces its `task.json`/`repo.json` with a tombstone, so a rebuild does
//! not resurrect it. Ledger files are never deleted.
//!
//! **Secrets** never enter these records: no table here holds a credential,
//! and the one capability-like column (an incoming transfer's claim token)
//! is left out.

use super::Db;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// The version of `task.json`'s `state` object and of `repo.json`.
pub const DISK_STATE_VERSION: u64 = 1;

/// One table whose rows are durable task state.
#[derive(Debug, Clone, Copy)]
pub struct CarriedTable {
    pub table: &'static str,
    /// Selects the task's rows; `?1` is the task id.
    pub rows_of_task: &'static str,
    /// The task a row belongs to, as an SQL expression over `{r}` (`NEW` or
    /// `OLD` in a trigger).
    pub owner: &'static str,
    /// Carried verbatim.
    pub columns: &'static [&'static str],
    /// Carried, but a change to them alone does not rewrite `task.json`
    /// (they move with every unrelated write).
    pub quiet: &'static [&'static str],
    /// Not carried, and why. Read by the classification check and by
    /// people; nothing at runtime needs it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub left_out: &'static [(&'static str, &'static str)],
}

const OWNED_BY_TASK_ID: &str = "task_id = ?1";
const RUN_OF_TASK: &str = "run_id IN (SELECT id FROM stage_run WHERE task_id = ?1)";
const RUN_OWNER: &str = "(SELECT task_id FROM stage_run WHERE id = {r}.run_id)";

/// Carried tables, in the order a rebuild inserts them (foreign keys first).
pub const CARRIED_TABLES: &[CarriedTable] = &[
    CarriedTable {
        table: "pipeline_item",
        rows_of_task: "id = ?1",
        owner: "{r}.id",
        columns: &[
            "id",
            "repo_id",
            "issue_number",
            "issue_title",
            "prompt",
            "pipeline_def",
            "stage",
            "pr_number",
            "pr_url",
            "branch",
            "agent_type",
            "agent_spawn_options",
            "created_at",
            "updated_at",
            "pinned",
            "pin_order",
            "display_name",
            "closed_at",
            "base_ref",
            "agent_provider",
            "pipeline",
            "notify_task_id",
            "notified_at",
            "parent_task_id",
            "pr_branch",
            "cloud_task_id",
            "revision_rounds",
            "initial_pipeline",
            "merge_signaled_at",
            "attention_requested",
        ],
        quiet: &["updated_at"],
        left_out: &[
            ("activity", "live display state, derived from the session"),
            ("activity_changed_at", "live display state"),
            ("activity_revision", "live display counter"),
            ("activity_event_baseline", "live event debounce"),
            ("activity_event_pending_at", "live event debounce"),
            ("runtime_status", "live session state reported by the daemon"),
            ("runtime_event_baseline", "live event debounce"),
            ("runtime_event_pending_at", "live event debounce"),
            ("unread_at", "live read state"),
            ("last_output_preview", "live terminal preview"),
            ("composer_text", "an unsent draft in a client"),
            ("composer_attestation", "an unsent draft in a client"),
            ("agent_session_id", "the live session mirror; each run carries its own provider session id"),
            ("port_offset", "port lease, reallocated per session"),
            ("port_env", "port lease, reallocated per session"),
            ("blocker_revision", "a display counter maintained by triggers"),
            ("blocked_event_baseline", "live event debounce"),
            ("teardown_started_at", "a teardown in progress; its run (stage_run kind teardown) is carried"),
        ],
    },
    CarriedTable {
        table: "stage_run",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "id",
            "task_id",
            "stage",
            "kind",
            "agent",
            "agent_provider",
            "model",
            "effort",
            "status",
            "result",
            "feedback",
            "session_id",
            "provider_session_id",
            "cwd",
            "resumed_from_run_id",
            "replaces_run_id",
            "no_work_termination",
            "resume_fallback_reason",
            "completion_transition",
            "trigger",
            "completion_bound",
            "started_at",
            "finished_at",
            "provider_override",
            "entry_channel_identity",
            "result_declared_role",
            "result_channel_identity",
            "workspace_id",
            "session_branch",
            "session_name",
            "transcript_ref",
            "workspace_report",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "stage_run_prompt",
        rows_of_task: RUN_OF_TASK,
        owner: RUN_OWNER,
        columns: &["run_id", "resolved_prompt", "created_at"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "workspace_setup_run",
        rows_of_task: RUN_OF_TASK,
        owner: RUN_OWNER,
        columns: &[
            "run_id",
            "status",
            "exit_code",
            "timed_out",
            "truncated",
            "commands",
            "output",
            "duration_ms",
            "finished_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "contextless_completion_attempt",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &["task_id", "attempt_key", "run_id", "result"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_provider_rejection",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "id",
            "task_id",
            "stage_run_id",
            "stage",
            "provider",
            "model",
            "effort",
            "source",
            "rule_id",
            "matched_text",
            "scope",
            "cli_version",
            "recovery",
            "replacement_run_id",
            "observed_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_provider_capacity_notice",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "id",
            "task_id",
            "stage_run_id",
            "stage",
            "provider",
            "model",
            "effort",
            "source",
            "rule_id",
            "matched_text",
            "scope",
            "cli_version",
            "observed_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "worktree",
        rows_of_task: "pipeline_item_id = ?1",
        owner: "{r}.pipeline_item_id",
        columns: &[
            "id",
            "pipeline_item_id",
            "path",
            "branch",
            "created_at",
            "setup_pending",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "stage_workspace",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "id",
            "task_id",
            "stage",
            "path",
            "branch",
            "created_at",
            "updated_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_branch_counter",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &["task_id", "last_allocated", "updated_at"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_stage_budget",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &["task_id", "stage", "spent", "updated_at"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "transition_commit",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "run_id",
            "task_id",
            "stage",
            "exit",
            "state",
            "result_id",
            "committed_sha",
            "created_at",
            "settled_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_stage_edge",
        rows_of_task: "dependent_task_id = ?1",
        owner: "{r}.dependent_task_id",
        columns: &[
            "id",
            "dependent_task_id",
            "dependent_stage",
            "upstream_task_id",
            "upstream_stage",
            "position",
            "created_at",
            "consumed_result_id",
            "consumed_sha",
            "consumed_at",
            "superseded_result_id",
            "superseded_sha",
            "superseded_at",
            "reserved_result_id",
            "reserved_sha",
            "reserved_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_dependency_wait",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "task_id",
            "from_stage",
            "to_stage",
            "generation",
            "payload",
            "created_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_join",
        rows_of_task: "parent_task_id = ?1",
        owner: "{r}.parent_task_id",
        columns: &[
            "id",
            "parent_task_id",
            "parent_stage",
            "parent_run_id",
            "base_sha",
            "base_branch",
            "created_at",
            "completed_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_join_member",
        rows_of_task: "join_id IN (SELECT id FROM task_join WHERE parent_task_id = ?1)",
        owner: "(SELECT parent_task_id FROM task_join WHERE id = {r}.join_id)",
        columns: &[
            "join_id",
            "position",
            "child_task_id",
            "spec",
            "create_error",
            "resolved_at",
            "outcome",
            "result_id",
            "result_status",
            "result_stage",
            "result_sha",
            "input_id",
            "notified_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "create_task_intent",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &["task_id", "request_json", "created_at"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "lifecycle_operation_intent",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &["id", "task_id", "kind", "phase", "payload_json", "created_at"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_ledger_sequence",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &["task_id", "high_water"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_ledger_continuation",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &["task_id", "operation_id", "kind", "payload", "created_at"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_review_context",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "task_id",
            "version",
            "pr_url",
            "head_repo",
            "head_ref",
            "head_sha",
            "base_ref",
            "base_sha",
            "producing_task_id",
            "producing_machine_id",
            "triage_parent_task_id",
            "triage_rank",
            "related_pr_urls",
            "updated_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "human_review_decision",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "id",
            "task_id",
            "review_context_version",
            "pr_url",
            "head",
            "head_sha",
            "base_ref",
            "base_sha",
            "action_text",
            "origin",
            "device_provenance",
            "source_machine_id",
            "created_at",
            "delivery_status",
            "delivery_detail",
            "delivered_at",
            "merge_task_id",
            "owner_desktop_id",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_transfer",
        rows_of_task: "local_task_id = ?1",
        owner: "{r}.local_task_id",
        columns: &[
            "id",
            "direction",
            "status",
            "source_peer_id",
            "target_peer_id",
            "source_task_id",
            "local_task_id",
            "started_at",
            "completed_at",
            "error",
            "payload_json",
            "source_desktop_id",
            "target_desktop_id",
            "sidecar_cleanup_completed_at",
            "dismissed_at",
        ],
        quiet: &[],
        left_out: &[
            (
                "claim_owner_token",
                "a capability held by the process that claimed an incoming transfer for a 30-second lease; it never leaves the database, and restart recovery re-claims a claimed transfer under a new token",
            ),
            (
                "claim_expires_at",
                "that lease's expiry, renewed every few seconds; meaningless after a restart, whose recovery re-claims",
            ),
        ],
    },
    CarriedTable {
        table: "task_transfer_provenance",
        rows_of_task: "pipeline_item_id = ?1",
        owner: "{r}.pipeline_item_id",
        columns: &[
            "pipeline_item_id",
            "source_peer_id",
            "source_task_id",
            "source_machine_task_label",
            "imported_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "task_transfer_workflow_claim",
        rows_of_task: "pipeline_item_id = ?1",
        owner: "{r}.pipeline_item_id",
        columns: &["pipeline_item_id", "transfer_id", "claimed_at"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "transfer_ledger_export",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &["task_id", "transfer_id", "exported_through", "exported_at"],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "transferred_task_context",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "task_id",
            "transfer_id",
            "workflow_definition",
            "previous_stage_result",
            "previous_main_result",
            "revision_feedback",
            "recorded_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "transferred_task_manifest",
        rows_of_task: "local_task_id = ?1",
        owner: "{r}.local_task_id",
        columns: &[
            "transfer_id",
            "repo_id",
            "local_task_id",
            "head_oid",
            "base_oid",
            "state",
            "created_at",
            "prepared_at",
            "content_commitment",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "transferred_task_history",
        rows_of_task: OWNED_BY_TASK_ID,
        owner: "{r}.task_id",
        columns: &[
            "task_id",
            "sequence",
            "origin_peer_id",
            "origin_task_id",
            "origin_run_id",
            "stage",
            "kind",
            "agent",
            "result",
            "feedback",
            "finished_at",
            "recorded_at",
        ],
        quiet: &[],
        left_out: &[],
    },
    CarriedTable {
        table: "transferred_task_state",
        rows_of_task: "pipeline_item_id = ?1",
        owner: "{r}.pipeline_item_id",
        columns: &[
            "pipeline_item_id",
            "transfer_id",
            "source_peer_id",
            "source_task_id",
            "ownership_generation",
            "state_sha256",
            "links",
            "session_start",
            "fresh_start_reason",
            "imported_at",
        ],
        quiet: &[],
        left_out: &[],
    },
];

/// Why a table is not carried in `state`.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotCarried {
    /// Restored from another disk record (named in the reason).
    OtherRecord,
    /// Statistics: a rebuilt database starts them empty.
    Statistics,
    /// Live-session or in-flight state that is re-derived at runtime.
    Transient,
    /// Durable, but meaningless in a rebuilt database (reason says why).
    CannotRebuild,
    /// Describes this database file or this machine, not a task.
    Machine,
}

/// Every table that is not in [`CARRIED_TABLES`], and why. The
/// classification check refuses a table in neither list.
#[cfg_attr(not(test), allow(dead_code))]
pub const NOT_CARRIED_TABLES: &[(&str, NotCarried, &str)] = &[
    ("repo", NotCarried::OtherRecord, "repos/<repo-id>/repo.json"),
    ("repo_sidebar_order", NotCarried::OtherRecord, "repos/<repo-id>/repo.json (sidebar order of the repository's remote)"),
    ("task_blocker", NotCarried::OtherRecord, "task.json links.dependencies"),
    ("task_input", NotCarried::OtherRecord, "the ledger's input entries"),
    ("task_ledger_entry", NotCarried::OtherRecord, "the ledger files themselves"),
    ("task_ledger_snapshot", NotCarried::OtherRecord, "this database's publication bookkeeping; a rebuild marks every task.json current"),
    ("task_ledger_backfill", NotCarried::OtherRecord, "this database's backfill bookkeeping; a rebuild marks every history imported"),
    ("repo_disk_snapshot", NotCarried::OtherRecord, "this database's publication bookkeeping for repo.json"),
    ("disk_record_removal", NotCarried::OtherRecord, "this database's publication outbox for tombstones"),
    ("activity_log", NotCarried::Statistics, "activity time accounting"),
    ("task_activity_interval", NotCarried::Statistics, "activity time accounting"),
    ("operator_event", NotCarried::Statistics, "operator interaction log"),
    ("provider_token_usage", NotCarried::Statistics, "provider usage accounting"),
    ("provider_usage_scan", NotCarried::Statistics, "provider usage scanner checkpoint"),
    ("provider_usage_discovery", NotCarried::Statistics, "provider usage scanner discovery state"),
    ("task_revision", NotCarried::Statistics, "revision request analytics"),
    ("task_pull_request", NotCarried::Statistics, "forge facts behind the analytics view"),
    ("task_port", NotCarried::Transient, "port leases, reallocated per session"),
    ("terminal_session", NotCarried::Transient, "live terminal bindings"),
    ("transfer_work", NotCarried::Transient, "the transfer engine's in-flight queue; restart recovery re-derives it from task_transfer"),
    ("transfer_work_phase", NotCarried::Transient, "the transfer engine's in-flight queue"),
    ("task_event", NotCarried::Transient, "the event feed, pruned after 14 days; the ledger is the record"),
    ("task_event_cursor_handle", NotCarried::Transient, "named event cursors, which expire"),
    ("copilot_wake_registration", NotCarried::Transient, "a live session's wake transport"),
    ("copilot_wake_attempt", NotCarried::Transient, "a live session's wake transport"),
    ("claude_channel_registration", NotCarried::Transient, "a live session's MCP channel"),
    ("claude_channel_attempt", NotCarried::Transient, "a live session's MCP channel"),
    ("event_subscription", NotCarried::CannotRebuild, "a subscriber's position is a task_event sequence number, and a rebuilt database restarts that feed empty: a restored position would silently skip or replay events, so the subscriber subscribes again"),
    ("task_serviced_watermark", NotCarried::CannotRebuild, "a manager's watermark is a task_event sequence number (see event_subscription)"),
    ("agent_terminal_attempt", NotCarried::CannotRebuild, "the final terminal frame of a run is a capture of session output, not task state; the run carries its transcript reference"),
    ("agent_run", NotCarried::Machine, "legacy table from the base schema with no reader or writer since stage_run replaced it"),
    ("trusted_peer", NotCarried::Machine, "this machine's paired peers and their public keys: pairing state whose authority is the machine's pairing store, not any task"),
    ("settings", NotCarried::Machine, "this machine's preferences (local config)"),
    ("schema_migrations", NotCarried::Machine, "describes this database file"),
    ("sqlite_sequence", NotCarried::Machine, "SQLite's own AUTOINCREMENT bookkeeping, re-derived from the inserted rows"),
];

/// A `task.json` or `repo.json` tombstone.
pub const REMOVED_KEY: &str = "removed";

pub(super) const SCHEMA: &str = r#"
    CREATE TABLE IF NOT EXISTS repo_disk_snapshot (
        -- No foreign key: a removed repository still owes its tombstone.
        repo_id TEXT PRIMARY KEY,
        revision INTEGER NOT NULL DEFAULT 1,
        published_revision INTEGER NOT NULL DEFAULT 0,
        publish_error TEXT
    );
    CREATE TABLE IF NOT EXISTS task_ledger_sequence (
        -- Every ledger sequence ever allocated to the task: allocation is
        -- always above it, so a sequence is never handed out twice.
        task_id TEXT PRIMARY KEY REFERENCES pipeline_item(id) ON DELETE CASCADE,
        high_water INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS disk_record_removal (
        kind TEXT NOT NULL CHECK (kind IN ('task', 'repo')),
        id TEXT NOT NULL,
        repo_id TEXT NOT NULL,
        removed_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
        publish_error TEXT,
        PRIMARY KEY (kind, id)
    );
"#;

/// Owe a new `task.json`. Every task has a `task_ledger_snapshot` row: the
/// task's insert trigger creates it, and migration `103` gave one to every
/// task that predates it.
fn bump_task(owner: &str) -> String {
    format!("UPDATE task_ledger_snapshot SET revision = revision + 1 WHERE task_id = {owner};")
}

fn bump_repos(where_clause: &str) -> String {
    format!("UPDATE repo_disk_snapshot SET revision = revision + 1 WHERE {where_clause};")
}

/// `AFTER UPDATE` of the carried columns that are not quiet, when the table
/// has columns that change without owing a new record; plain `AFTER UPDATE`
/// otherwise.
fn update_event(columns: &[&str], quiet: &[&str], left_out: usize) -> String {
    if quiet.is_empty() && left_out == 0 {
        return "UPDATE".into();
    }
    let watched: Vec<&str> = columns
        .iter()
        .copied()
        .filter(|column| !quiet.contains(column))
        .collect();
    format!("UPDATE OF {}", watched.join(", "))
}

/// The triggers that keep `task.json`/`repo.json` owed whenever what they
/// carry changes, and that record removals: `(name, CREATE TRIGGER ...)`.
///
/// Every connection parses them when it opens, so they are kept short: one
/// statement each, no column-by-column comparison. A write that sets a
/// carried column to its current value still owes a (identical) record. A
/// row that moves from one task to another is re-recorded for its new task
/// only; no writer moves one.
fn disk_state_triggers() -> Vec<(String, String)> {
    let mut triggers = Vec::new();
    let mut trigger = |name: String, sql: String| triggers.push((name, sql));
    for table in CARRIED_TABLES {
        let name = table.table;
        let owner = |row: &str| table.owner.replace("{r}", row);
        let update = update_event(table.columns, table.quiet, table.left_out.len());
        if name == "pipeline_item" {
            // A new task owes its first task.json, and a task re-created
            // under a removed id is no longer removed.
            trigger(
                "disk_state_pipeline_item_insert".into(),
                "CREATE TRIGGER disk_state_pipeline_item_insert AFTER INSERT ON pipeline_item
                 BEGIN
                   INSERT INTO task_ledger_snapshot (task_id) VALUES (NEW.id)
                   ON CONFLICT(task_id) DO UPDATE SET revision = revision + 1;
                   DELETE FROM disk_record_removal WHERE kind = 'task' AND id = NEW.id;
                 END"
                .into(),
            );
            // A removed task owes a tombstone instead of a task.json.
            trigger(
                "disk_state_pipeline_item_delete".into(),
                "CREATE TRIGGER disk_state_pipeline_item_delete AFTER DELETE ON pipeline_item
                 BEGIN
                   INSERT INTO disk_record_removal (kind, id, repo_id)
                   VALUES ('task', OLD.id, OLD.repo_id)
                   ON CONFLICT(kind, id) DO UPDATE SET
                     repo_id = excluded.repo_id, removed_at = excluded.removed_at,
                     publish_error = NULL;
                 END"
                .into(),
            );
        } else {
            trigger(
                format!("disk_state_{name}_insert"),
                format!(
                    "CREATE TRIGGER disk_state_{name}_insert AFTER INSERT ON {name} BEGIN {} END",
                    bump_task(&owner("NEW"))
                ),
            );
            trigger(
                format!("disk_state_{name}_delete"),
                format!(
                    "CREATE TRIGGER disk_state_{name}_delete AFTER DELETE ON {name} BEGIN {} END",
                    bump_task(&owner("OLD"))
                ),
            );
        }
        trigger(
            format!("disk_state_{name}_update"),
            format!(
                "CREATE TRIGGER disk_state_{name}_update AFTER {update} ON {name} BEGIN {} END",
                bump_task(&owner("NEW"))
            ),
        );
    }
    trigger(
        "disk_state_repo_insert".into(),
        "CREATE TRIGGER disk_state_repo_insert AFTER INSERT ON repo
         BEGIN
           INSERT INTO repo_disk_snapshot (repo_id) VALUES (NEW.id)
           ON CONFLICT(repo_id) DO UPDATE SET revision = revision + 1;
           DELETE FROM disk_record_removal WHERE kind = 'repo' AND id = NEW.id;
         END"
        .into(),
    );
    trigger(
        "disk_state_repo_update".into(),
        format!(
            "CREATE TRIGGER disk_state_repo_update AFTER {} ON repo BEGIN {} END",
            update_event(REPO_COLUMNS, REPO_QUIET_COLUMNS, 0),
            bump_repos("repo_id = NEW.id")
        ),
    );
    trigger(
        "disk_state_repo_delete".into(),
        "CREATE TRIGGER disk_state_repo_delete AFTER DELETE ON repo
         BEGIN
           DELETE FROM repo_disk_snapshot WHERE repo_id = OLD.id;
           INSERT INTO disk_record_removal (kind, id, repo_id)
           VALUES ('repo', OLD.id, OLD.id)
           ON CONFLICT(kind, id) DO UPDATE SET
             removed_at = excluded.removed_at, publish_error = NULL;
         END"
        .into(),
    );
    let sidebar = |row: &str| {
        bump_repos(&format!(
            "repo_id IN (SELECT id FROM repo WHERE remote_url_hash = {row}.remote_url_hash)"
        ))
    };
    trigger(
        "disk_state_repo_sidebar_order_insert".into(),
        format!(
            "CREATE TRIGGER disk_state_repo_sidebar_order_insert AFTER INSERT ON repo_sidebar_order BEGIN {} END",
            sidebar("NEW")
        ),
    );
    trigger(
        "disk_state_repo_sidebar_order_update".into(),
        format!(
            "CREATE TRIGGER disk_state_repo_sidebar_order_update AFTER UPDATE ON repo_sidebar_order BEGIN {} {} END",
            sidebar("OLD"),
            sidebar("NEW")
        ),
    );
    trigger(
        "disk_state_repo_sidebar_order_delete".into(),
        format!(
            "CREATE TRIGGER disk_state_repo_sidebar_order_delete AFTER DELETE ON repo_sidebar_order BEGIN {} END",
            sidebar("OLD")
        ),
    );
    triggers
}

fn installed_disk_state_triggers(
    conn: &Connection,
) -> Result<Vec<(String, String)>, rusqlite::Error> {
    let mut statement = conn.prepare(
        "SELECT name, sql FROM sqlite_master
         WHERE type = 'trigger' AND name LIKE 'disk\\_state\\_%' ESCAPE '\\'
         ORDER BY name",
    )?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

/// Remove every disk-state trigger. SQLite checks triggers when a table is
/// altered or rebuilt, so a migration that reshapes a carried table runs
/// without them; [`sync_disk_state_triggers`] puts them back afterwards.
pub(crate) fn drop_disk_state_triggers(conn: &Connection) -> Result<(), rusqlite::Error> {
    for (name, _) in installed_disk_state_triggers(conn)? {
        conn.execute_batch(&format!("DROP TRIGGER IF EXISTS \"{name}\""))?;
    }
    Ok(())
}

/// Install the triggers exactly as [`disk_state_triggers`] defines them,
/// in one transaction (the caller's, when there is one). Idempotent:
/// replaces whatever was installed before.
pub(super) fn install_disk_state_triggers(conn: &Connection) -> Result<(), rusqlite::Error> {
    let own_transaction = conn.is_autocommit();
    if own_transaction {
        conn.execute_batch("BEGIN IMMEDIATE")?;
    }
    let installed = (|| {
        drop_disk_state_triggers(conn)?;
        for (_, sql) in disk_state_triggers() {
            conn.execute_batch(&sql)?;
        }
        Ok(())
    })();
    if own_transaction {
        match &installed {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(_) => {
                let _ = conn.execute_batch("ROLLBACK");
            }
        }
    }
    installed
}

/// After migrations: the triggers are installed, and match this build's
/// definition, once migration `103_disk_state_records` has run. Writes
/// nothing when they already match.
pub(super) fn sync_disk_state_triggers(conn: &Connection) -> Result<(), rusqlite::Error> {
    let recorded: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE id = '103_disk_state_records')",
        [],
        |row| row.get(0),
    )?;
    if !recorded {
        return Ok(());
    }
    let mut desired = disk_state_triggers();
    desired.sort();
    if installed_disk_state_triggers(conn)? == desired {
        return Ok(());
    }
    install_disk_state_triggers(conn)
}

/// Migration `103_disk_state_records`: the bookkeeping tables and the
/// backfill (the triggers are installed after every migration has run, by
/// [`sync_disk_state_triggers`]). The backfill owes every open task a new
/// `task.json` (which now carries `state`) and every repository a
/// `repo.json`; the publisher writes them. Re-running it only owes them
/// again, so an interrupted backfill resumes by publishing what is owed.
pub(super) fn migrate_disk_state_records(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(SCHEMA)?;
    // The sequence high-water mark starts at every sequence allocated so
    // far (reservations included).
    conn.execute(
        "INSERT OR IGNORE INTO task_ledger_sequence (task_id, high_water)
         SELECT task_id, MAX(sequence) FROM task_ledger_entry WHERE true GROUP BY task_id",
        [],
    )?;
    // Every task gets a snapshot row the triggers can bump; a closed task's
    // is current as it stands.
    conn.execute(
        "INSERT OR IGNORE INTO task_ledger_snapshot (task_id, revision, published_revision)
         SELECT id, 0, 0 FROM pipeline_item WHERE true",
        [],
    )?;
    conn.execute(
        "UPDATE task_ledger_snapshot SET revision = revision + 1
         WHERE task_id IN (SELECT id FROM pipeline_item WHERE closed_at IS NULL)",
        [],
    )?;
    conn.execute(
        "INSERT INTO repo_disk_snapshot (repo_id) SELECT id FROM repo WHERE true
         ON CONFLICT(repo_id) DO UPDATE SET revision = revision + 1",
        [],
    )?;
    Ok(())
}

/// Columns of `repo.json`'s `registration`.
pub const REPO_COLUMNS: &[&str] = &[
    "id",
    "path",
    "name",
    "default_branch",
    "default_branch_source",
    "remote_url",
    "remote_url_hash",
    "sort_order",
    "created_at",
    "last_opened_at",
    "hidden",
];

/// Registration columns whose change alone does not rewrite `repo.json`.
const REPO_QUIET_COLUMNS: &[&str] = &["last_opened_at"];

/// An SQLite value as `task.json` writes it: integers and reals as numbers,
/// text as strings, NULL as null. No carried column holds a blob; one would
/// be written as `{"blob_hex": ...}` rather than dropped.
pub fn sql_to_json(value: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(text) => Value::String(String::from_utf8_lossy(text).to_string()),
        ValueRef::Blob(bytes) => json!({
            "blob_hex": bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
        }),
    }
}

/// The inverse of [`sql_to_json`].
pub fn json_to_sql(value: &Value) -> Result<rusqlite::types::Value, String> {
    use rusqlite::types::Value as Sql;
    Ok(match value {
        Value::Null => Sql::Null,
        Value::Bool(value) => Sql::Integer(i64::from(*value)),
        Value::Number(number) => match number.as_i64() {
            Some(value) => Sql::Integer(value),
            None => Sql::Real(
                number
                    .as_f64()
                    .ok_or_else(|| format!("unrepresentable number {number}"))?,
            ),
        },
        Value::String(text) => Sql::Text(text.clone()),
        Value::Object(object) => match object.get("blob_hex").and_then(Value::as_str) {
            Some(hex) if object.len() == 1 => Sql::Blob(
                (0..hex.len())
                    .step_by(2)
                    .map(|index| {
                        hex.get(index..index + 2)
                            .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                            .ok_or_else(|| "invalid blob_hex".to_string())
                    })
                    .collect::<Result<_, _>>()?,
            ),
            _ => return Err("an object is not a column value".into()),
        },
        Value::Array(_) => return Err("an array is not a column value".into()),
    })
}

impl Db {
    pub(super) fn carried_rows(
        &self,
        table: &CarriedTable,
        key: &str,
    ) -> Result<Vec<Value>, rusqlite::Error> {
        let columns = table
            .columns
            .iter()
            .map(|column| format!("\"{column}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let mut statement = self.conn.prepare(&format!(
            "SELECT rowid, {columns} FROM {} WHERE {} ORDER BY rowid",
            table.table, table.rows_of_task
        ))?;
        let rows = statement.query_map([key], |row| {
            let mut object = Map::new();
            object.insert("rowid".into(), json!(row.get::<_, i64>(0)?));
            for (index, column) in table.columns.iter().enumerate() {
                object.insert(column.to_string(), sql_to_json(row.get_ref(index + 1)?));
            }
            Ok(Value::Object(object))
        })?;
        rows.collect()
    }

    /// `task.json`'s `state`: the task's rows of every carried table, keyed
    /// by table (tables with no rows are omitted).
    pub(crate) fn task_state_record(&self, task_id: &str) -> Result<Value, rusqlite::Error> {
        let mut tables = Map::new();
        for table in CARRIED_TABLES {
            let rows = match self.carried_rows(table, task_id) {
                Ok(rows) => rows,
                // A schema-only test fixture may predate a table.
                Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                    if message.starts_with("no such table")
                        || message.starts_with("no such column") =>
                {
                    continue
                }
                Err(error) => return Err(error),
            };
            if !rows.is_empty() {
                tables.insert(table.table.to_string(), Value::Array(rows));
            }
        }
        // The ledger boundary these rows reflect, read in the same snapshot:
        // every committed entry (published or still pending) was written in
        // the transaction of the mutation it records, so its effects are in
        // the rows. A sequence only reserved (`kind` NULL) is not: the
        // operation that reserved it has not committed what it records.
        let (reflects_through, unreflected) = match self.ledger_state_boundary(task_id) {
            Ok(boundary) => boundary,
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                if message.starts_with("no such table") =>
            {
                (0, Vec::new())
            }
            Err(error) => return Err(error),
        };
        Ok(json!({
            "version": DISK_STATE_VERSION,
            "reflects_through": reflects_through,
            "unreflected_reservations": unreflected,
            "tables": tables,
        }))
    }

    /// The highest committed ledger sequence of the task, pending entries
    /// included, and the reserved-but-unfilled sequences below it.
    fn ledger_state_boundary(&self, task_id: &str) -> Result<(i64, Vec<i64>), rusqlite::Error> {
        let through: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(sequence), 0) FROM task_ledger_entry
             WHERE task_id = ? AND kind IS NOT NULL",
            [task_id],
            |row| row.get(0),
        )?;
        let mut statement = self.conn.prepare(
            "SELECT sequence FROM task_ledger_entry
             WHERE task_id = ? AND kind IS NULL AND sequence < ? ORDER BY sequence",
        )?;
        let reserved = statement
            .query_map(params![task_id, through], |row| row.get(0))?
            .collect::<Result<Vec<i64>, _>>()?;
        Ok((through, reserved))
    }

    /// `repo.json`: the registration row and the sidebar order of its
    /// remote. `None` when the repository is not registered.
    pub(crate) fn repo_disk_record(&self, repo_id: &str) -> Result<Option<Value>, rusqlite::Error> {
        self.with_read_transaction(|db| {
            let columns = REPO_COLUMNS
                .iter()
                .map(|column| format!("\"{column}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let registration = db
                .conn
                .query_row(
                    &format!("SELECT {columns} FROM repo WHERE id = ?"),
                    [repo_id],
                    |row| {
                        let mut object = Map::new();
                        for (index, column) in REPO_COLUMNS.iter().enumerate() {
                            object.insert(column.to_string(), sql_to_json(row.get_ref(index)?));
                        }
                        Ok(Value::Object(object))
                    },
                )
                .optional()?;
            let Some(registration) = registration else {
                return Ok(None);
            };
            let sidebar_order: Option<i64> =
                match registration.get("remote_url_hash").and_then(Value::as_str) {
                    Some(hash) => db
                        .conn
                        .query_row(
                            "SELECT sort_order FROM repo_sidebar_order WHERE remote_url_hash = ?",
                            [hash],
                            |row| row.get(0),
                        )
                        .optional()?,
                    None => None,
                };
            let (revision, _) = db.repo_disk_revisions(repo_id)?.unwrap_or((0, 0));
            Ok(Some(json!({
                "schema_version": crate::task_store::SCHEMA_VERSION,
                "repo_id": repo_id,
                "registration": registration,
                "sidebar_order": sidebar_order,
                "snapshot_revision": revision,
            })))
        })
    }

    pub(crate) fn repo_disk_revisions(
        &self,
        repo_id: &str,
    ) -> Result<Option<(i64, i64)>, rusqlite::Error> {
        self.conn
            .query_row(
                "SELECT revision, published_revision FROM repo_disk_snapshot WHERE repo_id = ?",
                [repo_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
    }

    /// Repositories whose `repo.json` is owed.
    pub(crate) fn repos_with_pending_disk_record(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut statement = self.conn.prepare(
            "SELECT repo_id FROM repo_disk_snapshot WHERE published_revision < revision
             ORDER BY repo_id",
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        match rows.collect() {
            Ok(rows) => Ok(rows),
            Err(error) if is_missing_disk_state_table(&error) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn acknowledge_repo_disk_record(
        &self,
        repo_id: &str,
        revision: i64,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE repo_disk_snapshot
             SET published_revision = MAX(published_revision, ?), publish_error = NULL
             WHERE repo_id = ?",
            params![revision, repo_id],
        )?;
        Ok(())
    }

    pub(crate) fn record_repo_disk_record_error(
        &self,
        repo_id: &str,
        error: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE repo_disk_snapshot SET publish_error = ? WHERE repo_id = ?",
            params![error, repo_id],
        )?;
        Ok(())
    }

    /// Removals whose tombstone is owed: `(kind, id, repo_id)`.
    pub(crate) fn pending_disk_removals(
        &self,
    ) -> Result<Vec<(String, String, String)>, rusqlite::Error> {
        let statement = self.conn.prepare(
            "SELECT kind, id, repo_id FROM disk_record_removal ORDER BY removed_at, kind, id",
        );
        let mut statement = match statement {
            Ok(statement) => statement,
            Err(error) if is_missing_disk_state_table(&error) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        rows.collect()
    }

    /// The tombstone for `(kind, id)` is on disk. A removal recorded again
    /// since (a newer `removed_at`) stays owed.
    pub(crate) fn acknowledge_disk_removal(
        &self,
        kind: &str,
        id: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM disk_record_removal WHERE kind = ? AND id = ?",
            params![kind, id],
        )?;
        Ok(())
    }

    pub(crate) fn record_disk_removal_error(
        &self,
        kind: &str,
        id: &str,
        error: &str,
    ) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "UPDATE disk_record_removal SET publish_error = ? WHERE kind = ? AND id = ?",
            params![error, kind, id],
        )?;
        Ok(())
    }
}

fn is_missing_disk_state_table(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::SqliteFailure(_, Some(message))
        if message.contains("no such table: repo_disk_snapshot")
            || message.contains("no such table: disk_record_removal"))
}

#[cfg(test)]
#[path = "task_state_tests.rs"]
mod tests;
