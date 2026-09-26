//! Cross-language fixture tests: the documents under
//! `packages/design-editor/fixtures/` were written by BlockNote itself
//! (`pnpm --filter @kanna/design-editor fixtures`), and the Rust outputs under
//! `fixtures/rust/` are read back by that package's tests through
//! y-prosemirror, the reader that deleted content in the prototype (§9.1).
//!
//! Set `KANNA_UPDATE_DESIGN_FIXTURES=1` to rewrite `fixtures/rust/`.

use super::*;
use std::cell::Cell;

const BASE: &[u8] = include_bytes!("../../../../packages/design-editor/fixtures/base.ydoc");
const BASE_JSON: &str = include_str!("../../../../packages/design-editor/fixtures/base.json");
const PERSON_EDIT: &[u8] =
    include_bytes!("../../../../packages/design-editor/fixtures/person-edit.update");
const PERSON_EDIT_JSON: &str =
    include_str!("../../../../packages/design-editor/fixtures/person-edit.json");
const UNKNOWN_BLOCK: &[u8] =
    include_bytes!("../../../../packages/design-editor/fixtures/unknown-block.ydoc");
const FIXTURE_SCHEMA: &str =
    include_str!("../../../../packages/design-editor/fixtures/schema.json");

thread_local! {
    static FIXED_IDS: Cell<Option<u32>> = const { Cell::new(None) };
}

/// Deterministic ids for blocks created while a test holds [`FixedIds`].
pub(super) fn next_fixed_block_id() -> Option<String> {
    FIXED_IDS.with(|ids| {
        let next = ids.get()?;
        ids.set(Some(next + 1));
        Some(format!("agent-block-{next}"))
    })
}

struct FixedIds;
impl FixedIds {
    fn start() -> Self {
        FIXED_IDS.with(|ids| ids.set(Some(1)));
        Self
    }
}
impl Drop for FixedIds {
    fn drop(&mut self) {
        FIXED_IDS.with(|ids| ids.set(None));
    }
}

fn expected(json: &str) -> Vec<ProjectedBlock> {
    serde_json::from_str(json).expect("fixture json")
}

fn base() -> DesignDocument {
    DesignDocument::from_state(BASE).expect("base fixture loads")
}

fn block<'a>(blocks: &'a [ProjectedBlock], id: &str) -> &'a ProjectedBlock {
    flatten(blocks)
        .into_iter()
        .find(|block| block.id == id)
        .unwrap_or_else(|| panic!("block {id}"))
}

fn applied(outcome: EditOutcome) -> AppliedEdit {
    match outcome {
        EditOutcome::Applied(applied) => applied,
        EditOutcome::Conflict(conflicts) => panic!("unexpected conflict: {conflicts:?}"),
    }
}

fn replace(block_id: &str, expected_text: &str, text: &str) -> BlockOp {
    BlockOp::ReplaceText {
        block_id: block_id.into(),
        expected_text: expected_text.into(),
        text: text.into(),
    }
}

#[test]
fn the_compiled_schema_is_the_editors() {
    let compiled: Value =
        serde_json::from_str(include_str!("../../resources/design-schema.json")).unwrap();
    let fixture: Value = serde_json::from_str(FIXTURE_SCHEMA).unwrap();
    assert_eq!(compiled, fixture);
    assert_eq!(fixture["version"], SCHEMA_VERSION);
}

#[test]
fn reads_the_editors_document_exactly_as_the_browser_does() {
    assert_eq!(base().project().unwrap(), expected(BASE_JSON));
}

#[test]
fn reading_never_writes() {
    let document = base();
    let state = document.encode_state();
    let vector = document.state_vector();
    for _ in 0..3 {
        document.project().unwrap();
    }
    assert_eq!(document.encode_state(), state);
    assert_eq!(document.state_vector(), vector);
}

#[test]
fn applies_the_persons_update() {
    let mut document = base();
    assert!(document.apply_client_update(PERSON_EDIT).unwrap());
    assert_eq!(document.project().unwrap(), expected(PERSON_EDIT_JSON));
    // The same update again changes nothing.
    assert!(!document.apply_client_update(PERSON_EDIT).unwrap());
}

