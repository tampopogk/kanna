use super::{lan_trust::PrivilegedTaskAccess, state::AppState};
use crate::{daemon_client::DaemonClient, db::Db};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use kanna_daemon::protocol::{Command, Event, SessionState, TerminalAttemptArchive};
use std::{collections::HashSet, sync::Arc};
type Error = (StatusCode, String);
fn internal(e: impl ToString) -> Error {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn resolve(db: &Db, task: &str) -> Result<String, Error> {
    db.resolve_pipeline_item_id(task)
        .map_err(internal)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Task not found".into()))
}
// Requested reads reconcile known launches after a lost Exit or server restart.
// There is deliberately no Snapshot(session_id) fallback and no timer service.
async fn reconcile(
    state: &AppState,
    task: &str,
    run: &str,
) -> Result<Option<TerminalAttemptArchive>, Error> {
    let db = Db::open(&state.config.db_path).map_err(internal)?;
    if let Some(archive) = db.agent_terminal_archive(task, run).map_err(internal)? {
        return Ok(Some(archive));
    }
    let attempts = db.agent_terminal_attempts(task).map_err(internal)?;
    let Some(attempt) = attempts.iter().find(|a| a.id == run) else {
        return Err((StatusCode::NOT_FOUND, "Attempt not owned by task".into()));
    };
    if !attempt.recorded_launch {
        return Ok(None);
    }
    drop(db);
    let Ok(mut daemon) = DaemonClient::connect(&state.config.daemon_dir).await else {
        return Ok(None);
    };
    match daemon
        .send_command(&Command::ReadAttemptArchive {
            attempt_id: run.to_string(),
        })
        .await
        .map_err(|error| error.to_string())
    {
        Ok(Event::AttemptArchive {
            archive: Some(archive),
        }) => {
            Db::open(&state.config.db_path)
                .map_err(internal)?
                .ingest_agent_terminal_archive(task, run, &archive)
                .map_err(internal)?;
            // The DB transaction is committed before the daemon copy can be
            // released. A failed acknowledgement leaks a safe duplicate only.
            if !matches!(
                daemon
                    .send_command(&Command::ReleaseAttemptArchive {
                        attempt_id: run.to_string()
                    })
                    .await,
                Ok(Event::Ok)
            ) {
                log::warn!("[attempt-archive] retained daemon duplicate for {run}");
            }
            Ok(Some(archive))
        }
        // Legacy/unsupported/missing archives stay unavailable. Never reinterpret
        // the live session as history to hide a failed or unsupported read.
        Ok(Event::AttemptArchive { archive: None }) => Ok(None),
        other => {
            log::warn!("[attempt-archive] unavailable {run}: {other:?}");
            Ok(None)
        }
    }
}
/// A listed attempt plus whether its terminal is still the live one.
///
/// Liveness is not a property of the row: the daemon owns terminal lifetime, so
/// only its session registry can say whether an attempt's PTY is still the one
/// a viewer attaches to.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ListedAttempt {
    #[serde(flatten)]
    attempt: crate::db::AgentTerminalAttempt,
    live: bool,
}

/// The attempts whose terminals the daemon still runs.
///
/// Neither record in the database can answer this. A run's status cannot: a
/// main agent completes at a manual gate while its PTY keeps running, and a
/// post then continues in that same terminal under a different run. Archive
/// absence cannot either: a finished attempt whose final frame never arrived is
/// history with nothing to show, not a live session. A daemon that is gone,
/// unreachable, or older than the launch binding reports nothing here, which
/// lists every attempt as history rather than inventing a live one.
async fn live_attempt_ids(state: &AppState) -> HashSet<String> {
    let Ok(mut daemon) = DaemonClient::connect(&state.config.daemon_dir).await else {
        return HashSet::new();
    };
    match daemon.send_command(&Command::List).await {
        Ok(Event::SessionList { sessions }) => sessions
            .into_iter()
            .filter(|session| !matches!(session.state, SessionState::Exited(_)))
            .filter_map(|session| session.attempt_id)
            .collect(),
        other => {
            log::warn!("[attempt-archive] session registry unavailable: {other:?}");
            HashSet::new()
        }
    }
}

