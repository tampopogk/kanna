//! Provider text is displayed, never executed: control characters (including
//! ESC that starts CSI/OSC sequences) become visible symbols.

/// Replaces control characters with visible stand-ins. Keeps `\n`; expands `\t`.
pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\r' => {}
            '\u{1b}' => out.push('␛'),
            '\u{7f}' => out.push('␡'),
            c if (c as u32) < 0x20 => out.push(char::from_u32(0x2400 + c as u32).unwrap_or('�')),
            c if (0x80..0xa0).contains(&(c as u32)) => {
                out.push_str(&format!("<U+{:04X}>", c as u32))
            }
            // Bidi overrides can visually reorder text.
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {
                out.push_str(&format!("<U+{:04X}>", c as u32))
            }
            c => out.push(c),
        }
    }
    out
}

/// Single-line variant: newlines shown as ⏎.
pub fn sanitize_line(s: &str) -> String {
    sanitize(s).replace('\n', "⏎")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutralizes_escape_sequences() {
        let csi = "\u{1b}[31mred\u{1b}[0m";
        assert_eq!(sanitize(csi), "␛[31mred␛[0m");
        let osc = "\u{1b}]0;title\u{7}";
        assert_eq!(sanitize(osc), "␛]0;title␇");
        let c1 = "a\u{9b}2Jb";
        assert_eq!(sanitize(c1), "a<U+009B>2Jb");
        assert!(!sanitize("x\u{1b}y\u{0}z\u{8}")
            .chars()
            .any(|c| c.is_control() && c != '\n'));
    }

    #[test]
    fn keeps_text_and_newlines() {
        assert_eq!(sanitize("héllo\n\tworld\r"), "héllo\n    world");
        assert_eq!(sanitize_line("a\nb"), "a⏎b");
    }
}
