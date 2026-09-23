//! Rebuild of SQL projections from task directories (spec §11, §16.11 —
//! component T13, first to third increments).
//!
//! Reads `<root>/repos/<repo-id>/repo.json` and
//! `<root>/repos/<repo-id>/tasks/<task-id>/` (`task.json` and the published
//! `ledger/`) with a versioned reader and projects them deterministically.
//! A rebuild writes the projection into a **new** database created with the
//! current schema, never into an existing one. Under disk authority (T13c,
//! [`super::authority`]) a missing database is rebuilt this way, scoped to
//! the installation's own repositories, and a live database is reconciled
//! from the projection of the tasks the disk is ahead for.
//! `docs/2026-09-23-disk-authority-inventory.md` says which disk record owns
//! each table; [`NOT_REBUILT`] lists what a rebuilt database lacks by design.
//!
//! What is projected, and from where:
//!
//! - **repository registration** (`repo`, `repo_sidebar_order`): `repo.json`.
//!   A repository with task directories but no `repo.json` gets a
//!   placeholder row (empty path, name = id) so its tasks satisfy the schema.
//! - **every carried table** ([`crate::db::task_state::CARRIED_TABLES`]: the
//!   task row, runs and their sessions and prompts, workspaces, branch
//!   counter, budgets, commit steps, edges and waits, joins, owed work,
//!   review and transfer records): `task.json`'s `state`, row for row,
//!   keeping each row's `rowid`. The branch counter is never below a
//!   `task-<id>-<n>` suffix any record names.
//! - **dependency blockers** (`task_blocker`): `task.json` `links.dependencies`.
//! - **inputs** (`task_input`): one row per `input` entry, keeping its row id.
//! - **the ledger** (`task_ledger_entry`): the files' exact bytes as published
//!   rows, so a server started on the rebuilt database continues each
//!   sequence.
//!
//! A `task.json` written before `state` existed (or by a peer that does not
//! write it) is projected as the first increment did: one stage run per run
//! that recorded a result (the newest wins; an engine-observed ending only
//! when the run recorded nothing else), T2's session identity from
//! `session_ref`, and budgets replayed from routed results and reset by a
//! person's send-back.
//!
//! **The ledger is newer than a stale `task.json`.** A crash between
//! publishing an entry and rewriting `task.json` leaves `state` behind the
//! ledger; the entries after `state.reflects_through` (the highest ledger
//! sequence the rows already reflect, read with them; a reservation below
//! it that was still unfilled counts as not reflected) are applied on top
//! of its rows (a verdict or engine-observed ending closes its run, a routed
//! result spends its budget, a send-back resets it).
//!
//! **Owed work is never re-done.** Rows are restored; nothing is executed.
//! An owed transition (`task_ledger_continuation`) from a stale `task.json`
//! is dropped only when a newer entry paid or replaced it — a transition, or
//! a verdict under another operation — since restoring it would run that
//! transition twice; after unrelated newer entries it stays owed.
//!
//! Unknown facts stay unknown: a column the disk does not carry is left NULL
//! or at its schema default, never guessed.

use super::{parse_ledger_file, LedgerFile};
use crate::db::task_state::{
    json_to_sql, CarriedTable, CARRIED_TABLES, DISK_STATE_VERSION, REMOVED_KEY, REPO_COLUMNS,
};
use crate::db::task_store::LedgerEntryKind;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The `schema_version`s of `task.json` and ledger envelopes this reader
/// understands. Anything else is refused, never guessed at.
pub const SUPPORTED_SCHEMA_VERSIONS: &[u64] = &[1];

/// Why a rebuilt database lacks a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    /// Statistics: a rebuilt database starts them empty.
    Statistics,
    /// Live-session or in-flight state, re-derived at runtime.
    Transient,
    /// Durable, but not rebuildable; the reason says why.
    Unrecoverable,
}

/// One class of fact a rebuild from task directories does not restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotRebuilt {
    pub fact: &'static str,
    pub gap: Gap,
    pub reason: &'static str,
}

/// What a rebuilt database lacks, by design. Everything else a task's
/// database rows hold is rebuilt exactly; the fixture round trip compares
/// it all and asserts no difference.
pub const NOT_REBUILT: &[NotRebuilt] = &[
    NotRebuilt {
        fact: "statistics",
        gap: Gap::Statistics,
        reason: "activity_log, task_activity_interval, operator_event, provider_token_usage, provider_usage_scan, provider_usage_discovery, task_revision, task_pull_request",
    },
    NotRebuilt {
        fact: "task.live_state",
        gap: Gap::Transient,
        reason: "the task row's live columns (activity, runtime status, read state, output preview, unsent drafts, port lease, live session mirror, event debounce baselines, teardown-in-progress marker): re-derived from the sessions",
    },
    NotRebuilt {
        fact: "session.transport",
        gap: Gap::Transient,
        reason: "terminal_session, task_port, copilot_wake_*, claude_channel_*: a live session's bindings",
    },
    NotRebuilt {
        fact: "transfer.work_queue",
        gap: Gap::Transient,
        reason: "transfer_work, transfer_work_phase: the transfer engine's in-flight queue, re-derived from task_transfer by restart recovery",
    },
    NotRebuilt {
        fact: "events.feed",
        gap: Gap::Transient,
        reason: "task_event and task_event_cursor_handle: an announcement feed pruned after 14 days; the ledger is the record",
    },
    NotRebuilt {
        fact: "subscription.position",
        gap: Gap::Unrecoverable,
        reason: "event_subscription and task_serviced_watermark hold task_event sequence numbers, and a rebuilt database restarts that feed empty: a restored position would skip or replay events, so subscribers subscribe again",
    },
    NotRebuilt {
        fact: "run.terminal_capture",
        gap: Gap::Unrecoverable,
        reason: "agent_terminal_attempt holds a run's final terminal frame, a capture of session output rather than task state; the run keeps its transcript reference",
    },
    NotRebuilt {
        fact: "transfer.claim_token",
        gap: Gap::Unrecoverable,
        reason: "an incoming transfer's claim token is a capability and never leaves the database, and its 30-second lease expiry means nothing after a restart; restart recovery re-claims a claimed transfer under a new token",
    },
    NotRebuilt {
        fact: "machine.pairing_and_preferences",
        gap: Gap::Unrecoverable,
        reason: "trusted_peer and settings belong to the machine (its pairing store and local config), not to any task directory",
    },
    NotRebuilt {
        fact: "publication_window",
        gap: Gap::Unrecoverable,
        reason: "a change committed in SQL but not yet written to disk when the database is lost (the publisher writes within seconds, and at every startup)",
    },
    NotRebuilt {
        fact: "history.never_captured",
        gap: Gap::Unrecoverable,
        reason: "facts no record ever held stay unknown: backfilled history's branch, commit, triggering result and channel; which numbers a branch-counter reservation spent without leaving a branch, directory or record",
    },
];

/// `task.json`, schema version 1, as the reader understands it.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskSnapshot {
    pub schema_version: u64,
    pub task_id: String,
    pub repo_id: String,
    pub title: Option<String>,
    pub origin_prompt: Option<String>,
    pub workflow_name: Option<String>,
    pub workflow_definition: Option<Value>,
    pub parent: Option<String>,
    pub dependencies: Vec<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub stage: Option<String>,
    pub branch: Option<String>,
    pub base_ref: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub closed_at: Option<String>,
    pub snapshot_revision: i64,
    pub published_through: i64,
    /// `state.tables` (T13): the task's rows of each carried table. `None`
    /// for a `task.json` written before `state` existed.
    pub state: Option<Map<String, Value>>,
    /// `state.reflects_through` (T13): the highest ledger sequence whose
    /// effects the `state` rows already hold, and the reserved sequences
    /// below it they do not. `None` for a `state` written before the field
    /// existed; the rebuild then falls back to `published_through`.
    pub state_reflects_through: Option<i64>,
    pub state_unreflected: Vec<i64>,
}

