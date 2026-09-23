//! Stage dependency edges (T4): the dependent's first workspace forks from
//! the commit its first base edge's upstream result recorded, further base
//! edges are listed to the session and never merged, and edges into later
//! stages gate the move without touching its base.

use super::*;
use crate::db::NewStageEdge;

fn dependent_request(prompt: &str) -> CreateTaskRequest {
    CreateTaskRequest {
        repo_id: "repo-1".to_string(),
        prompt: prompt.to_string(),
        display_name: None,
        workflow_name: None,
        stage: None,
        base_ref: None,
        diff_base_ref: None,
        agent: None,
        agent_provider: Some("claude".to_string()),
        agent_type: Some("agent".to_string()),
        model: None,
        effort: None,
        permission_mode: None,
        allowed_tools: None,
        disallowed_tools: None,
        max_turns: None,
        max_budget_usd: None,
        setup_cmds: None,
        task_template: None,
        resume_session_id: None,
        recovery_snapshot: None,
        transfer_import: None,
        notify_task_id: None,
        review_context: None,
        parent_task_id: None,
        blocker_task_ids: None,
        terminal_cols: None,
        terminal_rows: None,
    }
}

fn edge(upstream: &str, stage: &str) -> NewStageEdge {
    NewStageEdge {
        upstream_task_id: upstream.to_string(),
        upstream_stage: stage.to_string(),
        dependent_stage: None,
    }
}

/// Commit `file` on `branch` (created from `from` when new) without touching
/// the main checkout's branch; returns the new commit.
fn commit_on_branch(repo_root: &std::path::Path, branch: &str, from: &str, file: &str) -> String {
    let exists = Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "-q",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo_root)
        .status()
        .unwrap()
        .success();
    if !exists {
        run_git_fixture(repo_root, &["branch", branch, from]);
    }
    let scratch = repo_root
        .join(".kanna-worktrees")
        .join(format!("scratch-{branch}"));
    if !scratch.exists() {
        run_git_fixture(
            repo_root,
            &["worktree", "add", scratch.to_str().unwrap(), branch],
        );
    }
    std::fs::write(scratch.join(file), format!("{file} on {branch}\n")).unwrap();
    run_git_fixture(&scratch, &["add", file]);
    run_git_fixture(&scratch, &["commit", "-m", &format!("add {file}")]);
    run_git_fixture(&scratch, &["rev-parse", "HEAD"])
}

fn upstream_task(db: &Db, id: &str, stages: &[&str]) {
    db.insert_test_pipeline_item(id, "repo-1", id, Some(id), stages[0], "2026-09-23 00:00:00")
        .unwrap();
    db.pin_test_stages(id, stages);
}

/// The upstream leaves `from` for `to` with a success result recording `sha`.
fn depart(db: &Db, task_id: &str, from: &str, to: &str, sha: &str) -> String {
    let result = db.record_test_stage_result(task_id, from, "success", Some(sha));
    db.update_pipeline_item_stage(task_id, to).unwrap();
    result
}

fn session_text(session: &PreparedSessionSpawn) -> String {
    match session {
        PreparedSessionSpawn::Pty { args, .. } => args.join(" "),
        PreparedSessionSpawn::Agent {
            system_prompt,
            prompt,
            ..
        } => format!("{system_prompt}\n{prompt}"),
    }
}

fn is_ancestor(repo: &str, ancestor: &str, descendant: &str) -> bool {
    Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(repo)
        .status()
        .unwrap()
        .success()
}

fn dependency_start_entries(db: &Db, task_id: &str) -> Vec<serde_json::Value> {
    db.pending_ledger_entries(task_id)
        .unwrap()
        .into_iter()
        .filter_map(|entry| {
            let file = crate::task_store::parse_ledger_file(
                entry.file_name.as_deref()?,
                entry.payload.as_deref()?,
            )
            .ok()?;
            (file.body()["operation"] == "dependency_start").then(|| file.body().clone())
        })
        .collect()
}

