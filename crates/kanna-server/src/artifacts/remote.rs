//! Sharing artifacts through an artifact remote (spec §8 sharing, §16.9).
//!
//! An artifact remote is an ordinary Git remote that two Kanna homes can
//! both reach. Sending is pushing one artifact there and telling the other
//! person its id outside Kanna; receiving is fetching that id and opening it.
//! The `git` executable does the transport, so whatever SSH keys, agents and
//! credential helpers the user already has are what authenticate; Kanna
//! stores no credential of its own.
//!
//! What goes to the remote, and only this, under `refs/kanna/artifacts/shared/`:
//!
//! - `content/<tree-id>/<commit-id>`: the parentless commit that retains one
//!   artifact tree. The name is the manifest a receiver needs: holding only
//!   the tree id, it fetches `content/<tree-id>/*` and gets every commit any
//!   home retained that tree with. Two homes that published identical bytes
//!   hold two commits for one tree, which become two names, never a conflict.
//! - `records/<tree-id>/<versions|comments|decisions>/<record-id>`: one
//!   record, as a parentless commit whose tree is a single `record.json` blob
//!   holding the exact bytes the recording home stored. The commit is built
//!   deterministically from that blob, so every home that holds a record
//!   pushes the same object under the same name.
//!
//! Every name is written once and never forced. Concurrent comments from two
//! homes are two names, so pushing one can neither overwrite nor reject the
//! other. The local metadata history and local content refs are never
//! pushed, and neither is anything outside the artifact repository: no task
//! directory, transcript, environment or credential exists in the objects a
//! push names.
//!
//! Received objects are data. Fetching lands them under a private
//! `incoming/` namespace, each ref is validated (shape, bounds, record
//! schema, canonical record commit), and only then imported into the local
//! refs by [`ArtifactStore::import`]. A received decision is a record like
//! any other: nothing here reads one, and nothing reads one to move a task.
//!
//! Both directions hold the repository lock for their whole window, network
//! transfer included. Retention (`ArtifactStore::sweep_retention`) deletes
//! content refs and then prunes every unreachable object under that same
//! lock, and both directions have objects no local ref protects: a fetch's
//! objects before `git fetch` writes its `incoming/` refs and again between
//! validation and import, and a push's canonical record commits, built for
//! the push and never referenced locally. Holding the lock also means a push
//! never sends content retention collected while it was running.

use super::store::{
    declared_text, kanna_signature, normalize_artifact_path, parse_object_id, storage,
    ArtifactStore, PublishLimits, StoredRecord, COMMENTS_DIR, DECISIONS_DIR, FILE_MODE_BLOB,
    FILE_MODE_TREE, MAX_ANCHOR_EXCERPT_BYTES, MAX_ANCHOR_POSITION_BYTES, MAX_DECLARED_NAME_BYTES,
    MAX_TEXT_BYTES, RECORD_DIRS, VERSIONS_DIR,
};
use super::types::{
    ArtifactComment, ArtifactDecision, ArtifactFetchOutcome, ArtifactPushOutcome,
    ArtifactRefusedRef, ArtifactVersion, ARTIFACT_RECORD_SCHEMA_VERSION,
};
use super::{random_hex, ArtifactError};
use git2::{ObjectType, Oid, Repository, Signature, Time, Tree};
use std::collections::{BTreeSet, VecDeque};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) const SHARED_PREFIX: &str = "refs/kanna/artifacts/shared/";
const INCOMING_PREFIX: &str = super::store::INCOMING_REF_PREFIX;
const RECORD_FILE_NAME: &str = "record.json";
/// A record blob larger than this is refused unread. Generous: the largest
/// valid record is a comment of [`MAX_TEXT_BYTES`] plus a bounded anchor.
const MAX_RECORD_BYTES: usize = 256 * 1024;
/// How many `previous` links one push or fetch follows.
const MAX_CHAIN: usize = 256;
/// Refspecs per `git push`, to stay far below any argv limit.
const PUSH_BATCH: usize = 200;
const GIT_TIMEOUT: Duration = Duration::from_secs(300);

/// Test-only pause points inside the windows where objects exist without a
/// local ref: a test parks a push or fetch there and proves that a retention
/// sweep cannot run until it moves on.
#[cfg(test)]
pub(crate) mod pause {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{LazyLock, Mutex};

