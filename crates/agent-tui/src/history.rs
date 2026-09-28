//! Bounded, display-only history. Tool payloads and old approval cards are
//! deliberately excluded; loading history never produces harness messages.
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub user: bool,
    pub text: String,
}
const MAX_MESSAGES: usize = 40;
const MAX_HISTORY_BYTES: usize = 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 32 * 1024;

fn bounded(text: &str) -> String {
    let mut end = text.len().min(MAX_MESSAGE_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
fn text_blocks(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return bounded(text);
    }
    value
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .map(bounded)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .map(|text| bounded(&text))
        .unwrap_or_default()
}

pub fn codex(thread: &Value) -> Vec<Message> {
    let mut result = Vec::new();
    for turn in thread["turns"].as_array().into_iter().flatten().rev() {
        for item in turn["items"].as_array().into_iter().flatten().rev() {
            let (user, text) = match item["type"].as_str() {
                Some("userMessage") => (true, text_blocks(&item["content"])),
                Some("agentMessage") => (
                    false,
                    item["text"].as_str().map(bounded).unwrap_or_default(),
                ),
                _ => continue,
            };
            if !text.is_empty() {
                result.push(Message { user, text });
            }
            if result.len() == MAX_MESSAGES {
                result.reverse();
                return result;
            }
        }
    }
    result.reverse();
    result
}

pub fn claude(home: &Path, cwd: &str, session_id: &str) -> std::io::Result<Vec<Message>> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid Claude session id",
        ));
    }
    let project: String = cwd
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = home
        .join("projects")
        .join(project)
        .join(format!("{session_id}.jsonl"));
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(MAX_HISTORY_BYTES as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(MAX_HISTORY_BYTES as u64)
        .read_to_end(&mut bytes)?;
    let mut result = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n').rev() {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        let user = match value["type"].as_str() {
            Some("user") => true,
            Some("assistant") => false,
            _ => continue,
        };
        let text = text_blocks(&value["message"]["content"]);
        if !text.is_empty() {
            result.push(Message { user, text });
        }
        if result.len() == MAX_MESSAGES {
            break;
        }
    }
    result.reverse();
    Ok(result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn codex_history_only_contains_display_text() {
        let messages = super::codex(&serde_json::json!({"turns":[{"items":[
            {"type":"userMessage","content":[{"type":"text","text":"old user"}]},
            {"type":"commandExecution","command":"must not run"},
            {"type":"agentMessage","text":"old assistant"}
        ]}]}));
        assert_eq!(messages.len(), 2);
        assert!(messages[0].user);
        assert!(!messages[1].user);
        assert_eq!(messages[1].text, "old assistant");
    }
}
