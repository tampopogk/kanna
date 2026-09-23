use kanna_tool_catalog::{
    args_with_self_exclusion, bundled_catalog, clamp_task_event_hold_ms, clamp_task_event_limit,
    clamp_task_event_min_events, clamp_wait_timeout_secs, repo_context_task_id, resolve_request,
    resolve_request_with_repo_context, runtime_info_snapshot, task_event_batch_is_complete,
    task_event_self_exclusion, task_value_matches_wait_until, wait_resolved_result,
    wait_timeout_result, Catalog, Method, ParamLoc, ParamType, ResponseKind,
    RuntimeAdapterIdentity, WaitUntil, CLIENT_TOOL_CALL_BUDGET_SECS, DEFAULT_TASK_EVENT_LIMIT,
    DEFAULT_WAIT_TIMEOUT_SECS, MAX_TASK_EVENT_HOLD_MS, MAX_TASK_EVENT_LIMIT, MAX_WAIT_TIMEOUT_SECS,
};
use serde_json::json;
use std::fs;

#[test]
fn bundled_catalog_parses_and_declares_all_tools() {
    let catalog = bundled_catalog();
    let names = catalog
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec![
            "kanna_info",
            "kanna_list_machines",
            "kanna_guide",
            "kanna_subscribe_events",
            "kanna_read_event_subscription",
            "kanna_unsubscribe_events",
            "kanna_machine_stats",
            "kanna_list_transfer_peers",
            "kanna_list_repos",
            "kanna_add_repo",
            "kanna_reconcile_repo_metadata",
            "kanna_get_tasks",
            "kanna_list_recent_tasks",
            "kanna_get_task",
            "kanna_list_task_children",
            "kanna_wait_task",
            "kanna_wait_events",
            "kanna_notify_mobile",
            "kanna_set_task_workflow",
            "kanna_replace_task_workflow",
            "kanna_open_view",
            "kanna_workspace",
            "kanna_task_logs",
            "kanna_task_inputs",
            "kanna_task_transfers",
            "kanna_search_tasks",
            "kanna_list_repo_tasks",
            "kanna_doctor",
            "kanna_list_agents",
            "kanna_show_agent",
            "kanna_eject_agent",
            "kanna_create_task",
            "kanna_signal_agent",
            "kanna_signal_merge_handoff",
            "kanna_queue_reviewed_pr",
            "kanna_send_task_input",
            "kanna_send_task_raw_input",
            "kanna_close_task",
            "kanna_confirm_event_channel",
            "kanna_rename_task",
            "kanna_set_task_attention",
            "kanna_clear_task_attention",
            "kanna_record_task_serviced",
            "kanna_advance_stage",
            "kanna_push_task",
            "kanna_pull_task",
            "kanna_rerun_stage",
            "kanna_resume_task",
            "kanna_block_task",
            "kanna_unblock_task",
            "kanna_set_task_parent",
            "kanna_is_dependent_tasks_exist",
            "kanna_complete_stage",
            "kanna_request_revision",
            "kanna_publish_artifact",
            "kanna_get_artifact",
            "kanna_open_artifact",
            "kanna_close_artifact",
            "kanna_record_artifact_comment",
            "kanna_record_artifact_decision",
            "kanna_push_artifact",
            "kanna_fetch_artifact",
        ]
    );
}

#[test]
fn bundled_guides_are_topic_addressable_and_drive_schema_descriptions() {
    let catalog = bundled_catalog();
    assert_eq!(
        catalog.guide_topics(),
        vec!["config", "workflows", "agents", "tasks", "mobile"]
    );
    let config = catalog.render_guide("config").expect("config guide");
    assert!(config.contains("# Kanna Repository Configuration"));
    assert!(config.contains("arrays never concatenate"));
    assert!(config.contains("layer-coherent"));
    assert!(catalog
        .render_guide("workflows")
        .expect("workflow guide")
        .contains("Visibility belongs to the effective definition"));
    assert!(catalog
        .render_guide("agents")
        .expect("agent guide")
        .contains("EXTEND.md"));

    let request = resolve_request(&catalog, "kanna_guide", &json!({ "topic": "config" }))
        .expect("resolve guide");
    assert_eq!(request.kind, ResponseKind::Guide);
    assert!(request
        .local_response
        .is_some_and(|response| response["content"]
            .as_str()
            .is_some_and(|content| content.contains("Kanna Repository Configuration"))));
    assert!(catalog
        .config_schema_descriptions()
        .contains_key("/properties/agentProviders"));
}

#[test]
fn task_creation_and_workflow_guidance_distinguish_research_from_planning() {
    let catalog = bundled_catalog();
    let workflow_guide = catalog.render_guide("workflows").expect("workflow guide");
    assert!(workflow_guide.contains("public `research` workflow"));
    assert!(workflow_guide.contains("standalone manual product discussion"));
    assert!(workflow_guide.contains("never authorizes implementation"));
    assert!(workflow_guide.contains("technical approach research"));

    let create_task = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_create_task")
        .expect("create task tool");
    let workflow_name = create_task
        .params
        .iter()
        .find(|param| param.name == "workflow_name")
        .expect("workflow_name parameter");
    let description = workflow_name
        .description
        .as_deref()
        .expect("workflow_name description");
    assert!(description.contains("'research' is a standalone manual product discussion"));
    assert!(description.contains("recommendation never authorizes implementation"));
    assert!(description.contains("For an already chosen objective"));
    assert!(description.contains("'plan-build-review' adds a manual implementation-planning gate"));
}

#[test]
fn checked_in_config_schema_descriptions_match_catalog_guides() {
    let schema_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.kanna/config.schema.json");
    let schema: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(schema_path).expect("read checked-in config schema"),
    )
    .expect("parse checked-in config schema");

    for (pointer, description) in bundled_catalog().config_schema_descriptions() {
        let node = if pointer.is_empty() {
            &schema
        } else {
            schema
                .pointer(pointer)
                .unwrap_or_else(|| panic!("catalog guide references missing schema path {pointer}"))
        };
        assert_eq!(
            node["description"],
            json!(description),
            "schema description at {pointer} drifted from the catalog guide"
        );
    }
}

#[test]
fn generated_schema_preserves_required_order_types_and_enums() {
    let tools = bundled_catalog().tools_list_value();
    let info = tools
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "kanna_info")
        .expect("info tool");
    assert_eq!(
        info["inputSchema"]["properties"]["machine_id"]["type"],
        json!("string")
    );
    assert_eq!(info["annotations"], json!({ "readOnlyHint": true }));

    let list_task_children = tools
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "kanna_list_task_children")
        .expect("list task children tool");
    assert_eq!(
        list_task_children["annotations"],
        json!({ "readOnlyHint": true })
    );
    assert_eq!(
        list_task_children["inputSchema"]["required"],
        json!(["task_id"])
    );
    assert_eq!(
        list_task_children["inputSchema"]["properties"]["task_id"]["type"],
        json!("string")
    );
    assert!(
        list_task_children["description"]
            .as_str()
            .is_some_and(|description| description.contains("workflowName")),
        "list-task-children must document the workflow identity used to classify runless children"
    );

    let create_task = tools
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "kanna_create_task")
        .expect("create task tool");

    assert_eq!(create_task["inputSchema"]["required"], json!(["prompt"]));
    let allowed_tools = &create_task["inputSchema"]["properties"]["allowed_tools"];
    assert_eq!(allowed_tools["type"], json!("array"));
    assert_eq!(allowed_tools["items"], json!({ "type": "string" }));
    assert!(
        create_task["inputSchema"]["properties"]["stage"].is_null(),
        "agent-facing create-task tool should not expose stage overrides"
    );
    let agent = &create_task["inputSchema"]["properties"]["agent"];
    assert_eq!(
        agent["type"],
        json!("string"),
        "create-task must expose the agent override so orchestrators can bind any resolved agent"
    );
    assert!(
        agent["description"]
            .as_str()
            .is_some_and(|description| description.contains("kanna_list_agents")),
        "create-task must point orchestrators at resolved agent discovery"
    );
    assert_eq!(
        create_task["inputSchema"]["properties"]["model"]["description"],
        json!("Model id passed verbatim to the selected agent CLI: Claude uses '--model <id>', Copilot uses '--model=<id>', and Codex/OpenCode use '-m <id>'; Antigravity rejects model overrides. An explicit value overrides agent-definition frontmatter; omit it to use the provider default. Kanna does not maintain a model-id allowlist.")
    );
    assert_eq!(
        create_task["inputSchema"]["properties"]["effort"]["description"],
        json!("Provider-native reasoning effort passed without normalization. Codex uses model_reasoning_effort and validates against the selected model; Claude uses --effort (low|medium|high|xhigh|max); Copilot uses --effort (none|minimal|low|medium|high|xhigh|max); OpenCode uses --variant and validates against the selected model; Antigravity uses --effort (low|medium|high). Explicit task effort overrides repo agentProviders effort, then layered agent-definition frontmatter.")
    );

    let list_agents = tools
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "kanna_list_agents")
        .expect("list agents tool");
    let list_description = list_agents["description"]
        .as_str()
        .expect("list agents description");
    for source in ["built_in", "repo_override", "repo_authored"] {
        assert!(
            list_description.contains(source),
            "list-agents must document source value `{source}`"
        );
    }

    let wait = tools
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "kanna_wait_task")
        .expect("wait task tool");
    let until = &wait["inputSchema"]["properties"]["until"];
    assert_eq!(until["type"], json!("string"));
    assert_eq!(until["enum"], json!(["reconcile", "finished", "closed"]));

    let complete_stage = tools
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "kanna_complete_stage")
        .expect("complete stage tool");
    assert!(complete_stage["inputSchema"]["properties"]["machine_id"].is_null());
}

#[test]
fn task_input_and_resume_descriptions_document_delivery_and_recovery_contracts() {
    let tools = bundled_catalog().tools_list_value();
    let tools = tools.as_array().expect("tools array");
    let description = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .and_then(|tool| tool["description"].as_str())
            .unwrap_or_else(|| panic!("missing description for {name}"))
    };

    let send_input = description("kanna_send_task_input");
    for required in [
        "live daemon PTY session",
        "PTY process ID",
        "no_live_agent_session",
        "kanna_resume_task",
        "kanna_rerun_stage",
        // The owner's 2026-09-08 decision, stated where a caller reads it: a
        // live session always takes the message, and a human's unsent draft is
        // a collision rather than a reason to withhold it. A caller told
        // otherwise waits for a delivery that already happened.
        "always takes the message",
        "without waiting for the terminal to settle",
        "Nothing is ever queued, parked, or refused",
    ] {
        assert!(
            send_input.contains(required),
            "send-task-input must document `{required}`"
        );
    }

    let resume = description("kanna_resume_task");
    for required in [
        "cancelled or failed",
        "resumeFallbackReason",
        "older server",
        "kanna_rerun_stage",
    ] {
        assert!(
            resume.contains(required),
            "resume-task must document `{required}`"
        );
    }
}

#[test]
fn runtime_info_snapshot_allow_lists_status_and_keeps_identity_boundaries_separate() {
    let info = runtime_info_snapshot(
        "http://127.0.0.1:49199",
        RuntimeAdapterIdentity {
            name: "kanna-mcp",
            version: "0.1.0",
            mcp_protocol_version: Some("2025-11-25"),
            task_id: Some("task-safe"),
        },
        Ok(json!({
            "state": "running",
            "desktopId": "desktop-safe",
            "desktopName": "Safe Mac",
            "version": "9.8.7-staging.1",
            "environment": "staging",
            "serverVersion": "ignored-alias",
            "lanHost": "10.0.0.4",
            "lanPort": 48121,
            "pairingCode": "PAIR-SECRET",
            "kspStreamVersion": 2,
            "writePathHealth": {
                "healthy": true,
                "status": "healthy",
                "activeWorkspaceCommands": 0,
                "maxWorkspaceCommands": 4,
                "longRunningWorkspaceCommands": 0,
                "oldestWorkspaceCommandSeconds": null
            },
            "authToken": "AUTH-SECRET",
            "databasePath": "/private/kanna.db"
        })),
        &["kanna_info".to_string()],
    );

    assert_eq!(
        info["connection"]["effectiveBaseUrl"],
        "http://127.0.0.1:49199"
    );
    assert_eq!(info["connection"]["port"], 49199);
    assert_eq!(info["serverStatus"]["environment"], "staging");
    assert_eq!(info["serverStatus"]["version"], "9.8.7-staging.1");
    assert_eq!(
        info["lanAdvertisedEndpoint"],
        json!({ "host": "10.0.0.4", "port": 48121 })
    );
    assert_eq!(info["taskContext"]["taskId"], "task-safe");
    let rendered = info.to_string();
    for forbidden in [
        "PAIR-SECRET",
        "AUTH-SECRET",
        "/private/kanna.db",
        "pairingCode",
    ] {
        assert!(!rendered.contains(forbidden), "leaked {forbidden}");
    }
}

