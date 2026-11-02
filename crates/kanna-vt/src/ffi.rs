#![allow(non_camel_case_types)]

use std::ffi::c_void;

pub type GhosttyResult = i32;
pub const GHOSTTY_SUCCESS: GhosttyResult = 0;
pub const GHOSTTY_OUT_OF_MEMORY: GhosttyResult = -1;
pub const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
pub const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
pub const GHOSTTY_NO_VALUE: GhosttyResult = -4;

#[repr(C)]
pub struct GhosttyTerminalImpl {
    _private: [u8; 0],
}
pub type GhosttyTerminal = *mut GhosttyTerminalImpl;

#[repr(C)]
pub struct GhosttySnapshotDecoderImpl {
    _private: [u8; 0],
}
pub type GhosttySnapshotDecoder = *mut GhosttySnapshotDecoderImpl;

#[repr(C)]
pub struct GhosttyKeyEncoderImpl {
    _private: [u8; 0],
}
pub type GhosttyKeyEncoder = *mut GhosttyKeyEncoderImpl;

#[repr(C)]
pub struct GhosttyKeyEventImpl {
    _private: [u8; 0],
}
pub type GhosttyKeyEvent = *mut GhosttyKeyEventImpl;
#[repr(C)]
pub struct GhosttyAllocator {
    _private: [u8; 0],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyString {
    pub ptr: *const u8,
    pub len: usize,
}
pub const GHOSTTY_FORMATTER_FORMAT_PLAIN: i32 = 0;
pub const GHOSTTY_TERMINAL_OPT_USERDATA: i32 = 0;
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: i32 = 1;
pub const GHOSTTY_TERMINAL_OPT_TITLE_CHANGED: i32 = 5;
pub const GHOSTTY_TERMINAL_OPT_SIZE: i32 = 6;
pub const GHOSTTY_TERMINAL_OPT_COLOR_SCHEME: i32 = 7;
pub const GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES: i32 = 8;
pub const GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND: i32 = 11;
pub const GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND: i32 = 12;
pub const GHOSTTY_TERMINAL_OPT_COLOR_CURSOR: i32 = 13;
pub const GHOSTTY_TERMINAL_OPT_COLOR_PALETTE: i32 = 14;
pub const GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE: i32 = 22;
pub const GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK: i32 = 23;
pub const GHOSTTY_TERMINAL_OPT_PWD_CHANGED: i32 = 25;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES: i32 = 27;
pub const GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES: i32 = 28;
pub const GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES: i32 = 31;
pub const GHOSTTY_TERMINAL_OPT_MODE_DEFAULT: i32 = 33;
pub const GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT: i32 = 30;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE: i32 = 0;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_SET: i32 = 1;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR: i32 = 2;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE: i32 = 3;
pub const GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE: i32 = 4;
pub const GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT: i32 = 42;
pub const GHOSTTY_SEMANTIC_PROMPT_PROMPT_START: i32 = 1;
pub const GHOSTTY_SEMANTIC_PROMPT_PROMPT_PRIMARY: i32 = 0;
pub const GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS: i32 = 46;
pub const GHOSTTY_PROGRAM_STATUS_STATE_IDLE: i32 = 0;
pub const GHOSTTY_PROGRAM_STATUS_STATE_WORKING: i32 = 1;
pub const GHOSTTY_PROGRAM_STATUS_STATE_DONE: i32 = 2;
pub const GHOSTTY_PROGRAM_STATUS_STATE_BLOCKED: i32 = 3;
pub const GHOSTTY_PROGRAM_STATUS_STATE_ERROR: i32 = 4;
pub const GHOSTTY_PROGRAM_STATUS_STATE_CLEAR: i32 = 5;
pub const GHOSTTY_PROGRAM_STATUS_KIND_PERMISSION: i32 = 1;
pub const GHOSTTY_PROGRAM_STATUS_KIND_QUESTION: i32 = 2;
pub const GHOSTTY_PROGRAM_STATUS_KIND_AUTH: i32 = 3;
pub const GHOSTTY_TERMINAL_DATA_COLS: i32 = 1;
pub const GHOSTTY_TERMINAL_DATA_ROWS: i32 = 2;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_X: i32 = 3;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_Y: i32 = 4;
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: i32 = 6;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE: i32 = 7;
pub const GHOSTTY_TERMINAL_DATA_TITLE: i32 = 12;
pub const GHOSTTY_TERMINAL_DATA_PWD: i32 = 13;
pub const GHOSTTY_TERMINAL_DATA_TOTAL_ROWS: i32 = 14;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: i32 = 15;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_BYTES: i32 = 34;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_LINES: i32 = 35;
pub const GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND: i32 = 18;
pub const GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND: i32 = 19;
pub const GHOSTTY_TERMINAL_DATA_MODE: i32 = 37;
pub const GHOSTTY_COLOR_SCHEME_LIGHT: i32 = 0;
pub const GHOSTTY_COLOR_SCHEME_DARK: i32 = 1;
pub const GHOSTTY_TERMINAL_CURSOR_STYLE_BAR: i32 = 0;
pub const GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK: i32 = 1;
pub const GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE: i32 = 2;
pub const GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW: i32 = 3;
pub const GHOSTTY_TERMINAL_SCREEN_ALTERNATE: i32 = 1;
pub const GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES: i32 = 0;
pub const GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION: i32 = 1;
pub const GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_ROWS: i32 = 6;
pub const GHOSTTY_POINT_TAG_SCREEN: i32 = 2;
pub const GHOSTTY_KEY_ACTION_PRESS: i32 = 1;
pub const GHOSTTY_MODS_SHIFT: u16 = 1 << 0;
pub const GHOSTTY_MODS_CTRL: u16 = 1 << 1;
pub const GHOSTTY_MODS_ALT: u16 = 1 << 2;
pub const GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT: i32 = 6;
pub const GHOSTTY_OPTION_AS_ALT_TRUE: i32 = 1;
pub const GHOSTTY_KEY_UNIDENTIFIED: i32 = 0;
pub const GHOSTTY_KEY_BACKQUOTE: i32 = 1;
pub const GHOSTTY_KEY_BACKSLASH: i32 = 2;
pub const GHOSTTY_KEY_BRACKET_LEFT: i32 = 3;
pub const GHOSTTY_KEY_BRACKET_RIGHT: i32 = 4;
pub const GHOSTTY_KEY_COMMA: i32 = 5;
pub const GHOSTTY_KEY_DIGIT_0: i32 = 6;
pub const GHOSTTY_KEY_EQUAL: i32 = 16;
pub const GHOSTTY_KEY_A: i32 = 20;
pub const GHOSTTY_KEY_MINUS: i32 = 46;
pub const GHOSTTY_KEY_PERIOD: i32 = 47;
pub const GHOSTTY_KEY_QUOTE: i32 = 48;
pub const GHOSTTY_KEY_SEMICOLON: i32 = 49;
pub const GHOSTTY_KEY_SLASH: i32 = 50;
pub const GHOSTTY_KEY_BACKSPACE: i32 = 53;
pub const GHOSTTY_KEY_ENTER: i32 = 58;
pub const GHOSTTY_KEY_SPACE: i32 = 63;
pub const GHOSTTY_KEY_TAB: i32 = 64;
pub const GHOSTTY_KEY_DELETE: i32 = 68;
pub const GHOSTTY_KEY_END: i32 = 69;
pub const GHOSTTY_KEY_HOME: i32 = 71;
pub const GHOSTTY_KEY_INSERT: i32 = 72;
pub const GHOSTTY_KEY_PAGE_DOWN: i32 = 73;
pub const GHOSTTY_KEY_PAGE_UP: i32 = 74;
pub const GHOSTTY_KEY_ARROW_DOWN: i32 = 75;
pub const GHOSTTY_KEY_ARROW_LEFT: i32 = 76;
pub const GHOSTTY_KEY_ARROW_RIGHT: i32 = 77;
pub const GHOSTTY_KEY_ARROW_UP: i32 = 78;
pub const GHOSTTY_KEY_ESCAPE: i32 = 120;
pub const GHOSTTY_KEY_F1: i32 = 121;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPointCoordinate {
    pub x: u16,
    pub y: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyPointValue {
    pub coordinate: GhosttyPointCoordinate,
    pub _padding: [u64; 2],
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPoint {
    pub tag: i32,
    pub value: GhosttyPointValue,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyGridRef {
    pub size: usize,
    pub node: *mut c_void,
    pub x: u16,
    pub y: u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttySelection {
    pub size: usize,
    pub start: GhosttyGridRef,
    pub end: GhosttyGridRef,
    pub rectangle: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalSelectionFormatOptions {
    pub size: usize,
    pub emit: i32,
    pub unwrap: bool,
    pub trim: bool,
    pub selection: *const GhosttySelection,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyDeviceAttributesPrimary {
    pub conformance_level: u16,
    pub features: [u16; 64],
    pub num_features: usize,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyDeviceAttributesSecondary {
    pub device_type: u16,
    pub firmware_version: u16,
    pub rom_cartridge: u16,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyDeviceAttributesTertiary {
    pub unit_id: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyDeviceAttributes {
    pub primary: GhosttyDeviceAttributesPrimary,
    pub secondary: GhosttyDeviceAttributesSecondary,
    pub tertiary: GhosttyDeviceAttributesTertiary,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GhosttySizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct GhosttyTerminalModeConfig {
    pub mode: u16,
    pub value: bool,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalProgressReport {
    pub size: usize,
    pub state: i32,
    pub progress: i8,
}
pub type GhosttyTerminalProgressReportFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, report: *const GhosttyTerminalProgressReport);
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalSemanticPrompt {
    pub size: usize,
    pub kind: i32,
    pub prompt_kind: i32,
    pub has_exit_code: bool,
    pub exit_code: i32,
    pub command: GhosttyString,
    pub error: GhosttyString,
}
pub type GhosttyTerminalSemanticPromptFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, event: *const GhosttyTerminalSemanticPrompt);
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalProgramStatus {
    pub size: usize,
    pub state: i32,
    pub kind: i32,
    pub progress: i8,
    pub id: GhosttyString,
    pub app: GhosttyString,
    pub title: GhosttyString,
    pub message: GhosttyString,
}
pub type GhosttyTerminalProgramStatusFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, report: *const GhosttyTerminalProgramStatus);
pub type GhosttyTerminalColorSchemeFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, out: *mut i32) -> bool;
pub type GhosttyTerminalChangedFn = unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void);
pub type GhosttyTerminalWritePtyFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, data: *const u8, len: usize);
pub type GhosttyTerminalDeviceAttributesFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, out: *mut GhosttyDeviceAttributes) -> bool;
pub type GhosttyTerminalSizeFn =
    unsafe extern "C" fn(terminal: GhosttyTerminal, userdata: *mut c_void, out: *mut GhosttySizeReportSize) -> bool;

extern "C" {
    pub fn ghostty_free(allocator: *const GhosttyAllocator, ptr: *mut u8, len: usize);
    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_set(terminal: GhosttyTerminal, option: i32, value: *const c_void) -> GhosttyResult;
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    pub fn ghostty_terminal_get(terminal: GhosttyTerminal, data: i32, out: *mut c_void) -> GhosttyResult;
    pub fn ghostty_terminal_grid_ref(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_selection_format_alloc(
        terminal: GhosttyTerminal,
        allocator: *const GhosttyAllocator,
        options: GhosttyTerminalSelectionFormatOptions,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_snapshot_encode_alloc(
        terminal: GhosttyTerminal,
        allocator: *const GhosttyAllocator,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_snapshot_decoder_new_buf(
        allocator: *const GhosttyAllocator,
        decoder: *mut GhosttySnapshotDecoder,
        ptr: *const u8,
        len: usize,
    ) -> GhosttyResult;
    pub fn ghostty_snapshot_decoder_set(
        decoder: GhosttySnapshotDecoder,
        option: i32,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_snapshot_decoder_ready(decoder: GhosttySnapshotDecoder, terminal: *mut GhosttyTerminal) -> GhosttyResult;
    pub fn ghostty_snapshot_decoder_next(decoder: GhosttySnapshotDecoder) -> GhosttyResult;
    pub fn ghostty_snapshot_decoder_get(decoder: GhosttySnapshotDecoder, data: i32, out: *mut c_void) -> GhosttyResult;
    pub fn ghostty_snapshot_decoder_decode(
        decoder: GhosttySnapshotDecoder,
        terminal: *mut GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_snapshot_decoder_free(decoder: GhosttySnapshotDecoder);
    pub fn ghostty_key_encoder_new(allocator: *const GhosttyAllocator, encoder: *mut GhosttyKeyEncoder) -> GhosttyResult;
    pub fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);
    pub fn ghostty_key_encoder_setopt(encoder: GhosttyKeyEncoder, option: i32, value: *const c_void);
    pub fn ghostty_key_encoder_setopt_from_terminal(encoder: GhosttyKeyEncoder, terminal: GhosttyTerminal);
    pub fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        out_buf: *mut u8,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_key_event_new(allocator: *const GhosttyAllocator, event: *mut GhosttyKeyEvent) -> GhosttyResult;
    pub fn ghostty_key_event_free(event: GhosttyKeyEvent);
    pub fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: i32);
    pub fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: i32);
    pub fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: u16);
    pub fn ghostty_key_event_set_consumed_mods(event: GhosttyKeyEvent, mods: u16);
    pub fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: *const u8, len: usize);
    pub fn ghostty_key_event_set_unshifted_codepoint(event: GhosttyKeyEvent, codepoint: u32);
}
