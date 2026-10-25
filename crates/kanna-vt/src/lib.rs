mod ffi;
mod key;

pub use key::{Key, KeyError};

use std::ffi::c_void;
use std::fmt;
use std::ptr::{self, NonNull};

pub const GHOSTTY_COMMIT: &str = env!("KANNA_GHOSTTY_VT_COMMIT");
pub const MAX_UNFINISHED_SEQUENCE_BYTES: usize = 1 << 20;

const MAX_PENDING_REPLY_BYTES: usize = 1 << 20;

const DA1_FEATURES: [u16; 2] = [22, 52];

pub const MAX_PROGRAM_STATUSES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error {
    pub op: &'static str,
    pub code: i32,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.code {
            ffi::GHOSTTY_OUT_OF_MEMORY => "out of memory",
            ffi::GHOSTTY_INVALID_VALUE => "invalid value",
            ffi::GHOSTTY_OUT_OF_SPACE => "out of space",
            ffi::GHOSTTY_NO_VALUE => "no value",
            -5 => "I/O error",
            -6 => "limit exceeded",
            -7 => "rejected",
            _ => "error",
        };
        write!(f, "{} failed: {what} ({})", self.op, self.code)
    }
}

impl std::error::Error for Error {}

fn check(op: &'static str, code: ffi::GhosttyResult) -> Result<(), Error> {
    if code == ffi::GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(Error { op, code })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lines {
    Screen,
    Last(usize),
}

pub type Rgb = [u8; 3];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Option<Rgb>,
    pub palette: Box<[Rgb; 256]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode(pub u16);

impl Mode {
    pub const CURSOR_BLINKING: Mode = Mode(12);
    pub const BRACKETED_PASTE: Mode = Mode(2004);
    pub const SYNC_OUTPUT: Mode = Mode(2026);
    pub const GRAPHEME_CLUSTER: Mode = Mode(2027);
    pub const COLOR_SCHEME_REPORT: Mode = Mode(2031);
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    #[default]
    None,
    Set(u8),
    Error(Option<u8>),
    Indeterminate,
    Paused(Option<u8>),
}

impl Progress {
    pub fn as_str(self) -> &'static str {
        match self {
            Progress::None => "",
            Progress::Set(_) => "set",
            Progress::Error(_) => "error",
            Progress::Indeterminate => "indeterminate",
            Progress::Paused(_) => "pause",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl ProgramState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProgramState::Idle => "idle",
            ProgramState::Working => "working",
            ProgramState::Done => "done",
            ProgramState::Blocked => "blocked",
            ProgramState::Error => "error",
        }
    }

    fn ends_with_program(self) -> bool {
        matches!(self, ProgramState::Working | ProgramState::Blocked)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    Permission,
    Question,
    Auth,
}

impl Need {
    pub fn as_str(self) -> &'static str {
        match self {
            Need::Permission => "permission",
            Need::Question => "question",
            Need::Auth => "auth",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramStatus {
    pub id: String,
    pub state: ProgramState,
    pub need: Option<Need>,
    pub progress: Option<u8>,
    pub app: String,
    pub title: String,
    pub message: String,
}

#[derive(Default)]
struct ProgramStatuses {
    records: Vec<(u64, ProgramStatus)>,
    serial: u64,
}

impl ProgramStatuses {
    fn bump(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    fn put(&mut self, status: ProgramStatus) {
        let serial = self.bump();
        if let Some(record) = self.records.iter_mut().find(|(_, r)| r.id == status.id) {
            *record = (serial, status);
            return;
        }
        if self.records.len() >= MAX_PROGRAM_STATUSES {
            if let Some(oldest) = self.records.iter().enumerate().min_by_key(|(_, (s, _))| *s).map(|(i, _)| i) {
                self.records.swap_remove(oldest);
            }
        }
        self.records.push((serial, status));
    }

    fn clear(&mut self, id: &str) {
        let beneath = |other: &str| id.is_empty() || other == id || other.strip_prefix(id).is_some_and(|rest| rest.starts_with('/'));
        let before = self.records.len();
        self.records.retain(|(_, r)| !beneath(&r.id));
        if self.records.len() != before {
            self.bump();
        }
    }

    fn end_program(&mut self) {
        let before = self.records.len();
        self.records.retain(|(_, r)| !r.state.ends_with_program());
        if self.records.len() != before {
            self.bump();
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Changed {
    pub title: bool,
    pub pwd: bool,
}

pub struct Terminal {
    raw: NonNull<ffi::GhosttyTerminalImpl>,
    hooks: NonNull<Hooks>,
}

#[derive(Default)]
struct Hooks {
    replies: Vec<u8>,
    size: ffi::GhosttySizeReportSize,
    scheme: Option<Scheme>,
    changed: Changed,
    progress: Progress,
    statuses: ProgramStatuses,
}

unsafe impl Send for Terminal {}

impl Terminal {
    pub fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Result<Terminal, Error> {
        let mut raw: ffi::GhosttyTerminal = ptr::null_mut();
        check("ghostty_terminal_new", unsafe {
            ffi::ghostty_terminal_new(ptr::null(), &mut raw, cols.max(1), rows.max(1))
        })?;
        let mut terminal = Terminal::adopt(raw)?;
        terminal.set_scrollback_bytes(None)?;
        terminal.set_scrollback_lines(scrollback_lines)?;
        let limit = MAX_UNFINISHED_SEQUENCE_BYTES;
        terminal.set("continuation limit", ffi::GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES, &limit)?;
        Ok(terminal)
    }

    pub fn from_snapshot(bytes: &[u8]) -> Result<Terminal, Error> {
        Terminal::decode(bytes, None)
    }

    fn decode(bytes: &[u8], history_rows: Option<usize>) -> Result<Terminal, Error> {
        let mut decoder: ffi::GhosttySnapshotDecoder = ptr::null_mut();
        check("ghostty_snapshot_decoder_new_buf", unsafe {
            ffi::ghostty_snapshot_decoder_new_buf(ptr::null(), &mut decoder, bytes.as_ptr(), bytes.len())
        })?;
        let mut raw: ffi::GhosttyTerminal = ptr::null_mut();
        let limit = MAX_UNFINISHED_SEQUENCE_BYTES;
        let retain = true;
        let result = unsafe {
            check(
                "snapshot continuation limit",
                ffi::ghostty_snapshot_decoder_set(
                    decoder,
                    ffi::GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
                    &limit as *const usize as *const c_void,
                ),
            )
            .and_then(|()| {
                check(
                    "snapshot retain continuation",
                    ffi::ghostty_snapshot_decoder_set(
                        decoder,
                        ffi::GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
                        &retain as *const bool as *const c_void,
                    ),
                )
            })
            .and_then(|()| match history_rows {
                None => check("ghostty_snapshot_decoder_decode", ffi::ghostty_snapshot_decoder_decode(decoder, &mut raw)),
                Some(wanted) => check("ghostty_snapshot_decoder_ready", ffi::ghostty_snapshot_decoder_ready(decoder, &mut raw))
                    .and_then(|()| history(decoder, wanted)),
            })
        };
        unsafe { ffi::ghostty_snapshot_decoder_free(decoder) };
        match result {
            Ok(()) => Terminal::adopt(raw),
            Err(error) => {
                if !raw.is_null() {
                    unsafe { ffi::ghostty_terminal_free(raw) };
                }
                Err(error)
            }
        }
    }

    fn adopt(raw: ffi::GhosttyTerminal) -> Result<Terminal, Error> {
        let raw = NonNull::new(raw).ok_or(Error { op: "ghostty_terminal_new", code: ffi::GHOSTTY_INVALID_VALUE })?;
        let hooks = NonNull::from(Box::leak(Box::new(Hooks::default())));
        let mut terminal = Terminal { raw, hooks };
        let (columns, rows) = terminal.size();
        terminal.hooks().size = ffi::GhosttySizeReportSize { rows, columns, cell_width: 0, cell_height: 0 };
        let userdata: *mut Hooks = hooks.as_ptr();
        let write: ffi::GhosttyTerminalWritePtyFn = write_pty;
        let attributes: ffi::GhosttyTerminalDeviceAttributesFn = device_attributes;
        let size: ffi::GhosttyTerminalSizeFn = size_report;
        let scheme: ffi::GhosttyTerminalColorSchemeFn = color_scheme;
        let title: ffi::GhosttyTerminalChangedFn = title_changed;
        let pwd: ffi::GhosttyTerminalChangedFn = pwd_changed;
        let progress: ffi::GhosttyTerminalProgressReportFn = progress_report;
        let prompt: ffi::GhosttyTerminalSemanticPromptFn = semantic_prompt;
        let status: ffi::GhosttyTerminalProgramStatusFn = program_status;
        unsafe {
            check(
                "terminal userdata",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_USERDATA, userdata as *const c_void),
            )?;
            check(
                "terminal write_pty",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_WRITE_PTY, write as *const c_void),
            )?;
            check(
                "terminal device_attributes",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES, attributes as *const c_void),
            )?;
            check("terminal size", ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_SIZE, size as *const c_void))?;
            check(
                "terminal color_scheme",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_COLOR_SCHEME, scheme as *const c_void),
            )?;
            check(
                "terminal title_changed",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, title as *const c_void),
            )?;
            check(
                "terminal pwd_changed",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_PWD_CHANGED, pwd as *const c_void),
            )?;
            check(
                "terminal progress_report",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT, progress as *const c_void),
            )?;
            check(
                "terminal semantic_prompt",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT, prompt as *const c_void),
            )?;
            check(
                "terminal program_status",
                ffi::ghostty_terminal_set(raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS, status as *const c_void),
            )?;
        }
        Ok(terminal)
    }

    fn hooks(&mut self) -> &mut Hooks {
        unsafe { &mut *self.hooks.as_ptr() }
    }

    fn set<T>(&mut self, op: &'static str, option: i32, value: &T) -> Result<(), Error> {
        check(op, unsafe { ffi::ghostty_terminal_set(self.raw.as_ptr(), option, value as *const T as *const c_void) })
    }

    fn get<T: Default>(&self, data: i32) -> Option<T> {
        let mut out = T::default();
        let code = unsafe { ffi::ghostty_terminal_get(self.raw.as_ptr(), data, &mut out as *mut T as *mut c_void) };
        (code == ffi::GHOSTTY_SUCCESS).then_some(out)
    }

    fn get_string(&self, data: i32) -> String {
        let mut out = ffi::GhosttyString { ptr: ptr::null(), len: 0 };
        let code = unsafe { ffi::ghostty_terminal_get(self.raw.as_ptr(), data, &mut out as *mut _ as *mut c_void) };
        if code != ffi::GHOSTTY_SUCCESS || out.ptr.is_null() || out.len == 0 {
            return String::new();
        }
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(out.ptr, out.len) }).into_owned()
    }

    pub fn write(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            unsafe { ffi::ghostty_terminal_vt_write(self.raw.as_ptr(), bytes.as_ptr(), bytes.len()) };
        }
    }

    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.hooks().replies)
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), Error> {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let size = self.hooks().size;
        check("ghostty_terminal_resize", unsafe {
            ffi::ghostty_terminal_resize(self.raw.as_ptr(), cols, rows, size.cell_width, size.cell_height)
        })?;
        (self.hooks().size.columns, self.hooks().size.rows) = (cols, rows);
        Ok(())
    }

    pub fn set_cell_pixels(&mut self, width: u32, height: u32) {
        (self.hooks().size.cell_width, self.hooks().size.cell_height) = (width, height);
    }

    pub fn set_scrollback_lines(&mut self, lines: usize) -> Result<(), Error> {
        self.set("scrollback limit", ffi::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &lines)
    }

    pub fn set_scrollback_line_limit(&mut self, lines: Option<usize>) -> Result<(), Error> {
        let value = lines.as_ref().map_or(ptr::null(), |l| l as *const usize as *const c_void);
        check("scrollback limit", unsafe {
            ffi::ghostty_terminal_set(self.raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, value)
        })
    }

    pub fn set_default_colors(&mut self, colors: &Colors) -> Result<(), Error> {
        let rgb = |c: Rgb| ffi::GhosttyColorRgb { r: c[0], g: c[1], b: c[2] };
        self.set("foreground", ffi::GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &rgb(colors.foreground))?;
        self.set("background", ffi::GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &rgb(colors.background))?;
        let cursor = colors.cursor.map(rgb);
        let value = cursor.as_ref().map_or(ptr::null(), |c| c as *const ffi::GhosttyColorRgb as *const c_void);
        check("cursor color", unsafe { ffi::ghostty_terminal_set(self.raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, value) })?;
        let palette: Vec<ffi::GhosttyColorRgb> = colors.palette.iter().copied().map(rgb).collect();
        check("palette", unsafe {
            ffi::ghostty_terminal_set(self.raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, palette.as_ptr() as *const c_void)
        })
    }

    pub fn foreground(&self) -> Option<Rgb> {
        self.get::<ffi::GhosttyColorRgb>(ffi::GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND).map(|c| [c.r, c.g, c.b])
    }

    pub fn background(&self) -> Option<Rgb> {
        self.get::<ffi::GhosttyColorRgb>(ffi::GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND).map(|c| [c.r, c.g, c.b])
    }

    pub fn set_color_scheme(&mut self, scheme: Option<Scheme>) {
        let old = std::mem::replace(&mut self.hooks().scheme, scheme);
        if let (Some(scheme), true) = (scheme, old != scheme) {
            if self.mode(Mode::COLOR_SCHEME_REPORT) {
                let report: &[u8] = match scheme {
                    Scheme::Dark => b"\x1b[?997;1n",
                    Scheme::Light => b"\x1b[?997;2n",
                };
                self.hooks().replies.extend_from_slice(report);
            }
        }
    }

    pub fn set_default_cursor(&mut self, style: CursorStyle, blink: bool) -> Result<(), Error> {
        let style = match style {
            CursorStyle::Bar => ffi::GHOSTTY_TERMINAL_CURSOR_STYLE_BAR,
            CursorStyle::Block => ffi::GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK,
            CursorStyle::Underline => ffi::GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE,
            CursorStyle::BlockHollow => ffi::GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW,
        };
        self.set("cursor style", ffi::GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE, &style)?;
        self.set("cursor blink", ffi::GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK, &blink)
    }

    pub fn set_mode_default(&mut self, mode: Mode, value: bool) -> Result<(), Error> {
        self.set("mode default", ffi::GHOSTTY_TERMINAL_OPT_MODE_DEFAULT, &ffi::GhosttyTerminalModeConfig { mode: mode.0, value })
    }

    pub fn mode(&self, mode: Mode) -> bool {
        let mut config = ffi::GhosttyTerminalModeConfig { mode: mode.0, value: false };
        let code = unsafe {
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::GHOSTTY_TERMINAL_DATA_MODE, &mut config as *mut _ as *mut c_void)
        };
        code == ffi::GHOSTTY_SUCCESS && config.value
    }

    pub fn progress(&self) -> Progress {
        unsafe { (*self.hooks.as_ptr()).progress }
    }

    pub fn program_statuses(&self) -> Vec<ProgramStatus> {
        let mut records: Vec<ProgramStatus> = unsafe { &(*self.hooks.as_ptr()).statuses }.records.iter().map(|(_, r)| r.clone()).collect();
        records.sort_by(|a, b| a.id.cmp(&b.id));
        let apps: Vec<(String, String)> = records.iter().filter(|r| !r.app.is_empty()).map(|r| (r.id.clone(), r.app.clone())).collect();
        for record in records.iter_mut().filter(|r| r.app.is_empty()) {
            let mut id = record.id.as_str();
            while !id.is_empty() {
                id = id.rfind('/').map_or("", |slash| &id[..slash]);
                if let Some((_, app)) = apps.iter().find(|(i, _)| i == id) {
                    record.app = app.clone();
                    break;
                }
            }
        }
        records
    }

    pub fn program_status(&self) -> Option<ProgramStatus> {
        unsafe { &(*self.hooks.as_ptr()).statuses }.records.iter().find(|(_, r)| r.id.is_empty()).map(|(_, r)| r.clone())
    }

    pub fn program_status_serial(&self) -> u64 {
        unsafe { (*self.hooks.as_ptr()).statuses.serial }
    }

    pub fn end_program(&mut self) {
        self.hooks().statuses.end_program();
    }

    pub fn take_changed(&mut self) -> Changed {
        std::mem::take(&mut self.hooks().changed)
    }

    pub fn set_scrollback_bytes(&mut self, bytes: Option<usize>) -> Result<(), Error> {
        let value = bytes.as_ref().map_or(ptr::null(), |b| b as *const usize as *const c_void);
        check("scrollback byte limit", unsafe {
            ffi::ghostty_terminal_set(self.raw.as_ptr(), ffi::GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, value)
        })
    }

    pub fn scrollback_bytes(&self) -> Option<usize> {
        self.get(ffi::GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_BYTES)
    }

    pub fn scrollback_lines(&self) -> Option<usize> {
        self.get(ffi::GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_LINES)
    }

    pub fn size(&self) -> (u16, u16) {
        (self.get(ffi::GHOSTTY_TERMINAL_DATA_COLS).unwrap_or(0), self.get(ffi::GHOSTTY_TERMINAL_DATA_ROWS).unwrap_or(0))
    }

    pub fn cursor(&self) -> (u16, u16) {
        (
            self.get(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_X).unwrap_or(0),
            self.get(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_Y).unwrap_or(0),
        )
    }

    pub fn cursor_visible(&self) -> bool {
        self.get(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE).unwrap_or(true)
    }

    pub fn alternate_screen(&self) -> bool {
        self.get::<i32>(ffi::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN) == Some(ffi::GHOSTTY_TERMINAL_SCREEN_ALTERNATE)
    }

    pub fn title(&self) -> String {
        self.get_string(ffi::GHOSTTY_TERMINAL_DATA_TITLE)
    }

    pub fn pwd(&self) -> String {
        self.get_string(ffi::GHOSTTY_TERMINAL_DATA_PWD)
    }

    pub fn scrollback_rows(&self) -> usize {
        self.get(ffi::GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS).unwrap_or(0)
    }

    pub fn text(&self, lines: Lines) -> String {
        let total: usize = self.get(ffi::GHOSTTY_TERMINAL_DATA_TOTAL_ROWS).unwrap_or(0);
        let (cols, rows) = self.size();
        if total == 0 || cols == 0 || rows == 0 {
            return String::new();
        }
        let start = match lines {
            Lines::Screen => total.saturating_sub(rows as usize),
            Lines::Last(n) => total.saturating_sub(n.saturating_add(rows as usize)),
        };
        let mut out: Vec<String> = match self.format_rows(start, total - 1, cols - 1) {
            Some(text) => text.split('\n').map(|line| line.trim_end().to_string()).collect(),
            None => return String::new(),
        };
        while out.last().is_some_and(String::is_empty) {
            out.pop();
        }
        if let Lines::Last(n) = lines {
            out.drain(..out.len().saturating_sub(n));
        }
        out.join("\n")
    }

    fn format_rows(&self, first: usize, last: usize, last_col: u16) -> Option<String> {
        let point = |x: u16, y: usize| ffi::GhosttyPoint {
            tag: ffi::GHOSTTY_POINT_TAG_SCREEN,
            value: ffi::GhosttyPointValue { coordinate: ffi::GhosttyPointCoordinate { x, y: y.min(u32::MAX as usize) as u32 } },
        };
        let blank = ffi::GhosttyGridRef { size: std::mem::size_of::<ffi::GhosttyGridRef>(), node: ptr::null_mut(), x: 0, y: 0 };
        let (mut start, mut end) = (blank, blank);
        unsafe {
            if ffi::ghostty_terminal_grid_ref(self.raw.as_ptr(), point(0, first), &mut start) != ffi::GHOSTTY_SUCCESS
                || ffi::ghostty_terminal_grid_ref(self.raw.as_ptr(), point(last_col, last), &mut end) != ffi::GHOSTTY_SUCCESS
            {
                return None;
            }
        }
        let selection =
            ffi::GhosttySelection { size: std::mem::size_of::<ffi::GhosttySelection>(), start, end, rectangle: false };
        let options = ffi::GhosttyTerminalSelectionFormatOptions {
            size: std::mem::size_of::<ffi::GhosttyTerminalSelectionFormatOptions>(),
            emit: ffi::GHOSTTY_FORMATTER_FORMAT_PLAIN,
            unwrap: false,
            trim: true,
            selection: &selection,
        };
        let mut out: *mut u8 = ptr::null_mut();
        let mut len = 0usize;
        let code = unsafe {
            ffi::ghostty_terminal_selection_format_alloc(self.raw.as_ptr(), ptr::null(), options, &mut out, &mut len)
        };
        if code != ffi::GHOSTTY_SUCCESS {
            return None;
        }
        Some(String::from_utf8_lossy(&unsafe { take_buffer(out, len) }).into_owned())
    }

    pub fn snapshot(&self) -> Result<Vec<u8>, Error> {
        let mut out: *mut u8 = ptr::null_mut();
        let mut len = 0usize;
        check("ghostty_snapshot_encode_alloc", unsafe {
            ffi::ghostty_snapshot_encode_alloc(self.raw.as_ptr(), ptr::null(), &mut out, &mut len)
        })?;
        Ok(unsafe { take_buffer(out, len) })
    }

    pub fn snapshot_with_history(&self, rows: usize) -> Result<Vec<u8>, Error> {
        Terminal::decode(&self.snapshot()?, Some(rows))?.snapshot()
    }
}

