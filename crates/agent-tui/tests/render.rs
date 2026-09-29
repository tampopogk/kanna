//! Render snapshots (cell symbols) at 100×30 and 60×20, plus targeted color
//! checks for the layout rules: user shading, no name labels, one footer row
//! with the skin, jump pill, sprite placement.

mod common;

use std::time::Duration;

use agent_tui::app::{App, Focus};
use agent_tui::protocol::HarnessKind;
use agent_tui::ui::skins::{ColorMode, SkinId, Theme};
use agent_tui::ui::{draw, RenderCache};
use common::*;
use crossterm::event::KeyCode;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;
use serde_json::{json, Value};

fn feed(app: &mut App, v: Value) {
    app.on_record(record(0, &v.to_string()));
}

fn render(app: &mut App, w: u16, h: u16) -> Buffer {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut cache = RenderCache::default();
    term.draw(|f| draw(f, app, &mut cache, ColorMode::TrueColor))
        .unwrap();
    // Draw twice so viewport adjustments made during the first frame settle.
    term.draw(|f| draw(f, app, &mut cache, ColorMode::TrueColor))
        .unwrap();
    term.backend().buffer().clone()
}

fn dump(buf: &Buffer) -> String {
    let a = buf.area;
    (0..a.height)
        .map(|y| {
            (0..a.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn row_of(buf: &Buffer, needle: &str) -> Option<u16> {
    let d = dump(buf);
    d.lines().position(|l| l.contains(needle)).map(|y| y as u16)
}

fn claude_ready(skin: SkinId) -> App {
    let mut app = app_with(HarnessKind::Claude, None, Some("high"));
    app.set_skin(skin);
    app.quote = 0;
    app.cwd_label = "example".into();
    app.start();
    let id = outbox(&mut app)[0]["request_id"]
        .as_str()
        .unwrap()
        .to_string();
    feed(
        &mut app,
        json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"commands": [
            {"name": "compact", "description": "Clear conversation history but keep a summary", "argumentHint": "<instructions>"},
            {"name": "context", "description": "Show current context usage", "argumentHint": ""}
        ]}}}),
    );
    feed(
        &mut app,
        json!({"type": "system", "subtype": "init", "model": "claude-sonnet-4-6", "session_id": "s1"}),
    );
    app
}

/// A conversation mid-turn: tools running, a draft being written.
fn working(skin: SkinId) -> App {
    let mut app = claude_ready(skin);
    type_text(&mut app, "Fix the parser's handling of partial JSON lines.");
    app.on_key(key(KeyCode::Enter));
    outbox(&mut app);
    app.turn_started = Some(app.now - Duration::from_secs(18));
    feed(
        &mut app,
        json!({"type": "stream_event", "event": {"type": "message_start", "message": {"id": "m1"}}}),
    );
    feed(
        &mut app,
        json!({"type": "stream_event", "event": {"type": "content_block_start", "index": 0, "content_block": {"type": "text"}}}),
    );
    feed(
        &mut app,
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "I'll check the parser and add a regression test."}}}),
    );
    feed(
        &mut app,
        json!({"type": "assistant", "message": {"id": "m1", "model": "claude-sonnet-4-6", "content": [
            {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "src/parser.ts"}},
            {"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": "src/parser.ts", "old_string": "a", "new_string": "b"}},
            {"type": "tool_use", "id": "t3", "name": "mcp__project-docs__search_docs", "input": {"query": "stream parser", "limit": 2}},
            {"type": "tool_use", "id": "t4", "name": "mcp__project-docs__read_document", "input": {"path": "docs/parser-v2.md"}},
            {"type": "tool_use", "id": "t5", "name": "Bash", "input": {"command": "npm test -- parser"}}
        ]}}),
    );
    feed(
        &mut app,
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "1\tconst x = 1;\n2\texport {}"}]}}),
    );
    feed(
        &mut app,
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t2", "content": "updated"}]},
        "tool_use_result": {"filePath": "src/parser.ts", "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 3, "lines": ["-const message = JSON.parse(chunk);", "+pending += chunk;", "+const lines = pending.split('\\n');", "+pending = lines.pop();"]}]}}),
    );
    feed(
        &mut app,
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t3", "content": [{"type": "text", "text": "docs/streaming.md: Buffer partial records until a newline arrives."}]}]}}),
    );
    feed(
        &mut app,
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t4", "is_error": true, "content": "Document not found: docs/parser-v2.md"}]}}),
    );
    type_text(&mut app, "Also check CRLF input.");
    app
}

#[test]
fn welcome_graphite() {
    let mut app = claude_ready(SkinId::Graphite);
    let buf = render(&mut app, 100, 30);
    insta::assert_snapshot!(dump(&buf));
    let d = dump(&buf);
    assert!(d.contains("Ready when you are") && d.contains("What are we building?"));
    assert!(d.contains("Effort high (requested)"));
}

#[test]
fn welcome_matrix() {
    let mut app = claude_ready(SkinId::Matrix);
    let d = dump(&render(&mut app, 100, 30));
    assert!(
        d.contains("I know kung fu.") && d.contains("Never send a human to do a machine's job.")
    );
    insta::assert_snapshot!(d);
}

