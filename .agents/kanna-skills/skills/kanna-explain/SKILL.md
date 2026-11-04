---
name: kanna-explain
description: Use when you've been told you're in Kanna (a task's agent or the Kanna planning agent) when the user asks how some code works, wants to understand code you wrote or changed, or seems to be losing track of the code; and whenever the user asks you to explain or teach anything (a tool, a git workflow, a concept, a design), since a terminal chat doesn't render diagrams.
---

# Kanna explain

Explain code where it is, not in the chat: the explanation goes in comments on the code and in diagrams, and the chat stays a line or two.

When the chat is a terminal, mermaid shows there as source, and long markdown is a wall of text. Anything you'd explain with a diagram or more than a few lines goes in a markdown doc you open in a tab.

## Code: comment on it

```sh
kanna comment src/fdpass.rs:50 "Waits for a message and takes the terminals that came with it."
```

The file opens in a code tab beside you, those lines tinted and the rest dimmed, the comment just above them. `FILE:LINE` takes the whole function or block at that line; `FILE:FIRST-LAST` takes just those lines. A new comment in the same file replaces the last one.

- **The comment is the explanation.** Short and plain: what this part does and the one thing to remember about it.
- **Small parts.** A function, a match, a loop; not a whole file.

## The big picture: a temporary markdown doc

For how parts fit together (object hierarchies, class or layer stacks, who calls whom, how data flows), write a short markdown doc with a diagram in `.kanna/tmp/` in your worktree and comment on it. Kanna keeps that folder out of git, and it goes when the worktree does: leave the docs there.

```sh
kanna comment .kanna/tmp/shelf.md:3 "The daemon owns the Shelf; the app only shows it."
```

- **A diagram, not prose:** a mermaid `classDiagram` for a hierarchy, a `flowchart` for who calls whom, a `sequenceDiagram` for a flow, a `stateDiagram-v2` for states, a `gitGraph` for branches and merges, a layer diagram for a stack (the `layer-diagrams` skill), a table for a set of parallel things. A heading and the diagram; at most a line of text under it.
- **Only those diagram types draw in Kanna.** Any other (`erDiagram`, `gantt`, `pie`, …) shows as "Unsupported diagram type" and its source.
- **In a `sequenceDiagram`, no `;` or `#` in text:** mermaid reads `;` as the end of a statement (the diagram fails) and `#` as the start of a comment (the rest of the line goes). Write them as `#59;` and `#35;`, or reword.
- **Check it draws before you present it:** `kanna open md` prints `FILE:LINE: message` for each diagram that won't draw (a parse error, a type Kanna doesn't draw), and `kanna diagrams check FILE` checks without opening. Fix every one.
- **Then look at it:** take `kanna screenshot PANE` of the markdown pane, read the PNG, and fix labels that overlap or are cut off.
- **Big picture first,** then the code it points to.

## A change: its call tree first

To walk the user through a change (yours, or a diff) that crosses more than one function or layer (CLI, daemon, app…), first write `.kanna/tmp/<topic>.md` with a ` ```calltree ` block: the calls the change runs through, in the process each runs in, each with its `file:line`. Kanna draws it as a call tree, a lane per process, each `file:line` opening the code. Open it, comment on it, then go through the code one part at a time with `kanna comment`, in call order. A change in one function goes straight to `kanna comment`.

````markdown
# Rename a Shelf task

```calltree
kanna CLI
main          crates/kanna/src/lib.rs:14                kanna task name t12 "New name"
  parse       crates/kanna/src/commands/task.rs:143     task name TASK NAME → request
  wire        crates/kanna/src/commands/task.rs:260     task name with a TASK → tasks
    -> socket {"cmd":"tasks","action":"name"}
kanna-daemon
    Tasks::name    crates/kanna-daemon/src/tasks.rs:1043  finds the task on the Shelf
      rename_pane  crates/kanna-mux/src/core.rs:1371      its held agent's pane
      store        crates/kanna-daemon/src/tasks.rs:1067  on the reply: the new name
```
````

- **A line with no `PATH:LINE` names a process;** the calls under it are in its lane. **A call** is `NAME  PATH:LINE  NOTE`, fields split by two spaces, its depth its indent (two spaces a level), its path repo-relative. **`-> LABEL`** is a hop into the next process.
- **A call stack is one call a level:** a backtrace goes in the same way.
- **`kanna open md` checks it** like a diagram: a path or line that doesn't exist, an indent that jumps. Fix what it prints.
- **Comment on a row** with `kanna comment DOC:LINE` at that call's line.

## Not code: a doc in a tab

To explain something that isn't this repo's code (how `git subtree` works, a protocol, a tradeoff), write the doc in `.kanna/tmp/` the same way and open it in a tab:

```sh
kanna open md .kanna/tmp/git-subtree.md --tab
```

- **Diagrams first, then a short section per idea,** each with its command or example. A table where things compare.
- **Use the user's own case** for the examples (this repo's names, commits and commands), not generic ones.
- In the chat, a line saying the tab is open, then wait. Answer follow-ups by editing the doc (the tab reloads) or with `kanna comment` on it.

## A tour: queue the stops, the user steps through

Plan the route, then queue it in one command: the big-picture doc first, then the code, a small part per stop. Each stop is `FILE:LINE` (or `FILE:FIRST-LAST`) and its comment, quoted as one argument.

```sh
kanna tour .kanna/tmp/keeper-fds.md:5 "The whole trip: queue, send, take." \
  src/server.rs:92 "Queues the message with the terminals that go with it." \
  src/fdpass.rs:20 "Sends them across in one message." \
  src/fdpass.rs:50 "The daemon's side: takes whatever terminals came with it."
```

Kanna shows the first stop. Each comment has Back, where the user is (2 of 4) and Next, which opens or selects the next stop's file and scrolls to it. Nothing reaches you as they step: the comments are the explanation, so there's no round trip between stops.

1. In the chat, one line: how many stops, and "Next on the comment; ask here any time". Then stop.
2. More stops come to mind while they read? `kanna tour add FILE:LINE "…"` adds them at the end.
3. **A question during the tour:** run `kanna tour` to see which stop they're on, then answer with a stop right after it, shown at once, and a line in the chat. Next carries on with the tour.

   ```sh
   kanna tour add --next src/client.rs:244 "Each line says how many terminals are its, and takes that many."
   ```

A question outside a tour, or a single part, still gets a plain `kanna comment`.

## At the end

When the user says they're done (Done on the last stop ends the tour without telling you), offer to leave the tabs open or close them, and do what they pick (`kanna close PANE` for each tab the tour or `kanna comment` opened).
