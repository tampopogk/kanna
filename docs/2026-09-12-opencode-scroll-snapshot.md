# OpenCode scrolling lost on snapshot hydration

Task: `14932c39`. Baseline: authoritative `origin/main`
`33ae94f5779e0ca621410a88cc93d9e2d605c802` (HEAD and merge-base both exact,
initial working tree clean).

## Reproduction and cause

The installed OpenCode is `/Users/jeremyhale/.opencode/bin/opencode` 1.4.3.
Preserved tasks `b704c4b7` and `fbff18e0` were inspected only with read-only
Kanna task/detail and log APIs. Their running terminals, worktrees and stopped
inference services were not touched. Task `2ca3ce82`'s separate launch failure
belongs to `e218c437`; this change contains no launcher/config repair.

The canonical isolated desktop runner launched this worktree's debug build.
Before interaction, its native identity was verified as
`Kanna — task 14932c39 (0.0.68 @ 33ae94f57)`, with build metadata naming branch
and worktree `task-14932c39`. The regression uses a framework-owned fixture
repository and a task-local isolated OpenCode XDG store. OpenCode's supported
`import` command loads 30 numbered conversation turns, then its real TUI runs
with `--pure --session`. No model is run, and no terminal output is injected.
No prompt is submitted to an inference provider.

Before product edits:

- Fresh OpenCode: 20 upward wheel events emit SGR mouse input
  `ESC [ < 64 ; 13 ; 7 M`, forwarded as KSP `term_input_control`. Painted
  conversation moves from turns 25–29 to 13–17.
- Reload the desktop viewer, retaining the same live OpenCode PTY. Its daemon
  snapshot contains alternate-screen `1049`, mouse tracking `1003`, focus
  `1004` and bracketed-paste `2004`, but **no SGR encoding `1006`**.
- The desktop applies its normal queued reset and snapshot. xterm now emits
  legacy binary mouse reports: bytes `[27,91,77,96,45,39]` at those coordinates.
  Twenty events reach xterm's `onBinary`; zero reach KSP. Painted turns remain
  25–29. The conversation-movement assertion fails.

The chain is `HeadlessTerminal::snapshot_with_metadata` → pinned
`ghostty-xterm-compat-serialize` revision `06895c8` → daemon attach/geometry
snapshot → KSP (including the bounded terminal window) →
`terminalSessionLifecycle` / `applyTerminalSnapshot` → xterm wheel producer.
The serializer restores mouse tracking but omits SGR encoding. The viewer's
reset therefore changes the requested mouse protocol. Desktop currently only
forwards `onData`, so the resulting binary reports disappear before transport.
KSP and daemon control-input routing otherwise preserve bytes and never turn
mouse input into a submission.

This is application scrolling on the alternate screen, not normal xterm
scrollback or an inability to produce wheel events. The passive active-view
observer does not consume the gesture. The fresh-session control proves
OpenCode responds to its requested SGR reports without typing.

## Correction and scope

The daemon snapshot appends `CSI ? 1006 h` only when Ghostty reports that the
program actually enabled SGR mouse encoding. This repairs the authoritative
representation, keeping OpenCode's requested input semantics across attach,
resize and handoff. It is explicitly temporary compatibility for the pinned
serializer; remove it when that dependency preserves the mode itself.

No frontend gesture handler, resize election, alternate-screen ownership,
provider-specific scrolling override, editor behavior, launcher flag, or input
transport was changed. Remote desktop and mobile consume the same corrected
snapshot suffix; KSP's alternate-screen segment is retained intact. No separate
mobile or relay implementation is needed.

The general desktop legacy `onBinary` forwarding gap remains outside this
correction. Forwarding the accidentally downgraded protocol would conceal the
snapshot defect rather than preserve what OpenCode requested. Other omitted
serializer modes have not been generalized into this task.

## Verification

The initial native reproduction and mode round-trip regression both fail
before the correction. The same reload reproduction passes after it: 20 SGR
reports, zero binary reports, and painted turns 13–17 instead of 25–29.

The retained E2E covers ordinary shell scrollback, fresh OpenCode scrolling,
viewer reload, and an actual remote-viewer geometry claim to 90×30 followed by
OpenCode conversation scrolling in the native desktop. It checks painted
conversation markers and forwarded SGR input, not just scrollbar or size.

Commands and task-local receipts:

- `pnpm --dir apps/desktop test:e2e real/opencode-scroll.test.ts`
  (`.tmp/opencode-scroll/run3.log`: original failed reload;
  `run4.log`: same reproduction passing;
  `run7.log`: both final E2E tests passed).
- `cargo test -p kanna-daemon --lib headless_terminal_snapshot`:
  four tests passed, including resize/snapshot/restore, mode disable, ordinary
  shell, cursor visibility, and retained scrollback assertions.
- `pnpm --dir apps/desktop exec vitest run src/composables/terminalViewerInteraction.test.ts src/composables/terminalSnapshotApply.test.ts tests/e2e/realTiers.test.ts`:
  16 tests passed. Existing trusted gesture/resize ownership contracts remain.
- `cargo clippy -p kanna-daemon --lib -- -D warnings` and
  `cargo fmt -p kanna-daemon -- --check`: passed.
- Workspace-wide format checking reports pre-existing formatting differences
  in `crates/kanna-cli/src/commands/tool.rs` and `crates/kanna-mcp/src/main.rs`;
  neither file was changed.

`broken-*` and `fixed-*` under `.tmp/opencode-scroll/` preserve before/after
screenshots, visible text, input traces and snapshots. `identity.json` records
the native title, build metadata and isolated WebDriver endpoint. An initial
fixture-placement attempt failed the harness's live-repository containment
guard before starting OpenCode; the final test uses the framework's normal
fixture location without weakening that guard. Two intermediate expanded-test
runs exposed fixture setup omissions: disabling every provider opened OpenCode's
provider-selection dialog, and the secondary viewer was initially registered
without declaring visibility. The final test corrects both; neither needed a
product change. The canonical runner stopped its app, server, daemon, fixture
PTYs and tmux server; a final process inventory found no task-owned test stack.

Wheel evidence uses DOM `WheelEvent` in the native WKWebView, the same producer
used by tauri-plugin-webdriver 0.2.1's wheel action. It is not a physical
trackpad or iPhone test. Trusted viewer intent is covered by the existing
viewer-gesture contracts; this change does not modify that path. No installed
operator window was automated.
