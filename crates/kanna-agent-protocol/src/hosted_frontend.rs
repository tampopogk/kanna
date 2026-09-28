//! Authenticated PTY frontend control. Provider stdout is never a source of
//! these messages. Rendering and human keys remain on the PTY.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const CONFIG_ENV: &str = "KANNA_HOSTED_FRONTEND_CONFIG";
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_PENDING: usize = 64;
pub const MAX_PENDING_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RECEIPTS: usize = 4096;
pub const MAX_REQUEST_BYTES: usize = MAX_TEXT_BYTES * 2;
pub const MAX_RESPONSE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Binding {
    pub task_id: String,
    pub run_id: String,
    pub session_id: String,
    /// Fresh random value for each spawn, retained across daemon adoption.
    pub incarnation: String,
}

/// Private runtime file, mode 0600 inside a 0700 directory. Never log its token.
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub binding: Binding,
    pub capability: String,
    pub socket_path: String,
    pub journal_path: String,
    pub initial_delivery_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub binding: Binding,
    pub capability: String,
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Inspect,
    Submit {
        delivery_id: String,
        text: String,
        #[serde(default)]
        workflow_prompt: bool,
        #[serde(default)]
        run_id: Option<String>,
    },
    /// Fences future dispatch before session teardown/stage replacement.
    Retire {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Queued,
    Submitting,
    Submitted,
    Failed,
    Uncertain,
}

impl DeliveryState {
    pub fn pending(self) -> bool {
        matches!(self, Self::Queued | Self::Submitting)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Delivery {
    pub run_id: String,
    #[serde(default)]
    pub initial_prompt: bool,
    pub delivery_id: String,
    pub sequence: u64,
    pub payload_hash: String,
    /// Retained while pending/uncertain; confirmed text lives in Kanna's ledger.
    pub text: Option<String>,
    pub state: DeliveryState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Busy,
    Waiting,
    Idle,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    #[serde(default)]
    pub notice: Option<ProviderNotice>,
    pub version: u32,
    pub binding: Binding,
    pub frontend_pid: u32,
    pub active_run_id: String,
    pub sequence: u64,
    pub provider_session_id: Option<String>,
    pub state: RuntimeState,
    pub diagnostic: Option<String>,
    pub composer_text: String,
    pub queued_count: usize,
    pub deliveries: Vec<Delivery>,
    pub retired: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Snapshot { snapshot: Snapshot },
    Accepted { delivery: Delivery },
    Rejected { reason: String },
}

pub const FRONTENDS_ENV: &str = "KANNA_AGENT_FRONTENDS";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Frontend {
    #[default]
    Native,
    AgentTui,
}

pub fn parse_frontends(
    value: &serde_json::Value,
) -> Result<std::collections::BTreeMap<String, Frontend>, String> {
    let map = value
        .as_object()
        .ok_or("agentFrontends must be an object")?;
    if map.keys().any(|key| key != "claude" && key != "codex") {
        return Err("agentFrontends supports only claude and codex".into());
    }
    serde_json::from_value(value.clone())
        .map_err(|_| "agentFrontends values must be native or agent-tui".into())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    QuotaRejected,
    CapacityRefused,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderNotice {
    pub kind: NoticeKind,
    pub scope: Option<String>,
    pub text: String,
}
