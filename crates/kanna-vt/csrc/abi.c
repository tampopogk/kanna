#define GHOSTTY_STATIC
#include <ghostty/vt.h>
#include <stddef.h>

static const long long kanna_vt_abi_values[] = {
    sizeof(GhosttyResult),
    GHOSTTY_SUCCESS,
    GHOSTTY_OUT_OF_MEMORY,
    GHOSTTY_INVALID_VALUE,
    GHOSTTY_OUT_OF_SPACE,
    GHOSTTY_NO_VALUE,
    sizeof(GhosttyString),
    offsetof(GhosttyString, len),
    GHOSTTY_FORMATTER_FORMAT_PLAIN,
    GHOSTTY_TERMINAL_OPT_USERDATA,
    GHOSTTY_TERMINAL_OPT_WRITE_PTY,
    GHOSTTY_TERMINAL_OPT_SIZE,
    GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
    GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
    GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
    GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
    GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
    GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
    GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE,
    GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK,
    GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
    GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
    GHOSTTY_TERMINAL_DATA_COLOR_FOREGROUND,
    GHOSTTY_TERMINAL_DATA_COLOR_BACKGROUND,
    GHOSTTY_TERMINAL_DATA_MODE,
    GHOSTTY_COLOR_SCHEME_LIGHT,
    GHOSTTY_COLOR_SCHEME_DARK,
    sizeof(GhosttyColorScheme),
    GHOSTTY_TERMINAL_CURSOR_STYLE_BAR,
    GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK,
    GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE,
    GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW,
    sizeof(GhosttyTerminalCursorStyle),
    sizeof(GhosttyColorRgb),
    sizeof(GhosttyTerminalModeConfig),
    offsetof(GhosttyTerminalModeConfig, value),
    GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
    GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
    GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
    GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES,
    GHOSTTY_TERMINAL_DATA_COLS,
    GHOSTTY_TERMINAL_DATA_ROWS,
    GHOSTTY_TERMINAL_DATA_CURSOR_X,
    GHOSTTY_TERMINAL_DATA_CURSOR_Y,
    GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN,
    GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE,
    GHOSTTY_TERMINAL_DATA_TITLE,
    GHOSTTY_TERMINAL_DATA_PWD,
    GHOSTTY_TERMINAL_DATA_TOTAL_ROWS,
    GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
    GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_BYTES,
    GHOSTTY_TERMINAL_DATA_SCROLLBACK_MAX_LINES,
    sizeof(GhosttyTerminalScreen),
    GHOSTTY_TERMINAL_SCREEN_ALTERNATE,
    GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
    GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
    GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_ROWS,
    GHOSTTY_POINT_TAG_SCREEN,
    sizeof(GhosttyPointCoordinate),
    offsetof(GhosttyPointCoordinate, y),
    sizeof(GhosttyPointValue),
    sizeof(GhosttyPoint),
    offsetof(GhosttyPoint, value),
    sizeof(GhosttyGridRef),
    offsetof(GhosttyGridRef, node),
    offsetof(GhosttyGridRef, x),
    offsetof(GhosttyGridRef, y),
    sizeof(GhosttySelection),
    offsetof(GhosttySelection, start),
    offsetof(GhosttySelection, end),
    offsetof(GhosttySelection, rectangle),
    sizeof(GhosttyTerminalSelectionFormatOptions),
    offsetof(GhosttyTerminalSelectionFormatOptions, emit),
    offsetof(GhosttyTerminalSelectionFormatOptions, unwrap),
    offsetof(GhosttyTerminalSelectionFormatOptions, trim),
    offsetof(GhosttyTerminalSelectionFormatOptions, selection),
    sizeof(GhosttyDeviceAttributesPrimary),
    offsetof(GhosttyDeviceAttributesPrimary, features),
    offsetof(GhosttyDeviceAttributesPrimary, num_features),
    sizeof(GhosttyDeviceAttributesSecondary),
    offsetof(GhosttyDeviceAttributesSecondary, firmware_version),
    offsetof(GhosttyDeviceAttributesSecondary, rom_cartridge),
    sizeof(GhosttyDeviceAttributesTertiary),
    sizeof(GhosttyDeviceAttributes),
    offsetof(GhosttyDeviceAttributes, secondary),
    offsetof(GhosttyDeviceAttributes, tertiary),
    sizeof(GhosttySizeReportSize),
    offsetof(GhosttySizeReportSize, columns),
    offsetof(GhosttySizeReportSize, cell_width),
    offsetof(GhosttySizeReportSize, cell_height),
    GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT,
    GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE,
    GHOSTTY_TERMINAL_PROGRESS_STATE_SET,
    GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR,
    GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE,
    GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE,
    sizeof(GhosttyTerminalProgressReport),
    offsetof(GhosttyTerminalProgressReport, state),
    offsetof(GhosttyTerminalProgressReport, progress),
    GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT,
    GHOSTTY_SEMANTIC_PROMPT_PROMPT_START,
    GHOSTTY_SEMANTIC_PROMPT_PROMPT_PRIMARY,
    sizeof(GhosttyTerminalSemanticPrompt),
    offsetof(GhosttyTerminalSemanticPrompt, kind),
    offsetof(GhosttyTerminalSemanticPrompt, prompt_kind),
    offsetof(GhosttyTerminalSemanticPrompt, has_exit_code),
    offsetof(GhosttyTerminalSemanticPrompt, exit_code),
    offsetof(GhosttyTerminalSemanticPrompt, command),
    offsetof(GhosttyTerminalSemanticPrompt, error),
    GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS,
    GHOSTTY_PROGRAM_STATUS_STATE_IDLE,
    GHOSTTY_PROGRAM_STATUS_STATE_WORKING,
    GHOSTTY_PROGRAM_STATUS_STATE_DONE,
    GHOSTTY_PROGRAM_STATUS_STATE_BLOCKED,
    GHOSTTY_PROGRAM_STATUS_STATE_ERROR,
    GHOSTTY_PROGRAM_STATUS_STATE_CLEAR,
    sizeof(GhosttyProgramStatusState),
    GHOSTTY_PROGRAM_STATUS_KIND_PERMISSION,
    GHOSTTY_PROGRAM_STATUS_KIND_QUESTION,
    GHOSTTY_PROGRAM_STATUS_KIND_AUTH,
    sizeof(GhosttyProgramStatusKind),
    sizeof(GhosttyTerminalProgramStatus),
    offsetof(GhosttyTerminalProgramStatus, state),
    offsetof(GhosttyTerminalProgramStatus, kind),
    offsetof(GhosttyTerminalProgramStatus, progress),
    offsetof(GhosttyTerminalProgramStatus, id),
    offsetof(GhosttyTerminalProgramStatus, app),
    offsetof(GhosttyTerminalProgramStatus, title),
    offsetof(GhosttyTerminalProgramStatus, message),
    GHOSTTY_KEY_ACTION_PRESS,
    GHOSTTY_MODS_SHIFT,
    GHOSTTY_MODS_CTRL,
    GHOSTTY_MODS_ALT,
    GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
    GHOSTTY_OPTION_AS_ALT_TRUE,
    sizeof(GhosttyKey),
    sizeof(GhosttyOptionAsAlt),
    GHOSTTY_KEY_UNIDENTIFIED,
    GHOSTTY_KEY_BACKQUOTE,
    GHOSTTY_KEY_BACKSLASH,
    GHOSTTY_KEY_BRACKET_LEFT,
    GHOSTTY_KEY_BRACKET_RIGHT,
    GHOSTTY_KEY_COMMA,
    GHOSTTY_KEY_DIGIT_0,
    GHOSTTY_KEY_DIGIT_9,
    GHOSTTY_KEY_EQUAL,
    GHOSTTY_KEY_A,
    GHOSTTY_KEY_Z,
    GHOSTTY_KEY_MINUS,
    GHOSTTY_KEY_PERIOD,
    GHOSTTY_KEY_QUOTE,
    GHOSTTY_KEY_SEMICOLON,
    GHOSTTY_KEY_SLASH,
    GHOSTTY_KEY_BACKSPACE,
    GHOSTTY_KEY_ENTER,
    GHOSTTY_KEY_SPACE,
    GHOSTTY_KEY_TAB,
    GHOSTTY_KEY_DELETE,
    GHOSTTY_KEY_END,
    GHOSTTY_KEY_HOME,
    GHOSTTY_KEY_INSERT,
    GHOSTTY_KEY_PAGE_DOWN,
    GHOSTTY_KEY_PAGE_UP,
    GHOSTTY_KEY_ARROW_DOWN,
    GHOSTTY_KEY_ARROW_LEFT,
    GHOSTTY_KEY_ARROW_RIGHT,
    GHOSTTY_KEY_ARROW_UP,
    GHOSTTY_KEY_ESCAPE,
    GHOSTTY_KEY_F1,
    GHOSTTY_KEY_F12,
};

