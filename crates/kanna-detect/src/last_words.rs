use serde_json::Value;

pub const MOST: usize = 8 * 1024;

pub const READ: u64 = 2 * 1024 * 1024;

const CLAUDE_QUESTION: &str = "AskUserQuestion";

const CODEX_QUESTION: &str = "request_user_input";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Words {
    pub text: String,
    pub asking: bool,
}

impl Words {
    fn said(text: &str) -> Option<Words> {
        let text = text.trim();
        (!text.is_empty()).then(|| Words { text: tail(text).to_string(), asking: false })
    }

    fn asked(text: String) -> Option<Words> {
        Words::said(&text).map(|words| Words { asking: true, ..words })
    }
}

fn tail(text: &str) -> &str {
    let from = (text.len().saturating_sub(MOST)..=text.len()).find(|at| text.is_char_boundary(*at)).unwrap_or(0);
    &text[from..]
}

fn question(input: &Value) -> String {
    let mut lines = vec![];
    for asked in input["questions"].as_array().into_iter().flatten() {
        lines.extend(asked["question"].as_str().map(String::from));
        for option in asked["options"].as_array().into_iter().flatten() {
            let Some(label) = option["label"].as_str() else { continue };
            lines.push(match option["description"].as_str().filter(|d| !d.is_empty()) {
                Some(about) => format!("- {label}: {about}"),
                None => format!("- {label}"),
            });
        }
    }
    lines.join("\n")
}

fn rows(lines: &str) -> impl Iterator<Item = Value> + '_ {
    lines.lines().rev().filter_map(|line| serde_json::from_str(line).ok())
}

pub fn claude(lines: &str) -> Option<Words> {
    let mut answered: Vec<String> = vec![];
    for row in rows(lines).filter(|row| row["isSidechain"] != true) {
        let content = row["message"]["content"].as_array().cloned().unwrap_or_default();
        match row["type"].as_str() {
            Some("user") => answered.extend(content.iter().filter_map(|block| block["tool_use_id"].as_str().map(String::from))),
            Some("assistant") => {
                for block in content.iter().rev() {
                    match block["type"].as_str() {
                        Some("tool_use") if block["name"] == CLAUDE_QUESTION && !block["id"].as_str().is_some_and(|id| answered.iter().any(|a| a == id)) => {
                            if let Some(words) = Words::asked(question(&block["input"])) {
                                return Some(words);
                            }
                        }
                        Some("text") => {
                            if let Some(words) = block["text"].as_str().and_then(Words::said) {
                                return Some(words);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub fn codex(lines: &str) -> Option<Words> {
    let mut answered: Vec<String> = vec![];
    for row in rows(lines) {
        let payload = &row["payload"];
        match (row["type"].as_str(), payload["type"].as_str()) {
            (Some("response_item"), Some("function_call_output")) => answered.extend(payload["call_id"].as_str().map(String::from)),
            (Some("response_item"), Some("function_call")) if payload["name"] == CODEX_QUESTION && !payload["call_id"].as_str().is_some_and(|id| answered.iter().any(|a| a == id)) => {
                let input = payload["arguments"].as_str().and_then(|text| serde_json::from_str::<Value>(text).ok()).unwrap_or(Value::Null);
                if let Some(words) = Words::asked(question(&input)) {
                    return Some(words);
                }
            }
            (Some("event_msg"), Some("agent_message")) => {
                if let Some(words) = payload["message"].as_str().and_then(Words::said) {
                    return Some(words);
                }
            }
            (Some("response_item"), Some("message")) if payload["role"] == "assistant" => {
                let text: Vec<&str> = payload["content"].as_array().into_iter().flatten().filter(|block| block["type"] == "output_text").filter_map(|block| block["text"].as_str()).collect();
                if let Some(words) = Words::said(&text.join("\n")) {
                    return Some(words);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jsonl(rows: &[Value]) -> String {
        rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn claude_s_last_words_are_its_last_text_past_the_tools_and_subagents_after_it() {
        let lines = jsonl(&[
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Earlier" }] } }),
            json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "  Done with the fix.\n" }, { "type": "tool_use", "name": "Bash", "id": "t1", "input": {} }] } }),
            json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "t1" }] } }),
            json!({ "type": "assistant", "isSidechain": true, "message": { "content": [{ "type": "text", "text": "A subagent" }] } }),
            json!({ "type": "summary" }),
        ]);
        assert_eq!(claude(&lines), Some(Words { text: "Done with the fix.".into(), asking: false }));
        assert_eq!(claude("not json\n{\"type\":\"user\"}"), None);
    }

    #[test]
    fn a_pending_question_is_the_last_words_until_it_is_answered() {
        let asked = json!({ "type": "assistant", "message": { "content": [{ "type": "text", "text": "Let me ask." }, { "type": "tool_use", "name": "AskUserQuestion", "id": "q1", "input": { "questions": [{ "question": "Which one?", "options": [{ "label": "Keep", "description": "Leave it" }, { "label": "Drop" }] }] } }] } });
        assert_eq!(claude(&jsonl(&[asked.clone()])), Some(Words { text: "Which one?\n- Keep: Leave it\n- Drop".into(), asking: true }));
        let answered = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "q1" }] } });
        assert_eq!(claude(&jsonl(&[asked, answered])), Some(Words { text: "Let me ask.".into(), asking: false }));
    }

    #[test]
    fn codex_s_last_words_come_from_its_rollout() {
        let lines = jsonl(&[
            json!({ "type": "response_item", "payload": { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": "Do it" }] } }),
            json!({ "type": "response_item", "payload": { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "All done." }] } }),
            json!({ "type": "event_msg", "payload": { "type": "task_complete" } }),
        ]);
        assert_eq!(codex(&lines), Some(Words { text: "All done.".into(), asking: false }));
        let event = jsonl(&[json!({ "type": "event_msg", "payload": { "type": "agent_message", "message": "Said." } })]);
        assert_eq!(codex(&event), Some(Words { text: "Said.".into(), asking: false }));
        let asked = jsonl(&[json!({ "type": "response_item", "payload": { "type": "function_call", "name": "request_user_input", "call_id": "c", "arguments": "{\"questions\":[{\"question\":\"Which?\",\"options\":[{\"label\":\"A\"}]}]}" } })]);
        assert_eq!(codex(&asked), Some(Words { text: "Which?\n- A".into(), asking: true }));
    }

    #[test]
    fn last_words_keep_the_end_of_a_long_text_on_a_character() {
        let long = format!("{}é the end", "é".repeat(MOST));
        let words = Words::said(&long).unwrap();
        assert!(words.text.len() <= MOST && words.text.ends_with("the end"));
    }
}