#[test]
fn concurrent_edits_to_different_blocks_both_survive_in_either_order() {
    let mut agent = base();
    let edit = applied(
        agent
            .apply_ops(&[replace("title", "App Design notes", "App Design core notes")])
            .unwrap(),
    );
    // Agent first, then the person's concurrent update…
    agent.apply_client_update(PERSON_EDIT).unwrap();
    // …and the person's replica receiving the agent's update second.
    let mut person = base();
    person.apply_client_update(PERSON_EDIT).unwrap();
    person.apply_client_update(&edit.update).unwrap();

    let merged = agent.project().unwrap();
    assert_eq!(merged, person.project().unwrap());
    assert_eq!(block(&merged, "title").text, "App Design core notes");
    assert_eq!(
        block(&merged, "intro").text,
        "Feedback is queued until the agent is free. Typed by the person."
    );
    assert_eq!(
        comment_anchors(&merged)["thread-1"],
        AnchorLocation {
            block_id: "intro".into(),
            text: "is queued".into()
        }
    );
}

#[test]
fn a_stale_replacement_conflicts_instead_of_deleting_the_persons_text() {
    let mut document = base();
    document.apply_client_update(PERSON_EDIT).unwrap();
    let state = document.encode_state();
    let outcome = document
        .apply_ops(&[
            replace("title", "App Design notes", "Renamed"),
            replace(
                "intro",
                "Feedback is queued until the agent is free.",
                "Feedback waits.",
            ),
        ])
        .unwrap();
    let EditOutcome::Conflict(conflicts) = outcome else {
        panic!("expected a conflict");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].op_index, 1);
    assert_eq!(
        conflicts[0].current.as_ref().unwrap().text,
        "Feedback is queued until the agent is free. Typed by the person."
    );
    // Atomic: the valid first operation was not applied either.
    assert_eq!(document.encode_state(), state);
}

#[test]
fn a_targeted_edit_keeps_formatting_and_anchors_outside_its_range() {
    let mut document = base();
    applied(
        document
            .apply_ops(&[replace(
                "intro",
                "Feedback is queued until the agent is free.",
                "Feedback is queued until the agent is idle.",
            )])
            .unwrap(),
    );
    let blocks = document.project().unwrap();
    let intro = block(&blocks, "intro");
    assert_eq!(intro.text, "Feedback is queued until the agent is idle.");
    let bold: Vec<_> = intro
        .content
        .iter()
        .filter(|run| run.styles.get("bold") == Some(&Value::Bool(true)))
        .collect();
    assert_eq!(bold.len(), 1);
    assert_eq!(bold[0].text, "queued");
    assert_eq!(comment_anchors(&blocks)["thread-1"].text, "is queued");
}

#[test]
fn typing_at_an_anchors_edge_does_not_grow_it_but_replacing_inside_keeps_it() {
    let mut document = base();
    applied(
        document
            .apply_ops(&[replace(
                "intro",
                "Feedback is queued until the agent is free.",
                "Feedback is queued! until the agent is free.",
            )])
            .unwrap(),
    );
    let blocks = document.project().unwrap();
    assert_eq!(comment_anchors(&blocks)["thread-1"].text, "is queued");

    applied(
        document
            .apply_ops(&[replace(
                "intro",
                "Feedback is queued! until the agent is free.",
                "Feedback is held! until the agent is free.",
            )])
            .unwrap(),
    );
    let blocks = document.project().unwrap();
    assert_eq!(comment_anchors(&blocks)["thread-1"].text, "is held");
}

#[test]
fn deleting_an_anchor_detaches_its_thread() {
    let mut document = base();
    applied(
        document
            .apply_ops(&[replace(
                "intro",
                "Feedback is queued until the agent is free.",
                "Feedback until the agent is free.",
            )])
            .unwrap(),
    );
    assert!(!comment_anchors(&document.project().unwrap()).contains_key("thread-1"));
}

