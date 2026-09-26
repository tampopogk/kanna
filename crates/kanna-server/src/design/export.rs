//! Rendering an approved design for the record: the document as Markdown
//! (what the repository keeps) and as a self-contained HTML page (the
//! snapshot's rendered output), and the hand-off summary.

use super::document::{ProjectedBlock, ProjectedRun};
use super::service::ThreadView;
use serde_json::Value;
use std::collections::BTreeMap;

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn run_markdown(run: &ProjectedRun, plain: bool) -> String {
    if plain {
        return run.text.clone();
    }
    let mut text = escape_markdown(&run.text);
    if run.text.trim().is_empty() {
        return text;
    }
    let flag = |name: &str| run.styles.get(name) == Some(&Value::Bool(true));
    if flag("code") {
        text = format!("`{}`", run.text.replace('`', "\u{2018}"));
    }
    if flag("bold") {
        text = format!("**{text}**");
    }
    if flag("italic") {
        text = format!("_{text}_");
    }
    if flag("strike") {
        text = format!("~~{text}~~");
    }
    if let Some(href) = &run.href {
        text = format!("[{text}]({href})");
    }
    text
}

fn block_markdown(block: &ProjectedBlock, depth: usize, number: &mut usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let plain = block.kind == "codeBlock";
    let text: String = block
        .content
        .iter()
        .map(|run| run_markdown(run, plain))
        .collect();
    if block.kind != "numberedListItem" {
        *number = 0;
    }
    let line = match block.kind.as_str() {
        "heading" => {
            let level = block.props.get("level").and_then(Value::as_u64).unwrap_or(1).clamp(1, 6);
            format!("{} {text}", "#".repeat(level as usize))
        }
        "bulletListItem" | "toggleListItem" => format!("{indent}- {text}"),
        "numberedListItem" => {
            *number += 1;
            format!("{indent}{}. {text}", number)
        }
        "checkListItem" => {
            let checked = block.props.get("checked") == Some(&Value::Bool(true));
            format!("{indent}- [{}] {text}", if checked { "x" } else { " " })
        }
        "quote" => format!("> {text}"),
        "codeBlock" => {
            let language = block
                .props
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or("");
            format!("```{language}\n{text}\n```")
        }
        "divider" => "---".to_string(),
        _ => format!("{indent}{text}"),
    };
    out.push_str(&line);
    out.push_str("\n\n");
    let mut child_number = 0;
    for child in &block.children {
        block_markdown(child, depth + 1, &mut child_number, out);
    }
}

/// The document as Markdown.
pub(crate) fn document_markdown(title: &str, blocks: &[ProjectedBlock]) -> String {
    let mut out = format!("# {}\n\n", escape_markdown(title));
    let mut number = 0;
    for block in blocks {
        block_markdown(block, 0, &mut number, &mut out);
    }
    out.trim_end().to_string() + "\n"
}

fn run_html(run: &ProjectedRun, numbers: &BTreeMap<String, i64>) -> String {
    let mut html = escape_html(&run.text).replace('\n', "<br>");
    let flag = |name: &str| run.styles.get(name) == Some(&Value::Bool(true));
    for (name, tag) in [
        ("code", "code"),
        ("bold", "strong"),
        ("italic", "em"),
        ("underline", "u"),
        ("strike", "s"),
    ] {
        if flag(name) {
            html = format!("<{tag}>{html}</{tag}>");
        }
    }
    if let Some(href) = &run.href {
        html = format!("<a href=\"{}\">{html}</a>", escape_html(href));
    }
    for thread in &run.threads {
        let label = numbers
            .get(thread)
            .map(|number| format!("#{number}"))
            .unwrap_or_default();
        html = format!("<mark class=\"anchor\" title=\"thread {label}\">{html}</mark>");
    }
    html
}

fn block_html(block: &ProjectedBlock, numbers: &BTreeMap<String, i64>, out: &mut String) {
    let content: String = block.content.iter().map(|run| run_html(run, numbers)).collect();
    let body = match block.kind.as_str() {
        "heading" => {
            let level = block.props.get("level").and_then(Value::as_u64).unwrap_or(1).clamp(1, 6);
            format!("<h{level}>{content}</h{level}>")
        }
        "bulletListItem" | "toggleListItem" => format!("<ul><li>{content}</li></ul>"),
        "numberedListItem" => format!("<ol><li>{content}</li></ol>"),
        "checkListItem" => {
            let checked = block.props.get("checked") == Some(&Value::Bool(true));
            format!(
                "<p class=\"check\"><input type=\"checkbox\" disabled{}> {content}</p>",
                if checked { " checked" } else { "" }
            )
        }
        "quote" => format!("<blockquote>{content}</blockquote>"),
        "codeBlock" => format!("<pre><code>{content}</code></pre>"),
        "divider" => "<hr>".to_string(),
        _ => format!("<p>{content}</p>"),
    };
    out.push_str(&body);
    if !block.children.is_empty() {
        out.push_str("<div class=\"children\">");
        for child in &block.children {
            block_html(child, numbers, out);
        }
        out.push_str("</div>");
    }
    out.push('\n');
}