    type Key = (PathBuf, &'static str);
    /// Signals the pause was reached; waits to be released.
    type Armed = (Sender<()>, Receiver<()>);
    static ARMED: LazyLock<Mutex<HashMap<Key, Armed>>> = LazyLock::new(Default::default);

    /// Arm `point` for the repository at `path`, once. Returns a receiver
    /// that fires when the operation reaches it, and a sender that lets it go.
    pub(crate) fn arm(path: &Path, point: &'static str) -> (Receiver<()>, Sender<()>) {
        let (reached, on_reach) = channel();
        let (release, on_release) = channel();
        ARMED
            .lock()
            .unwrap()
            .insert((path.to_path_buf(), point), (reached, on_release));
        (on_reach, release)
    }

    pub(super) fn at(path: &Path, point: &'static str) {
        let armed = ARMED.lock().unwrap().remove(&(path.to_path_buf(), point));
        if let Some((reached, release)) = armed {
            let _ = reached.send(());
            let _ = release.recv();
        }
    }
}

#[cfg(test)]
fn pause_at(store: &ArtifactStore, point: &'static str) {
    pause::at(store.path(), point);
}

#[cfg(not(test))]
fn pause_at(_store: &ArtifactStore, _point: &'static str) {}

/// A validated `artifacts.remote`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactRemote {
    url: String,
    display: String,
}

impl ArtifactRemote {
    /// Accept an absolute or `~/` local path, a `file://`, `ssh://`,
    /// `git+ssh://`, `git://`, `http://` or `https://` URL, or scp-style
    /// `[user@]host:path`. Refused: anything that git would read as an option
    /// or hand to a remote helper (`ext::`, `fd::`, `<scheme>::`), relative
    /// paths, and the artifact repository itself.
    pub(crate) fn parse(
        configured: &str,
        home: &Path,
        store_path: &Path,
    ) -> Result<Self, ArtifactError> {
        let value = configured.trim();
        let invalid = |reason: &str| {
            ArtifactError::InvalidRemote(format!(
                "artifacts.remote {:?} is not usable: {reason}",
                redact(value)
            ))
        };
        if value.is_empty() {
            return Err(invalid("it is empty"));
        }
        if value.chars().any(char::is_control) {
            return Err(invalid("it contains control characters"));
        }
        if value.starts_with('-') {
            return Err(invalid("it would be read as a git option"));
        }
        if let Some((helper, _)) = value.split_once("::") {
            if !helper.is_empty()
                && helper
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+.-_".contains(&byte))
            {
                return Err(invalid("remote helpers (`<transport>::`) are not allowed"));
            }
        }
        let url = if let Some((scheme, _)) = value.split_once("://") {
            match scheme.to_ascii_lowercase().as_str() {
                "file" | "ssh" | "git+ssh" | "ssh+git" | "git" | "http" | "https" => {}
                _ => return Err(invalid("use a file, ssh, git, http or https URL")),
            }
            value.to_string()
        } else if let Some(rest) = value.strip_prefix("~/") {
            home.join(rest).to_string_lossy().into_owned()
        } else if Path::new(value).is_absolute() {
            value.to_string()
        } else if value
            .split_once(':')
            .is_some_and(|(host, _)| !host.is_empty() && !host.contains('/'))
        {
            // scp-like syntax: git reads `host:path` as SSH when no slash
            // precedes the first colon.
            value.to_string()
        } else {
            return Err(invalid("a local path must be absolute or start with ~/"));
        };
        if Path::new(&url).is_absolute() {
            let same = |a: &Path, b: &Path| {
                std::fs::canonicalize(a)
                    .ok()
                    .zip(std::fs::canonicalize(b).ok())
                    .is_some_and(|(a, b)| a == b)
            };
            if same(Path::new(&url), store_path) {
                return Err(invalid("it is this artifact repository itself"));
            }
        }
        Ok(Self {
            display: redact(&url),
            url,
        })
    }

    /// The remote as it may be shown: URL credentials removed.
    pub(crate) fn display(&self) -> &str {
        &self.display
    }
}

/// Drop `user:password@` from a URL. scp-style `user@host:path` keeps its
/// user name, which is not a secret.
///
/// A password may contain '/' and '@', so the authority cannot be found by
/// the first '/'. Any '@' after `://` makes everything from there through
/// the last '@' before the first '/' that follows the first '@' credentials.
/// That over-redacts a URL whose path contains '@' (`https://host/p@x` shows
/// as `https://***@x`), which is the safe side: an ambiguous
/// credential-bearing value is never echoed.
fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let Some(first_at) = rest.find('@') else {
        return url.to_string();
    };
    let slash = rest[first_at..]
        .find('/')
        .map_or(rest.len(), |offset| first_at + offset);
    let last_at = rest[..slash].rfind('@').unwrap_or(first_at);
    format!("{scheme}://***@{}", &rest[last_at + 1..])
}

// ---------------------------------------------------------------------------
// Push
// ---------------------------------------------------------------------------

