//! Drives the real `App` + adapters through transcripts recorded from live
//! claude 2.1.283 / codex-cli 0.157.1 sessions. Every record the spike sent is
//! reproduced by a UI action (typing, Enter, Tab to a card, Ctrl+C) and the
//! message the app emits is checked against what the harness accepted.

mod common;

use agent_tui::app::transcript::{CardState, EntryKind};
use agent_tui::app::{App, Focus, Status};
use agent_tui::protocol::{HarnessKind, MetaSource, ToolKind, ToolStatus};
use agent_tui::ui::transcript::tools_summary;
use common::*;
use crossterm::event::KeyCode;
use serde_json::Value;

fn s<'a>(v: &'a Value, p: &str) -> &'a str {
    v.pointer(p).and_then(Value::as_str).unwrap_or("")
}

fn matches(kind: HarnessKind, expected: &Value, got: &Value) -> bool {
    match kind {
        HarnessKind::Claude => match s(expected, "/type") {
            "control_request" => {
                s(got, "/type") == "control_request"
                    && s(got, "/request/subtype") == s(expected, "/request/subtype")
            }
            "user" => {
                s(got, "/type") == "user"
                    && got.pointer("/message/content") == expected.pointer("/message/content")
            }
            "control_response" => {
                let (e, g) = (&expected["response"], &got["response"]);
                if s(g, "/request_id") != s(e, "/request_id")
                    || s(g, "/response/behavior") != s(e, "/response/behavior")
                {
                    return false;
                }
                if s(e, "/response/behavior") != "allow" {
                    return true;
                }
                let lower = |v: &Value| serde_json::to_string(v).unwrap().to_lowercase();
                lower(&e["response"]["updatedInput"]) == lower(&g["response"]["updatedInput"])
            }
            _ => false,
        },
        HarnessKind::Codex => {
            if expected.get("method").is_some() {
                let same = got.get("method") == expected.get("method")
                    && got.get("id") == expected.get("id");
                match s(expected, "/method") {
                    "turn/start" => {
                        same && got.pointer("/params/input/0/text")
                            == expected.pointer("/params/input/0/text")
                    }
                    "turn/interrupt" => same && got.get("params") == expected.get("params"),
                    _ => same,
                }
            } else {
                got.get("id") == expected.get("id") && got.get("result") == expected.get("result")
            }
        }
    }
}

fn choose_label(kind: HarnessKind, sent: &Value) -> Option<String> {
    match kind {
        HarnessKind::Claude => match s(sent, "/response/response/behavior") {
            "deny" => Some("Deny".into()),
            "allow"
                if sent
                    .pointer("/response/response/updatedInput/answers")
                    .is_none() =>
            {
                Some("Allow once".into())
            }
            _ => None,
        },
        HarnessKind::Codex => {
            let r = &sent["result"];
            if let Some(d) = r.get("decision").and_then(Value::as_str) {
                return Some(
                    match d {
                        "accept" => "Allow once",
                        "acceptForSession" => "Allow for session",
                        "decline" => "Deny",
                        "cancel" => "Deny and stop turn",
                        other => other,
                    }
                    .into(),
                );
            }
            r.get("action").and_then(Value::as_str).map(|a| match a {
                "accept" => "Allow".into(),
                "decline" => "Decline".into(),
                _ => "Cancel".into(),
            })
        }
    }
}

/// Focuses the pending card with Tab and answers it like the spike did.
fn answer(app: &mut App, kind: HarnessKind, sent: &Value) {
    let card = app
        .transcript
        .first_pending_card()
        .expect("a pending card to answer");
    // A new card must not have stolen focus.
    assert_eq!(
        app.focus,
        Focus::Composer,
        "card arrived without taking focus"
    );
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Card(card));
    let entry = app.transcript.get(card).unwrap().clone();
    match &entry.kind {
        EntryKind::Approval(c) => {
            let label = choose_label(kind, sent).expect("approval decision");
            let idx = c
                .choices
                .iter()
                .position(|ch| ch.label == label)
                .unwrap_or_else(|| panic!("no choice {label:?} in {:?}", c.choices));
            for _ in 0..idx {
                app.on_key(key(KeyCode::Right));
            }
            app.on_key(key(KeyCode::Enter));
        }
        EntryKind::Question(q) => {
            let answers = sent
                .pointer("/response/response/updatedInput/answers")
                .and_then(Value::as_object)
                .expect("answers");
            let want = answers
                .values()
                .next()
                .and_then(Value::as_str)
                .unwrap()
                .to_lowercase();
            let idx = q.questions[0]
                .options
                .iter()
                .position(|o| o.label.to_lowercase() == want)
                .unwrap();
            for _ in 0..idx {
                app.on_key(key(KeyCode::Down));
            }
            app.on_key(key(KeyCode::Enter));
        }
        _ => unreachable!(),
    }
    assert_eq!(
        app.focus,
        Focus::Composer,
        "focus returns to the composer after answering"
    );
    assert!(
        !app.transcript.get(card).unwrap().pending_card(),
        "card is settled"
    );
}

