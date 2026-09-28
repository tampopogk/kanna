//! Conversation entries, per-turn tool groups keyed by call id, and request
//! cards. Retention is bounded; trimming only affects local display.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use serde_json::Value;

use crate::protocol::{Choice, NoticeLevel, Question, ToolKind, ToolStatus};
use crate::raw::RawId;

pub const MAX_ENTRIES: usize = 5_000;
/// Tool output kept per call; the raw record still holds the original.
pub const MAX_TOOL_OUTPUT: usize = 1024 * 1024;

pub type EntryId = u64;

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub call_id: String,
    pub kind: ToolKind,
    pub name: String,
    pub title: String,
    pub input: Value,
    pub output: String,
    pub output_truncated: bool,
    pub result: Option<Value>,
    pub status: ToolStatus,
    pub duration_ms: Option<u64>,
    pub diff: Option<String>,
    pub started: Option<Instant>,
    pub expanded: bool,
    pub raw_in_open: bool,
    pub raw_out_open: bool,
    pub raw: Vec<RawId>,
}

impl ToolCall {
    pub fn has_raw_sections(&self) -> bool {
        matches!(self.kind, ToolKind::Mcp { .. })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ToolGroup {
    pub expanded: bool,
    pub calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolCounts {
    pub completed: usize,
    pub failed: usize,
    pub running: usize,
    pub stopped: usize,
}

impl ToolGroup {
    pub fn counts(&self) -> ToolCounts {
        let mut c = ToolCounts::default();
        for call in &self.calls {
            match call.status {
                ToolStatus::Succeeded => c.completed += 1,
                ToolStatus::Failed | ToolStatus::Declined => c.failed += 1,
                ToolStatus::Running => c.running += 1,
                ToolStatus::Stopped => c.stopped += 1,
            }
        }
        c
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardState {
    Pending,
    /// Decision sent; label says what was chosen.
    Answered(String),
    /// The harness withdrew the request before it was answered.
    Resolved,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct ApprovalCard {
    pub request_id: String,
    pub title: String,
    pub subject: String,
    pub details: Vec<String>,
    pub choices: Vec<Choice>,
    pub selected: usize,
    pub state: CardState,
}

#[derive(Debug, Clone)]
pub struct QuestionCard {
    pub request_id: String,
    pub title: String,
    pub questions: Vec<Question>,
    /// Index of the question being answered.
    pub current: usize,
    pub answers: Vec<Vec<String>>,
    /// Row selected for the current question: options, then "type", then "decline".
    pub selected: usize,
    /// Toggled options for multi-select questions.
    pub toggled: Vec<bool>,
    pub typed: String,
    pub state: CardState,
}

impl QuestionCard {
    pub fn question(&self) -> Option<&Question> {
        self.questions.get(self.current)
    }
    pub fn type_row(&self) -> Option<usize> {
        let q = self.question()?;
        q.free_text.then_some(q.options.len())
    }
    pub fn decline_row(&self) -> usize {
        self.question()
            .map_or(0, |q| q.options.len() + usize::from(q.free_text))
    }
    pub fn row_count(&self) -> usize {
        self.decline_row() + 1
    }
}

#[derive(Debug, Clone)]
pub enum EntryKind {
    User {
        text: String,
    },
    Assistant {
        item_id: String,
        text: String,
        done: bool,
    },
    Tools(ToolGroup),
    Approval(ApprovalCard),
    Question(QuestionCard),
    Notice {
        level: NoticeLevel,
        text: String,
    },
    /// End-of-turn line: "✓ Turn complete · 24s", "■ Turn stopped", …
    TurnEnd {
        level: NoticeLevel,
        text: String,
    },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: EntryId,
    pub kind: EntryKind,
    /// Raw records behind this entry, in arrival order.
    pub raw: Vec<RawId>,
    /// Bumped on every mutation; used as a render-cache key.
    pub rev: u64,
}

impl Entry {
    pub fn pending_card(&self) -> bool {
        match &self.kind {
            EntryKind::Approval(c) => c.state == CardState::Pending,
            EntryKind::Question(c) => c.state == CardState::Pending,
            _ => false,
        }
    }
}

#[derive(Debug, Default)]
pub struct Transcript {
    entries: VecDeque<Entry>,
    next_id: EntryId,
    max: usize,
    pub discarded: u64,
    assistant_items: HashMap<String, EntryId>,
    /// call id -> tools entry holding it.
    calls: HashMap<String, EntryId>,
    requests: HashMap<String, EntryId>,
    /// The Tools group for the running turn.
    pub turn_tools: Option<EntryId>,
}

impl Transcript {
    pub fn new() -> Self {
        Self::with_capacity(MAX_ENTRIES)
    }

    pub fn with_capacity(max: usize) -> Self {
        Self {
            next_id: 1,
            max,
            ..Default::default()
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn index_of(&self, id: EntryId) -> Option<usize> {
        let first = self.entries.front()?.id;
        if id < first {
            return None;
        }
        let i = (id - first) as usize;
        (i < self.entries.len()).then_some(i)
    }

    pub fn get(&self, id: EntryId) -> Option<&Entry> {
        self.index_of(id).map(|i| &self.entries[i])
    }

    /// Mutable access that marks the entry changed.
    pub fn get_mut(&mut self, id: EntryId) -> Option<&mut Entry> {
        let i = self.index_of(id)?;
        let e = &mut self.entries[i];
        e.rev += 1;
        Some(e)
    }

    pub fn push(&mut self, kind: EntryKind, raw: Vec<RawId>) -> EntryId {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push_back(Entry {
            id,
            kind,
            raw,
            rev: 0,
        });
        while self.entries.len() > self.max {
            if let Some(old) = self.entries.pop_front() {
                self.discarded += 1;
                self.forget(&old);
            }
        }
        id
    }

    fn forget(&mut self, e: &Entry) {
        match &e.kind {
            EntryKind::Assistant { item_id, .. } => {
                self.assistant_items.remove(item_id);
            }
            EntryKind::Tools(g) => {
                for c in &g.calls {
                    self.calls.remove(&c.call_id);
                }
                if self.turn_tools == Some(e.id) {
                    self.turn_tools = None;
                }
            }
            EntryKind::Approval(c) => {
                self.requests.remove(&c.request_id);
            }
            EntryKind::Question(c) => {
                self.requests.remove(&c.request_id);
            }
            _ => {}
        }
    }

    pub fn assistant_entry(&self, item_id: &str) -> Option<EntryId> {
        self.assistant_items
            .get(item_id)
            .copied()
            .filter(|id| self.index_of(*id).is_some())
    }

    pub fn push_assistant(
        &mut self,
        item_id: &str,
        text: String,
        done: bool,
        raw: RawId,
    ) -> EntryId {
        let id = self.push(
            EntryKind::Assistant {
                item_id: item_id.to_string(),
                text,
                done,
            },
            vec![raw],
        );
        self.assistant_items.insert(item_id.to_string(), id);
        id
    }

    pub fn call_entry(&self, call_id: &str) -> Option<EntryId> {
        self.calls
            .get(call_id)
            .copied()
            .filter(|id| self.index_of(*id).is_some())
    }

    pub fn call_mut(&mut self, call_id: &str) -> Option<&mut ToolCall> {
        let eid = self.call_entry(call_id)?;
        match &mut self.get_mut(eid)?.kind {
            EntryKind::Tools(g) => g.calls.iter_mut().find(|c| c.call_id == call_id),
            _ => None,
        }
    }

    /// Adds a call to the running turn's Tools group, creating the group.
    pub fn add_call(&mut self, call: ToolCall) -> EntryId {
        let gid = match self.turn_tools.filter(|id| self.index_of(*id).is_some()) {
            Some(id) => id,
            None => {
                let id = self.push(EntryKind::Tools(ToolGroup::default()), vec![]);
                self.turn_tools = Some(id);
                id
            }
        };
        self.calls.insert(call.call_id.clone(), gid);
        if let Some(e) = self.get_mut(gid) {
            e.raw.extend(call.raw.iter().copied());
            if let EntryKind::Tools(g) = &mut e.kind {
                g.calls.push(call);
            }
        }
        gid
    }

    pub fn register_request(&mut self, request_id: &str, id: EntryId) {
        self.requests.insert(request_id.to_string(), id);
    }

    pub fn request_entry(&self, request_id: &str) -> Option<EntryId> {
        self.requests
            .get(request_id)
            .copied()
            .filter(|id| self.index_of(*id).is_some())
    }

    pub fn first_pending_card(&self) -> Option<EntryId> {
        self.entries.iter().find(|e| e.pending_card()).map(|e| e.id)
    }

    pub fn pending_cards(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.pending_card())
    }

    pub fn ids(&self) -> Vec<EntryId> {
        self.entries.iter().map(|e| e.id).collect()
    }

    pub fn last_id(&self) -> Option<EntryId> {
        self.entries.back().map(|e| e.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str) -> ToolCall {
        ToolCall {
            call_id: id.into(),
            kind: ToolKind::Shell,
            name: "Shell".into(),
            title: "ls".into(),
            input: Value::Null,
            output: String::new(),
            output_truncated: false,
            result: None,
            status: ToolStatus::Running,
            duration_ms: None,
            diff: None,
            started: None,
            expanded: false,
            raw_in_open: false,
            raw_out_open: false,
            raw: vec![],
        }
    }

    #[test]
    fn one_group_per_turn_and_bounded_retention() {
        let mut t = Transcript::with_capacity(3);
        let g1 = t.add_call(call("a"));
        let g2 = t.add_call(call("b"));
        assert_eq!(g1, g2);
        t.turn_tools = None;
        t.push(EntryKind::User { text: "x".into() }, vec![]);
        t.push(EntryKind::User { text: "y".into() }, vec![]);
        t.push(EntryKind::User { text: "z".into() }, vec![]);
        assert_eq!(t.len(), 3);
        assert_eq!(t.discarded, 1);
        assert!(
            t.call_entry("a").is_none(),
            "calls of discarded groups are forgotten"
        );
    }
}
