//! What `kanna_design_get` answers: the design as the agent needs it to act,
//! and nothing else.
//!
//! The person's surfaces read [`service::DesignView`], which carries the
//! bookkeeping they render (revisions, delivery ids, the stage chain). An
//! agent reads the whole document on every turn, so here a block is its id,
//! type and text; props appear only where they differ from the schema's
//! defaults, formatting runs only where the text has formatting, a link or a
//! comment anchor, and children only where there are some. Resolved threads
//! are a count unless asked for.

use serde::Serialize;
use serde_json::{Map, Value};

use super::document::{self, ProjectedBlock, ProjectedRun};
use super::service::{self, AnchorView, DesignError, DesignView, ElementAnchor, ThreadView};
use super::DesignRuntime;
use crate::db::Db;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentDesignView {
    pub(crate) task_id: String,
    /// Present only when the task has left its design stage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) in_design_stage: Option<bool>,
    pub(crate) position: String,
    /// The positions in workflow order.
    pub(crate) positions: Vec<AgentPosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scratch_repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) approval: Option<AgentApproval>,
    pub(crate) threads: Vec<AgentThread>,
    #[serde(skip_serializing_if = "is_zero")]
    pub(crate) resolved_threads: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) document: Option<Vec<AgentBlock>>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AgentPosition {
    pub(crate) name: String,
    pub(crate) label: String,
    /// What it shows: `mockup` (the HTML you publish for it) or `document`.
    pub(crate) shows: String,
    /// The artifact id of the HTML mockup it shows, once one is published.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) mockup: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentApproval {
    pub(crate) phase: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentThread {
    pub(crate) id: String,
    pub(crate) number: i64,
    pub(crate) kind: String,
    /// Present only for a resolved thread (read with `resolved`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) anchor: Option<AgentAnchor>,
    pub(crate) comments: Vec<AgentComment>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentAnchor {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) block_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) quote: Option<String>,
    /// Present only when the anchor is not in place: `pending` or
    /// `detached` text, or a pin on a replaced mockup (`outdated`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) state: Option<&'static str>,
    /// The anchored text now, when the person has changed it since.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) current_text: Option<String>,
    /// A pin: the mockup element the comment is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) element: Option<ElementAnchor>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AgentComment {
    pub(crate) author: String,
    pub(crate) body: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct AgentBlock {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) text: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub(crate) props: Map<String, Value>,
    /// The text's runs, only when one of them is formatted, a link or a
    /// comment anchor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) runs: Option<Vec<AgentRun>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) children: Vec<AgentBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct AgentRun {
    pub(crate) text: String,
    #[serde(skip_serializing_if = "Map::is_empty")]
    pub(crate) styles: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) href: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) threads: Vec<String>,
}

fn is_zero(count: &usize) -> bool {
    *count == 0
}

pub(crate) fn agent_view(
    db: &Db,
    runtime: &DesignRuntime,
    db_path: &str,
    task_id: &str,
    include_document: bool,
    include_resolved: bool,
) -> Result<AgentDesignView, DesignError> {
    let view = service::view(db, runtime, db_path, task_id, include_document)?;
    Ok(compact(view, include_resolved))
}

pub(crate) fn compact(view: DesignView, include_resolved: bool) -> AgentDesignView {
    let resolved_threads = view
        .threads
        .iter()
        .filter(|thread| thread.status != "open")
        .count();
    let threads = view
        .threads
        .into_iter()
        .filter(|thread| include_resolved || thread.status == "open")
        .map(compact_thread)
        .collect();
    AgentDesignView {
        task_id: view.task_id,
        in_design_stage: (!view.in_design_stage).then_some(false),
        position: view.position,
        positions: view
            .positions
            .into_iter()
            .map(|position| AgentPosition {
                name: position.name,
                label: position.label,
                shows: position.artifact,
                mockup: position.mockup.map(|mockup| mockup.artifact_id),
            })
            .collect(),
        scratch_repository: view.scratch_repository,
        approval: view.approval.map(|approval| AgentApproval {
            phase: approval.phase,
            stale: approval.stale,
            error: approval.error,
        }),
        threads,
        resolved_threads: if include_resolved {
            0
        } else {
            resolved_threads
        },
        document: view
            .document
            .map(|document| document.blocks.iter().map(compact_block).collect()),
    }
}

