use crate::perf;
use crate::settings::Settings;
use crate::wire::Wire;
use crate::Event;
use kanna_vt::{Key, Lines, Mode, ProgramStatus, Terminal};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

const OUTBOX: usize = 4 << 20;

pub const REPLAY: usize = 512 << 10;

const QUIET: Duration = Duration::from_millis(50);

const DRAIN_LIMIT: Duration = Duration::from_millis(500);

const DRAIN: u8 = 1;

const STOP: u8 = 2;

const RESUME: u8 = 3;

const HOLD: usize = 1 << 20;

const HOLD_LIMIT: Duration = Duration::from_secs(1);

const ECHO_LIMIT: Duration = Duration::from_secs(1);

pub struct Keystroke {
    typed: Instant,
    written: Option<Instant>,
}

pub type Typing = Arc<Mutex<Option<Keystroke>>>;

pub fn typed(typing: &Typing, at: Instant) {
    typing.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(Keystroke { typed: at, written: None });
}

pub fn written(typing: &Typing) {
    if let Some(keystroke) = typing.lock().unwrap_or_else(PoisonError::into_inner).as_mut().filter(|k| k.written.is_none()) {
        keystroke.written = Some(Instant::now());
        perf::since("input.written", keystroke.typed);
    }
}

fn echoed(typing: &Typing) -> Option<Instant> {
    let mut pending = typing.lock().unwrap_or_else(PoisonError::into_inner);
    let typed = pending.as_ref().filter(|k| k.written.is_some())?.typed;
    *pending = None;
    (typed.elapsed() < ECHO_LIMIT).then_some(typed)
}

pub struct Feed {
    screen: Terminal,
    outbox: Vec<u8>,
    overflowed: bool,
    written: u64,
    replay: VecDeque<u8>,
    grids: VecDeque<(u64, u16, u16)>,
    posted: bool,
    pub unsaved: bool,
    pub cols: u16,
    pub rows: u16,
    cwd: Option<String>,
    statuses_serial: u64,
    statuses_owner: Option<i32>,
    echo: Option<Instant>,
    pub shown: bool,
    pub inflight: bool,
}

pub struct Noticed {
    pub overflowed: bool,
    pub cwd: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Replayed {
    Output(Vec<u8>, u64),
    Grid(u16, u16),
}

pub struct Snapshot {
    pub data: Vec<u8>,
    pub offset: u64,
    pub more: bool,
}

pub struct Chunk {
    pub bytes: Vec<u8>,
    pub offset: u64,
    pub typed: Option<Instant>,
}

impl Feed {
    pub fn new(cols: u16, rows: u16, settings: &Settings) -> io::Result<Arc<Mutex<Feed>>> {
        let mut screen = Terminal::new(cols, rows, 0).map_err(io::Error::other)?;
        settings.start(&mut screen);
        Feed::with(screen, true)
    }

    pub fn restore(snapshot: &[u8], settings: &Settings) -> io::Result<Arc<Mutex<Feed>>> {
        let mut screen = Terminal::from_snapshot(snapshot).map_err(io::Error::other)?;
        settings.apply(&mut screen);
        screen.take_replies();
        Feed::with(screen, false)
    }

    fn with(screen: Terminal, unsaved: bool) -> io::Result<Arc<Mutex<Feed>>> {
        let (cols, rows) = screen.size();
        Ok(Arc::new(Mutex::new(Feed { screen, outbox: Vec::new(), overflowed: false, written: 0, replay: VecDeque::new(), grids: VecDeque::new(), posted: false, unsaved, cols, rows, cwd: None, statuses_serial: 0, statuses_owner: None, echo: None, shown: false, inflight: false })))
    }

    pub fn apply(&mut self, settings: &Settings) -> Vec<u8> {
        settings.apply(&mut self.screen);
        self.unsaved = true;
        self.screen.take_replies()
    }


    pub fn encode_key(&self, key: &Key) -> Result<Vec<u8>, kanna_vt::Error> {
        self.screen.encode_key(key)
    }

    pub fn paste(&self, text: &str) -> Vec<u8> {
        if text.is_empty() || !self.screen.mode(Mode::BRACKETED_PASTE) {
            return text.as_bytes().to_vec();
        }
        let text = text.replace("\x1b[200~", "").replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    }

