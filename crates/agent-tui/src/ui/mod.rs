//! Rendering. Everything here reads [`App`]; the only state it writes back is
//! the transcript viewport (top line, height) that scrolling depends on.

pub mod mascot;
pub mod sanitize;
pub mod skins;
pub mod text;
pub mod transcript;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

use crate::app::slash::CommandSource;
use crate::app::{App, Focus, Overlay, Status};
use crate::protocol::{MetaSource, MetaValue, NoticeLevel};
use crate::raw::Dir;
use sanitize::sanitize_line;
use skins::{ColorMode, SkinId, Theme, ALL_SKINS};
use text::{ellipsize, width};
pub use transcript::RenderCache;

const COMPOSER_MAX_ROWS: usize = 6;

pub fn draw(f: &mut Frame, app: &mut App, cache: &mut RenderCache, mode: ColorMode) {
    let theme = Theme::new(app.skin, mode);
    let area = f.area();
    f.render_widget(
        Block::default().style(Style::default().bg(theme.bg).fg(theme.ink)),
        area,
    );
    if area.width < 20 || area.height < 8 {
        f.render_widget(
            Paragraph::new("Terminal too small")
                .style(Style::default().fg(theme.warn).bg(theme.bg)),
            area,
        );
        return;
    }
    // Fixed rows first; the composer and the Duke header shrink to fit so no
    // region ever extends past the terminal (height >= 8 is guaranteed above).
    let search_h: u16 = u16::from(app.search.is_some());
    let footer_h: u16 = 1;
    const MIN_COMPOSER: u16 = 3;
    const MIN_TRANSCRIPT: u16 = 1;
    let big_header = app.skin == SkinId::Duke
        && area.width >= 60
        && area.height >= 4 + MIN_COMPOSER + search_h + footer_h + 3;
    let header_h: u16 = if big_header { 4 } else { 2 };
    let wanted_composer = 1 + composer_rows(app, area.width) as u16 + 1;
    let composer_room = area
        .height
        .saturating_sub(header_h + search_h + footer_h + MIN_TRANSCRIPT);
    let composer_h = wanted_composer.min(composer_room).max(MIN_COMPOSER);
    let transcript_h = area
        .height
        .saturating_sub(header_h + composer_h + search_h + footer_h)
        .max(MIN_TRANSCRIPT);
    let mut y = area.y;
    let header = Rect::new(area.x, y, area.width, header_h);
    y += header_h;
    let body = Rect::new(area.x, y, area.width, transcript_h);
    y += transcript_h;
    let search = Rect::new(area.x, y, area.width, search_h);
    y += search_h;
    let composer = Rect::new(area.x, y, area.width, composer_h);
    y += composer_h;
    let footer = Rect::new(area.x, y, area.width, footer_h);
    debug_assert!(footer.bottom() <= area.bottom(), "layout overflow");

    draw_header(f, app, &theme, header, big_header);
    draw_transcript(f, app, cache, &theme, body);
    if app.search.is_some() {
        draw_search(f, app, &theme, search);
    }
    draw_composer(f, app, &theme, composer);
    draw_footer(f, app, &theme, footer);
    if app.slash.open && app.focus == Focus::Composer && app.overlay == Overlay::None {
        draw_slash_menu(f, app, &theme, body);
    }
    match app.overlay.clone() {
        Overlay::None => {}
        Overlay::Help { selected } => draw_help(f, app, &theme, area, selected),
        Overlay::Inspector {
            title,
            records,
            scroll,
        } => {
            let max = draw_inspector(f, app, &theme, area, &title, &records, scroll);
            if let Overlay::Inspector { scroll, .. } = &mut app.overlay {
                *scroll = (*scroll).min(max);
            }
        }
        Overlay::SkinPicker { selected } => {
            draw_skin_picker(f, app, &theme, area, footer, selected)
        }
        Overlay::QuitConfirm { selected } => draw_quit(f, &theme, area, selected),
    }
}

