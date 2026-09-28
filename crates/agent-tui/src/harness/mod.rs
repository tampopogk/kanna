//! Harness adapters and construction from launch options.

pub mod claude;
pub mod codex;

use crate::protocol::{Adapter, HarnessKind};

#[derive(Debug, Clone)]
pub struct HarnessOptions {
    pub kind: HarnessKind,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub cwd: Option<String>,
    /// Override the executable (defaults to `claude` / `codex` on PATH).
    pub program: Option<String>,
    pub extra_args: Vec<String>,
}

pub fn make_adapter(o: &HarnessOptions) -> Box<dyn Adapter> {
    match o.kind {
        HarnessKind::Claude => Box::new(claude::ClaudeAdapter::new(claude::ClaudeConfig {
            program: o.program.clone().unwrap_or_else(|| "claude".into()),
            model: o.model.clone(),
            effort: o.effort.clone(),
            extra_args: o.extra_args.clone(),
        })),
        HarnessKind::Codex => Box::new(codex::CodexAdapter::new(codex::CodexConfig {
            program: o.program.clone().unwrap_or_else(|| "codex".into()),
            model: o.model.clone(),
            effort: o.effort.clone(),
            cwd: o.cwd.clone(),
            extra_args: o.extra_args.clone(),
        })),
    }
}
