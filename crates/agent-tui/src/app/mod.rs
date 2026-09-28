//! UI-free application state: session state machine, transcript, composer,
//! focus and key handling. The event loop feeds it records, keys and process
//! events, and drains its outbox to the harness's stdin.

pub mod composer;
pub mod slash;
pub mod transcript;

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::Value;

use crate::protocol::*;
use crate::raw::{Dir, RawId, RawLog};
use crate::transport::Record;
use crate::ui::skins::{SkinId, ALL_SKINS};
use composer::Composer;
use slash::{CommandSource, CommandSpec, SlashMenu};
use transcript::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Ready,
    Working,
    Stopping,
    Disconnected(String),
}

/// What the header shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Starting,
    Ready,
    Working,
    NeedsApproval,
    NeedsInput,
    Stopping,
    Disconnected,
    Degraded,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Status::Starting => "Starting",
            Status::Ready => "Ready",
            Status::Working => "Working",
            Status::NeedsApproval => "Needs approval",
            Status::NeedsInput => "Needs input",
            Status::Stopping => "Stopping",
            Status::Disconnected => "Disconnected",
            Status::Degraded => "Degraded",
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Status::Starting => "◌",
            Status::Ready => "✓",
            Status::Working => "●",
            Status::NeedsApproval => "!",
            Status::NeedsInput => "?",
            Status::Stopping => "■",
            Status::Disconnected => "✕",
            Status::Degraded => "⚠",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Entry(EntryId),
    Call(EntryId, usize),
    RawIn(EntryId, usize),
    RawOut(EntryId, usize),
}