fn meta_spans(
    label: &str,
    v: &Option<MetaValue>,
    theme: &Theme,
    show_label: bool,
    max: usize,
) -> Vec<Span<'static>> {
    let bg = Style::default().bg(theme.panel);
    let mut spans = Vec::new();
    if show_label {
        spans.push(Span::styled(format!("{label} "), bg.fg(theme.muted)));
    }
    match v {
        Some(m) if m.source == MetaSource::Requested => {
            spans.push(Span::styled(
                ellipsize(&sanitize_line(&m.value), max),
                bg.fg(theme.ink),
            ));
            spans.push(Span::styled(" (requested)", bg.fg(theme.muted)));
        }
        Some(m) => spans.push(Span::styled(
            ellipsize(&sanitize_line(&m.value), max),
            bg.fg(theme.ink),
        )),
        None => spans.push(Span::styled(
            "Not reported",
            bg.fg(theme.muted).add_modifier(Modifier::ITALIC),
        )),
    }
    spans
}

fn status_style(s: Status, theme: &Theme) -> Style {
    let bg = Style::default().bg(theme.panel);
    match s {
        Status::Ready => bg.fg(theme.accent),
        Status::Starting => bg.fg(theme.muted),
        Status::Working | Status::NeedsApproval | Status::NeedsInput | Status::Stopping => {
            bg.fg(theme.warn)
        }
        Status::Disconnected | Status::Degraded => bg.fg(theme.danger),
    }
}

fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| width(&s.content)).sum()
}

fn draw_header(f: &mut Frame, app: &App, theme: &Theme, area: Rect, big: bool) {
    let panel = Style::default().bg(theme.panel);
    f.render_widget(Block::default().style(panel), area);
    let w = area.width as usize;
    let st = app.status();
    let mut status = format!("{} {}", st.symbol(), st.word());
    if matches!(
        st,
        Status::Working | Status::NeedsApproval | Status::NeedsInput | Status::Stopping
    ) {
        if let Some(e) = transcript::elapsed_label(app) {
            status.push_str(&format!(" · {e}"));
        }
    }
    if app.queued_count > 0 {
        status.push_str(&format!(" · {} queued", app.queued_count));
    }
    let status_span = Span::styled(
        format!("{status} "),
        status_style(st, theme).add_modifier(Modifier::BOLD),
    );
    let logo_w = if big { 8 } else { 2 };
    let bold = if app.skin == SkinId::Duke {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let build = |labels: bool, model_max: usize| -> Vec<Span<'static>> {
        let mut s = vec![Span::styled(" ", panel)];
        if !big {
            s.push(Span::styled(
                "›_",
                panel.fg(theme.accent).add_modifier(Modifier::BOLD),
            ));
        } else {
            s.push(Span::styled(" ".repeat(logo_w), panel));
        }
        s.push(Span::styled("  ", panel));
        if labels {
            s.push(Span::styled("Harness ", panel.fg(theme.muted)));
        }
        s.push(Span::styled(
            app.harness.label().to_string(),
            panel.fg(theme.ink).add_modifier(Modifier::BOLD),
        ));
        if model_max > 0 {
            s.push(Span::styled("   ", panel));
            s.extend(
                meta_spans("Model", &app.meta.model, theme, labels, model_max)
                    .into_iter()
                    .map(|sp| sp.patch_style(Style::default().add_modifier(bold))),
            );
        }
        s.push(Span::styled("   ", panel));
        s.extend(
            meta_spans("Effort", &app.meta.effort, theme, labels, 16)
                .into_iter()
                .map(|sp| sp.patch_style(Style::default().add_modifier(bold))),
        );
        s
    };
    let status_w = width(&status_span.content);
    let fits = |spans: &Vec<Span>| spans_width(spans) + status_w < w;
    // Narrowing order: drop the field labels, then shorten the model, then drop it.
    let mut left = build(true, 64);
    if !fits(&left) {
        left = build(false, 64);
    }
    let mut max = 40;
    while !fits(&left) && max >= 8 {
        left = build(false, max);
        max -= 4;
    }
    if !fits(&left) {
        left = build(false, 0);
    }
    let lw = spans_width(&left);
    let mut spans = left;
    if lw + status_w <= w {
        spans.push(Span::styled(" ".repeat(w - lw - status_w), panel));
        spans.push(status_span);
    }
    let text_row = if big { 1 } else { 0 };
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y + text_row, area.width, 1),
    );
    if big {
        let sprite = mascot::render(&mascot::COMPACT, theme.mode, theme.panel);
        for (i, l) in sprite.into_iter().enumerate() {
            f.render_widget(
                Paragraph::new(l),
                Rect::new(area.x + 1, area.y + i as u16, 8, 1),
            );
        }
    }
    let rule_char = if app.skin == SkinId::Duke {
        "━"
    } else {
        "─"
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            rule_char.repeat(w),
            Style::default().fg(theme.header_rule).bg(theme.bg),
        )),
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
    );
}

