//! Artifact repository routes (spec §8): publish from a task workspace, read
//! and annotate by exact tree id, and open a read-only browser preview.
//!
//! Publishing is task-scoped because the bytes come from that task's
//! workspace. Everything else is repository-scoped: an artifact belongs to
//! the working repository's artifact store, not to the task that made it.

use super::lan_trust::PrivilegedTaskAccess;
use super::state::AppState;
use super::task_blockers::resolve_existing_task_id;
use crate::artifacts::store::{
    parse_object_id, ArtifactStore, BindingRequest, CommentRequest, DecisionRequest, PublishLimits,
    PublishRequest, RetentionSweep, TaskLifecycle,
};
use crate::artifacts::types::{ArtifactAnchor, ArtifactContentKind, ArtifactReference};
use crate::artifacts::{resolve_repository_path, ArtifactError};
use crate::db::{Db, Repo};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PublishArtifactRequest {
    path: String,
    kind: String,
    #[serde(default)]
    entrypoint: Option<String>,
    #[serde(default)]
    previous: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecordCommentRequest {
    author: String,
    body: String,
    #[serde(default)]
    anchor: Option<AnchorRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AnchorRequest {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    position: Option<String>,
    #[serde(default)]
    excerpt: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RecordDecisionRequest {
    who: String,
    what: String,
}

pub(super) async fn publish_artifact(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<String>,
    Json(request): Json<PublishArtifactRequest>,
) -> Response {
    blocking("artifact publish", move || {
        let kind = parse_kind(&request.kind)?;
        let db = open_db(&state)?;
        let task_id = resolve_existing_task_id(&db, &task_id)
            .map_err(|error| Box::new(error.into_response()))?;
        let item = db
            .get_pipeline_item(&task_id)
            .map_err(db_error)?
            .ok_or_else(|| refusal(StatusCode::NOT_FOUND, "task_not_found", "task not found"))?;
        if item.closed_at.is_some() {
            return Err(refusal(
                StatusCode::CONFLICT,
                "task_closed",
                "a closed task has no workspace to publish from",
            ));
        }
        let workspace = db
            .get_task_worktree_path(&task_id)
            .map_err(db_error)?
            .ok_or_else(|| {
                refusal(
                    StatusCode::CONFLICT,
                    "workspace_unavailable",
                    "the task workspace is unavailable",
                )
            })?;
        let repo = find_repo(&db, &item.repo_id)?;
        let policy = policy(&state, &repo)?;
        let path = resolve_repository_path(
            &state.artifact_storage,
            &repo.id,
            std::path::Path::new(&repo.path),
            policy.repository_path.as_deref(),
        )
        .map_err(artifact_error)?;
        let store = ArtifactStore::open_or_create(&path, &repo.id).map_err(artifact_error)?;
        let published = store
            .publish(PublishRequest {
                task_id: &task_id,
                workspace_root: std::path::Path::new(&workspace),
                source_path: &request.path,
                kind,
                entrypoint: request.entrypoint.as_deref(),
                previous: request.previous.as_deref(),
                retention: policy.retention,
                limits: PublishLimits::default(),
            })
            .map_err(artifact_error)?;
        Ok((StatusCode::CREATED, Json(published)).into_response())
    })
    .await
}

pub(super) async fn get_artifact(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
) -> Response {
    blocking("artifact read", move || {
        let store = existing_store(&state, &repo_id, &artifact_id)?;
        let detail = store.detail(&artifact_id).map_err(artifact_error)?;
        Ok(Json(detail).into_response())
    })
    .await
}

pub(super) async fn record_artifact_comment(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
    Json(request): Json<RecordCommentRequest>,
) -> Response {
    blocking("artifact comment", move || {
        let store = existing_store(&state, &repo_id, &artifact_id)?;
        let comment = store
            .record_comment(
                &artifact_id,
                CommentRequest {
                    author: &request.author,
                    body: &request.body,
                    anchor: request.anchor.map(|anchor| ArtifactAnchor {
                        path: anchor.path,
                        position: anchor.position,
                        excerpt: anchor.excerpt,
                    }),
                },
            )
            .map_err(artifact_error)?;
        Ok((StatusCode::CREATED, Json(comment)).into_response())
    })
    .await
}

pub(super) async fn record_artifact_decision(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
    Json(request): Json<RecordDecisionRequest>,
) -> Response {
    blocking("artifact decision", move || {
        let store = existing_store(&state, &repo_id, &artifact_id)?;
        let decision = store
            .record_decision(
                &artifact_id,
                DecisionRequest {
                    who: &request.who,
                    what: &request.what,
                },
            )
            .map_err(artifact_error)?;
        Ok((StatusCode::CREATED, Json(decision)).into_response())
    })
    .await
}

pub(super) async fn open_artifact_preview(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
) -> Response {
    let prepared = {
        let state = Arc::clone(&state);
        let repo_id = repo_id.clone();
        let artifact_id = artifact_id.clone();
        super::blocking::run_handler_blocking("artifact preview", move || {
            Ok((|| {
                let (store, repository_path) =
                    existing_store_with_path(&state, &repo_id, &artifact_id)?;
                let entrypoint = store.entrypoint(&artifact_id).map_err(artifact_error)?;
                Ok::<_, Refusal>((repository_path, entrypoint))
            })())
        })
        .await
    };
    let (repository_path, entrypoint) = match prepared {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(response)) => return *response,
        Err(error) => return error.into_response(),
    };
    let Some(entrypoint) = entrypoint else {
        return *refusal(
            StatusCode::CONFLICT,
            "no_entrypoint",
            "this artifact has no entrypoint to open; read its files with kanna_get_artifact",
        );
    };
    match state
        .artifact_previews
        .open(repo_id, artifact_id, repository_path, entrypoint)
        .await
    {
        Ok(opened) => Json(opened).into_response(),
        Err(super::artifact_preview::PreviewOpenError::Limit(limit)) => *refusal(
            StatusCode::TOO_MANY_REQUESTS,
            "preview_limit",
            &format!(
                "{limit} artifact previews are already open; close one (kanna_close_artifact) or let one expire"
            ),
        ),
        Err(super::artifact_preview::PreviewOpenError::Failed(message)) => *refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "preview_unavailable",
            &message,
        ),
    }
}

pub(super) async fn close_artifact_preview(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((repo_id, artifact_id)): Path<(String, String)>,
) -> Response {
    if let Err(error) = parse_object_id(&artifact_id) {
        return *artifact_error(error);
    }
    let closed = state.artifact_previews.close(&repo_id, &artifact_id).await;
    Json(json!({ "repoId": repo_id, "artifactId": artifact_id, "closed": closed })).into_response()
}

/// Most names one result may carry.
pub(crate) const MAX_RESULT_ARTIFACTS: usize = 64;
const MAX_REFERENCE_TEXT_BYTES: usize = 2048;

/// Resolve and bind the `artifacts` map of a result `complete_stage` is
/// about to accept, returning the object its ledger entry records.
///
/// `raw` maps a name to either a bare tree id (stored content in the task's
/// own repository) or one of T6's tagged references (`stored`, `commit`,
/// `pr`). Everything is validated before anything is written, and stored
/// content must resolve in the task's artifact repository — published there
/// and still retained — or the whole result is refused with nothing
/// recorded. Accepted stored references are bound to the task in the
/// artifact repository, which is what keeps them from retention while the
/// task is open; that binding is written before the result, so a failure
/// after it can only keep content longer, never lose it.
pub(super) fn bind_result_artifacts(
    state: &AppState,
    db: &Db,
    task_id: &str,
    run_id: &str,
    raw: &serde_json::Value,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let refuse = |message: String| {
        (
            StatusCode::BAD_REQUEST,
            format!("{message}; nothing was recorded"),
        )
    };
    let entries = raw.as_object().ok_or_else(|| {
        refuse("`artifacts` must be an object mapping a name to an artifact reference".into())
    })?;
    if entries.len() > MAX_RESULT_ARTIFACTS {
        return Err(refuse(format!(
            "`artifacts` names {} references; at most {MAX_RESULT_ARTIFACTS} are allowed",
            entries.len()
        )));
    }
    let item = db
        .get_pipeline_item(task_id)
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("db error: {error}"),
            )
        })?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("task not found: {task_id}")))?;
    let mut parsed = Vec::with_capacity(entries.len());
    for (name, value) in entries {
        let invalid_name = name.trim().is_empty()
            || name.trim() != name
            || name.len() > crate::artifacts::store::MAX_DECLARED_NAME_BYTES
            || name.chars().any(char::is_control);
        if invalid_name {
            return Err(refuse(format!(
                "artifact name {:?} must be non-empty trimmed text of at most {} bytes without control characters",
                name.chars().take(80).collect::<String>(),
                crate::artifacts::store::MAX_DECLARED_NAME_BYTES
            )));
        }
        let reference = match value {
            serde_json::Value::String(id) => ArtifactReference::Stored {
                repo_id: item.repo_id.clone(),
                artifact_id: id.clone(),
                kind: ArtifactContentKind::Document,
            },
            serde_json::Value::Object(_) => {
                serde_json::from_value::<ArtifactReference>(value.clone()).map_err(|error| {
                    refuse(format!("artifact {name:?} is not a reference: {error}"))
                })?
            }
            _ => {
                return Err(refuse(format!(
                    "artifact {name:?} must be a tree id or a reference object"
                )))
            }
        };
        let declared_kind = matches!(value, serde_json::Value::Object(_));
        validate_external_reference(name, &reference, &item.repo_id).map_err(refuse)?;
        parsed.push((name.as_str(), reference, declared_kind));
    }
    let requests = parsed
        .iter()
        .filter_map(|(name, reference, declared_kind)| match reference {
            ArtifactReference::Stored {
                artifact_id, kind, ..
            } => Some(BindingRequest {
                name,
                artifact_id,
                kind: declared_kind.then_some(*kind),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut bound = Vec::new();
    if !requests.is_empty() {
        let repo = find_repo(db, &item.repo_id).map_err(|_| {
            (
                StatusCode::CONFLICT,
                format!("task repository {} is not registered", item.repo_id),
            )
        })?;
        let policy = crate::task_creator::load_repo_artifact_policy(&state.repo_definitions, &repo)
            .map_err(|error| {
                (
                    StatusCode::CONFLICT,
                    format!("repository configuration could not be resolved: {error}"),
                )
            })?;
        let path = resolve_repository_path(
            &state.artifact_storage,
            &repo.id,
            std::path::Path::new(&repo.path),
            policy.repository_path.as_deref(),
        )
        .map_err(|error| refuse(error.to_string()))?;
        let store = ArtifactStore::open_existing(&path, &repo.id)
            .map_err(|error| refuse(error.to_string()))?
            .ok_or_else(|| {
                refuse(format!(
                    "repository {} has no artifact store, so artifact {} does not resolve",
                    repo.id, requests[0].artifact_id
                ))
            })?;
        bound = store
            .bind_to_result(task_id, Some(run_id), &requests)
            .map_err(|error| match error {
                ArtifactError::Storage(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
                other => refuse(format!("artifact reference does not resolve: {other}")),
            })?;
    }
    let mut bound = bound.into_iter();
    let mut recorded = serde_json::Map::new();
    for (name, reference, _) in parsed {
        let reference = match reference {
            ArtifactReference::Stored { .. } => bound.next().ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "artifact binding lost a reference".to_string(),
                )
            })?,
            other => other,
        };
        recorded.insert(
            name.to_string(),
            serde_json::to_value(reference).map_err(|error| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("cannot encode artifact reference: {error}"),
                )
            })?,
        );
    }
    Ok(serde_json::Value::Object(recorded))
}

