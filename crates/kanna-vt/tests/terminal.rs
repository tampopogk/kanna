use kanna_vt::{Changed, Colors, CursorStyle, Lines, Mode, Need, ProgramState, ProgramStatus, Progress, Scheme, Terminal, MAX_PROGRAM_STATUSES};

fn terminal(cols: u16, rows: u16, output: &str) -> Terminal {
    let mut t = Terminal::new(cols, rows, 1000).unwrap();
    t.write(output.as_bytes());
    t
}

#[test]
fn text_after_writes() {
    let mut t = terminal(20, 5, "hello\r\nworld   ");
    assert_eq!(t.text(Lines::Screen), "hello\nworld");
    assert_eq!(t.cursor(), (8, 1));
    let bytes = "\x1b[1;31mred\x1b[0m é 字".as_bytes();
    t.write(b"\r\n");
    for b in bytes {
        t.write(std::slice::from_ref(b));
    }
    assert_eq!(t.text(Lines::Screen), "hello\nworld\nred é 字");
    t.write(b"\x1b[2J\x1b[Htop\x1b[3;5Hmid");
    assert_eq!(t.text(Lines::Screen), "top\n\n    mid");
    assert_eq!(t.size(), (20, 5));
}

#[test]
fn empty_terminal_is_empty_text() {
    let t = Terminal::new(80, 24, 100).unwrap();
    assert_eq!(t.text(Lines::Screen), "");
    assert_eq!(t.text(Lines::Last(10)), "");
    assert_eq!(t.title(), "");
    assert_eq!(t.cursor(), (0, 0));
}

#[test]
fn long_lines_wrap_into_rows() {
    let t = terminal(5, 4, "abcdefghij12\r\nx");
    assert_eq!(t.text(Lines::Screen), "abcde\nfghij\n12\nx");
}

#[test]
fn alternate_screen_and_back() {
    let mut t = terminal(10, 3, "shell$ top");
    t.write(b"\x1b[?1049h\x1b[H\x1b[2Jfull\r\nscreen");
    assert!(t.alternate_screen());
    assert_eq!(t.text(Lines::Screen), "full\nscreen");
    assert_eq!(t.text(Lines::Last(100)), "full\nscreen");
    t.write(b"\x1b[?1049l");
    assert!(!t.alternate_screen());
    assert_eq!(t.text(Lines::Screen), "shell$ top");
}

#[test]
fn resize_keeps_text_and_reports_size() {
    let mut t = terminal(10, 4, "a\r\nb\r\nc\r\nd");
    t.resize(10, 2).unwrap();
    assert_eq!(t.size(), (10, 2));
    assert_eq!(t.text(Lines::Screen), "c\nd");
    assert_eq!(t.text(Lines::Last(10)), "a\nb\nc\nd");
    let mut t = terminal(10, 3, "abcdefgh");
    t.resize(4, 3).unwrap();
    assert_eq!(t.text(Lines::Screen), "abcd\nefgh");
    t.resize(0, 0).unwrap();
    assert_eq!(t.size(), (1, 1));
}

#[test]
fn last_lines_count_scrollback() {
    let mut t = Terminal::new(10, 3, 1000).unwrap();
    for i in 0..20 {
        t.write(format!("line {i}\r\n").as_bytes());
    }
    assert_eq!(t.text(Lines::Screen), "line 18\nline 19");
    assert_eq!(t.text(Lines::Last(4)), "line 16\nline 17\nline 18\nline 19");
    assert_eq!(t.text(Lines::Last(1)), "line 19");
    let all: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
    assert_eq!(t.text(Lines::Last(1000)), all.join("\n"));
    assert_eq!(t.text(Lines::Last(0)), "");
}

#[test]
fn scrollback_is_limited_in_whole_pages() {
    let mut t = Terminal::new(10, 3, 0).unwrap();
    t.set_scrollback_lines(100).unwrap();
    assert_eq!(t.scrollback_lines(), Some(100));
    for i in 0..50_000 {
        t.write(format!("line {i}\r\n").as_bytes());
    }
    assert!(t.scrollback_rows() < 5_000, "{} rows kept", t.scrollback_rows());
    assert_eq!(t.text(Lines::Last(1)), "line 49999");
}

