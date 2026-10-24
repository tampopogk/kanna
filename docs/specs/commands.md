# Commands

**Status:** built, except `kanna up` and the layout expression ([arranging panes](arranging-panes.md)) · **Part of:** [the kanna CLI](cli.md)

Every command, option, output and error is kmux's CLI's, except where [the CLI's differences](cli.md#2-where-kanna-differs-from-kmux) say otherwise. `kanna help`, `kanna help COMMAND` or `kanna COMMAND --help` print them. `PANE` can be a name (`site`) or an ID (`term-2`).

## 1. The commands

| Group | Command | What it does |
|---|---|---|
| Panes | `kanna open [term] [--cmd CMD] [--cwd DIR]` | Open a terminal pane (the default type, as in kmux). |
| | `kanna open web URL [--history]` | Open a web pane; `--history` gives it back/forward. |
| | `kanna open md FILE` | Show a markdown file, rendered natively with mermaid diagrams; it reloads when the file changes. |
| | `kanna open ios --app APP [--device NAME]` | Launch an iOS app in a simulator pane. |
| | `kanna close PANE` / `--tab TAB` / `--window WINDOW` | Close a pane, or a tab or window with everything in it. |
| | `kanna focus PANE` / `--tab TAB` / `--window WINDOW` | Bring a pane, tab or window to the front. |
| | `kanna zoom PANE` | Toggle zoom: the pane fills its tab ([zoom](zoom.md)). |
| | `kanna restart PANE` | Restart what's inside a pane. |
| | `kanna send PANE TEXT` | Type text into a terminal pane and press Return on its own, so agents submit it ([kanna send](kanna-send.md)). |
| | `kanna send-key PANE KEY...` | Press keys in a terminal pane ([3](#3-send-key)). Kanna's. |
| | `kanna read PANE [--lines N]` | Print a terminal pane's screen as text; with `--lines`, its last N lines, counting scrollback. Kanna's. |
| | `kanna screenshot [PANE\|--window WINDOW] [-o FILE] [--force]` | Save a PNG of a pane or a window and print its path ([screenshot](screenshot.md)). Kanna's. |
| | `kanna resize PANE SIZE` | Change one pane's size. |
| | `kanna move PANE --to OTHER [--side SIDE]` / `--tab TAB\|new` / `--window WINDOW\|new` | Move a pane beside another, swap two, or move it to another tab or window. `--window` puts it in a new tab at the end of that window, focused, as `--tab new --window` does; to its own window, a pane alone in its tab stays where it is and one in a split leaves it for a new tab. |
| | `kanna navigate PANE URL\|FILE\|--back\|--forward` | Point a web pane at a new URL (or move through its history), or a markdown pane at another `.md` file. |
| Tabs and windows | `kanna arrange LAYOUT` | Rearrange existing panes in one go. Today LAYOUT is kmux's JSON tree; the [layout expression](arranging-panes.md#2-the-layout-expression) is planned. |
| | `kanna rename-tab TAB TITLE` | Rename a tab; it keeps that name instead of following its pane's ([tab names](tab-names.md)). |
| | `kanna move-tab TAB [--window WINDOW\|new] [--index N]` | Reorder a tab, or move it to another window. |
| | `kanna window assign WINDOW TASK` / `kanna window free WINDOW` | Make a window one of a task's, shown and hidden with it, or free it ([workspaces](workspaces.md)). |
| | `kanna workspace list [--all]` | Your workspace (from `$KANNA_TERMINAL`): its dock, tabs and panes, the shown tab, focus and zoom, also off screen; `--all`: every window, workspace and pane ([workspace list](workspace-list.md)). |
| Information | `kanna capabilities` | Show what the running mux supports (and, for Kanna, whether the app is attached). |
| | `kanna diagrams check FILE...` | Check that a markdown file's mermaid diagrams draw in Kanna, without the app; `open md` prints the same failures ([diagram check](diagram-check.md)). Kanna's. |
| | `kanna fleet [status\|pair [CODE]\|name\|ping\|addr\|revoke]` | Your machines: pair one with a code, list them, check one's link, give it an address, or revoke it ([fleet identity](fleet-identity.md#5-interfaces)). Kanna's. |
| More | `kanna help [COMMAND\|instances\|raw]`, `kanna commands [--json]`, `kanna raw REQUEST` | Help; every command, for scripts; one control-protocol request as JSON. |
| | `kanna instances [--json]`, `kanna instances prune` | For developing Kanna: list the running instances of every channel, or delete the folders of dev instances whose worktree is gone ([instances](instances.md)). |
| Planned | `kanna up [FILE]` | Open a whole setup from a file, `kanna.yaml` by default ([layout files](arranging-panes.md#3-layout-files)). |

Kanna's own commands beyond these (`markup`, `rename-pane`, `task`, `status`, `toast` ([toasts](toasts.md)), `diagrams`, `completions`, `kanna PATH`), and how its `window` differs from kmux's, which assigns to a kmux workspace (of kmux's `workspace` command Kanna has only `list`): [the CLI's differences](cli.md#2-where-kanna-differs-from-kmux).

## 2. Options

| `open` option | Meaning |
|---|---|
| `--name NAME` | A stable name to use in later commands. |
| `--split [right\|down\|auto]` | Where to put the new pane relative to the focused one. |
| `--size FRACTION` | How much of the split space the new pane gets, e.g. `1/3`. |
| `--tab` | Open in a new tab instead of splitting, named after the pane: `kanna open code src/store.rs --tab` makes a tab `store.rs` ([tab names](tab-names.md)). |
| `--window ID\|new` | Open in that window, or a new one. |
| `--no-wait` | Return as soon as the pane is created, without waiting until it's running. |

Global options, as kmux's: `--json` (the reply as one line of JSON), `--help`/`-h`, `--version`, `--instance NAME`, `--bg` (`KANNA_BG=1`), `--fresh` (`KANNA_FRESH=1`); and Kanna's `--mux kanna|kmux` (`KANNA_MUX`; `kanna-dev` is another name for `kanna`), `--host MACHINE` (`KANNA_HOST`), which sends the command to another machine in your fleet ([fleet identity §5](fleet-identity.md#5-interfaces)); and `--`, which ends the options: everything after it is text, so `kanna send term-1 -- --json` types `--json` (kmux's CLI strips `--json` from anywhere in the arguments). Exit codes: [CLI](cli.md#5-output-errors-and-exit-codes).

## 3. send-key

`kanna send-key agent down down return`, `kanna send-key server ctrl+c`; the request is `{cmd: "send-key", args: {pane, keys: [...]}}`. Keys are `debug.key`'s names without ⌘:

| Key | Written |
|---|---|
| modifiers, held with the key | `ctrl+`, `alt+` (`opt+`, `option+`), `shift+`, combined: `ctrl+alt+a` |
| named keys | `return` (`enter`), `esc` (`escape`), `tab`, `space`, `backspace` and `delete` (⌫), `forward-delete` (⌦), `up`, `down`, `left`, `right`, `home`, `end`, `pageup`, `pagedown`, `insert`, `f1`–`f12` |
| a character | `a`, `A` (shift+a), `?`, `1`, `é`; on a US layout, so `shift+1` is `!` |
| refused | `cmd+…` (`super+…`): only Kanna.app has meaning for it; anything longer than a character that isn't a named key ("use kanna send") |

- **Encoded in the daemon, for the program's modes.** Each key goes through libghostty-vt's key encoder, set from the pane's screen as the daemon keeps it: application cursor and keypad keys, the kitty keyboard protocol's flags, modifyOtherKeys, alt sending ESC. So `up` is `ESC [ A` to a shell and `ESC O A` to a program that turned on application cursor keys, `esc` is `ESC [ 27 u` to one that asked for the kitty protocol, and the bytes match what Ghostty in the app would send (`ctrl+[` and `ctrl+shift+c` are CSI u, as Ghostty sends them).
- **One key at a time.** Keys are written in order, each its own write 50 ms after the one before, the first 50 ms after the command arrives, so a TUI reads separate key presses rather than a paste, even right after `kanna send`. Every key is checked first: if one isn't a key, or encodes to nothing in the current mode, nothing is sent. The reply comes after the last key is written, so a `read` after it sees its effect.
- **No app, no focus.** It works with the app closed and on a pane that isn't in front, like `send`.
- **Checked by** `crates/kanna-vt/tests/key.rs` (names, ctrl and alt, cursor key modes, kitty, modifyOtherKeys), `crates/kanna/tests/cli.rs` (the bytes a raw-mode program reads, ctrl+c interrupting a shell, the errors) and `send_key` of `scripts/e2e-kanna.sh` (the bytes, and the mock agent's menu, in application cursor mode, answered with `down return`).