    pub fn enter(&self) -> Vec<u8> {
        "return".parse::<Key>().ok().and_then(|key| self.encode_key(&key).ok()).filter(|bytes| !bytes.is_empty()).unwrap_or_else(|| b"\r".to_vec())
    }

    pub fn lock(feed: &Mutex<Feed>) -> MutexGuard<'_, Feed> {
        feed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn push(&mut self, bytes: &[u8]) -> (bool, Vec<u8>) {
        self.screen.write(bytes);
        self.unsaved = true;
        let replies = self.screen.take_replies();
        if self.screen.take_changed().pwd {
            if let Some(cwd) = cwd_from_url(&self.screen.pwd()) {
                self.cwd = Some(cwd);
            }
        }
        self.written += bytes.len() as u64;
        self.replay.extend(bytes);
        if self.replay.len() > REPLAY {
            let excess = self.replay.len() - REPLAY;
            self.replay.drain(..excess);
            let start = self.written - REPLAY as u64;
            while self.grids.front().is_some_and(|(at, _, _)| *at < start) {
                self.grids.pop_front();
            }
        }
        if self.overflowed || self.outbox.len() + bytes.len() > OUTBOX {
            self.outbox = Vec::new();
            self.overflowed = true;
        } else {
            self.outbox.extend_from_slice(bytes);
        }
        let post = (self.overflowed || self.cwd.is_some()) && !std::mem::replace(&mut self.posted, true);
        (post, replies)
    }

    pub fn noticed(&mut self) -> Noticed {
        let waiting = self.overflowed && self.inflight;
        self.posted = waiting;
        Noticed { overflowed: self.overflowed && !waiting, cwd: self.cwd.take() }
    }

    pub fn holds(&self) -> bool {
        self.shown && self.inflight && self.outbox.len() >= HOLD
    }

    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    pub fn chunk(&mut self, most: usize) -> Option<Chunk> {
        if self.outbox.is_empty() {
            return None;
        }
        let bytes = if self.outbox.len() <= most {
            std::mem::take(&mut self.outbox)
        } else {
            let rest = self.outbox.split_off(most);
            std::mem::replace(&mut self.outbox, rest)
        };
        if let Some(typed) = self.echo.filter(|_| self.outbox.is_empty()) {
            perf::since("echo.taken", typed);
        }
        let offset = self.written - self.outbox.len() as u64;
        Some(Chunk { bytes, offset, typed: if self.outbox.is_empty() { self.echo.take() } else { None } })
    }

    pub fn discard(&mut self) {
        self.outbox = Vec::new();
        self.overflowed = false;
        self.echo = None;
        self.inflight = false;
    }

    pub fn snapshot(&mut self, history: Option<usize>) -> Result<Snapshot, kanna_vt::Error> {
        let (data, more) = match history {
            Some(rows) => self.screen.snapshot_with_history(rows)?,
            None => (self.screen.snapshot()?, false),
        };
        self.outbox = Vec::new();
        self.overflowed = false;
        self.echo = None;
        Ok(Snapshot { data, offset: self.written, more })
    }

    pub fn replay(&mut self, from: u64) -> Option<Vec<Replayed>> {
        let start = self.written - self.replay.len() as u64;
        if from < start || from > self.written {
            return None;
        }
        let mut replayed = Vec::new();
        let mut at = from;
        let grids = self.grids.iter().filter(|(offset, _, _)| *offset >= from).map(|&(offset, cols, rows)| (offset, Some((cols, rows))));
        for (offset, grid) in grids.chain([(self.written, None)]) {
            if offset > at {
                let bytes = self.replay.range((at - start) as usize..(offset - start) as usize).copied().collect();
                replayed.push(Replayed::Output(bytes, offset));
                at = offset;
            }
            if let Some((cols, rows)) = grid {
                replayed.push(Replayed::Grid(cols, rows));
            }
        }
        self.outbox = Vec::new();
        self.overflowed = false;
        self.echo = None;
        Some(replayed)
    }

    pub fn save(&self) -> Result<Vec<u8>, kanna_vt::Error> {
        self.screen.snapshot()
    }

