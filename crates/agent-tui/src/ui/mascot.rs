//! The Duke sprite, drawn with text cells: each cell is `▀` with the top
//! pixel as foreground and the bottom pixel as background. Encoded from the
//! owner-supplied `assets/duke-sprite.png` (sampled at 16×12).

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::skins::{rgb, ColorMode};

/// Y hair · S skin · K black · R shirt · B pants · `.` transparent.
#[rustfmt::skip]
pub const FULL: [&str; 12] = [
    "..YYYYYYYYYYYY..",
    "..YSSYYYYYYSSY..",
    "..YSSSSSSSSSSY..",
    "..SKKKKSSKKKKS..",
    "..KKKKKKKKKKKK..",
    "..SSSSSSSSSSSS..",
    "..RRSSSSSSSSRR..",
    "SSSRRRRRRRRRRSSS",
    "SSSRRRRRRRRRRSSS",
    "..BBBBBBBBBBBB..",
    "..BBBBBBBBBBBB..",
    "...K..K..K..K...",
];

/// Header-sized version (8×6 pixels → 8 cols × 3 rows).
#[rustfmt::skip]
pub const COMPACT: [&str; 6] = [
    ".YYYYYY.",
    ".KKKKKK.",
    ".SSSSSS.",
    "SRRRRRRS",
    ".BBBBBB.",
    "..K..K..",
];

pub const PALETTE_KEYS: &str = "YSKRB.";

fn color(mode: ColorMode, key: u8) -> Option<Color> {
    let hex = match key {
        b'Y' => 0xFFDC1E,
        b'S' => 0xE67E55,
        b'K' => 0x0A0A0A,
        b'R' => 0xE10019,
        b'B' => 0x003F8C,
        _ => return None,
    };
    Some(rgb(mode, hex))
}

/// Renders a pixel grid into half-block lines; transparent pixels use `bg`.
pub fn render(grid: &[&str], mode: ColorMode, bg: Color) -> Vec<Line<'static>> {
    grid.chunks(2)
        .map(|pair| {
            let top = pair[0].as_bytes();
            let bottom = pair.get(1).map(|r| r.as_bytes());
            let spans = (0..top.len())
                .map(|x| {
                    let t = color(mode, top[x]);
                    let b = bottom.and_then(|r| color(mode, r[x]));
                    match (t, b) {
                        (None, None) => Span::styled(" ", Style::default().bg(bg)),
                        (Some(t), None) => Span::styled("▀", Style::default().fg(t).bg(bg)),
                        (None, Some(b)) => Span::styled("▄", Style::default().fg(b).bg(bg)),
                        (Some(t), Some(b)) => Span::styled("▀", Style::default().fg(t).bg(b)),
                    }
                })
                .collect::<Vec<_>>();
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(grid: &[&str], w: usize, h: usize) {
        assert_eq!(grid.len(), h);
        for row in grid {
            assert_eq!(row.len(), w, "row {row:?}");
            assert!(row.chars().all(|c| PALETTE_KEYS.contains(c)), "row {row:?}");
            let rev: String = row.chars().rev().collect();
            assert_eq!(&rev, row, "row {row:?} is not mirror-symmetric");
        }
    }

    #[test]
    fn full_grid_is_16x12_symmetric_palette_only() {
        check(&FULL, 16, 12);
    }

    #[test]
    fn compact_grid_is_8x6_symmetric_palette_only() {
        check(&COMPACT, 8, 6);
    }

    #[test]
    fn renders_as_half_blocks() {
        let lines = render(&FULL, ColorMode::TrueColor, Color::Black);
        assert_eq!(lines.len(), 6);
        assert!(lines.iter().all(|l| l.spans.len() == 16));
    }
}