#[test]
fn dependent_starts_from_the_recorded_sha_even_after_upstream_commits_more() {
    let repo_root = init_git_repo("stage-edge-recorded-sha");
    let config = test_config("stage-edge-recorded-sha");
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    upstream_task(&db, "task-a", &["plan", "build"]);
    let recorded = commit_on_branch(&repo_root, "task-a", "main", "plan.md");

    let created = create_dormant_task_with_stage_edges(
        &db,
        dependent_request("Build on the plan"),
        Some("dep00001".to_string()),
        &[edge("task-a", "plan")],
    )
    .unwrap();
    assert_eq!(created.worktree_path, None);
    // Unsatisfied: nothing is prepared, no workspace exists.
    assert!(
        prepare_start_dormant_task_for_api(&db, &config, &created.task_id, Vec::new())
            .unwrap()
            .is_none()
    );
    assert!(db
        .get_task_worktree_path(&created.task_id)
        .unwrap()
        .is_none());

    let result = depart(&db, "task-a", "plan", "build", &recorded);
    // The upstream keeps committing after it left the stage.
    let later = commit_on_branch(&repo_root, "task-a", "main", "build.md");
    assert_ne!(later, recorded);

    let prepared = prepare_start_dormant_task_for_api(&db, &config, &created.task_id, Vec::new())
        .unwrap()
        .expect("satisfied edge makes the dependent runnable");
    let head = run_git_fixture(std::path::Path::new(&prepared.cwd), &["rev-parse", "HEAD"]);
    assert_eq!(head, recorded, "forked from the result's SHA, not the tip");
    assert!(!is_ancestor(&prepared.cwd, &later, &head));
    let item = db.get_pipeline_item(&created.task_id).unwrap().unwrap();
    assert_eq!(item.base_ref.as_deref(), Some(recorded.as_str()));

    let stored = db
        .list_stage_edges_into(&created.task_id)
        .unwrap()
        .remove(0);
    assert_eq!(stored.consumed_result_id.as_deref(), Some(result.as_str()));
    assert_eq!(stored.consumed_sha.as_deref(), Some(recorded.as_str()));
    let entries = dependency_start_entries(&db, &created.task_id);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["dependencies"][0]["role"], "base");
    assert_eq!(entries[0]["dependencies"][0]["committed_sha"], recorded);
    assert_eq!(entries[0]["dependencies"][0]["result_id"], result);

    let text = session_text(&prepared.session);
    assert!(
        text.contains(&format!(
            "- `base`: task `task-a` left stage `plan` with result `{result}` at commit `{recorded}`. This workspace was forked from that commit."
        )),
        "unexpected session text: {text}"
    );

    // A second start (a restart re-deciding the same task) prepares nothing.
    assert!(
        prepare_start_dormant_task_for_api(&db, &config, &created.task_id, Vec::new())
            .unwrap()
            .is_none()
    );

    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_file(config.db_path);
}

#[test]
fn multi_base_first_stage_lists_the_extra_bases_and_merges_nothing() {
    let repo_root = init_git_repo("stage-edge-multi-base");
    let config = test_config("stage-edge-multi-base");
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    upstream_task(&db, "task-a", &["plan", "build"]);
    upstream_task(&db, "task-c", &["mockup", "review"]);
    // Both write the same file differently: a merge by the engine would
    // conflict, which is exactly why the engine does not merge.
    let first = commit_on_branch(&repo_root, "task-a", "main", "shared.md");
    let second = commit_on_branch(&repo_root, "task-c", "main", "shared.md");

    let created = create_dormant_task_with_stage_edges(
        &db,
        dependent_request("Combine plan and mockup"),
        Some("dep00002".to_string()),
        &[edge("task-a", "plan"), edge("task-c", "mockup")],
    )
    .unwrap();
    let first_result = depart(&db, "task-a", "plan", "build", &first);
    // One of two base edges satisfied: still waiting.
    assert!(
        prepare_start_dormant_task_for_api(&db, &config, &created.task_id, Vec::new())
            .unwrap()
            .is_none()
    );
    let second_result = depart(&db, "task-c", "mockup", "review", &second);

    // Legacy blocker branches handed in by an old caller are ignored too.
    let prepared = prepare_start_dormant_task_for_api(
        &db,
        &config,
        &created.task_id,
        vec!["task-c".to_string()],
    )
    .unwrap()
    .expect("both edges satisfied");
    let head = run_git_fixture(std::path::Path::new(&prepared.cwd), &["rev-parse", "HEAD"]);
    assert_eq!(head, first, "the first edge is the fork point");
    assert!(
        !is_ancestor(&prepared.cwd, &second, &head),
        "nothing merged"
    );
    let parents = run_git_fixture(
        std::path::Path::new(&prepared.cwd),
        &["rev-list", "--parents", "-n", "1", "HEAD"],
    );
    assert_eq!(parents.split_whitespace().count(), 2, "no merge commit");

    let entries = dependency_start_entries(&db, &created.task_id);
    assert_eq!(entries.len(), 1);
    let dependencies = entries[0]["dependencies"].as_array().unwrap();
    assert_eq!(
        dependencies
            .iter()
            .map(|dependency| (
                dependency["upstream_task_id"].as_str().unwrap(),
                dependency["role"].as_str().unwrap(),
                dependency["committed_sha"].as_str().unwrap(),
            ))
            .collect::<Vec<_>>(),
        vec![
            ("task-a", "base", first.as_str()),
            ("task-c", "merge", second.as_str()),
        ]
    );
    let text = session_text(&prepared.session);
    let base_line = text
        .find(&format!(
            "- `base`: task `task-a` left stage `plan` with result `{first_result}`"
        ))
        .expect("base listed");
    let merge_line = text
        .find(&format!(
            "- `merge`: task `task-c` left stage `mockup` with result `{second_result}` at commit `{second}`. Kanna did not merge it."
        ))
        .expect("extra base listed for the session to merge");
    assert!(base_line < merge_line, "edge order kept");

    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_file(config.db_path);
}

