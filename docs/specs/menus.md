# Menus and shortcuts

**Status:** built; Reload replaces Restart (2026-10-14); the Pane menu grouped (2026-11-11) · **Reference:** kmux ([`references/kmux`](../../references/kmux/docs/kmux-spec.md) §5) · **Part of:** [Windows, Tabs and Splits](windows.md)

Kanna's menus and shortcuts are kmux's ([its spec](../../references/kmux/docs/kmux-spec.md) §5), with the shortcut the user's Ghostty config binds to the same action where there is one (`new_split:right`, `goto_split:next`, `toggle_split_zoom`, `new_tab`, `next_tab`, `increase_font_size:1`, `copy_to_clipboard`, …):

## Contents

1. [The menus](#1-the-menus)
2. [Rules](#2-rules)
3. [Reload, not Restart](#3-reload-not-restart)
4. [The Pane menu](#4-the-pane-menu)

## 1. The menus

| Menu | Items |
|---|---|
| Kanna | Quit Kanna ⌘Q (no New Instance, unlike kmux) |
| File | New Desk ⌘N ([tasks](tasks.md#2-how-tasks-are-made)), Open Recent ▸ (recent projects, latest first, then Clear Menu: [recent projects](recent-projects.md)) |
| Edit | Undo ⌘Z, Redo ⇧⌘Z, Cut ⌘X, Copy ⌘C, Paste ⌘V, Select All ⌘A (sent to whatever has focus) |
| Pane | New Pane ▸ (New Web / iOS Pane Right and Below, New Markdown Pane Right… and Below…); Split Right ⌘D, Split Down ⇧⌘D, Next Pane ⌘], Previous Pane ⌘[, Zoom ⇧⌘↩, Move Pane to New Window, Close Pane ⌘W; Open URL… ⌘L, Back, Forward, Reload ⌘R, Comments, Home ⇧⌘H ([§4](#4-the-pane-menu)) |
| View | New Tab ⌘T, Next Tab ⇧⌘], Previous Tab ⇧⌘[ (from a Desk's docked pane they switch the shown tab and focus stays in the docked pane; from a tab's pane focus moves into the new tab), Zoom In ⌘+, Zoom Out ⌘−, Actual Size ⌘0 (a markdown or web pane's magnification, which scales the page without re-laying it out, or a terminal's font size; a pinch magnifies markdown and web panes too) |
| Window | New Window (no shortcut: ⌘N is New Desk), Close Window ⇧⌘W, Move Tab to New Window, Next Window ⌘\`, Previous Window ⇧⌘\` (its key equivalent `~` with ⌘, which AppKit matches: [Desk windows](desk-windows.md#4--and-)), and the list of windows |

## 2. Rules

- **Menu commands act on the window in front,** as in any Mac app: the window AppKit has as key, and that window's focused pane, so ⌘W in one window never reaches another's pane (a Desk's planning terminal included). When AppKit has none (a `--bg` app), it's the window the app last had in front, by its own record; never the daemon's, which has no key window ([the app owns the physical layout](physical-layout.md)). Pane, tab and window cycling go in reading order and wrap; zoom is the window's mode: while it's on, the focused pane fills the window wherever focus goes, until ⇧⌘↩ ([zoom](zoom.md)). A new web pane starts with its URL selected for editing; a new markdown pane asks for a file; a new iOS pane opens Settings.
- **Ghostty keybindings** that no menu item takes reach the app as actions: `new_split`, `goto_split` (previous, left and up go back; the rest forward), `toggle_split_zoom`, `new_tab`, `goto_tab` (next, previous), `new_window`, `close_window`, `goto_window`, and `close_surface` (closes the pane). The pane the key was typed in is focused first. `GhosttyRuntime` passes them on (`onAction`, `onClose`); the app decides.
- **Right-clicking a pane** opens the Pane menu for it (it takes focus first): in a terminal unless its program uses the mouse, below Copy and Paste (Copy greyed out without a selection; the right-click keeps it); below WebKit's own items in a web pane, below the text items in a markdown pane, alone in an iOS pane.
- **Instances** ([CLI](instances.md)): unlike kmux, Kanna has no New Instance menu item: kmux used one to work on another project, and in Kanna that's a Desk (File → New Desk). Instances are a developer and testing tool, reached through dev builds and `kanna --instance`. An instance other than the default one shows its name in its window titles (`Tab 1 — w1 · work`) and at the top of the Kanna menu; the name comes from the socket's folder, as the daemon's does.
- **Greyed out, never hidden,** where an item doesn't apply to the focused pane, as in any Mac menu bar: Open URL… on anything but a web pane; Back and Forward on anything but a web, markdown or code pane; Reload on anything but a web or diff pane; Comments on anything but a web or markdown pane; Home on anything but an iOS pane with its screen; Move Pane to New Window on a frozen pane; Close Pane on a docked pane; Next Pane, Previous Pane and Zoom with no pane focused. The menu keeps its shape whatever has focus, so an item is always where the hand expects it.

## 3. Reload, not Restart

The Pane menu has **Reload ⌘R**, for web panes only: it calls the page's own reload, so the pane stays on the page it's on. It replaced Restart ⌘R, which ended a pane's program and ran its command again; Kanna decided this first, and kmux followed (its PR #1).

```mermaid
flowchart LR
    K["⌘R"] --> W{"focused pane"}
    W -->|web| R["the page reloads in place"]
    W -->|"terminal, markdown,<br/>code, iOS"| T["greyed out; a terminal<br/>gets ⌘R like any key"]
    C["kanna restart PANE"] --> D["daemon: the pane's command<br/>runs again"]
```

- **Why no restart on a key:** in a terminal it would end an agent or a shell's program without asking. Restarting a command pane is `kanna restart`, which is deliberate.
- **A task's agent** restarts into its conversation: once it has started, its pane's command is the agent's resuming one ([which agents resume](tasks.md#which-agents-resume)).
- **An exited docked pane** comes back with `kanna restart` ([docked pane](docked-pane.md)).

## 4. The Pane menu

Three groups, from what makes a pane to what acts on the focused one:

```
Pane
├─ New Pane                      ▸  New Web Pane Right
│                                   New Web Pane Below
│                                   New iOS Pane Right
│                                   New iOS Pane Below
│                                   New Markdown Pane Right…
│                                   New Markdown Pane Below…
├─ ─────────────
├─ Split Right                ⌘D
├─ Split Down                ⇧⌘D
├─ Next Pane                  ⌘]
├─ Previous Pane              ⌘[
├─ Zoom                      ⇧⌘↩
├─ Move Pane to New Window
├─ Close Pane                 ⌘W
├─ ─────────────
├─ Open URL…                  ⌘L
├─ Back
├─ Forward
├─ Reload                     ⌘R
├─ Comments
└─ Home                      ⇧⌘H
```

- **New Pane ▸** holds the six non-terminal panes, which have no shortcut; a terminal split stays at the top level with its ⌘D and ⇧⌘D.
- **Every pane's commands** come next: they apply whatever the focused pane is.
- **The focused pane's type's commands** come last, together: web (Open URL…, Back, Forward, Reload, Comments), markdown and code (Back, Forward; markdown's Comments), diff (Reload), iOS (Home). The ones the focused pane's type doesn't have are greyed out ([§2](#2-rules)).
- **Shortcuts** are unchanged by the grouping; `debug.menu` lists the submenu's items under the menu `New Pane`, and e2e `terminal_colours` checks the Pane menu's order.
