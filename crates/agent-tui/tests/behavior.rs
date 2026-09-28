//! State-machine and interaction behavior from the design's acceptance
//! examples, driven with synthetic records shaped like the recorded ones.

mod common;

use agent_tui::app::transcript::{CardState, EntryKind};
use agent_tui::app::{App, Focus, Overlay, Phase, Status, Target};
use agent_tui::protocol::{HarnessKind, MetaSource, ToolStatus};
use agent_tui::ui::skins::SkinId;
use common::*;
use crossterm::event::KeyCode;
use serde_json::{json, Value};

fn feed(app: &mut App, v: Value) {
    app.on_record(record(0, &v.to_string()));
}

fn ready_codex() -> App {
    let mut app = app(HarnessKind::Codex);
    app.start();
    outbox(&mut app);
    feed(&mut app, json!({"id": 1, "result": {"userAgent": "x"}}));
    let out = outbox(&mut app);
    assert_eq!(out[0]["method"], "initialized");
    assert_eq!(out[1]["method"], "thread/start");
    feed(
        &mut app,
        json!({"id": 2, "result": {"thread": {"id": "t1"}, "model": "gpt-test", "reasoningEffort": "low", "approvalPolicy": "untrusted", "cwd": "/w", "modelProvider": "openai", "sandbox": {}}}),
    );
    assert_eq!(app.status(), Status::Ready);
    app
}

fn ready_claude() -> App {
    let mut app = app(HarnessKind::Claude);
    app.start();
    let init = outbox(&mut app);
    let id = init[0]["request_id"].as_str().unwrap().to_string();
    feed(
        &mut app,
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"commands": [{"name": "compact", "description": "Compact the conversation", "argumentHint": "<instructions>"}]}}}),
    );
    assert_eq!(app.status(), Status::Ready);
    app
}

fn send(app: &mut App, text: &str) -> Vec<Value> {
    type_text(app, text);
    app.on_key(key(KeyCode::Enter));
    outbox(app)
}

/// Codex: turn/start answered with a turn id.
fn codex_turn(app: &mut App, text: &str) {
    let out = send(app, text);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["method"], "turn/start");
    let id = out[0]["id"].clone();
    feed(
        app,
        json!({"id": id, "result": {"turn": {"id": "turn-1", "status": "inProgress"}}}),
    );
    feed(
        app,
        json!({"method": "turn/started", "params": {"threadId": "t1", "turn": {"id": "turn-1"}}}),
    );
}

fn codex_complete(app: &mut App, status: &str) {
    feed(
        app,
        json!({"method": "turn/completed", "params": {"threadId": "t1", "turn": {"id": "turn-1", "status": status, "error": null, "durationMs": 1200}}}),
    );
}

#[test]
fn enter_while_working_keeps_draft_and_does_not_send_or_queue() {
    let mut app = ready_codex();
    codex_turn(&mut app, "first");
    assert_eq!(app.status(), Status::Working);
    // Drafting while streaming loses no keystrokes.
    feed(
        &mut app,
        json!({"method": "item/agentMessage/delta", "params": {"itemId": "m1", "delta": "Hel"}}),
    );
    type_text(&mut app, "Also check ");
    feed(
        &mut app,
        json!({"method": "item/agentMessage/delta", "params": {"itemId": "m1", "delta": "lo"}}),
    );
    type_text(&mut app, "CRLF input.");
    assert_eq!(app.composer.text(), "Also check CRLF input.");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    assert!(
        outbox(&mut app).is_empty(),
        "no send and no silent queue while working"
    );
    assert_eq!(app.composer.text(), "Also check CRLF input.");
    assert!(app
        .hint
        .as_ref()
        .unwrap()
        .text
        .contains("Turn still running"));
    codex_complete(&mut app, "completed");
    assert!(
        outbox(&mut app).is_empty(),
        "nothing is sent automatically when the turn ends"
    );
    // Now the person explicitly sends it: exactly one send.
    app.on_key(key(KeyCode::Enter));
    let out = outbox(&mut app);
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0]["params"]["input"][0]["text"],
        "Also check CRLF input."
    );
    assert_eq!(
        out[0]["params"]["threadId"], "t1",
        "follow-up goes to the same thread"
    );
    app.on_key(key(KeyCode::Enter));
    assert!(
        outbox(&mut app).is_empty(),
        "empty composer after send: second Enter sends nothing"
    );
}

