//! Carried task state: the task directory, the rows beside the task row, and
//! the objects they name (spec §11, §16.11 — component T9).
//!
//! # What crosses
//!
//! One JSON document, [`TaskStateDocument`], staged as a sidecar artifact
//! exactly like the input ledger and bound to the payload by its SHA-256:
//!
//! - the task directory as published: `task.json` and every `ledger/` file,
//!   verbatim. The source flushes its outbox first and refuses to ship while
//!   an entry is still unpublished, so nothing recorded is left behind;
//! - [`CarriedTaskRows`]: T2's branch counter and stage workspaces, T1's
//!   budgets, T3's settled commit steps, and T4/T5 edges and joins that have
//!   nothing left owed across them (open ones refuse the transfer —
//!   [`crate::db::Db::transfer_state_blocker`]), plus the ownership
//!   generation;
//! - the references the destination needs objects for: stored artifacts the
//!   results name ([`TransferTaskStatePayload::artifact_bundle`], moved with
//!   the artifact repository's own push/fetch) and commits the results,
//!   commit steps and stage workspaces name that the repository bundle does
//!   not reach ([`TransferTaskStatePayload::history_bundle`]).
//!
//! # How the destination takes it
//!
//! The task gets a new id there, and ledger entry ids embed the task id, so
//! the source's directory cannot simply be copied into place. Instead:
//!
//! 1. every `result`, `transition` and `plan` entry is re-recorded, in order,
//!    as a historical entry of the destination task, carrying its first
//!    origin (`source.origin.ledger_entry`) and with the result ids it names
//!    rewritten to the entries that mirror them. Input entries are not
//!    re-recorded here: the input ledger already carries them with their
//!    origin (T0's import path);
//! 2. the verbatim files are kept beside the ledger under
//!    `transferred/<transfer-id>/`, so the exact bytes the source published
//!    stay readable;
//! 3. a `transition` entry with `operation: "transfer_import"` closes the
//!    import. Its `triggering_result_id` is the result that caused the
//!    source's session, so a new session of the stage is told about the same
//!    result the source session was, and its `transfer` object records the
//!    ownership generation and whether the destination session resumed the
//!    transcript or started fresh, and why.
//!
//! Transcripts stay opportunistic (spec §1): when none can be resumed the
//! destination starts a fresh session whose preamble names the ledger and
//! its triggering result, and whose prompt states the fresh-start reason.

