mod common;

use agent_tui::app::{App, Phase};
use agent_tui::launch::HostedLaunch;
use agent_tui::protocol::{AgentEvent, HarnessKind};
use agent_tui::ui::skins::SkinId;
use common::record;
use serde_json::{json, Value};

fn launch(kind: HarnessKind) -> HostedLaunch {
    let args = if kind == HarnessKind::Claude {
        vec!["--session-id".into(), "session-1".into()]
    } else {
        vec![]
    };
    HostedLaunch::parse(kind, "fixture-harness".into(), "/fixture".into(), &args).unwrap()
}

fn initialize(app: &mut App) {
    app.start();
    match app.harness {
        HarnessKind::Claude => app.on_record(record(
            1,
            &json!({"type":"control_response", "response":{
                "request_id":"agent-tui-init-1", "subtype":"success", "response":{}
            }})
            .to_string(),
        )),
        HarnessKind::Codex => {
            app.on_record(record(1, &json!({"id":1,"result":{}}).to_string()));
            app.on_record(record(
                2,
                &json!({"id":2,"result":{"thread":{"id":"thread-1"}}}).to_string(),
            ));
        }
    }
    assert_eq!(app.phase, Phase::Ready);
    app.take_outbox();
}

fn echo(id: &str, text: &str) -> Value {
    json!({"type":"user", "uuid":id, "isReplay":true,
        "message":{"role":"user", "content":text}, "session_id":"session-1", "parent_tool_use_id":null})
}

#[test]
fn claude_receipt_requires_matching_uuid_content_session_and_replay_marker() {
    let mut app = App::new(launch(HarnessKind::Claude).adapter(), SkinId::Graphite);
    initialize(&mut app);
    app.send_logical_prompt("line one\nline two".into(), "delivery-1")
        .unwrap();
    let outgoing: Value = serde_json::from_str(&app.take_outbox()[0]).unwrap();
    assert_eq!(outgoing["uuid"], "delivery-1");
    assert!(app.input_receipts.is_empty(), "a write is not acceptance");
    let expected = echo("delivery-1", "line one\nline two");
    for (field, value) in [
        ("uuid", json!("wrong")),
        ("session_id", json!("wrong")),
        ("isReplay", json!(false)),
    ] {
        let mut wrong = expected.clone();
        wrong[field] = value;
        app.on_record(record(3, &wrong.to_string()));
        assert!(app.input_receipts.is_empty());
    }
    app.on_record(record(4, &echo("delivery-1", "wrong text").to_string()));
    assert!(app.input_receipts.is_empty());
    app.on_record(record(5, &expected.to_string()));
    assert_eq!(app.input_receipts, [("delivery-1".into(), Ok(()))]);
    app.on_record(record(6, &expected.to_string()));
    assert_eq!(
        app.input_receipts.len(),
        1,
        "duplicate replay does not duplicate receipt"
    );
}

#[test]
fn codex_receipt_requires_the_matched_turn_start_response_and_turn_id() {
    let mut app = App::new(launch(HarnessKind::Codex).adapter(), SkinId::Graphite);
    initialize(&mut app);
    app.send_logical_prompt("message".into(), "delivery-1")
        .unwrap();
    assert!(app.input_receipts.is_empty());
    app.on_record(record(
        3,
        &json!({"method":"turn/started","params":{"turn":{"id":"turn-1"}}}).to_string(),
    ));
    assert!(
        app.input_receipts.is_empty(),
        "notification alone cannot correlate this input"
    );
    app.on_record(record(
        4,
        &json!({"id":99,"result":{"turn":{"id":"turn-1"}}}).to_string(),
    ));
    assert!(app.input_receipts.is_empty());
    app.on_record(record(
        5,
        &json!({"id":3,"result":{"turn":{"id":"turn-1"}}}).to_string(),
    ));
    assert_eq!(app.input_receipts, [("delivery-1".into(), Ok(()))]);
}

#[test]
fn codex_rejection_is_a_failure_receipt_and_missing_turn_id_is_uncertain() {
    for (response, failure) in [
        (
            json!({"id":3,"error":{"message":"permission policy rejected"}}),
            true,
        ),
        (json!({"id":3,"result":{}}), false),
    ] {
        let mut app = App::new(launch(HarnessKind::Codex).adapter(), SkinId::Graphite);
        initialize(&mut app);
        app.send_logical_prompt("message".into(), "delivery-1")
            .unwrap();
        // Even an earlier notification cannot fill in a malformed response.
        app.on_record(record(
            3,
            &json!({"method":"turn/started","params":{"turn":{"id":"turn-1"}}}).to_string(),
        ));
        app.on_record(record(4, &response.to_string()));
        if failure {
            assert_eq!(
                app.input_receipts,
                [(
                    "delivery-1".into(),
                    Err("permission policy rejected".into())
                )]
            );
        } else {
            assert!(app.input_receipts.is_empty());
            assert!(app.degraded.is_some());
        }
    }
}