#[test]
fn the_line_limit_governs_scrollback_unless_bytes_are_limited_too() {
    let mut t = Terminal::new(80, 24, 10_000).unwrap();
    assert_eq!(t.scrollback_bytes(), None);
    for i in 0..30_000 {
        t.write(format!("line {i}\r\n").as_bytes());
    }
    assert!((9_500..15_000).contains(&t.scrollback_rows()), "{} rows kept", t.scrollback_rows());
    t.set_scrollback_bytes(Some(1)).unwrap();
    assert_eq!(t.scrollback_bytes(), Some(1));
    t.write(b"more\r\n");
    assert!(t.scrollback_rows() < 5_000, "{} rows kept", t.scrollback_rows());
    t.set_scrollback_bytes(None).unwrap();
    assert_eq!(t.scrollback_bytes(), None);
}

#[test]
fn progress_reports_and_synchronized_output() {
    let mut t = terminal(20, 5, "");
    assert_eq!(t.progress(), Progress::None);
    t.write(b"\x1b]9;4;3\x07");
    assert_eq!(t.progress(), Progress::Indeterminate);
    t.write(b"\x1b]9;4;1;40\x07");
    assert_eq!(t.progress(), Progress::Set(40));
    t.write(b"\x1b]9;4;0\x07");
    assert_eq!(t.progress(), Progress::None);
    assert!(!t.mode(Mode::SYNC_OUTPUT));
    t.write(b"\x1b[?2026h");
    assert!(t.mode(Mode::SYNC_OUTPUT));
    t.write(b"\x1b[?2026l");
    assert!(!t.mode(Mode::SYNC_OUTPUT));
}

#[test]
fn title_and_pwd() {
    let t = terminal(20, 3, "\x1b]0;my title\x07\x1b]7;file://host/tmp/x\x1b\\");
    assert_eq!(t.title(), "my title");
    assert_eq!(t.pwd(), "file://host/tmp/x");
}

fn theme() -> Colors {
    let mut palette = Box::new([[0u8; 3]; 256]);
    for (i, c) in palette.iter_mut().enumerate() {
        *c = [i as u8, 255 - i as u8, 0x40];
    }
    Colors { foreground: [0xe5, 0xd4, 0xb0], background: [0x12, 0x34, 0x56], cursor: Some([0x2e, 0xc4, 0xb6]), palette }
}

#[test]
fn color_queries_are_answered_from_the_theme() {
    let mut t = terminal(20, 5, "\x1b]11;?\x07");
    assert!(t.take_replies().is_empty());
    t.set_default_colors(&theme()).unwrap();
    t.write(b"\x1b]11;?\x07\x1b]10;?\x1b\\\x1b]12;?\x07\x1b]4;1;?\x07");
    assert_eq!(
        String::from_utf8(t.take_replies()).unwrap(),
        "\x1b]11;rgb:1212/3434/5656\x07\x1b]10;rgb:e5e5/d4d4/b0b0\x1b\\\x1b]12;rgb:2e2e/c4c4/b6b6\x07\x1b]4;1;rgb:0101/fefe/4040\x07"
    );
    t.write(b"\x1b]11;#ffffff\x07\x1b]11;?\x07");
    assert_eq!(t.take_replies(), b"\x1b]11;rgb:ffff/ffff/ffff\x07");
    t.set_default_colors(&theme()).unwrap();
    t.write(b"\x1b]111\x07\x1b]11;?\x07");
    assert_eq!(t.take_replies(), b"\x1b]11;rgb:1212/3434/5656\x07");
    let r = Terminal::from_snapshot(&t.snapshot().unwrap()).unwrap();
    assert_eq!(r.background(), Some([0x12, 0x34, 0x56]));
}

