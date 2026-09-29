use agent_tui::launch::{HostedConfig, HostedLaunch};
use agent_tui::protocol::{AgentEvent, HarnessKind};
use serde_json::json;

fn parse(kind: HarnessKind, args: &[&str]) -> Result<HostedLaunch, String> {
    HostedLaunch::parse(
        kind,
        "/quoted dir/agent's binary".into(),
        "/work dir".into(),
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
}

#[test]
fn claude_preserves_configuration_and_removes_the_initial_prompt() {
    let prompt = "first 'line'\nsecond --line";
    let launch = parse(
        HarnessKind::Claude,
        &[
            "--model",
            "opus",
            "--effort",
            "high",
            "--autocompact",
            "auto",
            "--allowedTools",
            "Read,Edit",
            "--disallowedTools",
            "Bash",
            "--max-turns",
            "3",
            "--max-budget-usd",
            "0.25",
            "--append-system-prompt",
            "rules\n'system'",
            "--mcp-config",
            "/config dir/mcp.json",
            "--permission-mode",
            "default",
            "--session-id",
            "assigned-id",
            "--",
            prompt,
        ],
    )
    .unwrap();
    assert_eq!(launch.initial_prompt.as_deref(), Some(prompt));
    let spawn = launch.adapter().spawn_spec();
    assert_eq!(spawn.program, "/quoted dir/agent's binary");
    assert!(!spawn.args.contains(&prompt.to_string()));
    assert!(spawn
        .args
        .windows(2)
        .any(|p| p == ["--append-system-prompt", "rules\n'system'"]));
    assert!(spawn
        .args
        .windows(2)
        .any(|p| p == ["--mcp-config", "/config dir/mcp.json"]));
    assert!(spawn.args.contains(&"--replay-user-messages".into()));
    assert!(!spawn
        .args
        .contains(&"--dangerously-skip-permissions".into()));
}

#[test]
fn claude_explicit_bypass_and_resume_are_preserved() {
    let launch = parse(
        HarnessKind::Claude,
        &["--dangerously-skip-permissions", "--resume", "old-id"],
    )
    .unwrap();
    assert_eq!(launch.initial_prompt, None);
    let HostedConfig::Claude(config) = launch.config else {
        panic!()
    };
    assert_eq!(config.expected_session_id.as_deref(), Some("old-id"));
    assert_eq!(
        config.extra_args,
        ["--dangerously-skip-permissions", "--resume", "old-id"]
    );
}

#[test]
fn codex_overrides_keep_order_and_argument_boundaries() {
    let launch = parse(
        HarnessKind::Codex,
        &[
            "-c",
            "model_reasoning_effort=\"low\"",
            "-c",
            "mcp_servers.kanna-mcp.command=\"/dir with spaces/kanna-mcp\"",
            "-c",
            "model_reasoning_effort=\"high\"",
            "-m",
            "chosen-model",
            "hello\nworld",
        ],
    )
    .unwrap();
    assert_eq!(
        launch.adapter().spawn_spec().args,
        [
            "app-server",
            "-c",
            "model_reasoning_effort=\"low\"",
            "-c",
            "mcp_servers.kanna-mcp.command=\"/dir with spaces/kanna-mcp\"",
            "-c",
            "model_reasoning_effort=\"high\""
        ]
    );
    assert_eq!(launch.initial_prompt.as_deref(), Some("hello\nworld"));
    let mut adapter = launch.adapter();
    adapter.start();
    let out = adapter.on_record(&json!({"id":1,"result":{}}));
    assert_eq!(out.outgoing[1]["params"]["model"], "chosen-model");
    assert_eq!(out.outgoing[1]["params"]["cwd"], "/work dir");
}

#[test]
fn codex_sandbox_does_not_add_an_approval_bypass() {
    let launch = parse(
        HarnessKind::Codex,
        &["--sandbox", "workspace-write", "--", ""],
    )
    .unwrap();
    assert!(launch.initial_prompt.is_none());
    let mut adapter = launch.adapter();
    adapter.start();
    let out = adapter.on_record(&json!({"id":1,"result":{}}));
    assert_eq!(out.outgoing[1]["params"]["sandbox"], "workspace-write");
    assert!(out.outgoing[1]["params"].get("approvalPolicy").is_none());
}

#[test]
fn codex_yolo_maps_both_native_bypass_settings() {
    let launch = parse(HarnessKind::Codex, &["--yolo"]).unwrap();
    let mut adapter = launch.adapter();
    adapter.start();
    let out = adapter.on_record(&json!({"id":1,"result":{}}));
    assert_eq!(out.outgoing[1]["params"]["sandbox"], "danger-full-access");
    assert_eq!(out.outgoing[1]["params"]["approvalPolicy"], "never");
}

#[test]
fn codex_resume_verifies_identity_before_ready_or_input() {
    let launch = parse(HarnessKind::Codex, &["resume", "old-thread", "next turn"]).unwrap();
    let mut adapter = launch.adapter();
    adapter.start();
    let out = adapter.on_record(&json!({"id":1,"result":{}}));
    assert_eq!(out.outgoing[1]["method"], "thread/resume");
    assert_eq!(out.outgoing[1]["params"]["threadId"], "old-thread");
    assert!(adapter.send_prompt("too early").is_err());
    let out = adapter.on_record(&json!({"id":2,"result":{"thread":{"id":"old-thread"}}}));
    assert!(out
        .events
        .iter()
        .any(|e| matches!(e, AgentEvent::Ready { .. })));
    assert_eq!(
        adapter.send_prompt("next turn").unwrap()[0]["params"]["threadId"],
        "old-thread"
    );
}

#[test]
fn failed_or_mismatched_resume_never_falls_back_to_a_new_thread() {
    for response in [
        json!({"id":2,"error":{"message":"not found"}}),
        json!({"id":2,"result":{"thread":{"id":"different-thread"}}}),
    ] {
        let launch = parse(HarnessKind::Codex, &["resume", "old-thread"]).unwrap();
        let mut adapter = launch.adapter();
        adapter.start();
        adapter.on_record(&json!({"id":1,"result":{}}));
        let out = adapter.on_record(&response);
        assert!(out.outgoing.is_empty());
        assert!(out
            .events
            .iter()
            .all(|e| !matches!(e, AgentEvent::Ready { .. })));
        assert!(out
            .events
            .iter()
            .any(|e| matches!(e, AgentEvent::StartupFailed { .. })));
        assert!(adapter.send_prompt("must not send").is_err());
    }
}

#[test]
fn unsupported_or_ambiguous_options_fail_visibly() {
    for args in [
        vec!["--output-format", "text"],
        vec!["--input-format", "text"],
        vec!["--permission-prompt-tool", "other"],
        vec!["--model"],
        vec!["--session-id", "one", "--resume", "two"],
        vec!["--", "one", "two"],
    ] {
        assert!(parse(HarnessKind::Claude, &args).is_err(), "{args:?}");
    }
    for args in [
        vec!["--yolo", "--sandbox", "workspace-write"],
        vec!["--sandbox", "unknown"],
        vec!["--ask-for-approval", "unknown"],
        vec!["-c", "broken"],
        vec!["resume", "--last"],
        vec!["--allowedTools", "Read"],
    ] {
        assert!(parse(HarnessKind::Codex, &args).is_err(), "{args:?}");
    }
}