    pub fn resize(&mut self, id: &str, cols: u16, rows: u16, width: u16, height: u16) -> Vec<u8> {
        self.screen.set_cell_pixels(u32::from(width / cols.max(1)), u32::from(height / rows.max(1)));
        if (cols, rows) != (self.cols, self.rows) {
            (self.cols, self.rows) = (cols, rows);
            self.grids.push_back((self.written, cols, rows));
            self.unsaved = true;
                if let Err(error) = self.screen.resize(cols, rows) {
                log!("kanna-daemon: couldn't make {id}'s screen {cols}×{rows}: {error}");
            }
        }
        self.screen.take_replies()
    }

    pub fn claim_statuses(&mut self, group: impl FnOnce() -> Option<i32>) {
        let serial = self.screen.program_status_serial();
        if serial != self.statuses_serial {
            self.statuses_serial = serial;
            self.statuses_owner = group();
        }
    }

    pub fn foreground_changed(&mut self, group: Option<i32>) {
        if self.statuses_owner != group {
            self.screen.end_program();
            self.statuses_serial = self.screen.program_status_serial();
        }
    }

    pub fn restore_statuses(&mut self, owner: Option<i32>, records: Vec<ProgramStatus>) {
        self.screen.restore_program_records(records);
        self.statuses_serial = self.screen.program_status_serial();
        self.statuses_owner = owner;
    }

    pub fn statuses_serial(&self) -> u64 {
        self.screen.program_status_serial()
    }

    pub fn kept_statuses(&self) -> (Option<i32>, Vec<ProgramStatus>) {
        (self.statuses_owner, self.screen.program_records())
    }

    pub fn reported(&self, group: Option<i32>) -> Option<ProgramStatus> {
        self.screen.program_status().filter(|_| group.is_some() && self.statuses_owner == group)
    }

    pub fn turns(&self) -> u64 {
        self.screen.program_turns()
    }

    pub fn program_statuses(&self) -> Vec<ProgramStatus> {
        self.screen.program_statuses()
    }