/// The document as a self-contained page for the snapshot. Anchored text is
/// marked the way the editor marks it; no script, no external resource.
pub(crate) fn document_html(title: &str, blocks: &[ProjectedBlock], threads: &[ThreadView]) -> String {
    let numbers: BTreeMap<String, i64> = threads
        .iter()
        .map(|thread| (thread.id.clone(), thread.number))
        .collect();
    let mut body = String::new();
    for block in blocks {
        block_html(block, &numbers, &mut body);
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>{title}</title><style>\
         :root{{color-scheme:light dark;--fg:#1d1d1f;--bg:#fff;--anchor:rgba(123,79,240,.14);--line:#7b4ff0}}\
         @media (prefers-color-scheme:dark){{:root{{--fg:#ececf1;--bg:#18181b;--anchor:rgba(181,140,240,.22);--line:#b58cf0}}}}\
         body{{font:16px/1.6 system-ui,-apple-system,sans-serif;color:var(--fg);background:var(--bg);max-width:760px;margin:40px auto;padding:0 16px}}\
         mark.anchor{{background:var(--anchor);color:inherit;text-decoration:underline;text-decoration-color:var(--line);text-underline-offset:3px}}\
         .children{{margin-left:24px}} pre{{overflow:auto;padding:12px;border-radius:6px;background:rgba(127,127,127,.12)}}\
         ul,ol{{margin:4px 0}} blockquote{{border-left:3px solid var(--line);margin:8px 0;padding-left:12px}}\
         </style></head><body><h1>{title}</h1>\n{body}</body></html>\n",
        title = escape_html(title),
    )
}

/// The feedback, oldest first, as the record keeps it.
pub(crate) fn feedback_markdown(threads: &[ThreadView]) -> String {
    if threads.is_empty() {
        return "No feedback was given.\n".to_string();
    }
    let mut out = String::new();
    for thread in threads {
        let what = match &thread.anchor {
            Some(anchor) => format!(
                "on \u{201c}{}\u{201d}{}",
                anchor.quoted_text.as_deref().unwrap_or(""),
                if anchor.state == "detached" {
                    " (text since removed)"
                } else {
                    ""
                }
            ),
            None => "message to the agent".to_string(),
        };
        out.push_str(&format!(
            "### #{} {} — {}\n\n",
            thread.number,
            what,
            if thread.status == "resolved" {
                "resolved"
            } else {
                "open"
            }
        ));
        for comment in &thread.comments {
            let who = if comment.author == "agent" {
                "Agent"
            } else {
                "Person"
            };
            out.push_str(&format!("- **{who}:** {}\n", comment.body.replace('\n', " ")));
        }
        out.push('\n');
    }
    out
}

pub(crate) struct SummaryFacts<'a> {
    pub(crate) title: &'a str,
    pub(crate) task_id: &'a str,
    pub(crate) workflow_stage: &'a str,
    pub(crate) position: &'a str,
    pub(crate) epoch: i64,
    pub(crate) doc_revision: i64,
    pub(crate) doc_sha256: &'a str,
    pub(crate) source_commit: &'a str,
    pub(crate) artifact: Option<(&'a str, &'a str)>,
    pub(crate) approved_at: Option<&'a str>,
    pub(crate) next_stage: Option<&'a str>,
}

/// The hand-off summary the repository keeps beside the document.
pub(crate) fn summary_markdown(facts: &SummaryFacts<'_>, threads: &[ThreadView]) -> String {
    let open = threads.iter().filter(|thread| thread.status == "open").count();
    let mut out = format!(
        "# Design summary: {}\n\n\
         Approved for build{}. The approved design is `design.md` beside this file; \
         the software factory builds it properly from here{}.\n\n\
         | | |\n|---|---|\n\
         | Task | `{}` |\n| Design stage | {} (last position: {}) |\n| Design epoch | {} |\n\
         | Document | revision {}, sha256 `{}` |\n| Disposable repository commit | `{}` |\n",
        facts.title,
        facts
            .approved_at
            .map(|at| format!(" on {at}"))
            .unwrap_or_default(),
        facts
            .next_stage
            .map(|stage| format!(" (next stage: {stage})"))
            .unwrap_or_default(),
        facts.task_id,
        facts.workflow_stage,
        facts.position,
        facts.epoch,
        facts.doc_revision,
        facts.doc_sha256,
        facts.source_commit,
    );
    if let Some((repo_id, artifact_id)) = facts.artifact {
        out.push_str(&format!(
            "| Approved snapshot | artifact `{artifact_id}` in repository `{repo_id}`'s artifact store |\n"
        ));
    }
    out.push_str(&format!(
        "\n## Feedback\n\n{} thread{}, {} still open at approval.\n\n",
        threads.len(),
        if threads.len() == 1 { "" } else { "s" },
        open
    ));
    out.push_str(&feedback_markdown(threads));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: &str, text: &str) -> ProjectedBlock {
        ProjectedBlock {
            id: "b".into(),
            kind: kind.into(),
            props: serde_json::Map::new(),
            text: text.into(),
            content: vec![ProjectedRun {
                text: text.into(),
                styles: serde_json::Map::new(),
                href: None,
                threads: vec!["t1".into()],
            }],
            children: Vec::new(),
        }
    }

    #[test]
    fn markdown_and_html_carry_the_text_and_escape_it() {
        let blocks = vec![block("heading", "Title"), block("paragraph", "a <b> & *c*")];
        let markdown = document_markdown("Doc", &blocks);
        assert!(markdown.contains("# Title"));
        assert!(markdown.contains("a \\<b\\> & \\*c\\*"));
        let html = document_html("Doc", &blocks, &[]);
        assert!(html.contains("a &lt;b&gt; &amp; *c*"));
        assert!(html.contains("<mark class=\"anchor\""));
        assert!(!html.contains("<script"));
    }
}