#[test]
fn deltas_grow_in_place_without_duplicate_text() {
    let mut app = ready_codex();
    codex_turn(&mut app, "hi");
    feed(
        &mut app,
        json!({"method": "item/started", "params": {"item": {"type": "agentMessage", "id": "m1", "text": ""}}}),
    );
    for d in ["Hel", "lo ", "world"] {
        feed(
            &mut app,
            json!({"method": "item/agentMessage/delta", "params": {"itemId": "m1", "delta": d}}),
        );
    }
    feed(
        &mut app,
        json!({"method": "item/completed", "params": {"item": {"type": "agentMessage", "id": "m1", "text": "Hello world"}}}),
    );
    let texts: Vec<String> = app
        .transcript
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["Hello world".to_string()]);
}

#[test]
fn overlapping_tools_stay_separate() {
    let mut app = ready_codex();
    codex_turn(&mut app, "run two");
    let started = |id: &str, cmd: &str| json!({"method": "item/started", "params": {"item": {"type": "commandExecution", "id": id, "command": cmd, "cwd": "/w", "status": "inProgress", "commandActions": [{"type": "unknown", "command": cmd}]}}});
    let delta = |id: &str, d: &str| json!({"method": "item/commandExecution/outputDelta", "params": {"itemId": id, "delta": d, "threadId": "t1", "turnId": "turn-1"}});
    feed(&mut app, started("a", "make test"));
    feed(&mut app, started("b", "make lint"));
    feed(&mut app, delta("a", "test 1\n"));
    feed(&mut app, delta("b", "lint ok\n"));
    feed(&mut app, delta("a", "test 2\n"));
    feed(
        &mut app,
        json!({"method": "item/completed", "params": {"item": {"type": "commandExecution", "id": "b", "command": "make lint", "status": "completed", "exitCode": 0, "aggregatedOutput": "lint ok\n", "durationMs": 5}}}),
    );
    let group = app
        .transcript
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Tools(g) => Some(g.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(group.calls.len(), 2);
    assert_eq!(group.calls[0].output, "test 1\ntest 2\n");
    assert_eq!(group.calls[0].status, ToolStatus::Running);
    assert_eq!(group.calls[1].output, "lint ok\n");
    assert_eq!(group.calls[1].status, ToolStatus::Succeeded);
    let c = group.counts();
    assert_eq!((c.completed, c.running), (1, 1));
    assert!(!group.expanded, "tools group is collapsed by default");
}

#[test]
fn approval_answers_exactly_the_focused_request_once() {
    let mut app = ready_codex();
    codex_turn(&mut app, "do things");
    let req = |id: i64, cmd: &str| json!({"method": "item/commandExecution/requestApproval", "id": id, "params": {"threadId": "t1", "turnId": "turn-1", "itemId": format!("i{id}"), "command": cmd, "availableDecisions": ["accept", "cancel"]}});
    feed(&mut app, req(40, "rm -rf build"));
    feed(&mut app, req(41, "npm install"));
    assert_eq!(app.status(), Status::NeedsApproval);
    // Enter in the composer (meant for a draft) must not approve anything.
    type_text(&mut app, "draft");
    app.on_key(key(KeyCode::Enter));
    assert!(outbox(&mut app).is_empty());
    assert_eq!(app.focus, Focus::Composer);
    // Esc on a focused card leaves it pending.
    app.on_key(key(KeyCode::Tab));
    assert!(matches!(app.focus, Focus::Card(_)));
    app.on_key(key(KeyCode::Esc));
    assert!(outbox(&mut app).is_empty());
    assert_eq!(app.transcript.pending_cards().count(), 2);
    // Deny the first one (decline is always offered; see fixtures).
    app.on_key(key(KeyCode::Tab));
    let Focus::Card(first) = app.focus else {
        panic!()
    };
    let labels: Vec<String> = match &app.transcript.get(first).unwrap().kind {
        EntryKind::Approval(c) => c.choices.iter().map(|c| c.label.clone()).collect(),
        _ => panic!(),
    };
    assert_eq!(labels, vec!["Allow once", "Deny", "Deny and stop turn"]);
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Enter));
    let out = outbox(&mut app);
    assert_eq!(
        out,
        vec![json!({"jsonrpc": "2.0", "id": 40, "result": {"decision": "decline"}})]
    );
    // The second is untouched and still pending; answering again sends nothing.
    assert_eq!(app.transcript.pending_cards().count(), 1);
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        outbox(&mut app),
        vec![json!({"jsonrpc": "2.0", "id": 41, "result": {"decision": "accept"}})]
    );
    assert_eq!(app.transcript.pending_cards().count(), 0);
    assert_eq!(app.composer.text(), "draft", "draft preserved throughout");
}