#[test]
fn generated_schema_surfaces_descriptions_defaults_and_integer_bounds() {
    let tools = bundled_catalog().tools_list_value();
    let tools = tools.as_array().expect("tools array");

    for tool in tools {
        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .expect("properties object");
        for (name, property) in properties {
            assert!(
                property["description"]
                    .as_str()
                    .is_some_and(|d| !d.is_empty()),
                "{}.{name} must describe itself for agents",
                tool["name"]
            );
        }
    }

    let wait = tools
        .iter()
        .find(|tool| tool["name"] == "kanna_wait_task")
        .expect("wait task tool");
    let timeout = &wait["inputSchema"]["properties"]["timeout_secs"];
    assert_eq!(timeout["default"], json!(DEFAULT_WAIT_TIMEOUT_SECS));
    assert_eq!(timeout["maximum"], json!(MAX_WAIT_TIMEOUT_SECS));
    let poll = &wait["inputSchema"]["properties"]["poll_secs"];
    assert_eq!(poll["default"], json!(3));
    assert_eq!(poll["minimum"], json!(1));
    assert_eq!(
        wait["inputSchema"]["properties"]["until"]["default"],
        json!("reconcile")
    );
}

#[test]
fn generated_tools_mark_get_tools_read_only() {
    let catalog = bundled_catalog();
    let tools = catalog.tools_list_value();
    let tools = tools.as_array().expect("tools array");

    for (tool, def) in tools.iter().zip(&catalog.tools) {
        if def.method == kanna_tool_catalog::Method::Get {
            assert_eq!(
                tool["annotations"],
                json!({ "readOnlyHint": true }),
                "{} is a GET tool and should carry a read-only hint",
                def.name
            );
        } else {
            assert!(
                tool.get("annotations").is_none(),
                "{} mutates state and should not claim read-only",
                def.name
            );
        }
    }
}

#[test]
fn removed_approval_override_is_not_an_agent_tool() {
    let catalog = bundled_catalog();

    assert!(catalog
        .tools
        .iter()
        .all(|tool| tool.name != "kanna_override_approval"));
    let advance = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_advance_stage")
        .expect("advance tool");
    assert_eq!(advance.params.len(), 9);
    assert_eq!(advance.params[0].name, "machine_id");
    assert_eq!(advance.params[0].location, ParamLoc::Routing);
    assert_eq!(advance.params[1].name, "task_id");
    assert_eq!(advance.params[2].name, "source");
    assert_eq!(advance.params[2].location, ParamLoc::Body);
    // The per-advance provider override for the stage the advance enters.
    // These decide how the *next* stage spawns; none of them approves
    // anything, which is what this test is guarding the tool against.
    assert_eq!(advance.params[4].name, "next_stage_agent_provider");
    assert_eq!(advance.params[4].location, ParamLoc::Body);
    assert_eq!(advance.params[5].name, "next_stage_model");
    assert_eq!(advance.params[5].location, ParamLoc::Body);
    assert_eq!(advance.params[6].name, "next_stage_effort");
    assert_eq!(advance.params[6].location, ParamLoc::Body);
    assert_eq!(advance.params[7].name, "next_stage_provider_source");
    assert_eq!(advance.params[7].location, ParamLoc::Body);
    // A compare-and-set fence on the workflow the caller actually inspected,
    // for a task whose remaining stages can be published while it runs. It
    // refuses a stale advance; it authorizes nothing.
    assert_eq!(advance.params[8].name, "expected_definition");
    assert_eq!(advance.params[8].location, ParamLoc::Body);
    assert_eq!(advance.params[3].name, "next_stage_harness");
    assert_eq!(
        advance.params[3].key.as_deref(),
        Some("nextStageAgentProvider")
    );
}