#[test]
fn later_stage_edge_gates_the_advance_without_changing_its_base() {
    let repo_root = init_git_repo_with_workflow(
        "stage-edge-later-gate",
        "gated",
        "in progress",
        "auto",
        "claude",
    );
    let config = test_config("stage-edge-later-gate");
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    let definition =
        std::fs::read_to_string(repo_root.join(".kanna/workflows/gated.json")).unwrap();
    upstream_task(&db, "task-a", &["plan", "build"]);
    let upstream_sha = commit_on_branch(&repo_root, "task-a", "main", "upstream.md");

    db.insert_test_pipeline_item(
        "task-b",
        "repo-1",
        "Gated work",
        Some("Gated work"),
        "in progress",
        "2026-09-23 00:00:00",
    )
    .unwrap();
    run_git_fixture(&repo_root, &["branch", "task-b-branch", "main"]);
    db.update_test_pipeline_item_stage_context("task-b", "task-b-branch", "gated", None, "claude")
        .unwrap();
    db.update_test_pipeline_item_pipeline_def("task-b", &definition)
        .unwrap();
    insert_finished_stage_run(
        &db,
        "task-b",
        "in progress",
        "{\"status\":\"success\",\"summary\":\"done\"}",
    );
    db.insert_stage_edges(
        "task-b",
        &[NewStageEdge {
            upstream_task_id: "task-a".to_string(),
            upstream_stage: "plan".to_string(),
            dependent_stage: Some("pr".to_string()),
        }],
    )
    .unwrap();

    // A person's advance is refused while the edge is pending.
    let refused = match prepare_advance_stage_for_api(&db, &config, "task-b") {
        Err(error) => error,
        Ok(_) => panic!("advance into a gated stage must be refused"),
    };
    assert!(refused.starts_with("task is blocked:"), "{refused}");
    // An automatic completion parks and records what to replay.
    assert!(
        prepare_stage_completion_for_api(&db, &config, "task-b", Some("main"), Some("auto"))
            .unwrap()
            .is_none()
    );
    let wait = db.dependency_wait("task-b").unwrap().expect("parked wait");
    assert_eq!(wait.from_stage, "in progress");
    assert_eq!(wait.to_stage, "pr");
    assert_eq!(wait.payload["kind"], "main");
    assert_eq!(
        db.get_pipeline_item("task-b")
            .unwrap()
            .unwrap()
            .stage
            .as_deref(),
        Some("in progress")
    );

    let result = depart(&db, "task-a", "plan", "build", &upstream_sha);
    let run = match prepare_advance_stage_for_api(&db, &config, "task-b").unwrap() {
        PreparedStageTransition::Run(run) => run,
        _ => panic!("expected the gated stage to start"),
    };
    assert_eq!(run.next_stage, "pr");
    let head = run_git_fixture(std::path::Path::new(run.cwd()), &["rev-parse", "HEAD"]);
    assert!(
        !is_ancestor(run.cwd(), &upstream_sha, &head),
        "a gating edge never becomes the stage's base"
    );
    let text = session_text(&run.session);
    assert!(
        text.contains(&format!(
            "- `gate`: task `task-a` left stage `plan` with result `{result}` at commit `{upstream_sha}`. It only held this stage"
        )),
        "unexpected session text: {text}"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_file(config.db_path);
}

/// A later-stage entry prepares its session with the edge's input, then the
/// transition commits. An upstream departure in between must not change the
/// consumed record away from what the session was told; the newer result is
/// recorded as superseding it.
#[test]
fn departure_between_prepare_and_entry_commit_keeps_the_session_input() {
    let repo_root = init_git_repo_with_workflow(
        "stage-edge-entry-window",
        "gated",
        "in progress",
        "auto",
        "claude",
    );
    let config = test_config("stage-edge-entry-window");
    let db = Db::open_for_tests(&config.db_path).unwrap();
    db.insert_test_repo_with_path("repo-1", &repo_root.to_string_lossy(), "Repo One")
        .unwrap();
    let definition =
        std::fs::read_to_string(repo_root.join(".kanna/workflows/gated.json")).unwrap();
    upstream_task(&db, "task-a", &["plan", "build"]);
    let given = commit_on_branch(&repo_root, "task-a", "main", "first.md");
    db.insert_test_pipeline_item(
        "task-b",
        "repo-1",
        "Gated work",
        Some("Gated work"),
        "in progress",
        "2026-09-23 00:00:00",
    )
    .unwrap();
    run_git_fixture(&repo_root, &["branch", "task-b-branch", "main"]);
    db.update_test_pipeline_item_stage_context("task-b", "task-b-branch", "gated", None, "claude")
        .unwrap();
    db.update_test_pipeline_item_pipeline_def("task-b", &definition)
        .unwrap();
    insert_finished_stage_run(
        &db,
        "task-b",
        "in progress",
        "{\"status\":\"success\",\"summary\":\"done\"}",
    );
    db.insert_stage_edges(
        "task-b",
        &[NewStageEdge {
            upstream_task_id: "task-a".to_string(),
            upstream_stage: "plan".to_string(),
            dependent_stage: Some("pr".to_string()),
        }],
    )
    .unwrap();
    let given_result = depart(&db, "task-a", "plan", "build", &given);

    let run = match prepare_advance_stage_for_api(&db, &config, "task-b").unwrap() {
        PreparedStageTransition::Run(run) => run,
        _ => panic!("expected the gated stage to start"),
    };
    let text = session_text(&run.session);
    assert!(text.contains(&format!("result `{given_result}` at commit `{given}`")));

    // The upstream loops back and leaves again before the entry commits.
    db.record_test_stage_result("task-a", "build", "success", Some(&given));
    db.update_pipeline_item_stage("task-a", "plan").unwrap();
    let newer = commit_on_branch(&repo_root, "task-a", "main", "second.md");
    let newer_result = depart(&db, "task-a", "plan", "build", &newer);

    // The transition's commit (what spawning the prepared run records).
    db.update_pipeline_item_stage("task-b", "pr").unwrap();
    let edge = db.list_stage_edges_into("task-b").unwrap().remove(0);
    assert_eq!(
        edge.consumed_result_id.as_deref(),
        Some(given_result.as_str())
    );
    assert_eq!(edge.consumed_sha.as_deref(), Some(given.as_str()));
    assert_eq!(
        edge.superseded_result_id.as_deref(),
        Some(newer_result.as_str())
    );
    assert_eq!(edge.superseded_sha.as_deref(), Some(newer.as_str()));
    let superseded: i64 = db
        .connection_for_e2e_tests()
        .query_row(
            "SELECT COUNT(*) FROM task_event
             WHERE task_id = 'task-b' AND type = 'task.dependency_superseded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(superseded, 1);

    let _ = std::fs::remove_dir_all(&repo_root);
    let _ = std::fs::remove_file(config.db_path);
}
