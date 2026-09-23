//! The merge master as the merge window of a release-workflow task (spec §10,
//! T14): a live merge master claimed under the old one-stage workflow moves
//! onto the release workflow at a quiescent boundary without a second merge
//! master, and the task then walks merge window → QA gauntlet → ship staging
//! → soak → ship production. The daemon only records commands, so no stage
//! runs anything, and nothing here deploys.
use super::actions::{
    commit_branch_change, ledger_files, ledger_fixture_config, post_json, spawn_recording_daemon,
    wait_for_running_task_stage,
};
use super::*;
use crate::db::task_store::LedgerEntryKind;
use crate::task_creator::MergeSingletonMigration;
use kanna_daemon::protocol::Command as DaemonCommand;

const MASTER: &str = "merge-1";

/// The definition every merge master was claimed under before T14, exactly
/// as the old synthetic construction serialized it.
fn legacy_merge_workflow() -> String {
    serde_json::json!({
        "name": "singleton-merge",
        "stages": [{
            "name": "in progress",
            "agent": "merge",
            "prompt": "$TASK_PROMPT",
            "policy": { "transition": "manual" }
        }],
        "visibility": "internal"
    })
    .to_string()
}

struct ReleaseFixture {
    state: Arc<AppState>,
    app: axum::Router,
    db_path: String,
    commands: Arc<std::sync::Mutex<Vec<DaemonCommand>>>,
    repo_root: PathBuf,
    daemon_dir: PathBuf,
}

impl Drop for ReleaseFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.daemon_dir);
        let _ = std::fs::remove_dir_all(&self.repo_root);
    }
}

impl ReleaseFixture {
    fn new(label: &str) -> Self {
        Self::with_daemon(label, spawn_recording_daemon)
    }

    fn with_daemon(
        label: &str,
        spawn_daemon: impl FnOnce(&Path) -> Arc<std::sync::Mutex<Vec<DaemonCommand>>>,
    ) -> Self {
        let repo_root = crate::test_paths::unique_test_path(&format!("kanna-release-{label}"));
        init_test_git_repo(&repo_root);
        publish_test_origin_main(&repo_root);
        let worktree = commit_branch_change(&repo_root, "task-merge-1", "MERGED.md", "merged\n");
        let daemon_dir = crate::test_paths::unique_test_path(&format!("kanna-release-{label}-d"));
        std::fs::create_dir_all(&daemon_dir).unwrap();
        let commands = spawn_daemon(&daemon_dir);
        let config = ledger_fixture_config(&format!("release-{label}"), &daemon_dir);
        let db = Db::open_for_tests(&config.db_path).unwrap();
        db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
            .unwrap();
        db.insert_test_pipeline_item(
            MASTER,
            "repo-1",
            "",
            Some("Merge Master"),
            "in progress",
            "2026-09-20 10:00:00",
        )
        .unwrap();
        db.update_test_pipeline_item_stage_context(
            MASTER,
            "task-merge-1",
            "singleton-merge",
            None,
            "claude",
        )
        .unwrap();
        db.update_test_pipeline_item_pipeline_def(MASTER, &legacy_merge_workflow())
            .unwrap();
        db.upsert_worktree(
            "wt-merge-1",
            MASTER,
            &worktree.to_string_lossy(),
            "task-merge-1",
        )
        .unwrap();
        db.insert_stage_run(crate::db::NewStageRun {
            id: "merge-run",
            task_id: MASTER,
            stage: "in progress",
            kind: "main",
            agent: Some("merge"),
            agent_provider: Some("claude"),
            model: None,
            effort: None,
            status: "running",
            result: None,
            feedback: None,
            session_id: Some(MASTER),
            provider_session_id: None,
            cwd: Some(&worktree.to_string_lossy()),
            resumed_from_run_id: None,
        })
        .unwrap();
        db.insert_test_pipeline_item(
            "handoff-source",
            "repo-1",
            "Add the login form",
            Some("Add the login form"),
            "pr",
            "2026-09-20 11:00:00",
        )
        .unwrap();
        db.update_test_pipeline_item_stage_context(
            "handoff-source",
            "handoff-source",
            "default",
            Some("main"),
            "claude",
        )
        .unwrap();
        drop(db);
        let state = Arc::new(super::AppState::new(config.clone()));
        let app = super::router(Arc::clone(&state));
        Self {
            state,
            app,
            db_path: config.db_path,
            commands,
            repo_root,
            daemon_dir,
        }
    }

