# The App Link

**Status:** built · **Builds on:** [architecture](architecture.md) · **Related:** [the daemon follows the app's build](daemon-build.md), [layout](layout.md), [app panes](app-panes.md)

The app is a client of the daemon, like the CLI, but it stays connected: it shows the daemon's terminals, types into them, and answers the requests only the app can (the layout, its web, markdown and iOS panes, `debug.*`). The **app link** is that one connection, on the daemon's own socket (`$KANNA_SOCKET`).

## Contents

1. [The wire](#1-the-wire)
2. [Messages](#2-messages)
3. [Rules](#3-rules)
4. [The app starts the daemon](#4-the-app-starts-the-daemon)
5. [Glossary](#5-glossary)

## 1. The wire

**The control protocol's wire.** One JSON object per line, both ways. A message with `cmd` is a request, `{"id", "cmd", "args"}`; with an `id` it wants a reply, `{"id", "ok": true, …}` or `{"id", "ok": false, "error": {"code", "message"}}`, and without one it's a notice that gets none. Each side numbers its own requests, and a reply goes to the side that asked. Terminal bytes are base64 in a `data` field.

**Saying it's the app.** The app's first line is `{"id": 0, "cmd": "app.hello", "args": {"pid", "build"}}`. The daemon answers `{"id": 0, "ok": true, "daemon": {"pid", "build"}, "panes": [{"id", "name", "type"}, …]}`, listing every terminal, and the connection is the app link from then on. One app at a time: a second gets `app_attached` and is closed. If the daemon goes away, the app keeps trying to connect, and on reconnecting says hello and attaches its terminals again.

**The daemon follows the app's build.** `build` is the SHA-256 of the daemon binary. With no app attached, a hello of another build gets `other_build` and the daemon stops as on SIGTERM; the app then starts its own, which adopts every terminal. A hello without `build` is accepted. Detail: [daemon build](daemon-build.md).

```mermaid
sequenceDiagram
    participant A as Kanna app
    participant D as Kanna daemon
    participant C as kanna CLI
    A->>D: app.hello
    D->>A: ok, panes p1 p2
    Note over A: restore layout.json, drop panes that are gone, place the new ones
    A->>D: attach p1
    D->>A: snapshot p1 (the screen, GHOSTSNP)
    D->>A: grid p1 80×24
    A->>D: resize p1 to fit the view
    D->>A: grid p1 (the size it now has)
    Note over A,D: from now on, both ways
    A->>D: input p1 (keystrokes)
    D->>A: output p1 (what the program wrote)
    C->>D: open --split right
    D->>A: layout {op: place, pane p3, at: right}
    A->>D: ok {window w1, tab t1}
    D->>C: ok, p3 in w1
    C->>D: debug.type pane s1
    D->>A: debug.type pane p1 (names resolved to IDs)
    A->>D: ok
    D->>C: ok
```

## 2. Messages

| Message | From | What it says |
|---|---|---|
| `app.hello {pid, build}` | app | I'm the app, of this build; the reply accepts or refuses it (`app_attached`, `other_build`) and lists the daemon's terminals |
| `layout {op, …, panes}` (with an `id`) | daemon | one step of a command on the layout ([layout](layout.md#2-layout-steps)); the app's reply is the step's answer |
| `attach {pane}` / `detach {pane}` | app | I show (or no longer show) this terminal |
| `snapshot {pane, data}` | daemon | the terminal's whole screen as a GHOSTSNP snapshot; sent on attach, before anything else for that pane |
| `grid {pane, cols, rows}` | daemon | the terminal's size, after a snapshot or a resize |
| `output {pane, data}` | daemon | bytes the program wrote since the snapshot |
| `exited {pane, code}` | daemon | the program ended |
| `status {pane, status}` | daemon | an agent pane's status changed (`working`, `needs-you`, `idle`), a terminal ended (`exited`), or `null` when it's no longer an agent; for every terminal, shown or not, and once per pane after `app.hello` ([agent status](agent-status.md)) |
| `input {pane, data}` | app | bytes the view wrote: keys, paste, mouse and focus reports |
| `resize {pane, cols, rows, width, height}` | app | the view now fits this many cells (and pixels) |
| `settings {foreground, background, cursor, palette, scheme, cursorStyle, cursorBlink, graphemes}` | app | the terminal settings from the app's Ghostty config, sent before any attach and again when the config or appearance changes ([terminal](terminal.md#2-the-config)) |
| `debug.*` (with an `id`) | daemon | a request from a client, passed on; the app's reply is the client's |
| `start`, `stop`, `pane`; `update`; the app's own requests | both | web, markdown and iOS panes ([app panes](app-panes.md)) |

## 3. Rules

- **Requests from the daemon are answered in order.** The app answers `layout` on its main thread as it arrives, before anything after it; `debug.*` may answer later.
- **The daemon decides a terminal's size.** The view asks with `resize`, only for the size it has (a request Ghostty makes for another, such as its 800×600 before the view sizes it, is dropped); the daemon sets the pty's size, and the view takes the size `grid` gives it, in order with the output ([pane sizes](pane-sizes.md)). A resize beyond 1000 cells a side is refused.
- **Requests for the app** (every `debug.*` the core doesn't answer itself, which is all but `debug.saveState` and `debug.restoreState`) can come from any client. The daemon turns a top-level pane name into its ID, passes the request on and returns the app's reply, so a debug command the app adds needs no change in the daemon. With no app attached they fail with `no_app`; if the app goes away first, `app_gone`; with no answer within 10 s, `timeout`. The app answers one it doesn't know with kmux's `unknown command`. The app's debug commands: [windows](windows.md#5-debug-commands), [terminal](terminal.md#5-debug-mode).
- **`screenshot` is passed on the same way**, but isn't a debug command, so the app always answers it ([screenshot](screenshot.md)).
- **Debug commands are off by default.** The app answers `debug.*` only when started with `KANNA_DEBUG=1` (the test scripts set it). `debug.snapshot` takes a plain file name and writes a new file in `$KANNA_STATE_DIR` (or `$TMPDIR`); it never overwrites one.
- **Limits:** a line over 16 MiB drops the app; an app that falls 16 MiB behind on output stops getting it and starts its terminals over (snapshot, grid) once it has caught up, and one 64 MiB behind is dropped.
- **The layout is the app's:** the daemon keeps none and asks the app for every layout step ([layout](layout.md)).
- **Screens:** the daemon keeps each terminal's screen with libghostty-vt (`crates/kanna-vt`), fed from the pty, with up to 10 MB of scrollback. `read` answers from it, so it works with the app closed.
- **Queries are answered once, by the daemon.** Cursor position, device status and attributes, modes, size reports and XTVERSION are answered by the daemon's screen, written back to the pty as they're parsed, whether or not the app is attached. The app's surfaces parse the same output but drop their replies, so the program never gets two. Colour queries and scheme reports are answered from the app's `settings` ([terminal](terminal.md)).
- **Attaching:** the daemon takes the snapshot on its main loop under the lock the pty reader feeds the screen and the outbox with, and empties the outbox as it does: what the snapshot has isn't sent again, and every later byte follows as `output`. The app loads the snapshot (`ghostty_surface_load_snapshot`), then applies the `grid` that follows; both go through the same queue as output, in order. A restarted terminal is attached the same way, and the app, having had `exited` for it, attaches a new surface instead of loading the snapshot into the exited one ([terminal](terminal.md#4-the-apps-terminal-views)).
- **When a snapshot fails:** inside some unfinished sequences (an APC holding a C1 byte) libghostty-vt can't encode one. The daemon then sends nothing for that pane and tries again after the terminal's next output, or when it exits. If the app's surface refuses a snapshot (a full mailbox), it retries every 2 ms; after 250 refusals it goes on with the live output.

## 4. The app starts the daemon

Kanna.app carries the daemon, the keeper and the `kanna` CLI in `Contents/Helpers`, and nothing else has to be run first.

- **When:** at launch, as soon as nothing answers on the daemon socket; after losing a daemon it was attached to, once nothing has answered for 2 s (so a daemon being restarted on purpose gets there first). At most one start every 10 s. The daemon's lock makes a race harmless: a second daemon on the folder exits ([restarting](daemon-restart.md#1-files-on-disk)).
- **How:** in its own session (`setsid`), so it outlives the app; signals back to their defaults; no descriptors of the app's; stdin from `/dev/null`, stdout and stderr appended to `daemon.log` in the state folder. It gets the app's environment without `KANNA_*` and `KMUX_*` (only `KANNA_STATE_DIR` and `KANNA_DEBUG` pass), plus `KANNA_SOCKET` and `KANNA_KEEPER` (the bundled keeper). The daemon then starts the keeper.
- **Quitting the app** leaves the daemon and its terminals running; the next launch attaches to it if it's the same build, and replaces it if not.
- **Why the app and not launchd:** the daemon is needed only when the app or the CLI is, and the app knows the socket and state folder it uses. A launch agent would add install steps and a second definition of both. On Linux, which has no app yet, the daemon is started by hand.
- **The first terminal.** When the first attach after launch leaves the layout empty, the app opens a terminal, as kmux opens a window. Reopening the app from the Dock with nothing open does the same. `KANNA_NO_INITIAL_WINDOW=1` turns it off (the e2e scenarios set it).

## 5. Glossary

| Term | Meaning |
|---|---|
| **App link** | The app's one connection to the daemon, on the daemon's socket. |
| **Snapshot** | A terminal's whole screen (grid, scrollback, modes, colours, an unfinished sequence) as bytes (GHOSTSNP), sent on attach and saved on disk. |
| **Surface** | One terminal view's Ghostty terminal in the app (GhosttyKit): it draws the screen and handles keys, mouse, selection and clipboard. |
| **Outbox** | The daemon's queue of output not yet sent to the app. |