fn draw_transcript(
    f: &mut Frame,
    app: &mut App,
    cache: &mut RenderCache,
    theme: &Theme,
    area: Rect,
) {
    let h = area.height as usize;
    if app.transcript.is_empty() {
        let lines = transcript::welcome(app, theme, area.width, area.height);
        app.view.total = 0;
        app.view.height = h;
        f.render_widget(
            Paragraph::new(lines).style(Style::default().bg(theme.bg)),
            area,
        );
        return;
    }
    let lay = transcript::layout(app, theme, area.width, cache);
    let total = lay.lines.len();
    app.view.total = total;
    app.view.height = h;
    let max_top = total.saturating_sub(h);
    if app.view.ensure_visible {
        app.view.ensure_visible = false;
        if let Some((_, a, b)) = app
            .view
            .selected
            .and_then(|s| lay.ranges.iter().find(|(t, _, _)| *t == s).copied())
        {
            let cur = if app.view.follow {
                max_top
            } else {
                app.view.top.min(max_top)
            };
            let mut top = cur;
            if a < cur {
                top = a;
            } else if b > cur + h {
                top = if b - a > h { a } else { b - h };
            }
            let top = top.min(max_top);
            if top != cur || !app.view.follow {
                if top >= max_top {
                    app.view.follow = true;
                    app.view.new_count = 0;
                } else {
                    app.view.follow = false;
                    app.view.top = top;
                }
            }
        }
    }
    let top = if app.view.follow {
        max_top
    } else {
        app.view.top.min(max_top)
    };
    if !app.view.follow {
        app.view.top = top;
        if top >= max_top && app.view.new_count == 0 {
            app.view.follow = true;
        }
    }
    let visible: Vec<Line> = lay.lines.into_iter().skip(top).take(h).collect();
    f.render_widget(
        Paragraph::new(visible).style(Style::default().bg(theme.bg)),
        area,
    );
    if !app.view.follow {
        let label = if app.view.new_count > 0 {
            format!(" ↓ Jump to latest · {} new  Ctrl+L ", app.view.new_count)
        } else {
            " ↓ Jump to latest  Ctrl+L ".to_string()
        };
        let lw = width(&label) as u16;
        if lw + 2 < area.width {
            let x = area.x + (area.width - lw) / 2;
            let r = Rect::new(x, area.bottom() - 1, lw, 1);
            f.render_widget(Clear, r);
            f.render_widget(
                Paragraph::new(Span::styled(
                    label,
                    Style::default()
                        .bg(theme.panel)
                        .fg(theme.ink)
                        .add_modifier(Modifier::BOLD),
                )),
                r,
            );
        }
    }
}