unsafe fn history(decoder: ffi::GhosttySnapshotDecoder, wanted: usize) -> Result<(), Error> {
    let mut kept = 0usize;
    while kept < wanted {
        match ffi::ghostty_snapshot_decoder_next(decoder) {
            ffi::GHOSTTY_NO_VALUE => break,
            code => check("ghostty_snapshot_decoder_next", code)?,
        }
        let mut rows = 0usize;
        check("snapshot history rows", ffi::ghostty_snapshot_decoder_get(decoder, ffi::GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_ROWS, &mut rows as *mut usize as *mut c_void))?;
        kept += rows;
    }
    Ok(())
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe {
            ffi::ghostty_terminal_free(self.raw.as_ptr());
            drop(Box::from_raw(self.hooks.as_ptr()));
        }
    }
}

unsafe fn take_buffer(ptr: *mut u8, len: usize) -> Vec<u8> {
    if ptr.is_null() {
        return Vec::new();
    }
    let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
    ffi::ghostty_free(ptr::null(), ptr, len);
    bytes
}

unsafe extern "C" fn write_pty(_terminal: ffi::GhosttyTerminal, userdata: *mut c_void, data: *const u8, len: usize) {
    if userdata.is_null() || data.is_null() || len == 0 {
        return;
    }
    let replies = &mut (*(userdata as *mut Hooks)).replies;
    if replies.len().saturating_add(len) <= MAX_PENDING_REPLY_BYTES {
        replies.extend_from_slice(std::slice::from_raw_parts(data, len));
    }
}

