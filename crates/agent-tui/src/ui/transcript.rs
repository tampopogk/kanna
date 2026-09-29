//! Lays the transcript out as wrapped lines, remembering which line range
//! belongs to each selectable target. Per-entry results are cached.

use std::collections::HashMap;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::sanitize::{sanitize, sanitize_line};
use super::skins::Theme;
use super::text::{ellipsize, to_line, width, wrap_spans};
use crate::app::transcript::*;
use crate::app::{format_duration, App, Focus, Phase, Target};
use crate::protocol::{ChoiceTone, NoticeLevel, ToolKind, ToolStatus};

/// Content starts after a 1-column selection gutter and a 2-column indent.
const INDENT: usize = 3;
const MAX_DETAIL_LINES: usize = 40;

pub struct Layout {
    pub lines: Vec<Line<'static>>,
    pub ranges: Vec<(Target, usize, usize)>,
}

type Ranges = Vec<(Target, usize, usize)>;

#[derive(Default)]
pub struct RenderCache {
    entries: HashMap<EntryId, (CacheKey, Vec<Line<'static>>, Ranges)>,
}

#[derive(PartialEq, Clone)]
struct CacheKey {
    rev: u64,
    width: u16,
    theme: String,
    focused_card: bool,
    query: String,
}

pub fn layout(app: &App, theme: &Theme, width: u16, cache: &mut RenderCache) -> Layout {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges = Vec::new();
    let query = app
        .search
        .as_ref()
        .map(|s| s.query.to_lowercase())
        .unwrap_or_default();
    if app.transcript.discarded > 0 {
        lines.push(plain_line(
            theme,
            format!(
                "… {} older entries discarded from display — the session keeps its context",
                app.transcript.discarded
            ),
            Style::default().fg(theme.muted).bg(theme.bg),
            width,
        ));
    }
    let skin_key = format!("{:?}{:?}", theme.skin, theme.mode);
    let mut first = true;
    for e in app.transcript.iter() {
        if !first {
            lines.push(blank(theme, width));
        }
        first = false;
        let key = CacheKey {
            rev: e.rev,
            width,
            theme: skin_key.clone(),
            focused_card: app.focus == Focus::Card(e.id),
            query: query.clone(),
        };
        let cached = cache.entries.get(&e.id).filter(|(k, _, _)| *k == key);
        let (elines, eranges) = match cached {
            Some((_, l, r)) => (l.clone(), r.clone()),
            None => {
                let (l, r) = render_entry(app, e, theme, width, &query);
                cache.entries.insert(e.id, (key, l.clone(), r.clone()));
                (l, r)
            }
        };
        let base = lines.len();
        ranges.extend(eranges.into_iter().map(|(t, a, b)| (t, a + base, b + base)));
        lines.extend(elines);
    }
    if cache.entries.len() > app.transcript.len() + 64 {
        let live: std::collections::HashSet<EntryId> = app.transcript.ids().into_iter().collect();
        cache.entries.retain(|k, _| live.contains(k));
    }
    // Selection gutter.
    let marked = match app.focus {
        Focus::Transcript | Focus::Search => app.view.selected,
        _ => None,
    };
    if let Some(sel) = marked {
        if let Some((_, a, b)) = ranges.iter().find(|(t, _, _)| *t == sel) {
            for l in &mut lines[*a..*b] {
                mark_gutter(l, theme);
            }
        }
    }
    Layout { lines, ranges }
}

/// Replaces the first column of a line with the selection bar.
fn mark_gutter(l: &mut Line<'static>, theme: &Theme) {
    let bar = Span::styled("▎", Style::default().fg(theme.accent).bg(theme.bg));
    let Some(first) = l.spans.first_mut() else {
        return;
    };
    let mut chars = first.content.chars();
    chars.next();
    let rest: String = chars.collect();
    if rest.is_empty() {
        *first = bar;
    } else {
        first.content = rest.into();
        l.spans.insert(0, bar);
    }
}

fn blank(theme: &Theme, width: u16) -> Line<'static> {
    Line::from(Span::styled(
        " ".repeat(width as usize),
        Style::default().bg(theme.bg),
    ))
}

fn plain_line(theme: &Theme, text: String, style: Style, width: u16) -> Line<'static> {
    let t = ellipsize(&text, (width as usize).saturating_sub(INDENT + 1));
    let pad = (width as usize).saturating_sub(INDENT + super::text::width(&t));
    Line::from(vec![
        Span::styled(" ".repeat(INDENT), Style::default().bg(theme.bg)),
        Span::styled(t, style),
        Span::styled(" ".repeat(pad), Style::default().bg(theme.bg)),
    ])
}

