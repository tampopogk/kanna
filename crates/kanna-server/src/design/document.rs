//! The server's view of an App Design live document (docs/specs/app-design.md
//! §4 "Doc editing and comments").
//!
//! The document is a Yjs document written by BlockNote through y-prosemirror.
//! This module reads and edits it with Yrs, in the same schema the browser
//! uses (`packages/design-editor/src/schema.ts`, described by
//! `resources/design-schema.json`):
//!
//! ```text
//! <fragment "document-store">
//!   <blockGroup>
//!     <blockContainer id="…">
//!       <paragraph textAlignment="left" …>text with formatting attributes</paragraph>
//!       <blockGroup> nested blockContainers </blockGroup>?
//!     </blockContainer>
//! ```
//!
//! Formatting is Yjs text attributes: `bold: {}`, `textColor: {stringValue}`,
//! `link: {href, …}`, and the comment mark under `comment--<hash>` (the hash
//! y-prosemirror gives a mark that may overlap itself) carrying `threadId`.
//!
//! Two rules come from what the prototype taught (§9.1):
//!
//! - **Reads never write.** A projection is taken from a detached copy of the
//!   state, and a document holding anything this schema does not know fails
//!   the read with an explicit error rather than being normalised: a narrower
//!   reader "fixing" content is exactly how the prototype deleted text.
//! - **Edits are targeted.** An agent edits blocks by id with a precondition
//!   on each block's current text; the text change is the minimal range
//!   between what it expected and what it asked for, so concurrent edits to
//!   other blocks, and formatting and comment anchors outside that range,
//!   survive. A stale precondition is a conflict with the block's current
//!   content, never a silent overwrite. Nothing replaces the whole document.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};
use yrs::types::text::{Diff, YChange};
use yrs::types::xml::{XmlElementPrelim, XmlOut, XmlTextPrelim};
use yrs::types::Attrs;
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Any, Doc, Out, ReadTxn, StateVector, Text, Transact, Update, Xml, XmlElementRef,
    XmlFragment, XmlFragmentRef, XmlTextRef,
};

/// The schema every client of a design document must use. Kept equal to
/// `DESIGN_SCHEMA_VERSION` in `packages/design-editor/src/schema.ts` by the
/// fixture tests on both sides.
pub const SCHEMA_VERSION: &str = "kanna-design-doc/1 blocknote@0.55.0";
/// The Yjs root the blocks live in.
pub const FRAGMENT: &str = "document-store";

const COMMENT_ATTRIBUTE_PREFIX: &str = "comment--";
/// One update from a client, and a whole document: generous for text, small
/// enough that a runaway client cannot fill the disk or the relay.
pub const MAX_UPDATE_BYTES: usize = 1 << 20;
pub const MAX_DOCUMENT_BYTES: usize = 8 << 20;

#[derive(Debug, Deserialize)]
struct SchemaDescription {
    version: String,
    fragment: String,
    blocks: BTreeMap<String, BlockDescription>,
    styles: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct BlockDescription {
    content: String,
    props: BTreeMap<String, PropDescription>,
}

#[derive(Debug, Deserialize)]
struct PropDescription {
    default: Value,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    values: Option<Vec<Value>>,
}

static SCHEMA: LazyLock<SchemaDescription> = LazyLock::new(|| {
    let schema: SchemaDescription =
        serde_json::from_str(include_str!("../../resources/design-schema.json"))
            .expect("resources/design-schema.json is valid");
    assert_eq!(schema.version, SCHEMA_VERSION, "design schema version drifted");
    assert_eq!(schema.fragment, FRAGMENT, "design schema fragment drifted");
    schema
});

/// Why a document or an edit was refused.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DocumentError {
    /// The document holds something this schema does not know. Nothing was
    /// read into a narrower form and nothing was written.
    UnsupportedSchema { detail: String },
    /// The bytes are not a Yjs update.
    MalformedUpdate { detail: String },
    TooLarge { detail: String },
    /// The operation names a block that is not in the document.
    BlockNotFound { block_id: String },
    /// The operation is well formed but cannot apply to that block.
    InvalidOperation { detail: String },
}

impl std::fmt::Display for DocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchema { detail } => write!(f, "unsupported document schema: {detail}"),
            Self::MalformedUpdate { detail } => write!(f, "malformed document update: {detail}"),
            Self::TooLarge { detail } => write!(f, "document too large: {detail}"),
            Self::BlockNotFound { block_id } => write!(f, "block not found: {block_id}"),
            Self::InvalidOperation { detail } => write!(f, "invalid operation: {detail}"),
        }
    }
}