fn draw_search(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let Some(s) = &app.search else { return };
    let bg = Style::default().bg(theme.panel);
    let count = match (s.current, s.matches.len()) {
        (_, 0) if s.query.is_empty() => String::new(),
        (_, 0) => "no matches".to_string(),
        (Some(i), n) => format!("{}/{}", i + 1, n),
        (None, n) => format!("{n} matches"),
    };
    let focused = app.focus == Focus::Search;
    let spans = vec![
        Span::styled(
            " Search ",
            bg.fg(if focused { theme.accent } else { theme.muted })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(sanitize_line(&s.query), bg.fg(theme.ink)),
        Span::styled(if focused { "▏" } else { "" }, bg.fg(theme.accent)),
        Span::styled(format!("  {count}"), bg.fg(theme.muted)),
        Span::styled(
            "   Enter next · Shift+Enter prev · Esc close",
            bg.fg(theme.muted),
        ),
    ];
    f.render_widget(Paragraph::new(Line::from(spans)).style(bg), area);
}

/// Wraps the draft by display width and finds the cursor's row/column.
fn composer_layout(text: &str, cursor: usize, w: usize) -> (Vec<String>, usize, usize) {
    let w = w.max(1);
    let mut rows = vec![String::new()];
    let mut col = 0;
    let (mut crow, mut ccol) = (0, 0);
    for (i, c) in text.char_indices() {
        if i == cursor {
            crow = rows.len() - 1;
            ccol = col;
        }
        if c == '\n' {
            rows.push(String::new());
            col = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if col + cw > w {
            rows.push(String::new());
            col = 0;
        }
        rows.last_mut().unwrap().push(c);
        col += cw;
    }
    if cursor >= text.len() {
        crow = rows.len() - 1;
        ccol = col;
        if ccol >= w {
            rows.push(String::new());
            crow += 1;
            ccol = 0;
        }
    }
    (rows, crow, ccol)
}

fn composer_rows(app: &App, width: u16) -> usize {
    let (rows, _, _) = composer_layout(
        app.composer.text(),
        app.composer.cursor(),
        (width as usize).saturating_sub(5),
    );
    rows.len().clamp(1, COMPOSER_MAX_ROWS)
}

fn draw_composer(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let w = area.width as usize;
    let bg = Style::default().bg(theme.bg);
    let focused = app.focus == Focus::Composer && app.overlay == Overlay::None;
    let rule_char = if app.skin == SkinId::Duke {
        "━"
    } else {
        "─"
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            rule_char.repeat(w),
            bg.fg(theme.composer_rule),
        )),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let inner_w = w.saturating_sub(5);
    let rows_h = area.height.saturating_sub(2) as usize;
    let (rows, crow, ccol) = composer_layout(app.composer.text(), app.composer.cursor(), inner_w);
    let skip = (crow + 1).saturating_sub(rows_h);
    let prompt_style = bg
        .fg(if focused { theme.accent } else { theme.muted })
        .add_modifier(Modifier::BOLD);
    for i in 0..rows_h {
        let y = area.y + 1 + i as u16;
        let mut spans = vec![Span::styled(
            if i == 0 { " › " } else { "   " },
            prompt_style,
        )];
        if app.composer.is_empty() && i == 0 {
            spans.push(Span::styled(
                "Message your agent, or / for commands",
                bg.fg(theme.muted),
            ));
        } else if let Some(r) = rows.get(skip + i) {
            spans.push(Span::styled(sanitize_line(r), bg.fg(theme.ink)));
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(bg),
            Rect::new(area.x, y, area.width, 1),
        );
    }
    if focused {
        let x = area.x + 3 + ccol as u16;
        let y = area.y + 1 + (crow - skip) as u16;
        if x < area.right() && y < area.bottom() {
            f.set_cursor_position(Position::new(x, y));
        }
    }
    let hint_y = area.bottom() - 1;
    let (text, style) = if let Some(h) = &app.hint {
        let c = match h.level {
            NoticeLevel::Info => theme.muted,
            NoticeLevel::Warn => theme.warn,
            NoticeLevel::Error => theme.danger,
        };
        (h.text.clone(), bg.fg(c))
    } else if app.focus == Focus::Transcript {
        (
            "Transcript · ↑↓ select · Enter expand · Ctrl+R raw JSON · Tab composer".to_string(),
            bg.fg(theme.muted),
        )
    } else if app.turn_running() && !app.composer.is_empty() {
        (
            "Draft saved · waiting for current turn".to_string(),
            bg.fg(theme.muted),
        )
    } else {
        (String::new(), bg)
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            format!("   {}", ellipsize(&text, w.saturating_sub(4))),
            style,
        ))
        .style(bg),
        Rect::new(area.x, hint_y, area.width, 1),
    );
}

