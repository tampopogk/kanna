//! Authenticated connection to a PTY frontend. Its endpoint survives daemon
//! handoff; the capability is retained privately, never in SessionInfo.
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use kanna_agent_protocol::hosted_frontend::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub const FRONTENDS_ENV: &str = "KANNA_AGENT_FRONTENDS";
type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct Frontend {
    pub config_path: String,
    pub config: Config,
    unavailable: Option<String>,
}

pub fn random_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

impl Frontend {
    pub fn prepare(session_id: &str, env: &mut HashMap<String, String>) -> Result<Self, Error> {
        let task_id = env
            .get("KANNA_TASK_ID")
            .ok_or("hosted frontend needs a task id")?
            .clone();
        let run_id = env
            .get("KANNA_STAGE_RUN_ID")
            .ok_or("hosted frontend needs a run id")?
            .clone();
        if !crate::session_id::is_safe(&task_id) || !crate::session_id::is_safe(&run_id) {
            return Err("invalid hosted task/run binding".into());
        }
        let incarnation = random_id()?;
        // AF_UNIX paths on macOS are limited to 104 bytes. This application
        // runtime directory is deliberately independent of worktree paths.
        let directory = kanna_runtime_defaults::socket_dir().join(format!("kh-{}", incarnation));
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let config_path = directory.join("config.json");
        let config = Config {
            version: VERSION,
            binding: Binding {
                task_id,
                run_id,
                session_id: session_id.into(),
                incarnation,
            },
            capability: format!("{}{}", random_id()?, random_id()?),
            socket_path: directory
                .join("control.sock")
                .to_string_lossy()
                .into_owned(),
            journal_path: directory
                .join("receipts.json")
                .to_string_lossy()
                .into_owned(),
            initial_delivery_id: random_id()?,
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&config_path)?;
        file.write_all(&serde_json::to_vec(&config)?)?;
        file.sync_all()?;
        std::fs::File::open(&directory)?.sync_all()?;
        let config_path = config_path.to_string_lossy().into_owned();
        env.insert(CONFIG_ENV.into(), config_path.clone());
        Ok(Self {
            config_path,
            config,
            unavailable: None,
        })
    }