#[test]
fn claude_approval_routes_by_request_id() {
    let mut app = ready_claude();
    let out = send(&mut app, "go");
    assert_eq!(out[0]["type"], "user");
    feed(
        &mut app,
        json!({"type": "control_request", "request_id": "r-9", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}, "tool_use_id": "toolu_1"}}),
    );
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter));
    let out = outbox(&mut app);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["response"]["request_id"], "r-9");
    assert_eq!(out[0]["response"]["response"]["behavior"], "allow");
    assert_eq!(
        out[0]["response"]["response"]["updatedInput"],
        json!({"command": "ls"})
    );
}

#[test]
fn question_answer_routed_to_its_request() {
    // item/tool/requestUserInput follows the generated schema (not seen live).
    let mut app = ready_codex();
    codex_turn(&mut app, "ask me");
    feed(
        &mut app,
        json!({"method": "item/tool/requestUserInput", "id": "q-1", "params": {"threadId": "t1", "turnId": "turn-1", "itemId": "i1", "isBlocking": true, "questions": [
            {"id": "color", "header": "Color", "question": "Red or blue?", "options": [{"label": "Red", "description": ""}, {"label": "Blue", "description": ""}]},
            {"id": "why", "header": "Why", "question": "Why?", "options": null}
        ]}}),
    );
    assert_eq!(app.status(), Status::NeedsInput);
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert!(outbox(&mut app).is_empty(), "waits for every question");
    type_text(&mut app, "it is calm");
    app.on_key(key(KeyCode::Enter));
    let out = outbox(&mut app);
    assert_eq!(
        out,
        vec![
            json!({"jsonrpc": "2.0", "id": "q-1", "result": {"answers": {"color": {"answers": ["Blue"]}, "why": {"answers": ["it is calm"]}}}})
        ]
    );
    assert_eq!(
        app.composer.text(),
        "",
        "typing in the focused card did not leak into the draft"
    );
}

#[test]
fn interrupt_shows_stopping_then_only_turn_stopped() {
    let mut app = ready_codex();
    codex_turn(&mut app, "long");
    type_text(&mut app, "keep me");
    app.on_key(ctrl('c'));
    assert_eq!(app.status(), Status::Stopping);
    let out = outbox(&mut app);
    assert_eq!(out[0]["method"], "turn/interrupt");
    assert_eq!(
        out[0]["params"],
        json!({"threadId": "t1", "turnId": "turn-1"})
    );
    app.on_key(ctrl('c'));
    assert!(
        outbox(&mut app).is_empty(),
        "a second Ctrl+C does not resend"
    );
    codex_complete(&mut app, "interrupted");
    let last = app.transcript.iter().last().unwrap();
    match &last.kind {
        EntryKind::TurnEnd { text, .. } => assert_eq!(text, "■ Turn stopped"),
        other => panic!("{other:?}"),
    }
    assert_eq!(app.status(), Status::Ready);
    assert_eq!(app.composer.text(), "keep me");
}

#[test]
fn interrupt_before_turn_id_is_known_is_sent_when_it_arrives() {
    let mut app = ready_codex();
    let out = send(&mut app, "x");
    app.on_key(ctrl('c'));
    assert!(outbox(&mut app).is_empty());
    feed(
        &mut app,
        json!({"id": out[0]["id"], "result": {"turn": {"id": "turn-7"}}}),
    );
    let out = outbox(&mut app);
    assert_eq!(out[0]["method"], "turn/interrupt");
    assert_eq!(out[0]["params"]["turnId"], "turn-7");
}