#[test]
fn welcome_duke_with_sprite_and_compact_header() {
    let mut app = claude_ready(SkinId::Duke);
    let buf = render(&mut app, 100, 30);
    let d = dump(&buf);
    assert!(
        d.contains("Hail to the king, baby.")
            && d.contains("It's time to kick ass and chew bubble gum…")
    );
    assert!(!d.contains("›_"), "the sprite replaces the ›_ logo");
    // Header sprite: hair yellow top half in the first header row.
    let hair = Color::Rgb(0xFF, 0xDC, 0x1E);
    assert!((0..10).any(|x| buf[(x, 0)].fg == hair));
    // Welcome sprite: 6 rows of half blocks with the blue pants somewhere.
    let pants = Color::Rgb(0x00, 0x3F, 0x8C);
    let sprite_cells = (4..30)
        .flat_map(|y| (0..30).map(move |x| (x, y)))
        .filter(|&(x, y)| buf[(x, y)].fg == pants || buf[(x, y)].bg == pants)
        .count();
    assert!(sprite_cells >= 12, "sprite drawn in the welcome area");
    insta::assert_snapshot!(d);
}

#[test]
fn duke_narrow_falls_back_to_text_badge() {
    let mut app = claude_ready(SkinId::Duke);
    let d = dump(&render(&mut app, 58, 20));
    assert!(
        d.lines().next().unwrap().contains("›_"),
        "narrow header uses the text badge"
    );
    insta::assert_snapshot!(d);
}

#[test]
fn working_tools_collapsed() {
    let mut app = working(SkinId::Graphite);
    let buf = render(&mut app, 100, 30);
    let d = dump(&buf);
    assert!(d.contains("● Working · 00:18"));
    assert!(d.contains("› Tools · 3 completed · 1 failed · 1 running"));
    assert!(d.contains("Draft saved · waiting for current turn"));
    assert!(
        !d.contains("You") && !d.contains("Claude:"),
        "no speaker labels"
    );
    // User message is shaded with an accent bar; assistant text is not.
    let theme = Theme::new(SkinId::Graphite, ColorMode::TrueColor);
    let uy = row_of(&buf, "Fix the parser").unwrap();
    assert_eq!(buf[(1, uy)].symbol(), "▌");
    assert_eq!(buf[(1, uy)].fg, theme.accent);
    assert_eq!(buf[(10, uy)].bg, theme.user_bg);
    let ay = row_of(&buf, "I'll check the parser").unwrap();
    assert_eq!(buf[(10, ay)].bg, theme.bg);
    // One footer row holding both hints and the skin selector.
    let last = d.lines().last().unwrap();
    assert!(last.contains("Enter send") && last.contains("Skin Graphite"));
    insta::assert_snapshot!(d);
}

#[test]
fn working_tools_expanded_with_mcp_raw_json() {
    let mut app = working(SkinId::Graphite);
    app.on_key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Transcript);
    for _ in 0..4 {
        app.on_key(key(KeyCode::Up));
    }
    // Walk to the tools group (last entry before the draft).
    let tools = app.targets().into_iter().find(|t| matches!(t, agent_tui::app::Target::Entry(id) if matches!(app.transcript.get(*id).unwrap().kind, agent_tui::app::transcript::EntryKind::Tools(_)))).unwrap();
    app.view.selected = Some(tools);
    app.on_key(key(KeyCode::Enter)); // expand group
    for _ in 0..3 {
        app.on_key(key(KeyCode::Down));
    }
    app.on_key(key(KeyCode::Enter)); // expand the MCP search call
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter)); // raw input
    let d = dump(&render(&mut app, 100, 40));
    assert!(d.contains("⌄ Tools"));
    assert!(d.contains("⌄ ✓ MCP · project-docs / search_docs · succeeded"));
    assert!(
        d.contains("› ✕ MCP · project-docs / read_document · failed"),
        "sibling keeps its own arrow"
    );
    assert!(d.contains("⌄ Raw input · JSON") && d.contains("› Raw output · JSON"));
    assert!(d.contains("\"query\": \"stream parser\""));
    assert!(d.contains("+3 −1"), "edit diff counts");
    insta::assert_snapshot!(d);
}

#[test]
fn approval_card_pending_then_focused() {
    let mut app = working(SkinId::Graphite);
    feed(
        &mut app,
        json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "npm install"}, "description": "Install dependencies for the test run", "tool_use_id": "t9"}}),
    );
    let d = dump(&render(&mut app, 100, 30));
    assert!(d.contains("! Needs approval"));
    assert!(
        d.contains("Permission needed") && d.contains("[ Allow once ]") && d.contains("[ Deny ]")
    );
    assert!(d.contains("Tab to review"));
    insta::assert_snapshot!("approval_pending", d);
    app.on_key(key(KeyCode::Tab));
    let d = dump(&render(&mut app, 100, 30));
    assert!(d.contains("←→ choose · Enter confirm · Esc leave pending"));
    insta::assert_snapshot!("approval_focused", d);
}

