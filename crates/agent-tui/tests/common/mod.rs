#![allow(dead_code)]

use agent_tui::app::App;
use agent_tui::harness::{make_adapter, HarnessOptions};
use agent_tui::protocol::HarnessKind;
use agent_tui::transport::Record;
use agent_tui::ui::skins::SkinId;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

#[derive(Debug, Clone)]
pub enum Line {
    /// Received from the harness (raw text kept for framing).
    In(String),
    /// Sent to the harness by the spike driver.
    Out(Value),
}

/// Loads a recorded spike transcript: `< ` lines were received, `> ` sent.
pub fn load(name: &str) -> Vec<Line> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    text.lines()
        .filter_map(|l| {
            if let Some(r) = l.strip_prefix("< ") {
                Some(Line::In(r.to_string()))
            } else {
                l.strip_prefix("> ")
                    .map(|s| Line::Out(serde_json::from_str(s).expect("sent line is JSON")))
            }
        })
        .collect()
}

pub fn app(kind: HarnessKind) -> App {
    app_with(kind, None, None)
}

pub fn app_with(kind: HarnessKind, model: Option<&str>, effort: Option<&str>) -> App {
    let adapter = make_adapter(&HarnessOptions {
        kind,
        model: model.map(str::to_string),
        effort: effort.map(str::to_string),
        cwd: Some("/work/spike".into()),
        program: None,
        extra_args: vec![],
    });
    let mut a = App::new(adapter, SkinId::Graphite);
    a.quote = 0;
    a
}

pub fn record(line_no: u64, raw: &str) -> Record {
    Record {
        line_no,
        raw: raw.to_string(),
        parsed: serde_json::from_str(raw).map_err(|e| e.to_string()),
    }
}

pub fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

pub fn shift(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

pub fn type_text(app: &mut App, s: &str) {
    for c in s.chars() {
        if c == '\n' {
            app.on_key(ctrl('j'));
        } else {
            app.on_key(key(KeyCode::Char(c)));
        }
    }
}

pub fn outbox(app: &mut App) -> Vec<Value> {
    app.take_outbox()
        .into_iter()
        .map(|s| serde_json::from_str(&s).unwrap())
        .collect()
}
