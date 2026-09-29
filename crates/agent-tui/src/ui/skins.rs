//! Six skins: palettes (truecolor with a 256-color fallback) and the welcome
//! text that belongs to each.

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkinId {
    Graphite,
    Matrix,
    Dracula,
    Nord,
    SolarizedLight,
    Duke,
}

pub const ALL_SKINS: [SkinId; 6] = [
    SkinId::Graphite,
    SkinId::Matrix,
    SkinId::Dracula,
    SkinId::Nord,
    SkinId::SolarizedLight,
    SkinId::Duke,
];

impl SkinId {
    pub fn label(self) -> &'static str {
        match self {
            SkinId::Graphite => "Graphite",
            SkinId::Matrix => "Matrix",
            SkinId::Dracula => "Dracula",
            SkinId::Nord => "Nord",
            SkinId::SolarizedLight => "Solarized Light",
            SkinId::Duke => "Duke Nukem 3D",
        }
    }

    /// Short names accepted by `/theme` and `--skin`.
    pub fn slug(self) -> &'static str {
        match self {
            SkinId::Graphite => "graphite",
            SkinId::Matrix => "matrix",
            SkinId::Dracula => "dracula",
            SkinId::Nord => "nord",
            SkinId::SolarizedLight => "solarized",
            SkinId::Duke => "duke",
        }
    }

    pub fn parse(s: &str) -> Option<SkinId> {
        let norm: String = s
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect();
        ALL_SKINS.iter().copied().find(|k| {
            let label: String = k
                .label()
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect();
            norm == k.slug() || norm == label || (norm.len() >= 3 && label.starts_with(&norm))
        })
    }

    /// Candidate eyebrow lines; one is picked when the skin is selected.
    pub fn quotes(self) -> &'static [&'static str] {
        match self {
            SkinId::Matrix => &[
                "I know kung fu.",
                "Wake up, Neo.",
                "Follow the white rabbit.",
                "There is no spoon.",
            ],
            SkinId::Duke => &[
                "Hail to the king, baby.",
                "Come get some.",
                "Damn, I'm good.",
            ],
            _ => &["Ready when you are"],
        }
    }

    pub fn heading(self) -> &'static str {
        match self {
            SkinId::Matrix => "Never send a human to do a machine's job.",
            SkinId::Duke => "It's time to kick ass and chew bubble gum…",
            _ => "What are we building?",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
}

