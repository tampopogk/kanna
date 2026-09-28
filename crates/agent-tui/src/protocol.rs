//! Harness-neutral events and the adapter contract.
//!
//! Each harness adapter is a pure translator: it turns one incoming protocol
//! record into zero or more [`AgentEvent`]s (plus any protocol-level replies it
//! must send), and turns UI intents (prompt, answer, interrupt) into outgoing
//! records. It never touches the terminal or the process, which keeps it
//! testable against recorded transcripts.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessKind {
    Claude,
    Codex,
}

impl HarnessKind {
    pub fn label(self) -> &'static str {
        match self {
            HarnessKind::Claude => "Claude",
            HarnessKind::Codex => "Codex",
        }
    }
}

/// Where a header value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaSource {
    /// The harness reported it in its structured stream.
    Reported,
    /// Only known because we asked for it on the command line.
    Requested,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaValue {
    pub value: String,
    pub source: MetaSource,
}

impl MetaValue {
    pub fn reported(v: impl Into<String>) -> Self {
        Self {
            value: v.into(),
            source: MetaSource::Reported,
        }
    }
    pub fn requested(v: impl Into<String>) -> Self {
        Self {
            value: v.into(),
            source: MetaSource::Requested,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolKind {
    Shell,
    Read,
    Edit,
    Search,
    Mcp { server: String, tool: String },
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Succeeded,
    Failed,
    Declined,
    /// The turn ended (e.g. interrupted) before the tool reported completion.
    Stopped,
}

impl ToolStatus {
    pub fn is_failure(self) -> bool {
        matches!(self, ToolStatus::Failed | ToolStatus::Declined)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    Completed,
    Failed(Option<String>),
    Interrupted,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Cumulative session cost as reported by the harness.
    pub session_cost_usd: Option<f64>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommand {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceTone {
    Allow,
    Deny,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// Adapter-private key identifying the decision to send.
    pub key: String,
    pub label: String,
    pub tone: ChoiceTone,
}

impl Choice {
    pub fn new(key: impl Into<String>, label: impl Into<String>, tone: ChoiceTone) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            tone,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub header: String,
    pub text: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
    /// Whether a typed answer is accepted in place of an option.
    pub free_text: bool,
}

/// A decision the person made on a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// One of the [`Choice`]s offered on an approval card, by key.
    Choice(String),
    /// Answers for each question of a question card, in order.
    Answers(Vec<Vec<String>>),
    /// Decline a question card without answering.
    Decline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// Handshake finished; prompts can be sent.
    Ready {
        commands: Vec<HarnessCommand>,
    },
    /// The harness refused to start a session (auth, config, …).
    StartupFailed {
        message: String,
    },
    SessionMeta {
        model: Option<MetaValue>,
        effort: Option<MetaValue>,
        session_id: Option<String>,
    },
    /// Provider-confirmed acceptance of a specifically correlated logical input.
    InputAccepted {
        delivery_id: String,
    },
    /// The provider explicitly rejected a correlated turn/start request.
    InputRejected {
        delivery_id: String,
        reason: String,
    },
    TurnStarted,
    AssistantDelta {
        item_id: String,
        text: String,
    },
    /// A message finished; `text`, when present, is the authoritative full text.
    AssistantDone {
        item_id: String,
        text: Option<String>,
    },
    ToolStarted {
        call_id: String,
        kind: ToolKind,
        name: String,
        title: String,
        input: Value,
    },
    ToolOutputDelta {
        call_id: String,
        text: String,
    },
    ToolCompleted {
        call_id: String,
        status: ToolStatus,
        output: String,
        result: Option<Value>,
        duration_ms: Option<u64>,
        diff: Option<String>,
    },
    ApprovalRequest {
        request_id: String,
        title: String,
        /// The primary thing being approved (a command, a path, a tool name).
        subject: String,
        details: Vec<String>,
        choices: Vec<Choice>,
        call_id: Option<String>,
    },
    QuestionRequest {
        request_id: String,
        title: String,
        questions: Vec<Question>,
        call_id: Option<String>,
    },
    /// The harness no longer needs an answer for this request.
    RequestResolved {
        request_id: String,
    },
    TurnCompleted {
        outcome: TurnOutcome,
        usage: Option<Usage>,
    },
    Notice {
        level: NoticeLevel,
        text: String,
    },
    /// A protocol problem worth showing (unsupported request, bad record, …).
    Diagnostic {
        text: String,
    },
    /// Turn state can no longer be trusted; sends are disabled until a turn
    /// completion re-establishes it or a new session starts.
    Degraded {
        reason: String,
    },
    /// A record with no visible effect; kept for the raw inspector.
    Unknown,
}

/// What one incoming record produced.
#[derive(Debug, Default)]
pub struct Output {
    pub events: Vec<AgentEvent>,
    /// Protocol replies to write to the harness immediately.
    pub outgoing: Vec<Value>,
}

impl Output {
    pub fn event(e: AgentEvent) -> Self {
        Self {
            events: vec![e],
            outgoing: Vec::new(),
        }
    }
    pub fn unknown() -> Self {
        Self::event(AgentEvent::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Slash commands from the harness can be listed and run.
    pub harness_commands: bool,
    pub interrupt: bool,
    /// The harness reports reasoning effort in its session metadata.
    pub effort_reported: bool,
}

/// How to launch the harness process.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
}

pub trait Adapter: Send {
    fn kind(&self) -> HarnessKind;
    fn capabilities(&self) -> Capabilities;
    fn spawn_spec(&self) -> SpawnSpec;
    /// Records to send right after spawning.
    fn start(&mut self) -> Vec<Value>;
    fn on_record(&mut self, record: &Value) -> Output;
    /// A line that failed to parse. Returns the events it should produce.
    fn on_malformed(&mut self, raw: &str) -> Vec<AgentEvent>;
    fn send_prompt(&mut self, text: &str) -> Result<Vec<Value>, String>;
    fn send_logical_prompt(
        &mut self,
        _text: &str,
        _delivery_id: &str,
    ) -> Result<Vec<Value>, String> {
        Err("this adapter cannot acknowledge logical input".into())
    }
    fn respond(&mut self, request_id: &str, answer: &Answer) -> Result<Vec<Value>, String>;
    fn interrupt(&mut self) -> Result<Vec<Value>, String>;
}

/// Heuristic shared by adapters: does an unparseable line look like it was a
/// control/approval/completion record, so that turn state may now be wrong?
pub fn looks_like_control(raw: &str, markers: &[&str]) -> bool {
    markers.iter().any(|m| raw.contains(m))
}

/// Joins the text parts of an MCP-style content array (or returns a string).
pub fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|it| match it {
                Value::String(s) => Some(s.clone()),
                Value::Object(o) => match o.get("type").and_then(Value::as_str) {
                    Some("text") => o.get("text").and_then(Value::as_str).map(str::to_string),
                    Some(t) => Some(format!("[{t}]")),
                    None => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn str_at<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}