use super::payload::{TransferStagedFile, TransferTaskStatePayload};
use crate::db::task_store::{LedgerEntryKind, NewLedgerEntry};
use crate::db::transfer_task_state::{CarriedTaskRows, NewTransferredTaskState};
use crate::db::Db;
use crate::mutation_provenance::ChannelIdentity;
use crate::task_store::{self, LedgerFile, TriggeringResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Entries re-recorded at the destination are keyed on this source kind and
/// the source entry id, so a retried import records each exactly once.
const MIRRORED_SOURCE_KIND: &str = "transferred_ledger_entry";
/// The transfer's own closing transition is keyed on this and the transfer id.
const IMPORT_SOURCE_KIND: &str = "task_transfer";
/// Where history commits travel inside the history bundle.
const HISTORY_BUNDLE_REF_PREFIX: &str = "refs/kanna/transfer-out/";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStateDocument {
    pub version: u64,
    pub source_peer_id: String,
    pub source_task_id: String,
    pub source_repo_id: String,
    pub stage: String,
    /// The source's `task.json` as published, if it had one.
    pub task_json: Option<String>,
    /// Every published ledger file, in sequence order.
    pub ledger: Vec<CarriedLedgerFile>,
    pub rows: CarriedTaskRows,
    /// Stored artifacts the carried results reference, in the source repo.
    #[serde(default)]
    pub artifacts: Vec<CarriedArtifact>,
    /// Commits the ledger, commit steps and stage workspaces name, under the
    /// names the destination records them by (`commits/<sha>`,
    /// `branches/<branch>`).
    #[serde(default)]
    pub history_refs: Vec<CarriedRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedLedgerFile {
    pub file_name: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedArtifact {
    pub artifact_id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CarriedRef {
    pub name: String,
    pub oid: String,
}

/// How the destination's first session started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStart {
    Resumed,
    Fresh(String),
}

impl SessionStart {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Resumed => "resumed",
            Self::Fresh(_) => "fresh",
        }
    }

    pub fn fresh_start_reason(&self) -> Option<&str> {
        match self {
            Self::Resumed => None,
            Self::Fresh(reason) => Some(reason),
        }
    }
}

/// A verified document plus what the destination decided while importing it.
#[derive(Debug, Clone)]
pub struct ImportedTaskState {
    pub transfer_id: String,
    pub sha256: String,
    pub document: TaskStateDocument,
    pub destination_repo_id: String,
    pub session_start: SessionStart,
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

/// Does this task hold anything only a destination that reads carried task
/// state can keep? A task with none (no ledger entry, no T1–T5 row) is a
/// legacy task, which an older destination may still take as before.
pub fn task_requires_carry(db: &Db, task_id: &str) -> Result<bool, String> {
    let entries = db
        .published_ledger_files(task_id)
        .map_err(|error| format!("db error: {error}"))?;
    let unpublished = db
        .unpublished_ledger_entry_count(task_id)
        .map_err(|error| format!("db error: {error}"))?;
    if !entries.is_empty() || unpublished > 0 {
        return Ok(true);
    }
    let rows = db
        .export_carried_task_rows(task_id, "")
        .map_err(|error| format!("db error: {error}"))?;
    Ok(rows != CarriedTaskRows::default())
}

/// Read everything the task carries. Publishes the task's outbox first and
/// refuses while any entry is still not on disk: a transfer must not leave a
/// recorded result behind, and it cannot ship bytes that do not exist yet.
pub fn collect(
    db: &Db,
    db_path: &str,
    task: &crate::db::PipelineItem,
    source_peer_id: &str,
) -> Result<TaskStateDocument, String> {
    task_store::flush_task(db, db_path, &task.id)
        .map_err(|error| format!("task ledger of {} could not be published: {error}", task.id))?;
    let unpublished = db
        .unpublished_ledger_entry_count(&task.id)
        .map_err(|error| format!("db error: {error}"))?;
    if unpublished > 0 {
        return Err(format!(
            "task {} has {unpublished} ledger entries that are not on disk yet; the transfer \
             waits for them rather than leave them behind",
            task.id
        ));
    }
    let dir = task_store::task_dir(&task_store::root_for_db(db_path), &task.repo_id, &task.id);
    let task_json = match std::fs::read_to_string(dir.join("task.json")) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("read task.json of {}: {error}", task.id)),
    };
    let mut ledger = Vec::new();
    let mut artifacts = Vec::new();
    let mut seen_artifacts = BTreeSet::new();
    for (sequence, file_name) in db
        .published_ledger_files(&task.id)
        .map_err(|error| format!("db error: {error}"))?
    {
        let path = dir.join("ledger").join(&file_name);
        let bytes = std::fs::read(&path).map_err(|error| {
            format!(
                "published ledger file {} is unreadable: {error}",
                path.display()
            )
        })?;
        let file = task_store::parse_ledger_file(&file_name, &bytes)?;
        if file.sequence != sequence {
            return Err(format!(
                "ledger file {file_name} does not hold sequence {sequence}"
            ));
        }
        for reference in stored_artifact_references(&file.envelope, &task.repo_id) {
            if seen_artifacts.insert(reference.artifact_id.clone()) {
                artifacts.push(reference);
            }
        }
        ledger.push(CarriedLedgerFile {
            file_name,
            content: String::from_utf8(bytes)
                .map_err(|error| format!("ledger file is not UTF-8: {error}"))?,
        });
    }
    let mut rows = db
        .export_carried_task_rows(&task.id, source_peer_id)
        .map_err(|error| format!("db error: {error}"))?;
    rows.ownership_generation += 1;
    Ok(TaskStateDocument {
        version: super::payload::TASK_STATE_VERSION,
        source_peer_id: source_peer_id.to_string(),
        source_task_id: task.id.clone(),
        source_repo_id: task.repo_id.clone(),
        stage: task.stage.clone().unwrap_or_else(|| "in progress".into()),
        task_json,
        ledger,
        rows,
        artifacts,
        history_refs: Vec::new(),
    })
}