fn unsupported(detail: impl Into<String>) -> DocumentError {
    DocumentError::UnsupportedSchema {
        detail: detail.into(),
    }
}

/// A run of text with one set of formatting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedRun {
    pub text: String,
    pub styles: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<String>,
}

/// One block as an agent reads it: the same shape as
/// `packages/design-editor/src/projection.ts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedBlock {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub props: Map<String, Value>,
    pub text: String,
    pub content: Vec<ProjectedRun>,
    pub children: Vec<ProjectedBlock>,
}

impl ProjectedBlock {
    fn visit<'a>(&'a self, out: &mut Vec<&'a ProjectedBlock>) {
        out.push(self);
        for child in &self.children {
            child.visit(out);
        }
    }
}

/// Every block of a projection, depth first.
pub fn flatten(blocks: &[ProjectedBlock]) -> Vec<&ProjectedBlock> {
    let mut out = Vec::new();
    for block in blocks {
        block.visit(&mut out);
    }
    out
}

/// Thread id → the text its comment mark currently covers. A thread whose
/// anchor text was deleted is absent: it is detached, not lost.
pub fn comment_anchors(blocks: &[ProjectedBlock]) -> BTreeMap<String, AnchorLocation> {
    let mut anchors: BTreeMap<String, AnchorLocation> = BTreeMap::new();
    for block in flatten(blocks) {
        for run in &block.content {
            for thread in &run.threads {
                let entry = anchors
                    .entry(thread.clone())
                    .or_insert_with(|| AnchorLocation {
                        block_id: block.id.clone(),
                        text: String::new(),
                    });
                entry.text.push_str(&run.text);
            }
        }
    }
    anchors
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorLocation {
    pub block_id: String,
    pub text: String,
}

/// A new block an agent adds. Text is plain; formatting stays the person's.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct NewBlock {
    #[serde(rename = "type", default = "default_block_type")]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub props: Map<String, Value>,
}

fn default_block_type() -> String {
    "paragraph".into()
}

/// A targeted edit. Every edit of existing text names the text it expects.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BlockOp {
    /// Replace a block's text. Only the range that differs between
    /// `expected_text` and `text` changes, so anchors and formatting outside
    /// it are kept.
    ReplaceText {
        block_id: String,
        expected_text: String,
        text: String,
    },
    /// Insert a block after `after` (a sibling), as the last child of
    /// `parent`, or at the end of the document when neither is given.
    InsertBlock {
        #[serde(default)]
        after: Option<String>,
        #[serde(default)]
        parent: Option<String>,
        block: NewBlock,
    },
    DeleteBlock {
        block_id: String,
        expected_text: String,
    },
    /// Set props on a block (`level`, `checked`, `language`, …).
    UpdateProps {
        block_id: String,
        props: Map<String, Value>,
    },
}

impl BlockOp {
    fn block_id(&self) -> Option<&str> {
        match self {
            Self::ReplaceText { block_id, .. }
            | Self::DeleteBlock { block_id, .. }
            | Self::UpdateProps { block_id, .. } => Some(block_id),
            Self::InsertBlock { .. } => None,
        }
    }
}

