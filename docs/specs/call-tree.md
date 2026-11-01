# Call Tree

**Status:** designed, not built · **Builds on:** [markdown pane](app-panes.md), [diagram check](diagram-check.md), [code tour](code-tour.md) · **Mockup:** [call-tree.html](../../references/mockups/call-tree.html) · **Why:** agents walking you through a change that crosses processes (CLI, daemon, app) drew the call stack as a mermaid `sequenceDiagram` or `flowchart`, and neither shows calls nested in calls, or which process each runs in

A fenced `calls` block in a markdown doc draws as a call tree: one indented row per call, a coloured lane per process, a hop row where a call crosses into another process, and each row's `file:line` on the right, opening the code there. A call stack is the same block with one call a level: the path one request takes through the layers, or a crash's backtrace. It's how the `kanna-explain` skill has agents show a change's calls before commenting on each part.

| Dark, with a comment on a row | Light |
|---|---|
| ![The rename of a Shelf task as a call tree: main, parse and wire in the kanna CLI lane, a socket hop, then Tasks::name with rename_pane and store in the kanna-daemon lane; a kanna comment above Tasks::name, whose row is outlined](call-tree.png) | ![The same call tree in light mode, no comment](call-tree-light.png) |

## Contents

1. [The block](#1-the-block)
2. [Rules](#2-rules)
3. [How it's built](#3-how-its-built)

## 1. The block

````markdown
```calls
kanna CLI
main          crates/kanna/src/lib.rs:17                kanna task name t12 "New name"
  parse       crates/kanna/src/commands.rs:440          task name TASK NAME → request
  wire        crates/kanna/src/lib.rs:316               task name with a TASK → tasks
    -> socket {"cmd":"tasks","action":"name"}
kanna-daemon
    Tasks::name    crates/kanna-daemon/src/tasks.rs:1043  finds the task on the Shelf
      rename_pane  crates/kanna-mux/src/core.rs:1371      its held agent's pane
      store        crates/kanna-daemon/src/tasks.rs:1067  on the reply: the new name
```
````

| Line | Is |
|---|---|
| No location (`kanna CLI`) | A **process**: the calls under it, up to the next process, are in its lane |
| `NAME  PATH:LINE  [NOTE]` | A **call**: its depth is its indent (two spaces a level); fields are split by two or more spaces |
| `-> LABEL` | A **hop**: the next call is made in the next process, carried by LABEL (a socket request, an XPC message) |

## 2. Rules

- **One lane per process,** in the order they first appear, each with its colour from a fixed set; the legend above the rows names them. A process can come back (a reply handled in the CLI again).
- **Indent is nesting.** A call is a child of the nearest call above it with less indent; tree guides join them, through hop rows and comments. When a call has two or more children they're numbered 1, 2, …, in the order they run.
- **A call stack is a call tree** with one call a level: it draws the same, with no numbers. A backtrace (a panic, a crash report, `lldb`'s `bt`) goes in as one, innermost call last. A frame outside the repo keeps its own path (absolute, or `~/.cargo/…`): it's drawn dimmed, isn't a link and isn't checked.
- **Locations are repo-relative** (to the doc's repo). A row shows the location's package (the folder holding `src/`) dimmed, then `file:line`; the full path on hover. A click opens it in a code tab at that line, as `kanna open code PATH:LINE`.
- **Comments:** `kanna comment DOC:LINE` on a call's source line outlines that row and shows the comment above it, like a comment on code. A walkthrough steps down the tree one row at a time.
- **Checked** by `kanna diagrams check` and `kanna open md`, with mermaid's: each `FILE:LINE: message` for a repo path that doesn't exist, a line past the file's end, an indent that jumps more than one level, a call before any process, or a hop not followed by a process.
- **In a [markdown diff](markdown-diff.md),** a changed block shows before and after, like a diagram.

## 3. How it's built

- **Parser** in Rust, beside `kanna_merman::check`: it turns the block into processes, calls (depth, name, path, line, note) and hops, and gives the failures `kanna diagrams check` prints. The app gets the parsed tree through the same FFI the markdown pane uses for mermaid.
- **Renderer** in the markdown pane (`MarkdownRenderer`, where a `mermaid` block becomes a diagram): a `calls` block becomes a call-tree view, rows in the body font's monospace, colours from the theme's diagram palette.