unsafe extern "C" fn color_scheme(_terminal: ffi::GhosttyTerminal, userdata: *mut c_void, out: *mut i32) -> bool {
    if userdata.is_null() || out.is_null() {
        return false;
    }
    match (*(userdata as *mut Hooks)).scheme {
        Some(Scheme::Light) => *out = ffi::GHOSTTY_COLOR_SCHEME_LIGHT,
        Some(Scheme::Dark) => *out = ffi::GHOSTTY_COLOR_SCHEME_DARK,
        None => return false,
    }
    true
}

unsafe extern "C" fn title_changed(_terminal: ffi::GhosttyTerminal, userdata: *mut c_void) {
    if !userdata.is_null() {
        (*(userdata as *mut Hooks)).changed.title = true;
    }
}

unsafe extern "C" fn pwd_changed(_terminal: ffi::GhosttyTerminal, userdata: *mut c_void) {
    if !userdata.is_null() {
        (*(userdata as *mut Hooks)).changed.pwd = true;
    }
}

unsafe extern "C" fn progress_report(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    report: *const ffi::GhosttyTerminalProgressReport,
) {
    if userdata.is_null() || report.is_null() {
        return;
    }
    let report = &*report;
    let percent = (report.progress >= 0).then(|| report.progress.min(100) as u8);
    (*(userdata as *mut Hooks)).progress = match report.state {
        ffi::GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE => Progress::None,
        ffi::GHOSTTY_TERMINAL_PROGRESS_STATE_SET => Progress::Set(percent.unwrap_or(0)),
        ffi::GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR => Progress::Error(percent),
        ffi::GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE => Progress::Indeterminate,
        ffi::GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE => Progress::Paused(percent),
        _ => return,
    };
}

