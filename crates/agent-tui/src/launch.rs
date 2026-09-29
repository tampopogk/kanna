//! Kanna's native CLI argv, parsed before creating a JSON transport.
//!
//! This deliberately accepts a bounded contract. Unknown flags (especially
//! transport and security flags) fail closed instead of being silently lost.
use crate::harness::{claude::ClaudeConfig, codex::CodexConfig};
use crate::protocol::{Adapter, HarnessKind};

#[derive(Debug, Clone)]
pub struct HostedLaunch {
    pub initial_prompt: Option<String>,
    pub config: HostedConfig,
}

#[derive(Debug, Clone)]
pub enum HostedConfig {
    Claude(ClaudeConfig),
    Codex(CodexConfig),
}

impl HostedLaunch {
    pub fn parse(
        kind: HarnessKind,
        program: String,
        cwd: String,
        args: &[String],
    ) -> Result<Self, String> {
        let mut prompt = None;
        let mut claude = ClaudeConfig {
            program: program.clone(),
            ..Default::default()
        };
        let mut codex = CodexConfig {
            program,
            cwd: Some(cwd),
            ..Default::default()
        };
        let mut i = 0;
        let mut yolo = false;
        while i < args.len() {
            let arg = args[i].as_str();
            i += 1;
            let mut value = || -> Result<String, String> {
                let result = args
                    .get(i)
                    .cloned()
                    .ok_or_else(|| format!("{arg} requires a value"))?;
                i += 1;
                Ok(result)
            };
            match (kind, arg) {
                (_, "--") => {
                    for text in &args[i..] {
                        set_prompt(&mut prompt, text)?;
                    }
                    break;
                }
                (HarnessKind::Claude, "--model") => claude.model = Some(value()?),
                (HarnessKind::Claude, "--effort") => claude.effort = Some(value()?),
                (HarnessKind::Claude, "--session-id" | "--resume") => {
                    if claude.expected_session_id.is_some() {
                        return Err("only one Claude session binding is allowed".into());
                    }
                    let id = value()?;
                    if id.is_empty() {
                        return Err(format!("{arg} requires a session id"));
                    }
                    claude.expected_session_id = Some(id.clone());
                    claude.extra_args.extend([arg.into(), id]);
                }
                (
                    HarnessKind::Claude,
                    "--autocompact"
                    | "--allowedTools"
                    | "--disallowedTools"
                    | "--max-turns"
                    | "--max-budget-usd"
                    | "--append-system-prompt"
                    | "--mcp-config"
                    | "--permission-mode",
                ) => claude.extra_args.extend([arg.into(), value()?]),
                (
                    HarnessKind::Claude,
                    "--dangerously-skip-permissions" | "--allow-dangerously-skip-permissions",
                ) => {
                    claude.extra_args.push(arg.into());
                }
                (HarnessKind::Codex, "-m" | "--model") => codex.model = Some(value()?),
                (HarnessKind::Codex, "-c" | "--config") => {
                    let setting = value()?;
                    if setting
                        .split_once('=')
                        .is_none_or(|(key, _)| key.trim().is_empty())
                    {
                        return Err("Codex -c requires key=value".into());
                    }
                    // The provider parses TOML and applies restrictions; keep every
                    // override in order, including MCP command, args and environment.
                    codex.extra_args.extend(["-c".into(), setting]);
                }
                (HarnessKind::Codex, "--yolo" | "--dangerously-bypass-approvals-and-sandbox") => {
                    yolo = true
                }
                (HarnessKind::Codex, "--sandbox" | "-s") => {
                    let mode = value()?;
                    if !["read-only", "workspace-write", "danger-full-access"]
                        .contains(&mode.as_str())
                    {
                        return Err(format!("unsupported Codex sandbox: {mode}"));
                    }
                    codex.sandbox = Some(mode);
                }
                (HarnessKind::Codex, "--ask-for-approval" | "-a") => {
                    let policy = value()?;
                    if !["untrusted", "on-request", "never"].contains(&policy.as_str()) {
                        return Err(format!("unsupported Codex approval policy: {policy}"));
                    }
                    codex.approval_policy = Some(policy);
                }
                (HarnessKind::Codex, "resume") if prompt.is_none() && codex.resume.is_none() => {
                    let id = value()?;
                    if id.is_empty() || id.starts_with('-') {
                        return Err("Codex resume requires an explicit thread id".into());
                    }
                    codex.resume = Some(id);
                }
                (_, flag) if flag.starts_with('-') => {
                    return Err(format!("unsupported Kanna {} option: {flag}", kind.label()))
                }
                (_, text) => set_prompt(&mut prompt, text)?,
            }
        }
        let config = match kind {
            HarnessKind::Claude => {
                claude.replay_user_messages = true;
                HostedConfig::Claude(claude)
            }
            HarnessKind::Codex => {
                if yolo {
                    if codex.sandbox.is_some() || codex.approval_policy.is_some() {
                        return Err(
                            "Codex --yolo conflicts with explicit sandbox or approval settings"
                                .into(),
                        );
                    }
                    codex.sandbox = Some("danger-full-access".into());
                    codex.approval_policy = Some("never".into());
                }
                HostedConfig::Codex(codex)
            }
        };
        Ok(Self {
            initial_prompt: prompt.filter(|p| !p.is_empty()),
            config,
        })
    }

    pub fn adapter(&self) -> Box<dyn Adapter> {
        match &self.config {
            HostedConfig::Claude(config) => {
                Box::new(crate::harness::claude::ClaudeAdapter::new(config.clone()))
            }
            HostedConfig::Codex(config) => {
                Box::new(crate::harness::codex::CodexAdapter::new(config.clone()))
            }
        }
    }
}

fn set_prompt(prompt: &mut Option<String>, text: &str) -> Result<(), String> {
    if prompt.is_some() {
        return Err("more than one positional prompt supplied".into());
    }
    *prompt = Some(text.to_string());
    Ok(())
}