#define SIGNATURE(name, ret, ...) static ret (*const check_##name)(__VA_ARGS__) __attribute__((used)) = name
SIGNATURE(ghostty_free, void, const GhosttyAllocator *, uint8_t *, size_t);
SIGNATURE(ghostty_terminal_new, GhosttyResult, const GhosttyAllocator *, GhosttyTerminal *, uint16_t, uint16_t);
SIGNATURE(ghostty_terminal_free, void, GhosttyTerminal);
SIGNATURE(ghostty_terminal_resize, GhosttyResult, GhosttyTerminal, uint16_t, uint16_t, uint32_t, uint32_t);
SIGNATURE(ghostty_terminal_set, GhosttyResult, GhosttyTerminal, GhosttyTerminalOption, const void *);
SIGNATURE(ghostty_terminal_vt_write, void, GhosttyTerminal, const uint8_t *, size_t);
SIGNATURE(ghostty_terminal_get, GhosttyResult, GhosttyTerminal, GhosttyTerminalData, void *);
SIGNATURE(ghostty_terminal_grid_ref, GhosttyResult, GhosttyTerminal, GhosttyPoint, GhosttyGridRef *);
SIGNATURE(ghostty_terminal_selection_format_alloc, GhosttyResult, GhosttyTerminal, const GhosttyAllocator *,
          GhosttyTerminalSelectionFormatOptions, uint8_t **, size_t *);