impl Target {
    pub fn entry(self) -> EntryId {
        match self {
            Target::Entry(e) | Target::Call(e, _) | Target::RawIn(e, _) | Target::RawOut(e, _) => e,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Composer,
    Transcript,
    Card(EntryId),
    Search,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    None,
    Help {
        selected: usize,
    },
    Inspector {
        title: String,
        records: Vec<RawId>,
        scroll: usize,
    },
    SkinPicker {
        selected: usize,
    },
    QuitConfirm {
        selected: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub text: String,
    pub level: NoticeLevel,
}

#[derive(Debug, Clone, Default)]
pub struct ViewState {
    pub follow: bool,
    /// First visible transcript line while paused.
    pub top: usize,
    pub new_count: usize,
    pub selected: Option<Target>,
    pub ensure_visible: bool,
    /// Filled in by the renderer.
    pub height: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Default)]
pub struct SearchState {
    pub query: String,
    pub matches: Vec<Target>,
    pub current: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub model: Option<MetaValue>,
    pub effort: Option<MetaValue>,
    pub session_id: Option<String>,
}

pub struct App {
    pub adapter: Box<dyn Adapter>,
    pub harness: HarnessKind,
    pub caps: Capabilities,
    pub raw: RawLog,
    pub transcript: Transcript,
    pub composer: Composer,
    pub slash: SlashMenu,
    pub search: Option<SearchState>,
    pub focus: Focus,
    pub overlay: Overlay,
    pub phase: Phase,
    pub degraded: Option<String>,
    pub meta: Meta,
    pub commands: Vec<HarnessCommand>,
    pub hint: Option<Hint>,
    pub view: ViewState,
    pub skin: SkinId,
    pub quote: usize,
    pub now: Instant,
    pub turn_started: Option<Instant>,
    /// Whether the terminal reports Shift+Enter (kitty keyboard protocol).
    pub shift_enter: bool,
    pub cwd_label: String,
    pub should_quit: bool,
    pub restart_requested: bool,
    outbox: Vec<String>,
    pub input_receipts: Vec<(String, Result<(), String>)>,
}

impl App {
    pub fn new(adapter: Box<dyn Adapter>, skin: SkinId) -> Self {
        let harness = adapter.kind();
        let caps = adapter.capabilities();
        let mut app = Self {
            adapter,
            harness,
            caps,
            raw: RawLog::default(),
            transcript: Transcript::new(),
            composer: Composer::default(),
            slash: SlashMenu::default(),
            search: None,
            focus: Focus::Composer,
            overlay: Overlay::None,
            phase: Phase::Starting,
            degraded: None,
            meta: Meta::default(),
            commands: Vec::new(),
            hint: None,
            view: ViewState {
                follow: true,
                ..Default::default()
            },
            skin,
            quote: 0,
            now: Instant::now(),
            turn_started: None,
            shift_enter: false,
            cwd_label: String::new(),
            should_quit: false,
            restart_requested: false,
            outbox: Vec::new(),
            input_receipts: Vec::new(),
        };
        app.pick_quote();
        app
    }

    // ----- session plumbing -------------------------------------------------

    /// Records to write right after the harness starts.
    pub fn start(&mut self) {
        let msgs = self.adapter.start();
        self.send_all(msgs);
    }

    /// Replaces the adapter for `/new`; the transcript and draft stay.
    pub fn reset_session(&mut self, adapter: Box<dyn Adapter>) {
        // Whatever the old session left running or pending ended with it.
        self.finish_turn_locally(ToolStatus::Stopped);
        self.adapter = adapter;
        self.phase = Phase::Starting;
        self.degraded = None;
        self.meta = Meta::default();
        self.commands.clear();
        self.turn_started = None;
        self.transcript.turn_tools = None;
        self.restart_requested = false;
        self.push_entry(
            EntryKind::Notice {
                level: NoticeLevel::Info,
                text: "── New session ──".into(),
            },
            vec![],
        );
        self.hint = None;
    }

    pub fn take_outbox(&mut self) -> Vec<String> {
        std::mem::take(&mut self.outbox)
    }

    fn send_all(&mut self, msgs: Vec<Value>) -> Vec<RawId> {
        msgs.into_iter()
            .map(|m| {
                let text = serde_json::to_string(&m).unwrap_or_default();
                let id = self.raw.push(Dir::Out, None, &text, false);
                self.outbox.push(text);
                id
            })
            .collect()
    }

    pub fn tick(&mut self, now: Instant) {
        self.now = now;
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.turn_started
            .map(|t| self.now.saturating_duration_since(t))
    }

    pub fn status(&self) -> Status {
        match &self.phase {
            Phase::Disconnected(_) => Status::Disconnected,
            _ if self.degraded.is_some() => Status::Degraded,
            Phase::Starting => Status::Starting,
            Phase::Stopping => Status::Stopping,
            Phase::Ready | Phase::Working => {
                let mut approval = false;
                let mut question = false;
                for e in self.transcript.pending_cards() {
                    match e.kind {
                        EntryKind::Question(_) => question = true,
                        _ => approval = true,
                    }
                }
                if question {
                    Status::NeedsInput
                } else if approval {
                    Status::NeedsApproval
                } else if self.phase == Phase::Working {
                    Status::Working
                } else {
                    Status::Ready
                }
            }
        }
    }

    pub fn turn_running(&self) -> bool {
        matches!(self.phase, Phase::Working | Phase::Stopping)
    }

    fn push_entry(&mut self, kind: EntryKind, raw: Vec<RawId>) -> EntryId {
        let id = self.transcript.push(kind, raw);
        if !self.view.follow {
            self.view.new_count += 1;
        }
        id
    }

    fn notice(&mut self, level: NoticeLevel, text: impl Into<String>, raw: Vec<RawId>) {
        self.push_entry(
            EntryKind::Notice {
                level,
                text: text.into(),
            },
            raw,
        );
    }

    fn set_hint(&mut self, level: NoticeLevel, text: impl Into<String>) {
        self.hint = Some(Hint {
            text: text.into(),
            level,
        });
    }

    // ----- incoming ---------------------------------------------------------

    pub fn on_record(&mut self, rec: Record) {
        match rec.parsed {
            Ok(v) => {
                let raw = self.raw.push(Dir::In, Some(rec.line_no), &rec.raw, false);
                let out = self.adapter.on_record(&v);
                for e in out.events {
                    self.apply(e, raw);
                }
                let replies = self.send_all(out.outgoing);
                if let Some(e) = self
                    .transcript
                    .last_id()
                    .and_then(|id| self.transcript.get_mut(id))
                {
                    if e.raw.last() == Some(&raw) {
                        e.raw.extend(replies);
                    }
                }
            }
            Err(err) => {
                let raw = self.raw.push(Dir::In, Some(rec.line_no), &rec.raw, true);
                for e in self.adapter.on_malformed(&rec.raw) {
                    let e = match e {
                        AgentEvent::Diagnostic { text } => AgentEvent::Diagnostic {
                            text: format!("{text} (line {}: {err})", rec.line_no),
                        },
                        other => other,
                    };
                    self.apply(e, raw);
                }
            }
        }
    }

    /// stdout closed or the process exited.
    pub fn on_disconnect(&mut self, reason: &str, stderr_tail: &[String]) {
        if let Phase::Disconnected(_) = self.phase {
            return;
        }
        self.finish_turn_locally(ToolStatus::Stopped);
        self.phase = Phase::Disconnected(reason.to_string());
        let mut text = format!("Disconnected — {reason}. Transcript and draft are kept; nothing was resent. /new starts a new session.");
        let tail: Vec<&String> = stderr_tail.iter().rev().take(6).collect();
        if !tail.is_empty() {
            text.push_str("\nstderr:");
            for l in tail.into_iter().rev() {
                text.push_str("\n  ");
                text.push_str(l);
            }
        }
        self.notice(NoticeLevel::Error, text, vec![]);
    }

    pub fn on_spawn_error(&mut self, program: &str, err: &str) {
        self.phase = Phase::Disconnected(format!("could not start {program}"));
        self.notice(
            NoticeLevel::Error,
            format!(
                "Could not start `{program}`: {err}. Is it installed and on PATH? /new retries."
            ),
            vec![],
        );
    }

    /// Marks running tools and pending cards as ended without a harness signal.
    fn finish_turn_locally(&mut self, tool_status: ToolStatus) {
        if let Some(gid) = self.transcript.turn_tools {
            if let Some(e) = self.transcript.get_mut(gid) {
                if let EntryKind::Tools(g) = &mut e.kind {
                    for c in g
                        .calls
                        .iter_mut()
                        .filter(|c| c.status == ToolStatus::Running)
                    {
                        c.status = tool_status;
                    }
                }
            }
        }
        let pending: Vec<EntryId> = self.transcript.pending_cards().map(|e| e.id).collect();
        for id in pending {
            if let Some(e) = self.transcript.get_mut(id) {
                match &mut e.kind {
                    EntryKind::Approval(c) => c.state = CardState::Resolved,
                    EntryKind::Question(c) => c.state = CardState::Resolved,
                    _ => {}
                }
            }
        }
        if let Focus::Card(_) = self.focus {
            self.focus = Focus::Composer;
        }
        self.turn_started = None;
        self.transcript.turn_tools = None;
    }

    pub fn apply(&mut self, event: AgentEvent, raw: RawId) {
        match event {
            AgentEvent::InputAccepted { delivery_id } => {
                self.input_receipts.push((delivery_id, Ok(())))
            }
            AgentEvent::InputRejected {
                delivery_id,
                reason,
            } => self.input_receipts.push((delivery_id, Err(reason))),
            AgentEvent::Ready { commands } => {
                self.commands = commands;
                if self.phase == Phase::Starting {
                    self.phase = Phase::Ready;
                }
                self.refresh_slash();
            }
            AgentEvent::StartupFailed { message } => {
                self.phase = Phase::Disconnected(message.clone());
                let who = self.harness.label();
                self.notice(
                    NoticeLevel::Error,
                    format!("{message}\nCheck that {who} is installed and signed in, then /new to retry."),
                    vec![raw],
                );
            }
            AgentEvent::SessionMeta {
                model,
                effort,
                session_id,
            } => {
                if let Some(m) = model {
                    // A reported value always wins over a requested one.
                    if m.source == MetaSource::Reported
                        || self
                            .meta
                            .model
                            .as_ref()
                            .is_none_or(|x| x.source == MetaSource::Requested)
                    {
                        self.meta.model = Some(m);
                    }
                }
                if let Some(e) = effort {
                    if e.source == MetaSource::Reported
                        || self
                            .meta
                            .effort
                            .as_ref()
                            .is_none_or(|x| x.source == MetaSource::Requested)
                    {
                        self.meta.effort = Some(e);
                    }
                }
                if session_id.is_some() {
                    self.meta.session_id = session_id;
                }
            }
            AgentEvent::TurnStarted => {}
            AgentEvent::AssistantDelta { item_id, text } => {
                match self.transcript.assistant_entry(&item_id) {
                    Some(id) => {
                        if let Some(e) = self.transcript.get_mut(id) {
                            e.raw.push(raw);
                            if let EntryKind::Assistant { text: t, .. } = &mut e.kind {
                                t.push_str(&text);
                            }
                        }
                    }
                    None => {
                        let was_follow = self.view.follow;
                        self.transcript.push_assistant(&item_id, text, false, raw);
                        if !was_follow {
                            self.view.new_count += 1;
                        }
                    }
                }
            }
            AgentEvent::AssistantDone { item_id, text } => {
                match self.transcript.assistant_entry(&item_id) {
                    Some(id) => {
                        if let Some(e) = self.transcript.get_mut(id) {
                            e.raw.push(raw);
                            if let EntryKind::Assistant { text: t, done, .. } = &mut e.kind {
                                if let Some(full) = text {
                                    *t = full;
                                }
                                *done = true;
                            }
                        }
                    }
                    None => {
                        if let Some(full) = text.filter(|t| !t.trim().is_empty()) {
                            let was_follow = self.view.follow;
                            self.transcript.push_assistant(&item_id, full, true, raw);
                            if !was_follow {
                                self.view.new_count += 1;
                            }
                        }
                    }
                }
            }
            AgentEvent::ToolStarted {
                call_id,
                kind,
                name,
                title,
                input,
            } => {
                if let Some(c) = self.transcript.call_mut(&call_id) {
                    c.raw.push(raw);
                    return;
                }
                let before = self.transcript.last_id();
                self.transcript.add_call(ToolCall {
                    call_id,
                    kind,
                    name,
                    title,
                    input,
                    output: String::new(),
                    output_truncated: false,
                    result: None,
                    status: ToolStatus::Running,
                    duration_ms: None,
                    diff: None,
                    started: Some(self.now),
                    expanded: false,
                    raw_in_open: false,
                    raw_out_open: false,
                    raw: vec![raw],
                });
                if !self.view.follow && self.transcript.last_id() != before {
                    self.view.new_count += 1;
                }
            }
            AgentEvent::ToolOutputDelta { call_id, text } => {
                if let Some(c) = self.transcript.call_mut(&call_id) {
                    c.raw.push(raw);
                    append_capped(&mut c.output, &mut c.output_truncated, &text);
                }
            }
            AgentEvent::ToolCompleted {
                call_id,
                status,
                output,
                result,
                duration_ms,
                diff,
            } => {
                if self.transcript.call_entry(&call_id).is_none() {
                    self.apply(
                        AgentEvent::ToolStarted {
                            call_id: call_id.clone(),
                            kind: ToolKind::Other,
                            name: "Tool".into(),
                            title: String::new(),
                            input: Value::Null,
                        },
                        raw,
                    );
                }
                let now = self.now;
                if let Some(c) = self.transcript.call_mut(&call_id) {
                    if c.raw.last() != Some(&raw) {
                        c.raw.push(raw);
                    }
                    c.status = status;
                    if !output.is_empty() || c.output.is_empty() {
                        c.output.clear();
                        c.output_truncated = false;
                        append_capped(&mut c.output, &mut c.output_truncated, &output);
                    }
                    c.result = result;
                    c.duration_ms = duration_ms.or_else(|| {
                        c.started
                            .map(|s| now.saturating_duration_since(s).as_millis() as u64)
                    });
                    if diff.is_some() {
                        c.diff = diff;
                    }
                }
                if let Some(gid) = self.transcript.call_entry(&call_id) {
                    if let Some(e) = self.transcript.get_mut(gid) {
                        if e.raw.last() != Some(&raw) {
                            e.raw.push(raw);
                        }
                    }
                }
            }
            AgentEvent::ApprovalRequest {
                request_id,
                title,
                subject,
                details,
                choices,
                call_id: _,
            } => {
                let id = self.push_entry(
                    EntryKind::Approval(ApprovalCard {
                        request_id: request_id.clone(),
                        title,
                        subject,
                        details,
                        choices,
                        selected: 0,
                        state: CardState::Pending,
                    }),
                    vec![raw],
                );
                self.transcript.register_request(&request_id, id);
                if self.focus != Focus::Card(id) {
                    self.set_hint(
                        NoticeLevel::Warn,
                        "Approval needed — press Tab to review it. Enter here won't answer it.",
                    );
                }
            }
            AgentEvent::QuestionRequest {
                request_id,
                title,
                questions,
                call_id: _,
            } => {
                let n = questions.first().map_or(0, |q| q.options.len());
                let id = self.push_entry(
                    EntryKind::Question(QuestionCard {
                        request_id: request_id.clone(),
                        title,
                        questions,
                        current: 0,
                        answers: vec![],
                        selected: 0,
                        toggled: vec![false; n],
                        typed: String::new(),
                        state: CardState::Pending,
                    }),
                    vec![raw],
                );
                self.transcript.register_request(&request_id, id);
                self.set_hint(
                    NoticeLevel::Warn,
                    "A question is waiting — press Tab to answer it.",
                );
            }
            AgentEvent::RequestResolved { request_id } => {
                if let Some(id) = self.transcript.request_entry(&request_id) {
                    if let Some(e) = self.transcript.get_mut(id) {
                        e.raw.push(raw);
                        match &mut e.kind {
                            EntryKind::Approval(c) if c.state == CardState::Pending => {
                                c.state = CardState::Resolved
                            }
                            EntryKind::Question(c) if c.state == CardState::Pending => {
                                c.state = CardState::Resolved
                            }
                            _ => {}
                        }
                    }
                    if self.focus == Focus::Card(id) {
                        self.focus = Focus::Composer;
                    }
                }
            }
            AgentEvent::TurnCompleted { outcome, usage } => {
                let elapsed = self.elapsed();
                self.finish_turn_locally(ToolStatus::Stopped);
                let (level, text) = match &outcome {
                    TurnOutcome::Completed => {
                        (NoticeLevel::Info, completion_line(elapsed, usage.as_ref()))
                    }
                    TurnOutcome::Interrupted => (NoticeLevel::Info, "■ Turn stopped".to_string()),
                    TurnOutcome::Failed(msg) => (
                        NoticeLevel::Error,
                        match msg {
                            Some(m) if !m.is_empty() => format!("✕ Turn failed · {m}"),
                            _ => "✕ Turn failed".to_string(),
                        },
                    ),
                };
                self.push_entry(EntryKind::TurnEnd { level, text }, vec![raw]);
                if !matches!(self.phase, Phase::Disconnected(_)) {
                    self.phase = Phase::Ready;
                }
                if self.degraded.take().is_some() {
                    self.notice(
                        NoticeLevel::Info,
                        "Turn state re-established; sending is enabled again.",
                        vec![],
                    );
                }
                if self
                    .hint
                    .as_ref()
                    .is_some_and(|h| h.level != NoticeLevel::Error)
                {
                    self.hint = None;
                }
            }
            AgentEvent::Notice { level, text } => self.notice(level, text, vec![raw]),
            AgentEvent::Diagnostic { text } => {
                self.notice(NoticeLevel::Warn, format!("⚠ {text}"), vec![raw])
            }
            AgentEvent::Degraded { reason } => {
                self.degraded = Some(reason.clone());
                self.notice(
                    NoticeLevel::Error,
                    format!("Connection degraded: {reason}. Sending is disabled until the turn completes (Ctrl+C to stop, /new to restart)."),
                    vec![raw],
                );
            }
            AgentEvent::Unknown => {}
        }
    }

    // ----- outgoing ---------------------------------------------------------

    fn can_send_reason(&self) -> Option<String> {
        if let Some(r) = &self.degraded {
            return Some(format!(
                "Connection degraded ({r}) — draft kept. Ctrl+C stops the turn, /new restarts."
            ));
        }
        match &self.phase {
            Phase::Ready => None,
            Phase::Starting => Some(format!("{} is still starting — draft kept.", self.harness.label())),
            Phase::Working => Some("Turn still running — draft kept. Send it when the turn finishes, or Ctrl+C to stop.".into()),
            Phase::Stopping => Some("Stopping the turn — draft kept.".into()),
            Phase::Disconnected(_) => Some("Disconnected — draft kept. /new starts a new session.".into()),
        }
    }

    /// Structured input never visits the composer or local slash dispatcher.
    pub fn send_logical_prompt(&mut self, text: String, delivery_id: &str) -> Result<(), String> {
        if let Some(reason) = self.can_send_reason() {
            return Err(reason);
        }
        if self.transcript.pending_cards().next().is_some() {
            return Err("a request is still awaiting a human answer".into());
        }
        let messages = self.adapter.send_logical_prompt(&text, delivery_id)?;
        let raw = self.send_all(messages);
        self.transcript.turn_tools = None;
        self.push_entry(EntryKind::User { text }, raw);
        self.phase = Phase::Working;
        self.turn_started = Some(self.now);
        Ok(())
    }

    fn send_prompt(&mut self, text: String) {
        if let Some(why) = self.can_send_reason() {
            self.set_hint(NoticeLevel::Warn, why);
            return;
        }
        match self.adapter.send_prompt(&text) {
            Ok(msgs) => {
                let raw = self.send_all(msgs);
                self.transcript.turn_tools = None;
                self.view.follow = true;
                self.view.new_count = 0;
                self.push_entry(EntryKind::User { text }, raw);
                self.composer.clear();
                self.refresh_slash();
                self.phase = Phase::Working;
                self.turn_started = Some(self.now);
                self.hint = None;
            }
            Err(e) => self.set_hint(NoticeLevel::Warn, format!("Not sent: {e}. Draft kept.")),
        }
    }

    pub fn stop_turn(&mut self) {
        match self.phase {
            Phase::Working => match self.adapter.interrupt() {
                Ok(msgs) => {
                    self.send_all(msgs);
                    self.phase = Phase::Stopping;
                    self.hint = None;
                }
                Err(e) => self.set_hint(NoticeLevel::Warn, format!("Could not stop: {e}")),
            },
            Phase::Stopping => self.set_hint(NoticeLevel::Info, "Already stopping…"),
            _ => self.set_hint(NoticeLevel::Info, "Nothing to stop · Ctrl+Q quits"),
        }
    }

    fn answer_card(&mut self, id: EntryId, answer: Answer, label: String) {
        let Some(request_id) = self.transcript.get(id).and_then(|e| match &e.kind {
            EntryKind::Approval(c) if c.state == CardState::Pending => Some(c.request_id.clone()),
            EntryKind::Question(c) if c.state == CardState::Pending => Some(c.request_id.clone()),
            _ => None,
        }) else {
            return;
        };
        let result = if matches!(self.phase, Phase::Disconnected(_)) {
            Err("disconnected".to_string())
        } else {
            self.adapter.respond(&request_id, &answer)
        };
        let (state, raw) = match result {
            Ok(msgs) => (
                CardState::Answered(acknowledged(&label)),
                self.send_all(msgs),
            ),
            Err(e) if e.contains("no longer pending") => (CardState::Resolved, vec![]),
            Err(e) => (CardState::Failed(e), vec![]),
        };
        if let Some(e) = self.transcript.get_mut(id) {
            e.raw.extend(raw);
            match &mut e.kind {
                EntryKind::Approval(c) => c.state = state,
                EntryKind::Question(c) => c.state = state,
                _ => {}
            }
        }
        self.focus = Focus::Composer;
        self.hint = self.transcript.first_pending_card().map(|_| Hint {
            text: "Another request is waiting — press Tab to review it.".into(),
            level: NoticeLevel::Warn,
        });
    }

    // ----- commands ---------------------------------------------------------

    pub fn all_commands(&self) -> Vec<CommandSpec> {
        let mut v = slash::local_commands();
        if self.caps.harness_commands {
            v.extend(slash::harness_commands(&self.commands));
        }
        v
    }

    fn refresh_slash(&mut self) {
        let all = self.all_commands();
        self.slash
            .update(self.composer.text(), self.composer.cursor(), &all);
    }

    fn run_command(&mut self, text: &str) {
        let Some((name, args)) = slash::parse_command(text) else {
            self.set_hint(NoticeLevel::Warn, "Type a command name after /.");
            return;
        };
        let (name, args) = (name.to_string(), args.to_string());
        if slash::LOCAL_COMMANDS.iter().any(|(n, _, _)| *n == name) {
            self.composer.clear();
            self.refresh_slash();
            self.hint = None;
            match name.as_str() {
                "help" => self.overlay = Overlay::Help { selected: 0 },
                "status" => {
                    let s = self.status_text();
                    self.notice(NoticeLevel::Info, s, vec![]);
                    self.view.follow = true;
                }
                "theme" => {
                    if args.is_empty() {
                        let selected = ALL_SKINS.iter().position(|s| *s == self.skin).unwrap_or(0);
                        self.overlay = Overlay::SkinPicker { selected };
                    } else if let Some(s) = SkinId::parse(&args) {
                        self.set_skin(s);
                    } else {
                        let names: Vec<&str> = ALL_SKINS.iter().map(|s| s.slug()).collect();
                        self.composer.set(text);
                        self.set_hint(
                            NoticeLevel::Warn,
                            format!("Unknown skin “{args}”. Try: {}.", names.join(", ")),
                        );
                    }
                }
                "stop" => self.stop_turn(),
                "new" => {
                    // Only a healthy running turn blocks a restart. Stopping and
                    // Degraded may never see a completion, so /new must work there.
                    if self.phase == Phase::Working && self.degraded.is_none() {
                        self.set_hint(
                            NoticeLevel::Warn,
                            "A turn is running — Ctrl+C to stop it first.",
                        );
                    } else {
                        self.restart_requested = true;
                    }
                }
                "quit" => self.request_quit(),
                _ => {}
            }
            return;
        }
        if self.caps.harness_commands && self.commands.iter().any(|c| c.name == name) {
            self.send_prompt(text.trim_end().to_string());
            return;
        }
        let tip = if self.caps.harness_commands {
            ""
        } else {
            " Codex commands are not available over this interface."
        };
        self.set_hint(
            NoticeLevel::Warn,
            format!("Unknown command /{name} — not sent.{tip} Start with a space to send text beginning with /."),
        );
    }

    pub fn status_text(&self) -> String {
        let meta = |m: &Option<MetaValue>| match m {
            Some(v) if v.source == MetaSource::Requested => format!("{} (requested)", v.value),
            Some(v) => v.value.clone(),
            None => "Not reported".into(),
        };
        let mut s = format!(
            "Harness {} · Model {} · Effort {} · State {}",
            self.harness.label(),
            meta(&self.meta.model),
            meta(&self.meta.effort),
            self.status().word()
        );
        if let Some(id) = &self.meta.session_id {
            s.push_str(&format!("\nSession {id}"));
        }
        if self.caps.harness_commands {
            s.push_str(&format!(
                "\n{} harness commands available via /",
                self.commands.len()
            ));
        }
        if let Phase::Disconnected(r) = &self.phase {
            s.push_str(&format!("\nDisconnected: {r}"));
        }
        s
    }

    pub fn set_skin(&mut self, s: SkinId) {
        if s != self.skin {
            self.skin = s;
            self.pick_quote();
        }
    }

    fn pick_quote(&mut self) {
        use rand::Rng;
        let n = self.skin.quotes().len();
        self.quote = if n > 1 {
            rand::thread_rng().gen_range(0..n)
        } else {
            0
        };
    }

    /// The "Stop & quit" path: interrupt a running turn, then quit. Also used
    /// for SIGHUP/SIGTERM/SIGINT, where there is nobody to ask.
    pub fn stop_and_quit(&mut self) {
        self.overlay = Overlay::None;
        if self.phase == Phase::Working {
            self.stop_turn();
        }
        self.should_quit = true;
    }

    pub fn request_quit(&mut self) {
        if self.turn_running() || self.transcript.first_pending_card().is_some() {
            self.overlay = Overlay::QuitConfirm { selected: 0 };
        } else {
            self.should_quit = true;
        }
    }

    // ----- keys -------------------------------------------------------------

    pub fn on_paste(&mut self, text: &str) {
        match self.focus {
            Focus::Search => {
                if let Some(s) = &mut self.search {
                    s.query.push_str(&text.replace('\n', " "));
                }
                self.recompute_search();
            }
            Focus::Card(id) => {
                if let Some(e) = self.transcript.get_mut(id) {
                    if let EntryKind::Question(q) = &mut e.kind {
                        if let Some(row) = q.type_row() {
                            q.typed.push_str(&text.replace('\n', " "));
                            q.selected = row;
                        }
                    }
                }
            }
            _ => {
                if self.overlay == Overlay::None {
                    self.focus = Focus::Composer;
                    self.composer.insert_str(text);
                    self.refresh_slash();
                }
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('q') {
            if matches!(self.overlay, Overlay::QuitConfirm { .. }) {
                self.should_quit = true;
            } else {
                self.request_quit();
            }
            return;
        }
        if self.overlay != Overlay::None {
            self.on_overlay_key(key);
            return;
        }
        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) => return self.stop_turn(),
            (KeyCode::F(1), _) => {
                self.overlay = Overlay::Help { selected: 0 };
                return;
            }
            (KeyCode::F(2), _) => {
                let selected = ALL_SKINS.iter().position(|s| *s == self.skin).unwrap_or(0);
                self.overlay = Overlay::SkinPicker { selected };
                return;
            }
            (KeyCode::Char('r'), true) => return self.open_inspector(),
            (KeyCode::Char('f'), true) => {
                if self.search.is_none() {
                    self.search = Some(SearchState::default());
                }
                self.focus = Focus::Search;
                return;
            }
            (KeyCode::Char('l'), true) => return self.jump_to_latest(),
            (KeyCode::PageUp, _) => return self.scroll_by(-(self.page() as isize)),
            (KeyCode::PageDown, _) => return self.scroll_by(self.page() as isize),
            _ => {}
        }
        match self.focus {
            Focus::Composer => self.on_composer_key(key),
            Focus::Transcript => self.on_transcript_key(key),
            Focus::Card(id) => self.on_card_key(id, key),
            Focus::Search => self.on_search_key(key),
        }
    }

    fn cycle_focus(&mut self, back: bool) {
        let card = self.transcript.first_pending_card();
        let order: Vec<Focus> = [
            Some(Focus::Composer),
            card.map(Focus::Card),
            Some(Focus::Transcript),
        ]
        .into_iter()
        .flatten()
        .collect();
        let cur = match self.focus {
            Focus::Search => 0,
            f => order.iter().position(|x| *x == f).unwrap_or(0),
        };
        let next = if back {
            (cur + order.len() - 1) % order.len()
        } else {
            (cur + 1) % order.len()
        };
        self.focus = order[next];
        if let Focus::Card(id) = self.focus {
            self.view.selected = Some(Target::Entry(id));
            self.view.ensure_visible = true;
            self.hint = None;
        }
        if self.focus == Focus::Transcript && self.view.selected.is_none() {
            self.view.selected = self.targets().last().copied();
            self.view.ensure_visible = true;
        }
    }

    fn on_composer_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if self.slash.open {
            match (key.code, ctrl) {
                (KeyCode::Up, _) | (KeyCode::Char('p'), true) => return self.slash.prev(),
                (KeyCode::Down, _) | (KeyCode::Char('n'), true) => return self.slash.next(),
                (KeyCode::Tab, _) | (KeyCode::Enter, false) if !shift && !alt => {
                    if let Some(c) = self.slash.current().cloned() {
                        self.composer.set(&format!("/{} ", c.name));
                        self.refresh_slash();
                        if c.source == CommandSource::Harness && self.turn_running() {
                            self.set_hint(
                                NoticeLevel::Info,
                                format!("/{} runs when the current turn finishes.", c.name),
                            );
                        } else {
                            self.set_hint(
                                NoticeLevel::Info,
                                format!(
                                    "Enter runs /{}{}",
                                    c.name,
                                    if c.argument_hint.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" {}", c.argument_hint)
                                    }
                                ),
                            );
                        }
                    }
                    return;
                }
                (KeyCode::Esc, _) => {
                    let t = self.composer.text().to_string();
                    self.slash.dismiss(&t);
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Enter if shift || alt => self.composer.newline(),
            KeyCode::Enter => {
                let text = self.composer.text().to_string();
                if text.trim().is_empty() {
                    return;
                }
                if text.starts_with('/') {
                    self.run_command(&text);
                } else {
                    let t = text
                        .strip_prefix(' ')
                        .filter(|t| t.starts_with('/'))
                        .map(str::to_string)
                        .unwrap_or(text);
                    self.send_prompt(t);
                }
                return;
            }
            KeyCode::Char('j') if ctrl => self.composer.newline(),
            KeyCode::Char('a') if ctrl => self.composer.home(),
            KeyCode::Char('e') if ctrl => self.composer.end(),
            KeyCode::Char('u') if ctrl => self.composer.kill_to_line_start(),
            KeyCode::Char('w') if ctrl => self.composer.delete_word_back(),
            KeyCode::Backspace if alt || ctrl => self.composer.delete_word_back(),
            KeyCode::Char(c) if !ctrl => self.composer.insert_char(c),
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.left(),
            KeyCode::Right => self.composer.right(),
            KeyCode::Home => self.composer.home(),
            KeyCode::End => self.composer.end(),
            KeyCode::Up => {
                self.composer.up();
            }
            KeyCode::Down => {
                self.composer.down();
            }
            KeyCode::Tab => return self.cycle_focus(false),
            KeyCode::BackTab => return self.cycle_focus(true),
            KeyCode::Esc => {
                if self.search.is_some() {
                    self.search = None;
                }
                self.hint = None;
                return;
            }
            _ => return,
        }
        if self
            .hint
            .as_ref()
            .is_some_and(|h| h.level == NoticeLevel::Info)
        {
            self.hint = None;
        }
        self.refresh_slash();
    }

    fn on_transcript_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) => self.move_selection(-1),
            (KeyCode::Down, _) | (KeyCode::Char('n'), true) => self.move_selection(1),
            (KeyCode::Home, _) => {
                self.view.selected = self.targets().first().copied();
                self.view.ensure_visible = true;
            }
            (KeyCode::End, _) => {
                self.view.selected = self.targets().last().copied();
                self.jump_to_latest();
            }
            (KeyCode::Enter, _) | (KeyCode::Char(' '), false) => self.toggle_selected(),
            (KeyCode::Tab, _) => self.cycle_focus(false),
            (KeyCode::BackTab, _) => self.cycle_focus(true),
            (KeyCode::Esc, _) => self.focus = Focus::Composer,
            (KeyCode::Char(c), false) => {
                // Typing always goes to the draft.
                self.focus = Focus::Composer;
                self.composer.insert_char(c);
                self.refresh_slash();
            }
            _ => {}
        }
    }

    fn on_card_key(&mut self, id: EntryId, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(entry) = self.transcript.get(id) else {
            self.focus = Focus::Composer;
            return;
        };
        if !entry.pending_card() {
            self.focus = Focus::Composer;
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.focus = Focus::Composer;
                self.set_hint(
                    NoticeLevel::Info,
                    "Request left pending — Tab to return to it.",
                );
                return;
            }
            KeyCode::Tab => return self.cycle_focus(false),
            KeyCode::BackTab => return self.cycle_focus(true),
            _ => {}
        }
        if let EntryKind::Approval(card) = &entry.kind {
            let n = card.choices.len().max(1);
            let mut sel = card.selected;
            match (key.code, ctrl) {
                (KeyCode::Left | KeyCode::Up, _) | (KeyCode::Char('p'), true) => {
                    sel = (sel + n - 1) % n
                }
                (KeyCode::Right | KeyCode::Down, _) | (KeyCode::Char('n'), true) => {
                    sel = (sel + 1) % n
                }
                (KeyCode::Char(c), false) if c.is_ascii_digit() => {
                    let d = c.to_digit(10).unwrap_or(0) as usize;
                    if d >= 1 && d <= n {
                        sel = d - 1;
                    }
                }
                (KeyCode::Enter, _) => {
                    if let Some(choice) = card.choices.get(sel).cloned() {
                        return self.answer_card(id, Answer::Choice(choice.key), choice.label);
                    }
                }
                _ => {}
            }
            if let Some(e) = self.transcript.get_mut(id) {
                if let EntryKind::Approval(c) = &mut e.kind {
                    c.selected = sel;
                }
            }
            return;
        }
        let EntryKind::Question(card) = &entry.kind else {
            return;
        };
        let mut card = card.clone();
        let rows = card.row_count();
        let mut submit: Option<Vec<String>> = None;
        let mut decline = false;
        match (key.code, ctrl) {
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) => {
                card.selected = (card.selected + rows - 1) % rows
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), true) => {
                card.selected = (card.selected + 1) % rows
            }
            (KeyCode::Char(' '), false)
                if card.question().is_some_and(|q| q.multi_select)
                    && card.selected < card.toggled.len() =>
            {
                card.toggled[card.selected] = !card.toggled[card.selected];
            }
            (KeyCode::Char(c), false) => {
                if let Some(row) = card.type_row() {
                    card.typed.push(c);
                    card.selected = row;
                }
            }
            (KeyCode::Backspace, _) => {
                card.typed.pop();
            }
            (KeyCode::Enter, _) => {
                let q = card.question().cloned();
                if card.selected == card.decline_row() {
                    decline = true;
                } else if Some(card.selected) == card.type_row() {
                    if card.typed.trim().is_empty() {
                        self.set_hint(NoticeLevel::Warn, "Type an answer first.");
                    } else {
                        submit = Some(vec![card.typed.trim().to_string()]);
                    }
                } else if let Some(q) = q {
                    if q.multi_select && card.toggled.iter().any(|t| *t) {
                        submit = Some(
                            q.options
                                .iter()
                                .zip(&card.toggled)
                                .filter(|(_, t)| **t)
                                .map(|(o, _)| o.label.clone())
                                .collect(),
                        );
                    } else if let Some(o) = q.options.get(card.selected) {
                        submit = Some(vec![o.label.clone()]);
                    }
                }
            }
            _ => {}
        }
        if decline {
            return self.answer_card(id, Answer::Decline, "Declined".into());
        }
        if let Some(ans) = submit {
            card.answers.push(ans);
            card.current += 1;
            card.selected = 0;
            card.typed.clear();
            card.toggled = vec![false; card.question().map_or(0, |q| q.options.len())];
            if card.current >= card.questions.len() {
                let label = format!(
                    "Answered: {}",
                    card.answers
                        .iter()
                        .map(|a| a.join(", "))
                        .collect::<Vec<_>>()
                        .join(" · ")
                );
                let answers = card.answers.clone();
                if let Some(e) = self.transcript.get_mut(id) {
                    if let EntryKind::Question(c) = &mut e.kind {
                        *c = card;
                    }
                }
                return self.answer_card(id, Answer::Answers(answers), label);
            }
        }
        if let Some(e) = self.transcript.get_mut(id) {
            if let EntryKind::Question(c) = &mut e.kind {
                *c = card;
            }
        }
    }

    fn on_search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match (key.code, ctrl) {
            (KeyCode::Esc, _) => {
                self.search = None;
                self.focus = Focus::Composer;
            }
            (KeyCode::Enter, _) if shift => self.search_step(-1),
            (KeyCode::Enter, _) | (KeyCode::Down, _) | (KeyCode::Char('n'), true) => {
                self.search_step(1)
            }
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) => self.search_step(-1),
            (KeyCode::Tab, _) => self.focus = Focus::Transcript,
            (KeyCode::Backspace, _) => {
                if let Some(s) = &mut self.search {
                    s.query.pop();
                }
                self.recompute_search();
            }
            (KeyCode::Char(c), false) => {
                if let Some(s) = &mut self.search {
                    s.query.push(c);
                }
                self.recompute_search();
            }
            _ => {}
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let up = matches!(key.code, KeyCode::Up) || (ctrl && key.code == KeyCode::Char('p'));
        let down = matches!(key.code, KeyCode::Down) || (ctrl && key.code == KeyCode::Char('n'));
        let cmds = self.all_commands();
        match &mut self.overlay {
            Overlay::Help { selected } => {
                let n = cmds.len().max(1);
                if key.code == KeyCode::Esc || key.code == KeyCode::F(1) {
                    self.overlay = Overlay::None;
                } else if up {
                    *selected = (*selected + n - 1) % n;
                } else if down {
                    *selected = (*selected + 1) % n;
                } else if key.code == KeyCode::Enter {
                    if let Some(c) = cmds.get(*selected) {
                        self.composer.set(&format!("/{} ", c.name));
                        self.focus = Focus::Composer;
                        self.refresh_slash();
                    }
                    self.overlay = Overlay::None;
                }
            }
            Overlay::Inspector { scroll, .. } => match key.code {
                KeyCode::Esc => self.overlay = Overlay::None,
                KeyCode::Char('r') if ctrl => self.overlay = Overlay::None,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll += 1,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(20),
                KeyCode::PageDown => *scroll += 20,
                KeyCode::Home => *scroll = 0,
                _ => {}
            },
            Overlay::SkinPicker { selected } => {
                let n = ALL_SKINS.len();
                if key.code == KeyCode::Esc || key.code == KeyCode::F(2) {
                    self.overlay = Overlay::None;
                } else if up {
                    *selected = (*selected + n - 1) % n;
                } else if down {
                    *selected = (*selected + 1) % n;
                } else if key.code == KeyCode::Enter {
                    let s = ALL_SKINS[*selected];
                    self.overlay = Overlay::None;
                    self.set_skin(s);
                }
            }
            Overlay::QuitConfirm { selected } => match key.code {
                KeyCode::Esc => self.overlay = Overlay::None,
                KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Tab => {
                    *selected = 1 - *selected
                }
                KeyCode::Enter => {
                    if *selected == 0 {
                        self.stop_and_quit();
                    }
                    self.overlay = Overlay::None;
                }
                _ => {}
            },
            Overlay::None => {}
        }
    }

    // ----- view, selection, search -----------------------------------------

    fn page(&self) -> usize {
        self.view.height.saturating_sub(2).max(1)
    }

    pub fn max_top(&self) -> usize {
        self.view.total.saturating_sub(self.view.height)
    }

    pub fn scroll_by(&mut self, delta: isize) {
        let current = if self.view.follow {
            self.max_top()
        } else {
            self.view.top.min(self.max_top())
        };
        let top = (current as isize + delta).clamp(0, self.max_top() as isize) as usize;
        if top >= self.max_top() {
            self.jump_to_latest();
        } else {
            self.view.follow = false;
            self.view.top = top;
        }
    }

    pub fn jump_to_latest(&mut self) {
        self.view.follow = true;
        self.view.new_count = 0;
        self.view.ensure_visible = false;
    }

    /// Every selectable item in display order.
    pub fn targets(&self) -> Vec<Target> {
        let mut out = Vec::new();
        for e in self.transcript.iter() {
            out.push(Target::Entry(e.id));
            if let EntryKind::Tools(g) = &e.kind {
                if g.expanded {
                    for (i, c) in g.calls.iter().enumerate() {
                        out.push(Target::Call(e.id, i));
                        if c.expanded && c.has_raw_sections() {
                            out.push(Target::RawIn(e.id, i));
                            out.push(Target::RawOut(e.id, i));
                        }
                    }
                }
            }
        }
        out
    }

    fn move_selection(&mut self, delta: isize) {
        let ts = self.targets();
        if ts.is_empty() {
            return;
        }
        let cur = self
            .view
            .selected
            .and_then(|s| ts.iter().position(|t| *t == s));
        let next = match cur {
            Some(i) => (i as isize + delta).clamp(0, ts.len() as isize - 1) as usize,
            None => ts.len() - 1,
        };
        self.view.selected = Some(ts[next]);
        self.view.ensure_visible = true;
    }

    fn toggle_selected(&mut self) {
        let Some(t) = self.view.selected else { return };
        let Some(e) = self.transcript.get_mut(t.entry()) else {
            return;
        };
        match (t, &mut e.kind) {
            (Target::Entry(_), EntryKind::Tools(g)) => g.expanded = !g.expanded,
            (Target::Call(_, i), EntryKind::Tools(g)) => {
                if let Some(c) = g.calls.get_mut(i) {
                    c.expanded = !c.expanded;
                }
            }
            (Target::RawIn(_, i), EntryKind::Tools(g)) => {
                if let Some(c) = g.calls.get_mut(i) {
                    c.raw_in_open = !c.raw_in_open;
                }
            }
            (Target::RawOut(_, i), EntryKind::Tools(g)) => {
                if let Some(c) = g.calls.get_mut(i) {
                    c.raw_out_open = !c.raw_out_open;
                }
            }
            (Target::Entry(id), EntryKind::Approval(_) | EntryKind::Question(_)) => {
                if e.pending_card() {
                    self.focus = Focus::Card(id);
                }
            }
            _ => {}
        }
        self.view.ensure_visible = true;
    }

    fn open_inspector(&mut self) {
        let selected = if matches!(
            self.focus,
            Focus::Transcript | Focus::Card(_) | Focus::Search
        ) {
            self.view.selected
        } else {
            None
        };
        let (title, records) =
            match selected.and_then(|t| self.transcript.get(t.entry()).map(|e| (t, e))) {
                Some((t, e)) => {
                    let call = match (t, &e.kind) {
                        (
                            Target::Call(_, i) | Target::RawIn(_, i) | Target::RawOut(_, i),
                            EntryKind::Tools(g),
                        ) => g.calls.get(i),
                        _ => None,
                    };
                    match call {
                        Some(c) => (
                            format!("Raw JSON · {} {}", c.name, c.call_id),
                            c.raw.clone(),
                        ),
                        None => ("Raw JSON · selected entry".to_string(), e.raw.clone()),
                    }
                }
                None => {
                    let recs: Vec<RawId> = self
                        .raw
                        .iter()
                        .rev()
                        .take(200)
                        .map(|r| r.id)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    (
                        format!(
                            "Raw events · last {} of {}",
                            recs.len(),
                            self.raw.len() as u64 + self.raw.discarded
                        ),
                        recs,
                    )
                }
            };
        self.overlay = Overlay::Inspector {
            title,
            records,
            scroll: 0,
        };
    }

    fn recompute_search(&mut self) {
        let Some(s) = &self.search else { return };
        let q = s.query.to_lowercase();
        let mut matches = Vec::new();
        if !q.is_empty() {
            for e in self.transcript.iter() {
                let hit = |t: &str| t.to_lowercase().contains(&q);
                match &e.kind {
                    EntryKind::User { text }
                    | EntryKind::Assistant { text, .. }
                    | EntryKind::Notice { text, .. }
                    | EntryKind::TurnEnd { text, .. } => {
                        if hit(text) {
                            matches.push(Target::Entry(e.id));
                        }
                    }
                    EntryKind::Tools(g) => {
                        for (i, c) in g.calls.iter().enumerate() {
                            let input = if c.input.is_null() {
                                String::new()
                            } else {
                                c.input.to_string()
                            };
                            if hit(&c.title)
                                || hit(&c.name)
                                || hit(&c.output)
                                || hit(&input)
                                || c.diff.as_deref().is_some_and(hit)
                            {
                                matches.push(Target::Call(e.id, i));
                            }
                        }
                    }
                    EntryKind::Approval(c) => {
                        if hit(&c.title) || hit(&c.subject) || c.details.iter().any(|d| hit(d)) {
                            matches.push(Target::Entry(e.id));
                        }
                    }
                    EntryKind::Question(c) => {
                        if c.questions
                            .iter()
                            .any(|q| hit(&q.text) || q.options.iter().any(|o| hit(&o.label)))
                        {
                            matches.push(Target::Entry(e.id));
                        }
                    }
                }
            }
        }
        let s = self.search.as_mut().unwrap();
        s.current = if matches.is_empty() {
            None
        } else {
            Some(matches.len() - 1)
        };
        s.matches = matches;
        if let Some(i) = s.current {
            let t = s.matches[i];
            self.reveal(t);
        }
    }

    fn search_step(&mut self, delta: isize) {
        let Some(s) = &mut self.search else { return };
        if s.matches.is_empty() {
            return;
        }
        let n = s.matches.len() as isize;
        let i = s.current.map_or(0, |c| ((c as isize + delta) % n + n) % n) as usize;
        s.current = Some(i);
        let t = s.matches[i];
        self.reveal(t);
    }

    /// Selects a target, expanding what contains it, and scrolls to it.
    fn reveal(&mut self, t: Target) {
        if let Target::Call(eid, i) = t {
            if let Some(e) = self.transcript.get_mut(eid) {
                if let EntryKind::Tools(g) = &mut e.kind {
                    g.expanded = true;
                    if let Some(c) = g.calls.get_mut(i) {
                        c.expanded = true;
                    }
                }
            }
        }
        self.view.selected = Some(t);
        self.view.ensure_visible = true;
        self.view.follow = false;
    }
}