#[test]
fn eof_disconnects_keeping_draft_and_transcript_without_resend() {
    let mut app = ready_claude();
    send(&mut app, "hello");
    feed(
        &mut app,
        json!({"type": "assistant", "message": {"id": "m", "model": "claude-x", "content": [{"type": "text", "text": "partial"}]}}),
    );
    type_text(&mut app, "unsent draft");
    app.on_disconnect("Claude exited (exit code 1)", &["boom".to_string()]);
    assert_eq!(app.status(), Status::Disconnected);
    assert!(matches!(app.phase, Phase::Disconnected(_)));
    assert!(
        outbox(&mut app).is_empty(),
        "the possibly accepted prompt is never resent"
    );
    app.on_key(key(KeyCode::Enter));
    assert!(outbox(&mut app).is_empty());
    assert_eq!(app.composer.text(), "unsent draft");
    assert!(app.hint.as_ref().unwrap().text.contains("/new"));
    let last = app.transcript.iter().last().unwrap();
    assert!(
        matches!(&last.kind, EntryKind::Notice { text, .. } if text.contains("Disconnected") && text.contains("boom"))
    );
    assert!(app
        .transcript
        .iter()
        .any(|e| matches!(&e.kind, EntryKind::Assistant { text, .. } if text == "partial")));
    // Not a successful completion.
    assert!(!app
        .transcript
        .iter()
        .any(|e| matches!(&e.kind, EntryKind::TurnEnd { .. })));
    // /new asks for a restart.
    app.composer.clear();
    type_text(&mut app, "/new");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    assert!(app.restart_requested);
}

#[test]
fn malformed_control_record_degrades_until_turn_completes() {
    let mut app = ready_claude();
    send(&mut app, "hi");
    app.on_record(record(
        12,
        "{\"type\":\"control_request\",\"request_id\":\"r1\",\"request\":{\"subtype\":\"can_use_to",
    ));
    assert_eq!(app.status(), Status::Degraded);
    // A plain garbage line is only a diagnostic, and later records still flow.
    app.on_record(record(13, "not json"));
    feed(
        &mut app,
        json!({"type": "assistant", "message": {"id": "m", "content": [{"type": "text", "text": "still readable"}]}}),
    );
    assert!(app
        .transcript
        .iter()
        .any(|e| matches!(&e.kind, EntryKind::Assistant { text, .. } if text == "still readable")));
    feed(
        &mut app,
        json!({"type": "result", "subtype": "success", "is_error": false, "duration_ms": 10}),
    );
    assert_eq!(app.status(), Status::Ready);
    type_text(&mut app, "again");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        outbox(&mut app).len(),
        1,
        "sending works once state is re-established"
    );
    // The malformed line is inspectable with its line number.
    let bad = app.raw.iter().find(|r| r.malformed).unwrap();
    assert_eq!(bad.line_no, Some(12));
    let notice = app.transcript.iter().find_map(|e| match &e.kind {
        EntryKind::Notice { text, .. } if text.contains("line 13") => Some(text.clone()),
        _ => None,
    });
    assert!(notice.is_some());
}

#[test]
fn degraded_blocks_sends() {
    let mut app = ready_codex();
    app.on_record(record(
        3,
        "{\"method\":\"item/commandExecution/requestApproval\",\"id\":5,",
    ));
    assert_eq!(app.status(), Status::Degraded);
    type_text(&mut app, "x");
    app.on_key(key(KeyCode::Enter));
    assert!(outbox(&mut app).is_empty());
    assert!(app.hint.as_ref().unwrap().text.contains("degraded"));
}

#[test]
fn unsupported_requests_get_errors_never_approvals() {
    let mut app = ready_codex();
    feed(
        &mut app,
        json!({"method": "item/tool/call", "id": 77, "params": {"tool": "x"}}),
    );
    let out = outbox(&mut app);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["id"], 77);
    assert!(out[0].get("error").is_some());
    assert!(out[0].get("result").is_none());
    let mut app = ready_claude();
    feed(
        &mut app,
        json!({"type": "control_request", "request_id": "h1", "request": {"subtype": "hook_callback"}}),
    );
    let out = outbox(&mut app);
    assert_eq!(out[0]["response"]["subtype"], "error");
    assert_eq!(out[0]["response"]["request_id"], "h1");
}