unsafe extern "C" fn semantic_prompt(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    event: *const ffi::GhosttyTerminalSemanticPrompt,
) {
    if userdata.is_null() || event.is_null() {
        return;
    }
    let event = &*event;
    if event.kind == ffi::GHOSTTY_SEMANTIC_PROMPT_PROMPT_START && event.prompt_kind == ffi::GHOSTTY_SEMANTIC_PROMPT_PROMPT_PRIMARY {
        (*(userdata as *mut Hooks)).statuses.end_program();
    }
}

unsafe extern "C" fn program_status(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    report: *const ffi::GhosttyTerminalProgramStatus,
) {
    if userdata.is_null() || report.is_null() {
        return;
    }
    let report = &*report;
    let statuses = &mut (*(userdata as *mut Hooks)).statuses;
    let id = visible_text(report.id);
    let state = match report.state {
        ffi::GHOSTTY_PROGRAM_STATUS_STATE_IDLE => ProgramState::Idle,
        ffi::GHOSTTY_PROGRAM_STATUS_STATE_WORKING => ProgramState::Working,
        ffi::GHOSTTY_PROGRAM_STATUS_STATE_DONE => ProgramState::Done,
        ffi::GHOSTTY_PROGRAM_STATUS_STATE_BLOCKED => ProgramState::Blocked,
        ffi::GHOSTTY_PROGRAM_STATUS_STATE_ERROR => ProgramState::Error,
        ffi::GHOSTTY_PROGRAM_STATUS_STATE_CLEAR => return statuses.clear(&id),
        _ => return,
    };
    let need = match report.kind {
        ffi::GHOSTTY_PROGRAM_STATUS_KIND_PERMISSION => Some(Need::Permission),
        ffi::GHOSTTY_PROGRAM_STATUS_KIND_QUESTION => Some(Need::Question),
        ffi::GHOSTTY_PROGRAM_STATUS_KIND_AUTH => Some(Need::Auth),
        _ => None,
    };
    statuses.put(ProgramStatus {
        id,
        state,
        need: need.filter(|_| state == ProgramState::Blocked),
        progress: (0..=100).contains(&report.progress).then_some(report.progress as u8),
        app: visible_text(report.app),
        title: visible_text(report.title),
        message: visible_text(report.message),
    });
}