/// Past-tense acknowledgement for a sent decision ("Allow once" -> "Allowed once").
fn acknowledged(label: &str) -> String {
    let map = [
        ("Allow once", "Allowed once"),
        ("Allow for session", "Allowed for session"),
        ("Always allow this command", "Always allowed this command"),
        ("Allow", "Allowed"),
        ("Deny and stop turn", "Denied and stopped the turn"),
        ("Deny", "Denied"),
        ("Decline", "Declined"),
        ("Cancel", "Cancelled"),
        ("Grant for this turn", "Granted for this turn"),
        ("Grant for session", "Granted for session"),
    ];
    map.iter()
        .find(|(k, _)| *k == label)
        .map_or_else(|| label.to_string(), |(_, v)| v.to_string())
}

fn append_capped(buf: &mut String, truncated: &mut bool, text: &str) {
    if *truncated {
        return;
    }
    let room = MAX_TOOL_OUTPUT.saturating_sub(buf.len());
    if text.len() <= room {
        buf.push_str(text);
    } else {
        let mut cut = room;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        buf.push_str(&text[..cut]);
        *truncated = true;
    }
}

pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}h{:02}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else if s >= 10 {
        format!("{s}s")
    } else {
        format!("{:.1}s", d.as_secs_f64())
    }
}

fn compact_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn completion_line(elapsed: Option<Duration>, usage: Option<&Usage>) -> String {
    let mut parts = vec!["✓ Turn complete".to_string()];
    let dur = usage
        .and_then(|u| u.duration_ms)
        .map(Duration::from_millis)
        .or(elapsed);
    if let Some(d) = dur {
        parts.push(format_duration(d));
    }
    if let Some(u) = usage {
        if let Some(i) = u.input_tokens {
            parts.push(format!("{} in", compact_count(i)));
        }
        if let Some(o) = u.output_tokens {
            parts.push(format!("{} out", compact_count(o)));
        }
        if let Some(c) = u.session_cost_usd {
            parts.push(format!("session ${c:.2}"));
        }
    }
    parts.join(" · ")
}