SIGNATURE(ghostty_snapshot_encode_alloc, GhosttyResult, GhosttyTerminal, const GhosttyAllocator *, uint8_t **, size_t *);
SIGNATURE(ghostty_snapshot_decoder_new_buf, GhosttyResult, const GhosttyAllocator *, GhosttySnapshotDecoder *,
          const uint8_t *, size_t);
SIGNATURE(ghostty_snapshot_decoder_set, GhosttyResult, GhosttySnapshotDecoder, GhosttySnapshotDecoderOption, const void *);
SIGNATURE(ghostty_snapshot_decoder_ready, GhosttyResult, GhosttySnapshotDecoder, GhosttyTerminal *);
SIGNATURE(ghostty_snapshot_decoder_next, GhosttyResult, GhosttySnapshotDecoder);
SIGNATURE(ghostty_snapshot_decoder_get, GhosttyResult, GhosttySnapshotDecoder, GhosttySnapshotDecoderData, void *);
SIGNATURE(ghostty_snapshot_decoder_decode, GhosttyResult, GhosttySnapshotDecoder, GhosttyTerminal *);
SIGNATURE(ghostty_snapshot_decoder_free, void, GhosttySnapshotDecoder);
SIGNATURE(ghostty_key_encoder_new, GhosttyResult, const GhosttyAllocator *, GhosttyKeyEncoder *);
SIGNATURE(ghostty_key_encoder_free, void, GhosttyKeyEncoder);
SIGNATURE(ghostty_key_encoder_setopt, void, GhosttyKeyEncoder, GhosttyKeyEncoderOption, const void *);
SIGNATURE(ghostty_key_encoder_setopt_from_terminal, void, GhosttyKeyEncoder, GhosttyTerminal);
SIGNATURE(ghostty_key_encoder_encode, GhosttyResult, GhosttyKeyEncoder, GhosttyKeyEvent, char *, size_t, size_t *);
SIGNATURE(ghostty_key_event_new, GhosttyResult, const GhosttyAllocator *, GhosttyKeyEvent *);
SIGNATURE(ghostty_key_event_free, void, GhosttyKeyEvent);
SIGNATURE(ghostty_key_event_set_action, void, GhosttyKeyEvent, GhosttyKeyAction);
SIGNATURE(ghostty_key_event_set_key, void, GhosttyKeyEvent, GhosttyKey);
SIGNATURE(ghostty_key_event_set_mods, void, GhosttyKeyEvent, GhosttyMods);
SIGNATURE(ghostty_key_event_set_consumed_mods, void, GhosttyKeyEvent, GhosttyMods);
SIGNATURE(ghostty_key_event_set_utf8, void, GhosttyKeyEvent, const char *, size_t);
SIGNATURE(ghostty_key_event_set_unshifted_codepoint, void, GhosttyKeyEvent, uint32_t);
static void (*const check_write_pty)(GhosttyTerminal, void *, const uint8_t *, size_t) __attribute__((used)) =
    (GhosttyTerminalWritePtyFn)0;
static bool (*const check_device_attributes)(GhosttyTerminal, void *, GhosttyDeviceAttributes *) __attribute__((used)) =
    (GhosttyTerminalDeviceAttributesFn)0;
static bool (*const check_color_scheme)(GhosttyTerminal, void *, GhosttyColorScheme *) __attribute__((used)) =
    (GhosttyTerminalColorSchemeFn)0;
static void (*const check_title_changed)(GhosttyTerminal, void *) __attribute__((used)) = (GhosttyTerminalTitleChangedFn)0;
static void (*const check_pwd_changed)(GhosttyTerminal, void *) __attribute__((used)) = (GhosttyTerminalPwdChangedFn)0;
static void (*const check_progress)(GhosttyTerminal, void *, const GhosttyTerminalProgressReport *) __attribute__((used)) =
    (GhosttyTerminalProgressReportFn)0;
static bool (*const check_size)(GhosttyTerminal, void *, GhosttySizeReportSize *) __attribute__((used)) =
    (GhosttyTerminalSizeFn)0;

size_t kanna_vt_abi(long long *out, size_t cap) {
  size_t n = sizeof kanna_vt_abi_values / sizeof kanna_vt_abi_values[0];
  for (size_t i = 0; i < n && i < cap; i++) out[i] = kanna_vt_abi_values[i];
  return n;
}
