use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub const LONGEST: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Rx,
    Tx,
    Event,
}

impl Dir {
    pub fn mark(self) -> &'static str {
        match self {
            Dir::Rx => "<",
            Dir::Tx => ">",
            Dir::Event => "#",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    EspIdf,
    Zephyr,
    Arduino,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::EspIdf, Format::Zephyr, Format::Arduino];

    pub fn name(self) -> &'static str {
        match self {
            Format::EspIdf => "ESP-IDF",
            Format::Zephyr => "Zephyr",
            Format::Arduino => "Arduino",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub seq: u64,
    pub t: i64,
    pub dir: Dir,
    pub text: String,
    pub level: Option<char>,
}

impl Row {
    pub fn to_json(&self) -> Value {
        let mut out = json!({ "n": self.seq, "t": self.t, "d": self.dir.mark(), "text": self.text });
        if let Some(level) = self.level {
            out["l"] = json!(level.to_string());
        }
        out
    }
}

pub struct Line {
    pub t: i64,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Splitter {
    partial: Option<Line>,
    after_cr: bool,
}

impl Splitter {
    pub fn feed(&mut self, t: i64, data: &[u8]) -> Vec<Line> {
        let mut out = Vec::new();
        for &byte in data {
            if self.after_cr {
                self.after_cr = false;
                if byte == b'\n' {
                    continue;
                }
            }
            match byte {
                b'\n' => out.push(self.partial.take().unwrap_or(Line { t, bytes: Vec::new() })),
                b'\r' => {
                    out.push(self.partial.take().unwrap_or(Line { t, bytes: Vec::new() }));
                    self.after_cr = true;
                }
                byte => {
                    let line = self.partial.get_or_insert_with(|| Line { t, bytes: Vec::new() });
                    line.bytes.push(byte);
                    if line.bytes.len() >= LONGEST {
                        out.extend(self.partial.take());
                    }
                }
            }
        }
        out
    }

    pub fn partial(&self) -> Option<&Line> {
        self.partial.as_ref()
    }

    pub fn take_partial(&mut self) -> Option<Line> {
        self.partial.take()
    }

    pub fn reset(&mut self) {
        self.partial = None;
        self.after_cr = false;
    }
}

pub fn escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    while !rest.is_empty() {
        let (valid, bad) = match std::str::from_utf8(rest) {
            Ok(text) => (text, &[][..]),
            Err(error) => {
                let (good, after) = rest.split_at(error.valid_up_to());
                let skip = error.error_len().unwrap_or(after.len()).max(1);
                (std::str::from_utf8(good).unwrap_or_default(), &after[..skip])
            }
        };
        for c in valid.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\t' | '\x1b' => out.push(c),
                c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
                c if (0x80..0xa0).contains(&(c as u32)) => {
                    let mut utf8 = [0u8; 2];
                    for byte in c.encode_utf8(&mut utf8).bytes() {
                        out.push_str(&format!("\\x{byte:02x}"));
                    }
                }
                c => out.push(c),
            }
        }
        for byte in bad {
            out.push_str(&format!("\\x{byte:02x}"));
        }
        rest = &rest[valid.len() + bad.len()..];
    }
    out
}


pub fn strip_ansi(text: &str) -> String {
    static CSI: OnceLock<Regex> = OnceLock::new();
    CSI.get_or_init(|| Regex::new(r"\x1b(\[[0-?]*[ -/]*[@-~]|[@-Z\\-_])").unwrap()).replace_all(text, "").into_owned()
}