/// One published ledger file and its exact bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadEntry {
    pub file: LedgerFile,
    pub bytes: Vec<u8>,
}

/// One task directory, read and validated.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskDirectory {
    pub path: PathBuf,
    pub snapshot: TaskSnapshot,
    /// In sequence order.
    pub entries: Vec<ReadEntry>,
}

fn version_of(value: &Value, what: &str) -> Result<u64, String> {
    let version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{what} has no schema_version"))?;
    if SUPPORTED_SCHEMA_VERSIONS.contains(&version) {
        Ok(version)
    } else {
        Err(format!(
            "{what} has schema_version {version}; this reader understands {SUPPORTED_SCHEMA_VERSIONS:?}"
        ))
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Parse `task.json`. Unknown additional fields are allowed (later components
/// extend it additively); a missing identity or an unknown version is not.
pub fn parse_task_snapshot(bytes: &[u8]) -> Result<TaskSnapshot, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("task.json: {error}"))?;
    let schema_version = version_of(&value, "task.json")?;
    let required = |key: &str| text(&value, key).ok_or_else(|| format!("task.json has no {key}"));
    let workflow = value.get("workflow").cloned().unwrap_or(Value::Null);
    let links = value.get("links").cloned().unwrap_or(Value::Null);
    let pr = links.get("pr").cloned().unwrap_or(Value::Null);
    let dependencies = match links.get("dependencies") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "task.json links.dependencies holds a non-string".to_string())
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("task.json links.dependencies is not a list".into()),
    };
    let state = match value.get("state") {
        None | Some(Value::Null) => None,
        Some(state) => Some(parse_state(state)?),
    };
    let state_value = value.get("state").cloned().unwrap_or(Value::Null);
    let state_reflects_through = state_value.get("reflects_through").and_then(Value::as_i64);
    let state_unreflected = state_value
        .get("unreflected_reservations")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_i64).collect())
        .unwrap_or_default();
    Ok(TaskSnapshot {
        state,
        state_reflects_through,
        state_unreflected,
        schema_version,
        task_id: required("task_id")?,
        repo_id: required("repo_id")?,
        title: text(&value, "title"),
        origin_prompt: text(&value, "origin_prompt"),
        workflow_name: text(&workflow, "name"),
        workflow_definition: workflow.get("definition").filter(|d| !d.is_null()).cloned(),
        parent: text(&links, "parent"),
        dependencies,
        pr_url: text(&pr, "url"),
        pr_number: pr.get("number").and_then(Value::as_i64),
        stage: text(&value, "stage"),
        branch: text(&value, "branch"),
        base_ref: text(&value, "base_ref"),
        created_at: text(&value, "created_at"),
        updated_at: text(&value, "updated_at"),
        closed_at: text(&value, "closed_at"),
        snapshot_revision: value
            .get("snapshot_revision")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        published_through: value
            .get("ledger")
            .and_then(|ledger| ledger.get("published_through"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
    })
}

/// `task.json`'s `state`: a known version, and each table a list of rows
/// (objects). Tables this reader does not carry are refused rather than
/// dropped: a newer writer's state is not half-read.
fn parse_state(state: &Value) -> Result<Map<String, Value>, String> {
    let version = state.get("version").and_then(Value::as_u64);
    if version != Some(DISK_STATE_VERSION) {
        return Err(format!(
            "task.json state has version {version:?}; this reader understands {DISK_STATE_VERSION}"
        ));
    }
    let tables = match state.get("tables") {
        Some(Value::Object(tables)) => tables.clone(),
        _ => return Err("task.json state has no tables".into()),
    };
    for (table, rows) in &tables {
        let Some(carried) = carried_table(table) else {
            return Err(format!("task.json state carries unknown table {table}"));
        };
        let Some(rows) = rows.as_array() else {
            return Err(format!("task.json state.{table} is not a list"));
        };
        for row in rows {
            let Some(row) = row.as_object() else {
                return Err(format!("task.json state.{table} holds a non-object"));
            };
            if !row.get("rowid").is_some_and(Value::is_i64) {
                return Err(format!("task.json state.{table} row has no rowid"));
            }
            if let Some(column) = row
                .keys()
                .find(|column| *column != "rowid" && !carried.columns.contains(&column.as_str()))
            {
                return Err(format!(
                    "task.json state.{table} has unknown column {column}"
                ));
            }
            for (column, value) in row {
                json_to_sql(value)
                    .map_err(|error| format!("task.json state.{table}.{column}: {error}"))?;
            }
        }
    }
    Ok(tables)
}

fn carried_table(name: &str) -> Option<&'static CarriedTable> {
    CARRIED_TABLES.iter().find(|table| table.table == name)
}

/// Is this `task.json`/`repo.json` a tombstone for a removed record?
pub(crate) fn is_tombstone(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|value| value.get(REMOVED_KEY).and_then(Value::as_bool))
        == Some(true)
}

/// Read one task directory. Every entry's envelope must agree with its file
/// name and with the directory it is in; temporary files (`.`-prefixed) are
/// ignored, as the publisher leaves them only when it died mid-write.
pub fn read_task_directory(dir: &Path) -> Result<TaskDirectory, String> {
    let snapshot_path = dir.join("task.json");
    let bytes = std::fs::read(&snapshot_path)
        .map_err(|error| format!("read {}: {error}", snapshot_path.display()))?;
    let snapshot = parse_task_snapshot(&bytes)?;
    let named = |component: Option<&std::ffi::OsStr>| {
        component.map(|name| name.to_string_lossy().to_string())
    };
    if named(dir.file_name()).as_deref() != Some(snapshot.task_id.as_str()) {
        return Err(format!(
            "{} holds task.json for task {}",
            dir.display(),
            snapshot.task_id
        ));
    }
    let repo_dir = dir.parent().and_then(Path::parent);
    if named(repo_dir.and_then(Path::file_name)).as_deref() != Some(snapshot.repo_id.as_str()) {
        return Err(format!(
            "{} is not under repo {} named by its task.json",
            dir.display(),
            snapshot.repo_id
        ));
    }
    let ledger = dir.join("ledger");
    let mut entries = Vec::new();
    match std::fs::read_dir(&ledger) {
        Ok(listing) => {
            for item in listing {
                let item = item.map_err(|error| format!("read {}: {error}", ledger.display()))?;
                let name = item.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let bytes = std::fs::read(item.path())
                    .map_err(|error| format!("read {}: {error}", item.path().display()))?;
                let file = parse_ledger_file(&name, &bytes)?;
                validate_entry(&file, &snapshot.task_id)?;
                entries.push(ReadEntry { file, bytes });
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("read {}: {error}", ledger.display())),
    }
    entries.sort_by_key(|entry| entry.file.sequence);
    if let Some(pair) = entries
        .windows(2)
        .find(|pair| pair[0].file.sequence == pair[1].file.sequence)
    {
        return Err(format!(
            "ledger sequence {} appears twice ({} and {})",
            pair[0].file.sequence, pair[0].file.file_name, pair[1].file.file_name
        ));
    }
    Ok(TaskDirectory {
        path: dir.to_path_buf(),
        snapshot,
        entries,
    })
}

fn validate_entry(file: &LedgerFile, task_id: &str) -> Result<(), String> {
    let name = &file.file_name;
    version_of(&file.envelope, name)?;
    let envelope = &file.envelope;
    if envelope.get("sequence").and_then(Value::as_i64) != Some(file.sequence) {
        return Err(format!(
            "{name}: envelope sequence does not match the file name"
        ));
    }
    if envelope.get("kind").and_then(Value::as_str) != Some(file.kind.as_str()) {
        return Err(format!(
            "{name}: envelope kind does not match the file name"
        ));
    }
    if envelope.get("task_id").and_then(Value::as_str) != Some(task_id) {
        return Err(format!("{name}: envelope belongs to another task"));
    }
    let expected_id = crate::db::task_store::ledger_entry_id(task_id, file.sequence);
    if file.entry_id() != Some(expected_id.as_str()) {
        return Err(format!("{name}: entry_id is not {expected_id}"));
    }
    Ok(())
}

/// A task directory that could not be read, and why.
pub type Unreadable = (PathBuf, String);

/// `repos/<repo-id>/repo.json`, as the reader understands it.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoRecord {
    pub repo_id: String,
    /// The registration row, column by column ([`REPO_COLUMNS`]).
    pub registration: Map<String, Value>,
    pub sidebar_order: Option<i64>,
    pub snapshot_revision: i64,
    /// The installation that wrote it (T13c): the database whose repository
    /// this is. Absent in a `repo.json` written before the field existed.
    pub installation: Option<String>,
}