/// Push one artifact, every earlier version its `previous` links reach in
/// this repository, and every record about any of them.
pub(crate) fn push(
    store: &ArtifactStore,
    remote: &ArtifactRemote,
    artifact_id: &str,
) -> Result<ArtifactPushOutcome, ArtifactError> {
    // For the whole push: the record commits built below are unreferenced
    // until the remote holds them, and no content may expire mid-push.
    let _lock = store.lock()?;
    // `detail` reports a malformed, foreign-typed or unknown id exactly as a
    // local read would.
    store.detail(artifact_id)?;
    let requested = parse_object_id(artifact_id)?;
    if store.retained_commit(requested)?.is_none() {
        return Err(ArtifactError::ContentMissing {
            repo_id: store.repo_id().to_string(),
            artifact_id: artifact_id.to_string(),
        });
    }
    let chain = local_chain(store, remote, requested)?;
    let repository = store.repository();
    // (commit this push names, remote ref it names it under)
    let mut updates: Vec<(Oid, String)> = Vec::new();
    for &artifact in &chain {
        if let Some(commit) = store.retained_commit(artifact)? {
            updates.push((commit, shared_content_ref(artifact, commit)));
        }
        for record in store.stored_records(artifact)? {
            let commit = record_commit(repository, &record)?;
            updates.push((commit, shared_record_ref(&record)));
        }
    }

    // The record commits above exist only as unreferenced objects now.
    pause_at(store, "push-before-send");
    let mut created = Vec::new();
    let mut up_to_date = 0;
    // (remote ref, git's reason)
    let mut rejected: Vec<(String, String)> = Vec::new();
    for batch in updates.chunks(PUSH_BATCH) {
        let mut args = vec![
            "push".to_string(),
            "--porcelain".to_string(),
            "--no-verify".to_string(),
            remote.url.clone(),
        ];
        args.extend(
            batch
                .iter()
                .map(|(commit, name)| format!("{commit}:{name}")),
        );
        let output = run_git(store.path(), remote, &args)?;
        let mut reported = 0;
        for line in output.stdout.lines() {
            let mut fields = line.splitn(3, '\t');
            let (Some(flag), Some(refs)) = (fields.next(), fields.next()) else {
                continue;
            };
            let summary = fields.next().unwrap_or_default();
            let Some((_, destination)) = refs.split_once(':') else {
                continue;
            };
            reported += 1;
            match flag {
                "*" => created.push(destination.to_string()),
                "=" => up_to_date += 1,
                "!" => rejected.push((destination.to_string(), summary.to_string())),
                // A fast-forward, forced update or deletion of an immutable
                // name is never requested; report it rather than trust it.
                _ => rejected.push((
                    destination.to_string(),
                    format!("unexpected update {flag:?}"),
                )),
            }
        }
        if !output.success && reported == 0 {
            return Err(remote_failure(remote, "push", &output.stderr));
        }
    }
    // A creation loses a race when another push (from another home, or an
    // overlapping request on this one) creates the same name between ref
    // advertisement and update. The remote then holds exactly the commit
    // this push named, which is success, not a conflict.
    if !rejected.is_empty() {
        let held = remote_ref_values(store, remote, &rejected)?;
        rejected.retain(|(name, _)| {
            let expected = updates
                .iter()
                .find(|(_, update)| update == name)
                .map(|(commit, _)| *commit);
            if expected.is_some()
                && held
                    .iter()
                    .any(|(value, held_name)| held_name == name && Some(*value) == expected)
            {
                up_to_date += 1;
                false
            } else {
                true
            }
        });
    }
    if !rejected.is_empty() {
        return Err(ArtifactError::RemoteConflict {
            remote: remote.display.clone(),
            refs: rejected
                .into_iter()
                .map(|(name, reason)| format!("{name} ({reason})"))
                .collect(),
        });
    }
    Ok(ArtifactPushOutcome {
        remote: remote.display.clone(),
        artifact_id: requested.to_string(),
        artifact_ids: chain.iter().map(Oid::to_string).collect(),
        created_refs: created,
        up_to_date_refs: up_to_date,
    })
}

/// What the remote holds now under each of these names.
fn remote_ref_values(
    store: &ArtifactStore,
    remote: &ArtifactRemote,
    names: &[(String, String)],
) -> Result<Vec<(Oid, String)>, ArtifactError> {
    let mut held = Vec::new();
    for batch in names.chunks(PUSH_BATCH) {
        let mut args = vec![
            "ls-remote".to_string(),
            "--refs".to_string(),
            remote.url.clone(),
        ];
        args.extend(batch.iter().map(|(name, _)| name.clone()));
        let output = run_git(store.path(), remote, &args)?;
        if !output.success {
            return Err(remote_failure(remote, "ls-remote", &output.stderr));
        }
        for line in output.stdout.lines() {
            let Some((value, name)) = line.split_once('\t') else {
                continue;
            };
            if let Ok(value) = Oid::from_str(value) {
                held.push((value, name.to_string()));
            }
        }
    }
    Ok(held)
}

