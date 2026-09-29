//! Claude Code over `claude -p --input-format stream-json --output-format
//! stream-json` with the SDK control protocol on stdio.
//!
//! Verified against claude 2.1.283 (see `tests/fixtures/claude/*.transcript`):
//! - `control_request {subtype: initialize}` returns `commands`, `models`, …
//! - with `--permission-prompt-tool stdio`, permission prompts arrive as
//!   `control_request {subtype: can_use_tool}` and are answered with
//!   `control_response {behavior: allow|deny}`; AskUserQuestion arrives the same
//!   way (`requires_user_interaction: true`) and is answered by allowing with
//!   `updatedInput.answers`.
//! - `control_request {subtype: interrupt}` stops the turn; the turn's `result`
//!   then has `is_error: true`, `terminal_reason: "aborted_streaming"`.
//! - Slash commands from the initialize `commands` list run when sent as the
//!   user message text.
//! - Effort is not reported anywhere in the stream.

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::{json, Map, Value};

use crate::protocol::*;

#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    pub program: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub extra_args: Vec<String>,
    pub expected_session_id: Option<String>,
    pub replay_user_messages: bool,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            program: "claude".into(),
            model: None,
            effort: None,
            extra_args: Vec::new(),
            expected_session_id: None,
            replay_user_messages: false,
        }
    }
}

#[derive(Debug)]
enum PendingControl {
    Initialize,
    Interrupt,
}

#[derive(Debug)]
struct PendingPermission {
    input: Value,
    is_question: bool,
    questions: Vec<Question>,
}

pub struct ClaudeAdapter {
    cfg: ClaudeConfig,
    next_req: u64,
    pending_control: HashMap<String, PendingControl>,
    pending_perm: HashMap<String, PendingPermission>,
    /// message id -> text item ids streamed but not yet finalized, in order.
    streaming_text: HashMap<String, VecDeque<String>>,
    /// (message id, content index) -> item id, for deltas.
    block_items: HashMap<(String, u64), String>,
    current_msg: Option<String>,
    synthetic_items: u64,
    started_tools: HashSet<String>,
    last_model: Option<String>,
    ready: bool,
    turn_active: bool,
    interrupt_requested: bool,
    pending_input: Option<(String, String)>,
}

const CONTROL_MARKERS: &[&str] = &[
    "control_request",
    "control_response",
    "control_cancel",
    "\"result\"",
    "can_use_tool",
];

impl ClaudeAdapter {
    pub fn new(cfg: ClaudeConfig) -> Self {
        Self {
            cfg,
            next_req: 0,
            pending_control: HashMap::new(),
            pending_perm: HashMap::new(),
            streaming_text: HashMap::new(),
            block_items: HashMap::new(),
            current_msg: None,
            synthetic_items: 0,
            started_tools: HashSet::new(),
            last_model: None,
            ready: false,
            turn_active: false,
            interrupt_requested: false,
            pending_input: None,
        }
    }

    fn req_id(&mut self, prefix: &str) -> String {
        self.next_req += 1;
        format!("agent-tui-{prefix}-{}", self.next_req)
    }