#[test]
fn the_color_scheme_is_reported_when_asked_and_when_it_changes() {
    let mut t = terminal(20, 5, "\x1b[?996n");
    assert!(t.take_replies().is_empty());
    t.set_color_scheme(Some(Scheme::Dark));
    assert!(t.take_replies().is_empty());
    t.write(b"\x1b[?996n");
    assert_eq!(t.take_replies(), b"\x1b[?997;1n");
    t.write(b"\x1b[?2031h");
    t.set_color_scheme(Some(Scheme::Dark));
    assert!(t.take_replies().is_empty());
    t.set_color_scheme(Some(Scheme::Light));
    assert_eq!(t.take_replies(), b"\x1b[?997;2n");
}

#[test]
fn defaults_follow_the_apps_config() {
    let mut t = terminal(20, 5, "");
    assert!(!t.mode(Mode::GRAPHEME_CLUSTER));
    t.set_mode_default(Mode::GRAPHEME_CLUSTER, true).unwrap();
    assert!(t.mode(Mode::GRAPHEME_CLUSTER));
    t.write(b"\x1b[?2027l");
    assert!(!t.mode(Mode::GRAPHEME_CLUSTER));
    t.write(b"\x1bc");
    assert!(t.mode(Mode::GRAPHEME_CLUSTER));
    t.set_default_cursor(CursorStyle::Bar, false).unwrap();
    t.write(b"\x1b[?12$p");
    assert_eq!(t.take_replies(), b"\x1b[?12;2$y");
    t.write(b"\x1b[0 q\x1bP$q q\x1b\\");
    assert_eq!(t.take_replies(), b"\x1bP1$r6 q\x1b\\");
}

#[test]
fn title_and_pwd_changes_are_noticed() {
    let mut t = terminal(20, 3, "plain");
    assert_eq!(t.take_changed(), Changed::default());
    t.write(b"\x1b]2;t\x07");
    assert_eq!(t.take_changed(), Changed { title: true, pwd: false });
    t.write(b"\x1b]7;file://h/a\x07");
    assert_eq!(t.take_changed(), Changed { title: false, pwd: true });
    assert_eq!(t.take_changed(), Changed::default());
}

#[test]
fn queries_get_replies() {
    let mut t = terminal(20, 5, "ab\x1b[6n");
    assert_eq!(t.take_replies(), b"\x1b[1;3R");
    assert!(t.take_replies().is_empty());
}

#[test]
fn device_attributes_are_ghosttys() {
    let mut t = terminal(20, 5, "\x1b[c\x1b[>c");
    assert_eq!(t.take_replies(), b"\x1b[?62;22;52c\x1b[>1;10;0c");
    t.write(b"\x1b[>q");
    assert_eq!(String::from_utf8_lossy(&t.take_replies()), "\x1bP>|libghostty\x1b\\");
}

#[test]
fn size_reports_follow_the_grid() {
    let mut t = terminal(20, 5, "\x1b[18t");
    assert_eq!(t.take_replies(), b"\x1b[8;5;20t");
    t.resize(30, 7).unwrap();
    t.set_cell_pixels(10, 20);
    t.write(b"\x1b[18t\x1b[14t\x1b[16t");
    assert_eq!(t.take_replies(), b"\x1b[8;7;30t\x1b[4;140;300t\x1b[6;20;10t");
    t.write(b"\x1b[?2048h");
    assert_eq!(t.take_replies(), b"\x1b[48;7;30;140;300t");
    let mut r = Terminal::from_snapshot(&t.snapshot().unwrap()).unwrap();
    r.write(b"\x1b[18t");
    assert_eq!(r.take_replies(), b"\x1b[8;7;30t");
}