pub fn parse_repo_record(bytes: &[u8]) -> Result<RepoRecord, String> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| format!("repo.json: {error}"))?;
    version_of(&value, "repo.json")?;
    let repo_id = text(&value, "repo_id").ok_or("repo.json has no repo_id")?;
    let registration = value
        .get("registration")
        .and_then(Value::as_object)
        .cloned()
        .ok_or("repo.json has no registration")?;
    if registration.get("id").and_then(Value::as_str) != Some(repo_id.as_str()) {
        return Err("repo.json registration names another repository".into());
    }
    if let Some(column) = registration
        .keys()
        .find(|column| !REPO_COLUMNS.contains(&column.as_str()))
    {
        return Err(format!(
            "repo.json registration has unknown column {column}"
        ));
    }
    for (column, value) in &registration {
        json_to_sql(value).map_err(|error| format!("repo.json {column}: {error}"))?;
    }
    Ok(RepoRecord {
        repo_id,
        registration,
        sidebar_order: value.get("sidebar_order").and_then(Value::as_i64),
        snapshot_revision: value
            .get("snapshot_revision")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        installation: text(&value, "installation"),
    })
}

/// Everything a store root holds, read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoreScan {
    pub tasks: Vec<TaskDirectory>,
    pub repos: Vec<RepoRecord>,
    /// Directories or `repo.json` files that could not be read, and why.
    pub unreadable: Vec<Unreadable>,
    /// Tombstoned tasks and repositories: removed from their database,
    /// never rebuilt.
    pub removed: Vec<PathBuf>,
}

impl StoreScan {
    /// Only what `installation` wrote (T13c): the repositories whose
    /// `repo.json` names it, and the task directories under them. Several
    /// installations can share a store root (production and staging both
    /// use `~/.kanna`), and a database rebuilt or reconciled from disk must
    /// never take in another's tasks. Returns the other repositories' ids,
    /// unstamped ones included; their tasks, tombstones and unreadable
    /// entries are left out.
    pub fn for_installation(self, installation: &str) -> (StoreScan, Vec<String>) {
        let ours: BTreeSet<String> = self
            .repos
            .iter()
            .filter(|record| record.installation.as_deref() == Some(installation))
            .map(|record| record.repo_id.clone())
            .collect();
        let repo_of = |path: &Path| -> Option<String> {
            // `<root>/repos/<repo-id>/repo.json` or `.../<repo-id>/tasks/<task-id>`.
            let mut components = path
                .components()
                .rev()
                .map(|component| component.as_os_str().to_string_lossy().to_string());
            let last = components.next()?;
            if last == "repo.json" {
                components.next()
            } else {
                components.nth(1)
            }
        };
        let mut foreign: BTreeSet<String> = self
            .repos
            .iter()
            .filter(|record| !ours.contains(&record.repo_id))
            .map(|record| record.repo_id.clone())
            .collect();
        foreign.extend(
            self.tasks
                .iter()
                .map(|task| task.snapshot.repo_id.clone())
                .filter(|repo| !ours.contains(repo)),
        );
        let scoped = StoreScan {
            tasks: self
                .tasks
                .into_iter()
                .filter(|task| ours.contains(&task.snapshot.repo_id))
                .collect(),
            repos: self
                .repos
                .into_iter()
                .filter(|record| ours.contains(&record.repo_id))
                .collect(),
            unreadable: self
                .unreadable
                .into_iter()
                .filter(|(path, _)| repo_of(path).is_some_and(|repo| ours.contains(&repo)))
                .collect(),
            removed: self
                .removed
                .into_iter()
                .filter(|path| repo_of(path).is_some_and(|repo| ours.contains(&repo)))
                .collect(),
        };
        (scoped, foreign.into_iter().collect())
    }
}

/// Every task directory under a store root, ordered by repo then task id.
/// Directories that cannot be read are returned separately rather than
/// dropped silently.
pub fn scan_store(root: &Path) -> Result<(Vec<TaskDirectory>, Vec<Unreadable>), String> {
    let scan = scan_store_records(root)?;
    Ok((scan.tasks, scan.unreadable))
}

/// [`scan_store`], with repository records and tombstones.
pub fn scan_store_records(root: &Path) -> Result<StoreScan, String> {
    let sorted_dirs = |dir: &Path| -> Result<Vec<PathBuf>, String> {
        let mut dirs = match std::fs::read_dir(dir) {
            Ok(listing) => listing
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .filter(|path| {
                    !path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                })
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(format!("read {}: {error}", dir.display())),
        };
        dirs.sort();
        Ok(dirs)
    };
    let mut scan = StoreScan::default();
    for repo in sorted_dirs(&root.join("repos"))? {
        let record_path = repo.join("repo.json");
        match std::fs::read(&record_path) {
            Ok(bytes) if is_tombstone(&bytes) => {
                scan.removed.push(record_path);
                continue;
            }
            Ok(bytes) => match parse_repo_record(&bytes) {
                Ok(record)
                    if repo
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        == Some(record.repo_id.clone()) =>
                {
                    scan.repos.push(record)
                }
                Ok(record) => scan.unreadable.push((
                    record_path,
                    format!("repo.json names repository {}", record.repo_id),
                )),
                Err(error) => scan.unreadable.push((record_path, error)),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => scan.unreadable.push((record_path, error.to_string())),
        }
        for task in sorted_dirs(&repo.join("tasks"))? {
            if std::fs::read(task.join("task.json")).is_ok_and(|bytes| is_tombstone(&bytes)) {
                scan.removed.push(task);
                continue;
            }
            match read_task_directory(&task) {
                Ok(directory) => scan.tasks.push(directory),
                Err(error) => scan.unreadable.push((task, error)),
            }
        }
    }
    Ok(scan)
}

// ---------------------------------------------------------------------------
// Projection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRow {
    pub id: String,
    pub repo_id: String,
    pub prompt: Option<String>,
    pub display_name: Option<String>,
    pub workflow_name: Option<String>,
    pub workflow_definition: Option<String>,
    pub stage: Option<String>,
    pub branch: Option<String>,
    pub base_ref: Option<String>,
    pub parent_task_id: Option<String>,
    pub pr_url: Option<String>,
    pub pr_number: Option<i64>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub closed_at: Option<String>,
    /// The task's row comes from `state` ([`Projection::carried`]); this
    /// one only names it.
    pub from_state: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRunRow {
    pub id: String,
    pub task_id: String,
    pub stage: String,
    pub kind: String,
    pub status: String,
    /// `None` for a run known only by an engine-observed ending.
    pub result: Option<String>,
    pub feedback: Option<String>,
    pub no_work_termination: Option<String>,
    pub started_at: String,
    pub finished_at: String,
    pub result_declared_role: Option<String>,
    pub result_channel_identity: Option<String>,
    /// T2's session identity, from the envelope's `session_ref`.
    pub workspace_id: Option<String>,
    pub session_branch: Option<String>,
    pub session_name: Option<String>,
    pub transcript_ref: Option<String>,
}

/// T2's session identity as `session_ref` carries it beside T0's `{kind,
/// id}`; `None` for a reference with none of it (T0's shape).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SessionIdentity {
    workspace_id: Option<String>,
    branch: Option<String>,
    name: Option<String>,
    transcript_ref: Option<String>,
}