struct Builder<'a> {
    theme: &'a Theme,
    width: usize,
    query: &'a str,
    lines: Vec<Line<'static>>,
}

impl<'a> Builder<'a> {
    /// Adds wrapped text: `indent` columns of `bg`, then segments.
    fn text(
        &mut self,
        indent: usize,
        segs: Vec<(String, Style)>,
        bg: ratatui::style::Color,
        prefix: Option<(String, Style)>,
    ) {
        let prefix_w = prefix.as_ref().map_or(0, |p| width(&p.0));
        let avail = self.width.saturating_sub(indent + prefix_w + 1).max(8);
        for wrapped in wrap_spans(&segs, avail) {
            let mut spans = vec![
                Span::styled(" ", Style::default().bg(self.theme.bg)),
                Span::styled(
                    " ".repeat(indent.saturating_sub(1)),
                    Style::default().bg(if prefix.is_some() { self.theme.bg } else { bg }),
                ),
            ];
            if let Some((p, s)) = &prefix {
                spans.push(Span::styled(p.clone(), *s));
            }
            let mut used = indent + prefix_w;
            for (t, s) in wrapped {
                used += width(&t);
                spans.extend(highlight(&t, s, self.query, self.theme));
            }
            spans.push(Span::styled(
                " ".repeat(self.width.saturating_sub(used)),
                Style::default().bg(bg),
            ));
            self.lines.push(Line::from(spans));
        }
    }

    fn para(&mut self, indent: usize, text: &str, style: Style) {
        for l in sanitize(text).split('\n') {
            self.text(indent, vec![(l.to_string(), style)], self.theme.bg, None);
        }
    }

    fn row(&mut self, indent: usize, segs: Vec<(String, Style)>) {
        let mut used = indent;
        let mut spans = vec![Span::styled(
            " ".repeat(indent),
            Style::default().bg(self.theme.bg),
        )];
        let avail = self.width.saturating_sub(indent + 1);
        for (t, s) in segs {
            let room = avail.saturating_sub(used - indent);
            if room == 0 {
                break;
            }
            let t = ellipsize(&t, room);
            used += width(&t);
            spans.extend(highlight(&t, s, self.query, self.theme));
        }
        spans.push(Span::styled(
            " ".repeat(self.width.saturating_sub(used)),
            Style::default().bg(self.theme.bg),
        ));
        self.lines.push(Line::from(spans));
    }

    fn block(&mut self, indent: usize, text: &str, style: Style, max: usize) {
        let text = sanitize(text);
        let all: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
        for l in all.iter().take(max) {
            self.text(indent, vec![(l.to_string(), style)], self.theme.bg, None);
        }
        if all.len() > max {
            self.row(
                indent,
                vec![(
                    format!(
                        "… {} more lines · Ctrl+R shows the raw JSON",
                        all.len() - max
                    ),
                    Style::default().fg(self.theme.muted).bg(self.theme.bg),
                )],
            );
        }
    }
}

fn highlight(text: &str, style: Style, query: &str, theme: &Theme) -> Vec<Span<'static>> {
    if query.is_empty() || text.is_empty() {
        return vec![Span::styled(text.to_string(), style)];
    }
    let lower = text.to_lowercase();
    if lower.len() != text.len() {
        return vec![Span::styled(text.to_string(), style)];
    }
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = lower[pos..].find(query) {
        let s = pos + i;
        let e = s + query.len();
        if !text.is_char_boundary(s) || !text.is_char_boundary(e) {
            break;
        }
        if s > pos {
            out.push(Span::styled(text[pos..s].to_string(), style));
        }
        out.push(Span::styled(
            text[s..e].to_string(),
            style.bg(theme.highlight).add_modifier(Modifier::BOLD),
        ));
        pos = e;
    }
    out.push(Span::styled(text[pos..].to_string(), style));
    out
}