    fn db(&self) -> Db {
        Db::open(&self.db_path).unwrap()
    }

    async fn post(&self, action: &str, body: serde_json::Value) -> (StatusCode, String) {
        post_json(
            &self.app,
            &format!("/v1/tasks/{MASTER}/actions/{action}"),
            body,
        )
        .await
    }

    async fn settle(&self) {
        crate::http_api::wait_for_task_mutation_to_finish(&self.state, MASTER).await;
    }

    async fn migrate(&self) -> MergeSingletonMigration {
        super::super::signal_agent::migrate_merge_singleton(&self.state, MASTER)
            .await
            .unwrap()
    }

    fn pinned(&self) -> (Option<String>, serde_json::Value) {
        let item = self.db().get_pipeline_item(MASTER).unwrap().unwrap();
        (
            item.pipeline,
            serde_json::from_str(item.pipeline_def.as_deref().unwrap()).unwrap(),
        )
    }

    /// Agent sessions the daemon was asked to start for the task, as their
    /// argument lines.
    fn agent_spawns(&self) -> Vec<String> {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter_map(|command| match command {
                DaemonCommand::Spawn {
                    session_id, args, ..
                } if session_id == MASTER => Some(args.join(" ")),
                DaemonCommand::SpawnAgent { session_id, params } if session_id == MASTER => {
                    Some(params.prompt.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Anything the daemon was asked to type into a session.
    fn session_writes(&self) -> usize {
        self.commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    DaemonCommand::SubmitInputIfSession { .. } | DaemonCommand::Input { .. }
                )
            })
            .count()
    }

    fn run_for(&self, stage: &str) -> crate::db::StageRun {
        self.db()
            .list_stage_runs_for_task(MASTER)
            .unwrap()
            .into_iter()
            .rev()
            .find(|run| run.stage == stage)
            .unwrap_or_else(|| panic!("no run for stage {stage}"))
    }

    async fn complete(&self, stage: &str, summary: &str) {
        let run = self.run_for(stage);
        let (status, text) = self
            .post(
                "complete-stage",
                serde_json::json!({ "runId": run.id, "status": "success", "summary": summary }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        self.settle().await;
    }

    async fn advance(&self) {
        let (status, text) = self
            .post("advance-stage", serde_json::json!({ "source": "operator" }))
            .await;
        assert_eq!(status, StatusCode::OK, "{text}");
    }

    async fn wait_for_stage(&self, stage: &str) {
        let db = self.db();
        for _ in 0..100 {
            if db
                .get_pipeline_item(MASTER)
                .unwrap()
                .unwrap()
                .stage
                .as_deref()
                == Some(stage)
            {
                self.settle().await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the release never reached stage {stage}");
    }

    async fn hand_off(&self) -> (StatusCode, String) {
        post_json(
            &self.app,
            "/v1/tasks/handoff-source/actions/signal-merge-handoff",
            serde_json::json!({
                "branch": "feature/login",
                "target": "main",
                "prUrl": "https://github.com/acme/repo/pull/123",
                "summary": "Add the login form",
            }),
        )
        .await
    }

    fn open_merge_masters(&self) -> Vec<String> {
        self.db()
            .open_task_ids_with_pipeline("singleton-merge")
            .unwrap()
    }

    /// A handoff while the release is past its merge window is refused, types
    /// nothing, and never creates a second merge master.
    async fn assert_handoff_refused_outside_the_window(&self, stage: &str) {
        let writes = self.session_writes();
        let (status, text) = self.hand_off().await;
        assert_eq!(status, StatusCode::CONFLICT, "{text}");
        assert!(text.contains(&format!("stage '{stage}'")), "{text}");
        assert!(text.contains("left its merge window"), "{text}");
        assert_eq!(self.session_writes(), writes, "nothing was typed anywhere");
        assert_eq!(self.open_merge_masters(), vec![MASTER.to_string()]);
        assert!(self
            .db()
            .task_merge_signaled_at("handoff-source")
            .unwrap()
            .is_none());
    }
}

fn stage_names(definition: &serde_json::Value) -> Vec<String> {
    definition["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|stage| stage["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_live_merge_master_moves_onto_the_release_workflow_only_at_a_quiescent_boundary() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = ReleaseFixture::new("migrate");
    let before = fixture.pinned();
    let commands_before = fixture.commands.lock().unwrap().len();

    // Mid-turn: the merge master has not recorded its result. Not migrated.
    assert!(matches!(
        fixture.migrate().await,
        MergeSingletonMigration::Deferred(reason) if reason.contains("running")
    ));
    assert_eq!(fixture.pinned(), before);

    // The turn ends with the merge master's result: the quiescent boundary.
    fixture.complete("in progress", "Merged PR 91").await;
    assert_eq!(fixture.migrate().await, MergeSingletonMigration::Migrated);

    let (pipeline, definition) = fixture.pinned();
    assert_eq!(
        pipeline.as_deref(),
        Some("singleton-merge"),
        "claim marker kept"
    );
    assert_eq!(definition["name"], "release");
    assert_eq!(definition["routing"], "exits");
    assert_eq!(
        stage_names(&definition),
        [
            "in progress",
            "qa gauntlet",
            "ship staging",
            "soak",
            "ship production"
        ]
    );
    let db = fixture.db();
    let item = db.get_pipeline_item(MASTER).unwrap().unwrap();
    assert_eq!(item.stage.as_deref(), Some("in progress"));
    assert_eq!(item.display_name.as_deref(), Some("Merge Master"));
    assert!(item.closed_at.is_none());
    // Same run, same session: the conversation was not touched.
    let runs = db.list_stage_runs_for_task(MASTER).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, "merge-run");
    assert_eq!(runs[0].session_id.as_deref(), Some(MASTER));
    let sent: Vec<String> = fixture.commands.lock().unwrap()[commands_before..]
        .iter()
        .filter(|command| !matches!(command, DaemonCommand::List))
        .map(|command| format!("{command:?}"))
        .collect();
    assert!(
        sent.is_empty(),
        "migration only asks the daemon for its sessions: {sent:?}"
    );
    // Recorded as a workflow change made by the server.
    crate::task_store::flush_task(&db, &fixture.db_path, MASTER).unwrap();
    let plan = ledger_files(&fixture.db_path, MASTER)
        .into_iter()
        .rfind(|file| file.kind == LedgerEntryKind::Plan)
        .expect("the migration's plan entry");
    assert_eq!(plan.body()["to_workflow"], "singleton-merge");
    assert_eq!(plan.body()["after"]["name"], "release");
    assert_eq!(plan.envelope["channel_identity"]["kind"], "server");
    // Still the one merge master, found by its merge-window run.
    assert_eq!(fixture.open_merge_masters(), vec![MASTER.to_string()]);
    assert_eq!(
        db.find_open_agent_task("repo-1", "merge")
            .unwrap()
            .map(|task| task.task_id),
        Some(MASTER.to_string())
    );

    // Idempotent.
    let migrated = fixture.pinned();
    assert_eq!(
        fixture.migrate().await,
        MergeSingletonMigration::NotApplicable
    );
    assert_eq!(fixture.pinned(), migrated);
}

#[tokio::test]
async fn a_release_task_walks_every_stage_with_gates_parking_and_no_handoff_outside_the_window() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture = ReleaseFixture::new("walk");
    fixture
        .complete("in progress", "Merged PR 91 and PR 92")
        .await;
    assert_eq!(fixture.migrate().await, MergeSingletonMigration::Migrated);

    // Merge window -> QA gauntlet: a gate, no agent, parked for a person.
    fixture.advance().await;
    fixture.wait_for_stage("qa gauntlet").await;
    let gauntlet = fixture.run_for("qa gauntlet");
    assert_eq!(gauntlet.agent, None);
    assert_eq!(gauntlet.status, "running", "parked, awaiting a person");
    assert!(fixture.agent_spawns().is_empty());
    let (status, text) = fixture
        .post(
            "complete-stage",
            serde_json::json!({ "status": "success", "summary": "I am not here" }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "no session reports on a gate: {text}"
    );
    fixture
        .assert_handoff_refused_outside_the_window("qa gauntlet")
        .await;

    // QA gauntlet -> ship staging: the ship agent, told it may not publish.
    let (status, text) = fixture
        .post(
            "advance-stage",
            serde_json::json!({ "source": "operator", "summary": "Gauntlet passed" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    wait_for_running_task_stage(&fixture.db(), MASTER, "ship staging").await;
    fixture.settle().await;
    assert_eq!(
        fixture.run_for("ship staging").agent.as_deref(),
        Some("ship")
    );
    let spawns = fixture.agent_spawns();
    assert_eq!(spawns.len(), 1, "{spawns:?}");
    assert!(
        spawns[0].contains("authorizes no publish, deployment, rollback"),
        "{}",
        spawns[0]
    );
    fixture
        .assert_handoff_refused_outside_the_window("ship staging")
        .await;
    fixture
        .complete(
            "ship staging",
            "Would ship 1.4.0-staging.1; nothing published",
        )
        .await;
    assert_eq!(
        fixture
            .db()
            .get_pipeline_item(MASTER)
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("ship staging"),
        "a manual stage parks on its result"
    );

    // Ship staging -> soak: a gate; Kanna runs no timer.
    fixture.advance().await;
    fixture.wait_for_stage("soak").await;
    assert_eq!(fixture.run_for("soak").agent, None);
    assert_eq!(fixture.run_for("soak").status, "running");
    assert_eq!(fixture.agent_spawns().len(), 1, "a gate spawns nothing");

    // Soak -> ship production: the ship agent, told production needs a person.
    fixture.advance().await;
    wait_for_running_task_stage(&fixture.db(), MASTER, "ship production").await;
    fixture.settle().await;
    let spawns = fixture.agent_spawns();
    assert_eq!(spawns.len(), 2, "{spawns:?}");
    assert!(
        spawns[1].contains("authorizes no production operation"),
        "{}",
        spawns[1]
    );
    fixture
        .complete("ship production", "Would promote 1.4.0; nothing promoted")
        .await;

    // Leaving the final stage closes the release; only then may a handoff
    // claim a new merge master.
    fixture.advance().await;
    let db = fixture.db();
    for _ in 0..100 {
        if db
            .get_pipeline_item(MASTER)
            .unwrap()
            .unwrap()
            .closed_at
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(db
        .get_pipeline_item(MASTER)
        .unwrap()
        .unwrap()
        .closed_at
        .is_some());
    let stages: Vec<String> = db
        .list_stage_runs_for_task(MASTER)
        .unwrap()
        .into_iter()
        .map(|run| run.stage)
        .collect();
    assert_eq!(
        stages,
        [
            "in progress",
            "qa gauntlet",
            "ship staging",
            "soak",
            "ship production"
        ]
    );
    assert_eq!(
        fixture.agent_spawns().len(),
        2,
        "only the two ship sessions"
    );
}

/// A daemon that lists the merge master's live session with the status the
/// test sets, and accepts input into it.
fn spawn_live_session_daemon(
    daemon_dir: &Path,
    status: Arc<std::sync::Mutex<kanna_daemon::protocol::SessionStatus>>,
) -> Arc<std::sync::Mutex<Vec<DaemonCommand>>> {
    use kanna_daemon::protocol::{Event as DaemonEvent, SessionInfo, SessionState};
    use tokio::io::{AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    let socket_path = daemon_socket_path_for_dir(&daemon_dir.to_string_lossy());
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path).unwrap();
    let commands = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&commands);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let recorded = Arc::clone(&recorded);
            let status = Arc::clone(&status);
            tokio::spawn(async move {
                let (read_half, mut write_half) = stream.into_split();
                let mut reader = BufReader::new(read_half);
                while let Some(command) =
                    read_test_daemon_command_optional(&mut reader, &mut write_half).await
                {
                    if super::answer_terminal_carryover_probe(&command, &mut write_half).await {
                        continue;
                    }
                    let response = match &command {
                        DaemonCommand::List => DaemonEvent::SessionList {
                            sessions: vec![SessionInfo {
                                session_id: MASTER.to_string(),
                                pid: 42,
                                cwd: "/tmp".to_string(),
                                state: SessionState::Active,
                                idle_seconds: 0,
                                status: *status.lock().unwrap(),
                                status_observed: true,
                                kind: Default::default(),
                                composer_text: None,
                                composer_attestation: Default::default(),
                                attempt_id: None,
                            }],
                        },
                        DaemonCommand::Spawn { session_id, .. }
                        | DaemonCommand::SpawnAgent { session_id, .. } => {
                            DaemonEvent::SessionCreated {
                                session_id: session_id.clone(),
                            }
                        }
                        _ => DaemonEvent::Ok,
                    };
                    recorded.lock().unwrap().push(command);
                    if write_half
                        .write_all(
                            format!("{}\n", serde_json::to_string(&response).unwrap()).as_bytes(),
                        )
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            });
        }
    });
    commands
}

/// A merge master that has recorded a turn keeps a terminal run while a later
/// handoff starts a new turn in the same session; migration must wait for the
/// session, not the run, to be between turns.
#[tokio::test]
async fn a_merge_master_mid_turn_after_a_handoff_is_not_migrated_until_its_session_is_idle() {
    use kanna_daemon::protocol::SessionStatus;
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let status = Arc::new(std::sync::Mutex::new(SessionStatus::Idle));
    let daemon_status = Arc::clone(&status);
    let fixture = ReleaseFixture::with_daemon("mid-turn", move |dir| {
        spawn_live_session_daemon(dir, daemon_status)
    });
    let before = fixture.pinned();

    // First turn recorded: the run is terminal from here on.
    fixture.complete("in progress", "Merged PR 91").await;
    assert_eq!(fixture.run_for("in progress").status, "succeeded");

    // A handoff arrives and the merge master starts working on it.
    let (status_code, text) = fixture.hand_off().await;
    assert_eq!(status_code, StatusCode::OK, "{text}");
    assert_eq!(
        fixture.session_writes(),
        1,
        "the handoff reached the session"
    );
    *status.lock().unwrap() = SessionStatus::Busy;
    assert_eq!(
        fixture.run_for("in progress").status,
        "succeeded",
        "the new turn reopened no run"
    );
    assert!(matches!(
        fixture.migrate().await,
        MergeSingletonMigration::Deferred(reason) if reason.contains("mid-turn")
    ));
    assert_eq!(fixture.pinned(), before, "nothing changed mid-turn");

    // Waiting on a prompt is still mid-turn.
    *status.lock().unwrap() = SessionStatus::Waiting;
    assert!(matches!(
        fixture.migrate().await,
        MergeSingletonMigration::Deferred(_)
    ));
    assert_eq!(fixture.pinned(), before);

    // The turn ends; the session is idle at its composer.
    *status.lock().unwrap() = SessionStatus::Idle;
    assert_eq!(fixture.migrate().await, MergeSingletonMigration::Migrated);
    assert_eq!(fixture.pinned().1["name"], "release");
}

/// A daemon that cannot be asked is not evidence that the session is idle.
#[tokio::test]
async fn a_merge_master_whose_daemon_cannot_be_asked_is_not_migrated() {
    let _sidecar_guard = crate::test_sidecar_guard().await;
    let fixture =
        ReleaseFixture::with_daemon("no-daemon", |_| Arc::new(std::sync::Mutex::new(Vec::new())));
    fixture.complete("in progress", "Merged PR 91").await;
    let before = fixture.pinned();
    assert!(matches!(
        fixture.migrate().await,
        MergeSingletonMigration::Deferred(reason) if reason.contains("cannot be checked")
    ));
    assert_eq!(fixture.pinned(), before);
}