fn session_identity(envelope: &Value) -> Option<SessionIdentity> {
    let reference = envelope.get("session_ref").filter(|r| r.is_object())?;
    let transcript = reference.get("transcript").and_then(|transcript| {
        Some(crate::db::TranscriptRef {
            provider: text(transcript, "provider")?,
            session_id: text(transcript, "session_id")?,
            path: text(transcript, "path"),
        })
    });
    let identity = SessionIdentity {
        workspace_id: text(reference, "workspace_id"),
        branch: text(reference, "branch"),
        name: text(reference, "name"),
        // The column's own encoding, as T2's writer stores it.
        transcript_ref: transcript.and_then(|transcript| serde_json::to_string(&transcript).ok()),
    };
    (identity != SessionIdentity::default()).then_some(identity)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputRow {
    pub id: i64,
    pub task_id: String,
    pub run_id: Option<String>,
    pub stage: Option<String>,
    pub source: String,
    pub message: String,
    pub delivered_at: String,
    pub origin_peer_id: Option<String>,
    pub origin_task_id: Option<String>,
    pub origin_input_id: Option<i64>,
    pub origin_run_id: Option<String>,
    pub channel_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetRow {
    pub task_id: String,
    pub stage: String,
    pub spent: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerRow {
    pub task_id: String,
    pub sequence: i64,
    pub entry_id: String,
    pub kind: String,
    pub operation_id: Option<String>,
    pub source_kind: Option<String>,
    pub source_id: Option<String>,
    pub file_name: String,
    pub payload: Vec<u8>,
    pub recorded_at: String,
}

/// Ledger bookkeeping for one task: `task.json` is current and its history
/// is on disk, so nothing is owed to publish or backfill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLedgerMarker {
    pub task_id: String,
    pub snapshot_revision: i64,
    pub historical_entries: i64,
    pub marked_at: String,
}

/// One row of a carried table, from a task's `state`.
#[derive(Debug, Clone, PartialEq)]
pub struct CarriedRow {
    pub table: &'static str,
    pub task_id: String,
    /// Column → value, `rowid` included (absent only for a row the
    /// projection itself added).
    pub row: Map<String, Value>,
}

/// Everything one rebuild writes, in a stable order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Projection {
    /// Repository ids the tasks belong to.
    pub repos: Vec<String>,
    /// Registrations from `repo.json`, by repo id. A repository in `repos`
    /// with none gets a placeholder row.
    pub repo_records: Vec<RepoRecord>,
    pub tasks: Vec<TaskRow>,
    /// Rows of the carried tables, in [`CARRIED_TABLES`] order, then task,
    /// then rowid.
    pub carried: Vec<CarriedRow>,
    pub blockers: Vec<(String, String)>,
    pub stage_runs: Vec<StageRunRow>,
    pub inputs: Vec<InputRow>,
    pub budgets: Vec<BudgetRow>,
    pub ledger: Vec<LedgerRow>,
    pub markers: Vec<TaskLedgerMarker>,
    /// Facts noticed while projecting that a reader should know about (a
    /// stale `task.json`, a run known only by reference). Never errors.
    pub diagnostics: Vec<String>,
}

/// The ledger's ISO-8601 UTC timestamp in SQLite's `datetime('now')` shape.
/// Sub-second precision is dropped, as SQLite's own columns have none. Any
/// other shape is kept verbatim.
pub fn iso_to_sqlite_time(value: &str) -> String {
    let bytes = value.as_bytes();
    let iso = value.len() >= 20
        && bytes.get(10) == Some(&b'T')
        && value.ends_with('Z')
        && bytes.get(19).is_some_and(|b| *b == b'Z' || *b == b'.');
    if iso {
        format!("{} {}", &value[..10], &value[11..19])
    } else {
        value.to_string()
    }
}

fn recorded_at(file: &LedgerFile) -> String {
    file.envelope
        .get("recorded_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A stored JSON column, written verbatim from the envelope: a channel
/// identity from a newer peer keeps its evidence rather than collapsing to
/// whatever this build decodes it as.
fn json_column(value: Option<&Value>) -> Option<String> {
    value.filter(|value| !value.is_null()).map(Value::to_string)
}

/// The `stage_run.result` a result entry stands for: the `{status, summary,
/// metadata}` record the result call writes, with the artifact references and
/// the exit the session named; a legacy free-form result stays free-form.
fn stage_result_column(file: &LedgerFile, message: &str) -> String {
    let body = file.body();
    if body.get("legacy_format").and_then(Value::as_bool) == Some(true) {
        return message.to_string();
    }
    let mut result = json!({
        "status": body.get("status").cloned().unwrap_or(Value::Null),
        "summary": message,
        "metadata": body.get("metadata").cloned().unwrap_or(Value::Null),
    });
    if let Some(artifacts) = file
        .envelope
        .get("artifacts")
        .filter(|artifacts| artifacts.as_object().is_some_and(|map| !map.is_empty()))
    {
        result["artifacts"] = artifacts.clone();
    }
    if body.get("exit_source").and_then(Value::as_str) == Some("explicit") {
        if let Some(exit) = body.get("exit").filter(|exit| !exit.is_null()) {
            result["exit"] = exit.clone();
        }
    }
    result.to_string()
}

/// The stage run a verdict result entry stands for.
fn verdict_run_row(task_id: &str, run_id: &str, file: &LedgerFile) -> StageRunRow {
    let envelope = &file.envelope;
    let body = file.body();
    let message = file.message.as_deref().unwrap_or("");
    let status = text(body, "status").unwrap_or_else(|| "unknown".into());
    // A revision request records its summary and findings apart (T13);
    // older entries only joined them in the message.
    let request = body.get("request").cloned().unwrap_or(Value::Null);
    let revision = (text(&request, "kind").as_deref() == Some("revision_request"))
        .then(|| (text(&request, "summary"), text(&request, "findings")));
    let (summary, feedback) = match revision {
        Some((Some(summary), Some(findings))) => (summary, findings),
        _ => (message.to_string(), message.to_string()),
    };
    StageRunRow {
        id: run_id.to_string(),
        task_id: task_id.to_string(),
        stage: text(body, "stage").unwrap_or_default(),
        kind: text(body, "run_kind").unwrap_or_else(|| "main".into()),
        status: if status == "success" {
            "succeeded".into()
        } else {
            "failed".into()
        },
        result: Some(stage_result_column(file, &summary)),
        feedback: Some(feedback),
        no_work_termination: None,
        started_at: String::new(),
        finished_at: iso_to_sqlite_time(&recorded_at(file)),
        result_declared_role: text(envelope, "declared_role"),
        result_channel_identity: json_column(envelope.get("channel_identity")),
        workspace_id: None,
        session_branch: None,
        session_name: None,
        transcript_ref: None,
    }
}

/// The stage run an engine-observed ending stands for, when nothing else
/// records it.
fn ending_run_row(task_id: &str, run_id: &str, file: &LedgerFile) -> StageRunRow {
    let body = file.body();
    let ending = body.get("ending").cloned().unwrap_or(Value::Null);
    StageRunRow {
        id: run_id.to_string(),
        task_id: task_id.to_string(),
        stage: text(body, "stage").unwrap_or_default(),
        kind: text(body, "run_kind").unwrap_or_else(|| "main".into()),
        status: text(&ending, "run_status").unwrap_or_else(|| "failed".into()),
        result: None,
        feedback: None,
        no_work_termination: text(&ending, "no_work_termination"),
        started_at: String::new(),
        finished_at: iso_to_sqlite_time(&recorded_at(file)),
        result_declared_role: None,
        result_channel_identity: None,
        workspace_id: None,
        session_branch: None,
        session_name: None,
        transcript_ref: None,
    }
}

/// The destination budget a routed result spent (T1), and whether the claim
/// was exhausted.
fn budget_claim(task_id: &str, file: &LedgerFile) -> Option<(BudgetRow, bool)> {
    let budget = file
        .body()
        .get("budget")
        .filter(|budget| budget.is_object())?;
    Some((
        BudgetRow {
            task_id: task_id.to_string(),
            stage: text(budget, "stage")?,
            spent: budget.get("spent").and_then(Value::as_i64)?,
            updated_at: iso_to_sqlite_time(&recorded_at(file)),
        },
        budget.get("exhausted").and_then(Value::as_bool) == Some(true),
    ))
}

/// The stage a person's send-back gives a fresh budget: an operator exit
/// other than `advance` (operating a gate does not reset it).
fn send_back_stage(file: &LedgerFile) -> Option<String> {
    let body = file.body();
    (file.kind == LedgerEntryKind::Transition
        && text(body, "exit_source").as_deref() == Some("operator")
        && text(body, "exit").as_deref() != Some("advance"))
    .then(|| text(body, "to_stage"))
    .flatten()
}

fn project_task(directory: &TaskDirectory, projection: &mut Projection) {
    let snapshot = &directory.snapshot;
    let task_id = snapshot.task_id.as_str();
    projection.tasks.push(TaskRow {
        id: snapshot.task_id.clone(),
        repo_id: snapshot.repo_id.clone(),
        prompt: snapshot.origin_prompt.clone(),
        display_name: snapshot.title.clone(),
        workflow_name: snapshot.workflow_name.clone(),
        workflow_definition: snapshot.workflow_definition.as_ref().map(Value::to_string),
        stage: snapshot.stage.clone(),
        branch: snapshot.branch.clone(),
        base_ref: snapshot.base_ref.clone(),
        parent_task_id: snapshot.parent.clone(),
        pr_url: snapshot.pr_url.clone(),
        pr_number: snapshot.pr_number,
        created_at: snapshot.created_at.clone(),
        updated_at: snapshot.updated_at.clone(),
        closed_at: snapshot.closed_at.clone(),
        from_state: false,
    });
    let mut dependencies = snapshot.dependencies.clone();
    dependencies.sort();
    dependencies.dedup();
    projection.blockers.extend(
        dependencies
            .into_iter()
            .map(|blocker| (snapshot.task_id.clone(), blocker)),
    );

    let mut runs: BTreeMap<String, StageRunRow> = BTreeMap::new();
    let mut first_mention: BTreeMap<String, String> = BTreeMap::new();
    let mut sessions: BTreeMap<String, SessionIdentity> = BTreeMap::new();
    let mut budgets: BTreeMap<String, BudgetRow> = BTreeMap::new();
    let mut referenced_runs: Vec<(String, String)> = Vec::new();
    let mut historical = 0i64;
    let mut last_transition_to: Option<Option<String>> = None;
    let mut last_plan_after: Option<Value> = None;
    for ReadEntry { file, bytes } in &directory.entries {
        let envelope = &file.envelope;
        let at = recorded_at(file);
        let run_id = text(envelope, "run_id");
        if let Some(run_id) = &run_id {
            first_mention
                .entry(run_id.clone())
                .or_insert_with(|| iso_to_sqlite_time(&at));
            // Written once when the session starts, so every entry of the run
            // carries the same identity; the first one that has it is kept.
            if let Some(identity) = session_identity(envelope) {
                sessions.entry(run_id.clone()).or_insert(identity);
            }
        }
        if envelope.get("historical").and_then(Value::as_bool) == Some(true) {
            historical += 1;
        }
        // Every entry is mirrored into the outbox, whatever else it projects.
        let source = envelope.get("source").cloned().unwrap_or(Value::Null);
        projection.ledger.push(LedgerRow {
            task_id: task_id.to_string(),
            sequence: file.sequence,
            entry_id: file.entry_id().unwrap_or_default().to_string(),
            kind: file.kind.as_str().to_string(),
            operation_id: text(envelope, "operation_id"),
            source_kind: text(&source, "kind"),
            source_id: text(&source, "id"),
            file_name: file.file_name.clone(),
            payload: bytes.clone(),
            recorded_at: at.clone(),
        });
        let body = file.body();
        let message = file.message.as_deref().unwrap_or("");
        match file.kind {
            LedgerEntryKind::Result => {
                // A result a transfer carried (T9) ran on another machine and
                // records no local run; it projects under the carried key its
                // origin run is held by here, the same key its commit step uses.
                let run_id = run_id.or_else(|| carried_result_run_id(task_id, envelope));
                if let Some(run_id) = &run_id {
                    first_mention
                        .entry(run_id.clone())
                        .or_insert_with(|| iso_to_sqlite_time(&at));
                }
                let Some(run_id) = run_id else {
                    projection.diagnostics.push(format!(
                        "{task_id}: result {} names no run; no stage run projected",
                        file.file_name
                    ));
                    continue;
                };
                // An ending the engine observed (T13) is not a verdict: it
                // stands for the run only when nothing else does.
                if crate::db::task_store::is_engine_observed_result(body) {
                    if !runs.contains_key(&run_id) {
                        runs.insert(run_id.clone(), ending_run_row(task_id, &run_id, file));
                    }
                    continue;
                }
                runs.insert(run_id.clone(), verdict_run_row(task_id, &run_id, file));
                if let Some(budget) = budget_claim(task_id, file) {
                    // An exhausted claim changes nothing in SQL; it only
                    // proves the destination's spend if nothing else did.
                    if !budget.1 || !budgets.contains_key(&budget.0.stage) {
                        budgets.insert(budget.0.stage.clone(), budget.0);
                    }
                }
            }
            LedgerEntryKind::Input => {
                let Some(id) = body.get("input_id").and_then(Value::as_i64) else {
                    projection.diagnostics.push(format!(
                        "{task_id}: input {} has no input_id; not projected",
                        file.file_name
                    ));
                    continue;
                };
                let origin = envelope
                    .get("source")
                    .and_then(|source| source.get("origin"))
                    .cloned()
                    .unwrap_or(Value::Null);
                if let Some(run_id) = &run_id {
                    referenced_runs.push((run_id.clone(), file.file_name.clone()));
                }
                projection.inputs.push(InputRow {
                    id,
                    task_id: task_id.to_string(),
                    run_id,
                    stage: text(body, "stage"),
                    source: text(body, "source").unwrap_or_else(|| "unspecified".into()),
                    message: message.to_string(),
                    delivered_at: iso_to_sqlite_time(
                        body.get("delivered_at")
                            .and_then(Value::as_str)
                            .unwrap_or(&at),
                    ),
                    origin_peer_id: text(&origin, "peer_id"),
                    origin_task_id: text(&origin, "task_id"),
                    origin_input_id: origin.get("input_id").and_then(Value::as_i64),
                    origin_run_id: text(&origin, "run_id"),
                    channel_identity: json_column(envelope.get("channel_identity")),
                });
            }
            LedgerEntryKind::Transition => {
                let to_stage = text(body, "to_stage");
                if let Some(stage) = send_back_stage(file) {
                    budgets.remove(&stage);
                }
                last_transition_to = Some(to_stage);
            }
            LedgerEntryKind::Plan => {
                last_plan_after = body.get("after").cloned();
            }
        }
    }
    for (run_id, row) in runs.iter_mut() {
        row.started_at = first_mention.get(run_id).cloned().unwrap_or_default();
        if let Some(identity) = sessions.remove(run_id) {
            row.workspace_id = identity.workspace_id;
            row.session_branch = identity.branch;
            row.session_name = identity.name;
            row.transcript_ref = identity.transcript_ref;
        }
    }
    let state_runs: BTreeSet<String> = snapshot
        .state
        .as_ref()
        .and_then(|tables| tables.get("stage_run"))
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(|row| text(row, "id")).collect())
        .unwrap_or_default();
    for (run_id, file_name) in referenced_runs {
        if !runs.contains_key(&run_id) && !state_runs.contains(&run_id) {
            projection.diagnostics.push(format!(
                "{task_id}: run {run_id} is named by {file_name} but recorded no result; not projected, and the input's reference to it is dropped"
            ));
        }
    }
    if let Some(to_stage) = last_transition_to {
        if to_stage != snapshot.stage {
            projection.diagnostics.push(format!(
                "{task_id}: task.json stage {:?} differs from the last transition's {:?}",
                snapshot.stage, to_stage
            ));
        }
    }
    if let Some(after) = last_plan_after {
        if Some(&after) != snapshot.workflow_definition.as_ref() {
            projection.diagnostics.push(format!(
                "{task_id}: task.json workflow differs from the last plan entry's"
            ));
        }
    }
    if let Some(last) = directory.entries.last() {
        if last.file.sequence > snapshot.published_through {
            projection.diagnostics.push(format!(
                "{task_id}: task.json was written through sequence {} but the ledger reaches {}",
                snapshot.published_through, last.file.sequence
            ));
        }
    }
    match &snapshot.state {
        // The rows themselves: runs, budgets and everything else.
        Some(tables) => project_state(directory, tables, projection),
        // A task.json from before `state`: what the ledger alone says.
        None => {
            projection.stage_runs.extend(runs.into_values());
            projection.budgets.extend(budgets.into_values());
            let mut carried = Vec::new();
            let highest = directory
                .entries
                .last()
                .map_or(0, |entry| entry.file.sequence);
            raise_sequence_high_water(task_id, highest, &mut carried);
            projection.carried.extend(carried);
        }
    }
    projection.markers.push(TaskLedgerMarker {
        task_id: task_id.to_string(),
        snapshot_revision: snapshot.snapshot_revision.max(1),
        historical_entries: historical,
        marked_at: snapshot.created_at.clone().unwrap_or_default(),
    });
}

/// The number `n` of a `task-<task-id>-<n>` branch of this task.
fn branch_suffix(task_id: &str, branch: &str) -> Option<i64> {
    branch
        .strip_prefix(&format!("task-{task_id}-"))
        .filter(|suffix| !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|suffix| suffix.parse().ok())
}

/// Raise the task's ledger sequence high-water mark to `highest`, adding
/// the row when `task.json` carried none.
fn raise_sequence_high_water(task_id: &str, highest: i64, carried: &mut Vec<CarriedRow>) {
    let existing = carried
        .iter_mut()
        .find(|row| row.table == "task_ledger_sequence");
    match existing {
        Some(row) => {
            let high_water = row
                .row
                .get("high_water")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            row.row
                .insert("high_water".into(), json!(high_water.max(highest)));
        }
        None if highest > 0 => {
            let mut row = Map::new();
            row.insert("task_id".into(), json!(task_id));
            row.insert("high_water".into(), json!(highest));
            carried.push(CarriedRow {
                table: "task_ledger_sequence",
                task_id: task_id.to_string(),
                row,
            });
        }
        None => {}
    }
}

/// A result entry that is a verdict recorded live on this machine: not an
/// engine-observed ending, not history (backfilled or carried by a transfer,
/// whose runs the rows already hold or never held).
fn is_live_verdict(file: &LedgerFile) -> bool {
    file.kind == LedgerEntryKind::Result
        && file.envelope.get("historical").and_then(Value::as_bool) != Some(true)
        && !crate::db::task_store::is_engine_observed_result(file.body())
}

/// An owed transition carried in `state` stays owed unless a ledger entry
/// newer than `task.json` shows it was paid or replaced: a transition (the
/// dispatch it owed, or any move that fences it stale), or a newer verdict
/// under another operation (a corrected result replaces or clears it).
/// Unrelated newer entries (inputs, engine-observed endings, plans) leave it
/// owed; the continuation's own stage/generation fence still applies when
/// it is dispatched.
fn retain_unpaid_continuations(
    task_id: &str,
    newer: &[&LedgerFile],
    carried: &mut Vec<CarriedRow>,
    projection: &mut Projection,
) {
    carried.retain(|row| {
        if row.table != "task_ledger_continuation" {
            return true;
        }
        let operation = row.row.get("operation_id").and_then(Value::as_str);
        let paid = newer.iter().find(|file| {
            file.kind == LedgerEntryKind::Transition
                || (is_live_verdict(file)
                    && text(&file.envelope, "operation_id").as_deref() != operation)
        });
        match paid {
            Some(file) => {
                projection.diagnostics.push(format!(
                    "{task_id}: owed transition {} is not restored: {} was recorded after task.json and paid or replaced it",
                    operation.unwrap_or("?"),
                    file.file_name
                ));
                false
            }
            None => {
                if !newer.is_empty() {
                    projection.diagnostics.push(format!(
                        "{task_id}: owed transition {} is restored: no entry recorded after task.json paid or replaced it",
                        operation.unwrap_or("?")
                    ));
                }
                true
            }
        }
    });
}

/// Ledger entries newer than `task.json` happened after its `state` was
/// taken (a crash between publishing an entry and rewriting `task.json`).
/// Their effects on runs and budgets are applied on top of the rows: a
/// verdict or engine-observed ending closes its run, a routed result spends
/// its budget, a send-back resets it. A run the rows do not hold yet is
/// projected from the ledger alone.
fn apply_newer_entries(
    task_id: &str,
    newer: &[&LedgerFile],
    carried: &mut Vec<CarriedRow>,
    projection: &mut Projection,
) {
    let mut ledger_runs: BTreeMap<String, StageRunRow> = BTreeMap::new();
    let mut ledger_budgets: BTreeMap<String, BudgetRow> = BTreeMap::new();
    for file in newer {
        if file.kind == LedgerEntryKind::Transition {
            if let Some(stage) = send_back_stage(file) {
                carried.retain(|row| {
                    !(row.table == "task_stage_budget"
                        && row.row.get("stage").and_then(Value::as_str) == Some(stage.as_str()))
                });
                ledger_budgets.remove(&stage);
            }
            continue;
        }
        if file.kind != LedgerEntryKind::Result
            || file.envelope.get("historical").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(run_id) = text(&file.envelope, "run_id") else {
            continue;
        };
        let ending = crate::db::task_store::is_engine_observed_result(file.body());
        let row = if ending {
            ending_run_row(task_id, &run_id, file)
        } else {
            verdict_run_row(task_id, &run_id, file)
        };
        let existing = carried.iter_mut().find(|carried| {
            carried.table == "stage_run"
                && carried.row.get("id").and_then(Value::as_str) == Some(run_id.as_str())
        });
        match existing {
            Some(existing) => {
                let columns = &mut existing.row;
                columns.insert("status".into(), json!(row.status));
                columns.insert("finished_at".into(), json!(row.finished_at));
                columns.insert("no_work_termination".into(), json!(row.no_work_termination));
                if !ending {
                    columns.insert("result".into(), json!(row.result));
                    columns.insert("feedback".into(), json!(row.feedback));
                    columns.insert(
                        "result_declared_role".into(),
                        json!(row.result_declared_role),
                    );
                    columns.insert(
                        "result_channel_identity".into(),
                        json!(row.result_channel_identity),
                    );
                }
            }
            None if ending && ledger_runs.contains_key(&run_id) => {}
            None => {
                let mut row = row;
                row.started_at = row.finished_at.clone();
                if let Some(identity) = session_identity(&file.envelope) {
                    row.workspace_id = identity.workspace_id;
                    row.session_branch = identity.branch;
                    row.session_name = identity.name;
                    row.transcript_ref = identity.transcript_ref;
                }
                ledger_runs.insert(run_id.clone(), row);
            }
        }
        if let Some((budget, exhausted)) = budget_claim(task_id, file) {
            let carried_budget = carried.iter_mut().find(|carried| {
                carried.table == "task_stage_budget"
                    && carried.row.get("stage").and_then(Value::as_str)
                        == Some(budget.stage.as_str())
            });
            match carried_budget {
                // An exhausted claim changes nothing in SQL.
                Some(_) if exhausted => {}
                Some(existing) => {
                    existing.row.insert("spent".into(), json!(budget.spent));
                    existing
                        .row
                        .insert("updated_at".into(), json!(budget.updated_at));
                }
                None if exhausted && ledger_budgets.contains_key(&budget.stage) => {}
                None => {
                    ledger_budgets.insert(budget.stage.clone(), budget);
                }
            }
        }
    }
    projection.stage_runs.extend(ledger_runs.into_values());
    projection.budgets.extend(ledger_budgets.into_values());
}

/// A task's `state`, row for row. Three rules on top of copying:
///
/// - ledger entries newer than `task.json` are applied on top of its rows
///   ([`apply_newer_entries`]);
/// - an owed transition (`task_ledger_continuation`) is dropped only when a
///   newer entry paid or replaced it ([`retain_unpaid_continuations`]);
/// - the branch counter is raised to the highest `task-<id>-<n>` suffix any
///   record of the task names, so a rebuilt counter never hands out a
///   number already in use.
fn project_state(
    directory: &TaskDirectory,
    tables: &Map<String, Value>,
    projection: &mut Projection,
) {
    let snapshot = &directory.snapshot;
    let task_id = snapshot.task_id.as_str();
    let ledger_reaches = directory
        .entries
        .last()
        .map_or(0, |entry| entry.file.sequence);
    let mut named_suffix: Option<i64> = None;
    let mut name = |branch: Option<&str>| {
        if let Some(n) = branch.and_then(|branch| branch_suffix(task_id, branch)) {
            named_suffix = Some(named_suffix.map_or(n, |current: i64| current.max(n)));
        }
    };
    for (table, column) in [
        ("pipeline_item", "branch"),
        ("worktree", "branch"),
        ("stage_workspace", "branch"),
        ("stage_run", "session_branch"),
    ] {
        for row in tables
            .get(table)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            name(row.get(column).and_then(Value::as_str));
        }
    }
    for entry in &directory.entries {
        name(entry.file.body().get("branch").and_then(Value::as_str));
        name(
            entry
                .file
                .envelope
                .get("session_ref")
                .and_then(|reference| reference.get("branch"))
                .and_then(Value::as_str),
        );
    }
    let mut carried = Vec::new();
    for table in CARRIED_TABLES {
        let Some(rows) = tables.get(table.table).and_then(Value::as_array) else {
            continue;
        };
        for row in rows {
            let Some(row) = row.as_object() else {
                continue;
            };
            let mut row = row.clone();
            if table.table == "task_branch_counter" {
                let last = row.get("last_allocated").and_then(Value::as_i64);
                if let (Some(last), Some(named)) = (last, named_suffix) {
                    if named > last {
                        projection.diagnostics.push(format!(
                            "{task_id}: branch counter {last} is below the recorded branch suffix {named}; raised"
                        ));
                        row.insert("last_allocated".into(), json!(named));
                    }
                }
            }
            carried.push(CarriedRow {
                table: table.table,
                task_id: task_id.to_string(),
                row,
            });
        }
    }
    // Entries whose effects the rows do not hold yet: above the boundary
    // the rows were read at, or reserved below it and filled afterwards. A
    // `state` from before `reflects_through` existed falls back to the
    // publication watermark.
    let boundary = snapshot
        .state_reflects_through
        .unwrap_or(snapshot.published_through);
    let newer: Vec<&LedgerFile> = directory
        .entries
        .iter()
        .map(|entry| &entry.file)
        .filter(|file| {
            file.sequence > boundary || snapshot.state_unreflected.contains(&file.sequence)
        })
        .collect();
    if !newer.is_empty() {
        projection.diagnostics.push(format!(
            "{task_id}: task.json state reflects the ledger through sequence {boundary} and the ledger reaches {ledger_reaches}; the {} newer entries are applied on top of it",
            newer.len()
        ));
    }
    retain_unpaid_continuations(task_id, &newer, &mut carried, projection);
    apply_newer_entries(task_id, &newer, &mut carried, projection);
    // Sequences are never handed out twice: the rebuilt allocator starts
    // above every sequence the directory records, reflects or reserved.
    let highest = snapshot
        .state_unreflected
        .iter()
        .copied()
        .chain([ledger_reaches, boundary])
        .max()
        .unwrap_or(0);
    raise_sequence_high_water(task_id, highest, &mut carried);
    projection.carried.extend(carried);
    if let Some(task) = projection
        .tasks
        .iter_mut()
        .rev()
        .find(|task| task.id == task_id)
    {
        task.from_state = tables
            .get("pipeline_item")
            .and_then(Value::as_array)
            .is_some_and(|rows| !rows.is_empty());
    }
}

/// Project task directories into rows. Pure and deterministic: the same
/// directories in any order give the same projection.
pub fn project(directories: &[TaskDirectory]) -> Projection {
    project_records(directories, &[], &KnownRows::default())
}

/// [`project`] over a whole scanned store, repository records included.
pub fn project_store(scan: &StoreScan) -> Projection {
    project_records(&scan.tasks, &scan.repos, &KnownRows::default())
}

/// Rows a database already holds that a projection's rows may name (T13c).
/// A rebuild into a new database knows only what it projects; reconciling
/// some tasks of a live database keeps their references to tasks, runs and
/// joins the database holds and the projected directories do not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownRows {
    pub tasks: BTreeSet<String>,
    pub runs: BTreeSet<String>,
    pub joins: BTreeSet<String>,
}