#[test]
fn logical_submission_preserves_human_draft_cursor_focus_and_slash_text() {
    for kind in [HarnessKind::Claude, HarnessKind::Codex] {
        let mut app = App::new(launch(kind).adapter(), SkinId::Graphite);
        initialize(&mut app);
        app.on_paste("human draft");
        app.on_key(common::key(crossterm::event::KeyCode::Left));
        let draft = format!("{:?}", app.composer);
        let focus = app.focus;
        app.send_logical_prompt("/new\noperator text".into(), "delivery-1")
            .unwrap();
        assert_eq!(format!("{:?}", app.composer), draft);
        assert_eq!(app.focus, focus);
        assert!(
            !app.restart_requested,
            "logical slash text cannot execute a local action"
        );
        assert_eq!(app.take_outbox().len(), 1);
        assert!(app
            .send_logical_prompt("second".into(), "delivery-2")
            .is_err());
        assert!(app.take_outbox().is_empty());
    }
}

#[test]
fn pending_card_refuses_logical_input_without_answering_or_taking_focus() {
    let mut app = App::new(launch(HarnessKind::Codex).adapter(), SkinId::Graphite);
    initialize(&mut app);
    app.on_record(record(
        3,
        &json!({"id":50,"method":"item/commandExecution/requestApproval",
        "params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","command":"echo hi",
            "availableDecisions":["accept","decline"]}})
        .to_string(),
    ));
    let focus = app.focus;
    assert!(app
        .send_logical_prompt("accept".into(), "delivery-1")
        .is_err());
    assert_eq!(app.focus, focus);
    assert!(app.take_outbox().is_empty());
    assert_eq!(app.transcript.pending_cards().count(), 1);
}

#[test]
fn claude_identity_mismatch_stops_further_input() {
    let mut adapter = launch(HarnessKind::Claude).adapter();
    adapter.start();
    adapter.on_record(&json!({"type":"control_response","response":{
        "request_id":"agent-tui-init-1","subtype":"success","response":{}}}));
    let out = adapter
        .on_record(&json!({"type":"system","subtype":"init","session_id":"another-session"}));
    assert!(out
        .events
        .iter()
        .any(|e| matches!(e, AgentEvent::StartupFailed { .. })));
    assert!(adapter
        .send_logical_prompt("must not send", "delivery-1")
        .is_err());
}

#[test]
fn recorded_claude_2_1_284_echo_acknowledges_the_supplied_uuid() {
    let records = common::load("claude/hosted_ack.transcript");
    let common::Line::Out(sent) = &records[0] else {
        panic!()
    };
    let common::Line::In(echoed) = &records[1] else {
        panic!()
    };
    let echoed: Value = serde_json::from_str(echoed).unwrap();
    let session = echoed["session_id"].as_str().unwrap();
    let launch = HostedLaunch::parse(
        HarnessKind::Claude,
        "claude".into(),
        "/fixture".into(),
        &["--session-id".into(), session.into()],
    )
    .unwrap();
    let mut app = App::new(launch.adapter(), SkinId::Graphite);
    initialize(&mut app);
    let uuid = sent["uuid"].as_str().unwrap();
    app.send_logical_prompt(sent["message"]["content"].as_str().unwrap().into(), uuid)
        .unwrap();
    let actual: Value = serde_json::from_str(&app.take_outbox()[0]).unwrap();
    assert_eq!(&actual, sent, "adapter emits the live-probed user request");
    app.on_record(record(3, &echoed.to_string()));
    assert_eq!(app.input_receipts, [(uuid.into(), Ok(()))]);
}

#[test]
fn hosted_provider_notices_require_structured_rejection() {
    use kanna_agent_protocol::hosted_frontend::NoticeKind;
    let mut claude = App::new(launch(HarnessKind::Claude).adapter(), SkinId::Graphite);
    initialize(&mut claude);
    claude.on_record(record(
        10,
        &json!({"type":"rate_limit_event", "rate_limit_info":{
        "status":"allowed", "overageStatus":"rejected"}})
        .to_string(),
    ));
    assert!(claude.provider_notice.is_none());
    claude.on_record(record(
        11,
        &json!({"type":"rate_limit_event", "rate_limit_info":{
        "status":"rejected", "rateLimitType":"five_hour"}})
        .to_string(),
    ));
    let notice = claude.provider_notice.unwrap();
    assert_eq!(notice.kind, NoticeKind::QuotaRejected);
    assert_eq!(notice.scope.as_deref(), Some("five_hour"));
    for (code, expected) in [
        ("usageLimitExceeded", Some(NoticeKind::QuotaRejected)),
        ("serverOverloaded", Some(NoticeKind::CapacityRefused)),
        ("other", None),
    ] {
        let mut codex = App::new(launch(HarnessKind::Codex).adapter(), SkinId::Graphite);
        initialize(&mut codex);
        codex.on_record(record(
            10,
            &json!({"method":"turn/completed", "params":{"threadId":"thread-1",
            "turn":{"id":"turn-1", "status":"failed", "error":{"message":"quota exceeded",
                "codexErrorInfo":code}}}})
            .to_string(),
        ));
        assert_eq!(codex.provider_notice.map(|n| n.kind), expected);
    }
}