/// Shape checks for every reference kind. Stored content must be in the
/// task's own repository; commits and PRs name content that lives elsewhere
/// and are recorded as given once well formed.
fn validate_external_reference(
    name: &str,
    reference: &ArtifactReference,
    task_repo_id: &str,
) -> Result<(), String> {
    let is_sha = |value: &str| {
        (value.len() == 40 || value.len() == 64)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    match reference {
        ArtifactReference::Stored {
            repo_id,
            artifact_id,
            ..
        } => {
            if repo_id != task_repo_id {
                return Err(format!(
                    "artifact {name:?} is stored in repository {repo_id}; a result may only name stored artifacts of its own repository ({task_repo_id})"
                ));
            }
            parse_object_id(artifact_id)
                .map(|_| ())
                .map_err(|error| format!("artifact {name:?}: {error}"))
        }
        ArtifactReference::Commit { repo_id, sha } => {
            if repo_id.trim().is_empty() || repo_id.len() > MAX_REFERENCE_TEXT_BYTES {
                return Err(format!("commit reference {name:?} needs a repoId"));
            }
            if !is_sha(sha) {
                return Err(format!(
                    "commit reference {name:?} needs a full lowercase commit sha"
                ));
            }
            Ok(())
        }
        ArtifactReference::Pr { url, head_sha } => {
            let scheme_ok = url.starts_with("https://") || url.starts_with("http://");
            if !scheme_ok
                || url.len() > MAX_REFERENCE_TEXT_BYTES
                || url.chars().any(char::is_whitespace)
            {
                return Err(format!("pr reference {name:?} needs an http(s) url"));
            }
            if !is_sha(head_sha) {
                return Err(format!(
                    "pr reference {name:?} needs a full lowercase headSha"
                ));
            }
            Ok(())
        }
    }
}

/// Enforce retention in every registered repository's artifact store.
/// Repositories without a store, or whose location no longer resolves, are
/// skipped; one failing repository never stops the others.
pub(crate) fn sweep_artifact_retention(
    state: &AppState,
    now: std::time::SystemTime,
) -> Vec<(String, Result<RetentionSweep, String>)> {
    let db = match Db::open(&state.config().db_path) {
        Ok(db) => db,
        Err(error) => return vec![(String::new(), Err(format!("db error: {error}")))],
    };
    let repos = match db.list_repos() {
        Ok(repos) => repos,
        Err(error) => return vec![(String::new(), Err(format!("db error: {error}")))],
    };
    let lifecycle = |task_id: &str| match db.get_pipeline_item(task_id) {
        Ok(Some(item)) => match item.closed_at.as_deref() {
            None => TaskLifecycle::Open,
            Some(closed_at) => match parse_sqlite_utc(closed_at) {
                Some(at) => TaskLifecycle::Closed { at },
                None => TaskLifecycle::Unknown,
            },
        },
        _ => TaskLifecycle::Unknown,
    };
    let mut outcomes = Vec::new();
    for repo in repos {
        let Ok(policy) =
            crate::task_creator::load_repo_artifact_policy(&state.repo_definitions, &repo)
        else {
            continue;
        };
        let Ok(path) = resolve_repository_path(
            &state.artifact_storage,
            &repo.id,
            std::path::Path::new(&repo.path),
            policy.repository_path.as_deref(),
        ) else {
            continue;
        };
        let store = match ArtifactStore::open_existing(&path, &repo.id) {
            Ok(Some(store)) => store,
            Ok(None) => continue,
            Err(error) => {
                outcomes.push((repo.id.clone(), Err(error.to_string())));
                continue;
            }
        };
        outcomes.push((
            repo.id.clone(),
            store
                .sweep_retention(now, &lifecycle)
                .map_err(|error| error.to_string()),
        ));
    }
    outcomes
}

/// `YYYY-MM-DD HH:MM:SS[.fff]` (SQLite `datetime('now')`, UTC).
fn parse_sqlite_utc(value: &str) -> Option<std::time::SystemTime> {
    let value = value.trim().trim_end_matches('Z');
    let (date, time) = value.split_once([' ', 'T'])?;
    let mut date = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (date.next()??, date.next()??, date.next()??);
    let time = time.split('.').next()?;
    let mut time = time.split(':').map(|part| part.parse::<i64>().ok());
    let (hour, minute, second) = (time.next()??, time.next()??, time.next()??);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days-from-civil.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds)
        .ok()
        .map(|seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
}

async fn blocking(
    label: &'static str,
    work: impl FnOnce() -> Result<Response, Refusal> + Send + 'static,
) -> Response {
    match super::blocking::run_handler_blocking(label, move || Ok(work())).await {
        Ok(Ok(response)) => response,
        Ok(Err(response)) => *response,
        Err(error) => error.into_response(),
    }
}

fn parse_kind(value: &str) -> Result<ArtifactContentKind, Refusal> {
    match value {
        "document" => Ok(ArtifactContentKind::Document),
        "mockup" => Ok(ArtifactContentKind::Mockup),
        "media" => Ok(ArtifactContentKind::Media),
        "report" => Ok(ArtifactContentKind::Report),
        _ => Err(refusal(
            StatusCode::BAD_REQUEST,
            "invalid_kind",
            "kind must be one of document, mockup, media, report",
        )),
    }
}

fn existing_store(
    state: &AppState,
    repo_id: &str,
    artifact_id: &str,
) -> Result<ArtifactStore, Refusal> {
    existing_store_with_path(state, repo_id, artifact_id).map(|(store, _)| store)
}

/// Open a repository's artifact store for a read or annotation. A repository
/// with no artifact store yet has, by definition, no such artifact; nothing is
/// created on this path.
fn existing_store_with_path(
    state: &AppState,
    repo_id: &str,
    artifact_id: &str,
) -> Result<(ArtifactStore, std::path::PathBuf), Refusal> {
    parse_object_id(artifact_id).map_err(artifact_error)?;
    let db = open_db(state)?;
    let repo = find_repo(&db, repo_id)?;
    let policy = policy(state, &repo)?;
    let path = resolve_repository_path(
        &state.artifact_storage,
        &repo.id,
        std::path::Path::new(&repo.path),
        policy.repository_path.as_deref(),
    )
    .map_err(artifact_error)?;
    match ArtifactStore::open_existing(&path, &repo.id).map_err(artifact_error)? {
        Some(store) => Ok((store, path)),
        None => Err(artifact_error(ArtifactError::NotFound {
            repo_id: repo.id,
            artifact_id: artifact_id.to_string(),
        })),
    }
}

fn policy(
    state: &AppState,
    repo: &Repo,
) -> Result<crate::task_creator::RepoArtifactPolicy, Refusal> {
    crate::task_creator::load_repo_artifact_policy(&state.repo_definitions, repo).map_err(|error| {
        refusal(
            StatusCode::CONFLICT,
            "artifact_config_unresolved",
            &format!("repository configuration could not be resolved: {error}"),
        )
    })
}

fn find_repo(db: &Db, repo_id: &str) -> Result<Repo, Refusal> {
    db.get_repo(repo_id).map_err(db_error)?.ok_or_else(|| {
        refusal(
            StatusCode::NOT_FOUND,
            "repo_not_found",
            &format!("repository {repo_id} not found"),
        )
    })
}

fn open_db(state: &AppState) -> Result<Db, Refusal> {
    Db::open(&state.config().db_path).map_err(|error| {
        refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            &format!("db error: {error}"),
        )
    })
}