fn compact_thread(thread: ThreadView) -> AgentThread {
    AgentThread {
        id: thread.id,
        number: thread.number,
        kind: thread.kind,
        status: (thread.status != "open").then_some(thread.status),
        anchor: thread.anchor.map(compact_anchor),
        comments: thread
            .comments
            .into_iter()
            .map(|comment| AgentComment {
                author: comment.author,
                body: comment.body,
            })
            .collect(),
    }
}

fn compact_anchor(anchor: AnchorView) -> AgentAnchor {
    let current_text = anchor
        .current_text
        .filter(|current| Some(current) != anchor.quoted_text.as_ref());
    AgentAnchor {
        block_id: anchor.block_id,
        quote: anchor.quoted_text,
        state: (anchor.state != "attached").then_some(anchor.state),
        current_text,
        element: anchor.element,
    }
}

pub(crate) fn compact_block(block: &ProjectedBlock) -> AgentBlock {
    let props: Map<String, Value> = block
        .props
        .iter()
        .filter(|(name, value)| !document::is_default_prop(&block.kind, name, value))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let plain = block
        .content
        .iter()
        .all(|run| run.styles.is_empty() && run.href.is_none() && run.threads.is_empty());
    AgentBlock {
        id: block.id.clone(),
        kind: block.kind.clone(),
        text: block.text.clone(),
        props,
        runs: (!plain).then(|| merge_runs(&block.content)),
        children: block.children.iter().map(compact_block).collect(),
    }
}

/// Adjacent runs that read the same are one run.
fn merge_runs(runs: &[ProjectedRun]) -> Vec<AgentRun> {
    let mut merged: Vec<AgentRun> = Vec::new();
    for run in runs {
        let next = AgentRun {
            text: run.text.clone(),
            styles: run.styles.clone(),
            href: run.href.clone(),
            threads: run.threads.clone(),
        };
        match merged.last_mut() {
            Some(last)
                if last.styles == next.styles
                    && last.href == next.href
                    && last.threads == next.threads =>
            {
                last.text.push_str(&next.text);
            }
            _ => merged.push(next),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(text: &str, styles: Value, threads: &[&str]) -> ProjectedRun {
        ProjectedRun {
            text: text.into(),
            styles: styles.as_object().unwrap().clone(),
            href: None,
            threads: threads.iter().map(|id| id.to_string()).collect(),
        }
    }

    fn block(id: &str, kind: &str, props: Value, content: Vec<ProjectedRun>) -> ProjectedBlock {
        ProjectedBlock {
            id: id.into(),
            kind: kind.into(),
            props: props.as_object().unwrap().clone(),
            text: content.iter().map(|run| run.text.as_str()).collect(),
            content,
            children: Vec::new(),
        }
    }

    #[test]
    fn a_plain_block_is_its_id_type_and_text() {
        let heading = block(
            "h1",
            "heading",
            json!({"backgroundColor": "default", "textColor": "default",
                   "textAlignment": "left", "level": 2, "isToggleable": false}),
            vec![run("Title", json!({}), &[])],
        );
        assert_eq!(
            serde_json::to_value(compact_block(&heading)).unwrap(),
            json!({"id": "h1", "type": "heading", "text": "Title", "props": {"level": 2}})
        );
    }

    #[test]
    fn formatting_and_anchors_keep_their_runs_merged_and_children_nest() {
        let mut parent = block(
            "p1",
            "paragraph",
            json!({"backgroundColor": "default", "textColor": "default", "textAlignment": "center"}),
            vec![
                run("Say ", json!({}), &[]),
                run("this", json!({"bold": true}), &["th-1"]),
                run(" and", json!({}), &[]),
                run(" that", json!({}), &[]),
            ],
        );
        parent.children.push(block(
            "c1",
            "bulletListItem",
            json!({"backgroundColor": "default", "textColor": "default", "textAlignment": "left"}),
            vec![run("child", json!({}), &[])],
        ));
        assert_eq!(
            serde_json::to_value(compact_block(&parent)).unwrap(),
            json!({
                "id": "p1",
                "type": "paragraph",
                "text": "Say this and that",
                "props": {"textAlignment": "center"},
                "runs": [
                    {"text": "Say "},
                    {"text": "this", "styles": {"bold": true}, "threads": ["th-1"]},
                    {"text": " and that"},
                ],
                "children": [{"id": "c1", "type": "bulletListItem", "text": "child"}],
            })
        );
    }
}