    pub fn for_binding(binding: &Binding) -> Result<Self, Error> {
        if binding.incarnation.len() != 36
            || !binding
                .incarnation
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Err("invalid frontend incarnation".into());
        }
        let path = kanna_runtime_defaults::socket_dir()
            .join(format!("kh-{}", binding.incarnation))
            .join("config.json");
        let frontend = Self::load(&path.to_string_lossy())?;
        if frontend.config.binding != *binding {
            return Err("frontend journal belongs to another binding".into());
        }
        Ok(frontend)
    }

    pub fn final_snapshot(&self) -> Snapshot {
        let mut snapshot = self.disconnected_snapshot("frontend process exited".into());
        snapshot.retired = true;
        for entry in &mut snapshot.deliveries {
            entry.state = match entry.state {
                DeliveryState::Queued => DeliveryState::Failed,
                DeliveryState::Submitting => DeliveryState::Uncertain,
                _ => continue,
            };
            entry.error = Some("frontend exited before confirmation".into());
        }
        snapshot.queued_count = 0;
        snapshot
    }

    pub fn load(path: &str) -> Result<Self, Error> {
        let path = Path::new(path);
        for target in [path, path.parent().ok_or("missing host directory")?] {
            let metadata = std::fs::symlink_metadata(target)?;
            if metadata.file_type().is_symlink()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
            {
                return Err("host runtime configuration is not private".into());
            }
        }
        if std::fs::metadata(path)?.len() > 16 * 1024 {
            return Err("host runtime configuration exceeds size limit".into());
        }
        let config: Config = serde_json::from_slice(&std::fs::read(path)?)?;
        if config.version != VERSION {
            return Err("unsupported hosted frontend protocol".into());
        }
        if config.capability.len() < 64
            || Path::new(&config.socket_path).parent() != path.parent()
            || Path::new(&config.journal_path).parent() != path.parent()
        {
            return Err("invalid private hosted endpoint configuration".into());
        }
        Ok(Self {
            config_path: path.to_string_lossy().into_owned(),
            config,
            unavailable: None,
        })
    }

    pub fn unavailable(
        config_path: &str,
        session_id: &str,
        binding: Option<&crate::protocol::TerminalAttemptBinding>,
        reason: String,
    ) -> Self {
        Self {
            config_path: config_path.into(),
            unavailable: Some(reason),
            config: Config {
                version: VERSION,
                binding: Binding {
                    task_id: binding.map(|b| b.task_id.clone()).unwrap_or_default(),
                    run_id: binding
                        .map(|b| b.spawned_run_id.clone())
                        .unwrap_or_default(),
                    session_id: session_id.into(),
                    incarnation: String::new(),
                },
                capability: String::new(),
                socket_path: String::new(),
                journal_path: String::new(),
                initial_delivery_id: String::new(),
            },
        }
    }

    pub async fn request(&self, command: Command) -> Result<Response, Error> {
        if let Some(reason) = &self.unavailable {
            return Err(reason.clone().into());
        }
        let request = Request {
            version: VERSION,
            binding: self.config.binding.clone(),
            capability: self.config.capability.clone(),
            command,
        };
        let timeout = if matches!(request.command, Command::Inspect) {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(3)
        };
        let result = tokio::time::timeout(timeout, async {
            let mut stream = UnixStream::connect(&self.config.socket_path).await?;
            if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
                return Err::<_, Error>("host peer uid mismatch".into());
            }
            let mut bytes = serde_json::to_vec(&request)?;
            if bytes.len() >= MAX_REQUEST_BYTES {
                return Err("host request too large".into());
            }
            bytes.push(b'\n');
            stream.write_all(&bytes).await?;
            bytes.clear();
            BufReader::new(stream)
                .take((MAX_RESPONSE_BYTES + 1) as u64)
                .read_until(b'\n', &mut bytes)
                .await?;
            if bytes.len() > MAX_RESPONSE_BYTES || bytes.last() != Some(&b'\n') {
                return Err("invalid host response size".into());
            }
            Ok(serde_json::from_slice::<Response>(&bytes)?)
        })
        .await??;
        if let Response::Snapshot { snapshot } = &result {
            if snapshot.version != VERSION
                || snapshot.binding != self.config.binding
                || snapshot.frontend_pid == 0
            {
                return Err("host response binding mismatch".into());
            }
        }
        Ok(result)
    }

    pub fn journal(&self) -> Result<Snapshot, Error> {
        let mut bytes = Vec::new();
        std::fs::File::open(&self.config.journal_path)?
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err("host journal too large".into());
        }
        let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
        if snapshot.version != VERSION || snapshot.binding != self.config.binding {
            return Err("host journal binding mismatch".into());
        }
        Ok(snapshot)
    }

    pub fn disconnected_snapshot(&self, reason: String) -> Snapshot {
        let mut snapshot = self.journal().unwrap_or_else(|_| Snapshot {
            notice: None,
            version: VERSION,
            active_run_id: self.config.binding.run_id.clone(),
            binding: self.config.binding.clone(),
            frontend_pid: 0,
            sequence: 0,
            provider_session_id: None,
            state: RuntimeState::Unavailable,
            diagnostic: None,
            composer_text: String::new(),
            queued_count: 0,
            deliveries: Vec::new(),
            retired: false,
        });
        snapshot.state = RuntimeState::Unavailable;
        snapshot.diagnostic = Some(reason);
        snapshot
    }

    pub fn remove_socket(&self) {
        let _ = std::fs::remove_file(PathBuf::from(&self.config.socket_path));
    }
}