#[test]
fn unknown_events_stay_inspectable() {
    let mut app = ready_claude();
    feed(&mut app, json!({"type": "brand_new_event", "payload": 1}));
    app.on_key(ctrl('r'));
    match &app.overlay {
        Overlay::Inspector { records, .. } => {
            let found = records.iter().any(|id| {
                app.raw
                    .get(*id)
                    .is_some_and(|r| r.text.contains("brand_new_event"))
            });
            assert!(found);
        }
        other => panic!("{other:?}"),
    }
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn slash_menu_completes_then_runs_and_esc_keeps_text() {
    let mut app = ready_claude();
    type_text(&mut app, "/");
    assert!(app.slash.open);
    let n = app.slash.items.len();
    assert_eq!(n, 7, "6 local + 1 harness command");
    app.on_key(key(KeyCode::Up));
    assert_eq!(app.slash.selected, n - 1, "wraps upward");
    app.on_key(ctrl('n'));
    assert_eq!(app.slash.selected, 0, "Ctrl+N wraps down");
    type_text(&mut app, "th");
    app.on_key(key(KeyCode::Esc));
    assert!(!app.slash.open);
    assert_eq!(app.composer.text(), "/th", "Esc keeps text");
    type_text(&mut app, "e");
    assert!(app.slash.open);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.composer.text(), "/theme ");
    type_text(&mut app, "matrix");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.skin, SkinId::Matrix);
    assert!(
        outbox(&mut app).is_empty(),
        "local commands never reach the harness"
    );
    // A harness command is labelled and sent as the user message.
    type_text(&mut app, "/com");
    assert_eq!(app.slash.current().unwrap().name, "compact");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    let out = outbox(&mut app);
    assert_eq!(out[0]["message"]["content"], "/compact");
    // Codex exposes no harness commands; unknown ones are refused.
    let mut codex = ready_codex();
    type_text(&mut codex, "/compact");
    app.on_key(key(KeyCode::Esc));
    codex.on_key(key(KeyCode::Enter));
    assert!(outbox(&mut codex).is_empty());
    assert!(codex
        .hint
        .as_ref()
        .unwrap()
        .text
        .contains("Unknown command /compact"));
}

#[test]
fn skin_quote_is_fixed_once_selected() {
    let mut app = ready_claude();
    app.set_skin(SkinId::Duke);
    let q = app.quote;
    assert!(q < SkinId::Duke.quotes().len());
    for _ in 0..100 {
        app.tick(std::time::Instant::now());
        app.set_skin(SkinId::Duke);
    }
    assert_eq!(
        app.quote, q,
        "no rotation: the quote stays while the skin stays"
    );
}

#[test]
fn header_meta_prefers_reported_over_requested() {
    let mut app = app_with(HarnessKind::Claude, Some("haiku"), Some("low"));
    app.start();
    let id = outbox(&mut app)[0]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    feed(
        &mut app,
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"commands": [], "models": [{"value": "haiku", "resolvedModel": "claude-haiku-4-5"}]}}}),
    );
    assert_eq!(app.meta.model.as_ref().unwrap().value, "claude-haiku-4-5");
    assert_eq!(
        app.meta.model.as_ref().unwrap().source,
        MetaSource::Requested
    );
    assert_eq!(
        app.meta.effort.as_ref().unwrap().source,
        MetaSource::Requested
    );
    feed(
        &mut app,
        json!({"type": "system", "subtype": "init", "model": "claude-haiku-4-5-20251001", "session_id": "s"}),
    );
    assert_eq!(
        app.meta.model.as_ref().unwrap().source,
        MetaSource::Reported
    );
    assert_eq!(
        app.meta.effort.as_ref().unwrap().value,
        "low",
        "effort stays marked as requested"
    );
    let plain = ready_claude();
    assert!(
        plain.meta.effort.is_none(),
        "unreported effort shows as Not reported"
    );
}

#[test]
fn follow_live_pauses_on_scroll_and_counts_new() {
    let mut app = ready_codex();
    for i in 0..30 {
        codex_turn(&mut app, &format!("message {i}"));
        codex_complete(&mut app, "completed");
    }
    codex_turn(&mut app, "more");
    app.view.height = 10;
    app.view.total = 200;
    app.on_key(key(KeyCode::PageUp));
    assert!(!app.view.follow);
    let top = app.view.top;
    feed(
        &mut app,
        json!({"method": "item/agentMessage/delta", "params": {"itemId": "new", "delta": "streaming…"}}),
    );
    feed(
        &mut app,
        json!({"method": "item/started", "params": {"item": {"type": "commandExecution", "id": "c", "command": "ls", "status": "inProgress"}}}),
    );
    assert_eq!(app.view.top, top, "incoming output does not move the view");
    assert!(!app.view.follow);
    assert!(app.view.new_count >= 1);
    assert_eq!(
        app.focus,
        Focus::Composer,
        "incoming output never steals focus"
    );
    app.on_key(ctrl('l'));
    assert!(app.view.follow);
    assert_eq!(app.view.new_count, 0);
}