fn db_error(error: rusqlite::Error) -> Refusal {
    refusal(
        StatusCode::INTERNAL_SERVER_ERROR,
        "db_error",
        &format!("db error: {error}"),
    )
}

/// An error response. Boxed: a `Response` is large for an `Err` variant.
type Refusal = Box<Response>;

fn refusal(status: StatusCode, code: &str, message: &str) -> Refusal {
    Box::new((status, Json(json!({ "error": code, "message": message }))).into_response())
}

pub(super) fn artifact_status(error: &ArtifactError) -> StatusCode {
    match error {
        ArtifactError::InvalidRequest(_)
        | ArtifactError::InvalidId(_)
        | ArtifactError::WrongObjectType { .. }
        | ArtifactError::InvalidPath(_)
        | ArtifactError::InvalidEntrypoint(_)
        | ArtifactError::InvalidPrevious { .. } => StatusCode::BAD_REQUEST,
        ArtifactError::SourceNotFound(_)
        | ArtifactError::NotFound { .. }
        | ArtifactError::ContentMissing { .. }
        | ArtifactError::FileNotFound { .. } => StatusCode::NOT_FOUND,
        ArtifactError::WorkspaceUnavailable(_) | ArtifactError::Location(_) => StatusCode::CONFLICT,
        ArtifactError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
        ArtifactError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn artifact_error(error: ArtifactError) -> Refusal {
    let mut body = json!({ "error": error.code(), "message": error.to_string() });
    match &error {
        ArtifactError::NotFound {
            repo_id,
            artifact_id,
        }
        | ArtifactError::ContentMissing {
            repo_id,
            artifact_id,
        }
        | ArtifactError::InvalidPrevious {
            repo_id,
            artifact_id,
        } => {
            body["repoId"] = json!(repo_id);
            body["artifactId"] = json!(artifact_id);
        }
        ArtifactError::FileNotFound { artifact_id, path } => {
            body["artifactId"] = json!(artifact_id);
            body["path"] = json!(path);
        }
        _ => {}
    }
    Box::new((artifact_status(&error), Json(body)).into_response())
}
