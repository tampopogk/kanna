//! Word wrapping of styled spans to a column width.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Wraps one logical line made of styled segments into lines of at most
/// `width` columns, breaking at spaces where possible.
pub fn wrap_spans(segments: &[(String, Style)], width: usize) -> Vec<Vec<(String, Style)>> {
    let width = width.max(1);
    // Tokenize into words and spaces, keeping styles.
    let mut tokens: Vec<(String, Style)> = Vec::new();
    for (text, style) in segments {
        let mut cur = String::new();
        let mut cur_space: Option<bool> = None;
        for c in text.chars() {
            let sp = c == ' ';
            if cur_space.is_some_and(|s| s != sp) {
                tokens.push((std::mem::take(&mut cur), *style));
            }
            cur_space = Some(sp);
            cur.push(c);
        }
        if !cur.is_empty() {
            tokens.push((cur, *style));
        }
    }
    let mut lines: Vec<Vec<(String, Style)>> = vec![Vec::new()];
    let mut col = 0usize;
    for (tok, style) in tokens {
        let w = self::width(&tok);
        let is_space = tok.starts_with(' ');
        if col + w <= width {
            push_seg(lines.last_mut().unwrap(), &tok, style);
            col += w;
            continue;
        }
        if is_space {
            // Drop spaces at a break.
            lines.push(Vec::new());
            col = 0;
            continue;
        }
        if w <= width && col > 0 {
            lines.push(Vec::new());
            col = 0;
            push_seg(lines.last_mut().unwrap(), &tok, style);
            col += w;
            continue;
        }
        // Hard-break a long word.
        for c in tok.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            if col + cw > width && col > 0 {
                lines.push(Vec::new());
                col = 0;
            }
            push_seg(lines.last_mut().unwrap(), &c.to_string(), style);
            col += cw;
        }
    }
    // Spaces at the end of a wrapped line carry no meaning.
    let n = lines.len();
    for line in lines.iter_mut().take(n.saturating_sub(1)) {
        while let Some(last) = line.last_mut() {
            let trimmed = last.0.trim_end_matches(' ').len();
            last.0.truncate(trimmed);
            if last.0.is_empty() {
                line.pop();
            } else {
                break;
            }
        }
    }
    lines
}

fn push_seg(line: &mut Vec<(String, Style)>, text: &str, style: Style) {
    if let Some(last) = line.last_mut() {
        if last.1 == style {
            last.0.push_str(text);
            return;
        }
    }
    line.push((text.to_string(), style));
}

pub fn to_line(segs: Vec<(String, Style)>) -> Line<'static> {
    Line::from(
        segs.into_iter()
            .map(|(t, s)| Span::styled(t, s))
            .collect::<Vec<_>>(),
    )
}

/// Truncates to `width` columns with an ellipsis.
pub fn ellipsize(s: &str, width: usize) -> String {
    if self::width(s) <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut col = 0;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if col + cw + 1 > width {
            break;
        }
        out.push(c);
        col += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: Vec<Vec<(String, Style)>>) -> Vec<String> {
        lines
            .into_iter()
            .map(|l| l.into_iter().map(|(t, _)| t).collect())
            .collect()
    }

    #[test]
    fn wraps_at_spaces() {
        let segs = vec![("the quick brown fox jumps".to_string(), Style::default())];
        assert_eq!(
            plain(wrap_spans(&segs, 10)),
            vec!["the quick", "brown fox", "jumps"]
        );
    }

    #[test]
    fn hard_breaks_long_words() {
        let segs = vec![("abcdefghij".to_string(), Style::default())];
        assert_eq!(plain(wrap_spans(&segs, 4)), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn ellipsis() {
        assert_eq!(ellipsize("hello world", 6), "hello…");
        assert_eq!(ellipsize("hi", 6), "hi");
    }
}