#[test]
fn search_finds_tool_output_and_keeps_draft() {
    let mut app = ready_codex();
    codex_turn(&mut app, "run");
    feed(
        &mut app,
        json!({"method": "item/started", "params": {"item": {"type": "commandExecution", "id": "a", "command": "make", "status": "inProgress"}}}),
    );
    feed(
        &mut app,
        json!({"method": "item/completed", "params": {"item": {"type": "commandExecution", "id": "a", "command": "make", "status": "completed", "exitCode": 0, "aggregatedOutput": "needle found here"}}}),
    );
    codex_complete(&mut app, "completed");
    type_text(&mut app, "my draft");
    app.on_key(ctrl('f'));
    assert_eq!(app.focus, Focus::Search);
    type_text(&mut app, "NEEDLE");
    let s = app.search.as_ref().unwrap();
    assert_eq!(s.matches.len(), 1);
    assert!(matches!(s.matches[0], Target::Call(_, 0)));
    // The match is revealed: group and call expanded, follow paused.
    let group = app
        .transcript
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Tools(g) => Some(g.clone()),
            _ => None,
        })
        .unwrap();
    assert!(group.expanded && group.calls[0].expanded);
    assert!(!app.view.follow);
    app.on_key(key(KeyCode::Esc));
    assert!(app.search.is_none());
    assert_eq!(app.composer.text(), "my draft");
}

#[test]
fn transcript_navigation_expands_levels_independently() {
    let mut app = ready_claude();
    send(&mut app, "use mcp");
    feed(
        &mut app,
        json!({"type": "assistant", "message": {"id": "m", "content": [{"type": "tool_use", "id": "t1", "name": "mcp__docs__search", "input": {"q": "x"}}]}}),
    );
    feed(
        &mut app,
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "ok"}]}]}, "tool_use_result": [{"type": "text", "text": "ok"}]}),
    );
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Transcript);
    // Last target is the tools group; expand it, then the call, then raw input.
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Down));
    assert!(matches!(app.view.selected, Some(Target::RawIn(_, 0))));
    app.on_key(key(KeyCode::Enter));
    let call = app
        .transcript
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Tools(g) => Some(g.calls[0].clone()),
            _ => None,
        })
        .unwrap();
    assert!(call.expanded && call.raw_in_open && !call.raw_out_open);
    app.on_key(ctrl('r'));
    match &app.overlay {
        Overlay::Inspector { records, title, .. } => {
            assert!(title.contains("t1"));
            assert_eq!(
                records.len(),
                2,
                "tool_use and tool_result records, in order"
            );
        }
        o => panic!("{o:?}"),
    }
}

#[test]
fn quit_while_working_asks_first() {
    let mut app = ready_codex();
    codex_turn(&mut app, "work");
    app.on_key(ctrl('q'));
    assert!(matches!(app.overlay, Overlay::QuitConfirm { .. }));
    assert!(!app.should_quit);
    app.on_key(key(KeyCode::Right));
    app.on_key(key(KeyCode::Enter));
    assert!(!app.should_quit, "keep working");
    app.on_key(ctrl('q'));
    app.on_key(key(KeyCode::Enter));
    assert!(app.should_quit);
    assert_eq!(
        outbox(&mut app)[0]["method"],
        "turn/interrupt",
        "stop & quit interrupts the turn"
    );
    let mut idle = ready_codex();
    idle.on_key(ctrl('q'));
    assert!(idle.should_quit);
}

#[test]
fn paste_stays_a_draft() {
    let mut app = ready_codex();
    app.on_paste("line one\r\nline two\n");
    assert!(outbox(&mut app).is_empty());
    assert_eq!(app.composer.text(), "line one\nline two\n");
    app.on_key(shift(KeyCode::Enter));
    assert_eq!(
        app.composer.text(),
        "line one\nline two\n\n",
        "Shift+Enter inserts a newline"
    );
}