/// Replays a fixture, returning the app. `until` stops before the first sent
/// record for which it returns true.
fn replay(kind: HarnessKind, name: &str, until: impl Fn(&Value) -> bool) -> App {
    let mut app = app(kind);
    app.start();
    let mut pending: Vec<Value> = outbox(&mut app);
    // The spike's own control request ids -> the ids this app generated.
    let mut id_map: Vec<(String, String)> = Vec::new();
    for (i, line) in load(name).into_iter().enumerate() {
        match line.clone() {
            Line::In(mut raw) => {
                for (from, to) in &id_map {
                    raw = raw.replace(
                        &format!("\"request_id\":\"{from}\""),
                        &format!("\"request_id\":\"{to}\""),
                    );
                }
                app.on_record(record(i as u64 + 1, &raw));
            }
            Line::Out(sent) => {
                if until(&sent) {
                    break;
                }
                let is_answer = match kind {
                    HarnessKind::Claude => s(&sent, "/type") == "control_response",
                    HarnessKind::Codex => sent.get("method").is_none(),
                };
                if is_answer {
                    answer(&mut app, kind, &sent);
                } else if kind == HarnessKind::Claude && s(&sent, "/type") == "user" {
                    let text = s(&sent, "/message/content");
                    type_text(&mut app, text);
                    if app.slash.open {
                        // First Enter completes the menu item, the next one runs it.
                        app.on_key(key(KeyCode::Enter));
                        assert!(!app.slash.open);
                        assert_eq!(app.composer.text(), format!("{text} "));
                        assert!(
                            app.take_outbox().is_empty(),
                            "completing a command sends nothing"
                        );
                    }
                    app.on_key(key(KeyCode::Enter));
                } else if kind == HarnessKind::Codex && s(&sent, "/method") == "turn/start" {
                    type_text(&mut app, s(&sent, "/params/input/0/text"));
                    app.on_key(key(KeyCode::Enter));
                } else if s(&sent, "/request/subtype") == "interrupt"
                    || s(&sent, "/method") == "turn/interrupt"
                {
                    app.on_key(ctrl('c'));
                    assert_eq!(app.status(), Status::Stopping);
                }
            }
        }
        pending.extend(outbox(&mut app));
        if let Line::Out(sent) = line {
            let pos = pending.iter().position(|g| matches(kind, &sent, g));
            assert!(
                pos.is_some(),
                "{name}: expected the app to send {sent}\nbut it sent {pending:#?}"
            );
            let got = pending.remove(pos.unwrap());
            if s(&sent, "/type") == "control_request" {
                id_map.push((
                    s(&sent, "/request_id").to_string(),
                    s(&got, "/request_id").to_string(),
                ));
            }
        }
    }
    assert!(
        pending.is_empty(),
        "{name}: unexpected extra messages {pending:#?}"
    );
    app
}

/// A compact, human-readable dump of the transcript for snapshots.
fn summary(app: &App) -> String {
    let mut out = Vec::new();
    for e in app.transcript.iter() {
        let line = match &e.kind {
            EntryKind::User { text } => format!("you: {text}"),
            EntryKind::Assistant { text, done, .. } => format!(
                "assistant{}: {}",
                if *done { "" } else { " (streaming)" },
                text.chars().take(80).collect::<String>().replace('\n', "⏎")
            ),
            EntryKind::Tools(g) => {
                let mut s = tools_summary(g);
                for c in &g.calls {
                    let kind = match &c.kind {
                        ToolKind::Mcp { server, tool } => format!("MCP {server}/{tool}"),
                        _ => c.name.clone(),
                    };
                    s.push_str(&format!(
                        "\n    {:?} {kind} {}{}",
                        c.status,
                        c.title.chars().take(50).collect::<String>(),
                        if c.diff.is_some() { " [diff]" } else { "" }
                    ));
                }
                s
            }
            EntryKind::Approval(c) => {
                format!("approval [{}] {} ({:?})", c.subject, c.title, c.state)
            }
            EntryKind::Question(c) => format!("question {} ({:?})", c.questions[0].text, c.state),
            EntryKind::Notice { text, .. } => {
                format!("notice: {}", text.lines().next().unwrap_or(""))
            }
            EntryKind::TurnEnd { text, .. } => {
                format!("end: {}", text.split(" · ").next().unwrap_or(""))
            }
        };
        out.push(line);
    }
    out.push(format!("status: {:?}", app.status()));
    out.join("\n")
}

#[test]
fn claude_multi_turn_keeps_context() {
    let app = replay(
        HarnessKind::Claude,
        "claude/basic_followup_slash.transcript",
        |v| s(v, "/message/content") == "/cost",
    );
    let texts: Vec<_> = app
        .transcript
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Assistant { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts[0], "hello");
    assert_eq!(
        texts[1], "hello",
        "follow-up answered from the first turn's context"
    );
    assert_eq!(
        app.meta.model.as_ref().unwrap().value,
        "claude-haiku-4-5-20251001"
    );
    assert_eq!(
        app.meta.model.as_ref().unwrap().source,
        MetaSource::Reported
    );
    assert_eq!(
        app.meta.session_id.as_deref(),
        Some("eea0fbda-ad7f-41e7-9ee1-a1431b5c51a5")
    );
    insta::assert_snapshot!(summary(&app));
}