pub(super) async fn list(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path(task): Path<String>,
) -> Result<Json<Vec<ListedAttempt>>, Error> {
    let (task, attempts) = {
        let db = Db::open(&state.config.db_path).map_err(internal)?;
        let task = resolve(&db, &task)?;
        let attempts = db.agent_terminal_attempts(&task).map_err(internal)?;
        (task, attempts)
    };
    for attempt in attempts.iter().filter(|a| a.recorded_launch && !a.archived) {
        reconcile(&state, &task, &attempt.id).await?;
    }
    let attempts = Db::open(&state.config.db_path)
        .map_err(internal)?
        .agent_terminal_attempts(&task)
        .map_err(internal)?;
    let live = live_attempt_ids(&state).await;
    Ok(Json(
        attempts
            .into_iter()
            .map(|attempt| ListedAttempt {
                live: live.contains(&attempt.id),
                attempt,
            })
            .collect(),
    ))
}
pub(super) async fn read(
    _access: PrivilegedTaskAccess,
    State(state): State<Arc<AppState>>,
    Path((task, run)): Path<(String, String)>,
) -> Result<Json<Option<TerminalAttemptArchive>>, Error> {
    if !kanna_daemon::session_id::is_safe(&run) {
        return Err((StatusCode::BAD_REQUEST, "Unsafe attempt id".into()));
    }
    let task = {
        let db = Db::open(&state.config.db_path).map_err(internal)?;
        resolve(&db, &task)?
    };
    reconcile(&state, &task, &run).await.map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    #[tokio::test]
    async fn terminal_archive_legacy_daemon_never_falls_back_to_current_snapshot() {
        use tokio::io::AsyncBufReadExt;
        let state = crate::http_api::test_support::test_state_with_seed(
            "archive-legacy",
            "archive-legacy",
            crate::db::terminal_archives::tests::seed,
        );
        let socket =
            kanna_runtime_defaults::socket_path(std::path::Path::new(&state.config.daemon_dir));
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = tokio::io::BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            assert!(matches!(
                serde_json::from_str::<Command>(&line).unwrap(),
                Command::ReadAttemptArchive { .. }
            ));
            // Old daemons close on an unsupported command. Do not answer with
            // some current task's screen, and do not accept a fallback read.
        });
        assert!(reconcile(&state, "task-a", "run-task-a-1")
            .await
            .unwrap()
            .is_none());
        peer.await.unwrap();
        let _ = std::fs::remove_file(socket);
    }
    #[tokio::test]
    async fn attempt_archive_routes_enforce_ownership_and_keep_missing_history_explicit() {
        let app =
            crate::http_api::test_support::test_router_with_seed("archives", "archives", |db| {
                crate::db::terminal_archives::tests::seed(db);
                db.ingest_agent_terminal_archive(
                    "task-a",
                    "run-task-a-1",
                    &crate::db::terminal_archives::tests::archive(),
                )
                .unwrap();
            });
        for (path, status) in [
            (
                "/v1/tasks/task-a/terminal-attempts/run-task-a-1",
                StatusCode::OK,
            ),
            (
                "/v1/tasks/task-b/terminal-attempts/run-task-a-1",
                StatusCode::NOT_FOUND,
            ),
            (
                "/v1/tasks/task-a/terminal-attempts/legacy-task-a",
                StatusCode::OK,
            ),
            (
                "/v1/tasks/task-a/terminal-attempts/UPPERCASE",
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{path}");
            if path.ends_with("legacy-task-a") {
                assert_eq!(
                    axum::body::to_bytes(response.into_body(), usize::MAX)
                        .await
                        .unwrap(),
                    "null"
                );
            }
        }
    }
}