unsafe fn visible_text(text: ffi::GhosttyString) -> String {
    if text.ptr.is_null() || text.len == 0 {
        return String::new();
    }
    String::from_utf8_lossy(std::slice::from_raw_parts(text.ptr, text.len)).chars().filter(|c| !invisible(*c)).collect()
}

fn invisible(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{ad}' | '\u{61c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}')
}

unsafe extern "C" fn device_attributes(
    _terminal: ffi::GhosttyTerminal,
    _userdata: *mut c_void,
    out: *mut ffi::GhosttyDeviceAttributes,
) -> bool {
    if out.is_null() {
        return false;
    }
    let mut features = [0u16; 64];
    features[..DA1_FEATURES.len()].copy_from_slice(&DA1_FEATURES);
    *out = ffi::GhosttyDeviceAttributes {
        primary: ffi::GhosttyDeviceAttributesPrimary { conformance_level: 62, features, num_features: DA1_FEATURES.len() },
        secondary: ffi::GhosttyDeviceAttributesSecondary { device_type: 1, firmware_version: 10, rom_cartridge: 0 },
        tertiary: ffi::GhosttyDeviceAttributesTertiary { unit_id: 0 },
    };
    true
}

unsafe extern "C" fn size_report(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    out: *mut ffi::GhosttySizeReportSize,
) -> bool {
    if userdata.is_null() || out.is_null() {
        return false;
    }
    *out = (*(userdata as *mut Hooks)).size;
    true
}

#[cfg(all(test, kanna_vt_abi))]
mod abi {
    use super::ffi::*;
    use std::mem::{offset_of, size_of};

    extern "C" {
        fn kanna_vt_abi(out: *mut i64, cap: usize) -> usize;
    }