fn draw_footer(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let panel = Style::default().bg(theme.panel);
    let w = area.width as usize;
    let newline_key = if app.shift_enter {
        "Shift+Enter"
    } else {
        "Ctrl+J"
    };
    let stop = if app.turn_running() {
        ("Ctrl+C", "stop turn")
    } else {
        ("Ctrl+Q", "quit")
    };
    let hints: Vec<(&str, &str)> = vec![
        ("Enter", "send"),
        (newline_key, "newline"),
        stop,
        ("/", "commands"),
        ("Tab", "focus"),
        ("Ctrl+F", "search"),
        ("Ctrl+R", "raw"),
        ("F1", "help"),
    ];
    let skin = vec![
        Span::styled("● ", panel.fg(theme.accent)),
        Span::styled("Skin ", panel.fg(theme.muted)),
        Span::styled(
            app.skin.label(),
            panel.fg(theme.ink).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ▾ F2 ", panel.fg(theme.muted)),
    ];
    let skin_w = spans_width(&skin);
    let mut n = hints.len();
    let hint_spans = |n: usize| -> Vec<Span<'static>> {
        let mut s = vec![Span::styled(" ", panel)];
        for (k, d) in hints.iter().take(n) {
            s.push(Span::styled(k.to_string(), panel.fg(theme.ink)));
            s.push(Span::styled(format!(" {d}   "), panel.fg(theme.muted)));
        }
        s
    };
    while n > 0 && spans_width(&hint_spans(n)) + skin_w > w {
        n -= 1;
    }
    let mut spans = hint_spans(n);
    let used = spans_width(&spans);
    if used + skin_w <= w {
        spans.push(Span::styled(" ".repeat(w - used - skin_w), panel));
        spans.extend(skin);
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(panel), area);
}

fn popup_block(theme: &Theme, title: &str) -> Block<'static> {
    let border = if theme.skin == SkinId::Duke {
        ratatui::widgets::BorderType::Plain
    } else {
        ratatui::widgets::BorderType::Rounded
    };
    Block::default()
        .borders(Borders::ALL)
        .border_type(border)
        .border_style(Style::default().fg(theme.line).bg(theme.panel))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme.accent)
                .bg(theme.panel)
                .add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(theme.panel).fg(theme.ink))
}