#[test]
fn resolves_expected_requests_for_every_bundled_tool() {
    let catalog = bundled_catalog();
    let cases = [
        (
            "kanna_info",
            json!({}),
            Method::Get,
            ResponseKind::RuntimeInfo,
            "/v1/status",
            json!({}),
        ),
        (
            "kanna_list_machines",
            json!({}),
            Method::Get,
            ResponseKind::Json,
            "/v1/cloud/desktops",
            json!({}),
        ),
        (
            "kanna_guide",
            json!({ "topic": "config" }),
            Method::Get,
            ResponseKind::Guide,
            "",
            json!({ "topic": "config" }),
        ),
        (
            "kanna_list_repos",
            json!({}),
            Method::Get,
            ResponseKind::Json,
            "/v1/repos",
            json!({}),
        ),
        (
            "kanna_add_repo",
            json!({ "path": "/Users/me/project", "name": "Project" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos",
            json!({ "path": "/Users/me/project", "name": "Project" }),
        ),
        (
            "kanna_get_tasks",
            json!({ "runtime_state": "idle", "sort_by": "createdAt", "order": "asc", "limit": 12 }),
            Method::Get,
            ResponseKind::Json,
            "/v1/tasks?runtimeState=idle&sortBy=createdAt&order=asc&limit=12",
            json!({}),
        ),
        (
            "kanna_list_recent_tasks",
            json!({}),
            Method::Get,
            ResponseKind::Json,
            "/v1/tasks/recent",
            json!({}),
        ),
        (
            "kanna_get_task",
            json!({ "task_id": "task 1" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/tasks/task%201?agentView=true",
            json!({}),
        ),
        (
            "kanna_list_task_children",
            json!({ "task_id": "task 1" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/tasks/task%201/children",
            json!({}),
        ),
        (
            "kanna_task_logs",
            json!({ "task_id": "task 1", "tail": 25 }),
            Method::Get,
            ResponseKind::Text,
            "/v1/tasks/task%201/logs?tail=25&agentView=true",
            json!({}),
        ),
        (
            "kanna_task_inputs",
            json!({ "task_id": "task 1", "tail": 25 }),
            Method::Get,
            ResponseKind::Json,
            "/v1/tasks/task%201/inputs?tail=25",
            json!({}),
        ),
        (
            "kanna_search_tasks",
            json!({ "query": "review me" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/tasks/search?query=review%20me",
            json!({}),
        ),
        (
            "kanna_list_repo_tasks",
            json!({ "repo_id": "repo-1" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/repos/repo-1/tasks",
            json!({}),
        ),
        (
            "kanna_doctor",
            json!({ "repo_id": "repo-1", "candidate_path": "/repo/task" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/repos/repo-1/doctor?candidate_path=%2Frepo%2Ftask",
            json!({}),
        ),
        (
            "kanna_list_agents",
            json!({ "repo_id": "repo-1" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/repos/repo-1/agents",
            json!({}),
        ),
        (
            "kanna_create_task",
            json!({
                "prompt": "Inferred repo task"
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks",
            json!({
                "prompt": "Inferred repo task"
            }),
        ),
        (
            "kanna_create_task",
            json!({
                "repo_id": "repo-1",
                "prompt": "Blocked work",
                "display_name": "Short task title",
                "agent_provider": "codex",
                "model": "gpt-5.6-codex",
                "effort": "xhigh",
                "blocker_task_ids": ["blocker-1", "blocker-2"]
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks",
            json!({
                "repoId": "repo-1",
                "prompt": "Blocked work",
                "displayName": "Short task title",
                "agentProvider": "codex",
                "model": "gpt-5.6-codex",
                "effort": "xhigh",
                "blockerTaskIds": ["blocker-1", "blocker-2"]
            }),
        ),
        (
            "kanna_create_task",
            json!({
                "repo_id": "repo-1",
                "prompt": "Subtask",
                "parent_task_id": "task-parent"
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks",
            json!({
                "repoId": "repo-1",
                "prompt": "Subtask",
                "parentTaskId": "task-parent"
            }),
        ),
        (
            "kanna_signal_agent",
            json!({
                "repo_id": "repo-1",
                "agent": "merge",
                "message": "MERGE task-1 -> main: ready"
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos/repo-1/agents/merge/signal",
            json!({
                "message": "MERGE task-1 -> main: ready"
            }),
        ),
        (
            "kanna_queue_reviewed_pr",
            json!({ "task_id": "review-1", "review_context_version": 4,
                "head_sha": "abc", "instruction": "Queue this PR." }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/review-1/actions/queue-reviewed-pr",
            json!({ "reviewContextVersion": 4, "headSha": "abc", "instruction": "Queue this PR." }),
        ),
        (
            "kanna_signal_merge_handoff",
            json!({
                "task_id": "task-1",
                "branch": "task-task-1-4",
                "target": "main",
                "pr_url": "https://example.invalid/pull/1",
                "summary": "approved"
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/signal-merge-handoff",
            json!({
                "branch": "task-task-1-4",
                "target": "main",
                "prUrl": "https://example.invalid/pull/1",
                "summary": "approved"
            }),
        ),
        (
            "kanna_signal_agent",
            json!({
                "repo_id": "repo-1",
                "agent": "merge",
                "message": "MERGE task-1 -> main: ready",
                "agent_provider": "claude",
                "effort": "high"
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos/repo-1/agents/merge/signal",
            json!({
                "message": "MERGE task-1 -> main: ready",
                "agentProvider": "claude",
                "effort": "high"
            }),
        ),
        (
            "kanna_send_task_input",
            json!({ "task_id": "task-1", "input": "continue" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/input",
            json!({ "input": "continue" }),
        ),
        (
            "kanna_send_task_raw_input",
            json!({ "task_id": "task-1", "keys": ["down", "enter"] }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/raw-input",
            json!({ "keys": ["down", "enter"] }),
        ),
        (
            "kanna_send_task_raw_input",
            json!({ "task_id": "task-1", "bytes": "1b5b42" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/raw-input",
            json!({ "bytes": "1b5b42" }),
        ),
        (
            "kanna_close_task",
            json!({ "task_id": "task-1" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/close",
            json!({}),
        ),
        (
            "kanna_confirm_event_channel",
            json!({ "task_id": "task-1", "channel_id": "channel-7" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/claude-channel/confirm",
            json!({ "channelId": "channel-7" }),
        ),
        (
            "kanna_rename_task",
            json!({ "task_id": "task 1", "display_name": "Renamed task" }),
            Method::Patch,
            ResponseKind::Json,
            "/v1/tasks/task%201",
            json!({ "displayName": "Renamed task" }),
        ),
        (
            "kanna_advance_stage",
            json!({ "task_id": "task-1", "source": "manager" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/advance-stage",
            json!({ "source": "manager" }),
        ),
        (
            "kanna_rerun_stage",
            json!({ "task_id": "task-1" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/rerun-stage",
            json!({}),
        ),
        (
            "kanna_resume_task",
            json!({ "task_id": "task-1" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/resume",
            json!({}),
        ),
        (
            "kanna_block_task",
            json!({ "task_id": "task-1", "blocker_task_ids": ["blocker-1"] }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/block",
            json!({ "blockerTaskIds": ["blocker-1"] }),
        ),
        (
            "kanna_unblock_task",
            json!({ "task_id": "task-1" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/unblock",
            json!({}),
        ),
        (
            "kanna_set_task_parent",
            json!({ "task_id": "task-1", "parent_task_id": "task-parent" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/set-parent",
            json!({ "parentTaskId": "task-parent" }),
        ),
        (
            "kanna_set_task_parent",
            json!({ "task_id": "task-1" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/set-parent",
            json!({}),
        ),
        (
            "kanna_notify_mobile",
            json!({
                "title": "Staging shipped",
                "body": "The staging build is ready.",
                "task_id": "task-1"
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/mobile/notifications",
            json!({
                "title": "Staging shipped",
                "body": "The staging build is ready.",
                "taskId": "task-1"
            }),
        ),
        (
            "kanna_set_task_workflow",
            json!({ "task_id": "task-child", "workflow_name": "single-reviewer" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-child/actions/set-workflow",
            json!({ "workflowName": "single-reviewer" }),
        ),
        (
            "kanna_is_dependent_tasks_exist",
            json!({ "task_id": "task-1" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/tasks/task-1/dependent-tasks-exist",
            json!({}),
        ),
        (
            "kanna_complete_stage",
            json!({
                "task_id": "task-1",
                "status": "success",
                "summary": "done",
                "metadata": { "review": "passed" }
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/complete-stage",
            json!({
                "status": "success",
                "summary": "done",
                "metadata": { "review": "passed" }
            }),
        ),
        (
            "kanna_request_revision",
            json!({
                "task_id": "task-1",
                "summary": "needs work",
                "prompt": "fix it"
            }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/request-revision",
            json!({
                "targetStage": "in progress",
                "summary": "needs work",
                "prompt": "fix it"
            }),
        ),
        (
            "kanna_list_transfer_peers",
            json!({}),
            Method::Get,
            ResponseKind::Json,
            "/v1/transfers/peers",
            json!({}),
        ),
        (
            "kanna_task_transfers",
            json!({ "task_id": "task-1" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/tasks/task-1/transfers",
            json!({}),
        ),
        (
            "kanna_push_task",
            json!({ "task_id": "task-1", "to_machine": "desktop-studio", "transport": "lan" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/actions/push-to-peer",
            json!({ "targetMachine": "desktop-studio", "transport": "lan" }),
        ),
        (
            "kanna_pull_task",
            json!({ "source_task_id": "task-1", "from_machine": "peer-primary" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/transfers/actions/pull-task",
            json!({ "sourceTaskId": "task-1", "sourceMachine": "peer-primary" }),
        ),
        (
            "kanna_publish_artifact",
            json!({ "task_id": "task-1", "path": "mockups/login", "kind": "mockup", "previous": "0123456789abcdef0123456789abcdef01234567" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/tasks/task-1/artifacts",
            json!({ "path": "mockups/login", "kind": "mockup", "previous": "0123456789abcdef0123456789abcdef01234567" }),
        ),
        (
            "kanna_get_artifact",
            json!({ "repo_id": "repo-1", "artifact_id": "0123456789abcdef0123456789abcdef01234567" }),
            Method::Get,
            ResponseKind::Json,
            "/v1/repos/repo-1/artifacts/0123456789abcdef0123456789abcdef01234567",
            json!({}),
        ),
        (
            "kanna_open_artifact",
            json!({ "repo_id": "repo-1", "artifact_id": "0123456789abcdef0123456789abcdef01234567" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos/repo-1/artifacts/0123456789abcdef0123456789abcdef01234567/preview",
            json!({}),
        ),
        (
            "kanna_close_artifact",
            json!({ "repo_id": "repo-1", "artifact_id": "0123456789abcdef0123456789abcdef01234567" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos/repo-1/artifacts/0123456789abcdef0123456789abcdef01234567/preview/close",
            json!({}),
        ),
        (
            "kanna_record_artifact_comment",
            json!({ "repo_id": "repo-1", "artifact_id": "0123456789abcdef0123456789abcdef01234567", "author": "designer", "body": "too dark", "anchor": { "path": "css/site.css", "excerpt": "#123" } }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos/repo-1/artifacts/0123456789abcdef0123456789abcdef01234567/comments",
            json!({ "author": "designer", "body": "too dark", "anchor": { "path": "css/site.css", "excerpt": "#123" } }),
        ),
        (
            "kanna_record_artifact_decision",
            json!({ "repo_id": "repo-1", "artifact_id": "0123456789abcdef0123456789abcdef01234567", "who": "owner", "what": "ship v2" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos/repo-1/artifacts/0123456789abcdef0123456789abcdef01234567/decisions",
            json!({ "who": "owner", "what": "ship v2" }),
        ),
        (
            "kanna_push_artifact",
            json!({ "repo_id": "repo-1", "artifact_id": "0123456789abcdef0123456789abcdef01234567" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos/repo-1/artifacts/0123456789abcdef0123456789abcdef01234567/push",
            json!({}),
        ),
        (
            "kanna_fetch_artifact",
            json!({ "repo_id": "repo-1", "artifact_id": "0123456789abcdef0123456789abcdef01234567" }),
            Method::Post,
            ResponseKind::Json,
            "/v1/repos/repo-1/artifacts/0123456789abcdef0123456789abcdef01234567/fetch",
            json!({}),
        ),
    ];

    for (name, args, method, kind, path, body) in cases {
        let request = resolve_request(&catalog, name, &args).expect(name);
        assert_eq!(request.method, method, "{name}");
        assert_eq!(request.kind, kind, "{name}");
        assert_eq!(request.path, path, "{name}");
        assert_eq!(request.body, body, "{name}");
        assert_eq!(request.machine_id, None, "{name}");
    }

    let routed = resolve_request(
        &catalog,
        "kanna_get_task",
        &json!({ "machine_id": "desktop-studio", "task_id": "task-1" }),
    )
    .expect("routed task");
    assert_eq!(routed.machine_id.as_deref(), Some("desktop-studio"));
    assert_eq!(routed.path, "/v1/tasks/task-1?agentView=true");
    assert_eq!(routed.body, json!({}));

    let wait = resolve_request(
        &catalog,
        "kanna_wait_task",
        &json!({ "task_id": "task 1", "timeout_secs": 999, "poll_secs": 0, "until": "closed" }),
    )
    .expect("wait task");
    assert_eq!(wait.kind, ResponseKind::Wait);
    assert_eq!(wait.method, Method::Get);
    assert_eq!(wait.path, "/v1/tasks/task%201");
    let wait_spec = wait.wait.expect("wait spec");
    assert_eq!(wait_spec.task_id, "task 1");
    assert_eq!(wait_spec.timeout_secs, MAX_WAIT_TIMEOUT_SECS);
    assert_eq!(wait_spec.poll_secs, 1);
    assert_eq!(wait_spec.until, WaitUntil::Closed);
}

#[test]
fn revision_origin_is_optional_declared_provenance_with_a_closed_vocabulary() {
    let catalog = bundled_catalog();
    let origin = catalog
        .find_param("kanna_request_revision", "origin")
        .expect("revision origin parameter");
    assert_eq!(origin.param_type, ParamType::String);
    assert_eq!(origin.location, ParamLoc::Body);
    assert!(!origin.required);
    assert_eq!(
        origin.enum_values.as_deref(),
        Some(&["agent".to_string(), "human".to_string()][..])
    );
    let description = origin.description.as_deref().expect("origin description");
    assert!(description.contains("explicit human instruction"));
    assert!(description.contains("not authenticated human identity"));

    let request = resolve_request(
        &catalog,
        "kanna_request_revision",
        &json!({
            "task_id": "task-1",
            "summary": "continue review",
            "prompt": "Fix the remaining finding.",
            "origin": "human",
        }),
    )
    .expect("human-authorized revision request");
    assert_eq!(
        request.body,
        json!({
            "targetStage": "in progress",
            "summary": "continue review",
            "prompt": "Fix the remaining finding.",
            "origin": "human",
        })
    );

    let schema = catalog.tools_list_value();
    let revision = schema
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "kanna_request_revision")
        .expect("request revision schema");
    assert_eq!(
        revision["inputSchema"]["properties"]["origin"]["enum"],
        json!(["agent", "human"])
    );

    let error = resolve_request(
        &catalog,
        "kanna_request_revision",
        &json!({
            "task_id": "task-1",
            "summary": "continue review",
            "prompt": "Fix the remaining finding.",
            "origin": "owner",
        }),
    )
    .expect_err("unknown provenance must fail before HTTP");
    assert!(
        error.contains("origin must be one of agent, human"),
        "{error}"
    );
}

/// The transfer surface's whole hazard is that its calls succeed long before
/// anything moves. A manager once read a `scheduled: true` from the raw server
/// API as a completed move while the transfer was dying on a relay socket, so
/// every one of these declarations has to say outright that it schedules work,
/// and has to name the surface that answers the real question.
#[test]
fn transfer_tools_refuse_to_read_as_a_completed_move() {
    let catalog = bundled_catalog();
    let describe = |name: &str| {
        catalog
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} must be a catalog tool"))
            .description
            .clone()
    };

    for name in ["kanna_push_task", "kanna_pull_task"] {
        let description = describe(name);
        assert!(
            description.contains("DOES NOT MOVE THE TASK"),
            "{name} must say so in terms nothing can skim past: {description}"
        );
        assert!(
            description.contains("kanna_task_transfers"),
            "{name} must name the surface that reports the durable outcome"
        );
        assert!(
            description.contains("moved:false"),
            "{name} must name the response field that states it"
        );
    }

    // Direction is the other thing a caller cannot guess. A push runs where the
    // task lives, so it is routable; a pull runs where the task is going, so
    // routing it elsewhere would be a different operation entirely and the
    // argument is deliberately absent.
    let push = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_push_task")
        .expect("push tool");
    assert!(push
        .params
        .iter()
        .any(|param| param.name == "machine_id" && param.location == ParamLoc::Routing));
    let pull = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_pull_task")
        .expect("pull tool");
    assert!(
        !pull.params.iter().any(|param| param.name == "machine_id"),
        "a pull always runs on the machine it moves the task to"
    );
    assert!(resolve_request(
        &catalog,
        "kanna_pull_task",
        &json!({
            "source_task_id": "task-1",
            "from_machine": "peer-primary",
            "machine_id": "desktop-studio",
        }),
    )
    .expect_err("routing a pull is not a thing")
    .contains("unknown argument: machine_id"),);

    // Destinations are named by identity the server owns, so no agent surface
    // asks for a peer's key, endpoint, or relay credential.
    for name in ["kanna_push_task", "kanna_pull_task"] {
        let tool = catalog
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("tool");
        for param in &tool.params {
            assert!(
                !param.name.contains("token")
                    && !param.name.contains("public_key")
                    && !param.name.contains("secret")
                    && !param.name.contains("endpoint"),
                "{name} must not ask an agent for transport credentials: {}",
                param.name
            );
        }
    }

    // The credential that fails a cloud transfer belongs to the signed-in
    // desktop, so the tools say renewal is bounded and keeps it out of the
    // agent surface.
    assert!(describe("kanna_list_transfer_peers").contains("signed-in desktop"));
    for name in ["kanna_push_task", "kanna_pull_task"] {
        let description = describe(name);
        assert!(description.contains("bounded"), "{name}");
        assert!(description.contains("no credential enters"), "{name}");
    }

    let transfers = describe("kanna_task_transfers");
    for state in ["pending", "completed", "failed", "rejected"] {
        assert!(
            transfers.contains(state),
            "kanna_task_transfers must document its {state} verdict"
        );
    }
}

/// The multi-task wait blocks server-side, so its window is bound by the same
/// client budget as `kanna_wait_task`: the caller's `tools/call` is what dies
/// at 300s, whichever end of the connection is doing the waiting.
#[test]
fn wait_events_is_scoped_cursored_and_bounded_by_the_client_budget() {
    let catalog = bundled_catalog();

    // The watched set is an array in the schema and comma-joined on the wire,
    // so an agent hands over the ids it holds instead of formatting a query.
    let request = resolve_request(
        &catalog,
        "kanna_wait_events",
        &json!({ "task_ids": ["task-a", "task-b"], "cursor": "42" }),
    )
    .expect("wait events");
    assert_eq!(request.method, Method::Get);
    assert_eq!(request.kind, ResponseKind::Json);
    // `include_current_activity` is no longer forced onto the wire: it is
    // cursor-implied server-side now, so an untouched request omits it
    // entirely rather than carrying a catalog-level default. shortCursor is
    // appended last now: it used to piggyback on include_current_activity's
    // position in the param loop, and with that param absent it falls
    // through to the trailing append instead.
    assert_eq!(
        request.path,
        format!("/v1/task-events?taskIds=task-a%2Ctask-b&cursor=42&timeoutSecs={DEFAULT_WAIT_TIMEOUT_SECS}&shortCursor=true")
    );
    let tools = catalog.tools_list_value();
    let schema = tools
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "kanna_wait_events")
        .expect("wait events tool")["inputSchema"]
        .clone();
    assert_eq!(
        schema["properties"]["task_ids"]["items"],
        json!({ "type": "string" }),
        "task_ids must be declared as an array of strings"
    );
    assert!(
        schema["properties"].get("short_cursor").is_none(),
        "short cursors are automatic client policy, not an agent option"
    );
    assert!(resolve_request(
        &catalog,
        "kanna_wait_events",
        &json!({ "task_ids": ["task-a"], "short_cursor": false }),
    )
    .expect_err("the removed cursor-shape option must not be accepted")
    .contains("unknown argument: short_cursor"));

    let repo_scoped = resolve_request(
        &catalog,
        "kanna_wait_events",
        &json!({ "repo_id": "repo 1", "timeout_secs": 3600, "limit": 5 }),
    )
    .expect("wait events")
    .path;
    assert_eq!(
        repo_scoped,
        format!("/v1/task-events?repoId=repo%201&timeoutSecs={MAX_WAIT_TIMEOUT_SECS}&limit=5&shortCursor=true"),
        "an over-long window must be clamped before the client can kill the call"
    );

    // The scope a fan-out can name after losing the ids it created. Without it
    // the only alternative is the whole repo, and the caller filters the noise.
    let parent_scoped = resolve_request(
        &catalog,
        "kanna_wait_events",
        &json!({ "parent_task_id": "parent 1" }),
    )
    .expect("wait events")
    .path;
    assert_eq!(
        parent_scoped,
        format!("/v1/task-events?parentTaskId=parent%201&timeoutSecs={DEFAULT_WAIT_TIMEOUT_SECS}&shortCursor=true")
    );
    let description = &catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_wait_events")
        .expect("wait events tool")
        .description;
    assert!(
        description.contains("short cursor handle")
            && description.contains("constant-size cursor")
            && description.contains("read checkpoint")
            && description.contains("never rewinds acknowledged events")
            && description.contains("without cursor growth"),
        "the parent scope must document its bounded reparenting semantics: {description}"
    );
    assert!(
        description.contains("kanna-cli task watch")
            && description.contains("clamped to 240 seconds")
            && description.contains("abort tools/call around 300 seconds"),
        "the MCP wait must route arbitrarily long watches to the CLI process: {description}"
    );
}

#[test]
fn task_session_repo_defaulting_is_shared_by_every_catalog_client() {
    let catalog = bundled_catalog();
    let current_task = json!({ "id": "manager-1", "repoId": "repo-current" });

    for (tool, args, expected_path) in [
        (
            "kanna_get_tasks",
            json!({}),
            "/v1/tasks?repoId=repo-current&sortBy=updatedAt&order=desc&limit=50",
        ),
        (
            "kanna_list_recent_tasks",
            json!({}),
            "/v1/tasks/recent?repoId=repo-current",
        ),
        (
            "kanna_search_tasks",
            json!({ "query": "review" }),
            "/v1/tasks/search?query=review&repoId=repo-current",
        ),
        (
            // `include_current_activity` is cursor-implied server-side now
            // and no longer forced onto the wire by the catalog, so an
            // untouched request omits it entirely.
            "kanna_wait_events",
            json!({ "from": "now", "timeout_secs": 0 }),
            "/v1/task-events?repoId=repo-current&from=now&timeoutSecs=0&shortCursor=true",
        ),
    ] {
        assert_eq!(
            repo_context_task_id(tool, &args, Some("manager-1"), None),
            Ok(Some("manager-1".to_string()))
        );
        assert_eq!(
            resolve_request_with_repo_context(&catalog, tool, &args, Some(&current_task))
                .expect("resolve inferred repository request")
                .path,
            expected_path
        );
    }

    let create_args = json!({ "prompt": "child work" });
    assert_eq!(
        repo_context_task_id("kanna_create_task", &create_args, Some("manager-1"), None),
        Ok(Some("manager-1".to_string()))
    );
    assert_eq!(
        resolve_request_with_repo_context(
            &catalog,
            "kanna_create_task",
            &create_args,
            Some(&current_task)
        )
        .expect("resolve inferred create")
        .body["repoId"],
        "repo-current"
    );
}

#[test]
fn explicit_repository_and_machine_wide_scopes_win_over_task_context() {
    let catalog = bundled_catalog();
    let explicit = resolve_request_with_repo_context(
        &catalog,
        "kanna_wait_events",
        &json!({ "repo_id": "repo-explicit" }),
        Some(&json!({ "repoId": "repo-current" })),
    )
    .expect("resolve explicit repository");
    assert!(explicit.path.contains("repoId=repo-explicit"));

    for (tool, args) in [
        ("kanna_wait_events", json!({ "repo_id": "repo-explicit" })),
        (
            "kanna_wait_events",
            json!({ "repo_remote_url_hash": "remote-hash" }),
        ),
        ("kanna_wait_events", json!({ "task_ids": ["task-a"] })),
        ("kanna_wait_events", json!({ "parent_task_id": "parent-a" })),
        ("kanna_list_recent_tasks", json!({ "all_repos": true })),
        ("kanna_get_tasks", json!({ "all_repos": true })),
        (
            "kanna_search_tasks",
            json!({ "query": "x", "all_machines": true }),
        ),
    ] {
        assert_eq!(
            repo_context_task_id(tool, &args, Some("manager-1"), None),
            Ok(None),
            "{tool} should preserve its explicit scope"
        );
    }

    let error = repo_context_task_id(
        "kanna_wait_events",
        &json!({}),
        Some("manager-1"),
        Some("desktop-peer"),
    )
    .expect_err("a local task repo id cannot scope a remote machine");
    assert!(error.contains("repo_id is required"));
    assert!(error.contains("desktop-peer"));
    assert!(error.contains("repository IDs are machine-local"));
}

/// `parentTaskId` upward and `childTaskIds` downward are the same relation read
/// from both ends. An agent that only ever hears about the upward half has no
/// way back to a child it forgot, so the tool description has to name the
/// downward half and say that closed children are in it.
#[test]
fn get_task_documents_the_downward_view_of_parentage() {
    let catalog = bundled_catalog();
    let description = &catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_get_task")
        .expect("get task tool")
        .description;

    assert!(
        description.contains("childTaskIds"),
        "kanna_get_task must document childTaskIds: {description}"
    );
    assert!(
        description.contains("closed"),
        "kanna_get_task must say closed children are included, or an empty list \
         reads as 'nothing was dispatched': {description}"
    );
}

/// An agent supervising tasks reads only the tool description before deciding
/// which field answers "is this task alive?". Naming `activity` without saying
/// what it blends is what produced false quiet alarms against running agents.
#[test]
fn get_task_documents_both_state_dimensions_and_which_one_means_alive() {
    let catalog = bundled_catalog();
    let description = &catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_get_task")
        .expect("get task tool")
        .description;

    for field in ["runtimeState", "readState", "activity"] {
        assert!(
            description.contains(field),
            "kanna_get_task must document {field}: {description}"
        );
    }
    for value in ["busy", "waiting", "idle", "exited"] {
        assert!(
            description.contains(value),
            "kanna_get_task must name the runtime value {value}: {description}"
        );
    }
    assert!(
        description.contains("blend"),
        "kanna_get_task must say activity blends the two dimensions rather than          reporting either: {description}"
    );
}

/// The tool description is the only documentation an agent reads before
/// deciding whether the feed answers its question, so every event type the
/// server can emit has to be named there.
#[test]
fn wait_events_documents_every_event_type_the_server_emits() {
    let catalog = bundled_catalog();
    let description = &catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_wait_events")
        .expect("wait events tool")
        .description;

    for event_type in [
        "task.created",
        "run.started",
        "run.finished",
        "stage.changed",
        "task.pr_created",
        "task.revision_requested",
        "task.closed",
        "task.awaiting_input",
        "task.runtime_changed",
        "task.runtime_settled",
        "task.activity_changed",
        "task.blocked",
        "task.unblocked",
        "task.merge_signaled",
        "task.merge_handoff_missing",
        "task.input_delivered",
        "task.raw_input_delivered",
        "task.transfer_finalizing",
        "task.provider_quota_rejected",
        "task.provider_quota_parked",
        "task.provider_capacity_refused",
        "task.review_context_changed",
        "task.human_review_decision",
        "task.human_review_decision_delivery",
    ] {
        assert!(
            description.contains(event_type),
            "kanna_wait_events must document the {event_type} event"
        );
    }

    assert!(
        description.contains("previousRuntimeState, runtimeState and latestRunFinishedWithoutCompletion")
            && description.contains("never encodes human read/unread state")
            && description.contains("exclude_event_types"),
        "kanna_wait_events must point managers at the runtime signal and how to drop display noise: {description}"
    );

    assert!(
        description.contains("server-debounced")
            && description.contains("every activity direction")
            && description.contains("latestRunFinishedWithoutCompletion")
            && description.contains("no waiting-prompt placeholder")
            && description.contains("follow-up polling"),
        "kanna_wait_events must document the complete provider-neutral settled activity contract: {description}"
    );
}

// The window-vs-client-budget invariant itself is a compile-time assertion in
// the crate: a wait longer than the client's tools/call timeout is killed
// before it can answer, so it must not be expressible.

#[test]
fn wait_defaults_to_the_bounded_window_without_arguments() {
    let catalog = bundled_catalog();

    let wait = resolve_request(&catalog, "kanna_wait_task", &json!({ "task_id": "task-1" }))
        .expect("wait task")
        .wait
        .expect("wait spec");

    assert_eq!(wait.timeout_secs, DEFAULT_WAIT_TIMEOUT_SECS);
    assert!(wait.timeout_secs < CLIENT_TOOL_CALL_BUDGET_SECS);
    assert_eq!(wait.until, WaitUntil::Reconcile);
}

/// The cap lives in code, not only in `catalog.json`: `.kanna/mcp-tools.json`
/// overrides the bundled catalog, and an override that asks for a window the
/// client will kill must still be clamped.
#[test]
fn override_catalog_cannot_reintroduce_an_unsurvivable_wait_window() {
    let catalog: Catalog = serde_json::from_str(
        r#"{
          "tools": [{
            "name": "kanna_wait_task",
            "description": "Wait",
            "method": "GET",
            "path": "/v1/tasks/{task_id}",
            "response": "wait",
            "params": [
              { "name": "task_id", "description": "Task id.", "type": "string", "required": true, "location": "path" },
              { "name": "timeout_secs", "description": "Seconds.", "type": "integer", "required": false, "location": "body", "default": 3600, "max": 3600 }
            ]
          }]
        }"#,
    )
    .expect("override catalog parses");

    let defaulted = resolve_request(&catalog, "kanna_wait_task", &json!({ "task_id": "task-1" }))
        .expect("wait task")
        .wait
        .expect("wait spec");
    let explicit = resolve_request(
        &catalog,
        "kanna_wait_task",
        &json!({ "task_id": "task-1", "timeout_secs": 3600 }),
    )
    .expect("wait task")
    .wait
    .expect("wait spec");

    assert_eq!(defaulted.timeout_secs, MAX_WAIT_TIMEOUT_SECS);
    assert_eq!(explicit.timeout_secs, MAX_WAIT_TIMEOUT_SECS);
    assert_eq!(clamp_wait_timeout_secs(3600), MAX_WAIT_TIMEOUT_SECS);
    assert_eq!(clamp_wait_timeout_secs(30), 30);
}

#[test]
fn wait_results_carry_the_task_detail_and_an_outcome_discriminator() {
    let task = json!({ "id": "task-1", "stage": "review", "activity": "running" });

    let resolved = wait_resolved_result(task.clone());
    assert_eq!(resolved["waitOutcome"], json!("resolved"));
    assert_eq!(resolved["id"], json!("task-1"));
    assert_eq!(resolved["stage"], json!("review"));
    assert!(resolved["waitHint"].is_null());

    let timed_out = wait_timeout_result(task, "task-1", MAX_WAIT_TIMEOUT_SECS);
    assert_eq!(timed_out["waitOutcome"], json!("timeout"));
    assert_eq!(timed_out["waitTimeoutSecs"], json!(MAX_WAIT_TIMEOUT_SECS));
    assert_eq!(
        timed_out["id"],
        json!("task-1"),
        "a timed-out wait must still hand back the task state it polled"
    );
    assert_eq!(timed_out["stage"], json!("review"));
    let hint = timed_out["waitHint"].as_str().expect("wait hint");
    assert!(hint.contains("call kanna_wait_task again"), "{hint}");
}

#[test]
fn create_task_preserves_parent_for_genuine_dispatch_fan_out() {
    let catalog = bundled_catalog();
    let request = resolve_request(
        &catalog,
        "kanna_create_task",
        &json!({
            "repo_id": "repo-1",
            "prompt": "Specialty review dispatched from task parent-1.",
            "workflow_name": "specialty-review",
            "agent": "review-security",
            "base_ref": "task-parent-1-2",
            "parent_task_id": "parent-1"
        }),
    )
    .expect("dispatcher-style create-task call resolves");

    assert_eq!(request.method, Method::Post);
    assert_eq!(request.path, "/v1/tasks");
    assert_eq!(
        request.body,
        json!({
            "repoId": "repo-1",
            "prompt": "Specialty review dispatched from task parent-1.",
            "workflowName": "specialty-review",
            "agent": "review-security",
            "baseRef": "task-parent-1-2",
            "parentTaskId": "parent-1"
        })
    );
}

#[test]
fn create_task_rejects_undeclared_stage_override_argument() {
    let catalog = bundled_catalog();
    let err = resolve_request(
        &catalog,
        "kanna_create_task",
        &json!({
            "repo_id": "repo-1",
            "prompt": "Jump to PR",
            "stage": "pr"
        }),
    )
    .expect_err("stage should not be accepted by agent-facing create-task tools");

    assert!(err.contains("unknown argument: stage"));
}

#[test]
fn preserves_validation_error_strings() {
    let catalog = bundled_catalog();

    assert_eq!(
        resolve_request(&catalog, "kanna_search_tasks", &json!({})),
        Err("missing required argument: query".to_string())
    );
    assert_eq!(
        resolve_request(
            &catalog,
            "kanna_create_task",
            &json!({ "repo_id": "repo-1", "prompt": "x", "allowed_tools": [1] })
        ),
        Err("allowed_tools must be an array of strings".to_string())
    );
    assert_eq!(
        resolve_request(
            &catalog,
            "kanna_task_logs",
            &json!({ "task_id": "task-1", "tail": "25" })
        ),
        Err("tail must be an unsigned integer".to_string())
    );
    assert_eq!(
        resolve_request(
            &catalog,
            "kanna_rename_task",
            &json!({ "task_id": "task-1" })
        ),
        Err("missing required argument: display_name".to_string())
    );
    assert_eq!(
        resolve_request(
            &catalog,
            "kanna_complete_stage",
            &json!({ "task_id": "task-1", "status": "maybe", "summary": "done" })
        ),
        Err(
            "status must be one of success, unverified, partial, needs-input, declined, failure, \
             got maybe"
                .to_string()
        )
    );
    assert_eq!(
        resolve_request(
            &catalog,
            "kanna_wait_task",
            &json!({ "task_id": "task-1", "until": "later" })
        ),
        Err("until must be reconcile, finished or closed, got later".to_string())
    );
    let unknown_tool = resolve_request(&catalog, "kanna_unknown", &json!({}))
        .expect_err("unknown tool should fail");
    assert!(unknown_tool.starts_with("unknown tool: kanna_unknown"));
    assert!(
        unknown_tool.contains(
            "available tools: kanna_info, kanna_list_machines, kanna_guide, \
             kanna_subscribe_events, kanna_read_event_subscription, kanna_unsubscribe_events, kanna_machine_stats, kanna_list_transfer_peers, kanna_list_repos,"
        ),
        "unknown tool error should list available tools: {unknown_tool}"
    );
}

#[test]
fn type_mismatch_and_unknown_argument_errors_are_actionable() {
    let catalog = bundled_catalog();

    assert_eq!(
        resolve_request(&catalog, "kanna_get_task", &json!({ "task_id": 7 })),
        Err("task_id must be a string".to_string())
    );

    let unknown_arg = resolve_request(
        &catalog,
        "kanna_close_task",
        &json!({ "task_id": "task-1", "force": true }),
    )
    .expect_err("unknown argument should fail");
    assert_eq!(
        unknown_arg,
        "unknown argument: force (kanna_close_task accepts: machine_id, task_id)"
    );
}

#[test]
fn load_catalog_uses_override_and_falls_back_with_warning() {
    let root = std::env::temp_dir().join(format!("kanna-tool-catalog-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join(".kanna")).expect("create .kanna");
    let override_path = root.join(".kanna/mcp-tools.json");
    fs::write(
        &override_path,
        r#"{
          "tools": [{
            "name": "kanna_test_tool",
            "description": "Test tool",
            "method": "GET",
            "path": "/v1/test",
            "response": "json",
            "params": []
          }]
        }"#,
    )
    .expect("write override");

    let loaded = kanna_tool_catalog::load_catalog(&root);
    assert_eq!(loaded.catalog.tools[0].name, "kanna_info");
    assert_eq!(loaded.catalog.tools[1].name, "kanna_list_machines");
    assert_eq!(loaded.catalog.tools[2].name, "kanna_guide");
    assert_eq!(loaded.catalog.tools[3].name, "kanna_test_tool");
    assert_eq!(
        loaded.catalog.guide_topics(),
        vec!["config", "workflows", "agents", "tasks", "mobile"]
    );
    assert_eq!(
        loaded.watch_source.as_deref(),
        Some(override_path.as_path())
    );
    assert_eq!(loaded.warning, None);

    fs::write(&override_path, "{").expect("write invalid override");
    let loaded = kanna_tool_catalog::load_catalog(&root);
    assert!(loaded.warning.expect("warning").contains("failed to parse"));
    assert_eq!(loaded.catalog.tools[0].name, "kanna_info");
    assert_eq!(
        loaded.watch_source.as_deref(),
        Some(override_path.as_path())
    );

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn catalog_override_cannot_replace_safe_kanna_info_declaration() {
    let root = std::env::temp_dir().join(format!(
        "kanna-tool-catalog-info-override-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join(".kanna")).expect("create .kanna");
    fs::write(
        root.join(".kanna/mcp-tools.json"),
        r#"{
          "tools": [{
            "name": "kanna_info",
            "description": "Unsafe raw status passthrough",
            "method": "GET",
            "path": "/v1/status",
            "response": "json",
            "params": [{
              "name": "leak",
              "type": "string",
              "required": false,
              "location": "query"
            }]
          }]
        }"#,
    )
    .expect("write override");

    let loaded = kanna_tool_catalog::load_catalog(&root);
    let info = loaded.catalog.tools.first().expect("required info tool");
    assert_eq!(info.name, "kanna_info");
    assert_eq!(info.path, "/v1/status");
    assert_eq!(info.response_kind, ResponseKind::RuntimeInfo);
    assert_eq!(info.params.len(), 1);
    assert_eq!(info.params[0].name, "machine_id");
    assert_eq!(info.params[0].location, ParamLoc::Routing);
    assert!(info.description.contains("authoritative server"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn catalog_types_are_deserialized_from_manifest_values() {
    let catalog = bundled_catalog();
    let create_task = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_create_task")
        .expect("create task");
    let params = create_task
        .params
        .iter()
        .map(|param| {
            (
                param.name.as_str(),
                param.param_type,
                param.location,
                param.key.as_deref(),
            )
        })
        .collect::<Vec<_>>();

    assert!(params.contains(&("repo_id", ParamType::String, ParamLoc::Body, Some("repoId"))));
    assert!(params.contains(&(
        "display_name",
        ParamType::String,
        ParamLoc::Body,
        Some("displayName"),
    )));
    assert!(!params.iter().any(|(name, _, _, _)| *name == "agent_type"));
    assert!(params.contains(&(
        "blocker_task_ids",
        ParamType::StringArray,
        ParamLoc::Body,
        Some("blockerTaskIds"),
    )));
    assert!(params.contains(&(
        "parent_task_id",
        ParamType::String,
        ParamLoc::Body,
        Some("parentTaskId"),
    )));
}

#[test]
fn display_name_documents_the_prompt_fallback_rather_than_a_derivation() {
    // Nothing derives a title from the prompt: an omitted display_name leaves
    // the task titled by the prompt text itself. Describing it as a derivation
    // is what made template-driven fan-outs (the QA dispatcher's specialty
    // children) safe-looking to dispatch unnamed, and they all rendered alike.
    let catalog = bundled_catalog();
    let description = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_create_task")
        .expect("create task")
        .params
        .iter()
        .find(|param| param.name == "display_name")
        .expect("display_name param")
        .description
        .clone()
        .expect("display_name description");

    assert!(
        description.contains("falls back to the prompt text"),
        "display_name must document the prompt fallback: {description}"
    );
    assert!(
        !description.contains("derived from the prompt"),
        "display_name must not promise a derivation: {description}"
    );
}

#[test]
fn task_creation_guidance_uses_wait_surfaces_and_semantic_hierarchy() {
    let catalog = bundled_catalog();
    let create = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_create_task")
        .expect("create task");
    let description = create.description.as_str();
    let parent = create
        .params
        .iter()
        .find(|param| param.name == "parent_task_id")
        .and_then(|param| param.description.as_deref())
        .expect("parent_task_id description");
    assert!(description.contains("Ordinary durable repository work is top-level by default"));
    assert!(description.contains("Observe completion through kanna_wait_events"));
    assert!(parent.contains("genuine semantic subtask"));
    assert!(parent.contains("Omit for ordinary top-level work"));
    assert!(create
        .params
        .iter()
        .all(|param| param.name != "notify_task_id"));

    let error = resolve_request(
        &catalog,
        "kanna_create_task",
        &json!({
            "repo_id": "repo-1",
            "prompt": "Investigate flaky staging release",
            "notify_task_id": "task-manager-1"
        }),
    )
    .expect_err("retired notify_task_id must not resolve");
    assert!(
        error.contains("unknown argument: notify_task_id"),
        "{error}"
    );
}

/// The catalog, not the shape of the text, decides a command-line argument's
/// type. Every declared parameter must round-trip its own CLI spelling: a
/// string stays a string however numeric it looks, and an integer is still an
/// integer when it arrives as text.
#[test]
fn declared_types_decide_cli_argument_parsing_not_the_text() {
    let catalog = bundled_catalog();

    let task_id = catalog
        .find_param("kanna_get_task", "task_id")
        .expect("task_id param");
    assert_eq!(task_id.param_type, ParamType::String);
    assert_eq!(
        task_id.parse_cli_value("57808275").unwrap(),
        json!("57808275")
    );
    assert_eq!(
        task_id.parse_cli_value("5ad2bc89").unwrap(),
        json!("5ad2bc89")
    );
    // A value that parses as JSON of another type is still just text.
    assert_eq!(task_id.parse_cli_value("true").unwrap(), json!("true"));
    assert_eq!(
        task_id.parse_cli_value("{\"a\":1}").unwrap(),
        json!("{\"a\":1}")
    );

    let timeout = catalog
        .find_param("kanna_wait_task", "timeout_secs")
        .expect("timeout_secs param");
    assert_eq!(timeout.param_type, ParamType::Integer);
    assert_eq!(timeout.parse_cli_value("30").unwrap(), json!(30));
    assert_eq!(
        timeout.parse_cli_value("soon").unwrap_err(),
        "timeout_secs must be an unsigned integer, got soon"
    );

    let blockers = catalog
        .find_param("kanna_block_task", "blocker_task_ids")
        .expect("blocker_task_ids param");
    assert_eq!(blockers.param_type, ParamType::StringArray);
    assert_eq!(
        blockers.parse_cli_value("1234, ab12cd").unwrap(),
        json!(["1234", "ab12cd"])
    );
    assert_eq!(
        blockers.parse_cli_value(r#"["1234","ab12cd"]"#).unwrap(),
        json!(["1234", "ab12cd"])
    );
    assert!(blockers
        .parse_cli_value("[1234]")
        .unwrap_err()
        .contains("array of strings"));

    let metadata = catalog
        .find_param("kanna_complete_stage", "metadata")
        .expect("metadata param");
    assert_eq!(metadata.param_type, ParamType::Object);
    assert_eq!(
        metadata
            .parse_cli_value(r#"{"pr_url":"https://example.invalid/pull/1"}"#)
            .unwrap(),
        json!({ "pr_url": "https://example.invalid/pull/1" })
    );
    assert!(metadata
        .parse_cli_value("nope")
        .unwrap_err()
        .contains("must be a JSON object"));

    assert!(catalog.find_param("kanna_get_task", "depth").is_none());
    assert!(catalog
        .find_param("kanna_no_such_tool", "task_id")
        .is_none());
}

/// Every parameter the catalog declares must survive its own CLI spelling and
/// then pass `resolve_request` — the check that failed for all-digit task ids.
#[test]
fn every_declared_parameter_round_trips_a_cli_spelling() {
    for tool in bundled_catalog().tools {
        for param in &tool.params {
            let raw = match param.param_type {
                ParamType::String => param
                    .enum_values
                    .as_ref()
                    .and_then(|values| values.first().cloned())
                    .unwrap_or_else(|| "57808275".to_string()),
                ParamType::Integer => "7".to_string(),
                ParamType::Boolean => "true".to_string(),
                ParamType::StringArray => param
                    .enum_values
                    .as_ref()
                    .and_then(|values| values.first().cloned())
                    .unwrap_or_else(|| "57808275".to_string()),
                ParamType::Object => "{}".to_string(),
            };
            let value = param
                .parse_cli_value(&raw)
                .unwrap_or_else(|e| panic!("{}.{} rejected {raw}: {e}", tool.name, param.name));
            let expected_type_ok = match param.param_type {
                ParamType::String => value.is_string(),
                ParamType::Integer => value.is_u64(),
                ParamType::Boolean => value.is_boolean(),
                ParamType::StringArray => value.is_array(),
                ParamType::Object => value.is_object(),
            };
            assert!(
                expected_type_ok,
                "{}.{} parsed {raw} as {value}",
                tool.name, param.name
            );
        }

        let args = tool
            .params
            .iter()
            .filter(|param| !matches!(param.name.as_str(), "harness" | "next_stage_harness"))
            .map(|param| {
                let raw = match param.param_type {
                    ParamType::String => param
                        .enum_values
                        .as_ref()
                        .and_then(|values| values.first().cloned())
                        .unwrap_or_else(|| "57808275".to_string()),
                    ParamType::Integer => "7".to_string(),
                    ParamType::Boolean => "true".to_string(),
                    ParamType::StringArray => param
                        .enum_values
                        .as_ref()
                        .and_then(|values| values.first().cloned())
                        .unwrap_or_else(|| "57808275".to_string()),
                    ParamType::Object => "{}".to_string(),
                };
                (
                    param.name.clone(),
                    param.parse_cli_value(&raw).expect("cli value"),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        resolve_request(
            &bundled_catalog(),
            &tool.name,
            &serde_json::Value::Object(args),
        )
        .unwrap_or_else(|e| panic!("{} rejected its own CLI spelling: {e}", tool.name));
    }
}

/// The wait predicate that `kanna_wait_task` resolves on. `Finished` means a
/// termination was recorded — never that a display value happens to read
/// `unread`, which an actively working task whose output nobody read also
/// carries.
#[test]
fn finished_is_decided_by_a_recorded_termination_not_the_activity_flag() {
    let finished_but_idle = json!({
        "activity": "idle",
        "runtimeState": "idle",
        "closedAt": null,
        "latestRun": { "status": "failed" },
    });
    assert!(
        task_value_matches_wait_until(&finished_but_idle, WaitUntil::Finished),
        "a terminal stage run means finished whatever activity settled to"
    );
    assert!(
        !task_value_matches_wait_until(&finished_but_idle, WaitUntil::Closed),
        "finished is not closed"
    );

    // `idle` on its own is also what a task that has not started its first run
    // looks like, so it must not resolve by itself — on either dimension.
    let never_started = json!({
        "activity": "idle",
        "runtimeState": null,
        "closedAt": null,
        "latestRun": null,
    });
    assert!(!task_value_matches_wait_until(
        &never_started,
        WaitUntil::Finished
    ));
    let parked_at_composer = json!({
        "activity": "idle",
        "runtimeState": "idle",
        "closedAt": null,
        "latestRun": null,
    });
    assert!(!task_value_matches_wait_until(
        &parked_at_composer,
        WaitUntil::Finished
    ));

    let still_running = json!({
        "activity": "working",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
    });
    assert!(!task_value_matches_wait_until(
        &still_running,
        WaitUntil::Finished
    ));

    // `cancelled` is the transient state a rerun, resume, or close passes
    // through on the way to a replacement run, so it is not terminal.
    let cancelled = json!({
        "activity": "idle",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "cancelled" },
    });
    assert!(!task_value_matches_wait_until(
        &cancelled,
        WaitUntil::Finished
    ));

    // The regression this predicate exists to prevent: an agent that is busy
    // inside a long call, whose latest output the operator has not read. The
    // display value says `unread` on both, and only the runtime dimension
    // tells them apart.
    let busy_but_unread = json!({
        "activity": "unread",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
    });
    assert!(
        !task_value_matches_wait_until(&busy_but_unread, WaitUntil::Finished),
        "unread is read state; a busy agent has not finished"
    );

    // An agent session that ended without recording a verdict is a
    // termination, and is what the old `unread` clause was standing in for. An
    // agent that parks without its process ending is not covered by it — see
    // `task_state_matches_wait_until`.
    let session_exited = json!({
        "activity": "unread",
        "runtimeState": "exited",
        "closedAt": null,
        "latestRun": { "status": "cancelled" },
    });
    assert!(task_value_matches_wait_until(
        &session_exited,
        WaitUntil::Finished
    ));
    assert!(!task_value_matches_wait_until(
        &session_exited,
        WaitUntil::Closed
    ));

    let closed = json!({ "activity": "idle", "closedAt": "2026-08-13T22:00:00Z" });
    assert!(task_value_matches_wait_until(&closed, WaitUntil::Finished));
    assert!(task_value_matches_wait_until(&closed, WaitUntil::Closed));
}

/// `Reconcile` — the default `until` — resolves the instant a task becomes
/// blocked, badged, provider-parked, or provider-capacity-noticed, even while
/// its agent is still busy, because none of them need a settling window the
/// way runtime does: they are already durable facts on the task, not a
/// transition that can flicker. A plain busy task with none of them still
/// must not resolve, or every reconcile wait would return immediately.
/// `unread` is deliberately not one of these — see
/// `reconcile_does_not_resolve_on_unread_alone_while_busy` below.
#[test]
fn reconcile_resolves_on_any_actionable_signal_even_while_busy() {
    let plain_busy = json!({
        "activity": "working",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
    });
    assert!(
        !task_value_matches_wait_until(&plain_busy, WaitUntil::Reconcile),
        "a plain busy task with no actionable signal must not resolve reconcile"
    );

    let blocked_while_busy = json!({
        "activity": "working",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
        "blockedByTaskIds": ["blocker-1"],
    });
    assert!(
        task_value_matches_wait_until(&blocked_while_busy, WaitUntil::Reconcile),
        "an unresolved blocker resolves reconcile even while busy"
    );

    let badged_while_busy = json!({
        "activity": "working",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
        "attentionRequested": true,
    });
    assert!(
        task_value_matches_wait_until(&badged_while_busy, WaitUntil::Reconcile),
        "an attention badge resolves reconcile even while busy"
    );

    let provider_parked_while_busy = json!({
        "activity": "working",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
        "providerRejection": { "recovery": "parked-no-candidates" },
    });
    assert!(
        task_value_matches_wait_until(&provider_parked_while_busy, WaitUntil::Reconcile),
        "a provider-parked refusal resolves reconcile even while busy"
    );

    let capacity_noticed_while_busy = json!({
        "activity": "working",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
        "providerCapacityNotice": { "provider": "codex", "model": "gpt-6" },
    });
    assert!(
        task_value_matches_wait_until(&capacity_noticed_while_busy, WaitUntil::Reconcile),
        "a provider capacity notice resolves reconcile even while busy"
    );

    // A fallback-started refusal is not a park: recovery keeps going
    // automatically, so it must not resolve reconcile by itself.
    let fallback_started_while_busy = json!({
        "activity": "working",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
        "providerRejection": { "recovery": "fallback-started" },
    });
    assert!(
        !task_value_matches_wait_until(&fallback_started_while_busy, WaitUntil::Reconcile),
        "a fallback-started refusal is not parked and must not resolve reconcile"
    );

    // An empty blocker list and a null badge/notice must not themselves
    // resolve reconcile.
    let clear_of_every_signal = json!({
        "activity": "working",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
        "blockedByTaskIds": [],
        "attentionRequested": false,
        "providerRejection": null,
        "providerCapacityNotice": null,
    });
    assert!(
        !task_value_matches_wait_until(&clear_of_every_signal, WaitUntil::Reconcile),
        "explicit empty/null signal fields must not themselves resolve reconcile"
    );
}

/// `activity` stays `unread` for a task whose agent is actively working —
/// nobody has read the output yet, but the runtime is `busy` — which is
/// exactly the shape produced by sending a stopped child input and
/// immediately waiting on it (AGENTS.md's "Runtime and read state are two
/// dimensions": "A busy task may therefore remain `unread` until someone
/// reads that output"). Unlike the other actionable signals, `unread` must
/// NOT resolve `Reconcile` while the runtime is busy: doing so would report
/// an actively running agent as needing reconciliation and keep doing so on
/// every subsequent call, reintroducing the spin this predicate exists to
/// remove. Once the runtime is no longer busy, `unread` alone is enough —
/// there is nothing left running to wait out.
#[test]
fn reconcile_does_not_resolve_on_unread_alone_while_busy() {
    let unread_while_busy = json!({
        "activity": "unread",
        "runtimeState": "busy",
        "closedAt": null,
        "latestRun": { "status": "running" },
    });
    assert!(
        !task_value_matches_wait_until(&unread_while_busy, WaitUntil::Reconcile),
        "unread must not resolve reconcile while the runtime is busy"
    );

    let unread_while_idle = json!({
        "activity": "unread",
        "runtimeState": "idle",
        "closedAt": null,
        "latestRun": { "status": "running" },
    });
    assert!(
        task_value_matches_wait_until(&unread_while_idle, WaitUntil::Reconcile),
        "unread still resolves reconcile once the runtime is not busy"
    );

    let unread_with_no_runtime_state = json!({
        "activity": "unread",
        "closedAt": null,
        "latestRun": { "status": "running" },
    });
    assert!(
        task_value_matches_wait_until(&unread_with_no_runtime_state, WaitUntil::Reconcile),
        "unread resolves reconcile when the runtime dimension is unknown, not just busy"
    );
}

/// A server that predates the split sends no `runtimeState`. The wait must
/// still work off the terminal `stage_run`, and must not start resolving on
/// read state again to compensate.
#[test]
fn a_detail_without_the_runtime_dimension_falls_back_to_the_terminal_run() {
    let unread_only = json!({ "activity": "unread", "closedAt": null, "latestRun": null });
    assert!(!task_value_matches_wait_until(
        &unread_only,
        WaitUntil::Finished
    ));
    let terminal_run = json!({
        "activity": "unread",
        "closedAt": null,
        "latestRun": { "status": "succeeded" },
    });
    assert!(task_value_matches_wait_until(
        &terminal_run,
        WaitUntil::Finished
    ));
}

fn skew_info(server_status: serde_json::Value, client_tools: &[String]) -> serde_json::Value {
    runtime_info_snapshot(
        "http://127.0.0.1:49199",
        RuntimeAdapterIdentity {
            name: "kanna-mcp",
            version: "0.1.0",
            mcp_protocol_version: None,
            task_id: None,
        },
        Ok(server_status),
        client_tools,
    )
}

fn status_with(agent_api_tools: serde_json::Value) -> serde_json::Value {
    let mut status = json!({
        "state": "running",
        "desktopId": "desktop-1",
        "desktopName": "Mac",
        "version": "0.1.0",
        "environment": "development",
        "lanHost": "127.0.0.1",
        "lanPort": 48120,
    });
    if !agent_api_tools.is_null() {
        status["agentApiTools"] = agent_api_tools;
    }
    status
}

/// An agent whose instructions mandate a tool has to be able to tell "the
/// server says there are none" from "this server cannot be asked". Without
/// this, that difference only shows up as a 404, which is indistinguishable
/// from an ordinary not-found.
#[test]
fn kanna_info_reports_tools_the_connected_server_cannot_serve() {
    let client_tools = [
        "kanna_info".to_string(),
        "kanna_get_task".to_string(),
        "kanna_list_task_children".to_string(),
    ];

    let current = skew_info(
        status_with(json!([
            "kanna_info",
            "kanna_get_task",
            "kanna_list_task_children"
        ])),
        &client_tools,
    );
    assert_eq!(current["agentApi"]["status"], "current");
    assert_eq!(current["agentApi"]["unavailableTools"], json!([]));

    // The observed skew: a released app that predates `kanna_list_task_children`.
    let behind = skew_info(
        status_with(json!(["kanna_info", "kanna_get_task"])),
        &client_tools,
    );
    assert_eq!(behind["agentApi"]["status"], "server_behind");
    assert_eq!(
        behind["agentApi"]["unavailableTools"],
        json!(["kanna_list_task_children"])
    );

    // A server old enough not to advertise at all is itself the signal.
    let unknown = skew_info(status_with(json!(null)), &client_tools);
    assert_eq!(unknown["agentApi"]["status"], "unknown");
    assert_eq!(
        unknown["agentApi"]["serverAdvertisesCapabilities"],
        json!(false)
    );

    // An unreachable server reports `unknown` rather than omitting the block,
    // which would read as "no skew".
    let unreachable = runtime_info_snapshot(
        "http://127.0.0.1:49199",
        RuntimeAdapterIdentity {
            name: "kanna-mcp",
            version: "0.1.0",
            mcp_protocol_version: None,
            task_id: None,
        },
        Err("connection refused".to_string()),
        &client_tools,
    );
    assert_eq!(unreachable["agentApi"]["status"], "unknown");
}

/// Self-exclusion is catalog policy so `kanna-mcp` and `kanna-cli tool call`
/// cannot drift: a repository-scoped wait from inside a task session drops the
/// caller's own task, an explicit `task_ids` scope is taken literally, and
/// `exclude_own: false` is the documented way to opt out. `exclude_own` is a
/// task-scope filter and nothing more: it is consumed entirely client-side
/// (folded into `exclude_task_ids`) and never reaches the server on the wire.
#[test]
fn wait_events_self_exclusion_is_shared_catalog_policy() {
    let catalog = bundled_catalog();

    let defaulted = args_with_self_exclusion(
        "kanna_wait_events",
        &json!({ "from": "now" }),
        Some("manager-1"),
    )
    .expect("apply policy");
    assert_eq!(defaulted["exclude_task_ids"], json!(["manager-1"]));
    assert!(
        defaulted.get("exclude_own").is_none(),
        "exclude_own is consumed, never forwarded"
    );
    let resolved = resolve_request_with_repo_context(
        &catalog,
        "kanna_wait_events",
        &defaulted,
        Some(&json!({ "repoId": "repo-current" })),
    )
    .expect("resolve defaulted wait");
    // Neither excludeOwn (client-only, never sent) nor includeCurrentActivity
    // (cursor-implied server-side, no catalog default any more) reach the wire.
    assert_eq!(
        resolved.path,
        "/v1/task-events?repoId=repo-current&excludeTaskIds=manager-1&from=now&timeoutSecs=240&shortCursor=true"
    );

    let explicit_repo = args_with_self_exclusion(
        "kanna_wait_events",
        &json!({ "repo_id": "repo-explicit", "exclude_task_ids": ["other", "manager-1"] }),
        Some("manager-1"),
    )
    .expect("apply policy");
    assert_eq!(
        explicit_repo["exclude_task_ids"],
        json!(["other", "manager-1"])
    );

    let remote_hash = args_with_self_exclusion(
        "kanna_wait_events",
        &json!({ "repo_remote_url_hash": "hash", "exclude_task_ids": ["other"] }),
        Some("manager-1"),
    )
    .expect("apply policy");
    assert_eq!(
        remote_hash["exclude_task_ids"],
        json!(["other", "manager-1"])
    );

    for repository_scope_with_empty_literal in [
        json!({ "repo_id": "repo-explicit", "task_ids": [] }),
        json!({ "repo_id": "repo-explicit", "task_ids": [" "] }),
        json!({ "repo_id": "repo-explicit", "parent_task_id": "" }),
        json!({ "repo_id": "repo-explicit", "parent_task_id": null }),
    ] {
        let filtered = args_with_self_exclusion(
            "kanna_wait_events",
            &repository_scope_with_empty_literal,
            Some("manager-1"),
        )
        .expect("apply policy");
        assert_eq!(
            filtered["exclude_task_ids"],
            json!(["manager-1"]),
            "empty literal scopes fall through to repository scope"
        );
    }

    // An explicit `task_ids` list is already literal: naming your own id
    // there is a deliberate request to watch it, so self-exclusion never
    // applies and the args pass through completely unchanged (`exclude_own`
    // is still consumed, but there was none to remove here).
    let literal_task_ids = json!({ "task_ids": ["manager-1", "child-a"] });
    let unchanged =
        args_with_self_exclusion("kanna_wait_events", &literal_task_ids, Some("manager-1"))
            .expect("apply policy");
    assert_eq!(
        unchanged, literal_task_ids,
        "an explicit task_ids scope is taken completely literally"
    );

    // `parent_task_id` gets no such carve-out: self-exclusion still runs, but
    // it is a harmless no-op there, because a parent scope already excludes
    // the parent's own events structurally (only direct children are ever
    // returned) — adding the parent's own id to exclude_task_ids drops
    // nothing that scope would ever have produced anyway.
    let parent_scope = args_with_self_exclusion(
        "kanna_wait_events",
        &json!({ "parent_task_id": "manager-1" }),
        Some("manager-1"),
    )
    .expect("apply policy");
    assert_eq!(
        parent_scope,
        json!({ "parent_task_id": "manager-1", "exclude_task_ids": ["manager-1"] })
    );

    let included = args_with_self_exclusion(
        "kanna_wait_events",
        &json!({ "repo_id": "repo-explicit", "exclude_own": false, "exclude_task_ids": ["other"] }),
        Some("manager-1"),
    )
    .expect("apply policy");
    assert_eq!(
        included,
        json!({ "repo_id": "repo-explicit", "exclude_task_ids": ["other"] }),
        "exclude_own: false opts out of self-exclusion only, and is itself consumed"
    );

    let outside_session =
        args_with_self_exclusion("kanna_wait_events", &json!({ "repo_id": "repo-1" }), None)
            .expect("apply policy");
    assert_eq!(
        outside_session,
        json!({ "repo_id": "repo-1" }),
        "a caller that is not a task session has no own task to exclude, and defaults exclude_own to false"
    );

    // An explicit value always wins over the session default, in both
    // directions, and is always consumed rather than forwarded.
    let kept_own_events = args_with_self_exclusion(
        "kanna_wait_events",
        &json!({ "parent_task_id": "manager-1", "exclude_own": false }),
        Some("manager-1"),
    )
    .expect("apply policy");
    assert_eq!(
        kept_own_events,
        json!({ "parent_task_id": "manager-1" }),
        "exclude_own: false disables self-exclusion even for a session caller"
    );
    let excluded_outside_session = args_with_self_exclusion(
        "kanna_wait_events",
        &json!({ "parent_task_id": "outsider" }),
        None,
    )
    .expect("apply policy");
    assert_eq!(
        excluded_outside_session,
        json!({ "parent_task_id": "outsider" }),
        "no task id is known outside a session, so there is nothing to exclude by default"
    );

    let other_tool = args_with_self_exclusion(
        "kanna_list_recent_tasks",
        &json!({ "limit": 5 }),
        Some("manager-1"),
    )
    .expect("apply policy");
    assert_eq!(other_tool, json!({ "limit": 5 }));

    let error = args_with_self_exclusion(
        "kanna_wait_events",
        &json!({ "exclude_own": "yes" }),
        Some("manager-1"),
    )
    .expect_err("exclude_own must be boolean");
    assert!(error.contains("exclude_own must be a boolean"), "{error}");

    assert_eq!(
        task_event_self_exclusion(false, true, Some("  ")),
        None,
        "a blank task id is not a session"
    );
}

/// The batching parameters are the cheap answer to a manager that made ~4,700
/// wait calls in two days. They must reach the wire — the shaping happens in
/// the server, not the client — and carry the bounds the server enforces.
#[test]
fn wait_events_batching_parameters_reach_the_wire_with_their_bounds() {
    let catalog = bundled_catalog();

    for (name, key) in [
        ("event_types", "eventTypes"),
        ("min_events", "minEvents"),
        ("debounce_ms", "debounceMs"),
        ("min_interval_ms", "minIntervalMs"),
    ] {
        let param = catalog
            .find_param("kanna_wait_events", name)
            .unwrap_or_else(|| panic!("{name} must be declared"));
        assert_eq!(
            param.location,
            ParamLoc::Query,
            "{name} is shaped server-side"
        );
        assert_eq!(param.key.as_deref(), Some(key), "{name} wire key");
    }

    // `exclude_own` is the odd one out here: it is a client-only, task-scope
    // filter (folded into exclude_task_ids by args_with_self_exclusion) and
    // never reaches the server on the wire at all.
    let exclude_own = catalog
        .find_param("kanna_wait_events", "exclude_own")
        .expect("exclude_own must be declared");
    assert_eq!(
        exclude_own.location,
        ParamLoc::Client,
        "exclude_own is shaped client-side"
    );

    let min_events = catalog
        .find_param("kanna_wait_events", "min_events")
        .expect("min_events");
    assert_eq!(min_events.min, Some(1));
    assert_eq!(min_events.max, Some(MAX_TASK_EVENT_LIMIT as u64));
    for name in ["debounce_ms", "min_interval_ms"] {
        let hold = catalog
            .find_param("kanna_wait_events", name)
            .unwrap_or_else(|| panic!("{name}"));
        assert_eq!(hold.min, Some(0), "{name}");
        assert_eq!(
            hold.max,
            Some(MAX_TASK_EVENT_HOLD_MS),
            "{name} must advertise the ceiling the server clamps to"
        );
    }

    let resolved = resolve_request(
        &catalog,
        "kanna_wait_events",
        &json!({
            "task_ids": ["child-a"],
            "event_types": ["run.finished", "task.pr_created"],
            "exclude_own": true,
            "min_events": 5,
            "debounce_ms": 2000,
            "min_interval_ms": 5000,
        }),
    )
    .expect("resolve batched wait");
    // `exclude_own` is a raw arg here (this test calls `resolve_request`
    // directly, bypassing `args_with_self_exclusion`), so it is simply
    // dropped by its `Client` location — never reaches the wire.
    assert!(
        !resolved.path.contains("excludeOwn"),
        "exclude_own must never reach the server: {}",
        resolved.path
    );
    for expected in [
        "eventTypes=run.finished%2Ctask.pr_created",
        "minEvents=5",
        "debounceMs=2000",
        "minIntervalMs=5000",
    ] {
        assert!(
            resolved.path.contains(expected),
            "{expected} missing from {}",
            resolved.path
        );
    }

    // The declared bounds are enforced on the way out, and the server clamps
    // again on the way in, so the two cannot disagree about the ceiling.
    let over_ceiling = resolve_request(
        &catalog,
        "kanna_wait_events",
        &json!({ "task_ids": ["child-a"], "debounce_ms": 600_000 }),
    )
    .expect("resolve an over-ceiling hold window");
    assert!(
        over_ceiling
            .path
            .contains(&format!("debounceMs={MAX_TASK_EVENT_HOLD_MS}")),
        "{}",
        over_ceiling.path
    );
}

/// One rule decides when a batched wait may return, shared by the server's own
/// wait, its cross-machine fan-out, and the MCP client fan-in — so `min_events`
/// counts the same events wherever the fan-out happens to live.
#[test]
fn the_batch_release_rule_is_shared_and_never_holds_a_full_page() {
    // Nothing yet: the caller asked to wait for three.
    assert!(!task_event_batch_is_complete(2, false, 100, 3, true));
    assert!(task_event_batch_is_complete(3, false, 100, 3, true));
    // Enough events, but the hold window is still open.
    assert!(!task_event_batch_is_complete(3, false, 100, 3, false));
    // A page the caller must drain is never held: waiting cannot add to it.
    assert!(task_event_batch_is_complete(1, true, 100, 50, false));
    assert!(task_event_batch_is_complete(100, false, 100, 50, false));

    // A minimum above the page size would otherwise make every wait a timeout.
    assert_eq!(clamp_task_event_min_events(Some(5_000), 100), 100);
    assert_eq!(clamp_task_event_min_events(Some(0), 100), 1);
    assert_eq!(clamp_task_event_min_events(None, 100), 1);
    assert_eq!(clamp_task_event_limit(None), DEFAULT_TASK_EVENT_LIMIT);
    assert_eq!(clamp_task_event_limit(Some(9_000)), MAX_TASK_EVENT_LIMIT);
    assert_eq!(
        clamp_task_event_hold_ms(Some(u64::MAX)),
        MAX_TASK_EVENT_HOLD_MS
    );
    assert_eq!(clamp_task_event_hold_ms(None), 0);
}

/// `exclude_own` is advertised and validated like every other argument but
/// never reaches the wire: the server has no notion of "self" any more — the
/// old, separate `include_self` parameter was folded into it.
#[test]
fn exclude_own_is_a_client_only_parameter() {
    let catalog = bundled_catalog();
    let exclude_own = catalog
        .find_param("kanna_wait_events", "exclude_own")
        .expect("exclude_own declared");
    assert_eq!(exclude_own.location, ParamLoc::Client);
    assert_eq!(exclude_own.param_type, ParamType::Boolean);
    let exclude = catalog
        .find_param("kanna_wait_events", "exclude_task_ids")
        .expect("exclude_task_ids declared");
    assert_eq!(exclude.location, ParamLoc::Query);
    assert_eq!(exclude.param_type, ParamType::StringArray);
    assert_eq!(exclude.key.as_deref(), Some("excludeTaskIds"));

    // `resolve_request` alone (bypassing `args_with_self_exclusion`) drops a
    // client-only param outright — it never reaches the built path.
    let resolved = resolve_request(
        &catalog,
        "kanna_wait_events",
        &json!({ "repo_id": "repo-1", "exclude_own": true, "exclude_task_ids": ["a", "b"], "timeout_secs": 0 }),
    )
    .expect("resolve");
    assert_eq!(
        resolved.path,
        "/v1/task-events?repoId=repo-1&excludeTaskIds=a%2Cb&timeoutSecs=0&shortCursor=true"
    );

    let schema = bundled_catalog().tools_list_value();
    let wait_events = schema
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == "kanna_wait_events")
        .expect("wait events tool");
    let properties = &wait_events["inputSchema"]["properties"];
    assert!(properties.get("include_self").is_none());
    assert!(properties.get("exclude_own").is_some());
    assert!(properties.get("exclude_task_ids").is_some());
    assert!(wait_events["description"]
        .as_str()
        .expect("description")
        .contains("excludes the calling task's own events by default"));
}

/// The raw-input tool's advertised key vocabulary is the shared Rust table, not
/// a second list that happens to agree today.
///
/// Two surfaces read this vocabulary — the MCP schema an agent picks names out
/// of, and the server that turns a name into bytes — and they are in different
/// crates. A name that is in one and not the other is a tool call that
/// validates and then 400s, or a key an agent never learns exists.
#[test]
fn raw_input_key_vocabulary_matches_the_shared_terminal_key_table() {
    use kanna_runtime_defaults::terminal_keys::{
        terminal_key_names, CTRL_LETTERS_WITH_NAMED_EQUIVALENTS,
    };

    let catalog = bundled_catalog();
    let keys = catalog
        .find_param("kanna_send_task_raw_input", "keys")
        .expect("keys parameter");
    assert_eq!(keys.param_type, ParamType::StringArray);
    assert_eq!(keys.location, ParamLoc::Body);
    let advertised = keys
        .enum_values
        .clone()
        .expect("keys declares a vocabulary");
    assert_eq!(advertised, terminal_key_names());

    // The redundant spellings stay out of the advertised list: `enter`
    // declares a submission boundary and `ctrl-m` would not, so offering both
    // would let the same keystroke mean two different things to the composer.
    for letter in CTRL_LETTERS_WITH_NAMED_EQUIVALENTS {
        assert!(
            !advertised.contains(&format!("ctrl-{letter}")),
            "ctrl-{letter} duplicates a named key"
        );
    }
    for required in [
        "escape",
        "enter",
        "tab",
        "backspace",
        "up",
        "down",
        "left",
        "right",
    ] {
        assert!(
            advertised.iter().any(|name| name == required),
            "{required} missing"
        );
    }

    // The description has to carry the vocabulary too: an agent reads the
    // description long before a schema validator tells it what it got wrong.
    let description = keys.description.clone().expect("keys description");
    for name in &advertised {
        assert!(description.contains(name.as_str()), "{name} undocumented");
    }
}

/// The stage-completion vocabulary an agent picks a word out of is the shared
/// Rust table, not a second list that happens to agree today.
///
/// Three surfaces read it — the MCP schema, the CLI (which reaches
/// `resolve_request` with no JSON-Schema validator in front of it), and the
/// server that records the word — and they live in different crates. A word in
/// one and not another is a completion that validates and then 400s, or a
/// verdict an agent never learns it may use.
#[test]
fn complete_stage_vocabulary_matches_the_shared_stage_verdict_table() {
    use kanna_runtime_defaults::stage_verdict::{stage_verdict_names, StageVerdict};

    let catalog = bundled_catalog();
    let status = catalog
        .find_param("kanna_complete_stage", "status")
        .expect("status parameter");
    let advertised = status
        .enum_values
        .clone()
        .expect("status declares a vocabulary");
    assert_eq!(advertised, stage_verdict_names());

    // `closed` left the vocabulary on 2026-09-19: closing a task is a
    // lifecycle action and recording it as a verdict made "somebody stopped
    // this" indistinguishable from "the agent failed".
    assert!(!advertised.iter().any(|value| value == "closed"));

    // An agent reads the description long before a schema validator tells it
    // what it got wrong, so every word has to be teachable from the text.
    let description = status.description.clone().expect("status description");
    for verdict in advertised.iter() {
        assert!(
            description.contains(verdict.as_str()),
            "{verdict} undocumented"
        );
    }
    assert!(description.contains("no 'closed' status"));

    // The two historic words keep their exact spelling, so a caller written
    // against the old vocabulary still validates.
    assert_eq!(StageVerdict::parse("success"), Ok(StageVerdict::Success));
    assert_eq!(StageVerdict::parse("failure"), Ok(StageVerdict::Failure));
}

/// A closed vocabulary on a list constrains its items.
///
/// Declared on the array itself, the schema would say the array must *equal*
/// one of the strings, which no client can satisfy — and the CLI, which never
/// sees a JSON-Schema validator, would reject a perfectly good list.
#[test]
fn a_list_parameters_vocabulary_constrains_its_items() {
    let catalog = bundled_catalog();
    let tools = catalog.tools_list_value();
    let tool = tools
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == "kanna_send_task_raw_input")
        .expect("raw input tool");
    let keys = &tool["inputSchema"]["properties"]["keys"];
    assert_eq!(keys["type"], "array");
    assert!(
        keys.get("enum").is_none(),
        "vocabulary must not sit on the array"
    );
    assert_eq!(keys["items"]["type"], "string");
    assert_eq!(keys["items"]["enum"][0], "escape");

    let rejected = resolve_request(
        &catalog,
        "kanna_send_task_raw_input",
        &json!({ "task_id": "task-1", "keys": ["down", "arrow-up"] }),
    )
    .expect_err("an unknown key name must be refused");
    assert!(rejected.contains("arrow-up"), "{rejected}");

    resolve_request(
        &catalog,
        "kanna_send_task_raw_input",
        &json!({ "task_id": "task-1", "keys": ["down", "enter"] }),
    )
    .expect("a list of known keys resolves");
}

/// The two input tools must read as different things, because using the wrong
/// one is the whole failure this route exists to prevent.
#[test]
fn raw_input_description_separates_keys_from_delivered_messages() {
    let catalog = bundled_catalog();
    let description = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_send_task_raw_input")
        .map(|tool| tool.description.clone())
        .expect("raw input tool");

    // The incident's own sequence, and the two other examples the surface owes
    // a caller: a bare Escape and explicit bytes with nothing appended.
    assert!(
        description.contains("keys [\"escape\"]"),
        "escape example missing"
    );
    assert!(
        description.contains("keys [\"down\", \"enter\"]"),
        "down-then-enter example missing"
    );
    assert!(
        description.contains("bytes \"1b5b42\""),
        "raw bytes example missing"
    );

    // Honest about what it is not.
    assert!(description.contains("NOT recorded in kanna_task_inputs"));
    assert!(description.contains("task.raw_input_delivered"));
    assert!(description.contains("not an approval mechanism"));
    assert!(description.contains("do NOT resend"));
    // Terminal-mode limits are stated rather than a universal claim implied.
    assert!(description.contains("DECCKM"));
}

/// `value_for_param` fills in a declared `default` for any omitted parameter
/// and sends it on the wire (proven by `override_catalog_cannot_reintroduce_
/// an_unsurvivable_wait_window`'s `defaulted`/`explicit` cases resolving to
/// the same clamped value above). For `kanna_subscribe_events`'s rate-limit
/// override that would be a defect: the server distinguishes "omitted, keep
/// whatever this subscription already has" from "explicit, persist as an
/// override" purely by whether the key is present at all, so a resolver that
/// injects the documented default turns every omission into an explicit
/// (if numerically identical) override — breaking retry/resume for every
/// subscription that predates this feature or was registered without one.
#[test]
fn subscribe_events_timing_overrides_are_omitted_from_the_wire_when_not_given() {
    let catalog = bundled_catalog();
    let minimal = resolve_request(
        &catalog,
        "kanna_subscribe_events",
        &json!({ "task_id": "manager-1", "local_only": true, "delivery": "input" }),
    )
    .expect("minimal subscribe request resolves");

    for key in ["quietMs", "minAdmissionIntervalMs"] {
        assert!(
            minimal.body.get(key).is_none(),
            "{key} must be entirely absent from an omitted-knob request body, not merely null: {}",
            minimal.body
        );
    }

    // An explicit value — even one matching the documented default — must
    // still reach the wire, so the server can tell it apart from omission.
    let explicit = resolve_request(
        &catalog,
        "kanna_subscribe_events",
        &json!({
            "task_id": "manager-1",
            "local_only": true,
            "delivery": "input",
            "min_admission_interval_ms": 60_000,
        }),
    )
    .expect("explicit subscribe request resolves");
    assert_eq!(explicit.body["minAdmissionIntervalMs"], 60_000);

    // The rate limit is the only timing knob left. `max_hold_ms` collapsed
    // into `quiet_ms`, and `quiet_ms` itself went with the trailing-quiet
    // mechanism: both are rejected outright rather than silently ignored,
    // so a caller still passing one learns its timing is not being applied.
    for retired in ["max_hold_ms", "quiet_ms"] {
        let rejected = resolve_request(
            &catalog,
            "kanna_subscribe_events",
            &json!({
                "task_id": "manager-1",
                "local_only": true,
                "delivery": "input",
                retired: 300_000,
            }),
        );
        assert!(
            rejected.is_err(),
            "{retired} must be rejected, not silently accepted and ignored"
        );
    }
}

/// `diagnostic` reaches the wire the same way on all three subscription
/// tools' own terms: a body field for subscribe/read, but a query parameter
/// for unsubscribe, which has no body at all.
#[test]
fn diagnostic_maps_to_a_body_field_on_subscribe_and_read_but_a_query_param_on_unsubscribe() {
    let catalog = bundled_catalog();

    let subscribe_diagnostic = catalog
        .find_param("kanna_subscribe_events", "diagnostic")
        .expect("subscribe declares diagnostic");
    assert_eq!(subscribe_diagnostic.location, ParamLoc::Body);
    let read_diagnostic = catalog
        .find_param("kanna_read_event_subscription", "diagnostic")
        .expect("read declares diagnostic");
    assert_eq!(read_diagnostic.location, ParamLoc::Body);
    let unsubscribe_diagnostic = catalog
        .find_param("kanna_unsubscribe_events", "diagnostic")
        .expect("unsubscribe declares diagnostic");
    assert_eq!(unsubscribe_diagnostic.location, ParamLoc::Query);

    let subscribe = resolve_request(
        &catalog,
        "kanna_subscribe_events",
        &json!({ "task_id": "manager-1", "local_only": true, "delivery": "input", "diagnostic": true }),
    )
    .expect("subscribe resolves");
    assert_eq!(subscribe.body["diagnostic"], true);

    let read = resolve_request(
        &catalog,
        "kanna_read_event_subscription",
        &json!({ "subscription_id": "watch-1", "diagnostic": true }),
    )
    .expect("read resolves");
    assert_eq!(read.body["diagnostic"], true);

    let unsubscribe = resolve_request(
        &catalog,
        "kanna_unsubscribe_events",
        &json!({ "subscription_id": "watch-1", "diagnostic": true }),
    )
    .expect("unsubscribe resolves");
    assert!(
        unsubscribe.body.get("diagnostic").is_none(),
        "diagnostic must not also land in unsubscribe's body: {}",
        unsubscribe.body
    );
    assert!(
        unsubscribe.path.contains("diagnostic=true"),
        "diagnostic must reach unsubscribe as a query parameter: {}",
        unsubscribe.path
    );
}

#[test]
fn human_queue_tool_is_distinct_from_ordinary_policy_handoff() {
    let catalog = bundled_catalog();
    let policy = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_signal_merge_handoff")
        .unwrap();
    assert_eq!(
        policy
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "machine_id",
            "task_id",
            "branch",
            "target",
            "pr_url",
            "summary"
        ]
    );
    let relay = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_queue_reviewed_pr")
        .unwrap();
    for name in [
        "task_id",
        "review_context_version",
        "head_sha",
        "instruction",
    ] {
        assert!(relay
            .params
            .iter()
            .any(|param| param.name == name && param.required));
    }
    assert!(!relay
        .params
        .iter()
        .any(|param| param.name == "origin" || param.name == "device_provenance"));
}

#[test]
fn doctor_is_catalog_backed_read_only_candidate_validation() {
    let catalog = bundled_catalog();
    let tool = catalog
        .tools
        .iter()
        .find(|tool| tool.name == "kanna_doctor")
        .unwrap();
    assert_eq!(tool.method, Method::Get);
    assert_eq!(tool.path, "/v1/repos/{repo_id}/doctor");
    assert!(tool
        .params
        .iter()
        .any(|param| param.name == "candidate_path" && param.location == ParamLoc::Query));
    assert!(catalog
        .render_guide("config")
        .unwrap()
        .contains("kanna_doctor"));
}

#[test]
fn attention_tools_use_existing_action_transport_and_machine_routing() {
    let catalog = bundled_catalog();
    for (name, args, path, body) in [
        (
            "kanna_set_task_attention",
            json!({"task_id":"task 1", "machine_id":"remote"}),
            "/v1/tasks/task%201/actions/set-attention",
            json!({}),
        ),
        (
            "kanna_clear_task_attention",
            json!({"task_id":"task 1", "machine_id":"remote"}),
            "/v1/tasks/task%201/actions/clear-attention",
            json!({}),
        ),
    ] {
        let request = resolve_request(&catalog, name, &args).unwrap();
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.path, path);
        assert_eq!(request.body, body);
    }
}

#[test]
fn harness_aliases_use_legacy_wire_keys_and_refuse_duplicate_spellings() {
    let catalog = bundled_catalog();
    for (tool, old, new, wire, base) in [
        (
            "kanna_create_task",
            "agent_provider",
            "harness",
            "agentProvider",
            json!({"repo_id":"r", "prompt":"p"}),
        ),
        (
            "kanna_advance_stage",
            "next_stage_agent_provider",
            "next_stage_harness",
            "nextStageAgentProvider",
            json!({"task_id":"t"}),
        ),
    ] {
        let mut args = base;
        args[new] = "opencode".into();
        let request = resolve_request(&catalog, tool, &args).unwrap();
        assert_eq!(request.body[wire], "opencode");
        assert!(request.body.get(new).is_none());
        args[old] = "opencode".into();
        assert!(resolve_request(&catalog, tool, &args)
            .unwrap_err()
            .contains("conflicting"));
    }
}

#[test]
fn desktop_pane_controls_keep_machine_routing_and_destination_on_the_shared_wire() {
    let catalog = bundled_catalog();
    let args = json!({"task_id":"task-a", "operation":"move", "machine_id":"remote",
        "window_id":"window-a", "workspace_id":"opaque", "pane_id":"pane-2", "tab_id":"file:AGENTS.md"});
    let request = resolve_request(&catalog, "kanna_workspace", &args).unwrap();
    assert_eq!(request.machine_id.as_deref(), Some("remote"));
    assert_eq!(
        request.body,
        json!({"taskId":"task-a", "operation":"move", "windowId":"window-a",
        "workspaceId":"opaque", "paneId":"pane-2", "tabId":"file:AGENTS.md"})
    );
    assert!(resolve_request(
        &catalog,
        "kanna_workspace",
        &json!({"task_id":"task-a", "operation":"execute"})
    )
    .is_err());
    assert!(resolve_request(
        &catalog,
        "kanna_workspace",
        &json!({"task_id":"task-a", "operation":"split", "direction":"diagonal"})
    )
    .is_err());
}

/// A delivered mailbox page is bounded at the source, so the tool surface every
/// adapter renders from — MCP and the CLI both — has to say what a manager will
/// and will not find on it, in the same words on the tool that opens the
/// mailbox and the tool that reads it. The failure this prevents is quiet: an
/// agent reading a 280-character summary as the whole verdict, or concluding a
/// workflow carries no stage prompts, because nothing told it the page was
/// bounded and where the full text lives.
#[test]
fn subscription_descriptions_state_what_a_bounded_delivered_page_carries() {
    let catalog = bundled_catalog();
    for name in ["kanna_subscribe_events", "kanna_read_event_subscription"] {
        let description = catalog
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .map(|tool| tool.description.clone())
            .unwrap_or_else(|| panic!("{name} is declared"));

        // The compact page's own key list is unchanged and still stated.
        for key in [
            "wakeState",
            "batchId",
            "staleMachines",
            "waitOutcome",
            "machineErrors",
            "watchError",
        ] {
            assert!(
                description.contains(key),
                "{name} must keep documenting the compact key {key}"
            );
        }

        // What is bounded, and the marker that says so.
        assert!(
            description.contains("summaryTruncated"),
            "{name} must name the truncation marker"
        );
        assert!(
            description.contains("status and metadata verbatim"),
            "{name} must say the structured result facts survive"
        );
        assert!(
            description.contains("beforeDefinition and afterDefinition"),
            "{name} must say which definitions are bounded"
        );
        assert!(
            description.contains("notificationContext"),
            "{name} must say the relevance filter's working state is not delivered"
        );

        // And what is emphatically not bounded, so a page is still trustworthy.
        assert!(
            description.contains("acknowledgement by batchId are all unchanged"),
            "{name} must say the ack contract is untouched"
        );
        assert!(
            description.contains("payload.currentTask"),
            "{name} must say delivery-time task state still arrives"
        );
        assert!(
            description.contains("kanna_get_task"),
            "{name} must say where the full prose is read from"
        );
        assert!(
            description.contains("diagnostic true"),
            "{name} must keep pointing at the verbatim escape hatch"
        );
    }
}

#[test]
fn artifact_tools_name_exact_ids_and_state_that_retention_is_not_yet_enforced() {
    let catalog = bundled_catalog();
    let description = |name: &str| {
        catalog
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} declared"))
            .description
            .clone()
    };
    let publish = description("kanna_publish_artifact");
    assert!(publish.contains("tree id"), "{publish}");
    assert!(publish.contains("not yet enforced"), "{publish}");
    assert!(
        publish.contains("outside the working repository"),
        "{publish}"
    );
    for name in [
        "kanna_record_artifact_comment",
        "kanna_record_artifact_decision",
    ] {
        assert!(
            description(name).contains("not a verified identity"),
            "{name}"
        );
    }
    let push = description("kanna_push_artifact");
    assert!(push.contains("artifacts.remote"), "{push}");
    assert!(push.contains("never forced"), "{push}");
    assert!(push.contains("Kanna stores none"), "{push}");
    assert!(
        push.contains("no task directory, transcript, environment or credential"),
        "{push}"
    );
    let fetch = description("kanna_fetch_artifact");
    assert!(fetch.contains("by id alone"), "{fetch}");
    assert!(fetch.contains("A received decision is data"), "{fetch}");
    assert!(fetch.contains("artifact_not_on_remote"), "{fetch}");
    for name in ["kanna_push_artifact", "kanna_fetch_artifact"] {
        assert!(
            catalog.find_param(name, "remote").is_none(),
            "{name}: the remote is configuration, never a parameter"
        );
    }
    let kind = catalog
        .find_param("kanna_publish_artifact", "kind")
        .unwrap();
    assert_eq!(
        kind.enum_values.as_deref(),
        Some(
            ["document", "mockup", "media", "report"]
                .map(String::from)
                .as_slice()
        )
    );
    let error = resolve_request(
        &catalog,
        "kanna_publish_artifact",
        &json!({ "task_id": "t", "path": "p", "kind": "diagram" }),
    )
    .unwrap_err();
    assert!(error.contains("kind must be one of"), "{error}");
}