    #[test]
    fn ffi_matches_the_headers() {
        let rust: &[(&str, i64)] = &[
            ("sizeof GhosttyResult", size_of::<GhosttyResult>() as i64),
            ("GHOSTTY_SUCCESS", GHOSTTY_SUCCESS as i64),
            ("GHOSTTY_OUT_OF_MEMORY", GHOSTTY_OUT_OF_MEMORY as i64),
            ("GHOSTTY_INVALID_VALUE", GHOSTTY_INVALID_VALUE as i64),
            ("GHOSTTY_OUT_OF_SPACE", GHOSTTY_OUT_OF_SPACE as i64),
            ("GHOSTTY_NO_VALUE", GHOSTTY_NO_VALUE as i64),
            ("sizeof GhosttyString", size_of::<GhosttyString>() as i64),
            ("GhosttyString.len", offset_of!(GhosttyString, len) as i64),
            ("GHOSTTY_FORMATTER_FORMAT_PLAIN", GHOSTTY_FORMATTER_FORMAT_PLAIN as i64),
            ("GHOSTTY_TERMINAL_OPT_USERDATA", GHOSTTY_TERMINAL_OPT_USERDATA as i64),
            ("GHOSTTY_TERMINAL_OPT_WRITE_PTY", GHOSTTY_TERMINAL_OPT_WRITE_PTY as i64),
            ("GHOSTTY_TERMINAL_OPT_SIZE", GHOSTTY_TERMINAL_OPT_SIZE as i64),
            ("GHOSTTY_TERMINAL_OPT_TITLE_CHANGED", GHOSTTY_TERMINAL_OPT_TITLE_CHANGED as i64),
            ("GHOSTTY_TERMINAL_OPT_COLOR_SCHEME", GHOSTTY_TERMINAL_OPT_COLOR_SCHEME as i64),
            ("GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND", GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND as i64),
            ("GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND", GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND as i64),
            ("GHOSTTY_TERMINAL_OPT_COLOR_CURSOR", GHOSTTY_TERMINAL_OPT_COLOR_CURSOR as i64),
            ("GHOSTTY_TERMINAL_OPT_COLOR_PALETTE", GHOSTTY_TERMINAL_OPT_COLOR_PALETTE as i64),
            ("GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE", GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE as i64),
            ("GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK", GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK as i64),
            ("GHOSTTY_TERMINAL_OPT_PWD_CHANGED", GHOSTTY_TERMINAL_OPT_PWD_CHANGED as i64),
            ("GHOSTTY_TERMINAL_OPT_MODE_DEFAULT", GHOSTTY_TERMINAL_OPT_MODE_DEFAULT as i64),
            ("GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND", GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND as i64),
            ("GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND", GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND as i64),
            ("GHOSTTY_TERMINAL_DATA_MODE", GHOSTTY_TERMINAL_DATA_MODE as i64),
            ("GHOSTTY_COLOR_SCHEME_LIGHT", GHOSTTY_COLOR_SCHEME_LIGHT as i64),
            ("GHOSTTY_COLOR_SCHEME_DARK", GHOSTTY_COLOR_SCHEME_DARK as i64),
            ("sizeof GhosttyColorScheme", size_of::<i32>() as i64),
            ("GHOSTTY_TERMINAL_CURSOR_STYLE_BAR", GHOSTTY_TERMINAL_CURSOR_STYLE_BAR as i64),
            ("GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK", GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK as i64),
            ("GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE", GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE as i64),
            ("GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW", GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW as i64),
            ("sizeof GhosttyTerminalCursorStyle", size_of::<i32>() as i64),
            ("sizeof GhosttyColorRgb", size_of::<GhosttyColorRgb>() as i64),
            ("sizeof GhosttyTerminalModeConfig", size_of::<GhosttyTerminalModeConfig>() as i64),
            ("GhosttyTerminalModeConfig.value", offset_of!(GhosttyTerminalModeConfig, value) as i64),
            ("GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES", GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES as i64),
            ("GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES", GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES as i64),
            ("GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES", GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES as i64),
            ("GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES", GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES as i64),
            ("GHOSTTY_TERMINAL_DATA_COLS", GHOSTTY_TERMINAL_DATA_COLS as i64),
            ("GHOSTTY_TERMINAL_DATA_ROWS", GHOSTTY_TERMINAL_DATA_ROWS as i64),
            ("GHOSTTY_TERMINAL_DATA_CURSOR_X", GHOSTTY_TERMINAL_DATA_CURSOR_X as i64),
            ("GHOSTTY_TERMINAL_DATA_CURSOR_Y", GHOSTTY_TERMINAL_DATA_CURSOR_Y as i64),
            ("GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN", GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN as i64),
            ("GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE", GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE as i64),
            ("GHOSTTY_TERMINAL_DATA_TITLE", GHOSTTY_TERMINAL_DATA_TITLE as i64),
            ("GHOSTTY_TERMINAL_DATA_PWD", GHOSTTY_TERMINAL_DATA_PWD as i64),
            ("GHOSTTY_TERMINAL_DATA_TOTAL_ROWS", GHOSTTY_TERMINAL_DATA_TOTAL_ROWS as i64),
            ("GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS", GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS as i64),
            ("GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_BYTES", GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_BYTES as i64),
            ("GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_LINES", GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_LINES as i64),
            ("sizeof GhosttyTerminalScreen", size_of::<i32>() as i64),
            ("GHOSTTY_TERMINAL_SCREEN_ALTERNATE", GHOSTTY_TERMINAL_SCREEN_ALTERNATE as i64),
            ("GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES", GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES as i64),
            ("GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION", GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION as i64),
            ("GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_ROWS", GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_ROWS as i64),
            ("GHOSTTY_POINT_TAG_SCREEN", GHOSTTY_POINT_TAG_SCREEN as i64),
            ("sizeof GhosttyPointCoordinate", size_of::<GhosttyPointCoordinate>() as i64),
            ("GhosttyPointCoordinate.y", offset_of!(GhosttyPointCoordinate, y) as i64),
            ("sizeof GhosttyPointValue", size_of::<GhosttyPointValue>() as i64),
            ("sizeof GhosttyPoint", size_of::<GhosttyPoint>() as i64),
            ("GhosttyPoint.value", offset_of!(GhosttyPoint, value) as i64),
            ("sizeof GhosttyGridRef", size_of::<GhosttyGridRef>() as i64),
            ("GhosttyGridRef.node", offset_of!(GhosttyGridRef, node) as i64),
            ("GhosttyGridRef.x", offset_of!(GhosttyGridRef, x) as i64),
            ("GhosttyGridRef.y", offset_of!(GhosttyGridRef, y) as i64),
            ("sizeof GhosttySelection", size_of::<GhosttySelection>() as i64),
            ("GhosttySelection.start", offset_of!(GhosttySelection, start) as i64),
            ("GhosttySelection.end", offset_of!(GhosttySelection, end) as i64),
            ("GhosttySelection.rectangle", offset_of!(GhosttySelection, rectangle) as i64),
            ("sizeof GhosttyTerminalSelectionFormatOptions", size_of::<GhosttyTerminalSelectionFormatOptions>() as i64),
            ("GhosttyTerminalSelectionFormatOptions.emit", offset_of!(GhosttyTerminalSelectionFormatOptions, emit) as i64),
            ("GhosttyTerminalSelectionFormatOptions.unwrap", offset_of!(GhosttyTerminalSelectionFormatOptions, unwrap) as i64),
            ("GhosttyTerminalSelectionFormatOptions.trim", offset_of!(GhosttyTerminalSelectionFormatOptions, trim) as i64),
            ("GhosttyTerminalSelectionFormatOptions.selection", offset_of!(GhosttyTerminalSelectionFormatOptions, selection) as i64),
            ("sizeof GhosttyDeviceAttributesPrimary", size_of::<GhosttyDeviceAttributesPrimary>() as i64),
            ("GhosttyDeviceAttributesPrimary.features", offset_of!(GhosttyDeviceAttributesPrimary, features) as i64),
            ("GhosttyDeviceAttributesPrimary.num_features", offset_of!(GhosttyDeviceAttributesPrimary, num_features) as i64),
            ("sizeof GhosttyDeviceAttributesSecondary", size_of::<GhosttyDeviceAttributesSecondary>() as i64),
            ("GhosttyDeviceAttributesSecondary.firmware_version", offset_of!(GhosttyDeviceAttributesSecondary, firmware_version) as i64),
            ("GhosttyDeviceAttributesSecondary.rom_cartridge", offset_of!(GhosttyDeviceAttributesSecondary, rom_cartridge) as i64),
            ("sizeof GhosttyDeviceAttributesTertiary", size_of::<GhosttyDeviceAttributesTertiary>() as i64),
            ("sizeof GhosttyDeviceAttributes", size_of::<GhosttyDeviceAttributes>() as i64),
            ("GhosttyDeviceAttributes.secondary", offset_of!(GhosttyDeviceAttributes, secondary) as i64),
            ("GhosttyDeviceAttributes.tertiary", offset_of!(GhosttyDeviceAttributes, tertiary) as i64),
            ("sizeof GhosttySizeReportSize", size_of::<GhosttySizeReportSize>() as i64),
            ("GhosttySizeReportSize.columns", offset_of!(GhosttySizeReportSize, columns) as i64),
            ("GhosttySizeReportSize.cell_width", offset_of!(GhosttySizeReportSize, cell_width) as i64),
            ("GhosttySizeReportSize.cell_height", offset_of!(GhosttySizeReportSize, cell_height) as i64),
            ("GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT", GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT as i64),
            ("GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE", GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE as i64),
            ("GHOSTTY_TERMINAL_PROGRESS_STATE_SET", GHOSTTY_TERMINAL_PROGRESS_STATE_SET as i64),
            ("GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR", GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR as i64),
            ("GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE", GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE as i64),
            ("GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE", GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE as i64),
            ("sizeof GhosttyTerminalProgressReport", size_of::<GhosttyTerminalProgressReport>() as i64),
            ("GhosttyTerminalProgressReport.state", offset_of!(GhosttyTerminalProgressReport, state) as i64),
            ("GhosttyTerminalProgressReport.progress", offset_of!(GhosttyTerminalProgressReport, progress) as i64),
            ("GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT", GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT as i64),
            ("GHOSTTY_SEMANTIC_PROMPT_PROMPT_START", GHOSTTY_SEMANTIC_PROMPT_PROMPT_START as i64),
            ("GHOSTTY_SEMANTIC_PROMPT_PROMPT_PRIMARY", GHOSTTY_SEMANTIC_PROMPT_PROMPT_PRIMARY as i64),
            ("sizeof GhosttyTerminalSemanticPrompt", size_of::<GhosttyTerminalSemanticPrompt>() as i64),
            ("GhosttyTerminalSemanticPrompt.kind", offset_of!(GhosttyTerminalSemanticPrompt, kind) as i64),
            ("GhosttyTerminalSemanticPrompt.prompt_kind", offset_of!(GhosttyTerminalSemanticPrompt, prompt_kind) as i64),
            ("GhosttyTerminalSemanticPrompt.has_exit_code", offset_of!(GhosttyTerminalSemanticPrompt, has_exit_code) as i64),
            ("GhosttyTerminalSemanticPrompt.exit_code", offset_of!(GhosttyTerminalSemanticPrompt, exit_code) as i64),
            ("GhosttyTerminalSemanticPrompt.command", offset_of!(GhosttyTerminalSemanticPrompt, command) as i64),
            ("GhosttyTerminalSemanticPrompt.error", offset_of!(GhosttyTerminalSemanticPrompt, error) as i64),
            ("GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS", GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS as i64),
            ("GHOSTTY_PROGRAM_STATUS_STATE_IDLE", GHOSTTY_PROGRAM_STATUS_STATE_IDLE as i64),
            ("GHOSTTY_PROGRAM_STATUS_STATE_WORKING", GHOSTTY_PROGRAM_STATUS_STATE_WORKING as i64),
            ("GHOSTTY_PROGRAM_STATUS_STATE_DONE", GHOSTTY_PROGRAM_STATUS_STATE_DONE as i64),
            ("GHOSTTY_PROGRAM_STATUS_STATE_BLOCKED", GHOSTTY_PROGRAM_STATUS_STATE_BLOCKED as i64),
            ("GHOSTTY_PROGRAM_STATUS_STATE_ERROR", GHOSTTY_PROGRAM_STATUS_STATE_ERROR as i64),
            ("GHOSTTY_PROGRAM_STATUS_STATE_CLEAR", GHOSTTY_PROGRAM_STATUS_STATE_CLEAR as i64),
            ("sizeof GhosttyProgramStatusState", size_of::<i32>() as i64),
            ("GHOSTTY_PROGRAM_STATUS_KIND_PERMISSION", GHOSTTY_PROGRAM_STATUS_KIND_PERMISSION as i64),
            ("GHOSTTY_PROGRAM_STATUS_KIND_QUESTION", GHOSTTY_PROGRAM_STATUS_KIND_QUESTION as i64),
            ("GHOSTTY_PROGRAM_STATUS_KIND_AUTH", GHOSTTY_PROGRAM_STATUS_KIND_AUTH as i64),
            ("sizeof GhosttyProgramStatusKind", size_of::<i32>() as i64),
            ("sizeof GhosttyTerminalProgramStatus", size_of::<GhosttyTerminalProgramStatus>() as i64),
            ("GhosttyTerminalProgramStatus.state", offset_of!(GhosttyTerminalProgramStatus, state) as i64),
            ("GhosttyTerminalProgramStatus.kind", offset_of!(GhosttyTerminalProgramStatus, kind) as i64),
            ("GhosttyTerminalProgramStatus.progress", offset_of!(GhosttyTerminalProgramStatus, progress) as i64),
            ("GhosttyTerminalProgramStatus.id", offset_of!(GhosttyTerminalProgramStatus, id) as i64),
            ("GhosttyTerminalProgramStatus.app", offset_of!(GhosttyTerminalProgramStatus, app) as i64),
            ("GhosttyTerminalProgramStatus.title", offset_of!(GhosttyTerminalProgramStatus, title) as i64),
            ("GhosttyTerminalProgramStatus.message", offset_of!(GhosttyTerminalProgramStatus, message) as i64),
            ("GHOSTTY_KEY_ACTION_PRESS", GHOSTTY_KEY_ACTION_PRESS as i64),
            ("GHOSTTY_MODS_SHIFT", GHOSTTY_MODS_SHIFT as i64),
            ("GHOSTTY_MODS_CTRL", GHOSTTY_MODS_CTRL as i64),
            ("GHOSTTY_MODS_ALT", GHOSTTY_MODS_ALT as i64),
            ("GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT", GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT as i64),
            ("GHOSTTY_OPTION_AS_ALT_TRUE", GHOSTTY_OPTION_AS_ALT_TRUE as i64),
            ("sizeof GhosttyKey", size_of::<i32>() as i64),
            ("sizeof GhosttyOptionAsAlt", size_of::<i32>() as i64),
            ("GHOSTTY_KEY_UNIDENTIFIED", GHOSTTY_KEY_UNIDENTIFIED as i64),
            ("GHOSTTY_KEY_BACKQUOTE", GHOSTTY_KEY_BACKQUOTE as i64),
            ("GHOSTTY_KEY_BACKSLASH", GHOSTTY_KEY_BACKSLASH as i64),
            ("GHOSTTY_KEY_BRACKET_LEFT", GHOSTTY_KEY_BRACKET_LEFT as i64),
            ("GHOSTTY_KEY_BRACKET_RIGHT", GHOSTTY_KEY_BRACKET_RIGHT as i64),
            ("GHOSTTY_KEY_COMMA", GHOSTTY_KEY_COMMA as i64),
            ("GHOSTTY_KEY_DIGIT_0", GHOSTTY_KEY_DIGIT_0 as i64),
            ("GHOSTTY_KEY_DIGIT_9", (GHOSTTY_KEY_DIGIT_0 + 9) as i64),
            ("GHOSTTY_KEY_EQUAL", GHOSTTY_KEY_EQUAL as i64),
            ("GHOSTTY_KEY_A", GHOSTTY_KEY_A as i64),
            ("GHOSTTY_KEY_Z", (GHOSTTY_KEY_A + 25) as i64),
            ("GHOSTTY_KEY_MINUS", GHOSTTY_KEY_MINUS as i64),
            ("GHOSTTY_KEY_PERIOD", GHOSTTY_KEY_PERIOD as i64),
            ("GHOSTTY_KEY_QUOTE", GHOSTTY_KEY_QUOTE as i64),
            ("GHOSTTY_KEY_SEMICOLON", GHOSTTY_KEY_SEMICOLON as i64),
            ("GHOSTTY_KEY_SLASH", GHOSTTY_KEY_SLASH as i64),
            ("GHOSTTY_KEY_BACKSPACE", GHOSTTY_KEY_BACKSPACE as i64),
            ("GHOSTTY_KEY_ENTER", GHOSTTY_KEY_ENTER as i64),
            ("GHOSTTY_KEY_SPACE", GHOSTTY_KEY_SPACE as i64),
            ("GHOSTTY_KEY_TAB", GHOSTTY_KEY_TAB as i64),
            ("GHOSTTY_KEY_DELETE", GHOSTTY_KEY_DELETE as i64),
            ("GHOSTTY_KEY_END", GHOSTTY_KEY_END as i64),
            ("GHOSTTY_KEY_HOME", GHOSTTY_KEY_HOME as i64),
            ("GHOSTTY_KEY_INSERT", GHOSTTY_KEY_INSERT as i64),
            ("GHOSTTY_KEY_PAGE_DOWN", GHOSTTY_KEY_PAGE_DOWN as i64),
            ("GHOSTTY_KEY_PAGE_UP", GHOSTTY_KEY_PAGE_UP as i64),
            ("GHOSTTY_KEY_ARROW_DOWN", GHOSTTY_KEY_ARROW_DOWN as i64),
            ("GHOSTTY_KEY_ARROW_LEFT", GHOSTTY_KEY_ARROW_LEFT as i64),
            ("GHOSTTY_KEY_ARROW_RIGHT", GHOSTTY_KEY_ARROW_RIGHT as i64),
            ("GHOSTTY_KEY_ARROW_UP", GHOSTTY_KEY_ARROW_UP as i64),
            ("GHOSTTY_KEY_ESCAPE", GHOSTTY_KEY_ESCAPE as i64),
            ("GHOSTTY_KEY_F1", GHOSTTY_KEY_F1 as i64),
            ("GHOSTTY_KEY_F12", (GHOSTTY_KEY_F1 + 11) as i64),
        ];
        let mut c = vec![0i64; rust.len() + 8];
        let n = unsafe { kanna_vt_abi(c.as_mut_ptr(), c.len()) };
        assert_eq!(n, rust.len(), "csrc/abi.c and this list must name the same items");
        for (i, (name, value)) in rust.iter().enumerate() {
            assert_eq!(*value, c[i], "{name}: Rust has {value}, the C headers {}", c[i]);
        }
    }
}