/// [`project_store`], resolving references against `known` as well.
pub fn project_store_onto(scan: &StoreScan, known: &KnownRows) -> Projection {
    project_records(&scan.tasks, &scan.repos, known)
}

fn project_records(
    directories: &[TaskDirectory],
    repos: &[RepoRecord],
    known: &KnownRows,
) -> Projection {
    let mut ordered: Vec<&TaskDirectory> = directories.iter().collect();
    ordered.sort_by(|a, b| {
        (&a.snapshot.repo_id, &a.snapshot.task_id).cmp(&(&b.snapshot.repo_id, &b.snapshot.task_id))
    });
    let mut projection = Projection::default();
    for directory in ordered {
        project_task(directory, &mut projection);
    }
    let mut records: Vec<RepoRecord> = repos.to_vec();
    records.sort_by(|a, b| a.repo_id.cmp(&b.repo_id));
    records.dedup_by(|a, b| a.repo_id == b.repo_id);
    projection.repos = projection
        .tasks
        .iter()
        .map(|task| task.repo_id.clone())
        .chain(records.iter().map(|record| record.repo_id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    for repo_id in &projection.repos {
        if !records.iter().any(|record| &record.repo_id == repo_id) {
            projection.diagnostics.push(format!(
                "repo {repo_id}: no repo.json; its registration is a placeholder (empty path, name = id)"
            ));
        }
    }
    projection.repo_records = records;
    // Every table's rows in their original insertion order, all tasks'
    // task rows before anything that references one.
    let table_index = |name: &str| {
        CARRIED_TABLES
            .iter()
            .position(|table| table.table == name)
            .unwrap_or(usize::MAX)
    };
    projection.carried.sort_by_key(|carried| {
        (
            table_index(carried.table),
            carried
                .row
                .get("rowid")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX),
            carried.task_id.clone(),
        )
    });
    // The schema refuses references to rows that are not rebuilt; they are
    // reported rather than invented.
    let tasks: BTreeSet<String> = projection
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .chain(known.tasks.iter().cloned())
        .collect();
    let carried_ids = |projection: &Projection, table: &str, column: &str| -> BTreeSet<String> {
        projection
            .carried
            .iter()
            .filter(|carried| carried.table == table)
            .filter_map(|carried| text(&Value::Object(carried.row.clone()), column))
            .collect()
    };
    let runs: BTreeSet<String> = projection
        .stage_runs
        .iter()
        .map(|run| run.id.clone())
        .chain(carried_ids(&projection, "stage_run", "id"))
        .chain(known.runs.iter().cloned())
        .collect();
    let mut joins = carried_ids(&projection, "task_join", "id");
    joins.extend(known.joins.iter().cloned());
    let mut notes = Vec::new();
    projection.carried.retain(|carried| {
        let row = Value::Object(carried.row.clone());
        let dangling = match carried.table {
            "task_stage_edge" => text(&row, "upstream_task_id")
                .filter(|upstream| !tasks.contains(upstream))
                .map(|upstream| format!("upstream task {upstream}")),
            "stage_run_prompt" | "workspace_setup_run" | "contextless_completion_attempt" => {
                text(&row, "run_id")
                    .filter(|run| !runs.contains(run))
                    .map(|run| format!("run {run}"))
            }
            "task_join_member" => text(&row, "join_id")
                .filter(|join| !joins.contains(join))
                .map(|join| format!("join {join}")),
            _ => None,
        };
        if let Some(missing) = &dangling {
            notes.push(format!(
                "{}: a {} row names {missing}, which is not rebuilt; not projected",
                carried.task_id, carried.table
            ));
        }
        dangling.is_none()
    });
    projection.blockers.retain(|(blocked, blocker)| {
        let known = tasks.contains(blocker);
        if !known {
            notes.push(format!(
                "{blocked}: dependency on {blocker}, which has no task directory; not projected"
            ));
        }
        known
    });
    for input in &mut projection.inputs {
        if input.run_id.as_ref().is_some_and(|run| !runs.contains(run)) {
            input.run_id = None;
        }
    }
    projection.diagnostics.extend(notes);
    projection
        .inputs
        .sort_by(|a, b| (&a.task_id, a.id).cmp(&(&b.task_id, b.id)));
    projection
}

/// What an offline rebuild did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RebuildReport {
    pub tasks: usize,
    pub ledger_entries: usize,
    pub stage_runs: usize,
    pub inputs: usize,
    pub blockers: usize,
    pub budgets: usize,
    /// Rows of the carried tables written from `state`.
    pub carried_rows: usize,
    /// Task directories that could not be read, and why. Their tasks are not
    /// in the rebuilt database.
    pub unreadable: Vec<Unreadable>,
    /// Tombstones: tasks and repositories their database removed.
    pub removed: Vec<PathBuf>,
    pub diagnostics: Vec<String>,
}