/// The id and every earlier version reachable through `previous` links that
/// is published here, breadth first.
///
/// A `previous` link is followed freely only when a version record this home
/// wrote itself names it. A link that only a record received from a remote
/// supplies is followed only when the remote already holds valid content for
/// that tree: otherwise a peer could plant a version record whose `previous`
/// names a tree this home never shared, and the next reply push would send
/// it.
fn local_chain(
    store: &ArtifactStore,
    remote: &ArtifactRemote,
    start: Oid,
) -> Result<Vec<Oid>, ArtifactError> {
    let mut chain = vec![start];
    let mut index = 0;
    while index < chain.len() && chain.len() < MAX_CHAIN {
        for (previous, own) in store.previous_links_with_provenance(chain[index])? {
            if chain.contains(&previous) || !store.is_published(previous)? {
                continue;
            }
            if own || !fetch_one(store, remote, previous)?.content.is_empty() {
                chain.push(previous);
            }
        }
        index += 1;
    }
    Ok(chain)
}

fn shared_content_ref(tree: Oid, commit: Oid) -> String {
    format!("{SHARED_PREFIX}content/{tree}/{commit}")
}

fn shared_record_ref(record: &StoredRecord) -> String {
    format!(
        "{SHARED_PREFIX}records/{}/{}/{}",
        record.artifact, record.directory, record.record_id
    )
}

/// The canonical commit carrying one record: fixed identity and time, a
/// single `record.json` entry, no parent. Any home holding the same record
/// bytes builds the same commit id.
fn record_commit(repository: &Repository, record: &StoredRecord) -> Result<Oid, ArtifactError> {
    let mut builder = repository.treebuilder(None).map_err(storage)?;
    builder
        .insert(RECORD_FILE_NAME, record.blob, FILE_MODE_BLOB)
        .map_err(storage)?;
    let tree = repository
        .find_tree(builder.write().map_err(storage)?)
        .map_err(storage)?;
    let identity = kanna_signature()?;
    let signature = Signature::new(
        identity.name().unwrap_or("Kanna"),
        identity.email().unwrap_or("kanna@localhost"),
        &Time::new(0, 0),
    )
    .map_err(storage)?;
    repository
        .commit(
            None,
            &signature,
            &signature,
            &format!(
                "kanna artifact {} {} {}\n",
                record.directory, record.artifact, record.record_id
            ),
            &tree,
            &[],
        )
        .map_err(storage)
}

// ---------------------------------------------------------------------------
// Fetch
// ---------------------------------------------------------------------------

/// What one id's fetch delivered, after validation.
#[derive(Default)]
struct Received {
    any_refs: bool,
    content: Vec<(Oid, Oid)>,
    records: Vec<StoredRecord>,
    refused: Vec<ArtifactRefusedRef>,
}

/// Fetch one artifact by tree id alone, then every earlier version its
/// `previous` links name, importing their content and records.
pub(crate) fn fetch(
    store: &ArtifactStore,
    remote: &ArtifactRemote,
    artifact_id: &str,
) -> Result<ArtifactFetchOutcome, ArtifactError> {
    let requested = parse_object_id(artifact_id)?;
    let mut queue = VecDeque::from([requested]);
    let mut visited = BTreeSet::new();
    let mut fetched = Vec::new();
    let mut content_retained = Vec::new();
    let mut records_imported = 0;
    let mut refused = Vec::new();
    let mut missing = Vec::new();

    while let Some(artifact) = queue.pop_front() {
        if !visited.insert(artifact) || visited.len() > MAX_CHAIN {
            continue;
        }
        // From before `git fetch` writes the first object until the import
        // has given every accepted one a local ref: nothing in between may
        // be pruned by a retention sweep.
        let _lock = store.lock()?;
        let received = fetch_one(store, remote, artifact)?;
        if artifact == requested {
            if !received.any_refs {
                return Err(ArtifactError::NotOnRemote {
                    remote: remote.display.clone(),
                    artifact_id: artifact.to_string(),
                });
            }
            if received.content.is_empty() && store.retained_commit(artifact)?.is_none() {
                let reasons = received
                    .refused
                    .iter()
                    .map(|refusal| format!("{}: {}", refusal.ref_name, refusal.reason))
                    .collect::<Vec<_>>();
                return Err(if reasons.is_empty() {
                    ArtifactError::NotOnRemote {
                        remote: remote.display.clone(),
                        artifact_id: artifact.to_string(),
                    }
                } else {
                    ArtifactError::RemoteFailed(format!(
                        "artifact remote {} offered content for {artifact} that was refused: {}",
                        remote.display,
                        reasons.join("; ")
                    ))
                });
            }
        }
        refused.extend(received.refused);
        // The private namespace is gone: nothing but the lock protects what
        // arrived until the import below gives it local refs.
        pause_at(store, "fetch-before-import");
        if received.any_refs {
            let report = store.import(&received.content, &received.records)?;
            fetched.push(artifact.to_string());
            content_retained.extend(report.content_retained.iter().map(Oid::to_string));
            records_imported += report.records_imported;
            refused.extend(report.conflicting.iter().map(|record| ArtifactRefusedRef {
                ref_name: shared_record_ref(record),
                reason: "a local record with this id holds different bytes; the local record was kept"
                    .to_string(),
            }));
        }
        if store.retained_commit(artifact)?.is_none() {
            missing.push(artifact.to_string());
        }
        for previous in store.previous_links(artifact)? {
            if !visited.contains(&previous) {
                queue.push_back(previous);
            }
        }
    }

    Ok(ArtifactFetchOutcome {
        remote: remote.display.clone(),
        artifact_id: requested.to_string(),
        fetched,
        content_retained,
        records_imported,
        refused,
        missing,
        detail: store.detail(&requested.to_string())?,
    })
}

