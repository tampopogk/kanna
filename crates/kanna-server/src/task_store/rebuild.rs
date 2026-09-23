//! Offline rebuild of SQL projections from task directories (spec §16.11,
//! component T13, first increment).
//!
//! Reads `<root>/repos/<repo-id>/tasks/<task-id>/` (`task.json` and the
//! published `ledger/`) with a versioned reader, projects them
//! deterministically, and writes the projection into a **new** database
//! created with the current schema. It never opens an existing database:
//! SQLite stays authoritative, and this is a dry run whose output is compared
//! against it. `docs/2026-09-23-disk-authority-inventory.md` says which disk
//! record will own each table and which facts are not on disk yet
//! ([`NOT_REBUILT`]).
//!
//! What is projected, and from where:
//!
//! - **task** (`pipeline_item`) and **workflow pin**: from `task.json`.
//! - **dependency blockers** (`task_blocker`): `task.json` `links.dependencies`.
//! - **stage runs** (`stage_run`): one row per run that recorded a result
//!   entry; the newest result of a run wins, as a corrected verdict does in
//!   SQL. A run with no result entry (running, or ended without a verdict)
//!   is not on disk and is not invented.
//! - **inputs** (`task_input`): one row per `input` entry, keeping its row id.
//! - **transitions and plans**: the ledger itself, mirrored into the
//!   `task_ledger_entry` outbox as published rows with the file's exact bytes,
//!   so a server started on the rebuilt database continues the sequence
//!   instead of colliding with files already on disk.
//! - **stage budgets** (`task_stage_budget`): replayed from the budget each
//!   routed result records, reset by a person's send-back (an operator exit
//!   other than `advance`).
//!
//! Unknown facts stay unknown: a column the disk does not carry is left NULL
//! or at its schema default; where the schema requires a value the ledger
//! only approximates (a run's start time), the approximation is named in
//! [`NOT_REBUILT`] and never compared as if it were the fact.

use super::{parse_ledger_file, LedgerFile};
use crate::db::task_store::LedgerEntryKind;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The `schema_version`s of `task.json` and ledger envelopes this reader
/// understands. Anything else is refused, never guessed at.
pub const SUPPORTED_SCHEMA_VERSIONS: &[u64] = &[1];

/// One class of fact the projector does not rebuild from disk yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotRebuilt {
    /// The fact class, as the round-trip comparison names it.
    pub fact: &'static str,
    /// Why, and where its disk authority will come from.
    pub reason: &'static str,
    /// `false` for a required column the projector fills with a documented
    /// approximation: comparing it would only measure clock timing.
    pub compared: bool,
}