    fn on_control_response(&mut self, v: &Value) -> Output {
        let resp = v.get("response").cloned().unwrap_or(Value::Null);
        let Some(id) = resp.get("request_id").and_then(Value::as_str) else {
            return Output::event(AgentEvent::Degraded {
                reason: "control_response without request_id".into(),
            });
        };
        let ok = resp.get("subtype").and_then(Value::as_str) == Some("success");
        let err_text = resp
            .get("error")
            .map(content_text)
            .unwrap_or_else(|| "unknown error".into());
        match self.pending_control.remove(id) {
            Some(PendingControl::Initialize) => {
                if !ok {
                    return Output::event(AgentEvent::StartupFailed {
                        message: format!("Claude rejected initialize: {err_text}"),
                    });
                }
                self.ready = true;
                let body = resp.get("response").cloned().unwrap_or(Value::Null);
                let commands = body
                    .get("commands")
                    .and_then(Value::as_array)
                    .map(|cmds| {
                        cmds.iter()
                            .filter_map(|c| {
                                let name = c.get("name")?.as_str()?.to_string();
                                Some(HarnessCommand {
                                    name,
                                    description: c
                                        .get("description")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    argument_hint: c
                                        .get("argumentHint")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut events = vec![AgentEvent::Ready { commands }];
                let effort = self.cfg.effort.clone().map(MetaValue::requested);
                let model = self.cfg.model.as_ref().map(|m| {
                    // Resolve an alias through the harness's own model table.
                    let resolved = body.get("models").and_then(Value::as_array).and_then(|ms| {
                        ms.iter()
                            .find(|x| x.get("value").and_then(Value::as_str) == Some(m.as_str()))
                            .and_then(|x| x.get("resolvedModel").and_then(Value::as_str))
                    });
                    MetaValue::requested(resolved.unwrap_or(m))
                });
                if model.is_some() || effort.is_some() {
                    events.push(AgentEvent::SessionMeta {
                        model,
                        effort,
                        session_id: None,
                    });
                }
                Output {
                    events,
                    outgoing: vec![],
                }
            }
            Some(PendingControl::Interrupt) => {
                if ok {
                    Output::unknown()
                } else {
                    Output::event(AgentEvent::Diagnostic {
                        text: format!("Interrupt was rejected: {err_text}"),
                    })
                }
            }
            None => Output::event(AgentEvent::Diagnostic {
                text: format!("control_response for unknown request {id}"),
            }),
        }
    }

    fn on_control_request(&mut self, v: &Value) -> Output {
        let Some(id) = v
            .get("request_id")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Output::event(AgentEvent::Degraded {
                reason: "control_request without request_id".into(),
            });
        };
        let req = v.get("request").cloned().unwrap_or(Value::Null);
        let subtype = req.get("subtype").and_then(Value::as_str).unwrap_or("");
        if subtype != "can_use_tool" {
            let text = format!(
                "Claude sent an unsupported control request ({}); answered with an error.",
                if subtype.is_empty() {
                    "no subtype"
                } else {
                    subtype
                }
            );
            return Output {
                events: vec![AgentEvent::Diagnostic { text }],
                outgoing: vec![json!({
                    "type": "control_response",
                    "response": {"subtype": "error", "request_id": id, "error": format!("agent-tui does not support {subtype}")}
                })],
            };
        }
        let tool = req.get("tool_name").and_then(Value::as_str).unwrap_or("");
        let input = req
            .get("input")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));
        let call_id = req
            .get("tool_use_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        if tool.is_empty() {
            return Output {
                events: vec![AgentEvent::Degraded {
                    reason: "can_use_tool without tool_name".into(),
                }],
                outgoing: vec![deny_response(&id, "Malformed permission request")],
            };
        }
        if tool == "AskUserQuestion" {
            if let Some(questions) = parse_ask_questions(&input) {
                self.pending_perm.insert(
                    id.clone(),
                    PendingPermission {
                        input,
                        is_question: true,
                        questions: questions.clone(),
                    },
                );
                return Output::event(AgentEvent::QuestionRequest {
                    request_id: id,
                    title: "Claude has a question".into(),
                    questions,
                    call_id,
                });
            }
        }
        let display = req
            .get("display_name")
            .and_then(Value::as_str)
            .unwrap_or(tool);
        let (kind, _, title) = classify_tool(tool, &input);
        let mut details = Vec::new();
        if let Some(d) = req.get("description").and_then(Value::as_str) {
            if d != title {
                details.push(d.to_string());
            }
        }
        if let Some(p) = req.get("blocked_path").and_then(Value::as_str) {
            details.push(format!("Path: {p}"));
        }
        if let Some(r) = req.get("decision_reason").and_then(Value::as_str) {
            details.push(r.to_string());
        }
        // Show what an edit would change, not just where.
        let preview = |label: char, text: &str, out: &mut Vec<String>| {
            let lines: Vec<&str> = text.lines().collect();
            out.extend(lines.iter().take(8).map(|l| format!("{label} {l}")));
            if lines.len() > 8 {
                out.push(format!("  … {} more lines", lines.len() - 8));
            }
        };
        if kind == ToolKind::Edit {
            if let (Some(old), Some(new)) = (
                input.get("old_string").and_then(Value::as_str),
                input.get("new_string").and_then(Value::as_str),
            ) {
                preview('-', old, &mut details);
                preview('+', new, &mut details);
            } else if let Some(content) = input.get("content").and_then(Value::as_str) {
                preview('+', content, &mut details);
            }
        }
        let heading = match kind {
            ToolKind::Shell => "Claude wants to run a command".to_string(),
            ToolKind::Edit => "Claude wants to edit a file".to_string(),
            ToolKind::Mcp {
                ref server,
                ref tool,
            } => format!("Claude wants to call MCP {server} / {tool}"),
            _ => format!("Claude wants to use {display}"),
        };
        let subject = match kind {
            ToolKind::Mcp { .. } | ToolKind::Other => {
                serde_json::to_string(&input).unwrap_or_default()
            }
            _ if title.is_empty() => display.to_string(),
            _ => title,
        };
        self.pending_perm.insert(
            id.clone(),
            PendingPermission {
                input,
                is_question: false,
                questions: vec![],
            },
        );
        Output::event(AgentEvent::ApprovalRequest {
            request_id: id,
            title: heading,
            subject,
            details,
            choices: vec![
                Choice::new("allow", "Allow once", ChoiceTone::Allow),
                Choice::new("deny", "Deny", ChoiceTone::Deny),
            ],
            call_id,
        })
    }

    fn on_stream_event(&mut self, v: &Value) -> Output {
        if !v.get("parent_tool_use_id").is_none_or(Value::is_null) {
            return Output::unknown();
        }
        let ev = v.get("event").cloned().unwrap_or(Value::Null);
        match ev.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                self.current_msg = ev
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                Output::unknown()
            }
            Some("content_block_start") => {
                if ev.pointer("/content_block/type").and_then(Value::as_str) == Some("text") {
                    if let (Some(msg), Some(idx)) = (
                        self.current_msg.clone(),
                        ev.get("index").and_then(Value::as_u64),
                    ) {
                        self.text_item(&msg, idx);
                    }
                }
                Output::unknown()
            }
            Some("content_block_delta") => {
                if ev.pointer("/delta/type").and_then(Value::as_str) != Some("text_delta") {
                    return Output::unknown();
                }
                let text = ev
                    .pointer("/delta/text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let (Some(msg), Some(idx)) = (
                    self.current_msg.clone(),
                    ev.get("index").and_then(Value::as_u64),
                ) else {
                    return Output::unknown();
                };
                let item_id = self.text_item(&msg, idx);
                Output::event(AgentEvent::AssistantDelta { item_id, text })
            }
            _ => Output::unknown(),
        }
    }

    fn text_item(&mut self, msg: &str, idx: u64) -> String {
        let key = (msg.to_string(), idx);
        if let Some(id) = self.block_items.get(&key) {
            return id.clone();
        }
        let id = format!("{msg}#{idx}");
        self.block_items.insert(key, id.clone());
        self.streaming_text
            .entry(msg.to_string())
            .or_default()
            .push_back(id.clone());
        id
    }

    fn on_assistant(&mut self, v: &Value) -> Output {
        if !v.get("parent_tool_use_id").is_none_or(Value::is_null) {
            return Output::unknown();
        }
        let msg = v.get("message").cloned().unwrap_or(Value::Null);
        let msg_id = msg
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut events = Vec::new();
        if let Some(model) = msg.get("model").and_then(Value::as_str) {
            if model != "<synthetic>" && self.last_model.as_deref() != Some(model) {
                self.last_model = Some(model.to_string());
                events.push(AgentEvent::SessionMeta {
                    model: Some(MetaValue::reported(model)),
                    effort: None,
                    session_id: None,
                });
            }
        }
        for block in msg
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let streamed = self
                        .streaming_text
                        .get_mut(&msg_id)
                        .and_then(VecDeque::pop_front);
                    let item_id = streamed.unwrap_or_else(|| {
                        self.synthetic_items += 1;
                        format!("{msg_id}#full{}", self.synthetic_items)
                    });
                    events.push(AgentEvent::AssistantDone {
                        item_id,
                        text: Some(text),
                    });
                }
                Some("tool_use") => {
                    let Some(call_id) = block.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    if !self.started_tools.insert(call_id.to_string()) {
                        continue;
                    }
                    let name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    let (kind, label, title) = classify_tool(&name, &input);
                    events.push(AgentEvent::ToolStarted {
                        call_id: call_id.to_string(),
                        kind,
                        name: label,
                        title,
                        input,
                    });
                }
                _ => {}
            }
        }
        if events.is_empty() {
            events.push(AgentEvent::Unknown);
        }
        Output {
            events,
            outgoing: vec![],
        }
    }

    fn on_user(&mut self, v: &Value) -> Output {
        if !v.get("parent_tool_use_id").is_none_or(Value::is_null) {
            return Output::unknown();
        }
        let mut events = Vec::new();
        let content = v
            .pointer("/message/content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(call_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let output = content_text(block.get("content").unwrap_or(&Value::Null));
            let result = v.get("tool_use_result").cloned();
            let diff = result.as_ref().and_then(structured_patch_diff);
            let declined = is_error
                && v.get("tool_result_meta")
                    .and_then(Value::as_array)
                    .is_some_and(|m| m.iter().any(|x| x.get("non_execution_kind").is_some()));
            let status = if declined {
                ToolStatus::Declined
            } else if is_error {
                ToolStatus::Failed
            } else {
                ToolStatus::Succeeded
            };
            events.push(AgentEvent::ToolCompleted {
                call_id: call_id.to_string(),
                status,
                output,
                result,
                duration_ms: None,
                diff,
            });
        }
        if events.is_empty() {
            events.push(AgentEvent::Unknown);
        }
        Output {
            events,
            outgoing: vec![],
        }
    }

    fn on_result(&mut self, v: &Value) -> Output {
        let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        let terminal = v
            .get("terminal_reason")
            .and_then(Value::as_str)
            .unwrap_or("");
        let outcome = if self.interrupt_requested && (is_error || terminal.starts_with("aborted")) {
            TurnOutcome::Interrupted
        } else if is_error {
            let msg = v
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    v.get("subtype")
                        .and_then(Value::as_str)
                        .map(|s| s.replace('_', " "))
                });
            TurnOutcome::Failed(msg)
        } else {
            TurnOutcome::Completed
        };
        let usage = Usage {
            // Prompt tokens include cache reads and writes, which Claude counts separately.
            input_tokens: v
                .pointer("/usage/input_tokens")
                .and_then(Value::as_u64)
                .map(|n| {
                    n + v
                        .pointer("/usage/cache_read_input_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        + v.pointer("/usage/cache_creation_input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0)
                }),
            output_tokens: v.pointer("/usage/output_tokens").and_then(Value::as_u64),
            session_cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
            duration_ms: v.get("duration_ms").and_then(Value::as_u64),
        };
        self.turn_active = false;
        self.interrupt_requested = false;
        self.streaming_text.clear();
        let mut events: Vec<AgentEvent> = self
            .pending_perm
            .drain()
            .map(|(request_id, _)| AgentEvent::RequestResolved { request_id })
            .collect();
        events.push(AgentEvent::TurnCompleted {
            outcome,
            usage: Some(usage),
        });
        Output {
            events,
            outgoing: vec![],
        }
    }
}