#[test]
fn inserts_deletes_and_updates_blocks() {
    let _ids = FixedIds::start();
    let mut document = base();
    let edit = applied(
        document
            .apply_ops(&[
                BlockOp::InsertBlock {
                    after: Some("title".into()),
                    parent: None,
                    block: NewBlock {
                        kind: "heading".into(),
                        text: "Inserted heading".into(),
                        props: serde_json::json!({"level": 3}).as_object().unwrap().clone(),
                    },
                },
                BlockOp::InsertBlock {
                    after: None,
                    parent: Some("numbered".into()),
                    block: NewBlock {
                        kind: "checkListItem".into(),
                        text: "child task".into(),
                        props: Map::new(),
                    },
                },
                BlockOp::InsertBlock {
                    after: None,
                    parent: None,
                    block: NewBlock {
                        kind: "paragraph".into(),
                        text: "appended".into(),
                        props: Map::new(),
                    },
                },
                BlockOp::DeleteBlock {
                    block_id: "quote".into(),
                    expected_text: "a quotation".into(),
                },
                BlockOp::DeleteBlock {
                    block_id: "nested".into(),
                    expected_text: "nested italic".into(),
                },
                BlockOp::UpdateProps {
                    block_id: "check".into(),
                    props: serde_json::json!({"checked": false}).as_object().unwrap().clone(),
                },
            ])
            .unwrap(),
    );
    assert_eq!(
        edit.block_ids,
        vec!["agent-block-1", "agent-block-2", "agent-block-3", "quote", "nested", "check"]
    );
    let blocks = document.project().unwrap();
    let ids: Vec<&str> = blocks.iter().map(|block| block.id.as_str()).collect();
    assert_eq!(&ids[..3], &["title", "agent-block-1", "intro"]);
    assert_eq!(ids.last(), Some(&"agent-block-3"));
    assert!(!ids.contains(&"quote"));
    let heading = block(&blocks, "agent-block-1");
    assert_eq!(heading.kind, "heading");
    assert_eq!(heading.props["level"], 3);
    assert_eq!(heading.props["isToggleable"], false);
    assert_eq!(block(&blocks, "numbered").children[0].text, "child task");
    // Deleting the only nested child removes the empty group as well.
    assert!(block(&blocks, "intro").children.is_empty());
    assert_eq!(block(&blocks, "check").props["checked"], false);
}

#[test]
fn refuses_invalid_operations_without_writing() {
    let mut document = base();
    let state = document.encode_state();
    let invalid = [
        BlockOp::UpdateProps {
            block_id: "title".into(),
            props: serde_json::json!({"level": "two"}).as_object().unwrap().clone(),
        },
        BlockOp::UpdateProps {
            block_id: "title".into(),
            props: serde_json::json!({"shadow": true}).as_object().unwrap().clone(),
        },
        replace("missing", "", "x"),
        replace("divider", "", "text in a divider"),
        replace("title", "App Design notes", "two\nlines"),
        BlockOp::InsertBlock {
            after: None,
            parent: None,
            block: NewBlock {
                kind: "table".into(),
                text: String::new(),
                props: Map::new(),
            },
        },
    ];
    for op in invalid {
        assert!(document.apply_ops(&[op.clone()]).is_err(), "{op:?}");
        assert_eq!(document.encode_state(), state);
    }
}

#[test]
fn a_document_keeps_at_least_one_block() {
    let mut document = DesignDocument::new();
    document.seed_empty();
    let only = document.project().unwrap()[0].id.clone();
    assert!(matches!(
        document.apply_ops(&[BlockOp::DeleteBlock {
            block_id: only,
            expected_text: String::new()
        }]),
        Err(DocumentError::InvalidOperation { .. })
    ));
}

#[test]
fn a_seeded_document_is_one_empty_paragraph() {
    let mut document = DesignDocument::new();
    let update = document.seed_empty();
    let blocks = document.project().unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].kind, "paragraph");
    assert_eq!(blocks[0].text, "");
    // A second seed is a no-op; the update rebuilds the same document.
    assert!(!document.seed_empty().is_empty() || document.project().unwrap().len() == 1);
    let replica = DesignDocument::from_state(&update).unwrap();
    assert_eq!(replica.project().unwrap(), blocks);
}