/// `{"type": "stored", "repoId": <repo>, ...}` references in an envelope's
/// `artifacts`, in this task's repository. A reference into another
/// repository is not this task's to move (T6 refuses to bind one anyway).
fn stored_artifact_references(envelope: &Value, repo_id: &str) -> Vec<CarriedArtifact> {
    envelope
        .get("artifacts")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|artifacts| artifacts.values())
        .filter(|reference| reference.get("type").and_then(Value::as_str) == Some("stored"))
        .filter(|reference| reference.get("repoId").and_then(Value::as_str) == Some(repo_id))
        .filter_map(|reference| {
            Some(CarriedArtifact {
                artifact_id: reference.get("artifactId")?.as_str()?.to_string(),
                kind: reference
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("document")
                    .to_string(),
            })
        })
        .collect()
}

/// Commits the carried state names: results' and commit steps' recorded
/// commits, and the tips of the task's own stage workspace branches.
fn named_history(document: &TaskStateDocument) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut commits = BTreeSet::new();
    for file in &document.ledger {
        let Ok(parsed) = task_store::parse_ledger_file(&file.file_name, file.content.as_bytes())
        else {
            continue;
        };
        if let Some(sha) = parsed.body().get("committed_sha").and_then(Value::as_str) {
            commits.insert(sha.to_string());
        }
    }
    for commit in &document.rows.transition_commits {
        if let Some(sha) = &commit.committed_sha {
            commits.insert(sha.clone());
        }
    }
    let branches = document
        .rows
        .links
        .stage_workspaces
        .iter()
        .filter(|workspace| workspace.origin_task_id == document.source_task_id)
        .map(|workspace| workspace.branch.clone())
        .collect();
    (commits, branches)
}

