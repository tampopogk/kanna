pub mod client;
pub mod fdpass;
mod ghostty_env;
pub mod paths;
mod pty;
pub mod server;
mod session;
mod sig;
pub mod upgrade;

use serde_json::{json, Value};

pub fn login_shell() -> std::ffi::OsString {
    std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .or_else(|| ghostty_env::current_user().map(|u| u.shell).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "/bin/sh".into())
}

pub const PROTOCOL_VERSION: u64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    Code(i32),
    Signal(i32),
}

impl ExitStatus {
    pub fn shell_status(self) -> i32 {
        match self {
            ExitStatus::Code(c) => c,
            ExitStatus::Signal(s) => 128 + s,
        }
    }

    pub(crate) fn to_json(self) -> Value {
        match self {
            ExitStatus::Code(c) => json!({"code": c}),
            ExitStatus::Signal(s) => json!({"signal": s}),
        }
    }

    pub(crate) fn from_json(v: &Value) -> Option<Self> {
        if let Some(c) = v["code"].as_i64() {
            return Some(ExitStatus::Code(c as i32));
        }
        v["signal"].as_i64().map(|s| ExitStatus::Signal(s as i32))
    }
}