pub fn level(text: &str) -> (Option<char>, Option<Format>) {
    static PATTERNS: OnceLock<[(Format, Regex); 3]> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            (Format::EspIdf, Regex::new(r"^([EWIDV]) \(\d+\) ").unwrap()),
            (Format::Zephyr, Regex::new(r"^\[\d+:\d{2}:\d{2}\.\d{3},\d{3}\] <(err|wrn|inf|dbg)> ").unwrap()),
            (Format::Arduino, Regex::new(r"^(?:\[\s*\d+\])?\[([EWIDV])\]\[[^\]]*\]").unwrap()),
        ]
    });
    let plain = strip_ansi(text);
    for (format, pattern) in patterns {
        if let Some(found) = pattern.captures(&plain) {
            let level = match &found[1] {
                "err" => 'E',
                "wrn" => 'W',
                "inf" => 'I',
                "dbg" => 'D',
                other => other.chars().next().unwrap_or('I'),
            };
            return (Some(level), Some(*format));
        }
    }
    const PANICS: [&str; 4] = ["Guru Meditation Error", "abort() was called", "assert failed", "***** Kernel panic"];
    if PANICS.iter().any(|p| plain.contains(p)) {
        return (Some('E'), None);
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unescape(text: &str) -> Vec<u8> {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'\\' {
                if bytes.get(i + 1) == Some(&b'\\') {
                    out.push(b'\\');
                    i += 2;
                    continue;
                }
                if bytes.get(i + 1) == Some(&b'x') {
                    if let Some(byte) = text.get(i + 2..i + 4).and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                        out.push(byte);
                        i += 4;
                        continue;
                    }
                }
            }
            out.push(bytes[i]);
            i += 1;
        }
        out
    }

    fn texts(lines: Vec<Line>) -> Vec<String> {
        lines.into_iter().map(|l| escape(&l.bytes)).collect()
    }

    #[test]
    fn lines_end_at_lf_crlf_or_a_lone_cr_even_across_chunks() {
        let mut s = Splitter::default();
        assert_eq!(texts(s.feed(1, b"one\r\ntwo\nthr")), ["one", "two"]);
        assert_eq!(escape(&s.partial().unwrap().bytes), "thr");
        assert_eq!(texts(s.feed(2, b"ee\r")), ["three"]);
        assert_eq!(texts(s.feed(3, b"\nfour\rfive\n")), ["four", "five"]);
        assert!(s.partial().is_none());
    }

    #[test]
    fn a_line_with_no_end_is_cut_at_the_longest() {
        let mut s = Splitter::default();
        let lines = s.feed(1, &vec![b'a'; LONGEST * 2 + 5]);
        assert_eq!(lines.iter().map(|l| l.bytes.len()).collect::<Vec<_>>(), [LONGEST, LONGEST]);
        assert_eq!(s.partial().unwrap().bytes.len(), 5);
    }

    #[test]
    fn a_line_keeps_the_time_of_its_first_byte() {
        let mut s = Splitter::default();
        s.feed(10, b"uart:~$ ");
        let lines = s.feed(20, b"x\n");
        assert_eq!(lines[0].t, 10);
    }

    #[test]
    fn bytes_that_are_not_text_come_back_exactly() {
        let raw: Vec<u8> = vec![b'a', 0, 0xff, 0xfe, b'\\', 0x1b, b'[', b'0', b'm', b'\t', 0x7f, 0xc3, 0xa9, 0xe2, 0x82];
        let text = escape(&raw);
        assert_eq!(text, "a\\x00\\xff\\xfe\\\\\x1b[0m\t\\x7fé\\xe2\\x82");
        assert_eq!(unescape(&text), raw);
    }

    #[test]
    fn c1_controls_are_escaped_as_their_bytes() {
        let raw = "a\u{9b}2Jb".as_bytes().to_vec();
        let text = escape(&raw);
        assert_eq!(text, "a\\xc2\\x9b2Jb");
        assert_eq!(unescape(&text), raw);
    }

    #[test]
    fn levels_of_the_three_formats_and_panics() {
        assert_eq!(level("\x1b[0;31mE (5341) wifi: connect failed\x1b[0m"), (Some('E'), Some(Format::EspIdf)));
        assert_eq!(level("W (201) spi_flash: big"), (Some('W'), Some(Format::EspIdf)));
        assert_eq!(level("[00:00:02.001,220] <err> bme280: read failed"), (Some('E'), Some(Format::Zephyr)));
        assert_eq!(level("[00:00:00.006,103] <dbg> bme280: id"), (Some('D'), Some(Format::Zephyr)));
        assert_eq!(level("[  1234][W][main.cpp:12] loop(): slow"), (Some('W'), Some(Format::Arduino)));
        assert_eq!(level("[I][wifi.cpp:3] begin(): up"), (Some('I'), Some(Format::Arduino)));
        assert_eq!(level("Guru Meditation Error: Core  0 panic'ed (LoadProhibited)."), (Some('E'), None));
        assert_eq!(level("hello"), (None, None));
        assert_eq!(level("Executing (1)"), (None, None));
    }
}