#[test]
fn snapshot_round_trip() {
    let mut t = Terminal::new(30, 6, 1000).unwrap();
    for i in 0..40 {
        t.write(format!("\x1b[3{}mline {i}\x1b[0m\r\n", i % 8).as_bytes());
    }
    t.write("\x1b]2;snap\x07wide 字 é".as_bytes());
    let snap = t.snapshot().unwrap();
    assert!(snap.starts_with(b"GHOSTSNP"));
    let r = Terminal::from_snapshot(&snap).unwrap();
    assert_eq!(r.size(), t.size());
    assert_eq!(r.cursor(), t.cursor());
    assert_eq!(r.title(), "snap");
    assert_eq!(r.text(Lines::Screen), t.text(Lines::Screen));
    assert_eq!(r.text(Lines::Last(1000)), t.text(Lines::Last(1000)));
    let (mut a, mut b) = (t, r);
    for out in [&mut a, &mut b] {
        out.write(b" more\r\nnext");
    }
    assert_eq!(a.text(Lines::Last(1000)), b.text(Lines::Last(1000)));
    let again = Terminal::from_snapshot(&b.snapshot().unwrap()).unwrap();
    assert_eq!(again.text(Lines::Last(1000)), a.text(Lines::Last(1000)));
}

#[test]
fn snapshot_of_alternate_screen_and_unfinished_sequence() {
    let mut t = terminal(20, 4, "shell\x1b[?1049h\x1b[Htop");
    t.write(b"\x1b[3");
    let mut r = Terminal::from_snapshot(&t.snapshot().unwrap()).unwrap();
    assert!(r.alternate_screen());
    for out in [&mut t, &mut r] {
        out.write(b";1Hx\x1b[?1049l");
    }
    assert_eq!(r.text(Lines::Screen), t.text(Lines::Screen));
    assert_eq!(r.text(Lines::Screen), "shell");
}

#[test]
fn bad_snapshots_are_refused() {
    let t = terminal(20, 4, "hello");
    let snap = t.snapshot().unwrap();
    assert!(Terminal::from_snapshot(b"").is_err());
    assert!(Terminal::from_snapshot(b"GHOSTSNP").is_err());
    assert!(Terminal::from_snapshot(&snap[..snap.len() / 2]).is_err());
    assert!(Terminal::from_snapshot(&snap[..snap.len() - 1]).is_err());
    let mut flipped = snap.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0x40;
    assert!(Terminal::from_snapshot(&flipped).is_err());
}

#[test]
fn snapshot_inside_an_apc_ended_by_a_c1_byte_keeps_the_new_sequence() {
    let mut t = terminal(20, 4, "before");
    t.write(b"\x1b_a\x9d");
    let mut r = Terminal::from_snapshot(&t.snapshot().unwrap()).unwrap();
    t.write(b"2;title\x07after");
    r.write(b"2;title\x07after");
    assert_eq!(r.text(Lines::Screen), "beforeafter");
    assert_eq!(r.text(Lines::Screen), t.text(Lines::Screen));
}

#[test]
fn snapshot_waits_for_an_overlong_unfinished_sequence_to_end() {
    let mut t = terminal(20, 4, "before");
    let mut osc = b"\x1b]2;".to_vec();
    osc.resize(osc.len() + kanna_vt::MAX_UNFINISHED_SEQUENCE_BYTES + 1, b'x');
    t.write(&osc);
    assert!(t.snapshot().is_err());
    t.write(b"\x07after");
    let r = Terminal::from_snapshot(&t.snapshot().unwrap()).unwrap();
    assert_eq!(r.text(Lines::Screen), "beforeafter");
}

#[test]
fn snapshot_after_widening_reflow_keeps_all_history() {
    let mut t = Terminal::from_snapshot(include_bytes!("fixtures/reflow-drops-history.snap")).unwrap();
    assert_eq!(t.size(), (126, 1));
    t.resize(204, 86).unwrap();
    let r = Terminal::from_snapshot(&t.snapshot().unwrap()).unwrap();
    assert_eq!(r.scrollback_rows(), t.scrollback_rows());
    assert_eq!(r.text(Lines::Last(10_000)), t.text(Lines::Last(10_000)));
}