impl ColorMode {
    pub fn detect() -> ColorMode {
        match std::env::var("COLORTERM").map(|v| v.to_lowercase()) {
            Ok(v) if v.contains("truecolor") || v.contains("24bit") => ColorMode::TrueColor,
            _ => ColorMode::Ansi256,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub skin: SkinId,
    pub mode: ColorMode,
    pub bg: Color,
    pub panel: Color,
    pub surface: Color,
    pub line: Color,
    pub ink: Color,
    pub muted: Color,
    pub accent: Color,
    pub warn: Color,
    pub danger: Color,
    pub selected: Color,
    pub on_accent: Color,
    pub user_bg: Color,
    pub header_rule: Color,
    pub composer_rule: Color,
    pub add: Color,
    pub del: Color,
    pub highlight: Color,
}

struct Palette {
    bg: u32,
    panel: u32,
    surface: u32,
    line: u32,
    ink: u32,
    muted: u32,
    accent: u32,
    warn: u32,
    danger: u32,
    selected: u32,
    on_accent: u32,
    user_bg: u32,
    header_rule: u32,
    composer_rule: u32,
    add: u32,
    del: u32,
    highlight: u32,
}

fn palette(skin: SkinId) -> Palette {
    match skin {
        SkinId::Graphite => Palette {
            bg: 0x101314,
            panel: 0x171b1c,
            surface: 0x191f1b,
            line: 0x303737,
            ink: 0xe1e6df,
            muted: 0x8c9993,
            accent: 0xbdde9c,
            warn: 0xeac38c,
            danger: 0xe38c7f,
            selected: 0x303b31,
            on_accent: 0x192016,
            user_bg: 0x1f2723,
            header_rule: 0x303737,
            composer_rule: 0x526044,
            add: 0xbdde9c,
            del: 0xe38c7f,
            highlight: 0x5b4a22,
        },
        SkinId::Matrix => Palette {
            bg: 0x030b05,
            panel: 0x08140b,
            surface: 0x0b1c0e,
            line: 0x20482c,
            ink: 0xb5ffc5,
            muted: 0x79b788,
            accent: 0x54ff85,
            warn: 0xe5f58a,
            danger: 0xff6b6b,
            selected: 0x12371e,
            on_accent: 0x031308,
            user_bg: 0x0d2211,
            header_rule: 0x20482c,
            composer_rule: 0x2f7a45,
            add: 0x54ff85,
            del: 0xff6b6b,
            highlight: 0x3d4a12,
        },
        SkinId::Dracula => Palette {
            bg: 0x282a36,
            panel: 0x303240,
            surface: 0x343646,
            line: 0x57576c,
            ink: 0xf8f8f2,
            muted: 0xb5b4cb,
            accent: 0xbd93f9,
            warn: 0xffb86c,
            danger: 0xff5555,
            selected: 0x49365f,
            on_accent: 0x21172e,
            user_bg: 0x363849,
            header_rule: 0x57576c,
            composer_rule: 0x6c5a93,
            add: 0x50fa7b,
            del: 0xff5555,
            highlight: 0x6b5a2e,
        },
        SkinId::Nord => Palette {
            bg: 0x2e3440,
            panel: 0x343d4c,
            surface: 0x3b4556,
            line: 0x59677b,
            ink: 0xeceff4,
            muted: 0xb0bfd1,
            accent: 0x88c0d0,
            warn: 0xebcb8b,
            danger: 0xbf616a,
            selected: 0x3c5766,
            on_accent: 0x202f3a,
            user_bg: 0x3b4556,
            header_rule: 0x59677b,
            composer_rule: 0x5e81ac,
            add: 0xa3be8c,
            del: 0xbf616a,
            highlight: 0x6a5d3a,
        },
        SkinId::SolarizedLight => Palette {
            bg: 0xfdf6e3,
            panel: 0xeee8d5,
            surface: 0xeee8d5,
            line: 0xbdbaa7,
            ink: 0x34494c,
            muted: 0x586e75,
            accent: 0x006f78,
            warn: 0x875700,
            danger: 0xb2322c,
            selected: 0xdce9de,
            on_accent: 0xfff9e8,
            user_bg: 0xeee8d5,
            header_rule: 0xbdbaa7,
            composer_rule: 0x6c9a9e,
            add: 0x4f7a00,
            del: 0xb2322c,
            highlight: 0xf2d98a,
        },
        SkinId::Duke => Palette {
            bg: 0x1d1c19,
            panel: 0x2b2924,
            surface: 0x343029,
            line: 0x6a5b3e,
            ink: 0xf3e5c9,
            muted: 0xc0ad8c,
            accent: 0xffc247,
            warn: 0xff8268,
            danger: 0xff5a4a,
            selected: 0x504024,
            on_accent: 0x211b0d,
            user_bg: 0x343029,
            header_rule: 0xa73d2d,
            composer_rule: 0xa73d2d,
            add: 0xffc247,
            del: 0xff5a4a,
            highlight: 0x7a4b1a,
        },
    }
}

pub fn rgb(mode: ColorMode, hex: u32) -> Color {
    let (r, g, b) = ((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    match mode {
        ColorMode::TrueColor => Color::Rgb(r, g, b),
        ColorMode::Ansi256 => Color::Indexed(ansi256(r, g, b)),
    }
}

/// Nearest xterm-256 index (6×6×6 cube or grayscale ramp).
pub fn ansi256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest = |v: u8| -> usize {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, l)| (i32::from(**l) - i32::from(v)).abs())
            .map(|(i, _)| i)
            .unwrap()
    };
    let (ri, gi, bi) = (nearest(r), nearest(g), nearest(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let dist = |c: (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(c.0, r) + d(c.1, g) + d(c.2, b)
    };
    let avg = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gi_idx = if avg < 8 { 0 } else { ((avg - 8) / 10).min(23) } as u8;
    let gv = 8 + 10 * gi_idx;
    if dist((gv, gv, gv)) < dist(cube) {
        232 + gi_idx
    } else {
        16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8
    }
}

impl Theme {
    pub fn new(skin: SkinId, mode: ColorMode) -> Theme {
        let p = palette(skin);
        let c = |h| rgb(mode, h);
        Theme {
            skin,
            mode,
            bg: c(p.bg),
            panel: c(p.panel),
            surface: c(p.surface),
            line: c(p.line),
            ink: c(p.ink),
            muted: c(p.muted),
            accent: c(p.accent),
            warn: c(p.warn),
            danger: c(p.danger),
            selected: c(p.selected),
            on_accent: c(p.on_accent),
            user_bg: c(p.user_bg),
            header_rule: c(p.header_rule),
            composer_rule: c(p.composer_rule),
            add: c(p.add),
            del: c(p.del),
            highlight: c(p.highlight),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skins() {
        assert_eq!(SkinId::parse("matrix"), Some(SkinId::Matrix));
        assert_eq!(SkinId::parse("Duke Nukem 3D"), Some(SkinId::Duke));
        assert_eq!(
            SkinId::parse("solarized-light"),
            Some(SkinId::SolarizedLight)
        );
        assert_eq!(SkinId::parse("sol"), Some(SkinId::SolarizedLight));
        assert_eq!(SkinId::parse("nope"), None);
    }

    #[test]
    fn welcome_text_per_skin() {
        for s in [
            SkinId::Graphite,
            SkinId::Dracula,
            SkinId::Nord,
            SkinId::SolarizedLight,
        ] {
            assert_eq!(s.quotes(), &["Ready when you are"]);
            assert_eq!(s.heading(), "What are we building?");
        }
        assert!(SkinId::Matrix.quotes().contains(&"I know kung fu."));
        assert!(SkinId::Duke.quotes().contains(&"Hail to the king, baby."));
    }

    #[test]
    fn ansi256_fallback() {
        assert_eq!(ansi256(0, 0, 0), 16);
        assert_eq!(ansi256(255, 255, 255), 231);
        assert_eq!(ansi256(0x80, 0x80, 0x80), 244);
        assert_eq!(ansi256(255, 0, 0), 196);
    }
}