/// Fetch `content/<id>/*` and `records/<id>/*` into a private namespace,
/// validate every ref that arrived, and remove the namespace again. The
/// caller holds the repository lock until it has imported what it keeps:
/// once the namespace is gone nothing else protects those objects.
fn fetch_one(
    store: &ArtifactStore,
    remote: &ArtifactRemote,
    artifact: Oid,
) -> Result<Received, ArtifactError> {
    let nonce = random_hex(8).map_err(ArtifactError::Storage)?;
    let incoming = format!("{INCOMING_PREFIX}{nonce}/");
    let _cleanup = IncomingRefs {
        repository: store.repository(),
        prefix: incoming.clone(),
    };
    let args = vec![
        "-c".to_string(),
        "transfer.fsckObjects=true".to_string(),
        "fetch".to_string(),
        "--no-tags".to_string(),
        "--no-write-fetch-head".to_string(),
        "--no-auto-maintenance".to_string(),
        "--quiet".to_string(),
        remote.url.clone(),
        format!("{SHARED_PREFIX}content/{artifact}/*:{incoming}content/{artifact}/*"),
        format!("{SHARED_PREFIX}records/{artifact}/*:{incoming}records/{artifact}/*"),
    ];
    let output = run_git(store.path(), remote, &args)?;
    if !output.success {
        return Err(remote_failure(remote, "fetch", &output.stderr));
    }

    let repository = store.repository();
    let mut received = Received::default();
    let references = repository
        .references_glob(&format!("{incoming}*"))
        .map_err(storage)?;
    for reference in references {
        let reference = reference.map_err(storage)?;
        let Some(name) = reference.name() else {
            continue;
        };
        let Some(relative) = name.strip_prefix(&incoming) else {
            continue;
        };
        received.any_refs = true;
        let shared_name = format!("{SHARED_PREFIX}{relative}");
        let Some(target) = reference.target() else {
            received
                .refused
                .push(refusal(shared_name, "not a direct ref"));
            continue;
        };
        let parts = relative.split('/').collect::<Vec<_>>();
        let result = match parts.as_slice() {
            ["content", tree, commit] => {
                validate_content(repository, artifact, tree, commit, target)
                    .map(|pair| received.content.push(pair))
            }
            ["records", about, directory, record_id] => {
                validate_record(repository, artifact, about, directory, record_id, target)
                    .map(|record| received.records.push(record))
            }
            _ => Err("not a Kanna artifact ref".to_string()),
        };
        if let Err(reason) = result {
            received.refused.push(refusal(shared_name, &reason));
        }
    }
    Ok(received)
}

fn refusal(ref_name: String, reason: &str) -> ArtifactRefusedRef {
    ArtifactRefusedRef {
        ref_name,
        reason: reason.to_string(),
    }
}

/// Removes a fetch's private ref namespace, whatever happened.
struct IncomingRefs<'a> {
    repository: &'a Repository,
    prefix: String,
}

impl Drop for IncomingRefs<'_> {
    fn drop(&mut self) {
        let Ok(references) = self
            .repository
            .references_glob(&format!("{}*", self.prefix))
        else {
            return;
        };
        let names = references
            .flatten()
            .filter_map(|reference| reference.name().map(str::to_string))
            .collect::<Vec<_>>();
        for name in names {
            if let Ok(mut reference) = self.repository.find_reference(&name) {
                let _ = reference.delete();
            }
        }
    }
}