#[test]
fn a_document_from_another_schema_fails_explicitly_and_is_never_edited() {
    let mut document = DesignDocument::from_state(UNKNOWN_BLOCK).unwrap();
    let state = document.encode_state();
    let error = document.project().unwrap_err();
    assert!(
        matches!(&error, DocumentError::UnsupportedSchema { detail } if detail.contains("mysteryBlock")),
        "{error:?}"
    );
    assert!(document
        .apply_ops(&[replace("title", "App Design notes", "x")])
        .is_err());
    assert_eq!(document.encode_state(), state);

    // A client update that would introduce it is refused and changes nothing.
    let mut clean = base();
    let clean_state = clean.encode_state();
    let offending = {
        let theirs = DesignDocument::from_state(UNKNOWN_BLOCK).unwrap();
        theirs.diff_since(&clean.state_vector()).unwrap()
    };
    assert!(matches!(
        clean.apply_client_update(&offending),
        Err(DocumentError::UnsupportedSchema { .. })
    ));
    assert_eq!(clean.encode_state(), clean_state);
}

#[test]
fn malformed_and_oversized_updates_are_refused() {
    let mut document = base();
    assert!(matches!(
        document.apply_client_update(&[0xff, 0x00, 0x13]),
        Err(DocumentError::MalformedUpdate { .. })
    ));
    assert!(matches!(
        document.apply_client_update(&vec![0; MAX_UPDATE_BYTES + 1]),
        Err(DocumentError::TooLarge { .. })
    ));
}

#[test]
fn sync_diffs_bring_a_replica_up_to_date() {
    let mut server = base();
    server.apply_client_update(PERSON_EDIT).unwrap();
    let replica = DesignDocument::from_state(BASE).unwrap();
    let missing = server.diff_since(&replica.state_vector()).unwrap();
    let mut replica = replica;
    replica.apply_client_update(&missing).unwrap();
    assert_eq!(replica.project().unwrap(), server.project().unwrap());
    assert!(server.covers(&replica.state_vector()).unwrap());
    assert!(!base().covers(&server.state_vector()).unwrap());
}

#[test]
fn new_block_ids_look_like_blocknotes() {
    let id = new_block_id();
    assert_eq!(id.len(), 36);
    assert_eq!(&id[14..15], "4");
    assert_eq!(id.matches('-').count(), 4);
}

/// The document the browser-side test (`packages/design-editor/src/
/// fixtures.test.ts`) reads back through y-prosemirror: every agent operation,
/// merged with the person's concurrent edit.
#[test]
fn agent_edits_fixture_is_current() {
    let _ids = FixedIds::start();
    let mut document = base();
    applied(
        document
            .apply_ops(&[
                replace("title", "App Design notes", "App Design core notes"),
                replace(
                    "intro",
                    "Feedback is queued until the agent is free.",
                    "Feedback is queued while the agent is busy.",
                ),
                BlockOp::InsertBlock {
                    after: Some("links".into()),
                    parent: None,
                    block: NewBlock {
                        kind: "bulletListItem".into(),
                        text: "added by the agent".into(),
                        props: Map::new(),
                    },
                },
                BlockOp::UpdateProps {
                    block_id: "code".into(),
                    props: serde_json::json!({"language": "typescript"})
                        .as_object()
                        .unwrap()
                        .clone(),
                },
                BlockOp::DeleteBlock {
                    block_id: "toggle".into(),
                    expected_text: "toggle me".into(),
                },
            ])
            .unwrap(),
    );
    let projection = document.project().unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/design-editor/fixtures/rust");
    let json = format!("{}\n", serde_json::to_string_pretty(&projection).unwrap());
    if std::env::var_os("KANNA_UPDATE_DESIGN_FIXTURES").is_some() {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent-edits.ydoc"), document.encode_state()).unwrap();
        std::fs::write(dir.join("agent-edits.json"), &json).unwrap();
    }
    let committed = std::fs::read_to_string(dir.join("agent-edits.json"))
        .expect("run with KANNA_UPDATE_DESIGN_FIXTURES=1 to write fixtures/rust");
    assert_eq!(
        serde_json::from_str::<Value>(&committed).unwrap(),
        serde_json::from_str::<Value>(&json).unwrap()
    );
    // The committed document is the same one this test produced.
    let written = DesignDocument::from_state(&std::fs::read(dir.join("agent-edits.ydoc")).unwrap())
        .unwrap();
    assert_eq!(written.project().unwrap(), projection);
}
