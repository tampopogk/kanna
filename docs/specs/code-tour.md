# Code Tour

**Status:** built · **Mockup:** [`references/mockups/code-tour.html`](../../references/mockups/code-tour.html) · **Builds on:** the [code viewer](code-viewer.md)

A **code tour** is the agent commenting on code while it explains it, so you keep a mental model of how it works, whether you're a vibe coder, an engineer or not technical at all. As it talks, the agent comments on the part it's talking about: the file opens in a code tab, those lines are tinted and the rest dimmed, and the comment shows just above them. You ask questions as it goes; it comments on whatever answers them.

Kanna gives the agent one command, `kanna comment`; how to explain code is a skill (`kanna-explain`), not a feature, and comments are one of its ways, beside markdown docs with diagrams.

![The agent commented on client.rs:244 to answer a question; the comment covers the whole loop](code-tour.png)

## Contents

1. [The loop](#1-the-loop)
2. [Rules](#2-rules)
3. [CLI](#3-cli)
4. [The skill](#4-the-skill)
5. [How it's built](#5-how-its-built)
6. [Glossary](#6-glossary)

## 1. The loop

```mermaid
sequenceDiagram
    participant You
    participant A as agent
    participant D as daemon
    participant App as Kanna.app
    You->>A: how does the keeper hand a terminal to the daemon?
    A->>D: kanna comment server.rs:92 "…"
    D->>App: snap line 92
    App-->>D: 92–103
    D->>App: code tab for server.rs, with the comment
    A->>You: a line: where it is, go on?
    You->>A: a question
    A->>D: kanna comment client.rs:244 "…"
    You->>A: thanks, close those
    A->>D: kanna close p86 …
```

## 2. Rules

- **A comment** is on some lines of a file. `kanna comment FILE:LINE "text"` opens the file in a code tab, as `kanna open code` does (with no placement, a tab on the task's Desk, or from a terminal in a plain window, a split beside it), or selects the code tab in that window that already shows it, and adds the comment there.
- **Only the newest comment shows.** Its lines are tinted, with a bar in the gutter, the rest of the file is dimmed, and the comment shows in a box just above its first line, with the lines it's on; the tab scrolls to it. A new comment in the same tab replaces it. **[Later, its own task]** ⌃− and ⌃⇧− go back and forward through the comments, across tabs, as in VS Code.
- **A line snaps to its scope.** `FILE:LINE` becomes the scope that opens on that line (a function, a type, a loop, a block), or else the smallest scope the line is in, from the viewer's tree-sitter scopes, so `client.rs:244` covers the loop it's in, 242–259. `FILE:FIRST-LAST` is kept as written, for a few lines inside a long function. A file without scopes takes the line alone.
- **Comments keep their lines.** A comment is snapped once, when it's made; if the file changes afterwards, it stays on the same line numbers.

- **Each tab has its own comment.** The tabs stay until closed: you close them when you're done, or ask the agent to (`kanna close`); a tab's comment goes with it.
- **They come back after a relaunch.**
- **Light and dark** follow the system.

![Light mode: the second comment on server.rs replaced the first](code-tour-light.png)

- **On a markdown file it's a thread.** `kanna comment DOC.md:LINE "text"` opens the doc with comments on, as `kanna markup DOC.md` does (or turns them on in the markdown tab that already shows it), and starts a [doc markup](doc-markup.md) thread there, written by the agent, on the block the line is in (a paragraph, a list item, a heading), and selects it. **[For now]** The markdown pane can't mark an image or a diagram, so a line on one puts the thread on the text just above it. The thread stays in the list beside the doc, like any other; your reply in it is sent to the agent, as a comment of yours would be. Good for explaining architecture from a doc with diagrams.

![The agent started c3 on the paragraph with the key idea; you can reply beside it](code-tour-md.png)

- **Problems:** a missing file fails with "No such file: …", as `open code` does; a line past the end fails with "… has N lines"; `FILE.md:FIRST-LAST` fails, since a doc's comment is on one block. Without Kanna.app the CLI starts it, as for any pane but a terminal.

## 3. CLI

| Command | Does |
|---|---|
| `kanna comment FILE:LINE[-LAST] TEXT... [placement]` | comments on those lines; placement options as `open`, for a file that isn't open yet. Answers `opened p87 · fdpass.rs:20–48`, without `opened` when it used a tab already open; on a markdown file, `c3 · how-it-works.md:12 in p6` |
| `kanna workspace list --all --json` | shows a code pane's comment (`path`, `first`, `last`, `text`) |
| `kanna raw '{"cmd":"debug.code","args":{"pane":"p86"}}'` | for tests: also the comment, and which lines are tinted and dimmed |

## 4. The skill

`kanna-explain` (in `.agents/kanna-skills`) tells the agent when and how to explain code so the user keeps a mental model of it:

- **When:** the user asks how something works, wants to understand code the agent wrote or changed, or seems to be losing track of the code.
- **The explanation is in the comments, not the chat.** A comment on the code (`kanna comment FILE:LINE`) says what that part does and the one thing to remember about it; the chat gets a line or two at most.
- **The big picture in a temporary markdown doc:** object hierarchies, class or layer stacks, call and data flows, as a diagram (mermaid class or sequence diagram, layer diagram, table) in a short doc in the worktree's `.kanna/tmp/`, commented on the same way (`kanna comment DOC.md:LINE`); the doc first, the code after.
- **One comment at a time.** Comment on one part, focus its pane, say where you are in a line, and stop: ask the user to tell you when to go on. Never comment on several parts in one turn.
- **Follow the questions.** Answer by commenting on the code that answers it.
- **Small parts.** A function or a block; `FILE:FIRST-LAST` for a few lines inside a long one.
- **At the end, offer to leave the tabs open or close them** (`kanna close`), and do what the user picks. The docs stay: Kanna keeps `.kanna/tmp/` out of git (the repo's `info/exclude`, beside `.kanna-worktrees/`), and they go with the worktree.

## 5. How it's built

Read top to bottom, as the [code viewer's](code-viewer.md#5-how-its-built) layers; only what's new is named.

```mermaid
flowchart TB
    subgraph CLIP["CLI processes"]
        CLI["kanna CLI, agents<br/>comment"]
    end
    subgraph APP["Kanna app process"]
        L3["3 · KannaCode<br/>comment box · tint and dim<br/>snap"]
        L3S["3 · kanna-syntax<br/>scopes (as now)"]
        L3 -->|"C call"| L3S
    end
    subgraph DAEMON["Kanna daemon process"]
        L4["4 · kanna-mux<br/>+ comment command<br/>comments on code records"]
    end
    CLI -->|"socket"| L4
    L3 <-->|"app link: snap, start"| L4
    classDef kanna fill:#2f6f4f,stroke:#6fcf97,color:#ffffff
    classDef kmux fill:#2f4f8f,stroke:#7fa7ff,color:#ffffff
    classDef client fill:#6b4f2f,stroke:#e0a960,color:#ffffff
    class L3,L3S kanna
    class L4 kmux
    class CLI client
```

| Piece | Where | What's new |
|---|---|---|
| CLI | `crates/kanna` | `comment`, completing source files |
| Markdown | `crates/kanna` (`threads.rs`) | `comment` on a `.md` file adds a thread, written by the agent, to the doc's thread file, quoting the block at that line, after opening the doc for comments as `markup` does (or turning them on in the tab that shows it); the pane picks the thread up as it does a reply, and selects a new one the agent started |
| Comment command | `crates/kanna-mux` (daemon) | `comment` finds or opens the file's code pane (the same-file rule), asks the app to snap, and puts the comment on the pane's record |
| Comment in the pane | `apps/kanna/Sources/KannaCode` | the comment box above its lines, tint and dim |
| Snap | `apps/kanna/Sources/Kanna` | `AppCommands.swift`: answers the daemon's `app.snap` (path, line, last) with the snapped lines, from `kanna-syntax`'s scopes; the daemon sends any `app.` command to the app, as it does `debug.` ones |
| Contents | `apps/kanna/Sources/Kanna/Panes` | passes a code pane its comment |

- **The comment lives on the code pane's record**, `comment: {path, first, last, text}`, so the saved layout keeps it and it goes when the pane closes. A pane shows it while it shows that file.
- **Snapping is the app's**, since tree-sitter is compiled into the app only (it costs about 18 MB, so the daemon doesn't link it too): the daemon asks the app over the app link, as it already asks it for the layout, and stores the lines it answers.
- **A new comment restarts the pane**, as `navigate` does, at the comment's first line.
- **The box** is a view laid into the text view's space above the comment's first line (as a gap in the line layout), so the code below it moves down and the line numbers stay with their lines. The box is placed from the laid-out gap whenever the layout manager finishes laying out past it (a highlight, a reload, a zoom, a resize). The code pane lays its text out contiguously: with non-contiguous layout the lines below the top are estimated, the gap is drawn where the estimate puts it, and a box placed from the full layout lands dozens of lines away.
- **Dimming** is drawn over the text view (the background colour, partly transparent, over the lines outside the comment), so the highlighting keeps its colours.
- **Checks:** `crates/kanna-daemon/tests/panes.rs` for `comment` (opening, reusing a tab, another file, a placement, the records, bad requests), with the test app answering `app.snap`; the CLI's parsing and `block_at` unit tests in `crates/kanna`; `CodeScopesTests` for snapping; e2e `code_tour`: comments on a scope, a range and another file, read back through `debug.code` (lines, text, the box in view), a missing file and a line past the end, two comments on a doc through `debug.md` (the thread, its mark, the agent, which is selected), and a relaunch; e2e `code_comment_box`: the box lies inside the gap above the first commented line, in a tab already open with sticky rows showing, after scrolling away and back, in a new split beside it, with a comment that wraps, and in dark mode.

## 6. Glossary

| Term | Meaning |
|---|---|
| **tour** | The agent explaining code while commenting on it |
| **`kanna-explain`** | The skill for explaining code: comments, markdown docs with diagrams |
| **comment** | A note the agent puts on some lines of a file, shown above them |
| **snapping** | Widening a comment's line to the scope it opens or sits in |