fn validate_content(
    repository: &Repository,
    artifact: Oid,
    tree: &str,
    commit: &str,
    target: Oid,
) -> Result<(Oid, Oid), String> {
    let tree = parse_object_id(tree).map_err(|error| error.to_string())?;
    let commit = parse_object_id(commit).map_err(|error| error.to_string())?;
    if tree != artifact {
        return Err(format!("names tree {tree}, not the requested {artifact}"));
    }
    if target != commit {
        return Err(format!(
            "points at {target}, not the commit its name claims"
        ));
    }
    let object = repository
        .find_commit(commit)
        .map_err(|_| "does not point at a commit".to_string())?;
    if object.parent_count() != 0 {
        return Err("the retaining commit has parents".to_string());
    }
    if object.tree_id() != tree {
        return Err(format!("the commit retains tree {}", object.tree_id()));
    }
    let root = repository
        .find_tree(tree)
        .map_err(|_| "the tree is not present".to_string())?;
    let mut budget = TreeBudget {
        limits: PublishLimits::default(),
        files: 0,
        bytes: 0,
    };
    check_tree(repository, &root, 0, &mut budget)?;
    Ok((tree, commit))
}

struct TreeBudget {
    limits: PublishLimits,
    files: usize,
    bytes: u64,
}

/// Received content must be something Kanna could have published: regular
/// files and directories only, within the publication limits.
fn check_tree(
    repository: &Repository,
    tree: &Tree<'_>,
    depth: usize,
    budget: &mut TreeBudget,
) -> Result<(), String> {
    if depth >= budget.limits.max_depth {
        return Err(format!(
            "nested deeper than {} directories",
            budget.limits.max_depth
        ));
    }
    for entry in tree.iter() {
        let name = entry
            .name()
            .ok_or_else(|| "has a non-UTF-8 file name".to_string())?;
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.eq_ignore_ascii_case(".git")
            || name.contains(['/', '\\', '\0'])
        {
            return Err(format!("has an unsafe entry name {name:?}"));
        }
        match (entry.filemode(), entry.kind()) {
            (FILE_MODE_TREE, Some(ObjectType::Tree)) => {
                let child = repository
                    .find_tree(entry.id())
                    .map_err(|_| format!("directory {name} is missing"))?;
                check_tree(repository, &child, depth + 1, budget)?;
            }
            (FILE_MODE_BLOB | 0o100_755, Some(ObjectType::Blob)) => {
                let size = repository
                    .find_blob(entry.id())
                    .map_err(|_| format!("file {name} is missing"))?
                    .size() as u64;
                budget.files += 1;
                budget.bytes += size;
                if size > budget.limits.max_file_bytes
                    || budget.files > budget.limits.max_files
                    || budget.bytes > budget.limits.max_total_bytes
                {
                    return Err("exceeds the artifact size limits".to_string());
                }
            }
            (mode, _) => {
                return Err(format!(
                    "{name} has mode {mode:o}; only regular files and directories are accepted"
                ))
            }
        }
    }
    Ok(())
}

fn validate_record(
    repository: &Repository,
    artifact: Oid,
    about: &str,
    directory: &str,
    record_id: &str,
    target: Oid,
) -> Result<StoredRecord, String> {
    let about = parse_object_id(about).map_err(|error| error.to_string())?;
    if about != artifact {
        return Err(format!("is about {about}, not the requested {artifact}"));
    }
    let directory = RECORD_DIRS
        .into_iter()
        .find(|known| *known == directory)
        .ok_or_else(|| format!("{directory:?} is not a record kind"))?;
    if !is_record_id(record_id) {
        return Err(format!("{record_id:?} is not a record id"));
    }
    let commit = repository
        .find_commit(target)
        .map_err(|_| "does not point at a commit".to_string())?;
    let tree = commit
        .tree()
        .map_err(|_| "the record tree is missing".to_string())?;
    let entry = match (tree.len(), tree.get_name(RECORD_FILE_NAME)) {
        (1, Some(entry))
            if entry.filemode() == FILE_MODE_BLOB && entry.kind() == Some(ObjectType::Blob) =>
        {
            entry
        }
        _ => {
            return Err(format!(
                "a record commit holds exactly one {RECORD_FILE_NAME}"
            ))
        }
    };
    let blob = repository
        .find_blob(entry.id())
        .map_err(|_| "the record blob is missing".to_string())?;
    if blob.size() > MAX_RECORD_BYTES {
        return Err(format!("the record exceeds {MAX_RECORD_BYTES} bytes"));
    }
    check_record(directory, blob.content(), artifact, record_id)?;
    let record = StoredRecord {
        directory,
        artifact,
        record_id: record_id.to_string(),
        blob: blob.id(),
    };
    // A record name is immutable only if every home builds the same commit
    // for it; anything else would later collide with this home's own push.
    let canonical = record_commit(repository, &record).map_err(|error| error.to_string())?;
    if canonical != target {
        return Err("not a canonical Kanna record commit".to_string());
    }
    Ok(record)
}