fn render_entry(
    app: &App,
    e: &Entry,
    theme: &Theme,
    width: u16,
    query: &str,
) -> (Vec<Line<'static>>, Vec<(Target, usize, usize)>) {
    let mut b = Builder {
        theme,
        width: width as usize,
        query,
        lines: Vec::new(),
    };
    let mut ranges = Vec::new();
    let bg = Style::default().bg(theme.bg);
    let muted = bg.fg(theme.muted);
    match &e.kind {
        EntryKind::User { text } => {
            let s = Style::default().fg(theme.ink).bg(theme.user_bg);
            let bar = (
                "▌ ".to_string(),
                Style::default().fg(theme.accent).bg(theme.user_bg),
            );
            for l in sanitize(text).split('\n') {
                b.text(
                    1,
                    vec![(l.to_string(), s)],
                    theme.user_bg,
                    Some(bar.clone()),
                );
            }
        }
        EntryKind::Assistant { text, .. } => {
            let mut in_code = false;
            for l in sanitize(text).split('\n') {
                if l.trim_start().starts_with("```") {
                    in_code = !in_code;
                    b.text(INDENT, vec![(l.to_string(), muted)], theme.bg, None);
                    continue;
                }
                if in_code {
                    b.text(
                        INDENT,
                        vec![(
                            l.to_string(),
                            Style::default().fg(theme.ink).bg(theme.surface),
                        )],
                        theme.surface,
                        None,
                    );
                } else if let Some(h) = l
                    .strip_prefix("# ")
                    .or(l.strip_prefix("## "))
                    .or(l.strip_prefix("### "))
                {
                    b.text(
                        INDENT,
                        vec![(h.to_string(), bg.fg(theme.ink).add_modifier(Modifier::BOLD))],
                        theme.bg,
                        None,
                    );
                } else {
                    b.text(INDENT, inline_markdown(l, theme), theme.bg, None);
                }
            }
        }
        EntryKind::Tools(g) => {
            render_tools(&mut b, &mut ranges, e.id, g, theme);
            return (b.lines, ranges);
        }
        EntryKind::Approval(c) => render_approval(&mut b, app, e.id, c, theme),
        EntryKind::Question(c) => render_question(&mut b, app, e.id, c, theme),
        EntryKind::Notice { level, text } | EntryKind::TurnEnd { level, text } => {
            let style = match level {
                NoticeLevel::Info => muted,
                NoticeLevel::Warn => bg.fg(theme.warn),
                NoticeLevel::Error => bg.fg(theme.danger),
            };
            if let EntryKind::TurnEnd { .. } = e.kind {
                if let Some(rest) = text.strip_prefix('✓') {
                    b.text(
                        INDENT,
                        vec![("✓".into(), bg.fg(theme.accent)), (rest.to_string(), muted)],
                        theme.bg,
                        None,
                    );
                    ranges.push((Target::Entry(e.id), 0, b.lines.len()));
                    return (b.lines, ranges);
                }
            }
            b.para(INDENT, text, style);
        }
    }
    ranges.push((Target::Entry(e.id), 0, b.lines.len()));
    (b.lines, ranges)
}

fn inline_markdown(l: &str, theme: &Theme) -> Vec<(String, Style)> {
    let base = Style::default().fg(theme.ink).bg(theme.bg);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut bold = false;
    let mut code = false;
    let chars: Vec<char> = l.chars().collect();
    let mut i = 0;
    let style = |bold: bool, code: bool| {
        let mut s = base;
        if bold {
            s = s.add_modifier(Modifier::BOLD);
        }
        if code {
            s = s.bg(theme.surface).fg(theme.accent);
        }
        s
    };
    while i < chars.len() {
        if chars[i] == '`' {
            out.push((std::mem::take(&mut cur), style(bold, code)));
            code = !code;
            i += 1;
            continue;
        }
        if !code && chars[i] == '*' && chars.get(i + 1) == Some(&'*') {
            out.push((std::mem::take(&mut cur), style(bold, code)));
            bold = !bold;
            i += 2;
            continue;
        }
        cur.push(chars[i]);
        i += 1;
    }
    out.push((cur, style(bold, code)));
    out.retain(|(t, _)| !t.is_empty());
    if out.is_empty() {
        out.push((String::new(), base));
    }
    out
}

fn status_symbol(s: ToolStatus, theme: &Theme) -> (String, Style) {
    let bg = Style::default().bg(theme.bg);
    match s {
        ToolStatus::Succeeded => ("✓".into(), bg.fg(theme.accent)),
        ToolStatus::Failed => ("✕".into(), bg.fg(theme.danger)),
        ToolStatus::Declined => ("⊘".into(), bg.fg(theme.danger)),
        ToolStatus::Running => ("●".into(), bg.fg(theme.warn)),
        ToolStatus::Stopped => ("■".into(), bg.fg(theme.muted)),
    }
}

