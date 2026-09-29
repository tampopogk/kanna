//! The frontend owns its socket and durable receipt journal, so daemon adoption
//! does not disconnect it or replay input. Only the UI event loop dispatches.
use std::io::{self, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use kanna_agent_protocol::hosted_frontend::*;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::app::{App, Phase, Status};

pub struct Incoming {
    command: Command,
    reply: oneshot::Sender<Response>,
}

pub struct Host {
    config: Config,
    registered: bool,
    pub snapshot: Snapshot,
    incoming: mpsc::Receiver<Incoming>,
    listener: JoinHandle<()>,
}

pub fn payload_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn private_path(path: &Path, directory: bool) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || metadata.is_dir() != directory
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        bail!(
            "host runtime path must be private and owned by the current user: {}",
            path.display()
        );
    }
    Ok(())
}

impl Host {
    pub fn open(path: &Path, initial_prompt: Option<String>) -> Result<Self> {
        private_path(path, false)?;
        let directory = path
            .parent()
            .context("host config needs a private directory")?;
        private_path(directory, true)?;
        if std::fs::metadata(path)?.len() > 16 * 1024 {
            bail!("host configuration exceeds size limit");
        }
        let config: Config = serde_json::from_slice(&std::fs::read(path)?)?;
        if config.version != VERSION {
            bail!(
                "unsupported Kanna frontend protocol {}; expected {VERSION}",
                config.version
            );
        }
        if config.capability.len() < 64 || config.binding.incarnation.is_empty() {
            bail!("invalid Kanna host capability or incarnation");
        }
        for resource in [&config.socket_path, &config.journal_path] {
            let resource = Path::new(resource);
            if resource.parent() != Some(directory) || resource.exists() {
                bail!("host resources must be new files in the private runtime directory");
            }
        }
        let listener = UnixListener::bind(&config.socket_path)?;
        std::fs::set_permissions(&config.socket_path, std::fs::Permissions::from_mode(0o600))?;
        let (sender, incoming) = mpsc::channel(16);
        let credentials = config.clone();
        let listener = tokio::spawn(async move {
            let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
            loop {
                let Ok(permit) = slots.clone().acquire_owned().await else {
                    break;
                };
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let sender = sender.clone();
                let credentials = credentials.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let operation = async {
                        if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
                            return Err(io::Error::new(
                                io::ErrorKind::PermissionDenied,
                                "wrong peer uid",
                            ));
                        }
                        let (read, mut write) = stream.into_split();
                        let mut bytes = Vec::new();
                        BufReader::new(read)
                            .take((MAX_REQUEST_BYTES + 1) as u64)
                            .read_until(b'\n', &mut bytes)
                            .await?;
                        if bytes.len() > MAX_REQUEST_BYTES || bytes.last() != Some(&b'\n') {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "oversized or incomplete request",
                            ));
                        }
                        let request: Request = serde_json::from_slice(&bytes)?;
                        let valid_token = request.capability.len() == credentials.capability.len()
                            && request
                                .capability
                                .bytes()
                                .zip(credentials.capability.bytes())
                                .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                                == 0;
                        let response = if request.version != VERSION
                            || request.binding != credentials.binding
                            || !valid_token
                        {
                            Response::Rejected {
                                reason: "host protocol, binding or capability mismatch".into(),
                            }
                        } else {
                            let (reply, received) = oneshot::channel();
                            sender
                                .send(Incoming {
                                    command: request.command,
                                    reply,
                                })
                                .await
                                .map_err(|_| {
                                    io::Error::new(io::ErrorKind::BrokenPipe, "frontend stopped")
                                })?;
                            received.await.map_err(|_| {
                                io::Error::new(io::ErrorKind::BrokenPipe, "frontend stopped")
                            })?
                        };
                        let mut bytes = serde_json::to_vec(&response)?;
                        bytes.push(b'\n');
                        write.write_all(&bytes).await
                    };
                    let _ =
                        tokio::time::timeout(std::time::Duration::from_secs(5), operation).await;
                });
            }
        });
        let mut host = Self {
            snapshot: Snapshot {
                notice: None,
                version: VERSION,
                active_run_id: config.binding.run_id.clone(),
                binding: config.binding.clone(),
                frontend_pid: std::process::id(),
                sequence: 0,
                provider_session_id: None,
                state: RuntimeState::Busy,
                diagnostic: None,
                composer_text: String::new(),
                queued_count: 0,
                deliveries: Vec::new(),
                retired: false,
            },
            config,
            incoming,
            listener,
            registered: false,
        };
        if let Some(text) = initial_prompt.filter(|text| !text.is_empty()) {
            host.submit(host.config.initial_delivery_id.clone(), text)?;
            host.snapshot.deliveries[0].initial_prompt = true;
            host.persist()?;
        } else {
            host.persist()?;
        }
        Ok(host)
    }

    pub async fn recv(&mut self) -> Option<Incoming> {
        self.incoming.recv().await
    }

    pub fn handle(&mut self, incoming: Incoming) -> Result<()> {
        let response = match incoming.command {
            Command::Inspect => {
                self.registered = true;
                Response::Snapshot {
                    snapshot: self.snapshot.clone(),
                }
            }
            Command::Submit {
                delivery_id,
                text,
                workflow_prompt,
                run_id,
            } => match self
                .submit_bound(delivery_id, text, workflow_prompt, run_id)
                .and_then(|mut delivery| {
                    if workflow_prompt && !delivery.initial_prompt {
                        delivery.initial_prompt = true;
                        if let Some(entry) = self
                            .snapshot
                            .deliveries
                            .iter_mut()
                            .find(|entry| entry.delivery_id == delivery.delivery_id)
                        {
                            entry.initial_prompt = true;
                        }
                        self.persist()?;
                    }
                    Ok(delivery)
                }) {
                Ok(delivery) => Response::Accepted { delivery },
                Err(error) if error.downcast_ref::<io::Error>().is_some() => return Err(error),
                Err(error) => Response::Rejected {
                    reason: error.to_string(),
                },
            },
            Command::Retire { reason } => {
                self.retire(&reason)?;
                Response::Snapshot {
                    snapshot: self.snapshot.clone(),
                }
            }
        };
        // A disconnected caller can reconcile by the same id; the durable
        // acceptance is not undone merely because this response was lost.
        let _ = incoming.reply.send(response);
        Ok(())
    }

    fn submit_bound(
        &mut self,
        delivery_id: String,
        text: String,
        workflow_prompt: bool,
        run_id: Option<String>,
    ) -> Result<Delivery> {
        let run_id = run_id.unwrap_or_else(|| self.snapshot.active_run_id.clone());
        if let Some(existing) = self
            .snapshot
            .deliveries
            .iter()
            .find(|entry| entry.delivery_id == delivery_id)
        {
            if existing.run_id != run_id {
                bail!("delivery id belongs to a different run");
            }
        }
        if run_id != self.snapshot.active_run_id {
            if !workflow_prompt || run_id.is_empty() {
                bail!("input targets a stale run");
            }
            // A post reuses the conversation, not the previous run's queue.
            // The active turn finishes normally; only undispatched old input
            // is failed before the new run's prompt is accepted.
            for entry in &mut self.snapshot.deliveries {
                if entry.state == DeliveryState::Queued {
                    entry.state = DeliveryState::Failed;
                    entry.error = Some("run changed before this input could be dispatched".into());
                }
            }
            self.snapshot.active_run_id = run_id;
            self.persist()?;
        }
        self.submit(delivery_id, text)
    }

    fn submit(&mut self, delivery_id: String, text: String) -> Result<Delivery> {
        if text.is_empty()
            || text.len() > MAX_TEXT_BYTES
            || delivery_id.is_empty()
            || delivery_id.len() > 128
        {
            bail!("invalid delivery id or input size");
        }
        let hash = payload_hash(&text);
        if let Some(existing) = self
            .snapshot
            .deliveries
            .iter()
            .find(|entry| entry.delivery_id == delivery_id)
        {
            if existing.payload_hash != hash {
                bail!("delivery id was already used with a different payload");
            }
            return Ok(existing.clone());
        }
        if self.snapshot.retired {
            bail!("frontend incarnation is retired");
        }
        if self.snapshot.deliveries.len() >= MAX_RECEIPTS {
            bail!("frontend receipt limit reached; start a new Kanna run");
        }
        let pending = self
            .snapshot
            .deliveries
            .iter()
            .filter(|entry| entry.state.pending())
            .count();
        let retained: usize = self
            .snapshot
            .deliveries
            .iter()
            .filter_map(|entry| entry.text.as_ref())
            .map(|text| serde_json::to_string(text).map_or(MAX_PENDING_BYTES, |json| json.len()))
            .sum();
        if pending >= MAX_PENDING
            || retained + serde_json::to_string(&text)?.len() > MAX_PENDING_BYTES
        {
            bail!("frontend input queue is full; input was not accepted");
        }
        let delivery = Delivery {
            run_id: self.snapshot.active_run_id.clone(),
            initial_prompt: false,
            delivery_id,
            sequence: self
                .snapshot
                .deliveries
                .last()
                .map_or(1, |entry| entry.sequence + 1),
            payload_hash: hash,
            text: Some(text),
            state: DeliveryState::Queued,
            error: None,
        };
        self.snapshot.deliveries.push(delivery.clone());
        if self.snapshot.state == RuntimeState::Idle {
            self.snapshot.state = RuntimeState::Busy;
        }
        if let Err(error) = self.persist() {
            self.snapshot.deliveries.pop();
            return Err(error);
        }
        Ok(delivery)
    }

    /// Called before every outbox flush. The journal is synced before a
    /// provider write, including the initial prompt. No subprocess can dispatch.
    pub fn advance(&mut self, app: &mut App) -> Result<()> {
        let before = self.snapshot.clone();
        for (id, receipt) in std::mem::take(&mut app.input_receipts) {
            if let Some(entry) = self
                .snapshot
                .deliveries
                .iter_mut()
                .find(|entry| entry.delivery_id == id)
            {
                if matches!(
                    entry.state,
                    DeliveryState::Submitting | DeliveryState::Uncertain
                ) {
                    match receipt {
                        Ok(()) => {
                            entry.state = DeliveryState::Submitted;
                            if !entry.initial_prompt {
                                entry.text = None;
                            }
                            entry.error = None;
                        }
                        Err(reason) => {
                            entry.state = DeliveryState::Failed;
                            entry.error = Some(reason);
                        }
                    }
                }
            }
        }
        self.snapshot.notice = app.provider_notice.clone();
        self.snapshot.provider_session_id = app.meta.session_id.clone();
        self.snapshot.composer_text = app.composer.text().to_string();
        self.snapshot.state = match app.status() {
            Status::NeedsApproval | Status::NeedsInput => RuntimeState::Waiting,
            Status::Ready => RuntimeState::Idle,
            Status::Disconnected | Status::Degraded => RuntimeState::Unavailable,
            _ => RuntimeState::Busy,
        };
        self.snapshot.diagnostic = match &app.phase {
            Phase::Disconnected(reason) => Some(reason.clone()),
            _ => app.degraded.clone(),
        };
        if matches!(app.phase, Phase::Disconnected(_)) {
            self.fail_pending("harness disconnected");
            self.snapshot.retired = true;
        } else if app.phase == Phase::Ready {
            // Completion without a correlated acceptance is not proof that the
            // provider rejected it. Preserve uncertainty and never resend.
            for entry in &mut self.snapshot.deliveries {
                if entry.state == DeliveryState::Submitting {
                    entry.state = DeliveryState::Uncertain;
                    entry.error = Some("turn ended without correlated provider acceptance".into());
                }
            }
        }
        if self.snapshot.state == RuntimeState::Idle
            && self
                .snapshot
                .deliveries
                .iter()
                .any(|entry| entry.state == DeliveryState::Queued)
        {
            self.snapshot.state = if app.provider_notice.is_some() {
                RuntimeState::Unavailable
            } else {
                RuntimeState::Busy
            };
        }
        if self.snapshot != before {
            self.persist()?;
        }
        if self.registered
            && !self.snapshot.retired
            && app.status() == Status::Ready
            && app.provider_notice.is_none()
        {
            if let Some(index) = self
                .snapshot
                .deliveries
                .iter()
                .position(|entry| entry.state == DeliveryState::Queued)
            {
                self.snapshot.deliveries[index].state = DeliveryState::Submitting;
                self.persist()?;
                let entry = &self.snapshot.deliveries[index];
                match app.send_logical_prompt(
                    entry.text.clone().expect("queued text"),
                    &entry.delivery_id,
                ) {
                    Ok(()) => self.snapshot.state = RuntimeState::Busy,
                    Err(reason) => {
                        self.snapshot.deliveries[index].state = DeliveryState::Failed;
                        self.snapshot.deliveries[index].error = Some(reason);
                    }
                }
                self.persist()?;
            }
        }
        app.queued_count = self
            .snapshot
            .deliveries
            .iter()
            .filter(|entry| entry.state == DeliveryState::Queued)
            .count();
        Ok(())
    }

    fn fail_pending(&mut self, reason: &str) {
        for entry in &mut self.snapshot.deliveries {
            entry.state = match entry.state {
                DeliveryState::Queued => DeliveryState::Failed,
                DeliveryState::Submitting => DeliveryState::Uncertain,
                _ => continue,
            };
            entry.error = Some(reason.to_string());
        }
    }

    pub fn retire(&mut self, reason: &str) -> Result<()> {
        self.snapshot.retired = true;
        self.snapshot.state = RuntimeState::Unavailable;
        self.snapshot.diagnostic = Some(reason.into());
        self.fail_pending(reason);
        self.persist()
    }

    fn persist(&mut self) -> Result<()> {
        self.snapshot.queued_count = self
            .snapshot
            .deliveries
            .iter()
            .filter(|entry| entry.state == DeliveryState::Queued)
            .count();
        self.snapshot.sequence += 1;
        let path = Path::new(&self.config.journal_path);
        let temporary = path.with_extension("next");
        let bytes = serde_json::to_vec(&self.snapshot)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(path.parent().context("journal directory")?)?.sync_all()?;
        Ok(())
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.listener.abort();
        let _ = std::fs::remove_file(PathBuf::from(&self.config.socket_path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::HostedLaunch;
    use crate::protocol::HarnessKind;
    use crate::transport::Record;
    use crate::ui::skins::SkinId;
    use serde_json::json;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap();
            let path = root
                .join(".tmp")
                .join(format!("h{:x}", rand::random::<u32>()));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn app() -> App {
        let launch =
            HostedLaunch::parse(HarnessKind::Codex, "codex".into(), "/fixture".into(), &[])
                .unwrap();
        let mut app = App::new(launch.adapter(), SkinId::Graphite);
        app.hosted = true;
        app.start();
        app.on_record(record(json!({"id":1,"result":{}})));
        app.on_record(record(
            json!({"id":2,"result":{"thread":{"id":"thread-1"}}}),
        ));
        app.take_outbox();
        app
    }
    fn record(value: serde_json::Value) -> Record {
        let mut framer = crate::transport::JsonlFramer::new();
        framer.push(format!("{value}\n").as_bytes()).remove(0)
    }

    // Construct the queue without binding a socket; network tests exercise
    // Host::open separately with short application-managed runtime paths.
    fn host(fixture: &Fixture) -> Host {
        let config = Config {
            version: VERSION,
            binding: Binding {
                task_id: "task".into(),
                run_id: "run".into(),
                session_id: "task".into(),
                incarnation: "generation".into(),
            },
            capability: "a".repeat(64),
            socket_path: fixture.0.join("socket").to_string_lossy().into_owned(),
            journal_path: fixture
                .0
                .join("journal.json")
                .to_string_lossy()
                .into_owned(),
            initial_delivery_id: "initial".into(),
        };
        let (_, incoming) = mpsc::channel(1);
        Host {
            snapshot: Snapshot {
                notice: None,
                version: VERSION,
                active_run_id: config.binding.run_id.clone(),
                binding: config.binding.clone(),
                frontend_pid: std::process::id(),
                sequence: 0,
                provider_session_id: None,
                state: RuntimeState::Busy,
                diagnostic: None,
                composer_text: String::new(),
                queued_count: 0,
                deliveries: Vec::new(),
                retired: false,
            },
            config,
            incoming,
            registered: true,
            listener: tokio::spawn(async {}),
        }
    }

    #[tokio::test]
    async fn fifo_dispatch_is_durable_and_preserves_draft() {
        let fixture = Fixture::new();
        let mut host = host(&fixture);
        let mut app = app();
        app.composer.set("human draft\nwith cursor");
        host.submit("first".into(), "first\nmessage".into())
            .unwrap();
        host.submit("second".into(), "/new is literal logical text".into())
            .unwrap();
        host.advance(&mut app).unwrap();
        assert_eq!(app.composer.text(), "human draft\nwith cursor");
        assert_eq!(app.phase, Phase::Working);
        assert_eq!(app.queued_count, 1);
        let persisted: Snapshot =
            serde_json::from_slice(&std::fs::read(&host.config.journal_path).unwrap()).unwrap();
        assert_eq!(persisted.deliveries[0].state, DeliveryState::Submitting);
        assert_eq!(app.take_outbox().len(), 1);
        host.advance(&mut app).unwrap();
        assert!(app.take_outbox().is_empty());
        app.on_record(record(json!({"id":3,"result":{"turn":{"id":"turn-1"}}})));
        app.on_record(record(json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}})));
        host.advance(&mut app).unwrap();
        assert_eq!(host.snapshot.deliveries[0].state, DeliveryState::Submitted);
        assert_eq!(host.snapshot.deliveries[1].state, DeliveryState::Submitting);
        assert!(!app.restart_requested);
        assert_eq!(app.composer.text(), "human draft\nwith cursor");
        assert_eq!(app.take_outbox().len(), 1);
    }

    #[tokio::test]
    async fn retries_bounds_and_retirement_do_not_replay() {
        let fixture = Fixture::new();
        let mut host = host(&fixture);
        let first = host.submit("id".into(), "text".into()).unwrap();
        assert_eq!(host.submit("id".into(), "text".into()).unwrap(), first);
        assert!(host.submit("id".into(), "changed".into()).is_err());
        for n in 1..MAX_PENDING {
            host.submit(n.to_string(), "text".into()).unwrap();
        }
        assert!(host.submit("overflow".into(), "text".into()).is_err());
        let mut app = app();
        host.advance(&mut app).unwrap();
        host.retire("new run").unwrap();
        assert_eq!(host.snapshot.deliveries[0].state, DeliveryState::Uncertain);
        assert!(host.snapshot.deliveries[1..]
            .iter()
            .all(|entry| entry.state == DeliveryState::Failed));
        assert!(host.submit("late".into(), "text".into()).is_err());
        assert_eq!(
            host.submit("id".into(), "text".into()).unwrap().state,
            DeliveryState::Uncertain
        );
    }

    #[tokio::test]
    async fn pending_approval_blocks_queue_without_answering_card() {
        let fixture = Fixture::new();
        let mut host = host(&fixture);
        let mut app = app();
        app.phase = Phase::Working;
        app.on_record(record(json!({"id":77,"method":"item/commandExecution/requestApproval", "params":{"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","command":"dangerous command","cwd":"/fixture"}})));
        assert_eq!(app.status(), Status::NeedsApproval);
        host.submit("queued".into(), "approve".into()).unwrap();
        host.advance(&mut app).unwrap();
        assert_eq!(host.snapshot.state, RuntimeState::Waiting);
        assert_eq!(host.snapshot.deliveries[0].state, DeliveryState::Queued);
        assert!(app.take_outbox().is_empty());
        assert_eq!(app.transcript.pending_cards().count(), 1);
    }

    #[tokio::test]
    async fn missing_provider_ack_is_uncertain_after_completion() {
        let fixture = Fixture::new();
        let mut host = host(&fixture);
        let mut app = app();
        host.submit("id".into(), "text".into()).unwrap();
        host.advance(&mut app).unwrap();
        app.take_outbox();
        app.phase = Phase::Ready;
        host.advance(&mut app).unwrap();
        assert_eq!(host.snapshot.deliveries[0].state, DeliveryState::Uncertain);
        assert_eq!(host.snapshot.deliveries[0].text.as_deref(), Some("text"));
        assert!(app.take_outbox().is_empty());
    }
    async fn exchange(config: Config, request: Request) -> Response {
        let mut stream = tokio::net::UnixStream::connect(&config.socket_path)
            .await
            .unwrap();
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        stream.write_all(&bytes).await.unwrap();
        let mut bytes = Vec::new();
        BufReader::new(stream)
            .read_until(b'\n', &mut bytes)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn socket_authentication_and_lost_response_reconcile_by_id() {
        let fixture = Fixture::new();
        let temporary = host(&fixture);
        let config = temporary.config.clone();
        drop(temporary);
        let path = fixture.0.join("config.json");
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(&serde_json::to_vec(&config).unwrap())
            .unwrap();
        let mut host = Host::open(&path, None).unwrap();
        let request = Request {
            version: VERSION,
            binding: config.binding.clone(),
            capability: config.capability.clone(),
            command: Command::Inspect,
        };
        {
            let mut invalid = request.clone();
            // Each mismatch is rejected before the UI event loop sees it.
            invalid.capability = "bad".into();
            assert!(matches!(
                exchange(config.clone(), invalid).await,
                Response::Rejected { .. }
            ));
        }
        let mut stale = request.clone();
        stale.binding.incarnation = "old".into();
        assert!(matches!(
            exchange(config.clone(), stale).await,
            Response::Rejected { .. }
        ));
        let mut future = request.clone();
        future.version += 1;
        assert!(matches!(
            exchange(config.clone(), future).await,
            Response::Rejected { .. }
        ));
        let mut submit = request.clone();
        submit.command = Command::Submit {
            delivery_id: "lost-reply".into(),
            text: "one message".into(),
            workflow_prompt: false,
            run_id: Some("run".into()),
        };
        let mut stream = tokio::net::UnixStream::connect(&config.socket_path)
            .await
            .unwrap();
        stream
            .write_all(format!("{}\n", serde_json::to_string(&submit).unwrap()).as_bytes())
            .await
            .unwrap();
        let incoming = tokio::time::timeout(std::time::Duration::from_secs(1), host.recv())
            .await
            .unwrap()
            .unwrap();
        drop(stream);
        host.handle(incoming).unwrap();
        let call = tokio::spawn(exchange(config.clone(), submit));
        let incoming = host.recv().await.unwrap();
        host.handle(incoming).unwrap();
        assert!(matches!(call.await.unwrap(), Response::Accepted { .. }));
        assert_eq!(host.snapshot.deliveries.len(), 1);
        assert_eq!(host.snapshot.deliveries[0].state, DeliveryState::Queued);
        drop(host);
        assert!(!Path::new(&config.socket_path).exists());
        assert!(Path::new(&config.journal_path).exists());
    }

    #[tokio::test]
    async fn post_run_cancels_old_queue_and_waits_for_active_turn() {
        let fixture = Fixture::new();
        let mut host = host(&fixture);
        let mut app = app();
        app.phase = Phase::Working;
        host.submit("old".into(), "old operator input".into())
            .unwrap();
        host.submit_bound(
            "post".into(),
            "post prompt".into(),
            true,
            Some("run-post".into()),
        )
        .unwrap();
        host.advance(&mut app).unwrap();
        assert_eq!(host.snapshot.deliveries[0].state, DeliveryState::Failed);
        assert_eq!(host.snapshot.deliveries[1].state, DeliveryState::Queued);
        assert!(app.take_outbox().is_empty());
        assert!(host
            .submit_bound("stale".into(), "late".into(), false, Some("run".into()))
            .is_err());
        app.phase = Phase::Ready;
        host.advance(&mut app).unwrap();
        assert_eq!(host.snapshot.deliveries[1].run_id, "run-post");
        assert_eq!(app.take_outbox().len(), 1);
    }
}