/// `<13-digit milliseconds>-<16 lowercase hex>`, as `new_record_identity`
/// writes them.
fn is_record_id(value: &str) -> bool {
    let Some((order, suffix)) = value.split_once('-') else {
        return false;
    };
    // `<13-digit millis>` (T6 increment 1) or `s<12-digit sequence>` (the
    // persisted record sequence), then 16 lowercase hex.
    let order_ok = match order.strip_prefix('s') {
        Some(sequence) => {
            sequence.len() == 12 && sequence.bytes().all(|byte| byte.is_ascii_digit())
        }
        None => order.len() == 13 && order.bytes().all(|byte| byte.is_ascii_digit()),
    };
    order_ok
        && suffix.len() == 16
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A received record must parse as its kind, be about the tree its name
/// says, carry the id its name says, and respect the bounds a local write
/// enforces.
fn check_record(
    directory: &str,
    bytes: &[u8],
    artifact: Oid,
    record_id: &str,
) -> Result<(), String> {
    let artifact = artifact.to_string();
    let unreadable = |error: serde_json::Error| format!("unreadable record: {error}");
    let (schema_version, id, about) = match directory {
        VERSIONS_DIR => {
            let version: ArtifactVersion = serde_json::from_slice(bytes).map_err(unreadable)?;
            if let Some(previous) = &version.previous {
                parse_object_id(previous).map_err(|error| error.to_string())?;
            }
            if let Some(entrypoint) = &version.entrypoint {
                normalize_artifact_path(entrypoint).map_err(|error| error.to_string())?;
            }
            bounded("repoId", &version.repo_id, MAX_DECLARED_NAME_BYTES)?;
            bounded(
                "producedBy.taskId",
                &version.produced_by.task_id,
                MAX_DECLARED_NAME_BYTES,
            )?;
            bounded(
                "storage.ref",
                &version.storage.ref_name,
                MAX_DECLARED_NAME_BYTES,
            )?;
            bounded(
                "storage.commit",
                &version.storage.commit,
                MAX_DECLARED_NAME_BYTES,
            )?;
            (
                version.schema_version,
                version.record_id,
                version.artifact_id,
            )
        }
        COMMENTS_DIR => {
            let comment: ArtifactComment = serde_json::from_slice(bytes).map_err(unreadable)?;
            declared(&comment.author, "author", MAX_DECLARED_NAME_BYTES)?;
            declared(&comment.body, "body", MAX_TEXT_BYTES)?;
            bounded("repoId", &comment.repo_id, MAX_DECLARED_NAME_BYTES)?;
            if let Some(anchor) = &comment.anchor {
                if let Some(path) = &anchor.path {
                    normalize_artifact_path(path).map_err(|error| error.to_string())?;
                }
                if let Some(position) = &anchor.position {
                    bounded("anchor.position", position, MAX_ANCHOR_POSITION_BYTES)?;
                }
                if let Some(excerpt) = &anchor.excerpt {
                    bounded("anchor.excerpt", excerpt, MAX_ANCHOR_EXCERPT_BYTES)?;
                }
            }
            (
                comment.schema_version,
                comment.record_id,
                comment.about_artifact_id,
            )
        }
        DECISIONS_DIR => {
            let decision: ArtifactDecision = serde_json::from_slice(bytes).map_err(unreadable)?;
            declared(&decision.who, "who", MAX_DECLARED_NAME_BYTES)?;
            declared(&decision.what, "what", MAX_TEXT_BYTES)?;
            bounded("repoId", &decision.repo_id, MAX_DECLARED_NAME_BYTES)?;
            (
                decision.schema_version,
                decision.record_id,
                decision.about_artifact_id,
            )
        }
        other => return Err(format!("{other:?} is not a record kind")),
    };
    if schema_version == 0 || schema_version > ARTIFACT_RECORD_SCHEMA_VERSION {
        return Err(format!("record schema {schema_version} is not supported"));
    }
    if id != record_id {
        return Err(format!("the record calls itself {id:?}"));
    }
    if about != artifact {
        return Err(format!("the record is about {about}"));
    }
    Ok(())
}

fn declared(value: &str, name: &str, limit: usize) -> Result<(), String> {
    declared_text(name, value, limit)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn bounded(name: &str, value: &str, limit: usize) -> Result<(), String> {
    if value.len() > limit {
        return Err(format!("{name} exceeds {limit} bytes"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Running git
// ---------------------------------------------------------------------------

pub(super) struct GitOutput {
    pub(super) success: bool,
    pub(super) stdout: String,
    pub(super) stderr: String,
}

/// Run `git` against the artifact repository with the user's own transport
/// configuration, and nothing that could make it act on another repository,
/// prompt, or run hooks.
fn run_git(
    git_dir: &Path,
    remote: &ArtifactRemote,
    args: &[String],
) -> Result<GitOutput, ArtifactError> {
    run_git_bounded(Path::new("git"), git_dir, remote, args, GIT_TIMEOUT)
}

/// How long output may keep arriving after git itself has exited. Git has
/// written everything it reports by then; anything still holding the pipes
/// is a descendant (ssh, a credential helper, an askpass program).
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// [`run_git`] with the program and time limit injectable for fault tests.
/// Git runs in a process group of its own. Nothing here waits past
/// `timeout`: on expiry the whole group is killed, and output pipes a
/// descendant keeps open are abandoned rather than joined.
pub(super) fn run_git_bounded(
    program: &Path,
    git_dir: &Path,
    remote: &ArtifactRemote,
    args: &[String],
    timeout: Duration,
) -> Result<GitOutput, ArtifactError> {
    use std::os::unix::process::CommandExt as _;

    let deadline = Instant::now() + timeout;
    let mut command = Command::new(program);
    command
        .arg("--git-dir")
        .arg(git_dir)
        .args(["-c", "protocol.ext.allow=never"])
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .current_dir(git_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        // The server inherits the environment of whatever launched it; a
        // nested git must address only the artifact repository.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_NAMESPACE")
        .env("GIT_TERMINAL_PROMPT", "0");
    let mut child = command.spawn().map_err(|error| {
        ArtifactError::RemoteFailed(format!("cannot run git for the artifact remote: {error}"))
    })?;
    // The group id is git's pid; it stays reserved while any member lives.
    let group = child.id() as libc::pid_t;
    let kill_group = || {
        // SAFETY: plain syscall on a process group this function created.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    };
    let (finished, drained) = std::sync::mpsc::channel();
    let stdout = drain(child.stdout.take(), finished.clone());
    let stderr = drain(child.stderr.take(), finished);
    let timed_out = || {
        ArtifactError::RemoteFailed(format!(
            "git did not finish with artifact remote {} within {} seconds",
            remote.display,
            timeout.as_secs_f32()
        ))
    };

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                kill_group();
                let _ = child.wait();
                return Err(timed_out());
            }
            Err(error) => {
                kill_group();
                let _ = child.wait();
                return Err(ArtifactError::RemoteFailed(format!(
                    "cannot wait for git: {error}"
                )));
            }
        }
    };

    // Git has exited. Collect its output until both pipes close, for at most
    // the grace period and never past the deadline.
    let drain_deadline = deadline.min(Instant::now() + DRAIN_GRACE);
    let mut open = 2;
    while open > 0 {
        let remaining = drain_deadline.saturating_duration_since(Instant::now());
        if drained.recv_timeout(remaining).is_err() {
            break;
        }
        open -= 1;
    }
    if open > 0 {
        // A descendant still holds a pipe: end it, and keep what git wrote.
        kill_group();
        if Instant::now() >= deadline {
            return Err(timed_out());
        }
    }
    let collected = |buffer: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>| {
        let bytes = buffer.lock().map(|bytes| bytes.clone()).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Ok(GitOutput {
        success: status.success(),
        stdout: collected(&stdout),
        stderr: collected(&stderr),
    })
}

/// Read a pipe into a shared buffer on a detached thread, signalling when it
/// reaches end of file. The caller never joins it, so a pipe held open by
/// some other process cannot hold the caller.
fn drain(
    pipe: Option<impl Read + Send + 'static>,
    finished: std::sync::mpsc::Sender<()>,
) -> std::sync::Arc<std::sync::Mutex<Vec<u8>>> {
    let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = std::sync::Arc::clone(&buffer);
    std::thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut chunk = [0_u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        if let Ok(mut bytes) = sink.lock() {
                            bytes.extend_from_slice(&chunk[..read]);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        }
        let _ = finished.send(());
    });
    buffer
}

fn remote_failure(remote: &ArtifactRemote, operation: &str, stderr: &str) -> ArtifactError {
    let mut message = stderr.trim().to_string();
    if remote.url != remote.display {
        message = message.replace(&remote.url, &remote.display);
    }
    // Git quotes URLs in several forms; drop userinfo from any of them.
    let message = message
        .split_inclusive(char::is_whitespace)
        .map(|token| {
            if token.contains("://") {
                let trimmed = token.trim_end();
                let quote = |c: char| c == '\'' || c == '"';
                let inner = trimmed.trim_matches(quote);
                token.replacen(inner, &redact(inner), 1)
            } else {
                token.to_string()
            }
        })
        .collect::<String>();
    ArtifactError::RemoteFailed(format!(
        "git {operation} with artifact remote {} failed: {}",
        remote.display,
        if message.is_empty() {
            "no output"
        } else {
            &message
        }
    ))
}