fn status_word(s: ToolStatus) -> &'static str {
    match s {
        ToolStatus::Succeeded => "succeeded",
        ToolStatus::Failed => "failed",
        ToolStatus::Declined => "declined",
        ToolStatus::Running => "running",
        ToolStatus::Stopped => "stopped",
    }
}

fn diff_counts(diff: &str) -> (usize, usize) {
    let mut add = 0;
    let mut del = 0;
    for l in diff.lines() {
        if l.starts_with("+++") || l.starts_with("---") {
            continue;
        }
        if l.starts_with('+') {
            add += 1;
        } else if l.starts_with('-') {
            del += 1;
        }
    }
    (add, del)
}

pub fn tools_summary(g: &ToolGroup) -> String {
    let c = g.counts();
    let mut parts = vec!["Tools".to_string(), format!("{} completed", c.completed)];
    if c.failed > 0 {
        parts.push(format!("{} failed", c.failed));
    }
    if c.running > 0 {
        parts.push(format!("{} running", c.running));
    }
    if c.stopped > 0 {
        parts.push(format!("{} stopped", c.stopped));
    }
    parts.join(" · ")
}

fn duration_label(c: &ToolCall) -> Option<String> {
    let ms = c.duration_ms?;
    Some(if ms < 1000 {
        format!("{ms} ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    })
}

fn render_tools(
    b: &mut Builder,
    ranges: &mut Vec<(Target, usize, usize)>,
    id: EntryId,
    g: &ToolGroup,
    theme: &Theme,
) {
    let bg = Style::default().bg(theme.bg);
    let muted = bg.fg(theme.muted);
    let arrow = if g.expanded { "⌄ " } else { "› " };
    b.row(
        INDENT,
        vec![(arrow.into(), muted), (tools_summary(g), muted)],
    );
    ranges.push((Target::Entry(id), 0, 1));
    if !g.expanded {
        return;
    }
    for (i, c) in g.calls.iter().enumerate() {
        let start = b.lines.len();
        let (sym, sym_style) = status_symbol(c.status, theme);
        let mut segs = vec![
            ((if c.expanded { "⌄ " } else { "› " }).to_string(), muted),
            (sym, sym_style),
            (" ".into(), bg),
        ];
        match &c.kind {
            ToolKind::Mcp { server, tool } => {
                segs.push((
                    format!("MCP · {server} / {tool}"),
                    bg.fg(theme.ink).add_modifier(Modifier::BOLD),
                ));
                segs.push((format!(" · {}", status_word(c.status)), muted));
            }
            _ => {
                segs.push((
                    c.name.clone(),
                    bg.fg(theme.ink).add_modifier(Modifier::BOLD),
                ));
                if !c.title.is_empty() {
                    segs.push((format!(" {}", sanitize_line(&c.title)), muted));
                }
                if let Some(d) = &c.diff {
                    let (a, r) = diff_counts(d);
                    segs.push((format!(" +{a}"), bg.fg(theme.add)));
                    segs.push((format!(" −{r}"), bg.fg(theme.del)));
                }
                if c.status == ToolStatus::Running {
                    segs.push((" · running".into(), bg.fg(theme.warn)));
                } else if c.status.is_failure() || c.status == ToolStatus::Stopped {
                    segs.push((format!(" · {}", status_word(c.status)), muted));
                }
                if let Some(d) = duration_label(c) {
                    segs.push((format!(" · {d}"), muted));
                }
            }
        }
        b.row(INDENT + 2, segs);
        let header_end = b.lines.len();
        if c.expanded {
            render_call_detail(b, ranges, id, i, c, theme);
        }
        ranges.push((
            Target::Call(id, i),
            start,
            if c.expanded && !c.has_raw_sections() {
                b.lines.len()
            } else {
                header_end
            },
        ));
    }
}

fn render_call_detail(
    b: &mut Builder,
    ranges: &mut Vec<(Target, usize, usize)>,
    id: EntryId,
    i: usize,
    c: &ToolCall,
    theme: &Theme,
) {
    let ind = INDENT + 6;
    let bg = Style::default().bg(theme.bg);
    let muted = bg.fg(theme.muted);
    let ink = bg.fg(theme.ink);
    let out_style = if c.status.is_failure() {
        bg.fg(theme.danger)
    } else {
        muted
    };
    match &c.kind {
        ToolKind::Mcp { .. } => {
            let mut meta = vec![c.call_id.clone()];
            if let Some(d) = duration_label(c) {
                meta.push(d);
            }
            meta.push(status_word(c.status).to_string());
            b.row(ind, vec![(meta.join(" · "), muted)]);
            if !c.output.is_empty() {
                b.block(
                    ind,
                    &c.output,
                    if c.status.is_failure() {
                        bg.fg(theme.danger)
                    } else {
                        ink
                    },
                    12,
                );
            } else if c.status == ToolStatus::Running {
                b.row(ind, vec![("Waiting for result…".into(), muted)]);
            }
            let start = b.lines.len();
            b.row(
                ind,
                vec![
                    ((if c.raw_in_open { "⌄ " } else { "› " }).into(), muted),
                    ("Raw input · JSON".into(), ink),
                ],
            );
            if c.raw_in_open {
                json_block(b, ind + 2, &c.input, theme);
            }
            ranges.push((Target::RawIn(id, i), start, b.lines.len()));
            let start = b.lines.len();
            b.row(
                ind,
                vec![
                    ((if c.raw_out_open { "⌄ " } else { "› " }).into(), muted),
                    ("Raw output · JSON".into(), ink),
                ],
            );
            if c.raw_out_open {
                match &c.result {
                    Some(v) => json_block(b, ind + 2, v, theme),
                    None => b.row(ind + 2, vec![("(no output yet)".into(), muted)]),
                }
            }
            ranges.push((Target::RawOut(id, i), start, b.lines.len()));
        }
        ToolKind::Shell => {
            let cmd = c
                .input
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| c.title.clone());
            b.block(ind, &format!("$ {cmd}"), ink, 6);
            if !c.output.is_empty() {
                b.block(ind, &c.output, out_style, MAX_DETAIL_LINES);
            }
        }
        _ => {
            if let Some(d) = &c.diff {
                let lines: Vec<&str> = d.lines().collect();
                for l in lines.iter().take(MAX_DETAIL_LINES * 2) {
                    let s = if l.starts_with("+++") || l.starts_with("---") {
                        muted.add_modifier(Modifier::BOLD)
                    } else if l.starts_with('+') {
                        bg.fg(theme.add)
                    } else if l.starts_with('-') {
                        bg.fg(theme.del)
                    } else if l.starts_with("@@") {
                        muted
                    } else {
                        ink
                    };
                    b.text(ind, vec![(sanitize_line(l), s)], theme.bg, None);
                }
                if lines.len() > MAX_DETAIL_LINES * 2 {
                    b.row(
                        ind,
                        vec![(
                            format!("… {} more diff lines", lines.len() - MAX_DETAIL_LINES * 2),
                            muted,
                        )],
                    );
                }
            } else if !c.input.is_null() && c.kind != ToolKind::Read {
                let compact = serde_json::to_string(&c.input).unwrap_or_default();
                b.block(ind, &compact, ink, 6);
            }
            if !c.output.is_empty() && (c.diff.is_none() || c.status.is_failure()) {
                b.block(
                    ind,
                    &c.output,
                    out_style,
                    if c.kind == ToolKind::Read {
                        12
                    } else {
                        MAX_DETAIL_LINES
                    },
                );
            }
        }
    }
    if c.output_truncated {
        b.row(
            ind,
            vec![("(output truncated at 1 MB for display)".into(), muted)],
        );
    }
    if c.status == ToolStatus::Running
        && c.output.is_empty()
        && !matches!(c.kind, ToolKind::Mcp { .. })
    {
        b.row(ind, vec![("running…".into(), bg.fg(theme.warn))]);
    }
}

fn json_block(b: &mut Builder, indent: usize, v: &Value, theme: &Theme) {
    let pretty = serde_json::to_string_pretty(v).unwrap_or_default();
    let s = Style::default().fg(theme.ink).bg(theme.surface);
    let all: Vec<&str> = pretty.lines().collect();
    let max = 200;
    for l in all.iter().take(max) {
        b.text(indent, vec![(sanitize_line(l), s)], theme.surface, None);
    }
    if all.len() > max {
        b.row(
            indent,
            vec![(
                format!("… {} more lines · Ctrl+R shows everything", all.len() - max),
                Style::default().fg(theme.muted).bg(theme.bg),
            )],
        );
    }
}

/// Box drawing helpers for cards.
struct CardBox {
    indent: usize,
    inner: usize,
    border: Style,
    fill: Style,
}

impl CardBox {
    fn new(b: &Builder, border: Style, theme: &Theme) -> Self {
        let inner = b.width.saturating_sub(INDENT + 5).clamp(16, 78);
        CardBox {
            indent: INDENT,
            inner,
            border,
            fill: Style::default().bg(theme.bg),
        }
    }

    fn top(&self, b: &mut Builder, title: &str, title_style: Style) {
        let t = ellipsize(title, self.inner.saturating_sub(2));
        let rest = self.inner.saturating_sub(width(&t) + 1);
        b.lines.push(Line::from(vec![
            Span::styled(" ".repeat(self.indent), self.fill),
            Span::styled("┌─ ", self.border),
            Span::styled(t, title_style),
            Span::styled(format!(" {}┐", "─".repeat(rest)), self.border),
            Span::styled(
                " ".repeat(b.width.saturating_sub(self.indent + self.inner + 4)),
                self.fill,
            ),
        ]));
    }

    fn bottom(&self, b: &mut Builder) {
        b.lines.push(Line::from(vec![
            Span::styled(" ".repeat(self.indent), self.fill),
            Span::styled(format!("└{}┘", "─".repeat(self.inner + 2)), self.border),
            Span::styled(
                " ".repeat(b.width.saturating_sub(self.indent + self.inner + 4)),
                self.fill,
            ),
        ]));
    }

    /// A wrapped row inside the box.
    fn row(&self, b: &mut Builder, segs: Vec<(String, Style)>, fill: Style) {
        for wrapped in wrap_spans(&segs, self.inner.max(1)) {
            let used: usize = wrapped.iter().map(|(t, _)| width(t)).sum();
            let mut spans = vec![
                Span::styled(" ".repeat(self.indent), self.fill),
                Span::styled("│ ", self.border),
            ];
            for (t, s) in wrapped {
                spans.extend(highlight(&t.replace('\u{a0}', " "), s, b.query, b.theme));
            }
            spans.push(Span::styled(
                " ".repeat(self.inner.saturating_sub(used)),
                fill,
            ));
            spans.push(Span::styled(" │", self.border));
            spans.push(Span::styled(
                " ".repeat(b.width.saturating_sub(self.indent + self.inner + 4)),
                self.fill,
            ));
            b.lines.push(Line::from(spans));
        }
    }
}

fn settled_line(b: &mut Builder, state: &CardState, what: &str, theme: &Theme) {
    let bg = Style::default().bg(theme.bg);
    let (sym, sym_style, text) = match state {
        CardState::Answered(label) => {
            let deny = label.starts_with("Den")
                || label.starts_with("Decline")
                || label.starts_with("Cancel");
            (
                if deny { "✕" } else { "✓" },
                bg.fg(if deny { theme.danger } else { theme.accent }),
                label.clone(),
            )
        }
        CardState::Resolved => ("○", bg.fg(theme.muted), "No longer needed".to_string()),
        CardState::Failed(e) => (
            "✕",
            bg.fg(theme.danger),
            format!("Could not send the decision: {e}"),
        ),
        CardState::Pending => return,
    };
    b.row(
        INDENT,
        vec![
            (sym.into(), sym_style),
            (format!(" {text}"), bg.fg(theme.ink)),
            (format!(" · {}", sanitize_line(what)), bg.fg(theme.muted)),
        ],
    );
}

fn render_approval(b: &mut Builder, app: &App, id: EntryId, c: &ApprovalCard, theme: &Theme) {
    if c.state != CardState::Pending {
        settled_line(b, &c.state, &format!("{}: {}", c.title, c.subject), theme);
        return;
    }
    let focused = app.focus == Focus::Card(id);
    let border = Style::default()
        .bg(theme.bg)
        .fg(if focused { theme.accent } else { theme.warn });
    let cb = CardBox::new(b, border, theme);
    let fill = Style::default().bg(theme.bg);
    cb.top(
        b,
        "Permission needed",
        fill.fg(theme.warn).add_modifier(Modifier::BOLD),
    );
    cb.row(b, vec![(sanitize_line(&c.title), fill.fg(theme.ink))], fill);
    if !c.subject.is_empty() {
        let code = Style::default().bg(theme.surface).fg(theme.ink);
        for l in sanitize(&c.subject).split('\n').take(12) {
            cb.row(b, vec![(format!(" {l}"), code)], code);
        }
    }
    for d in &c.details {
        cb.row(b, vec![(sanitize_line(d), fill.fg(theme.muted))], fill);
    }
    cb.row(b, vec![(String::new(), fill)], fill);
    let mut segs = Vec::new();
    for (i, ch) in c.choices.iter().enumerate() {
        let selected = i == c.selected;
        let style = if selected && focused {
            Style::default()
                .bg(theme.accent)
                .fg(theme.on_accent)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            fill.fg(theme.ink).add_modifier(Modifier::BOLD)
        } else {
            fill.fg(match ch.tone {
                ChoiceTone::Deny => theme.muted,
                _ => theme.ink,
            })
        };
        // Non-breaking spaces keep each button on one line when wrapping.
        segs.push((format!("[ {} ]", ch.label).replace(' ', "\u{a0}"), style));
        segs.push(("  ".into(), fill));
    }
    cb.row(b, segs, fill);
    let hint = if focused {
        "←→ choose · Enter confirm · Esc leave pending"
    } else {
        "Tab to review · Enter in the composer won't answer this"
    };
    cb.row(b, vec![(hint.into(), fill.fg(theme.muted))], fill);
    cb.bottom(b);
}

fn render_question(b: &mut Builder, app: &App, id: EntryId, c: &QuestionCard, theme: &Theme) {
    if c.state != CardState::Pending {
        let what = c
            .questions
            .iter()
            .map(|q| q.text.clone())
            .collect::<Vec<_>>()
            .join(" / ");
        settled_line(b, &c.state, &what, theme);
        return;
    }
    let focused = app.focus == Focus::Card(id);
    let fill = Style::default().bg(theme.bg);
    let border = fill.fg(if focused { theme.accent } else { theme.warn });
    let cb = CardBox::new(b, border, theme);
    let title = if c.questions.len() > 1 {
        format!("{} · {} of {}", c.title, c.current + 1, c.questions.len())
    } else {
        c.title.clone()
    };
    cb.top(b, &title, fill.fg(theme.warn).add_modifier(Modifier::BOLD));
    let Some(q) = c.question() else {
        cb.bottom(b);
        return;
    };
    if !q.header.is_empty() {
        cb.row(
            b,
            vec![(sanitize_line(&q.header), fill.fg(theme.muted))],
            fill,
        );
    }
    cb.row(
        b,
        vec![(
            sanitize_line(&q.text),
            fill.fg(theme.ink).add_modifier(Modifier::BOLD),
        )],
        fill,
    );
    for (i, o) in q.options.iter().enumerate() {
        let sel = i == c.selected;
        let marker = if sel && focused { "❯ " } else { "  " };
        let check = if q.multi_select {
            if c.toggled.get(i).copied().unwrap_or(false) {
                "[x] "
            } else {
                "[ ] "
            }
        } else {
            ""
        };
        let label_style = if sel && focused {
            fill.fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            fill.fg(theme.ink)
        };
        let mut segs = vec![
            (marker.to_string(), fill.fg(theme.accent)),
            (
                format!("{check}{}. {}", i + 1, sanitize_line(&o.label)),
                label_style,
            ),
        ];
        if !o.description.is_empty() {
            segs.push((
                format!(" — {}", sanitize_line(&o.description)),
                fill.fg(theme.muted),
            ));
        }
        cb.row(b, segs, fill);
    }
    if let Some(row) = c.type_row() {
        let sel = c.selected == row;
        let marker = if sel && focused { "❯ " } else { "  " };
        let text = if c.typed.is_empty() {
            "Type an answer…".to_string()
        } else {
            format!("{}▏", sanitize_line(&c.typed))
        };
        cb.row(
            b,
            vec![
                (marker.into(), fill.fg(theme.accent)),
                (
                    format!("✎ {text}"),
                    if c.typed.is_empty() {
                        fill.fg(theme.muted)
                    } else {
                        fill.fg(theme.ink)
                    },
                ),
            ],
            fill,
        );
    }
    let sel = c.selected == c.decline_row();
    cb.row(
        b,
        vec![
            (
                (if sel && focused { "❯ " } else { "  " }).into(),
                fill.fg(theme.accent),
            ),
            ("Decline to answer".into(), fill.fg(theme.muted)),
        ],
        fill,
    );
    let hint = if !focused {
        "Tab to answer · Enter in the composer won't answer this"
    } else if q.multi_select {
        "↑↓ choose · Space toggle · Enter confirm · Esc leave pending"
    } else {
        "↑↓ choose · type to answer · Enter confirm · Esc leave pending"
    };
    cb.row(b, vec![(hint.into(), fill.fg(theme.muted))], fill);
    cb.bottom(b);
}

/// The welcome block shown before the first message.
pub fn welcome(app: &App, theme: &Theme, cols: u16, height: u16) -> Vec<Line<'static>> {
    let w = cols as usize;
    let bg = Style::default().bg(theme.bg);
    let quote = app.skin.quotes().get(app.quote).copied().unwrap_or("");
    let session = match &app.phase {
        Phase::Starting => (
            "◌".to_string(),
            format!(" Connecting to {}…", app.harness.label()),
            theme.muted,
        ),
        Phase::Disconnected(r) => (
            "✕".to_string(),
            format!(" Not connected · {}", sanitize_line(r)),
            theme.danger,
        ),
        _ => (
            "✓".to_string(),
            format!(
                " Connected · {}{}",
                app.harness.label(),
                if app.cwd_label.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", app.cwd_label)
                }
            ),
            theme.accent,
        ),
    };
    let text_rows: Vec<Vec<(String, Style)>> = vec![
        vec![(quote.to_string(), bg.fg(theme.accent).add_modifier(Modifier::BOLD))],
        vec![(app.skin.heading().to_string(), bg.fg(theme.ink).add_modifier(Modifier::BOLD))],
        vec![],
        vec![("Start a conversation in this project. Your agent can read files, make changes and run commands.".to_string(), bg.fg(theme.muted))],
        vec![],
        vec![(session.0, bg.fg(session.2)), (session.1, bg.fg(theme.muted))],
    ];
    let sprite = if app.skin == crate::ui::skins::SkinId::Duke {
        Some(super::mascot::render(
            &super::mascot::FULL,
            theme.mode,
            theme.bg,
        ))
    } else {
        None
    };
    let side_by_side = sprite.is_some() && w >= 56;
    let text_x = INDENT + if side_by_side { 16 + 3 } else { 0 };
    let text_w = w.saturating_sub(text_x + 1).max(10);
    let mut body: Vec<Line<'static>> = Vec::new();
    let mut wrapped_text: Vec<Line<'static>> = Vec::new();
    for r in &text_rows {
        if r.is_empty() {
            wrapped_text.push(Line::from(Span::styled(
                " ".repeat(w.saturating_sub(text_x)),
                bg,
            )));
            continue;
        }
        for segs in wrap_spans(r, text_w) {
            let used: usize = segs.iter().map(|(t, _)| width(t)).sum();
            let mut l = to_line(segs);
            l.spans.push(Span::styled(
                " ".repeat(w.saturating_sub(text_x + used)),
                bg,
            ));
            wrapped_text.push(l);
        }
    }
    match sprite {
        Some(sp) if side_by_side => {
            let rows = sp.len().max(wrapped_text.len());
            let text_off = sp.len().saturating_sub(wrapped_text.len()) / 2;
            for i in 0..rows {
                let mut spans = vec![Span::styled(" ".repeat(INDENT), bg)];
                match sp.get(i) {
                    Some(l) => spans.extend(l.spans.clone()),
                    None => spans.push(Span::styled(" ".repeat(16), bg)),
                }
                spans.push(Span::styled("   ", bg));
                if let Some(t) = i.checked_sub(text_off).and_then(|j| wrapped_text.get(j)) {
                    spans.extend(t.spans.clone());
                } else {
                    spans.push(Span::styled(" ".repeat(w.saturating_sub(text_x)), bg));
                }
                body.push(Line::from(spans));
            }
        }
        Some(sp) => {
            for l in sp {
                let mut spans = vec![Span::styled(" ".repeat(INDENT), bg)];
                spans.extend(l.spans);
                spans.push(Span::styled(" ".repeat(w.saturating_sub(INDENT + 16)), bg));
                body.push(Line::from(spans));
            }
            body.push(Line::from(Span::styled(" ".repeat(w), bg)));
            for t in wrapped_text {
                let mut spans = vec![Span::styled(" ".repeat(INDENT), bg)];
                spans.extend(t.spans);
                body.push(Line::from(spans));
            }
        }
        None => {
            for t in wrapped_text {
                let mut spans = vec![Span::styled(" ".repeat(INDENT), bg)];
                spans.extend(t.spans);
                body.push(Line::from(spans));
            }
        }
    }
    let top_pad = ((height as usize).saturating_sub(body.len()) / 3).min(6);
    let mut out: Vec<Line<'static>> = (0..top_pad)
        .map(|_| Line::from(Span::styled(" ".repeat(w), bg)))
        .collect();
    out.extend(body);
    out
}

pub fn elapsed_label(app: &App) -> Option<String> {
    let d = app.elapsed()?;
    let s = d.as_secs();
    Some(if s >= 3600 {
        format_duration(d)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    })
}