/// Rebuild every task under `root` into a new database at `target`, created
/// with the current schema. Refuses an existing `target`: this never writes
/// into a database that holds anything else, least of all the live one.
pub fn rebuild_into_new_database(root: &Path, target: &Path) -> Result<RebuildReport, String> {
    if target.exists() {
        return Err(format!(
            "{} already exists; a rebuild only writes a new database",
            target.display()
        ));
    }
    rebuild_scan_into_new_database(scan_store_records(root)?, target)
}

/// [`rebuild_into_new_database`] from an already scanned (and possibly
/// scoped, see [`StoreScan::for_installation`]) store.
pub fn rebuild_scan_into_new_database(
    scan: StoreScan,
    target: &Path,
) -> Result<RebuildReport, String> {
    if target.exists() {
        return Err(format!(
            "{} already exists; a rebuild only writes a new database",
            target.display()
        ));
    }
    let projection = project_store(&scan);
    let target_path = target
        .to_str()
        .ok_or_else(|| format!("{} is not a UTF-8 path", target.display()))?;
    let db = crate::db::Db::open_migrated(target_path)
        .map_err(|error| format!("open target: {error}"))?;
    if !db
        .is_empty_for_disk_rebuild()
        .map_err(|error| format!("inspect target: {error}"))?
    {
        return Err(format!("{} is not an empty database", target.display()));
    }
    db.apply_disk_projection(&projection)
        .map_err(|error| format!("write projection: {error}"))?;
    let carried_count = |table: &str| {
        projection
            .carried
            .iter()
            .filter(|carried| carried.table == table)
            .count()
    };
    Ok(RebuildReport {
        tasks: projection.tasks.len(),
        ledger_entries: projection.ledger.len(),
        stage_runs: projection.stage_runs.len() + carried_count("stage_run"),
        inputs: projection.inputs.len(),
        blockers: projection.blockers.len(),
        budgets: projection.budgets.len() + carried_count("task_stage_budget"),
        carried_rows: projection.carried.len(),
        unreadable: scan.unreadable,
        removed: scan.removed,
        diagnostics: projection.diagnostics,
    })
}

/// The run a carried result (T9) projects under: its origin run's carried
/// key, or the origin entry's when the origin recorded no run.
fn carried_result_run_id(task_id: &str, envelope: &Value) -> Option<String> {
    let source = envelope.get("source")?;
    if source.get("kind").and_then(Value::as_str) != Some("transferred_ledger_entry") {
        return None;
    }
    let origin = source.get("origin")?;
    let origin_run = origin
        .get("run_id")
        .and_then(Value::as_str)
        .or_else(|| origin.get("ledger_entry").and_then(Value::as_str))?;
    Some(crate::db::transfer_task_state::carried_run_id(
        task_id, origin_run,
    ))
}
