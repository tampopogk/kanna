//! Codex over `codex app-server` (JSON-RPC on stdio, experimental).
//!
//! Verified against codex-cli 0.157.1 (see `tests/fixtures/codex/*.transcript`):
//! `initialize` → `initialized` → `thread/start` (response carries `model` and
//! `reasoningEffort`) → `turn/start` per prompt → item notifications →
//! `turn/completed`. `turn/interrupt {threadId, turnId}` ends a turn with
//! `status: "interrupted"`. Approvals are server requests answered by id;
//! command approvals list their `availableDecisions`. MCP tool calls ask via
//! `mcpServer/elicitation/request`. `item/tool/requestUserInput` and
//! `item/permissions/requestApproval` follow the generated schema only (not
//! observed live). Codex exposes no slash-command interface here.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::protocol::*;

#[derive(Debug, Clone)]
pub struct CodexConfig {
    pub program: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub cwd: Option<String>,
    pub extra_args: Vec<String>,
    pub resume: Option<String>,
    pub sandbox: Option<String>,
    pub approval_policy: Option<String>,
}

impl Default for CodexConfig {
    fn default() -> Self {
        Self {
            program: "codex".into(),
            model: None,
            effort: None,
            cwd: None,
            extra_args: Vec::new(),
            resume: None,
            sandbox: None,
            approval_policy: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    Initialize,
    ThreadStart,
    TurnStart,
    TurnInterrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReqKind {
    Command,
    FileChange,
    Permissions,
    UserInput,
    Elicitation,
}

#[derive(Debug)]
struct ServerRequest {
    id: Value,
    kind: ReqKind,
    params: Value,
    /// For command approvals: the decisions the server offered, by choice key.
    decisions: Vec<Value>,
    questions: Vec<Question>,
}

pub struct CodexAdapter {
    cfg: CodexConfig,
    next_id: i64,
    pending: HashMap<i64, Pending>,
    input_requests: HashMap<i64, String>,
    requests: HashMap<String, ServerRequest>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    turn_active: bool,
    interrupt_when_known: bool,
    /// Cumulative (input, output) tokens reported so far, and at the last turn end.
    usage_total: Option<(u64, u64)>,
    usage_at_turn_end: (u64, u64),
    /// fileChange item id -> (paths, unified diff), for approval cards.
    file_changes: HashMap<String, (String, String)>,
}

const CONTROL_MARKERS: &[&str] = &[
    "\"id\"",
    "requestApproval",
    "turn/completed",
    "elicitation",
    "requestUserInput",
];

impl CodexAdapter {
    pub fn new(cfg: CodexConfig) -> Self {
        Self {
            cfg,
            next_id: 0,
            pending: HashMap::new(),
            input_requests: HashMap::new(),
            requests: HashMap::new(),
            thread_id: None,
            turn_id: None,
            turn_active: false,
            interrupt_when_known: false,
            usage_total: None,
            usage_at_turn_end: (0, 0),
            file_changes: HashMap::new(),
        }
    }

    fn request(&mut self, kind: Pending, method: &str, params: Value) -> Value {
        self.next_id += 1;
        self.pending.insert(self.next_id, kind);
        json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
    }

    fn interrupt_msg(&mut self) -> Option<Value> {
        let (t, u) = (self.thread_id.clone()?, self.turn_id.clone()?);
        Some(self.request(
            Pending::TurnInterrupt,
            "turn/interrupt",
            json!({"threadId": t, "turnId": u}),
        ))
    }

    fn on_response(&mut self, v: &Value) -> Output {
        let id = v.get("id").and_then(Value::as_i64);
        let Some(kind) = id.and_then(|i| self.pending.remove(&i)) else {
            return Output::event(AgentEvent::Diagnostic {
                text: format!(
                    "Response to an unknown request id {}",
                    v.get("id").unwrap_or(&Value::Null)
                ),
            });
        };
        let err = v.get("error").map(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| e.to_string())
        });
        let delivery_id = id.and_then(|id| self.input_requests.remove(&id));
        let result = v.get("result").cloned().unwrap_or(Value::Null);
        match (kind, err) {
            (Pending::Initialize, Some(e)) => Output::event(AgentEvent::StartupFailed {
                message: format!("Codex rejected initialize: {e}"),
            }),
            (Pending::Initialize, None) => {
                let mut params = serde_json::Map::new();
                if let Some(cwd) = &self.cfg.cwd {
                    params.insert("cwd".into(), json!(cwd));
                }
                if let Some(m) = &self.cfg.model {
                    params.insert("model".into(), json!(m));
                }
                if let Some(e) = &self.cfg.effort {
                    params.insert("config".into(), json!({"model_reasoning_effort": e}));
                }
                if let Some(sandbox) = &self.cfg.sandbox {
                    params.insert("sandbox".into(), json!(sandbox));
                }
                if let Some(policy) = &self.cfg.approval_policy {
                    params.insert("approvalPolicy".into(), json!(policy));
                }
                let method = if let Some(id) = &self.cfg.resume {
                    params.insert("threadId".into(), json!(id));
                    "thread/resume"
                } else {
                    "thread/start"
                };
                let start = self.request(Pending::ThreadStart, method, Value::Object(params));
                Output {
                    events: vec![AgentEvent::Unknown],
                    outgoing: vec![json!({"jsonrpc": "2.0", "method": "initialized"}), start],
                }
            }
            (Pending::ThreadStart, Some(e)) => Output::event(AgentEvent::StartupFailed {
                message: format!(
                    "Codex could not {} a thread: {e}",
                    if self.cfg.resume.is_some() {
                        "resume"
                    } else {
                        "start"
                    }
                ),
            }),
            (Pending::ThreadStart, None) => {
                self.thread_id = result
                    .pointer("/thread/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if self.thread_id.is_none() {
                    return Output::event(AgentEvent::StartupFailed {
                        message: "thread/start returned no thread id".into(),
                    });
                }
                if self
                    .cfg
                    .resume
                    .as_ref()
                    .is_some_and(|id| Some(id) != self.thread_id.as_ref())
                {
                    self.thread_id = None;
                    return Output::event(AgentEvent::StartupFailed {
                        message: "Codex resumed a different thread; refusing to dispatch input"
                            .into(),
                    });
                }
                let model = result
                    .get("model")
                    .and_then(Value::as_str)
                    .map(MetaValue::reported);
                let effort = result
                    .get("reasoningEffort")
                    .and_then(Value::as_str)
                    .map(MetaValue::reported);
                Output {
                    events: vec![
                        AgentEvent::SessionMeta {
                            model,
                            effort,
                            session_id: self.thread_id.clone(),
                        },
                        AgentEvent::Ready { commands: vec![] },
                    ],
                    outgoing: vec![],
                }
            }
            (Pending::TurnStart, Some(e)) => {
                self.turn_active = false;
                self.interrupt_when_known = false;
                let mut events = Vec::new();
                if let Some(delivery_id) = delivery_id {
                    events.push(AgentEvent::InputRejected {
                        delivery_id,
                        reason: e.clone(),
                    });
                }
                events.push(AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Failed(Some(e)),
                    usage: None,
                });
                Output {
                    events,
                    outgoing: vec![],
                }
            }
            (Pending::TurnStart, None) => {
                let acknowledged_turn = result
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty());
                if let Some(t) = acknowledged_turn {
                    self.turn_id = Some(t.to_string());
                }
                let mut out = Output::unknown();
                if let Some(delivery_id) = delivery_id {
                    if acknowledged_turn.is_some() {
                        out.events.push(AgentEvent::InputAccepted { delivery_id });
                    } else {
                        out.events.push(AgentEvent::Degraded {
                            reason:
                                "turn/start response has no turn id; input acceptance is uncertain"
                                    .into(),
                        });
                    }
                }
                if self.interrupt_when_known {
                    self.interrupt_when_known = false;
                    out.outgoing.extend(self.interrupt_msg());
                }
                out
            }
            (Pending::TurnInterrupt, Some(e)) => Output::event(AgentEvent::Diagnostic {
                text: format!("Interrupt was rejected: {e}"),
            }),
            (Pending::TurnInterrupt, None) => Output::unknown(),
        }
    }

    fn on_server_request(&mut self, v: &Value) -> Output {
        let id = v.get("id").cloned().unwrap_or(Value::Null);
        let key = request_key(&id);
        let method = v.get("method").and_then(Value::as_str).unwrap_or("");
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let reject = |msg: String, code: i64| Output {
            events: vec![AgentEvent::Diagnostic { text: msg.clone() }],
            outgoing: vec![
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}}),
            ],
        };
        if !params.is_object() {
            return reject(
                format!("Codex sent {method} without params; answered with an error."),
                -32602,
            );
        }
        let call_id = params
            .get("itemId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let s = |p: &str| params.get(p).and_then(Value::as_str).map(str::to_string);
        match method {
            "item/commandExecution/requestApproval" => {
                let mut decisions: Vec<Value> = params
                    .get("availableDecisions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_else(|| vec![json!("accept"), json!("decline"), json!("cancel")]);
                // codex 0.157 omits "decline" from availableDecisions yet accepts it
                // (see fixtures); always offer a deny that lets the turn continue.
                if !decisions.iter().any(|d| d == "decline") {
                    let at = decisions
                        .iter()
                        .position(|d| d == "cancel")
                        .unwrap_or(decisions.len());
                    decisions.insert(at, json!("decline"));
                }
                let choices = decisions
                    .iter()
                    .enumerate()
                    .map(|(i, d)| decision_choice(i, d))
                    .collect();
                let command = params
                    .pointer("/commandActions/0/command")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| s("command"))
                    .unwrap_or_default();
                let mut details = Vec::new();
                if let Some(c) = s("cwd") {
                    details.push(format!("Directory: {c}"));
                }
                if let Some(r) = s("reason") {
                    details.push(r);
                }
                self.requests.insert(
                    key.clone(),
                    ServerRequest {
                        id,
                        kind: ReqKind::Command,
                        params,
                        decisions,
                        questions: vec![],
                    },
                );
                Output::event(AgentEvent::ApprovalRequest {
                    request_id: key,
                    title: "Codex wants to run a command".into(),
                    subject: command,
                    details,
                    choices,
                    call_id,
                })
            }
            "item/fileChange/requestApproval" => {
                let mut details = Vec::new();
                if let Some(r) = s("reason") {
                    details.push(r);
                }
                if let Some(g) = s("grantRoot") {
                    details.push(format!("Grants write access under {g}"));
                }
                let (subject, diff) = call_id
                    .as_ref()
                    .and_then(|c| self.file_changes.get(c).cloned())
                    .unwrap_or_else(|| ("the proposed patch".into(), String::new()));
                let diff_lines: Vec<&str> = diff
                    .lines()
                    .filter(|l| !l.starts_with("---") && !l.starts_with("+++"))
                    .collect();
                details.extend(diff_lines.iter().take(12).map(|l| l.to_string()));
                if diff_lines.len() > 12 {
                    details.push(format!(
                        "… {} more diff lines (expand the Edit call in Tools)",
                        diff_lines.len() - 12
                    ));
                }
                self.requests.insert(
                    key.clone(),
                    ServerRequest {
                        id,
                        kind: ReqKind::FileChange,
                        params,
                        decisions: vec![],
                        questions: vec![],
                    },
                );
                Output::event(AgentEvent::ApprovalRequest {
                    request_id: key,
                    title: "Codex wants to change files".into(),
                    subject,
                    details,
                    choices: vec![
                        Choice::new("accept", "Allow once", ChoiceTone::Allow),
                        Choice::new("acceptForSession", "Allow for session", ChoiceTone::Allow),
                        Choice::new("decline", "Deny", ChoiceTone::Deny),
                        Choice::new("cancel", "Deny and stop turn", ChoiceTone::Deny),
                    ],
                    call_id,
                })
            }
            "item/permissions/requestApproval" => {
                let mut details = Vec::new();
                if let Some(r) = s("reason") {
                    details.push(r);
                }
                let perms = params.get("permissions").cloned().unwrap_or(Value::Null);
                let subject = serde_json::to_string(&perms).unwrap_or_default();
                self.requests.insert(
                    key.clone(),
                    ServerRequest {
                        id,
                        kind: ReqKind::Permissions,
                        params,
                        decisions: vec![],
                        questions: vec![],
                    },
                );
                Output::event(AgentEvent::ApprovalRequest {
                    request_id: key,
                    title: "Codex requests additional permissions".into(),
                    subject,
                    details,
                    choices: vec![
                        Choice::new("turn", "Grant for this turn", ChoiceTone::Allow),
                        Choice::new("session", "Grant for session", ChoiceTone::Allow),
                        Choice::new("deny", "Deny", ChoiceTone::Deny),
                    ],
                    call_id,
                })
            }
            "item/tool/requestUserInput" => {
                let questions: Vec<Question> = params
                    .get("questions")
                    .and_then(Value::as_array)
                    .map(|qs| {
                        qs.iter()
                            .filter_map(|q| {
                                Some(Question {
                                    id: q.get("id")?.as_str()?.to_string(),
                                    header: q
                                        .get("header")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    text: q.get("question")?.as_str()?.to_string(),
                                    options: q
                                        .get("options")
                                        .and_then(Value::as_array)
                                        .map(|os| {
                                            os.iter()
                                                .filter_map(|o| {
                                                    Some(QuestionOption {
                                                        label: o
                                                            .get("label")?
                                                            .as_str()?
                                                            .to_string(),
                                                        description: o
                                                            .get("description")
                                                            .and_then(Value::as_str)
                                                            .unwrap_or("")
                                                            .to_string(),
                                                    })
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default(),
                                    multi_select: false,
                                    free_text: q
                                        .get("isOther")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false)
                                        || q.get("options").is_none_or(Value::is_null),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if questions.is_empty() {
                    return reject(
                        "Codex asked for input without readable questions; answered with an error."
                            .into(),
                        -32602,
                    );
                }
                self.requests.insert(
                    key.clone(),
                    ServerRequest {
                        id,
                        kind: ReqKind::UserInput,
                        params,
                        decisions: vec![],
                        questions: questions.clone(),
                    },
                );
                Output::event(AgentEvent::QuestionRequest {
                    request_id: key,
                    title: "Codex has a question".into(),
                    questions,
                    call_id,
                })
            }
            "mcpServer/elicitation/request" => {
                let server = s("serverName").unwrap_or_else(|| "MCP server".into());
                let message = s("message").unwrap_or_default();
                let form_fields = params
                    .pointer("/requestedSchema/properties")
                    .and_then(Value::as_object)
                    .is_some_and(|p| !p.is_empty());
                let mut details = Vec::new();
                if let Some(ps) = params
                    .pointer("/_meta/tool_params_display")
                    .and_then(Value::as_array)
                {
                    for p in ps {
                        let n = p
                            .get("display_name")
                            .or(p.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("?");
                        let val = p
                            .get("value")
                            .map(|v| {
                                v.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| v.to_string())
                            })
                            .unwrap_or_default();
                        details.push(format!("{n}: {val}"));
                    }
                }
                // URL mode asks the person to visit a URL; never offer Allow
                // without showing exactly which one.
                let url_mode = s("mode").as_deref() == Some("url");
                let url = s("url").filter(|u| !u.trim().is_empty());
                if url_mode {
                    match &url {
                        Some(u) => {
                            details.insert(0, format!("URL: {u}"));
                            details.push(
                                "agent-tui does not open links; allowing tells the server you will visit it."
                                    .into(),
                            );
                        }
                        None => details.push("This URL request did not include a URL.".into()),
                    }
                }
                let mut choices = Vec::new();
                if url_mode && url.is_none() {
                    // Nothing verifiable to allow.
                } else if form_fields {
                    details.push(
                        "This request asks for form input, which agent-tui cannot fill in yet."
                            .into(),
                    );
                } else {
                    choices.push(Choice::new("accept", "Allow", ChoiceTone::Allow));
                }
                choices.push(Choice::new("decline", "Decline", ChoiceTone::Deny));
                choices.push(Choice::new("cancel", "Cancel", ChoiceTone::Neutral));
                self.requests.insert(
                    key.clone(),
                    ServerRequest {
                        id,
                        kind: ReqKind::Elicitation,
                        params,
                        decisions: vec![],
                        questions: vec![],
                    },
                );
                Output::event(AgentEvent::ApprovalRequest {
                    request_id: key,
                    title: format!("{server} requests permission"),
                    subject: message,
                    details,
                    choices,
                    call_id: None,
                })
            }
            _ => reject(
                format!("Codex sent an unsupported request ({method}); answered with an error."),
                -32601,
            ),
        }
    }

    fn on_notification(&mut self, v: &Value) -> Output {
        let method = v.get("method").and_then(Value::as_str).unwrap_or("");
        let p = v.get("params").cloned().unwrap_or(Value::Null);
        let s = |ptr: &str| p.pointer(ptr).and_then(Value::as_str).map(str::to_string);
        match method {
            "turn/started" => {
                if let Some(t) = s("/turn/id") {
                    self.turn_id = Some(t);
                }
                let mut out = Output::event(AgentEvent::TurnStarted);
                if self.interrupt_when_known && self.turn_id.is_some() {
                    self.interrupt_when_known = false;
                    out.outgoing.extend(self.interrupt_msg());
                }
                out
            }
            "item/agentMessage/delta" => match (s("/itemId"), s("/delta")) {
                (Some(item_id), Some(text)) => {
                    Output::event(AgentEvent::AssistantDelta { item_id, text })
                }
                _ => Output::event(AgentEvent::Diagnostic {
                    text: "agentMessage delta without itemId/delta".into(),
                }),
            },
            "item/commandExecution/outputDelta" => match (s("/itemId"), s("/delta")) {
                (Some(call_id), Some(text)) => {
                    Output::event(AgentEvent::ToolOutputDelta { call_id, text })
                }
                _ => Output::unknown(),
            },
            "item/started" => self.on_item(p.get("item").unwrap_or(&Value::Null), false),
            "item/completed" => self.on_item(p.get("item").unwrap_or(&Value::Null), true),
            "thread/tokenUsage/updated" => {
                let total = p
                    .pointer("/tokenUsage/total")
                    .cloned()
                    .unwrap_or(Value::Null);
                if let (Some(i), Some(o)) = (
                    total.get("inputTokens").and_then(Value::as_u64),
                    total.get("outputTokens").and_then(Value::as_u64),
                ) {
                    self.usage_total = Some((i, o));
                }
                Output::unknown()
            }
            "thread/settings/updated" => {
                let model = s("/threadSettings/model").map(MetaValue::reported);
                let effort = s("/threadSettings/effort").map(MetaValue::reported);
                if model.is_none() && effort.is_none() {
                    return Output::unknown();
                }
                Output::event(AgentEvent::SessionMeta {
                    model,
                    effort,
                    session_id: None,
                })
            }
            "serverRequest/resolved" => {
                let key = request_key(p.get("requestId").unwrap_or(&Value::Null));
                self.requests.remove(&key);
                Output::event(AgentEvent::RequestResolved { request_id: key })
            }
            "turn/completed" => {
                let status = s("/turn/status").unwrap_or_default();
                let error = p
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        p.pointer("/turn/error")
                            .filter(|e| !e.is_null())
                            .map(|e| e.to_string())
                    });
                let outcome = match status.as_str() {
                    "completed" => TurnOutcome::Completed,
                    "interrupted" => TurnOutcome::Interrupted,
                    _ => TurnOutcome::Failed(error.or(Some(status))),
                };
                let mut usage = Usage {
                    duration_ms: p.pointer("/turn/durationMs").and_then(Value::as_u64),
                    ..Usage::default()
                };
                if let Some((i, o)) = self.usage_total {
                    let (pi, po) = self.usage_at_turn_end;
                    usage.input_tokens = Some(i.saturating_sub(pi));
                    usage.output_tokens = Some(o.saturating_sub(po));
                    self.usage_at_turn_end = (i, o);
                }
                self.turn_active = false;
                self.turn_id = None;
                self.interrupt_when_known = false;
                self.file_changes.clear();
                let mut events: Vec<AgentEvent> = self
                    .requests
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
            "error" => {
                let msg = s("/error/message")
                    .or_else(|| s("/message"))
                    .unwrap_or_else(|| p.to_string());
                let retry = p.get("willRetry").and_then(Value::as_bool).unwrap_or(false);
                Output::event(AgentEvent::Notice {
                    level: if retry {
                        NoticeLevel::Warn
                    } else {
                        NoticeLevel::Error
                    },
                    text: if retry {
                        format!("{msg} (retrying)")
                    } else {
                        msg
                    },
                })
            }
            "warning" | "configWarning" | "deprecationNotice" => {
                let msg = s("/message")
                    .or_else(|| s("/summary"))
                    .unwrap_or_else(|| p.to_string());
                Output::event(AgentEvent::Notice {
                    level: NoticeLevel::Warn,
                    text: msg,
                })
            }
            _ => Output::unknown(),
        }
    }

    fn on_item(&mut self, item: &Value, completed: bool) -> Output {
        let Some(id) = item.get("id").and_then(Value::as_str).map(str::to_string) else {
            return Output::unknown();
        };
        let s = |k: &str| {
            item.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let status_of = |raw: &str| match raw {
            "completed" => ToolStatus::Succeeded,
            "failed" => ToolStatus::Failed,
            "declined" => ToolStatus::Declined,
            _ => ToolStatus::Running,
        };
        let duration = item.get("durationMs").and_then(Value::as_u64);
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "agentMessage" => {
                if completed {
                    Output::event(AgentEvent::AssistantDone {
                        item_id: id,
                        text: Some(s("text")),
                    })
                } else {
                    Output::unknown()
                }
            }
            "commandExecution" => {
                let title = item
                    .pointer("/commandActions/0/command")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| s("command"));
                if !completed {
                    return Output::event(AgentEvent::ToolStarted {
                        call_id: id,
                        kind: ToolKind::Shell,
                        name: "Shell".into(),
                        title,
                        input: json!({"command": item.get("command"), "cwd": item.get("cwd")}),
                    });
                }
                let mut status = status_of(&s("status"));
                if status == ToolStatus::Succeeded
                    && item
                        .get("exitCode")
                        .and_then(Value::as_i64)
                        .is_some_and(|c| c != 0)
                {
                    status = ToolStatus::Failed;
                }
                let mut output = s("aggregatedOutput");
                if let Some(code) = item.get("exitCode").and_then(Value::as_i64) {
                    if code != 0 {
                        output.push_str(&format!(
                            "{}exit code {code}",
                            if output.is_empty() || output.ends_with('\n') {
                                ""
                            } else {
                                "\n"
                            }
                        ));
                    }
                }
                Output::event(AgentEvent::ToolCompleted {
                    call_id: id,
                    status,
                    output,
                    result: Some(item.clone()),
                    duration_ms: duration,
                    diff: None,
                })
            }
            "fileChange" => {
                let changes = item
                    .get("changes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let paths: Vec<String> = changes
                    .iter()
                    .filter_map(|c| c.get("path").and_then(Value::as_str).map(short_path))
                    .collect();
                let diff: String = changes
                    .iter()
                    .map(|c| {
                        let p = c.get("path").and_then(Value::as_str).unwrap_or("?");
                        format!(
                            "--- {p}\n+++ {p}\n{}",
                            c.get("diff").and_then(Value::as_str).unwrap_or("")
                        )
                    })
                    .collect();
                if !completed {
                    self.file_changes
                        .insert(id.clone(), (paths.join(", "), diff));
                    return Output::event(AgentEvent::ToolStarted {
                        call_id: id,
                        kind: ToolKind::Edit,
                        name: "Edit".into(),
                        title: paths.join(", "),
                        input: Value::Array(changes),
                    });
                }
                Output::event(AgentEvent::ToolCompleted {
                    call_id: id,
                    status: status_of(&s("status")),
                    output: String::new(),
                    result: Some(item.clone()),
                    duration_ms: duration,
                    diff: if diff.is_empty() { None } else { Some(diff) },
                })
            }
            "mcpToolCall" => {
                if !completed {
                    let args = item.get("arguments").cloned().unwrap_or(Value::Null);
                    let title = args
                        .as_object()
                        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
                        .unwrap_or_default();
                    return Output::event(AgentEvent::ToolStarted {
                        call_id: id,
                        kind: ToolKind::Mcp {
                            server: s("server"),
                            tool: s("tool"),
                        },
                        name: "MCP".into(),
                        title,
                        input: args,
                    });
                }
                let error = item
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let result = item.get("result").cloned().filter(|r| !r.is_null());
                let output = match (&error, &result) {
                    (Some(e), _) => e.clone(),
                    (None, Some(r)) => content_text(r.get("content").unwrap_or(&Value::Null)),
                    (None, None) => String::new(),
                };
                let mut status = status_of(&s("status"));
                if error.is_some() {
                    status = ToolStatus::Failed;
                }
                Output::event(AgentEvent::ToolCompleted {
                    call_id: id,
                    status,
                    output,
                    result: result.or_else(|| item.get("error").cloned()),
                    duration_ms: duration,
                    diff: None,
                })
            }
            t @ ("webSearch"
            | "dynamicToolCall"
            | "collabAgentToolCall"
            | "imageGeneration"
            | "imageView") => {
                let name = match t {
                    "webSearch" => "WebSearch".to_string(),
                    "imageView" => "ViewImage".to_string(),
                    "imageGeneration" => "ImageGen".to_string(),
                    _ => item
                        .get("tool")
                        .and_then(Value::as_str)
                        .unwrap_or(t)
                        .to_string(),
                };
                if !completed {
                    let title = ["query", "prompt", "path"]
                        .iter()
                        .map(|k| s(k))
                        .find(|x| !x.is_empty())
                        .unwrap_or_default();
                    return Output::event(AgentEvent::ToolStarted {
                        call_id: id,
                        kind: ToolKind::Other,
                        name,
                        title,
                        input: item
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| item.clone()),
                    });
                }
                let failed = item.get("success").and_then(Value::as_bool) == Some(false)
                    || s("status") == "failed";
                Output::event(AgentEvent::ToolCompleted {
                    call_id: id,
                    status: if failed {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Succeeded
                    },
                    output: String::new(),
                    result: Some(item.clone()),
                    duration_ms: duration,
                    diff: None,
                })
            }
            _ => Output::unknown(),
        }
    }
}

fn short_path(p: &str) -> String {
    p.rsplit('/').next().unwrap_or(p).to_string()
}

pub fn request_key(id: &Value) -> String {
    match id {
        Value::String(s) => format!("s:{s}"),
        other => other.to_string(),
    }
}

fn decision_choice(i: usize, d: &Value) -> Choice {
    let key = i.to_string();
    match d {
        Value::String(s) => match s.as_str() {
            "accept" => Choice::new(key, "Allow once", ChoiceTone::Allow),
            "acceptForSession" => Choice::new(key, "Allow for session", ChoiceTone::Allow),
            "decline" => Choice::new(key, "Deny", ChoiceTone::Deny),
            "cancel" => Choice::new(key, "Deny and stop turn", ChoiceTone::Deny),
            other => Choice::new(key, other, ChoiceTone::Neutral),
        },
        Value::Object(o) if o.contains_key("acceptWithExecpolicyAmendment") => {
            Choice::new(key, "Always allow this command", ChoiceTone::Allow)
        }
        Value::Object(o) if o.contains_key("applyNetworkPolicyAmendment") => {
            Choice::new(key, "Apply network rule", ChoiceTone::Neutral)
        }
        Value::Object(o) => Choice::new(
            key,
            o.keys()
                .next()
                .cloned()
                .unwrap_or_else(|| "Decision".into()),
            ChoiceTone::Neutral,
        ),
        other => Choice::new(key, other.to_string(), ChoiceTone::Neutral),
    }
}

impl Adapter for CodexAdapter {
    fn kind(&self) -> HarnessKind {
        HarnessKind::Codex
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            harness_commands: false,
            interrupt: true,
            effort_reported: true,
        }
    }

    fn spawn_spec(&self) -> SpawnSpec {
        let mut args = vec!["app-server".to_string()];
        args.extend(self.cfg.extra_args.iter().cloned());
        SpawnSpec {
            program: self.cfg.program.clone(),
            args,
        }
    }

    fn start(&mut self) -> Vec<Value> {
        vec![self.request(
            Pending::Initialize,
            "initialize",
            json!({"clientInfo": {"name": "agent-tui", "title": "agent-tui", "version": env!("CARGO_PKG_VERSION")}}),
        )]
    }

    fn on_record(&mut self, v: &Value) -> Output {
        let has_id = v.get("id").is_some();
        let has_method = v.get("method").is_some();
        match (has_method, has_id) {
            (true, true) => self.on_server_request(v),
            (true, false) => self.on_notification(v),
            (false, true) if v.get("result").is_some() || v.get("error").is_some() => {
                self.on_response(v)
            }
            _ => Output::event(AgentEvent::Diagnostic {
                text: "Record is neither a JSON-RPC request, response nor notification".into(),
            }),
        }
    }

    fn on_malformed(&mut self, raw: &str) -> Vec<AgentEvent> {
        let mut ev = vec![AgentEvent::Diagnostic {
            text: "Unparseable record from Codex".into(),
        }];
        if looks_like_control(raw, CONTROL_MARKERS) {
            ev.push(AgentEvent::Degraded {
                reason: "a malformed request, response or completion arrived".into(),
            });
        }
        ev
    }

    fn send_prompt(&mut self, text: &str) -> Result<Vec<Value>, String> {
        let Some(thread) = self.thread_id.clone() else {
            return Err("Codex is still starting".into());
        };
        if self.turn_active {
            return Err("a turn is already running".into());
        }
        self.turn_active = true;
        self.turn_id = None;
        Ok(vec![self.request(
            Pending::TurnStart,
            "turn/start",
            json!({"threadId": thread, "input": [{"type": "text", "text": text, "text_elements": []}]}),
        )])
    }

    fn send_logical_prompt(&mut self, text: &str, delivery_id: &str) -> Result<Vec<Value>, String> {
        let messages = self.send_prompt(text)?;
        let id = messages[0]["id"]
            .as_i64()
            .ok_or("turn/start has no request id")?;
        self.input_requests.insert(id, delivery_id.to_string());
        Ok(messages)
    }

    fn respond(&mut self, request_id: &str, answer: &Answer) -> Result<Vec<Value>, String> {
        let Some(req) = self.requests.remove(request_id) else {
            return Err("this request is no longer pending".into());
        };
        let result = match (req.kind, answer) {
            (ReqKind::Command, Answer::Choice(k)) => k
                .parse::<usize>()
                .ok()
                .and_then(|i| req.decisions.get(i))
                .map(|d| json!({"decision": d})),
            (ReqKind::FileChange, Answer::Choice(k))
                if ["accept", "acceptForSession", "decline", "cancel"].contains(&k.as_str()) =>
            {
                Some(json!({"decision": k}))
            }
            (ReqKind::Permissions, Answer::Choice(k)) => match k.as_str() {
                "turn" | "session" => Some(
                    json!({"permissions": req.params.get("permissions").cloned().unwrap_or(json!({})), "scope": k}),
                ),
                "deny" => Some(json!({"permissions": {}, "scope": "turn"})),
                _ => None,
            },
            (ReqKind::Elicitation, Answer::Choice(k))
                if ["accept", "decline", "cancel"].contains(&k.as_str()) =>
            {
                Some(json!({"action": k, "content": null}))
            }
            (ReqKind::UserInput, Answer::Answers(ans)) => {
                let mut map = serde_json::Map::new();
                for (q, a) in req.questions.iter().zip(ans) {
                    map.insert(q.id.clone(), json!({"answers": a}));
                }
                Some(json!({"answers": map}))
            }
            (ReqKind::UserInput, Answer::Decline) => {
                let mut map = serde_json::Map::new();
                for q in &req.questions {
                    map.insert(q.id.clone(), json!({"answers": []}));
                }
                Some(json!({"answers": map}))
            }
            _ => None,
        };
        match result {
            Some(r) => Ok(vec![json!({"jsonrpc": "2.0", "id": req.id, "result": r})]),
            None => {
                self.requests.insert(request_id.to_string(), req);
                Err("that answer does not fit this request".into())
            }
        }
    }

    fn interrupt(&mut self) -> Result<Vec<Value>, String> {
        if !self.turn_active {
            return Err("no turn is running".into());
        }
        match self.interrupt_msg() {
            Some(m) => Ok(vec![m]),
            None => {
                // turn/start has not answered yet; interrupt as soon as the id is known.
                self.interrupt_when_known = true;
                Ok(vec![])
            }
        }
    }
}