/// A precondition that no longer holds: the block's current content, so the
/// agent can retry against what the person actually wrote.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditConflict {
    pub op_index: usize,
    pub block_id: String,
    pub expected_text: String,
    pub current: Option<ProjectedBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedEdit {
    /// Ids of the blocks each operation touched or created, in order.
    pub block_ids: Vec<String>,
    /// The Yjs update the edit produced, for persistence and for clients.
    #[serde(skip)]
    pub update: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditOutcome {
    Applied(AppliedEdit),
    /// Nothing was written.
    Conflict(Vec<EditConflict>),
}

/// The authoritative document of one design session.
pub struct DesignDocument {
    doc: Doc,
}

impl Default for DesignDocument {
    fn default() -> Self {
        Self::new()
    }
}

impl DesignDocument {
    pub fn new() -> Self {
        Self { doc: Doc::new() }
    }

    /// Load a document from an encoded state (a Yjs update from nothing).
    pub fn from_state(state: &[u8]) -> Result<Self, DocumentError> {
        let document = Self::new();
        if !state.is_empty() {
            document.apply_unchecked(state)?;
        }
        Ok(document)
    }

    pub fn encode_state(&self) -> Vec<u8> {
        self.doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
    }

    pub fn state_vector(&self) -> Vec<u8> {
        self.doc.transact().state_vector().encode_v1()
    }

    /// What a client with `state_vector` lacks.
    pub fn diff_since(&self, state_vector: &[u8]) -> Result<Vec<u8>, DocumentError> {
        let vector = if state_vector.is_empty() {
            StateVector::default()
        } else {
            StateVector::decode_v1(state_vector).map_err(|error| {
                DocumentError::MalformedUpdate {
                    detail: format!("state vector: {error}"),
                }
            })?
        };
        Ok(self.doc.transact().encode_state_as_update_v1(&vector))
    }

    /// Whether the document holds everything `state_vector` describes.
    pub fn covers(&self, state_vector: &[u8]) -> Result<bool, DocumentError> {
        let theirs = StateVector::decode_v1(state_vector).map_err(|error| {
            DocumentError::MalformedUpdate {
                detail: format!("state vector: {error}"),
            }
        })?;
        let ours = self.doc.transact().state_vector();
        Ok(theirs.iter().all(|(client, clock)| ours.get(client) >= *clock))
    }

    fn apply_unchecked(&self, update: &[u8]) -> Result<(), DocumentError> {
        let update = Update::decode_v1(update).map_err(|error| DocumentError::MalformedUpdate {
            detail: error.to_string(),
        })?;
        self.doc
            .transact_mut()
            .apply_update(update)
            .map_err(|error| DocumentError::MalformedUpdate {
                detail: error.to_string(),
            })
    }

    /// Apply a client's update, refusing one that would leave the document
    /// outside the schema. It is tried on a copy first, so a refused update
    /// changes nothing. Returns whether the document changed.
    pub fn apply_client_update(&mut self, update: &[u8]) -> Result<bool, DocumentError> {
        if update.len() > MAX_UPDATE_BYTES {
            return Err(DocumentError::TooLarge {
                detail: format!("update of {} bytes exceeds {MAX_UPDATE_BYTES}", update.len()),
            });
        }
        let before = self.state_vector();
        let trial = Self::from_state(&self.encode_state())?;
        trial.apply_unchecked(update)?;
        trial.project()?;
        let size = trial.encode_state().len();
        if size > MAX_DOCUMENT_BYTES {
            return Err(DocumentError::TooLarge {
                detail: format!("document of {size} bytes would exceed {MAX_DOCUMENT_BYTES}"),
            });
        }
        self.apply_unchecked(update)?;
        Ok(self.state_vector() != before)
    }

    /// The document as blocks, read from a detached copy of its state.
    pub fn project(&self) -> Result<Vec<ProjectedBlock>, DocumentError> {
        let copy = Doc::new();
        copy.transact_mut()
            .apply_update(
                Update::decode_v1(&self.encode_state()).map_err(|error| {
                    DocumentError::MalformedUpdate {
                        detail: error.to_string(),
                    }
                })?,
            )
            .map_err(|error| DocumentError::MalformedUpdate {
                detail: error.to_string(),
            })?;
        let fragment = copy.get_or_insert_xml_fragment(FRAGMENT);
        let txn = copy.transact();
        project_fragment(&txn, &fragment)
    }

    /// Apply targeted edits atomically: every precondition is checked
    /// against the current document first, and a single conflict writes
    /// nothing.
    pub fn apply_ops(&mut self, ops: &[BlockOp]) -> Result<EditOutcome, DocumentError> {
        if ops.is_empty() {
            return Err(DocumentError::InvalidOperation {
                detail: "no operations".into(),
            });
        }
        let projected = self.project()?;
        let by_id: HashMap<&str, &ProjectedBlock> = flatten(&projected)
            .into_iter()
            .map(|block| (block.id.as_str(), block))
            .collect();
        let mut conflicts = Vec::new();
        for (index, op) in ops.iter().enumerate() {
            validate_op(op, &by_id)?;
            let expected = match op {
                BlockOp::ReplaceText {
                    block_id,
                    expected_text,
                    ..
                }
                | BlockOp::DeleteBlock {
                    block_id,
                    expected_text,
                } => Some((block_id, expected_text)),
                _ => None,
            };
            if let Some((block_id, expected_text)) = expected {
                let current = by_id.get(block_id.as_str()).copied();
                if current.map(|block| &block.text) != Some(expected_text) {
                    conflicts.push(EditConflict {
                        op_index: index,
                        block_id: block_id.clone(),
                        expected_text: expected_text.clone(),
                        current: current.cloned(),
                    });
                }
            }
        }
        if !conflicts.is_empty() {
            return Ok(EditOutcome::Conflict(conflicts));
        }
        let top_level = projected.len();
        let deletes_top_level = ops
            .iter()
            .filter(|op| {
                matches!(op, BlockOp::DeleteBlock { block_id, .. }
                    if projected.iter().any(|block| &block.id == block_id))
            })
            .count();
        let inserts = ops
            .iter()
            .filter(|op| matches!(op, BlockOp::InsertBlock { .. }))
            .count();
        if deletes_top_level >= top_level && inserts == 0 {
            return Err(DocumentError::InvalidOperation {
                detail: "a document keeps at least one block".into(),
            });
        }

        let before = self.doc.transact().state_vector();
        let fragment = self.doc.get_or_insert_xml_fragment(FRAGMENT);
        let mut block_ids = Vec::with_capacity(ops.len());
        {
            let mut txn = self.doc.transact_mut();
            for op in ops {
                block_ids.push(apply_op(&mut txn, &fragment, op)?);
            }
        }
        let update = self.doc.transact().encode_state_as_update_v1(&before);
        Ok(EditOutcome::Applied(AppliedEdit { block_ids, update }))
    }

    /// A document with one empty paragraph, the shape BlockNote starts with.
    pub fn seed_empty(&mut self) -> Vec<u8> {
        let fragment = self.doc.get_or_insert_xml_fragment(FRAGMENT);
        let before = self.doc.transact().state_vector();
        {
            let mut txn = self.doc.transact_mut();
            if fragment.len(&txn) == 0 {
                let group = fragment.push_back(&mut txn, XmlElementPrelim::empty("blockGroup"));
                insert_container(
                    &mut txn,
                    &group,
                    0,
                    &new_block_id(),
                    &NewBlock {
                        kind: "paragraph".into(),
                        text: String::new(),
                        props: Map::new(),
                    },
                )
                .expect("a paragraph is always valid");
            }
        }
        self.doc.transact().encode_state_as_update_v1(&before)
    }
}

/// A BlockNote-style block id (a UUID v4 rendering).
pub fn new_block_id() -> String {
    #[cfg(test)]
    if let Some(id) = tests::next_fixed_block_id() {
        return id;
    }
    let hex = crate::artifacts::random_hex(16).unwrap_or_else(|_| {
        format!(
            "{:032x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        )
    });
    let mut chars: Vec<char> = hex.chars().collect();
    chars[12] = '4';
    chars[16] = ['8', '9', 'a', 'b'][(chars[16].to_digit(16).unwrap_or(0) % 4) as usize];
    let hex: String = chars.into_iter().collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

fn element_children<T: ReadTxn>(txn: &T, element: &impl XmlFragment) -> Vec<XmlOut> {
    element.children(txn).collect()
}

fn project_fragment<T: ReadTxn>(
    txn: &T,
    fragment: &XmlFragmentRef,
) -> Result<Vec<ProjectedBlock>, DocumentError> {
    let children = element_children(txn, fragment);
    match children.as_slice() {
        [] => Ok(Vec::new()),
        [XmlOut::Element(group)] if group.tag().as_ref() == "blockGroup" => {
            project_group(txn, group)
        }
        _ => Err(unsupported(
            "the document root must hold exactly one blockGroup",
        )),
    }
}

fn project_group<T: ReadTxn>(
    txn: &T,
    group: &XmlElementRef,
) -> Result<Vec<ProjectedBlock>, DocumentError> {
    element_children(txn, group)
        .into_iter()
        .map(|child| match child {
            XmlOut::Element(container) if container.tag().as_ref() == "blockContainer" => {
                project_container(txn, &container)
            }
            XmlOut::Element(other) => Err(unsupported(format!(
                "a blockGroup holds <{}>, not a blockContainer",
                other.tag()
            ))),
            _ => Err(unsupported("a blockGroup holds text")),
        })
        .collect()
}

fn attribute_json<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> Map<String, Value> {
    element
        .attributes(txn)
        .map(|(key, value)| (key.to_string(), out_to_json(&value)))
        // An unset optional prop (a numbered list's `start`) is stored as
        // undefined; the editor reads it as absent.
        .filter(|(_, value)| !value.is_null())
        .collect()
}

fn out_to_json(value: &Out) -> Value {
    match value {
        Out::Any(any) => any_to_json(any),
        // Element attributes are always plain values in this schema.
        _ => Value::Null,
    }
}

fn any_to_json(any: &Any) -> Value {
    match any {
        Any::Null | Any::Undefined => Value::Null,
        Any::Bool(value) => Value::Bool(*value),
        Any::Number(value) => {
            if value.fract() == 0.0 && value.abs() < 9.0e15 {
                Value::from(*value as i64)
            } else {
                serde_json::Number::from_f64(*value)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        Any::BigInt(value) => Value::from(*value),
        Any::String(value) => Value::String(value.to_string()),
        Any::Buffer(_) => Value::Null,
        Any::Array(values) => Value::Array(values.iter().map(any_to_json).collect()),
        Any::Map(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), any_to_json(value)))
                .collect(),
        ),
    }
}

fn json_to_any(value: &Value) -> Any {
    match value {
        Value::Null => Any::Null,
        Value::Bool(value) => Any::Bool(*value),
        Value::Number(number) => match number.as_i64() {
            Some(int) => Any::Number(int as f64),
            None => Any::Number(number.as_f64().unwrap_or(0.0)),
        },
        Value::String(value) => Any::String(value.as_str().into()),
        Value::Array(values) => Any::Array(values.iter().map(json_to_any).collect()),
        Value::Object(values) => Any::Map(Arc::new(
            values
                .iter()
                .map(|(key, value)| (key.clone(), json_to_any(value)))
                .collect(),
        )),
    }
}

fn project_container<T: ReadTxn>(
    txn: &T,
    container: &XmlElementRef,
) -> Result<ProjectedBlock, DocumentError> {
    let id = match container.get_attribute(txn, "id") {
        Some(Out::Any(Any::String(id))) if !id.is_empty() => id.to_string(),
        _ => return Err(unsupported("a blockContainer has no id")),
    };
    let children = element_children(txn, container);
    let (content, nested) = match children.as_slice() {
        [XmlOut::Element(content)] => (content, None),
        [XmlOut::Element(content), XmlOut::Element(group)]
            if group.tag().as_ref() == "blockGroup" =>
        {
            (content, Some(group))
        }
        _ => {
            return Err(unsupported(format!(
                "block {id} is not one content node and an optional blockGroup"
            )))
        }
    };
    let kind = content.tag().to_string();
    let Some(description) = SCHEMA.blocks.get(&kind) else {
        return Err(unsupported(format!("block {id} has unknown type <{kind}>")));
    };
    let props = attribute_json(txn, content);
    for key in props.keys() {
        if !description.props.contains_key(key) {
            return Err(unsupported(format!(
                "block {id} (<{kind}>) has unknown prop {key}"
            )));
        }
    }
    let runs = project_inline(txn, &id, &kind, &description.content, content)?;
    let children = match nested {
        Some(group) => project_group(txn, group)?,
        None => Vec::new(),
    };
    Ok(ProjectedBlock {
        text: runs.iter().map(|run| run.text.as_str()).collect(),
        id,
        kind,
        props,
        content: runs,
        children,
    })
}

fn project_inline<T: ReadTxn>(
    txn: &T,
    id: &str,
    kind: &str,
    content_kind: &str,
    content: &XmlElementRef,
) -> Result<Vec<ProjectedRun>, DocumentError> {
    let mut runs: Vec<ProjectedRun> = Vec::new();
    let mut push = |run: ProjectedRun| {
        if let Some(last) = runs.last_mut() {
            if last.styles == run.styles && last.href == run.href && last.threads == run.threads {
                last.text.push_str(&run.text);
                return;
            }
        }
        runs.push(run);
    };
    for child in element_children(txn, content) {
        if !holds_text(content_kind) {
            return Err(unsupported(format!(
                "block {id} (<{kind}>) holds content but its type has none"
            )));
        }
        match child {
            XmlOut::Text(text) => {
                for chunk in text.diff(txn, YChange::identity) {
                    let run = project_chunk(id, chunk)?;
                    if content_kind == "plain" && (!run.styles.is_empty() || run.href.is_some()) {
                        return Err(unsupported(format!(
                            "block {id} (<{kind}>) is plain text but holds formatting"
                        )));
                    }
                    push(run);
                }
            }
            XmlOut::Element(element) if element.tag().as_ref() == "hardBreak" => {
                push(ProjectedRun {
                    text: "\n".into(),
                    styles: Map::new(),
                    href: None,
                    threads: Vec::new(),
                });
            }
            XmlOut::Element(element) => {
                return Err(unsupported(format!(
                    "block {id} holds inline <{}>",
                    element.tag()
                )))
            }
            XmlOut::Fragment(_) => {
                return Err(unsupported(format!("block {id} holds a fragment")))
            }
        }
    }
    Ok(runs)
}

/// `inline` content is styled text; `plain` (a code block) is text that may
/// carry a comment anchor but no formatting; `none` holds nothing.
fn holds_text(content_kind: &str) -> bool {
    matches!(content_kind, "inline" | "plain")
}

fn project_chunk(id: &str, chunk: Diff<YChange>) -> Result<ProjectedRun, DocumentError> {
    let text = match &chunk.insert {
        Out::Any(Any::String(text)) => text.to_string(),
        _ => return Err(unsupported(format!("block {id} holds an embedded value"))),
    };
    let mut run = ProjectedRun {
        text,
        styles: Map::new(),
        href: None,
        threads: Vec::new(),
    };
    for (key, value) in chunk.attributes.iter().flat_map(|attrs| attrs.iter()) {
        if matches!(value, Any::Null | Any::Undefined) {
            continue;
        }
        let key = key.as_ref();
        if key.starts_with(COMMENT_ATTRIBUTE_PREFIX) {
            let Any::Map(mark) = value else {
                return Err(unsupported(format!("block {id} has a malformed comment mark")));
            };
            if let Some(Any::String(thread)) = mark.get("threadId") {
                if !thread.is_empty() {
                    run.threads.push(thread.to_string());
                }
            }
        } else if key == "link" {
            let href = match value {
                Any::Map(link) => match link.get("href") {
                    Some(Any::String(href)) => href.to_string(),
                    _ => String::new(),
                },
                _ => return Err(unsupported(format!("block {id} has a malformed link"))),
            };
            run.href = Some(href);
        } else if let Some(style) = SCHEMA.styles.get(key) {
            let styled = match (style.as_str(), value) {
                ("boolean", _) => Value::Bool(true),
                ("string", Any::Map(attrs)) => match attrs.get("stringValue") {
                    Some(Any::String(value)) => Value::String(value.to_string()),
                    _ => return Err(unsupported(format!("block {id} has malformed {key}"))),
                },
                _ => return Err(unsupported(format!("block {id} has malformed {key}"))),
            };
            run.styles.insert(key.to_string(), styled);
        } else {
            return Err(unsupported(format!("block {id} has unknown formatting {key}")));
        }
    }
    run.threads.sort();
    Ok(run)
}

// ---------------------------------------------------------------------------
// Editing
// ---------------------------------------------------------------------------

fn validate_props(kind: &str, props: &Map<String, Value>) -> Result<(), DocumentError> {
    let description = SCHEMA
        .blocks
        .get(kind)
        .ok_or_else(|| DocumentError::InvalidOperation {
            detail: format!("unknown block type {kind}"),
        })?;
    for (key, value) in props {
        let Some(prop) = description.props.get(key) else {
            return Err(DocumentError::InvalidOperation {
                detail: format!("{kind} has no prop {key}"),
            });
        };
        let type_ok = match prop.kind.as_str() {
            "string" => value.is_string(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            _ => true,
        };
        let value_ok = prop
            .values
            .as_ref()
            .is_none_or(|values| values.contains(value));
        if !type_ok || !value_ok {
            return Err(DocumentError::InvalidOperation {
                detail: format!("{kind}.{key} cannot be {value}"),
            });
        }
    }
    Ok(())
}

fn validate_op(op: &BlockOp, by_id: &HashMap<&str, &ProjectedBlock>) -> Result<(), DocumentError> {
    if let Some(block_id) = op.block_id() {
        if !by_id.contains_key(block_id) {
            return Err(DocumentError::BlockNotFound {
                block_id: block_id.to_string(),
            });
        }
    }
    match op {
        BlockOp::ReplaceText { block_id, text, .. } => {
            let block = by_id[block_id.as_str()];
            if !holds_text(&SCHEMA.blocks[&block.kind].content) {
                return Err(DocumentError::InvalidOperation {
                    detail: format!("block {block_id} ({}) holds no text", block.kind),
                });
            }
            if block.content.iter().any(|run| run.text == "\n") || text.contains('\n') {
                return Err(DocumentError::InvalidOperation {
                    detail: format!(
                        "block {block_id}: line breaks inside a block are not supported; insert a block instead"
                    ),
                });
            }
        }
        BlockOp::InsertBlock {
            after,
            parent,
            block,
        } => {
            if after.is_some() && parent.is_some() {
                return Err(DocumentError::InvalidOperation {
                    detail: "insert_block takes `after` or `parent`, not both".into(),
                });
            }
            for anchor in after.iter().chain(parent.iter()) {
                if !by_id.contains_key(anchor.as_str()) {
                    return Err(DocumentError::BlockNotFound {
                        block_id: anchor.clone(),
                    });
                }
            }
            validate_props(&block.kind, &block.props)?;
            if block.text.contains('\n') {
                return Err(DocumentError::InvalidOperation {
                    detail: "a block's text is one line; insert one block per paragraph".into(),
                });
            }
            if !block.text.is_empty() && !holds_text(&SCHEMA.blocks[&block.kind].content) {
                return Err(DocumentError::InvalidOperation {
                    detail: format!("a {} holds no text", block.kind),
                });
            }
        }
        BlockOp::UpdateProps { block_id, props } => {
            validate_props(&by_id[block_id.as_str()].kind, props)?;
        }
        BlockOp::DeleteBlock { .. } => {}
    }
    Ok(())
}

/// The container element for a block id, with its parent group and index.
fn find_container<T: ReadTxn>(
    txn: &T,
    group: &XmlElementRef,
    block_id: &str,
) -> Option<(XmlElementRef, XmlElementRef, u32)> {
    for (index, child) in element_children(txn, group).into_iter().enumerate() {
        let XmlOut::Element(container) = child else {
            continue;
        };
        if matches!(container.get_attribute(txn, "id"), Some(Out::Any(Any::String(id))) if id.as_ref() == block_id)
        {
            return Some((group.clone(), container, index as u32));
        }
        if let Some(XmlOut::Element(nested)) = container.get(txn, 1) {
            if let Some(found) = find_container(txn, &nested, block_id) {
                return Some(found);
            }
        }
    }
    None
}

fn root_group<T: ReadTxn>(txn: &T, fragment: &XmlFragmentRef) -> Result<XmlElementRef, DocumentError> {
    match fragment.get(txn, 0) {
        Some(XmlOut::Element(group)) => Ok(group),
        _ => Err(unsupported("the document has no blockGroup")),
    }
}

fn content_element<T: ReadTxn>(txn: &T, container: &XmlElementRef) -> Result<XmlElementRef, DocumentError> {
    match container.get(txn, 0) {
        Some(XmlOut::Element(content)) => Ok(content),
        _ => Err(unsupported("a block has no content node")),
    }
}

fn apply_op(
    txn: &mut yrs::TransactionMut,
    fragment: &XmlFragmentRef,
    op: &BlockOp,
) -> Result<String, DocumentError> {
    let root = root_group(txn, fragment)?;
    let locate = |txn: &yrs::TransactionMut, block_id: &str| {
        find_container(txn, &root, block_id).ok_or_else(|| DocumentError::BlockNotFound {
            block_id: block_id.to_string(),
        })
    };
    match op {
        BlockOp::ReplaceText {
            block_id,
            expected_text,
            text,
        } => {
            let (_, container, _) = locate(txn, block_id)?;
            let content = content_element(txn, &container)?;
            replace_text(txn, &content, expected_text, text)?;
            Ok(block_id.clone())
        }
        BlockOp::InsertBlock {
            after,
            parent,
            block,
        } => {
            let id = new_block_id();
            match (after, parent) {
                (Some(after), _) => {
                    let (group, _, index) = locate(txn, after)?;
                    insert_container(txn, &group, index + 1, &id, block)?;
                }
                (None, Some(parent)) => {
                    let (_, container, _) = locate(txn, parent)?;
                    let group = match container.get(txn, 1) {
                        Some(XmlOut::Element(group)) => group,
                        _ => container.push_back(txn, XmlElementPrelim::empty("blockGroup")),
                    };
                    let index = group.len(txn);
                    insert_container(txn, &group, index, &id, block)?;
                }
                (None, None) => {
                    let index = root.len(txn);
                    insert_container(txn, &root, index, &id, block)?;
                }
            }
            Ok(id)
        }
        BlockOp::DeleteBlock { block_id, .. } => {
            let (group, _, index) = locate(txn, block_id)?;
            group.remove_range(txn, index, 1);
            // A nested group left empty is not valid ProseMirror content.
            if group.len(txn) == 0 {
                if let Some(XmlOut::Element(owner)) = group.parent() {
                    if owner.tag().as_ref() == "blockContainer" {
                        owner.remove_range(txn, 1, 1);
                    }
                }
            }
            Ok(block_id.clone())
        }
        BlockOp::UpdateProps { block_id, props } => {
            let (_, container, _) = locate(txn, block_id)?;
            let content = content_element(txn, &container)?;
            for (key, value) in props {
                content.insert_attribute(txn, key.as_str(), json_to_any(value));
            }
            Ok(block_id.clone())
        }
    }
}

fn insert_container(
    txn: &mut yrs::TransactionMut,
    group: &XmlElementRef,
    index: u32,
    id: &str,
    block: &NewBlock,
) -> Result<(), DocumentError> {
    let description = SCHEMA
        .blocks
        .get(&block.kind)
        .ok_or_else(|| DocumentError::InvalidOperation {
            detail: format!("unknown block type {}", block.kind),
        })?;
    let container = group.insert(txn, index, XmlElementPrelim::empty("blockContainer"));
    container.insert_attribute(txn, "id", Any::String(id.into()));
    let content = container.push_back(txn, XmlElementPrelim::empty(block.kind.as_str()));
    // Every prop, typed as the editor writes it (numbers stay numbers), so
    // y-prosemirror reads exactly what BlockNote would have produced.
    for (key, prop) in &description.props {
        // A prop without a default (a numbered list's `start`) is absent until
        // set, as BlockNote writes it.
        match block.props.get(key).unwrap_or(&prop.default) {
            Value::Null => {}
            value => {
                content.insert_attribute(txn, key.as_str(), json_to_any(value));
            }
        }
    }
    if !block.text.is_empty() {
        content.push_back(txn, XmlTextPrelim::new(block.text.as_str()));
    }
    Ok(())
}

/// Formatting at each character of a block's text, for choosing what an
/// inserted range inherits.
fn char_attributes<T: ReadTxn>(txn: &T, text: &XmlTextRef) -> Vec<Attrs> {
    let mut out = Vec::new();
    for chunk in text.diff(txn, YChange::identity) {
        let attrs: Attrs = chunk.attributes.map(|attrs| *attrs).unwrap_or_default();
        if let Out::Any(Any::String(value)) = &chunk.insert {
            for _ in value.chars() {
                out.push(attrs.clone());
            }
        }
    }
    out
}

fn replace_text(
    txn: &mut yrs::TransactionMut,
    content: &XmlElementRef,
    expected: &str,
    replacement: &str,
) -> Result<(), DocumentError> {
    let text = match content.get(txn, 0) {
        Some(XmlOut::Text(text)) => {
            if content.len(txn) > 1 {
                return Err(DocumentError::InvalidOperation {
                    detail: "block holds more than one text node".into(),
                });
            }
            text
        }
        None => content.push_back(txn, XmlTextPrelim::new("")),
        Some(_) => {
            return Err(DocumentError::InvalidOperation {
                detail: "block content is not text".into(),
            })
        }
    };
    let old: Vec<char> = expected.chars().collect();
    let new: Vec<char> = replacement.chars().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let max_suffix = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let deleted = &old[prefix..old.len() - suffix];
    let inserted: String = new[prefix..new.len() - suffix].iter().collect();
    if deleted.is_empty() && inserted.is_empty() {
        return Ok(());
    }
    // Byte offsets: the document counts text in UTF-8 bytes (Yrs default).
    let byte_at = |chars: &[char], index: usize| -> u32 {
        chars[..index].iter().map(|c| c.len_utf8()).sum::<usize>() as u32
    };
    let start = byte_at(&old, prefix);
    let attributes = char_attributes(txn, &text);
    let inherited = inherited_attributes(&attributes, prefix, deleted.len());
    if !deleted.is_empty() {
        let len = deleted.iter().map(|c| c.len_utf8()).sum::<usize>() as u32;
        text.remove_range(txn, start, len);
    }
    if !inserted.is_empty() {
        text.insert_with_attributes(txn, start, &inserted, inherited);
    }
    Ok(())
}

/// What an inserted range at `prefix` should carry. Text that replaces a
/// range takes the formatting of the range's first character. Pure insertion
/// takes the preceding character's formatting (or the following one's at the
/// start), except that a comment anchor only grows when the insertion is
/// strictly inside it, the way the non-inclusive comment mark behaves while
/// typing in the editor.
fn inherited_attributes(chars: &[Attrs], prefix: usize, deleted: usize) -> Attrs {
    if deleted > 0 {
        return chars.get(prefix).cloned().unwrap_or_default();
    }
    let before = prefix.checked_sub(1).and_then(|index| chars.get(index));
    let after = chars.get(prefix);
    let mut attrs = before.or(after).cloned().unwrap_or_default();
    attrs.retain(|key, value| {
        if !key.starts_with(COMMENT_ATTRIBUTE_PREFIX) && key.as_ref() != "link" {
            return true;
        }
        let inside = |side: Option<&Attrs>| side.and_then(|attrs| attrs.get(key)) == Some(value);
        inside(before) && inside(after)
    });
    attrs
}

#[cfg(test)]
#[path = "document_tests.rs"]
mod tests;