#[test]
fn stopped_turn() {
    let mut app = working(SkinId::Nord);
    app.on_key(ctrl('c'));
    let d = dump(&render(&mut app, 100, 30));
    assert!(d.contains("■ Stopping"));
    feed(
        &mut app,
        json!({"type": "result", "subtype": "error_during_execution", "is_error": true, "terminal_reason": "aborted_streaming"}),
    );
    let d = dump(&render(&mut app, 100, 30));
    assert!(d.contains("■ Turn stopped"));
    assert!(d.contains("✓ Ready"));
    assert!(d.contains("1 stopped"), "the running tool is shown stopped");
    insta::assert_snapshot!(d);
}

#[test]
fn disconnected_keeps_draft() {
    let mut app = working(SkinId::Dracula);
    app.on_disconnect(
        "Claude exited (exit code 1)",
        &["Error: not logged in".into()],
    );
    let d = dump(&render(&mut app, 100, 30));
    assert!(d.contains("✕ Disconnected"));
    assert!(d.contains("Also check CRLF input."));
    insta::assert_snapshot!(d);
}

#[test]
fn slash_menu() {
    let mut app = claude_ready(SkinId::Graphite);
    type_text(&mut app, "/");
    let d = dump(&render(&mut app, 100, 30));
    assert!(
        d.contains("/help")
            && d.contains("local")
            && d.contains("/compact")
            && d.contains("claude")
    );
    insta::assert_snapshot!(d);
}

#[test]
fn solarized_light_is_light() {
    let mut app = working(SkinId::SolarizedLight);
    let buf = render(&mut app, 100, 30);
    let theme = Theme::new(SkinId::SolarizedLight, ColorMode::TrueColor);
    assert_eq!(buf[(50, 10)].bg, theme.bg);
    assert_eq!(theme.bg, Color::Rgb(0xfd, 0xf6, 0xe3));
    insta::assert_snapshot!(dump(&buf));
}

#[test]
fn narrow_working_keeps_composer_and_drops_hints() {
    let mut app = working(SkinId::Graphite);
    let d = dump(&render(&mut app, 60, 20));
    assert!(d.contains("Also check CRLF input."));
    let last = d.lines().last().unwrap();
    assert!(last.contains("Skin Graphite"));
    assert!(
        !last.contains("F1 help"),
        "hints dropped before the skin selector"
    );
    insta::assert_snapshot!(d);
}

#[test]
fn jump_to_latest_pill() {
    let mut app = working(SkinId::Graphite);
    feed(
        &mut app,
        json!({"type": "assistant", "message": {"id": "m1b", "content": [{"type": "text", "text": "Reading the tests:\n1\n2\n3\n4\n5\n6\n7\n8"}]}}),
    );
    render(&mut app, 100, 12);
    app.on_key(key(KeyCode::PageUp));
    feed(
        &mut app,
        json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t5", "content": "PASS"}]}}),
    );
    feed(
        &mut app,
        json!({"type": "assistant", "message": {"id": "m2", "content": [{"type": "text", "text": "All checks passed."}]}}),
    );
    let d = dump(&render(&mut app, 100, 12));
    assert!(d.contains("↓ Jump to latest · 1 new"), "{d}");
    app.on_key(ctrl('l'));
    let d = dump(&render(&mut app, 100, 12));
    assert!(!d.contains("Jump to latest"));
    assert!(d.contains("All checks passed."));
}

#[test]
fn provider_escape_sequences_are_not_emitted() {
    let mut app = claude_ready(SkinId::Graphite);
    type_text(&mut app, "x");
    app.on_key(key(KeyCode::Enter));
    feed(
        &mut app,
        json!({"type": "assistant", "message": {"id": "m", "content": [{"type": "text", "text": "red \u{1b}[31mtext\u{1b}]0;pwned\u{7}"}]}}),
    );
    let buf = render(&mut app, 80, 20);
    let d = dump(&buf);
    assert!(d.contains("␛[31mtext␛]0;pwned␇"), "{d}");
    assert!(!d.contains('\u{1b}'));
}

#[test]
fn short_terminals_never_overflow() {
    for skin in [SkinId::Graphite, SkinId::Duke] {
        for search in [false, true] {
            for w in [20u16, 60, 80] {
                for h in 8u16..=14 {
                    let mut app = working(skin);
                    for _ in 0..5 {
                        app.on_key(ctrl('j'));
                        type_text(&mut app, "more");
                    }
                    assert_eq!(app.composer.line_count(), 6);
                    if search {
                        app.on_key(ctrl('f'));
                        type_text(&mut app, "parser");
                    }
                    let buf = render(&mut app, w, h);
                    // The footer is always the last row and the composer text is visible.
                    let d = dump(&buf);
                    assert_eq!(
                        d.split('\n').count(),
                        h as usize,
                        "{skin:?} {w}x{h} search={search}"
                    );
                    if w >= 60 {
                        assert!(
                            d.lines().last().unwrap().contains("Skin"),
                            "{skin:?} {w}x{h} search={search}\n{d}"
                        );
                        assert!(d.contains("more"), "draft visible at {w}x{h}\n{d}");
                    }
                }
            }
        }
    }
}