#[test]
fn stale_card_answer_after_resolution_sends_nothing() {
    let mut app = ready_codex();
    codex_turn(&mut app, "x");
    feed(
        &mut app,
        json!({"method": "item/fileChange/requestApproval", "id": 3, "params": {"threadId": "t1", "turnId": "turn-1", "itemId": "f1"}}),
    );
    feed(
        &mut app,
        json!({"method": "serverRequest/resolved", "params": {"threadId": "t1", "requestId": 3}}),
    );
    let card = app
        .transcript
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Approval(c) => Some(c.state.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(card, CardState::Resolved);
    app.on_key(key(KeyCode::Tab));
    assert_ne!(app.focus, Focus::Card(0));
    app.on_key(key(KeyCode::Enter));
    assert!(outbox(&mut app).is_empty());
}

fn run_new(app: &mut App) {
    app.composer.clear();
    type_text(app, "/new");
    app.on_key(key(KeyCode::Enter)); // completes the menu item
    app.on_key(key(KeyCode::Enter)); // runs it
}

#[test]
fn new_session_escapes_stopping_after_malformed_completion() {
    let mut app = ready_claude();
    send(&mut app, "hi");
    // The turn's completion arrives malformed: state is uncertain.
    app.on_record(record(
        40,
        "{\"type\":\"result\",\"subtype\":\"success\",\"is_err",
    ));
    assert_eq!(app.status(), Status::Degraded);
    // Following the notice's advice: Ctrl+C, which can never be acknowledged…
    app.on_key(ctrl('c'));
    assert_eq!(app.phase, Phase::Stopping);
    outbox(&mut app);
    app.on_key(ctrl('c'));
    assert_eq!(app.phase, Phase::Stopping);
    // …then /new, which must be accepted.
    run_new(&mut app);
    assert!(app.restart_requested, "hint was {:?}", app.hint);
    app.reset_session(agent_tui::harness::make_adapter(
        &agent_tui::harness::HarnessOptions {
            kind: HarnessKind::Claude,
            model: None,
            effort: None,
            cwd: None,
            program: None,
            extra_args: vec![],
        },
    ));
    assert_eq!(app.status(), Status::Starting);
    assert!(app.degraded.is_none());
}

#[test]
fn new_session_escapes_degraded_while_idle() {
    let mut app = ready_codex();
    app.on_record(record(
        9,
        "{\"method\":\"turn/completed\",\"params\":{\"thr",
    ));
    assert_eq!(app.status(), Status::Degraded);
    run_new(&mut app);
    assert!(app.restart_requested);
}

#[test]
fn new_session_still_refused_during_a_healthy_turn() {
    let mut app = ready_codex();
    codex_turn(&mut app, "work");
    run_new(&mut app);
    assert!(!app.restart_requested);
    assert!(app.hint.as_ref().unwrap().text.contains("Ctrl+C"));
}

#[test]
fn url_mode_elicitation_shows_the_url_before_allow() {
    // Shape from the generated schema (McpServerElicitationRequestParams, mode "url"); not seen live.
    let mut app = ready_codex();
    codex_turn(&mut app, "sign in");
    feed(
        &mut app,
        json!({"method": "mcpServer/elicitation/request", "id": 12, "params": {
            "threadId": "t1", "turnId": "turn-1", "serverName": "github", "mode": "url",
            "elicitationId": "e-1", "message": "Authorize access", "url": "https://example.com/oauth?x=\u{1b}[31m1"
        }}),
    );
    let card = app
        .transcript
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Approval(c) => Some(c.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        card.details[0].starts_with("URL: https://example.com/oauth?x="),
        "{:?}",
        card.details
    );
    assert_eq!(card.choices[0].label, "Allow");
    // The rendered card shows the URL with the escape neutralized.
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    let mut cache = agent_tui::ui::RenderCache::default();
    term.draw(|f| {
        agent_tui::ui::draw(
            f,
            &mut app,
            &mut cache,
            agent_tui::ui::skins::ColorMode::TrueColor,
        )
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                + "\n"
        })
        .collect();
    assert!(
        text.contains("URL: https://example.com/oauth?x=␛[31m1"),
        "{text}"
    );
    app.on_key(key(KeyCode::Tab));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        outbox(&mut app),
        vec![json!({"jsonrpc": "2.0", "id": 12, "result": {"action": "accept", "content": null}})]
    );
}

#[test]
fn url_mode_elicitation_without_url_cannot_be_allowed() {
    let mut app = ready_codex();
    codex_turn(&mut app, "x");
    feed(
        &mut app,
        json!({"method": "mcpServer/elicitation/request", "id": 13, "params": {
            "threadId": "t1", "serverName": "github", "mode": "url", "elicitationId": "e-2", "message": "Authorize"
        }}),
    );
    let card = app
        .transcript
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Approval(c) => Some(c.clone()),
            _ => None,
        })
        .unwrap();
    assert!(card.choices.iter().all(|c| c.label != "Allow"));
}