/// Facts a rebuild from task directories cannot yet restore — the input to
/// later T13 increments. The fixture round trip asserts that exactly the
/// compared ones differ.
pub const NOT_REBUILT: &[NotRebuilt] = &[
    NotRebuilt {
        fact: "repo.registration",
        reason: "repository path, name, default branch and remote live only in SQL (their authority will be the repo directory / local config); the rebuild writes a placeholder row (empty path, name = repo id, schema defaults) only so tasks satisfy the schema's foreign key",
        compared: true,
    },
    NotRebuilt {
        fact: "task.agent_type",
        reason: "task-level agent defaults are not in task.json",
        compared: true,
    },
    NotRebuilt {
        fact: "task.agent_provider",
        reason: "task-level provider default is not in task.json",
        compared: true,
    },
    NotRebuilt {
        fact: "task.initial_pipeline",
        reason: "the workflow the task was created with is not in task.json; only later plan entries name a from_workflow",
        compared: true,
    },
    NotRebuilt {
        fact: "task.revision_rounds",
        reason: "legacy revision round counter: legacy revision entries record no round number",
        compared: true,
    },
    NotRebuilt {
        fact: "task.attention_requested",
        reason: "attention badge is SQL-only",
        compared: true,
    },
    NotRebuilt {
        fact: "task.pinned",
        reason: "sidebar pin is SQL-only (a local preference)",
        compared: true,
    },
    NotRebuilt {
        fact: "task.worktree",
        reason: "the task's worktree row (path, branch, setup state) is not in the ledger",
        compared: true,
    },
    NotRebuilt {
        fact: "task.stage_workspace",
        reason: "T2's stage_workspace rows (directory path per stage) are not in the ledger; session_ref names only the workspace id",
        compared: true,
    },
    NotRebuilt {
        fact: "task.branch_counter",
        reason: "T2's task_branch_counter is not in the ledger; T2 re-seeds it above repository refs and every branch the task's records name, so only numbers spent by attempts that left no branch, directory or record can be reissued",
        compared: true,
    },
    NotRebuilt {
        fact: "run.exists",
        reason: "a stage run with no result entry (running, or ended without a verdict: session exit, spawn failure, teardown) writes nothing to the ledger",
        compared: true,
    },
    NotRebuilt {
        fact: "run.session",
        reason: "agent, provider, model, effort, provider session id, cwd, resume/replace links, entry trigger and channel, and T2's workspace_report: not in any ledger entry (T2's session_ref restores workspace id, session branch, name and transcript)",
        compared: true,
    },
    NotRebuilt {
        fact: "run.completion",
        reason: "completion_transition and completion_bound are engine bookkeeping on the run, not in result entries",
        compared: true,
    },
    NotRebuilt {
        fact: "run.resolved_prompt",
        reason: "the prompt a session was started with (stage_run_prompt) is not in the ledger",
        compared: true,
    },
    NotRebuilt {
        fact: "run.summary",
        reason: "a revision request's ledger message joins its summary and findings; complete-stage summaries rebuild exactly",
        compared: true,
    },
    NotRebuilt {
        fact: "run.feedback",
        reason: "a revision request's findings are only inside the joined ledger message; complete-stage feedback rebuilds exactly",
        compared: true,
    },
    NotRebuilt {
        fact: "run.result_declared_role",
        reason: "a backfilled (historical) result records no declared role, by T0's rule that history is never attributed after the fact; live results rebuild exactly",
        compared: true,
    },
    NotRebuilt {
        fact: "run.result_channel",
        reason: "a backfilled (historical) result records the channel as unknown, by the same rule; live results rebuild exactly",
        compared: true,
    },
    NotRebuilt {
        fact: "input.run_id",
        reason: "an input to a run that recorded no result names a run the rebuild cannot create, and the schema's foreign key forbids a dangling reference: projected as NULL",
        compared: true,
    },
    NotRebuilt {
        fact: "run.started_at",
        reason: "a run's start is not recorded; projected as its first ledger mention",
        compared: false,
    },
    NotRebuilt {
        fact: "run.finished_at",
        reason: "projected as its newest result's recorded_at, which is written in the same transaction but not the same clock read",
        compared: false,
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
    Ok(TaskSnapshot {
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

/// Every task directory under a store root, ordered by repo then task id.
/// Directories that cannot be read are returned separately rather than
/// dropped silently.
pub fn scan_store(root: &Path) -> Result<(Vec<TaskDirectory>, Vec<Unreadable>), String> {
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
    let mut read = Vec::new();
    let mut unreadable = Vec::new();
    for repo in sorted_dirs(&root.join("repos"))? {
        for task in sorted_dirs(&repo.join("tasks"))? {
            match read_task_directory(&task) {
                Ok(directory) => read.push(directory),
                Err(error) => unreadable.push((task, error)),
            }
        }
    }
    Ok((read, unreadable))
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRunRow {
    pub id: String,
    pub task_id: String,
    pub stage: String,
    pub kind: String,
    pub status: String,
    pub result: String,
    pub feedback: Option<String>,
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

/// Everything one rebuild writes, in a stable order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Projection {
    /// Repository ids the tasks belong to. Only ids: the registration is not
    /// on disk (see [`NOT_REBUILT`]).
    pub repos: Vec<String>,
    pub tasks: Vec<TaskRow>,
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
        let body = file.body();
        let message = file.message.as_deref().unwrap_or("");
        match file.kind {
            LedgerEntryKind::Result => {
                let Some(run_id) = run_id else {
                    projection.diagnostics.push(format!(
                        "{task_id}: result {} names no run; no stage run projected",
                        file.file_name
                    ));
                    continue;
                };
                let status = text(body, "status").unwrap_or_else(|| "unknown".into());
                runs.insert(
                    run_id.clone(),
                    StageRunRow {
                        id: run_id.clone(),
                        task_id: task_id.to_string(),
                        stage: text(body, "stage").unwrap_or_default(),
                        kind: text(body, "run_kind").unwrap_or_else(|| "main".into()),
                        status: if status == "success" {
                            "succeeded".into()
                        } else {
                            "failed".into()
                        },
                        result: stage_result_column(file, message),
                        feedback: Some(message.to_string()),
                        started_at: String::new(),
                        finished_at: iso_to_sqlite_time(&at),
                        result_declared_role: text(envelope, "declared_role"),
                        result_channel_identity: json_column(envelope.get("channel_identity")),
                        workspace_id: None,
                        session_branch: None,
                        session_name: None,
                        transcript_ref: None,
                    },
                );
                if let Some(budget) = body.get("budget").filter(|budget| budget.is_object()) {
                    let stage = text(budget, "stage");
                    let spent = budget.get("spent").and_then(Value::as_i64);
                    let exhausted = budget.get("exhausted").and_then(Value::as_bool) == Some(true);
                    if let (Some(stage), Some(spent)) = (stage, spent) {
                        // An exhausted claim changes nothing in SQL; it only
                        // proves the destination's spend if nothing else did.
                        if !exhausted || !budgets.contains_key(&stage) {
                            budgets.insert(
                                stage.clone(),
                                BudgetRow {
                                    task_id: task_id.to_string(),
                                    stage,
                                    spent,
                                    updated_at: iso_to_sqlite_time(&at),
                                },
                            );
                        }
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
                // A person sending the task back gives it a fresh budget
                // there; operating a gate (`advance`) does not.
                let send_back = text(body, "exit_source").as_deref() == Some("operator")
                    && text(body, "exit").as_deref() != Some("advance");
                if let (true, Some(stage)) = (send_back, &to_stage) {
                    budgets.remove(stage);
                }
                last_transition_to = Some(to_stage);
            }
            LedgerEntryKind::Plan => {
                last_plan_after = body.get("after").cloned();
            }
        }
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
            recorded_at: at,
        });
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
    for (run_id, file_name) in referenced_runs {
        if !runs.contains_key(&run_id) {
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
    projection.stage_runs.extend(runs.into_values());
    projection.budgets.extend(budgets.into_values());
    projection.markers.push(TaskLedgerMarker {
        task_id: task_id.to_string(),
        snapshot_revision: snapshot.snapshot_revision.max(1),
        historical_entries: historical,
        marked_at: snapshot.created_at.clone().unwrap_or_default(),
    });
}

/// Project task directories into rows. Pure and deterministic: the same
/// directories in any order give the same projection.
pub fn project(directories: &[TaskDirectory]) -> Projection {
    let mut ordered: Vec<&TaskDirectory> = directories.iter().collect();
    ordered.sort_by(|a, b| {
        (&a.snapshot.repo_id, &a.snapshot.task_id).cmp(&(&b.snapshot.repo_id, &b.snapshot.task_id))
    });
    let mut projection = Projection::default();
    for directory in ordered {
        project_task(directory, &mut projection);
    }
    projection.repos = projection
        .tasks
        .iter()
        .map(|task| task.repo_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // The schema refuses references to rows that are not rebuilt; they are
    // reported rather than invented.
    let tasks: BTreeSet<String> = projection
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect();
    let runs: BTreeSet<String> = projection
        .stage_runs
        .iter()
        .map(|run| run.id.clone())
        .collect();
    let mut notes = Vec::new();
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
    /// Task directories that could not be read, and why. Their tasks are not
    /// in the rebuilt database.
    pub unreadable: Vec<Unreadable>,
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
    let (directories, unreadable) = scan_store(root)?;
    let projection = project(&directories);
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
    Ok(RebuildReport {
        tasks: projection.tasks.len(),
        ledger_entries: projection.ledger.len(),
        stage_runs: projection.stage_runs.len(),
        inputs: projection.inputs.len(),
        blockers: projection.blockers.len(),
        budgets: projection.budgets.len(),
        unreadable,
        diagnostics: projection.diagnostics,
    })
}