fn deny_response(id: &str, message: &str) -> Value {
    json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": id, "response": {"behavior": "deny", "message": message}}
    })
}

fn parse_ask_questions(input: &Value) -> Option<Vec<Question>> {
    let qs = input.get("questions")?.as_array()?;
    let mut out = Vec::new();
    for (i, q) in qs.iter().enumerate() {
        let text = q.get("question")?.as_str()?.to_string();
        let options = q
            .get("options")
            .and_then(Value::as_array)
            .map(|os| {
                os.iter()
                    .filter_map(|o| {
                        Some(QuestionOption {
                            label: o.get("label")?.as_str()?.to_string(),
                            description: o
                                .get("description")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(Question {
            id: i.to_string(),
            header: q
                .get("header")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            text,
            options,
            multi_select: q
                .get("multiSelect")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            free_text: true,
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Converts Claude's `structuredPatch` into unified-diff text.
fn structured_patch_diff(result: &Value) -> Option<String> {
    let hunks = result.get("structuredPatch")?.as_array()?;
    if hunks.is_empty() {
        return None;
    }
    let mut out = String::new();
    if let Some(p) = result.get("filePath").and_then(Value::as_str) {
        out.push_str(&format!("--- {p}\n+++ {p}\n"));
    }
    for h in hunks {
        let n = |k: &str| h.get(k).and_then(Value::as_u64).unwrap_or(0);
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            n("oldStart"),
            n("oldLines"),
            n("newStart"),
            n("newLines")
        ));
        for l in h
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(s) = l.as_str() {
                out.push_str(s);
                out.push('\n');
            }
        }
    }
    Some(out)
}

/// Returns (kind, display name, one-line title) for a Claude tool call.
pub fn classify_tool(name: &str, input: &Value) -> (ToolKind, String, String) {
    let s = |k: &str| {
        input
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    if let Some(rest) = name.strip_prefix("mcp__") {
        let (server, tool) = rest.split_once("__").unwrap_or((rest, ""));
        let title = input
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        return (
            ToolKind::Mcp {
                server: server.into(),
                tool: tool.into(),
            },
            "MCP".into(),
            title,
        );
    }
    match name {
        "Bash" => (ToolKind::Shell, "Shell".into(), s("command")),
        "Read" | "NotebookRead" => (ToolKind::Read, "Read".into(), s("file_path")),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            let p = if input.get("notebook_path").is_some() {
                s("notebook_path")
            } else {
                s("file_path")
            };
            (ToolKind::Edit, name.into(), p)
        }
        "Grep" | "Glob" => (ToolKind::Search, name.into(), s("pattern")),
        "WebFetch" => (ToolKind::Other, name.into(), s("url")),
        "WebSearch" => (ToolKind::Other, name.into(), s("query")),
        "Task" | "Agent" => (ToolKind::Other, name.into(), s("description")),
        _ => {
            let title = ["description", "command", "file_path", "query", "prompt"]
                .iter()
                .map(|k| s(k))
                .find(|v| !v.is_empty())
                .unwrap_or_default();
            (ToolKind::Other, name.into(), title)
        }
    }
}

impl Adapter for ClaudeAdapter {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Claude
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            harness_commands: true,
            interrupt: true,
            effort_reported: false,
        }
    }

    fn spawn_spec(&self) -> SpawnSpec {
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "stdio",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if let Some(m) = &self.cfg.model {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(e) = &self.cfg.effort {
            args.extend(["--effort".into(), e.clone()]);
        }
        if self.cfg.replay_user_messages {
            args.push("--replay-user-messages".into());
        }
        args.extend(self.cfg.extra_args.iter().cloned());
        SpawnSpec {
            program: self.cfg.program.clone(),
            args,
        }
    }

    fn start(&mut self) -> Vec<Value> {
        let id = self.req_id("init");
        self.pending_control
            .insert(id.clone(), PendingControl::Initialize);
        vec![
            json!({"type": "control_request", "request_id": id, "request": {"subtype": "initialize"}}),
        ]
    }

    fn on_record(&mut self, v: &Value) -> Output {
        match v.get("type").and_then(Value::as_str) {
            Some("control_response") => self.on_control_response(v),
            Some("control_request") => self.on_control_request(v),
            Some("control_cancel_request") => match v.get("request_id").and_then(Value::as_str) {
                Some(id) => {
                    self.pending_perm.remove(id);
                    Output::event(AgentEvent::RequestResolved {
                        request_id: id.to_string(),
                    })
                }
                None => Output::unknown(),
            },
            Some("system") => match v.get("subtype").and_then(Value::as_str) {
                Some("init") => {
                    let model = v.get("model").and_then(Value::as_str).map(|m| {
                        self.last_model = Some(m.to_string());
                        MetaValue::reported(m)
                    });
                    let session_id = v
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if self
                        .cfg
                        .expected_session_id
                        .as_ref()
                        .is_some_and(|id| Some(id) != session_id.as_ref())
                    {
                        self.ready = false;
                        return Output::event(AgentEvent::StartupFailed {
                            message: "Claude reported a different session identity; refusing further input".into(),
                        });
                    }
                    Output::event(AgentEvent::SessionMeta {
                        model,
                        effort: None,
                        session_id,
                    })
                }
                Some("api_retry") => Output::event(AgentEvent::Notice {
                    level: NoticeLevel::Warn,
                    text: format!(
                        "API retry{}",
                        v.get("error")
                            .map(|e| format!(": {}", content_text(e)))
                            .unwrap_or_default()
                    ),
                }),
                _ => Output::unknown(),
            },
            Some("rate_limit_event")
                if v.pointer("/rate_limit_info/status").and_then(Value::as_str)
                    == Some("rejected") =>
            {
                use kanna_agent_protocol::hosted_frontend::{NoticeKind, ProviderNotice};
                Output::event(AgentEvent::ProviderNotice(ProviderNotice {
                    kind: NoticeKind::QuotaRejected,
                    scope: v
                        .pointer("/rate_limit_info/model")
                        .or_else(|| v.pointer("/rate_limit_info/rateLimitType"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    text: "Claude reported rate_limit_info.status=rejected".into(),
                }))
            }
            Some("stream_event") => self.on_stream_event(v),
            Some("assistant") => self.on_assistant(v),
            Some("user") if v.get("isReplay").and_then(Value::as_bool) == Some(true) => {
                let matched = self.pending_input.as_ref().is_some_and(|(id, text)| {
                    v.get("uuid").and_then(Value::as_str) == Some(id.as_str())
                        && v.pointer("/message/role").and_then(Value::as_str) == Some("user")
                        && v.pointer("/message/content").and_then(Value::as_str)
                            == Some(text.as_str())
                        && self.cfg.expected_session_id.as_ref().is_none_or(|id| {
                            v.get("session_id").and_then(Value::as_str) == Some(id.as_str())
                        })
                });
                if matched {
                    let (delivery_id, _) = self.pending_input.take().unwrap();
                    Output::event(AgentEvent::InputAccepted { delivery_id })
                } else {
                    Output::unknown()
                }
            }
            Some("user") => self.on_user(v),
            Some("result") => self.on_result(v),
            Some(_) => Output::unknown(),
            None => Output::event(AgentEvent::Diagnostic {
                text: "Record without a type field".into(),
            }),
        }
    }

    fn on_malformed(&mut self, raw: &str) -> Vec<AgentEvent> {
        let mut ev = vec![AgentEvent::Diagnostic {
            text: "Unparseable record from Claude".into(),
        }];
        if looks_like_control(raw, CONTROL_MARKERS) {
            ev.push(AgentEvent::Degraded {
                reason: "a malformed control or result record arrived".into(),
            });
        }
        ev
    }

    fn send_prompt(&mut self, text: &str) -> Result<Vec<Value>, String> {
        if !self.ready {
            return Err("Claude is still starting".into());
        }
        if self.turn_active {
            return Err("a turn is already running".into());
        }
        self.turn_active = true;
        Ok(vec![json!({
            "type": "user",
            "message": {"role": "user", "content": text},
            "parent_tool_use_id": null,
            "session_id": ""
        })])
    }

    fn send_logical_prompt(&mut self, text: &str, delivery_id: &str) -> Result<Vec<Value>, String> {
        if !self.cfg.replay_user_messages {
            return Err("Claude replay acknowledgements were not enabled".into());
        }
        if self.pending_input.is_some() {
            return Err("previous Claude input has no correlated acknowledgement".into());
        }
        let mut messages = self.send_prompt(text)?;
        messages[0]["uuid"] = json!(delivery_id);
        if let Some(session_id) = &self.cfg.expected_session_id {
            messages[0]["session_id"] = json!(session_id);
        }
        self.pending_input = Some((delivery_id.to_string(), text.to_string()));
        Ok(messages)
    }

    fn respond(&mut self, request_id: &str, answer: &Answer) -> Result<Vec<Value>, String> {
        let Some(p) = self.pending_perm.remove(request_id) else {
            return Err("this request is no longer pending".into());
        };
        let body = match (answer, p.is_question) {
            (Answer::Choice(k), false) if k == "allow" => {
                json!({"behavior": "allow", "updatedInput": p.input})
            }
            (Answer::Choice(k), false) if k == "deny" => {
                json!({"behavior": "deny", "message": "The user denied this in agent-tui."})
            }
            (Answer::Answers(ans), true) => {
                let mut input = p.input.clone();
                let mut map = Map::new();
                for (q, a) in p.questions.iter().zip(ans) {
                    map.insert(q.text.clone(), Value::String(a.join(", ")));
                }
                if let Some(o) = input.as_object_mut() {
                    o.insert("answers".into(), Value::Object(map));
                }
                json!({"behavior": "allow", "updatedInput": input})
            }
            (Answer::Decline, true) => {
                json!({"behavior": "deny", "message": "The user declined to answer."})
            }
            _ => {
                self.pending_perm.insert(request_id.to_string(), p);
                return Err("that answer does not fit this request".into());
            }
        };
        Ok(vec![json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": request_id, "response": body}
        })])
    }

    fn interrupt(&mut self) -> Result<Vec<Value>, String> {
        if !self.turn_active {
            return Err("no turn is running".into());
        }
        let id = self.req_id("interrupt");
        self.pending_control
            .insert(id.clone(), PendingControl::Interrupt);
        self.interrupt_requested = true;
        Ok(vec![
            json!({"type": "control_request", "request_id": id, "request": {"subtype": "interrupt"}}),
        ])
    }
}