/// Record the history refs and bundle the objects the repository bundle does
/// not already carry (everything not reachable from `head_oid`). Returns
/// `false` when nothing needs bundling. Blocking.
pub fn stage_history_bundle(
    repo_path: &Path,
    document: &mut TaskStateDocument,
    head_oid: &str,
    transfer_id: &str,
    bundle_path: &Path,
) -> Result<bool, String> {
    let git = |args: &[&str]| super::git::git(repo_path, args);
    let (commits, branches) = named_history(document);
    let mut refs = Vec::new();
    for sha in commits {
        if !is_object_id(&sha) {
            continue;
        }
        // A commit the source no longer has cannot be carried, and inventing
        // one would be worse; the ledger still records the id.
        if git(&["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok() {
            refs.push(CarriedRef {
                name: format!("commits/{sha}"),
                oid: sha,
            });
        }
    }
    for branch in branches {
        if super::git::normalize_ref(Some(&branch)).is_none() {
            continue;
        }
        if let Ok(oid) = git(&[
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{branch}^{{commit}}"),
        ]) {
            refs.push(CarriedRef {
                name: format!("branches/{branch}"),
                oid,
            });
        }
    }
    document.history_refs = refs.clone();
    let outside_head: Vec<&CarriedRef> = refs
        .iter()
        .filter(|reference| {
            git(&["merge-base", "--is-ancestor", &reference.oid, head_oid]).is_err()
        })
        .collect();
    if outside_head.is_empty() {
        return Ok(false);
    }
    let staging_prefix = format!("{HISTORY_BUNDLE_REF_PREFIX}{transfer_id}/");
    let mut staged_refs = Vec::new();
    for reference in &outside_head {
        let name = format!("{staging_prefix}{}", reference.name);
        git(&["update-ref", &name, &reference.oid])?;
        staged_refs.push(name);
    }
    let bundle = bundle_path
        .to_str()
        .ok_or_else(|| "history bundle path is not valid unicode".to_string())?;
    let mut arguments = vec!["bundle", "create", bundle];
    arguments.extend(staged_refs.iter().map(String::as_str));
    let exclude = format!("^{head_oid}");
    arguments.push(&exclude);
    let created = git(&arguments);
    for name in &staged_refs {
        let _ = git(&["update-ref", "-d", name]);
    }
    created?;
    Ok(true)
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn encode(document: &TaskStateDocument) -> Result<Vec<u8>, String> {
    serde_json::to_vec(document).map_err(|error| format!("failed to encode task state: {error}"))
}

// ---------------------------------------------------------------------------
// Artifact objects
// ---------------------------------------------------------------------------

fn fresh_scratch_repository(scratch: &Path) -> Result<(), String> {
    if scratch.exists() {
        std::fs::remove_dir_all(scratch)
            .map_err(|error| format!("remove {}: {error}", scratch.display()))?;
    }
    std::fs::create_dir_all(scratch)
        .map_err(|error| format!("create {}: {error}", scratch.display()))?;
    super::git::git(scratch, &["init", "--bare", "--quiet", "."]).map(|_| ())
}

fn scratch_remote(
    scratch: &Path,
    home: &Path,
    store_path: &Path,
) -> Result<crate::artifacts::remote::ArtifactRemote, String> {
    crate::artifacts::remote::ArtifactRemote::parse(&scratch.to_string_lossy(), home, store_path)
        .map_err(|error| error.to_string())
}

/// Source: push each referenced artifact — with every earlier version and
/// record the artifact repository's own push sends — into a scratch bare
/// repository, and bundle it. A referenced artifact this repository no
/// longer holds fails the transfer: the results that name it would arrive
/// naming nothing. Blocking.
pub fn stage_artifact_bundle(
    store_path: &Path,
    home: &Path,
    repo_id: &str,
    artifacts: &[CarriedArtifact],
    scratch: &Path,
    bundle_path: &Path,
) -> Result<(), String> {
    let store = crate::artifacts::store::ArtifactStore::open_existing(store_path, repo_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!("the task's results reference artifacts, but repository {repo_id} has no artifact store")
        })?;
    fresh_scratch_repository(scratch)?;
    let outcome = (|| -> Result<(), String> {
        let remote = scratch_remote(scratch, home, store_path)?;
        for artifact in artifacts {
            crate::artifacts::remote::push(&store, &remote, &artifact.artifact_id).map_err(
                |error| {
                    format!(
                        "referenced artifact {} cannot be carried: {error}",
                        artifact.artifact_id
                    )
                },
            )?;
        }
        let bundle = bundle_path
            .to_str()
            .ok_or_else(|| "artifact bundle path is not valid unicode".to_string())?;
        super::git::git(scratch, &["bundle", "create", bundle, "--all"]).map(|_| ())
    })();
    let _ = std::fs::remove_dir_all(scratch);
    outcome
}

/// Destination: take the carried artifacts into this repository's artifact
/// store with its own fetch (and its validation), then check each is held
/// with its content. Blocking; idempotent.
pub fn import_artifact_bundle(
    store_path: &Path,
    home: &Path,
    repo_id: &str,
    artifacts: &[CarriedArtifact],
    bundle_path: &Path,
    scratch: &Path,
) -> Result<(), String> {
    let store = crate::artifacts::store::ArtifactStore::open_or_create(store_path, repo_id)
        .map_err(|error| error.to_string())?;
    fresh_scratch_repository(scratch)?;
    let outcome = (|| -> Result<(), String> {
        let bundle = bundle_path
            .to_str()
            .ok_or_else(|| "artifact bundle path is not valid unicode".to_string())?;
        super::git::git(scratch, &["fetch", "--no-tags", bundle, "+refs/*:refs/*"])?;
        let remote = scratch_remote(scratch, home, store_path)?;
        for artifact in artifacts {
            crate::artifacts::remote::fetch(&store, &remote, &artifact.artifact_id).map_err(
                |error| {
                    format!(
                        "carried artifact {} could not be imported: {error}",
                        artifact.artifact_id
                    )
                },
            )?;
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(scratch);
    outcome?;
    verify_artifacts(store_path, repo_id, artifacts)
}

/// Every carried artifact is held here with its content.
pub fn verify_artifacts(
    store_path: &Path,
    repo_id: &str,
    artifacts: &[CarriedArtifact],
) -> Result<(), String> {
    if artifacts.is_empty() {
        return Ok(());
    }
    let store = crate::artifacts::store::ArtifactStore::open_existing(store_path, repo_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("repository {repo_id} has no artifact store"))?;
    for artifact in artifacts {
        let detail = store
            .detail(&artifact.artifact_id)
            .map_err(|error| format!("carried artifact {}: {error}", artifact.artifact_id))?;
        if !detail.retained {
            return Err(format!(
                "carried artifact {} arrived without its content",
                artifact.artifact_id
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Destination: verification
// ---------------------------------------------------------------------------

/// Verify and decode fetched task state before any destination task exists:
/// digest, version, identity, and that every ledger file parses as the entry
/// of the source task its name says it is.
pub fn decode(
    bytes: &[u8],
    metadata: &TransferTaskStatePayload,
    source_peer_id: &str,
    source_task_id: &str,
) -> Result<TaskStateDocument, String> {
    if super::payload::sha256_hex(bytes) != metadata.sha256 {
        return Err("transferred task state checksum does not match its payload".into());
    }
    let document: TaskStateDocument = serde_json::from_slice(bytes)
        .map_err(|error| format!("transferred task state is invalid: {error}"))?;
    if document.version != metadata.version {
        return Err(format!(
            "transferred task state version {} does not match its payload",
            document.version
        ));
    }
    if document.source_peer_id != source_peer_id || document.source_task_id != source_task_id {
        return Err("transferred task state source identity does not match its task".into());
    }
    let mut previous = 0;
    for file in &document.ledger {
        let parsed = task_store::parse_ledger_file(&file.file_name, file.content.as_bytes())
            .map_err(|error| format!("transferred ledger entry is invalid: {error}"))?;
        if parsed.sequence <= previous {
            return Err("transferred ledger is not in sequence order".into());
        }
        previous = parsed.sequence;
        let expected = crate::db::task_store::ledger_entry_id(source_task_id, parsed.sequence);
        if parsed.entry_id() != Some(expected.as_str())
            || parsed.envelope.get("task_id").and_then(Value::as_str) != Some(source_task_id)
        {
            return Err(format!(
                "transferred ledger entry {} does not belong to task {source_task_id}",
                file.file_name
            ));
        }
    }
    if document.rows.ownership_generation < 1 {
        return Err("transferred task state has no ownership generation".into());
    }
    for reference in &document.history_refs {
        if !is_object_id(&reference.oid)
            || super::git::normalize_ref(Some(&reference.name)).is_none()
        {
            return Err("transferred task state names an invalid history ref".into());
        }
    }
    Ok(document)
}

pub fn verify_staged_file(bytes: &[u8], metadata: &TransferStagedFile) -> Result<(), String> {
    if super::payload::sha256_hex(bytes) != metadata.sha256 {
        return Err(format!(
            "transferred {} checksum does not match its payload",
            metadata.filename
        ));
    }
    Ok(())
}

/// Bring the carried history commits into the destination repository under
/// `refs/kanna/transfers/<transfer-id>/history/`, so every commit the ledger
/// names resolves here. Blocking; idempotent.
pub fn import_history_refs(
    repo_path: &Path,
    transfer_id: &str,
    document: &TaskStateDocument,
    bundle_path: Option<&Path>,
) -> Result<(), String> {
    let git = |args: &[&str]| super::git::git(repo_path, args);
    let destination_prefix = format!("refs/kanna/transfers/{transfer_id}/history/");
    if let Some(bundle_path) = bundle_path {
        let bundle = bundle_path
            .to_str()
            .ok_or_else(|| "history bundle path is not valid unicode".to_string())?;
        git(&["bundle", "verify", bundle])?;
        let refspec =
            format!("+{HISTORY_BUNDLE_REF_PREFIX}*:refs/kanna/transfers/{transfer_id}/incoming/*");
        git(&["fetch", "--no-tags", bundle, &refspec])?;
        if let Ok(names) = git(&[
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/kanna/transfers/{transfer_id}/incoming/"),
        ]) {
            for name in names.lines().filter(|name| !name.is_empty()) {
                let _ = git(&["update-ref", "-d", name]);
            }
        }
    }
    for reference in &document.history_refs {
        git(&[
            "update-ref",
            &format!("{destination_prefix}{}", reference.name),
            &reference.oid,
        ])
        .map_err(|error| {
            format!(
                "carried history commit {} ({}) is not available here: {error}",
                reference.oid, reference.name
            )
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Destination: the ledger
// ---------------------------------------------------------------------------

struct PlannedEntry {
    kind: LedgerEntryKind,
    source_entry_id: String,
    operation_id: Option<String>,
    recorded_at: Option<String>,
    declared_role: Option<String>,
    body: Value,
    message: Option<String>,
    artifacts: Value,
    origin: Value,
}

fn parsed_ledger(document: &TaskStateDocument) -> Result<Vec<LedgerFile>, String> {
    document
        .ledger
        .iter()
        .map(|file| task_store::parse_ledger_file(&file.file_name, file.content.as_bytes()))
        .collect()
}

/// The entries the destination re-records, in source order: every entry but
/// inputs (which the input ledger carries).
fn mirrored_files(files: &[LedgerFile]) -> Vec<&LedgerFile> {
    files
        .iter()
        .filter(|file| file.kind != LedgerEntryKind::Input)
        .collect()
}

/// The result that caused the source's session of `stage`, by source entry
/// id — what the destination session of that stage is told about.
fn source_trigger_id(files: &[LedgerFile], stage: &str) -> Option<String> {
    task_store::resolve_trigger_in(files, stage).map(|trigger| trigger.entry_id)
}

/// Where each mirrored entry lands at the destination, by source entry id:
/// the entries already recorded by an earlier attempt where they exist,
/// otherwise the sequences after the destination's current last one.
fn destination_ids(
    db: &Db,
    destination_task_id: &str,
    mirrored: &[&LedgerFile],
) -> Result<HashMap<String, String>, StateError> {
    let mut ids = HashMap::new();
    let mut last = db.last_ledger_sequence(destination_task_id)?;
    for file in mirrored {
        let source_id = file.entry_id().unwrap_or_default().to_string();
        let entry_id = match db.ledger_entry_for_source(
            destination_task_id,
            MIRRORED_SOURCE_KIND,
            &source_id,
        )? {
            Some(existing) => existing.entry_id,
            None => {
                last += 1;
                crate::db::task_store::ledger_entry_id(destination_task_id, last)
            }
        };
        ids.insert(source_id, entry_id);
    }
    Ok(ids)
}

/// A failure inside one of this module's write transactions.
struct StateError(String);

impl From<rusqlite::Error> for StateError {
    fn from(error: rusqlite::Error) -> Self {
        Self(format!("db error: {error}"))
    }
}

fn remap(value: &mut Value, key: &str, ids: &HashMap<String, String>) {
    if let Some(slot) = value.get_mut(key) {
        if let Some(mapped) = slot.as_str().and_then(|id| ids.get(id)) {
            *slot = Value::String(mapped.clone());
        }
    }
}

fn plan_entries(
    state: &ImportedTaskState,
    mirrored: &[&LedgerFile],
    ids: &HashMap<String, String>,
) -> Vec<PlannedEntry> {
    let document = &state.document;
    mirrored
        .iter()
        .map(|file| {
            let envelope = &file.envelope;
            let mut body = file.body().clone();
            match file.kind {
                LedgerEntryKind::Transition => remap(&mut body, "triggering_result_id", ids),
                LedgerEntryKind::Plan => remap(&mut body, "result_id", ids),
                _ => {}
            }
            let mut artifacts = envelope
                .get("artifacts")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let Some(references) = artifacts.as_object_mut() {
                for reference in references.values_mut() {
                    if reference.get("type").and_then(Value::as_str) == Some("stored")
                        && reference.get("repoId").and_then(Value::as_str)
                            == Some(document.source_repo_id.as_str())
                    {
                        reference["repoId"] = Value::String(state.destination_repo_id.clone());
                    }
                }
            }
            let source_entry_id = file.entry_id().unwrap_or_default().to_string();
            // First origin: an entry this source itself mirrored from an
            // earlier hop keeps naming where it was first recorded.
            let first_origin = envelope
                .pointer("/source/origin")
                .filter(|origin| origin.get("ledger_entry").is_some())
                .cloned();
            let origin = first_origin.unwrap_or_else(|| {
                json!({
                    "peer_id": document.source_peer_id,
                    "task_id": document.source_task_id,
                    "ledger_entry": source_entry_id,
                    "sequence": file.sequence,
                    "run_id": envelope.get("run_id"),
                    "session_ref": envelope.get("session_ref"),
                    "channel_identity": envelope.get("channel_identity"),
                    "source": envelope.get("source"),
                })
            });
            PlannedEntry {
                kind: file.kind,
                source_entry_id,
                operation_id: envelope
                    .get("operation_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                recorded_at: envelope
                    .get("recorded_at")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                declared_role: envelope
                    .get("declared_role")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                body,
                message: file.message.clone(),
                artifacts,
                origin,
            }
        })
        .collect()
}

/// The triggering result a destination session of the carried stage is
/// told about, before the entries are recorded — the preamble is prepared
/// before the task row exists to record them against. [`record_ledger`]
/// refuses to proceed if the entry does not land where this predicts.
pub fn predicted_trigger(
    state: &ImportedTaskState,
    destination_task_id: &str,
) -> Result<Option<TriggeringResult>, String> {
    let files = parsed_ledger(&state.document)?;
    let Some(trigger_id) = source_trigger_id(&files, &state.document.stage) else {
        return Ok(None);
    };
    let mirrored = mirrored_files(&files);
    let Some(position) = mirrored
        .iter()
        .position(|file| file.entry_id() == Some(trigger_id.as_str()))
    else {
        return Ok(None);
    };
    let sequence = position as i64 + 1;
    let file = mirrored[position];
    let mut trigger = TriggeringResult::from_file(file)
        .ok_or_else(|| "carried trigger is not a result".to_string())?;
    trigger.entry_id = crate::db::task_store::ledger_entry_id(destination_task_id, sequence);
    trigger.file = format!(
        "ledger/{}",
        crate::db::task_store::ledger_file_name(sequence, LedgerEntryKind::Result)
    );
    // The run named no run here; the preamble says so rather than naming a
    // run id this machine has no record of.
    trigger.run_id = None;
    Ok(Some(trigger))
}

/// Did the carried trigger land at the entry [`predicted_trigger`] named?
pub fn trigger_matches_prediction(
    state: &ImportedTaskState,
    destination_task_id: &str,
    ids: &HashMap<String, String>,
) -> bool {
    let Ok(files) = parsed_ledger(&state.document) else {
        return false;
    };
    let recorded = source_trigger_id(&files, &state.document.stage)
        .and_then(|source_id| ids.get(&source_id).cloned());
    let predicted = predicted_trigger(state, destination_task_id)
        .ok()
        .flatten()
        .map(|trigger| trigger.entry_id);
    recorded == predicted
}

/// Re-record the carried entries as historical entries of the destination
/// task (step 1 of the module docs) and return where each landed, by source
/// entry id. Idempotent. Must run before anything else records an entry for
/// a newly created destination task, so [`predicted_trigger`] holds.
pub fn record_ledger(
    db: &Db,
    destination_task_id: &str,
    state: &ImportedTaskState,
) -> Result<HashMap<String, String>, String> {
    let files = parsed_ledger(&state.document)?;
    let mirrored = mirrored_files(&files);
    db.with_immediate_transaction(|db| -> Result<HashMap<String, String>, StateError> {
        let ids = destination_ids(db, destination_task_id, &mirrored)?;
        for entry in plan_entries(state, &mirrored, &ids) {
            let expected = ids.get(&entry.source_entry_id).cloned().unwrap_or_default();
            let recorded = db.enqueue_ledger_entry_with_artifacts(
                NewLedgerEntry {
                    task_id: destination_task_id,
                    kind: entry.kind,
                    operation_id: entry.operation_id.as_deref(),
                    source_kind: MIRRORED_SOURCE_KIND,
                    source_id: &entry.source_entry_id,
                    source_origin: Some(entry.origin),
                    historical: true,
                    recorded_at: entry.recorded_at.as_deref(),
                    // The run belongs to the machine that ran it; its id
                    // stays in the origin, never as a local run here.
                    run_id: None,
                    declared_role: entry.declared_role.as_deref(),
                    // Verified where it happened, not here (T8's rule
                    // for imported history).
                    channel_identity: &ChannelIdentity::Unknown,
                    body: entry.body,
                    message: entry.message.as_deref(),
                    hold_events_after: None,
                    reserved_sequence: None,
                },
                Some(&entry.artifacts),
            )?;
            if recorded.entry_id != expected {
                return Err(StateError(format!(
                    "carried ledger entry {} landed at {} instead of {expected}",
                    entry.source_entry_id, recorded.entry_id
                )));
            }
        }
        Ok(ids)
    })
    .map_err(|StateError(error)| error)
}

/// Record the carried rows and the closing `transfer_import` transition
/// (step 3), after the inputs. Idempotent.
pub fn record_import(
    db: &Db,
    destination_task_id: &str,
    destination_branch: Option<&str>,
    state: &ImportedTaskState,
    ids: &HashMap<String, String>,
) -> Result<(), String> {
    let document = &state.document;
    let files = parsed_ledger(document)?;
    let trigger = source_trigger_id(&files, &document.stage)
        .and_then(|source_id| ids.get(&source_id).cloned());
    db.with_immediate_transaction(|db| -> Result<(), StateError> {
        db.import_carried_task_rows(
            &NewTransferredTaskState {
                task_id: destination_task_id,
                transfer_id: &state.transfer_id,
                source_peer_id: &document.source_peer_id,
                source_task_id: &document.source_task_id,
                ownership_generation: document.rows.ownership_generation,
                state_sha256: &state.sha256,
                links: &document.rows.links,
                session_start: state.session_start.label(),
                fresh_start_reason: state.session_start.fresh_start_reason(),
            },
            &document.rows,
            ids,
        )?;
        db.enqueue_ledger_entry(NewLedgerEntry {
            task_id: destination_task_id,
            kind: LedgerEntryKind::Transition,
            operation_id: None,
            source_kind: IMPORT_SOURCE_KIND,
            source_id: &state.transfer_id,
            source_origin: None,
            historical: false,
            recorded_at: None,
            run_id: None,
            declared_role: None,
            channel_identity: &ChannelIdentity::Server,
            body: json!({
                "from_stage": document.stage,
                "to_stage": document.stage,
                "branch": destination_branch,
                "trigger": "transfer",
                "operation": "transfer_import",
                "triggering_result_id": trigger,
                "exit": null,
                "exit_source": null,
                "transfer": {
                    "transfer_id": state.transfer_id,
                    "source_peer_id": document.source_peer_id,
                    "source_task_id": document.source_task_id,
                    "ownership_generation": document.rows.ownership_generation,
                    "state_sha256": state.sha256,
                    "carried_entries": ids.len(),
                    "session": state.session_start.label(),
                    "fresh_start_reason": state.session_start.fresh_start_reason(),
                },
            }),
            message: None,
            hold_events_after: None,
            reserved_sequence: None,
        })?;
        Ok(())
    })
    .map_err(|StateError(error)| error)
}

/// Keep the source's published files verbatim beside the destination's
/// ledger (step 2). Immutable, like the ledger: a retry that would write
/// different bytes fails.
pub fn archive(task_dir: &Path, state: &ImportedTaskState) -> Result<(), String> {
    let archive = archive_dir(task_dir, &state.transfer_id);
    if let Some(task_json) = &state.document.task_json {
        task_store::publish_immutable(&archive, "task.json", task_json.as_bytes())?;
    }
    let ledger = archive.join("ledger");
    for file in &state.document.ledger {
        task_store::publish_immutable(&ledger, &file.file_name, file.content.as_bytes())?;
    }
    Ok(())
}

pub fn archive_dir(task_dir: &Path, transfer_id: &str) -> PathBuf {
    task_dir.join("transferred").join(transfer_id)
}

/// Read back what the import recorded and check it is what was carried:
/// the state row, every mirrored entry on disk, and the closing transition.
pub fn verify(
    db: &Db,
    db_path: &str,
    destination_task_id: &str,
    state: &ImportedTaskState,
) -> Result<(), String> {
    let recorded = db
        .transferred_task_state(destination_task_id)
        .map_err(|error| format!("db error: {error}"))?
        .ok_or_else(|| "transferred task state was not recorded".to_string())?;
    if recorded.transfer_id != state.transfer_id
        || recorded.state_sha256 != state.sha256
        || recorded.ownership_generation != state.document.rows.ownership_generation
        || recorded.links != state.document.rows.links
    {
        return Err("recorded transferred task state differs from what was carried".into());
    }
    let task_dir = task_store::task_dir_for(db, db_path, destination_task_id)
        .ok_or_else(|| format!("task not found: {destination_task_id}"))?;
    let on_disk = task_store::read_ledger(&task_dir)?;
    let files = parsed_ledger(&state.document)?;
    for source in mirrored_files(&files) {
        let source_id = source.entry_id().unwrap_or_default();
        let mirrored = on_disk.iter().find(|file| {
            file.envelope
                .pointer("/source/kind")
                .and_then(Value::as_str)
                == Some(MIRRORED_SOURCE_KIND)
                && file.envelope.pointer("/source/id").and_then(Value::as_str) == Some(source_id)
        });
        match mirrored {
            Some(file) if file.kind == source.kind && file.message == source.message => {}
            _ => {
                return Err(format!(
                    "carried ledger entry {source_id} is not on disk at the destination"
                ))
            }
        }
    }
    let closed = on_disk.iter().any(|file| {
        file.envelope
            .pointer("/source/kind")
            .and_then(Value::as_str)
            == Some(IMPORT_SOURCE_KIND)
            && file.envelope.pointer("/source/id").and_then(Value::as_str)
                == Some(state.transfer_id.as_str())
    });
    if !closed {
        return Err("the transfer's import transition is not on disk".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "task_state_tests.rs"]
mod tests;