#[test]
fn claude_unlisted_slash_command_is_not_sent() {
    let mut app = replay(
        HarnessKind::Claude,
        "claude/basic_followup_slash.transcript",
        |v| s(v, "/message/content") == "/cost",
    );
    type_text(&mut app, "/cost");
    app.on_key(key(KeyCode::Esc)); // close the (empty) menu if any
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.take_outbox().is_empty(),
        "unknown command must not be sent as a prompt"
    );
    assert!(app
        .hint
        .as_ref()
        .unwrap()
        .text
        .contains("Unknown command /cost"));
    assert_eq!(app.composer.text(), "/cost", "draft kept");
}

#[test]
fn claude_listed_slash_command_runs() {
    let app = replay(
        HarnessKind::Claude,
        "claude/slash_command.transcript",
        |_| false,
    );
    assert!(app.commands.iter().any(|c| c.name == "context"));
    let first = app.transcript.iter().find_map(|e| match &e.kind {
        EntryKind::Assistant { text, .. } => Some(text.clone()),
        _ => None,
    });
    assert!(first.unwrap().starts_with("## Context Usage"));
    insta::assert_snapshot!(summary(&app));
}

#[test]
fn claude_approval_deny_then_allow_and_question() {
    let app = replay(
        HarnessKind::Claude,
        "claude/approval_question.transcript",
        |_| false,
    );
    insta::assert_snapshot!(summary(&app));
}

#[test]
fn claude_interrupt_then_follow_up() {
    let app = replay(HarnessKind::Claude, "claude/interrupt.transcript", |_| {
        false
    });
    let s = summary(&app);
    assert!(s.contains("end: ■ Turn stopped"), "{s}");
    assert_eq!(app.status(), Status::Ready);
    insta::assert_snapshot!(s);
}

#[test]
fn claude_mcp_success_and_error() {
    let app = replay(HarnessKind::Claude, "claude/mcp.transcript", |_| false);
    let group = app
        .transcript
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Tools(g) => Some(g.clone()),
            _ => None,
        })
        .unwrap();
    let mcp: Vec<_> = group
        .calls
        .iter()
        .filter(|c| matches!(c.kind, ToolKind::Mcp { .. }))
        .collect();
    assert_eq!(mcp.len(), 2);
    assert_eq!(
        mcp[0].kind,
        ToolKind::Mcp {
            server: "project-docs".into(),
            tool: "search_docs".into()
        }
    );
    assert_eq!(mcp[0].status, ToolStatus::Succeeded);
    assert_eq!(mcp[1].status, ToolStatus::Failed);
    assert_eq!(mcp[1].output, "document 'missing-42' not found");
    assert!(
        mcp[1].raw.len() >= 2,
        "call keeps its raw records (tool_use and tool_result)"
    );
    assert_eq!(group.counts().failed, 1);
    insta::assert_snapshot!(summary(&app));
}

#[test]
fn claude_edit_has_diff() {
    let app = replay(HarnessKind::Claude, "claude/edit.transcript", |_| false);
    let diff = app
        .transcript
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Tools(g) => g.calls.iter().find_map(|c| c.diff.clone()),
            _ => None,
        })
        .unwrap();
    assert!(diff.contains("-beta\n+gamma"), "{diff}");
}

#[test]
fn codex_follow_up_approvals_and_interrupt() {
    let app = replay(
        HarnessKind::Codex,
        "codex/basic_approval_interrupt.transcript",
        |_| false,
    );
    assert_eq!(app.meta.model.as_ref().unwrap().value, "gpt-6-astra");
    assert_eq!(app.meta.effort.as_ref().unwrap().value, "medium");
    assert_eq!(
        app.meta.effort.as_ref().unwrap().source,
        MetaSource::Reported
    );
    let s = summary(&app);
    assert!(s.contains("end: ■ Turn stopped"), "{s}");
    insta::assert_snapshot!(s);
}

#[test]
fn codex_mcp_elicitation_and_failure() {
    let app = replay(HarnessKind::Codex, "codex/mcp.transcript", |_| false);
    insta::assert_snapshot!(summary(&app));
}

#[test]
fn codex_file_change_approval_with_diff() {
    let app = replay(HarnessKind::Codex, "codex/edit.transcript", |_| false);
    let s = summary(&app);
    assert!(s.contains("[diff]"), "{s}");
    assert!(app
        .transcript
        .iter()
        .any(|e| matches!(&e.kind, EntryKind::Approval(c) if c.state == CardState::Answered("Allowed once".into()))));
    insta::assert_snapshot!(s);
}