    pub fn text(&self, lines: Option<usize>) -> String {
        self.screen.text(lines.map_or(Lines::Screen, Lines::Last))
    }
}

pub fn cwd_from_url(url: &str) -> Option<String> {
    let (rest, raw) = if let Some(rest) = url.strip_prefix("file://") {
        (rest, false)
    } else {
        (url.strip_prefix("kitty-shell-cwd://")?, true)
    };
    let slash = rest.find('/')?;
    let (host, path) = rest.split_at(slash);
    if !is_local(host) {
        return None;
    }
    if raw {
        return Some(path.to_string());
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let byte = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok().and_then(|h| u8::from_str_radix(h, 16).ok())?;
                out.push(byte);
                i += 3;
            }
            b'?' | b'#' => break,
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn is_local(host: &str) -> bool {
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let mut name = [0u8; 256];
    if unsafe { libc::gethostname(name.as_mut_ptr() as *mut libc::c_char, name.len()) } != 0 {
        return false;
    }
    let length = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    let local = String::from_utf8_lossy(&name[..length]).to_string();
    let short = |h: &str| h.split('.').next().unwrap_or(h).to_ascii_lowercase();
    host.eq_ignore_ascii_case(&local) || short(host) == short(&local) && (local.ends_with(".local") || host.ends_with(".local"))
}

pub struct Reader {
    wake: File,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Reader {
    pub fn start(id: &str, serial: u64, master: OwnedFd, feed: Arc<Mutex<Feed>>, typing: Typing, wire: Arc<Wire>, replies: Sender<Vec<u8>>, events: Sender<Event>) -> io::Result<Reader> {
        let (asleep, wake) = pipe()?;
        let pane = id.to_string();
        let thread = std::thread::Builder::new()
            .name(format!("kanna.read.{id}"))
            .spawn(move || read(Pty { pane, serial, master: File::from(master), wake: File::from(asleep), feed, typing, wire, replies, events }))?;
        Ok(Reader { wake: File::from(wake), thread: Some(thread) })
    }

    pub fn drain(&self) {
        let _ = (&self.wake).write(&[DRAIN]);
    }

    pub fn stop(&self) {
        let _ = (&self.wake).write(&[STOP]);
    }

    pub fn resumer(&self) -> io::Result<Resumer> {
        self.wake.try_clone().map(Resumer)
    }

    pub fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct Resumer(File);

impl Resumer {
    pub fn resume(&self) {
        let _ = (&self.0).write(&[RESUME]);
    }
}

struct Pty {
    pane: String,
    serial: u64,
    master: File,
    wake: File,
    feed: Arc<Mutex<Feed>>,
    typing: Typing,
    wire: Arc<Wire>,
    replies: Sender<Vec<u8>>,
    events: Sender<Event>,
}

impl Pty {
    fn drained(&self) -> bool {
        self.events.send(Event::Drained { terminal: self.pane.clone(), serial: self.serial }).is_ok()
    }
}

fn read(mut pty: Pty) {
    let mut buffer = vec![0u8; 65536];
    let mut draining: Option<Instant> = None;
    let mut held: Option<Instant> = None;
    let mut released = false;
    loop {
        let mut fds = [
            libc::pollfd { fd: pty.master.as_raw_fd(), events: if held.is_some() { 0 } else { libc::POLLIN }, revents: 0 },
            libc::pollfd { fd: pty.wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        let timeout = match (held, draining) {
            (Some(until), _) => until.saturating_duration_since(Instant::now()).as_millis() as libc::c_int,
            (None, Some(_)) => QUIET.as_millis() as libc::c_int,
            (None, None) => -1,
        };
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if fds[1].revents != 0 {
            let mut asked = [0u8; 64];
            match pty.wake.read(&mut asked) {
                Ok(n @ 1..) if asked[..n].contains(&STOP) => return,
                Ok(n @ 1..) => {
                    if asked[..n].contains(&RESUME) {
                        held = None;
                        released = false;
                    }
                    if asked[..n].contains(&DRAIN) {
                        held = None;
                        draining.get_or_insert_with(|| Instant::now() + DRAIN_LIMIT);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                _ => return,
            }
            continue;
        }
        if held.is_some_and(|until| Instant::now() >= until) {
            held = None;
            released = true;
            continue;
        }
        if held.is_some() {
            continue;
        }
        if draining.is_some_and(|limit| ready == 0 || Instant::now() >= limit) {
            draining = None;
            if !pty.drained() {
                return;
            }
            continue;
        }
        if fds[0].revents == 0 {
            continue;
        }
        let n = match pty.master.read(&mut buffer) {
            Ok(n @ 1..) => n,
            Err(error) if matches!(error.kind(), io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock) => continue,
            _ => {
                pty.drained();
                return;
            }
        };
        let echo = echoed(&pty.typing);
        if let Some(typed) = echo {
            perf::since("echo.read", typed);
        }
        let (post, replies) = {
            let mut feed = Feed::lock(&pty.feed);
            let pushed = feed.push(&buffer[..n]);
            feed.claim_statuses(|| kanna_detect::program::foreground_group(pty.master.as_raw_fd()));
            if echo.is_some() {
                feed.echo = feed.echo.or(echo);
            }
            pty.wire.deliver(&pty.pane, &mut feed);
            if !released && draining.is_none() && feed.holds() {
                held = Some(Instant::now() + HOLD_LIMIT);
            }
            pushed
        };
        if let Some(typed) = echo {
            perf::since("echo.parsed", typed);
        }
        if !replies.is_empty() {
            let _ = pty.replies.send(replies);
        }
        if post && pty.events.send(Event::Output { terminal: pty.pane.clone(), serial: pty.serial }).is_err() {
            return;
        }
    }
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&read, &write] {
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    unsafe { libc::fcntl(write.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
    Ok((read, write))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn overlong_osc() -> Vec<u8> {
        let mut osc = b"\x1b]2;".to_vec();
        osc.resize(osc.len() + kanna_vt::MAX_UNFINISHED_SEQUENCE_BYTES + 1, b'x');
        osc
    }

    fn feed(cols: u16, rows: u16, output: &str) -> Arc<Mutex<Feed>> {
        let feed = Feed::new(cols, rows, &Settings::default()).unwrap();
        Feed::lock(&feed).push(output.as_bytes());
        feed
    }

    #[test]
    fn read_trims_blanks_and_counts_scrollback() {
        let f = feed(20, 5, "$ echo hi\r\nhi\r\n\x1b[1;32m$\x1b[0m ");
        assert_eq!(Feed::lock(&f).text(None), "$ echo hi\nhi\n$");
        let f = feed(10, 3, "1\r\n2\r\n3\r\n4\r\n5");
        let f = Feed::lock(&f);
        assert_eq!(f.text(None), "3\n4\n5");
        assert_eq!(f.text(Some(4)), "2\n3\n4\n5");
        assert_eq!(f.text(Some(100)), "1\n2\n3\n4\n5");
    }

    #[test]
    fn a_shell_reporting_its_folder_changes_the_cwd() {
        let f = feed(20, 5, "\x1b]7;file://localhost/tmp/a%20b\x07");
        assert_eq!(Feed::lock(&f).noticed().cwd.as_deref(), Some("/tmp/a b"));
        assert_eq!(Feed::lock(&f).noticed().cwd, None);
        let (posted, _) = Feed::lock(&f).push(b"\x1b]7;file://elsewhere.example/etc\x07\x1b]7;kitty-shell-cwd:///x%y\x07");
        assert!(posted, "a new folder is the main loop's to publish");
        assert_eq!(Feed::lock(&f).noticed().cwd.as_deref(), Some("/x%y"));
        assert_eq!(cwd_from_url("file:///a%2"), None);
        assert_eq!(cwd_from_url("http://localhost/a"), None);
        assert_eq!(cwd_from_url("file://"), None);
    }

    #[test]
    fn queries_are_answered() {
        let f = Feed::new(20, 5, &Settings::default()).unwrap();
        let (posted, replies) = Feed::lock(&f).push(b"\x1b[2;3H\x1b[6n");
        assert!(!posted, "plain output goes to the app from the reader, not through the main loop");
        assert_eq!(replies, b"\x1b[2;3R");
    }

    #[test]
    fn a_snapshot_replaces_the_outbox_unless_it_fails() {
        let f = feed(20, 5, "before");
        let mut f = Feed::lock(&f);
        f.push(&overlong_osc());
        assert!(f.snapshot(None).is_err());
        assert!(f.chunk(16).is_some_and(|chunk| chunk.bytes.len() == 16 && chunk.offset == 16), "the outbox is kept, and taken a chunk at a time");
        f.push(b"\x07after");
        let snapshot = f.snapshot(None).unwrap();
        assert!(f.chunk(usize::MAX).is_none());
        assert_eq!(snapshot.offset, (6 + overlong_osc().len() + 6) as u64);
        assert_eq!(Terminal::from_snapshot(&snapshot.data).unwrap().text(Lines::Screen), "beforeafter");
    }

    #[test]
    fn an_outbox_past_its_limit_overflows_and_a_snapshot_starts_it_over() {
        let f = Feed::new(80, 24, &Settings::default()).unwrap();
        let mut f = Feed::lock(&f);
        let line = vec![b'x'; 1 << 20];
        let posts: Vec<bool> = (0..5).map(|_| f.push(&line).0).collect();
        assert_eq!(posts, [false, false, false, false, true], "the main loop hears once, when it overflows");
        assert!(f.overflowed());
        assert!(f.noticed().overflowed);
        f.snapshot(None).unwrap();
        assert!(!f.overflowed() && f.chunk(usize::MAX).is_none());
    }

    #[test]
    fn a_full_scrollback_stays_within_its_byte_limit() {
        let f = Feed::new(1000, 50, &Settings::default()).unwrap();
        let mut f = Feed::lock(&f);
        let line: String = (0..998u32).map(|c| char::from_u32(0x4e00 + c % 2000).unwrap()).collect();
        for i in 0..3000 {
            f.push(format!("\x1b[38;5;{}m\x1b[48;2;{i};1;2m{line}\x1b[0m\r\n", i % 256).as_bytes());
        }
        let snapshot = f.snapshot(None).unwrap();
        assert!(snapshot.data.len() < crate::settings::MAX_SCROLLBACK_BYTES, "{} bytes", snapshot.data.len());
        assert!(!snapshot.more);
        let bounded = f.snapshot(Some(100)).unwrap();
        assert!(bounded.data.len() * 4 < snapshot.data.len(), "{} of {} bytes", bounded.data.len(), snapshot.data.len());
        assert!(bounded.more);
    }

    #[test]
    fn a_bounded_snapshot_in_the_alternate_screen_still_says_the_shell_has_more() {
        let f = feed(80, 24, "");
        let mut f = Feed::lock(&f);
        for i in 0..5000 {
            f.push(format!("line {i}\r\n").as_bytes());
        }
        f.push(b"\x1b[?1049h full screen");
        let bounded = f.snapshot(Some(100)).unwrap();
        assert!(bounded.more);
    }

    #[test]
    fn a_replay_gives_the_output_and_sizes_since_an_offset_in_order() {
        let f = feed(20, 5, "one ");
        let mut f = Feed::lock(&f);
        f.push(b"two ");
        f.resize("t", 30, 6, 0, 0);
        f.push(b"three");
        assert_eq!(f.replay(4), Some(vec![Replayed::Output(b"two ".to_vec(), 8), Replayed::Grid(30, 6), Replayed::Output(b"three".to_vec(), 13)]));
        assert_eq!(f.replay(8), Some(vec![Replayed::Grid(30, 6), Replayed::Output(b"three".to_vec(), 13)]));
        assert_eq!(f.replay(13), Some(vec![]));
        assert_eq!(f.replay(14), None);
        assert!(f.chunk(usize::MAX).is_none(), "the replay sent what the outbox held");
    }

    #[test]
    fn a_replay_reaches_back_only_as_far_as_the_ring() {
        let f = feed(20, 5, "");
        let mut f = Feed::lock(&f);
        f.resize("t", 30, 6, 0, 0);
        let chunk = vec![b'x'; 1000];
        for _ in 0..(REPLAY / 1000 + 10) {
            f.push(&chunk);
        }
        let written = ((REPLAY / 1000 + 10) * 1000) as u64;
        assert_eq!(f.replay(0), None);
        assert_eq!(f.replay(written - REPLAY as u64 - 1), None);
        let replayed = f.replay(written - REPLAY as u64).unwrap();
        assert_eq!(replayed, vec![Replayed::Output(vec![b'x'; REPLAY], written)]);
    }

    fn reported(f: &Feed, group: i32) -> Option<&'static str> {
        f.reported(Some(group)).map(|r| r.state.as_str())
    }

    #[test]
    fn the_program_status_probe_is_answered() {
        let f = Feed::new(20, 5, &Settings::default()).unwrap();
        let (_, replies) = Feed::lock(&f).push(b"\x1b]7501;?\x1b\\");
        assert!(replies.starts_with(b"\x1b]7501;?:states=") && replies.ends_with(b"\x1b\\"), "{replies:?}");
    }

    #[test]
    fn a_record_belongs_to_the_program_in_the_foreground_when_it_reported() {
        let f = Feed::new(20, 5, &Settings::default()).unwrap();
        let mut f = Feed::lock(&f);
        f.push(b"\x1b]7501;state=working\x1b\\");
        assert_eq!(f.reported(Some(7)), None, "unclaimed");
        f.claim_statuses(|| Some(7));
        assert_eq!(reported(&f, 7), Some("working"));
        assert_eq!(reported(&f, 8), None);
        assert_eq!(f.reported(None), None);
        f.claim_statuses(|| panic!("nothing new was reported"));
        f.foreground_changed(Some(7));
        assert_eq!(reported(&f, 7), Some("working"), "the program reported before Kanna saw it take the foreground");
    }

    #[test]
    fn working_and_blocked_go_when_the_program_ends_or_a_prompt_starts_and_a_reset_clears_all() {
        let f = Feed::new(20, 5, &Settings::default()).unwrap();
        let mut f = Feed::lock(&f);
        f.push(b"\x1b]7501;state=blocked:kind=question\x1b\\");
        f.claim_statuses(|| Some(7));
        f.foreground_changed(Some(8));
        assert_eq!(f.program_statuses(), vec![], "blocked ends with its program");
        f.push(b"\x1b]7501;state=done\x1b\\");
        f.claim_statuses(|| Some(7));
        f.foreground_changed(Some(8));
        assert_eq!(f.program_statuses().len(), 1, "done stays");
        assert_eq!(reported(&f, 8), None, "but it's another program's");
        f.push(b"\x1b]7501;state=working\x1b\\");
        f.claim_statuses(|| Some(8));
        assert_eq!(reported(&f, 8), Some("working"));
        f.push(b"\x1b]133;A\x07$ ");
        f.claim_statuses(|| Some(8));
        assert_eq!(reported(&f, 8), None, "a new prompt");
        f.push(b"\x1b]7501;state=idle\x1b\\\x1b]7501;state=error:id=x\x1b\\\x1bc");
        f.claim_statuses(|| Some(8));
        assert!(f.program_statuses().is_empty(), "a full reset");
    }
}