fn draw_slash_menu(f: &mut Frame, app: &App, theme: &Theme, body: Rect) {
    let items = &app.slash.items;
    let visible = items.len().min(8);
    let h = visible as u16 + 2;
    let w = (body.width.saturating_sub(4)).min(76);
    if h > body.height || w < 20 {
        return;
    }
    let r = Rect::new(body.x + 2, body.bottom() - h, w, h);
    f.render_widget(Clear, r);
    let block = popup_block(theme, "Commands");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let start = app.slash.selected.saturating_sub(visible - 1);
    let running = app.turn_running();
    let panel = Style::default().bg(theme.panel);
    for (row, (i, c)) in items
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
    {
        let sel = i == app.slash.selected;
        let base = if sel {
            Style::default().bg(theme.selected)
        } else {
            panel
        };
        let tag = match c.source {
            CommandSource::Local => "local".to_string(),
            CommandSource::Harness if running => {
                format!("{} · when ready", app.harness.label().to_lowercase())
            }
            CommandSource::Harness => app.harness.label().to_lowercase(),
        };
        let name = format!("/{}", c.name);
        let tag_w = width(&tag) + 1;
        let iw = inner.width as usize;
        let mut spans = vec![Span::styled(
            if sel { "❯ " } else { "  " },
            base.fg(theme.accent),
        )];
        let dim = c.source == CommandSource::Harness && running;
        spans.push(Span::styled(
            name.clone(),
            base.fg(if dim { theme.muted } else { theme.ink })
                .add_modifier(Modifier::BOLD),
        ));
        let mut used = 2 + width(&name);
        if !c.argument_hint.is_empty() {
            let a = format!(" {}", c.argument_hint);
            used += width(&a);
            spans.push(Span::styled(a, base.fg(theme.muted)));
        }
        let room = iw.saturating_sub(used + tag_w + 2);
        if room > 4 {
            let d = ellipsize(&format!("  {}", sanitize_line(&c.description)), room);
            used += width(&d);
            spans.push(Span::styled(d, base.fg(theme.muted)));
        }
        spans.push(Span::styled(
            " ".repeat(iw.saturating_sub(used + tag_w)),
            base,
        ));
        spans.push(Span::styled(
            format!("{tag} "),
            base.fg(if c.source == CommandSource::Local {
                theme.accent
            } else {
                theme.warn
            }),
        ));
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
        );
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn draw_help(f: &mut Frame, app: &App, theme: &Theme, area: Rect, selected: usize) {
    let r = centered(area, 78, 34);
    f.render_widget(Clear, r);
    let block = popup_block(theme, "Help · Esc close");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let p = Style::default().bg(theme.panel);
    let key = |k: &str, d: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {k:<14}"),
                p.fg(theme.ink).add_modifier(Modifier::BOLD),
            ),
            Span::styled(d.to_string(), p.fg(theme.muted)),
        ])
    };
    let newline = if app.shift_enter {
        "Ctrl+J / Shift+Enter"
    } else {
        "Ctrl+J"
    };
    let mut lines = vec![
        Line::from(Span::styled(
            " Keys",
            p.fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
        key("Enter", "send the draft (only when the agent is ready)"),
        key(newline, "insert a newline"),
        key(
            "Tab",
            "cycle focus: composer → pending request → transcript",
        ),
        key(
            "↑↓ PgUp PgDn",
            "select / scroll in the transcript; Enter expands",
        ),
        key("Ctrl+C", "stop the current turn"),
        key(
            "Ctrl+F",
            "search (Enter / Shift+Enter move between matches)",
        ),
        key("Ctrl+R", "raw JSON for the selection, or the event log"),
        key("Ctrl+L", "jump to latest and follow live output"),
        key("F2", "choose a skin"),
        key("Ctrl+Q", "quit (asks first while a turn runs)"),
        Line::from(""),
        Line::from(Span::styled(
            " Commands · ↑↓ or Ctrl+N/P · Enter puts it in the composer",
            p.fg(theme.accent).add_modifier(Modifier::BOLD),
        )),
    ];
    let cmds = app.all_commands();
    let room = (inner.height as usize)
        .saturating_sub(lines.len() + 1)
        .max(1);
    let start = selected.saturating_sub(room.saturating_sub(1));
    for (i, c) in cmds.iter().enumerate().skip(start).take(room) {
        let sel = i == selected;
        let base = if sel {
            Style::default().bg(theme.selected)
        } else {
            p
        };
        let tag = if c.source == CommandSource::Local {
            "local"
        } else {
            app.harness.label()
        };
        let line = format!("/{} {}", c.name, c.argument_hint);
        lines.push(Line::from(vec![
            Span::styled(if sel { " ❯ " } else { "   " }, base.fg(theme.accent)),
            Span::styled(format!("{:<22}", ellipsize(&line, 22)), base.fg(theme.ink)),
            Span::styled(format!("{:<8}", tag.to_lowercase()), base.fg(theme.muted)),
            Span::styled(
                ellipsize(
                    &sanitize_line(&c.description),
                    (inner.width as usize).saturating_sub(34),
                ),
                base.fg(theme.muted),
            ),
        ]));
    }
    if !app.caps.harness_commands {
        lines.push(Line::from(Span::styled(
            format!(
                "   {} commands are not available over its app-server interface.",
                app.harness.label()
            ),
            p.fg(theme.muted),
        )));
    }
    f.render_widget(Paragraph::new(lines).style(p), inner);
}

/// Returns the maximum useful scroll offset.
fn draw_inspector(
    f: &mut Frame,
    app: &App,
    theme: &Theme,
    area: Rect,
    title: &str,
    records: &[u64],
    scroll: usize,
) -> usize {
    let r = area;
    f.render_widget(Clear, r);
    let block = popup_block(theme, &format!("{title} · ↑↓ PgUp PgDn · Esc close"));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let p = Style::default().bg(theme.panel);
    let mut lines: Vec<Line> = Vec::new();
    if records.is_empty() {
        lines.push(Line::from(Span::styled(
            " No raw records for this item.",
            p.fg(theme.muted),
        )));
    }
    for id in records {
        match app.raw.get(*id) {
            None => lines.push(Line::from(Span::styled(
                format!(" ── #{id} · discarded from the bounded log ──"),
                p.fg(theme.muted),
            ))),
            Some(rec) => {
                let dir = match rec.dir {
                    Dir::In => format!(
                        "← received{}",
                        rec.line_no
                            .map(|l| format!(" · line {l}"))
                            .unwrap_or_default()
                    ),
                    Dir::Out => "→ sent".to_string(),
                };
                let mut head = format!(" ── #{} {dir}", rec.id);
                if rec.malformed {
                    head.push_str(" · MALFORMED");
                }
                if rec.truncated > 0 {
                    head.push_str(&format!(" · {} bytes truncated for display", rec.truncated));
                }
                head.push_str(" ──");
                lines.push(Line::from(Span::styled(
                    head,
                    p.fg(if rec.malformed {
                        theme.danger
                    } else {
                        theme.accent
                    }),
                )));
                let body = serde_json::from_str::<serde_json::Value>(&rec.text)
                    .ok()
                    .and_then(|v| serde_json::to_string_pretty(&v).ok())
                    .unwrap_or_else(|| rec.text.clone());
                for l in body.lines() {
                    lines.push(Line::from(Span::styled(
                        format!(" {}", sanitize_line(l)),
                        p.fg(theme.ink),
                    )));
                }
            }
        }
    }
    let max = lines.len().saturating_sub(inner.height as usize);
    let s = scroll.min(max);
    let visible: Vec<Line> = lines
        .into_iter()
        .skip(s)
        .take(inner.height as usize)
        .collect();
    f.render_widget(Paragraph::new(visible).style(p), inner);
    max
}

fn draw_skin_picker(
    f: &mut Frame,
    app: &App,
    theme: &Theme,
    area: Rect,
    footer: Rect,
    selected: usize,
) {
    let h = ALL_SKINS.len() as u16 + 2;
    let w = 30u16.min(area.width);
    let r = Rect::new(
        area.right().saturating_sub(w + 1),
        footer.y.saturating_sub(h),
        w,
        h,
    );
    f.render_widget(Clear, r);
    let block = popup_block(theme, "Skin");
    let inner = block.inner(r);
    f.render_widget(block, r);
    for (i, s) in ALL_SKINS.iter().enumerate() {
        let t = Theme::new(*s, theme.mode);
        let sel = i == selected;
        let base = if sel {
            Style::default().bg(theme.selected)
        } else {
            Style::default().bg(theme.panel)
        };
        let spans = vec![
            Span::styled(if sel { " ❯ " } else { "   " }, base.fg(theme.accent)),
            Span::styled("● ", base.fg(t.accent)),
            Span::styled(format!("{:<17}", s.label()), base.fg(theme.ink)),
            Span::styled(
                if *s == app.skin { "✓" } else { " " },
                base.fg(theme.accent),
            ),
        ];
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(base),
            Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
        );
    }
}

fn draw_quit(f: &mut Frame, theme: &Theme, area: Rect, selected: usize) {
    let r = centered(area, 50, 6);
    f.render_widget(Clear, r);
    let block = popup_block(theme, "Quit?");
    let inner = block.inner(r);
    f.render_widget(block, r);
    let p = Style::default().bg(theme.panel);
    let btn = |label: &str, sel: bool| {
        Span::styled(
            format!("[ {label} ]"),
            if sel {
                Style::default()
                    .bg(theme.accent)
                    .fg(theme.on_accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                p.fg(theme.ink)
            },
        )
    };
    let lines = vec![
        Line::from(Span::styled(
            " The agent is still working.",
            p.fg(theme.ink),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(" ", p),
            btn("Stop & quit", selected == 0),
            Span::styled("  ", p),
            btn("Keep working", selected == 1),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).style(p), inner);
}