#[cfg(test)]
mod real_daemon_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use std::{
        path::PathBuf,
        process::{Child, Command as ProcessCommand},
        time::Duration,
    };
    use tower::ServiceExt;
    struct OwnedDaemon(Child);
    impl Drop for OwnedDaemon {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn daemon_binary() -> PathBuf {
        let binary = std::env::var_os("KANNA_DAEMON_TEST_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join(".build/debug/kanna-daemon")
            });
        assert!(
            binary.is_file(),
            "build the focused daemon fixture first: {binary:?}"
        );
        binary
    }
    async fn start_owned_daemon(daemon_dir: &str) -> (OwnedDaemon, DaemonClient) {
        std::fs::create_dir_all(daemon_dir).unwrap();
        let mut owned = OwnedDaemon(
            ProcessCommand::new(daemon_binary())
                .env("KANNA_DAEMON_DIR", daemon_dir)
                .env(
                    "KANNA_TERMINAL_RECOVERY_BIN",
                    "/nonexistent-archive-fixture-sidecar",
                )
                .spawn()
                .unwrap(),
        );
        let daemon = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(owned.0.try_wait().unwrap().is_none());
                if let Ok(daemon) = DaemonClient::connect(daemon_dir).await {
                    break daemon;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        (owned, daemon)
    }
    /// Every listed attempt as (id, live, archived), in the route's own order.
    async fn listed_attempts(app: &axum::Router) -> Vec<(String, bool, bool)> {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/tasks/task-a/terminal-attempts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        serde_json::from_slice::<Vec<serde_json::Value>>(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap()
        .into_iter()
        .map(|attempt| {
            (
                attempt["id"].as_str().unwrap().to_string(),
                attempt["live"].as_bool().unwrap(),
                attempt["archived"].as_bool().unwrap(),
            )
        })
        .collect()
    }
    /// A stage run's terminal outlives the run that opened it: a main agent
    /// completing at a manual gate, and the post that continues in the same
    /// PTY, must not turn the session a viewer is watching into history.
    #[tokio::test]
    async fn attempt_stays_live_through_main_completion_and_post_continuation() {
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let state = crate::http_api::test_support::test_state_with_seed(
            "archive-live",
            "archive-live",
            |db| crate::db::terminal_archives::tests::seed_at(db, &cwd),
        );
        let (_owned, mut daemon) = start_owned_daemon(&state.config.daemon_dir).await;
        let stop = std::path::Path::new(&state.config.daemon_dir).join("stop-live-attempt");
        {
            let db = Db::open(&state.config.db_path).unwrap();
            db.insert_stage_run(crate::db::NewStageRun {
                id: "run-task-a-3",
                task_id: "task-a",
                stage: "review",
                kind: "main",
                agent: Some("review"),
                agent_provider: None,
                model: None,
                effort: None,
                status: "running",
                result: None,
                feedback: None,
                session_id: Some("task-a"),
                provider_session_id: None,
                cwd: Some(&cwd),
                resumed_from_run_id: None,
            })
            .unwrap();
            db.bind_agent_terminal_attempt("run-task-a-3").unwrap();
        }
        let spawn = serde_json::from_value::<Command>(serde_json::json!({
            "type":"Spawn","session_id":"task-a","executable":"/bin/sh",
            "args":["-c",format!("printf 'LIVE\r\n'; while [ ! -f {} ]; do sleep 0.1; done; exit 5", stop.display())],
            "cwd":cwd,
            "env":{"KANNA_TASK_ID":"task-a","KANNA_STAGE_RUN_ID":"run-task-a-3"},
            "cols":120,"rows":24
        }))
        .unwrap();
        assert!(matches!(
            daemon.send_command(&spawn).await.unwrap(),
            Event::SessionCreated { .. }
        ));
        let app = crate::http_api::router(state.clone());
        let history = [
            ("run-task-a-1".to_string(), false, false),
            ("run-task-a-2".to_string(), false, false),
            ("legacy-task-a".to_string(), false, false),
        ];
        let live_now = |flag: bool, archived: bool| {
            history
                .iter()
                .cloned()
                .chain([("run-task-a-3".to_string(), flag, archived)])
                .collect::<Vec<_>>()
        };
        assert_eq!(listed_attempts(&app).await, live_now(true, false));

        // The agent records its verdict at a manual gate and a post continues in
        // the same terminal. The run is finished; the terminal is not.
        {
            let db = Db::open(&state.config.db_path).unwrap();
            db.finish_stage_run("run-task-a-3", "succeeded", Some("success"), None)
                .unwrap();
            db.insert_stage_run(crate::db::NewStageRun {
                id: "post-task-a-3",
                task_id: "task-a",
                stage: "review",
                kind: "post",
                agent: Some("commit"),
                agent_provider: None,
                model: None,
                effort: None,
                status: "running",
                result: None,
                feedback: None,
                session_id: Some("task-a"),
                provider_session_id: None,
                cwd: Some(&cwd),
                resumed_from_run_id: None,
            })
            .unwrap();
        }
        assert_eq!(listed_attempts(&app).await, live_now(true, false));

        // The terminal exits and its final frame is lost before the server can
        // ingest it: history with nothing to show, never a live session and
        // never a row that disappears.
        std::fs::write(&stop, b"").unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if matches!(
                    daemon
                        .send_command(&Command::ReadAttemptArchive {
                            attempt_id: "run-task-a-3".into()
                        })
                        .await
                        .unwrap(),
                    Event::AttemptArchive { archive: Some(_) }
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            daemon
                .send_command(&Command::ReleaseAttemptArchive {
                    attempt_id: "run-task-a-3".into()
                })
                .await
                .unwrap(),
            Event::Ok
        ));
        assert_eq!(listed_attempts(&app).await, live_now(false, false));
    }
    #[tokio::test]
    async fn terminal_archive_real_daemon_to_http_reconciles_after_same_id_reuse() {
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let state = crate::http_api::test_support::test_state_with_seed(
            "archive-real",
            "archive-real",
            |db| crate::db::terminal_archives::tests::seed_at(db, &cwd),
        );
        let binary = std::env::var_os("KANNA_DAEMON_TEST_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join(".build/debug/kanna-daemon")
            });
        assert!(
            binary.is_file(),
            "build the focused daemon fixture first: {binary:?}"
        );
        std::fs::create_dir_all(&state.config.daemon_dir).unwrap();
        let mut owned = OwnedDaemon(
            ProcessCommand::new(binary)
                .env("KANNA_DAEMON_DIR", &state.config.daemon_dir)
                .env(
                    "KANNA_TERMINAL_RECOVERY_BIN",
                    "/nonexistent-archive-fixture-sidecar",
                )
                .spawn()
                .unwrap(),
        );
        let mut daemon = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(owned.0.try_wait().unwrap().is_none());
                if let Ok(daemon) = DaemonClient::connect(&state.config.daemon_dir).await {
                    break daemon;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let spawn = |run: &str, script: &str| {
            serde_json::from_value::<Command>(serde_json::json!({"type":"Spawn","session_id":"task-a","executable":"/bin/sh","args":["-c",script],"cwd":cwd,"env":{"KANNA_TASK_ID":"task-a","KANNA_STAGE_RUN_ID":run},"cols":120,"rows":24})).unwrap()
        };
        assert!(matches!(daemon.send_command(&spawn("run-task-a-1","printf 'SERVER_FIRST\r\n'; awk 'BEGIN {for(i=0;i<4500;i++) print i \" café abcdefghijklmnopqrstuvwxyz abcdefghijklmnopqrstuvwxyz abcdefghijklmnopqrstuvwxyz\"}'; printf 'SERVER_LAST\r\n'; exit 7")).await.unwrap(),Event::SessionCreated{..}));
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if matches!(
                    daemon
                        .send_command(&Command::ReadAttemptArchive {
                            attempt_id: "run-task-a-1".into()
                        })
                        .await
                        .unwrap(),
                    Event::AttemptArchive { archive: Some(_) }
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(matches!(
            daemon
                .send_command(&spawn(
                    "run-task-a-2",
                    "printf 'REPLACEMENT_ONLY\r\n'; sleep 2; exit 0"
                ))
                .await
                .unwrap(),
            Event::SessionCreated { .. }
        ));
        // No watcher consumed A's Exit. Requested HTTP reconciliation must use
        // the known immutable binding even though the live task ID now is B.
        let app = crate::http_api::router(state.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/tasks/task-a/terminal-attempts/run-task-a-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let archive: TerminalAttemptArchive = serde_json::from_slice(&bytes).unwrap();
        let vt = &archive.snapshot.as_ref().unwrap().vt;
        assert!(vt.len() > 256 * 1024);
        assert!(vt.contains("SERVER_FIRST") && vt.contains("SERVER_LAST"));
        assert!(!vt.contains("REPLACEMENT_ONLY"));
        assert_eq!(archive.observed_exit_code, Some(7));
        // A fresh state represents server restart; the committed archive is
        // readable even after the daemon is gone, and repeated reads agree.
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if matches!(
                    daemon
                        .send_command(&Command::ReadAttemptArchive {
                            attempt_id: "run-task-a-2".into()
                        })
                        .await
                        .unwrap(),
                    Event::AttemptArchive { archive: Some(_) }
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(reconcile(&state, "task-a", "run-task-a-2")
            .await
            .unwrap()
            .is_some());
        drop(daemon);
        drop(owned);
        let restarted = crate::http_api::router(Arc::new(AppState::new(state.config.clone())));
        let response = restarted
            .oneshot(
                Request::builder()
                    .uri("/v1/tasks/task-a/terminal-attempts/run-task-a-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
            bytes
        );
    }

    /// A workspace teardown is archived by exactly the same daemon path an
    /// agent session is: it has a `stage_run` of its own written before the
    /// session exists, so `KANNA_STAGE_RUN_ID` can be bound into the detached
    /// `td-{branch}` session's environment, and the attempt list and read
    /// route carry it beside the stage's agent attempt.
    ///
    /// Liveness is the same question for a cleanup as for an agent session and
    /// has the same single answer — the daemon's session registry. A teardown
    /// binds its attempt through the identical `run-{task}-…` env rule, so
    /// while its `td-` session is still running it lists as the live terminal,
    /// which is the truth: that PTY exists and a viewer can attach to it. It
    /// becomes history the moment it exits. No kind is special-cased, because
    /// the registry is answering about a terminal, not about an agent.
    #[tokio::test]
    async fn workspace_teardown_session_archives_and_lists_with_its_exit_status() {
        let cwd = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let state = crate::http_api::test_support::test_state_with_seed(
            "archive-teardown",
            "archive-teardown",
            |db| {
                crate::db::terminal_archives::tests::seed_at(db, &cwd);
                db.insert_stage_run(crate::db::NewStageRun {
                    id: "run-task-a-teardown",
                    task_id: "task-a",
                    stage: "review",
                    kind: crate::db::stage_runs::TEARDOWN_RUN_KIND,
                    agent: None,
                    agent_provider: None,
                    model: None,
                    effort: None,
                    status: "running",
                    result: None,
                    feedback: None,
                    session_id: Some("td-task-a-2"),
                    provider_session_id: None,
                    cwd: Some(&cwd),
                    resumed_from_run_id: None,
                })
                .unwrap();
                db.bind_agent_terminal_attempt("run-task-a-teardown")
                    .unwrap();
            },
        );
        let (_owned, mut daemon) = start_owned_daemon(&state.config.daemon_dir).await;
        let stop = std::path::Path::new(&state.config.daemon_dir).join("stop-teardown");
        // Exactly the spawn the server builds for a teardown: the departed
        // workspace's session id, and the teardown run bound into its
        // environment. No completion context — a cleanup records no verdict.
        let spawn = serde_json::from_value::<Command>(serde_json::json!({
            "type": "Spawn",
            "session_id": "td-task-a-2",
            "executable": "/bin/sh",
            "args": ["-c", format!("printf 'TEARDOWN_RAN\r\n'; while [ ! -f {} ]; do sleep 0.1; done; exit 5", stop.display())],
            "cwd": cwd,
            "env": {"KANNA_TASK_ID": "task-a", "KANNA_STAGE_RUN_ID": "run-task-a-teardown"},
            "cols": 120,
            "rows": 24
        }))
        .unwrap();
        assert!(matches!(
            daemon.send_command(&spawn).await.unwrap(),
            Event::SessionCreated { .. }
        ));

        let app = crate::http_api::router(state.clone());
        // The cleanup is still running, so its terminal is the live one. The
        // stage's own agent attempts are history, and stay history.
        assert_eq!(
            listed_attempts(&app).await,
            vec![
                ("run-task-a-1".to_string(), false, false),
                ("run-task-a-2".to_string(), false, false),
                ("legacy-task-a".to_string(), false, false),
                ("run-task-a-teardown".to_string(), true, false),
            ]
        );

        std::fs::write(&stop, b"").unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if matches!(
                    daemon
                        .send_command(&Command::ReadAttemptArchive {
                            attempt_id: "run-task-a-teardown".into()
                        })
                        .await
                        .unwrap(),
                    Event::AttemptArchive { archive: Some(_) }
                ) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/v1/tasks/task-a/terminal-attempts")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let attempts: Vec<serde_json::Value> = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let teardown = attempts
            .iter()
            .find(|attempt| attempt["id"] == "run-task-a-teardown")
            .expect("the teardown attempt is listed");
        assert_eq!(teardown["kind"], "teardown");
        // Labelled by the stage whose workspace it tore down.
        assert_eq!(teardown["stage"], "review");
        assert_eq!(teardown["archived"], true);
        assert_eq!(teardown["observedExitCode"], 5);
        // The terminal is gone, so the cleanup is history like any other
        // finished attempt.
        assert_eq!(teardown["live"], false);
        assert!(
            attempts.iter().any(|attempt| attempt["kind"] == "main"),
            "the stage's own agent attempts are still listed"
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/tasks/task-a/terminal-attempts/run-task-a-teardown")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let archive: TerminalAttemptArchive = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(archive.binding.spawned_run_id, "run-task-a-teardown");
        assert!(archive
            .snapshot
            .as_ref()
            .unwrap()
            .vt
            .contains("TEARDOWN_RAN"));
        assert_eq!(archive.observed_exit_code, Some(5));
    }
}