#[test]
fn widening_a_full_scrollback_restores_what_the_screen_keeps() {
    let mut t = Terminal::new(80, 24, 10_000).unwrap();
    t.set_scrollback_bytes(Some(10_000_000)).unwrap();
    for i in 0..30_000 {
        t.write(format!("line {i} {}\r\n", "x".repeat(i % 70)).as_bytes());
    }
    for (cols, rows) in [(200, 50), (60, 20), (240, 60)] {
        t.resize(cols, rows).unwrap();
        let r = Terminal::from_snapshot(&t.snapshot().unwrap()).unwrap();
        assert_eq!(r.scrollback_rows(), t.scrollback_rows(), "at {cols}×{rows}");
        assert_eq!(r.text(Lines::Last(100_000)), t.text(Lines::Last(100_000)), "at {cols}×{rows}");
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
#[test]
fn hostile_output_never_panics() {
    let nasty: [&[u8]; 22] = [
        b"\x1b[99999999;99999999H",
        b"\x1b[0;0r",
        b"\x1b[65535;1r",
        b"\x1b[5;3r",
        b"\x1b[99999L",
        b"\x1b[99999M",
        b"\x1b[99999P",
        b"\x1b[99999@",
        b"\x1b[99999X",
        b"\x1b[99999S",
        b"\x1b[99999T",
        b"\x1b[99999A\x1b[99999D",
        b"\x1b[?1049h\x1b[?1049h\x1b[?47l\x1b[?1049l",
        b"\x1bM\x1bM\x1bD\x1bE\x1b8\x1bc",
        b"\xff\xfe\xc3\x28\xf0\x28\x8c\x28",
        b"\x1b[;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;H",
        b"\x1b[8;9999;9999t\x1b[4;99999;99999t\x1b[18t\x1b[14t",
        b"\x1b]4;1;rgb:ff/00/00\x07\x1b]10;?\x07\x1b]11;?\x1b\\\x1b]52;c;?\x07",
        b"\x1b_Ga=T,f=100,s=99999,v=99999;AAAA\x1b\\",
        b"\x1bP+q544e\x1b\\\x1b[c\x1b[>c\x1b[=c\x1b[>q\x1b[?996n",
        b"\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\\x1b[?2026h\x1b[?2026l",
        b"\x1b[>1u\x1b[=5;1u\x1b[<u\x1b(0lqk\x1b(B\x1b#8",
    ];
    let mut t = Terminal::new(80, 24, 2000).unwrap();
    for bytes in nasty {
        t.write(bytes);
        t.write("wide 字 and e\u{301} ".as_bytes());
        let _ = t.text(Lines::Screen);
    }
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let alphabet = b"\x1b[]?;:0123456789HJKLMPSTX@ABCDEFGdfhlmnqrstu78DcP_\\\r\n\x08\t\x07\x9b\x9dab";
    for round in 0..3000 {
        let len = (rng.next() % 300) as usize;
        let bytes: Vec<u8> = (0..len)
            .map(|_| {
                let r = rng.next();
                if r.is_multiple_of(4) { (r >> 8) as u8 } else { alphabet[(r >> 8) as usize % alphabet.len()] }
            })
            .collect();
        t.write(&bytes);
        if round % 50 == 0 {
            t.resize((rng.next() % 300) as u16, (rng.next() % 100) as u16).unwrap();
        }
        let _ = t.text(Lines::Screen);
        let _ = t.text(Lines::Last((rng.next() % 50) as usize + 1));
        let _ = (t.size(), t.cursor(), t.title(), t.pwd(), t.take_replies());
        if round % 100 == 0 {
            if let Ok(snap) = t.snapshot() {
                let r = Terminal::from_snapshot(&snap).expect("own snapshot loads");
                assert_eq!(r.size(), t.size());
                assert_eq!(r.cursor(), t.cursor());
                assert_eq!(r.text(Lines::Screen), t.text(Lines::Screen));
                let (kept, all) = (r.text(Lines::Last(500)), t.text(Lines::Last(500)));
                assert!(all.ends_with(&kept), "the restored history isn't the original's newest lines");
                t = r;
            }
        }
    }
}
#[test]
fn hostile_snapshots_never_panic() {
    let mut t = Terminal::new(40, 10, 1000).unwrap();
    for i in 0..60 {
        t.write(format!("\x1b[1;3{}mrow {i} 字\x1b[0m\r\n", i % 8).as_bytes());
    }
    let snap = t.snapshot().unwrap();
    let mut rng = Rng(0x2545f4914f6cdd1d);
    for _ in 0..2000 {
        let mut bytes = snap.clone();
        match rng.next() % 4 {
            0 => bytes.truncate((rng.next() as usize) % bytes.len()),
            1 => {
                for _ in 0..1 + rng.next() % 8 {
                    let i = (rng.next() as usize) % bytes.len();
                    bytes[i] = rng.next() as u8;
                }
            }
            2 => {
                let i = 10 + (rng.next() as usize) % (bytes.len() - 10);
                bytes[i] ^= 1 << (rng.next() % 8);
                assert!(Terminal::from_snapshot(&bytes).is_err(), "a flipped bit at {i} was accepted");
            }
            _ => bytes = (0..(rng.next() % 4096)).map(|_| rng.next() as u8).collect(),
        }
        if let Ok(mut r) = Terminal::from_snapshot(&bytes) {
            r.write(b"still works\r\n");
            let _ = r.text(Lines::Last(100));
        }
    }
}

#[test]
fn a_resize_is_reported_in_band_when_asked() {
    let mut t = terminal(20, 5, "\x1b[?2048h");
    t.take_replies();
    t.set_cell_pixels(10, 20);
    t.resize(30, 7).unwrap();
    assert_eq!(t.take_replies(), b"\x1b[48;7;30;140;300t");
}

fn states(t: &Terminal) -> Vec<(String, ProgramState)> {
    t.program_statuses().into_iter().map(|r| (r.id, r.state)).collect()
}

#[test]
fn the_program_status_query_is_answered_and_nothing_is_echoed() {
    let mut t = Terminal::new(20, 5, 0).unwrap();
    let answer = "\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth";
    t.write(b"\x1b]7501;?\x1b\\");
    assert_eq!(String::from_utf8(t.take_replies()).unwrap(), format!("{answer}\x1b\\"));
    t.write(b"\x1b]7501;?\x07");
    assert_eq!(String::from_utf8(t.take_replies()).unwrap(), format!("{answer}\x07"));
    t.write(b"\x1b]7501;state=blocked:kind=permission:app=terraform:msg=QXBwbHk/\x1b\\");
    assert!(t.take_replies().is_empty());
    assert_eq!(t.program_status_serial(), 1);
}

#[test]
fn a_program_status_report_is_parsed_whole() {
    let mut t = Terminal::new(20, 5, 0).unwrap();
    t.write(b"\x1b]7501;state=blocked:kind=permission:progress=40:app=terraform:title=QnVpbGQ=:msg=QXBwbHk/\x1b\\");
    let root = ProgramStatus {
        id: String::new(),
        state: ProgramState::Blocked,
        need: Some(Need::Permission),
        progress: Some(40),
        app: "terraform".into(),
        title: "Build".into(),
        message: "Apply?".into(),
    };
    assert_eq!(t.program_status(), Some(root.clone()));
    assert_eq!(t.program_statuses(), vec![root]);
    t.write(b"\x1b]7501;state=idle:kind=question:progress=10\x1b\\");
    let idle = t.program_status().unwrap();
    assert_eq!((idle.state, idle.need, idle.progress, idle.app.as_str()), (ProgramState::Idle, None, None, ""), "a report replaces its record");
    t.write(b"\x1b]7501;state=working:msg=YeKArmLigItj\x07");
    assert_eq!(t.program_status().unwrap().message, "abc", "invisible formatting is dropped");
    let serial = t.program_status_serial();
    t.write(b"\x1b]7501;state=sleeping\x1b\\\x1b]7501;progress=5\x1b\\");
    assert_eq!(t.program_status_serial(), serial, "a report that fails the spec's checks is ignored");
    assert_eq!(t.program_status().unwrap().state, ProgramState::Working);
}

#[test]
fn records_are_kept_per_id_and_cleared_with_their_children() {
    let mut t = Terminal::new(20, 5, 0).unwrap();
    t.write(b"\x1b]7501;state=working\x1b\\\x1b]7501;state=working:id=build\x1b\\\x1b]7501;state=done:id=build/test\x1b\\\x1b]7501;state=error:id=builder\x1b\\");
    assert_eq!(states(&t).len(), 4);
    t.write(b"\x1b]7501;state=clear:id=build\x1b\\");
    assert_eq!(states(&t), vec![(String::new(), ProgramState::Working), ("builder".into(), ProgramState::Error)]);
    t.write(b"\x1b]7501;state=clear\x1b\\");
    assert!(states(&t).is_empty());
}

#[test]
fn a_new_prompt_or_the_program_ending_drops_working_and_blocked() {
    let mut t = Terminal::new(20, 5, 0).unwrap();
    let all = b"\x1b]7501;state=working\x1b\\\x1b]7501;state=blocked:id=a\x1b\\\x1b]7501;state=done:id=b\x1b\\\x1b]7501;state=error:id=c\x1b\\\x1b]7501;state=idle:id=d\x1b\\";
    t.write(all);
    t.write(b"\x1b]133;A;k=s\x07");
    assert_eq!(states(&t).len(), 5, "only a primary prompt is a new prompt");
    t.write(b"\x1b]133;A\x07");
    assert_eq!(states(&t), vec![("b".into(), ProgramState::Done), ("c".into(), ProgramState::Error), ("d".into(), ProgramState::Idle)]);
    t.write(all);
    t.end_program();
    assert_eq!(states(&t), vec![("b".into(), ProgramState::Done), ("c".into(), ProgramState::Error), ("d".into(), ProgramState::Idle)]);
    t.write(b"\x1b]7501;state=working\x1b\\prompt\x1b]133;A\x07\x1b]7501;state=working:id=e\x1b\\");
    assert_eq!(states(&t).last(), Some(&("e".into(), ProgramState::Working)), "reports and prompts apply in order");
    assert!(t.program_status().is_none());
}

#[test]
fn a_full_reset_clears_every_record() {
    let mut t = Terminal::new(20, 5, 0).unwrap();
    t.write(b"\x1b]7501;state=done\x1b\\\x1b]7501;state=error:id=x\x1b\\");
    t.write(b"\x1bc");
    assert!(t.program_statuses().is_empty());
}

#[test]
fn the_oldest_record_goes_past_the_limit() {
    let mut t = Terminal::new(20, 5, 0).unwrap();
    for i in 0..MAX_PROGRAM_STATUSES {
        t.write(format!("\x1b]7501;state=working:id=r{i}\x1b\\").as_bytes());
    }
    t.write(b"\x1b]7501;state=done:id=r0\x1b\\\x1b]7501;state=working:id=new\x1b\\");
    let ids: Vec<String> = t.program_statuses().into_iter().map(|r| r.id).collect();
    assert_eq!(ids.len(), MAX_PROGRAM_STATUSES);
    assert!(ids.contains(&"r0".into()) && ids.contains(&"new".into()) && !ids.contains(&"r1".into()));
}

#[test]
fn a_record_without_an_app_takes_its_nearest_ancestor_s() {
    let mut t = Terminal::new(20, 5, 0).unwrap();
    t.write(b"\x1b]7501;state=working:app=deploy\x1b\\\x1b]7501;state=working:id=us\x1b\\\x1b]7501;state=working:id=eu:app=eu-tool\x1b\\\x1b]7501;state=done:id=eu/db\x1b\\");
    let apps: Vec<(String, String)> = t.program_statuses().into_iter().map(|r| (r.id, r.app)).collect();
    assert_eq!(apps, [("", "deploy"), ("eu", "eu-tool"), ("eu/db", "eu-tool"), ("us", "deploy")].map(|(i, a)| (i.to_string(), a.to_string())));
}

