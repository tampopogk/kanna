use super::state::{AppState, TunneledHttpInvoke};
use crate::{
    db::Db,
    terminal_editor::{self, EditorChoice},
};
use axum::http::StatusCode;
use axum::{
    extract::{ConnectInfo, Path, State},
    Extension, Json,
};
use kanna_daemon::protocol::{Command, Event};
use std::{net::SocketAddr, sync::Arc};

type HttpError = (StatusCode, String);
fn invalid(e: impl ToString) -> HttpError {
    (StatusCode::BAD_REQUEST, e.to_string())
}

// This integration is local-only. In particular an authenticated tunnel must
// never reinterpret another machine's pathname as a local editor target.
fn require_local(
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    tunneled: Option<Extension<TunneledHttpInvoke>>,
) -> Result<(), HttpError> {
    if tunneled.is_some() || !peer.is_some_and(|Extension(ConnectInfo(p))| p.ip().is_loopback()) {
        return Err((
            StatusCode::FORBIDDEN,
            "Terminal editing is available only on the desktop holding the local workspace".into(),
        ));
    }
    Ok(())
}

pub(super) async fn choices(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    tunneled: Option<Extension<TunneledHttpInvoke>>,
) -> Result<Json<Vec<EditorChoice>>, HttpError> {
    require_local(peer, tunneled)?;
    super::blocking::run_handler_blocking("terminal editor detection", move || {
        let db = Db::open(&state.config.db_path).map_err(invalid)?;
        terminal_editor::editor_choices(&db)
            .map(Json)
            .map_err(invalid)
    })
    .await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OpenEditorRequest {
    path: String,
    worktree_path: String,
    command: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct EditorSession {
    session_id: String,
    worktree_path: String,
    file_path: String,
    command: String,
}

pub(super) async fn open(
    State(state): State<Arc<AppState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    tunneled: Option<Extension<TunneledHttpInvoke>>,
    Path(task_id): Path<String>,
    Json(request): Json<OpenEditorRequest>,
) -> Result<Json<EditorSession>, HttpError> {
    require_local(peer, tunneled)?;
    let task_id = super::task_actions::resolve_task_id_for_mutation(&state, &task_id).await?;
    let _mutation = state.begin_requested_task_mutation(&task_id).await;
    let state_for_prepare = state.clone();
    let id = task_id.clone();
    let (session, choice) =
        super::blocking::run_handler_blocking("terminal editor prepare", move || {
            let db = Db::open(&state_for_prepare.config.db_path).map_err(invalid)?;
            let item = db
                .get_pipeline_item(&id)
                .map_err(invalid)?
                .ok_or_else(|| invalid("Task not found"))?;
            if item.closed_at.is_some() {
                return Err(invalid("This task is closed"));
            }
            let worktree = db
                .get_task_worktree_path(&id)
                .map_err(invalid)?
                .ok_or_else(|| invalid("Task workspace is unavailable"))?;
            if worktree != request.worktree_path {
                return Err((
                    StatusCode::CONFLICT,
                    "The task workspace changed. Reopen the file preview before editing".into(),
                ));
            }
            // Reuse the contained file resolver's absolute/relative path and symlink
            // validation. Editors themselves are user tools, not filesystem sandboxes.
            let file_path = crate::task_files::task_editor_file_path(&db, &id, &request.path)
                .map_err(invalid)?;
            let choice = terminal_editor::editor_choices(&db)
                .map_err(invalid)?
                .into_iter()
                .find(|c| c.command == request.command)
                .ok_or_else(|| {
                    invalid("Editor choice changed. Choose an installed terminal editor again")
                })?;
            Ok((
                EditorSession {
                    session_id: terminal_editor::session_id(
                        &id,
                        &worktree,
                        &file_path,
                        &choice.command,
                    ),
                    worktree_path: worktree,
                    file_path,
                    command: choice.command.clone(),
                },
                choice,
            ))
        })
        .await?;
    let mut daemon = crate::daemon_client::DaemonClient::connect(&state.config.daemon_dir)
        .await
        .map_err(invalid)?;
    let sessions = match daemon.send_command(&Command::List).await.map_err(invalid)? {
        Event::SessionList { sessions } => sessions,
        other => {
            return Err(invalid(format!(
                "Unable to inspect editor sessions: {other:?}"
            )))
        }
    };
    if let Some(existing) = sessions.iter().find(|s| s.session_id == session.session_id) {
        if terminal_editor::session_is_live(&existing.state) {
            return Ok(Json(session));
        }
        crate::task_creator::kill_session_replacing(
            &mut daemon,
            &state.session_replacements,
            &session.session_id,
        )
        .await
        .map_err(invalid)?;
    }
    let mut args = choice.args;
    // Absolute file name is a single argv entry, never an option or shell text.
    args.push(
        std::path::Path::new(&session.worktree_path)
            .join(&session.file_path)
            .to_string_lossy()
            .into_owned(),
    );
    let event = daemon
        .send_command(&Command::Spawn {
            session_id: session.session_id.clone(),
            executable: choice.executable,
            args,
            cwd: session.worktree_path.clone(),
            env: [
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("TERM_PROGRAM".into(), "kanna".into()),
            ]
            .into(),
            cols: 80,
            rows: 24,
            agent_provider: None,
            agent_executable: None,
            terminal_prelude: None,
            operator_input_only: false,
        })
        .await
        .map_err(invalid)?;
    match event {
        Event::SessionCreated { .. } => Ok(Json(session)),
        other => Err(invalid(format!(
            "Unable to start terminal editor: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanna_daemon::protocol::SessionInfo;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn local() -> Option<Extension<ConnectInfo<SocketAddr>>> {
        Some(Extension(ConnectInfo("127.0.0.1:1234".parse().unwrap())))
    }

    #[tokio::test]
    async fn terminal_editor_launch_is_local_contained_workspace_bound_and_reused() {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("actual workspace");
        std::fs::create_dir(&workspace).unwrap();
        let filename = "notes ' $(touch injected).txt";
        // Editing must not inherit the remote preview's 1 MiB text limit.
        std::fs::write(workspace.join(filename), vec![b'x'; 2 * 1024 * 1024]).unwrap();
        let worktree = workspace.to_string_lossy().to_string();
        let socket = kanna_runtime_defaults::socket_path(root.path());
        let listener = tokio::net::UnixListener::bind(socket).unwrap();
        let state = super::super::test_support::test_state_with_daemon_dir(
            "editor-test",
            "Editor test",
            &root.path().to_string_lossy(),
            |db| {
                db.insert_test_repo("repo-1", "repo").unwrap();
                db.insert_test_pipeline_item(
                    "task-1",
                    "repo-1",
                    "",
                    None,
                    "in progress",
                    "2026-09-11 10:00:00",
                )
                .unwrap();
                db.upsert_worktree("wt-1", "task-1", &worktree, "not-derived-from-this-branch")
                    .unwrap();
                // A custom executable is valid: the operator, not a whitelist,
                // chooses what terminal tool to run. No shell interprets its args.
                db.set_setting("terminalEditorCommand", "/bin/cat -u")
                    .unwrap();
            },
        );
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let commands = seen.clone();
        let server = tokio::spawn(async move {
            let mut sessions: Vec<SessionInfo> = Vec::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let (reader, mut writer) = stream.into_split();
                let mut lines = BufReader::new(reader).lines();
                while let Some(line) = lines.next_line().await.unwrap() {
                    let command: Command = serde_json::from_str(&line).unwrap();
                    let event = match &command {
                        Command::List => Event::SessionList {
                            sessions: sessions.clone(),
                        },
                        Command::Spawn {
                            session_id, cwd, ..
                        } => {
                            sessions.push(serde_json::from_value(serde_json::json!({ "session_id": session_id, "pid": 123, "cwd": cwd, "state": "Active", "idle_seconds": 0, "status": "idle" })).unwrap());
                            Event::SessionCreated {
                                session_id: session_id.clone(),
                            }
                        }
                        Command::Kill { session_id } => {
                            sessions.retain(|s| &s.session_id != session_id);
                            Event::Ok
                        }
                        Command::NegotiateProtectedInput { .. } => Event::ProtectedInputReady {
                            version: kanna_daemon::protocol::PROTECTED_INPUT_PROTOCOL_VERSION,
                        },
                        other => panic!("unexpected {other:?}"),
                    };
                    commands.lock().unwrap().push(command);
                    writer
                        .write_all(
                            format!("{}\n", serde_json::to_string(&event).unwrap()).as_bytes(),
                        )
                        .await
                        .unwrap();
                }
            }
        });
        let request = || OpenEditorRequest {
            path: filename.into(),
            worktree_path: worktree.clone(),
            command: "/bin/cat -u".into(),
        };
        assert_eq!(
            open(
                State(state.clone()),
                local(),
                Some(Extension(TunneledHttpInvoke)),
                Path("task-1".into()),
                Json(request())
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            open(
                State(state.clone()),
                None,
                None,
                Path("task-1".into()),
                Json(request())
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::FORBIDDEN
        );
        let session = open(
            State(state.clone()),
            local(),
            None,
            Path("task-1".into()),
            Json(request()),
        )
        .await
        .unwrap()
        .0;
        let again = open(
            State(state.clone()),
            local(),
            None,
            Path("task-1".into()),
            Json(request()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(session.session_id, again.session_id);
        assert_eq!(session.worktree_path, worktree);
        let spawned = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| match c {
                Command::Spawn { cwd, args, .. } => Some((cwd.clone(), args.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            spawned,
            vec![(
                worktree.clone(),
                vec![
                    "-u".to_string(),
                    workspace.join(filename).to_string_lossy().to_string()
                ]
            )]
        );
        assert!(!workspace.join("injected").exists());
        let db = Db::open(&state.config.db_path).unwrap();
        db.upsert_worktree("wt-1", "task-1", "/different/workspace", "next-stage")
            .unwrap();
        assert_eq!(
            open(
                State(state.clone()),
                local(),
                None,
                Path("task-1".into()),
                Json(request())
            )
            .await
            .unwrap_err()
            .0,
            StatusCode::CONFLICT
        );
        db.upsert_worktree("wt-1", "task-1", &worktree, "previous-stage")
            .unwrap();
        let mut outside = request();
        outside.path = "../outside".into();
        assert!(open(
            State(state.clone()),
            local(),
            None,
            Path("task-1".into()),
            Json(outside)
        )
        .await
        .is_err());
        let mut daemon = crate::daemon_client::DaemonClient::connect(&state.config.daemon_dir)
            .await
            .unwrap();
        terminal_editor::close_task_editors(&mut daemon, &state.session_replacements, "task-1")
            .await
            .unwrap();
        assert!(seen.lock().unwrap().iter().any(
            |c| matches!(c, Command::Kill { session_id } if session_id == &session.session_id)
        ));
        drop(daemon);
        db.close_pipeline_item("task-1").unwrap();
        assert!(open(
            State(state),
            local(),
            None,
            Path("task-1".into()),
            Json(request())
        )
        .await
        .is_err());
        server.abort();
        let _ = server.await;
    }
}
