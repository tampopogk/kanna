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
kanna comment .kanna/tmp/backlog.md:3 "The daemon owns the backlog; the app only shows it."
```

- **A diagram, not prose:** a mermaid `classDiagram` for a hierarchy, a `flowchart` for who calls whom, a `sequenceDiagram` for a flow, a `stateDiagram-v2` for states, a `gitGraph` for branches and merges, a layer diagram for a stack (the `layer-diagrams` skill), a table for a set of parallel things. A heading and the diagram; at most a line of text under it.
- **Only those diagram types draw in Kanna.** Any other (`erDiagram`, `gantt`, `pie`, …) shows as "Unsupported diagram type" and its source.
- **Big picture first,** then the code it points to.

## Not code: a doc in a tab

To explain something that isn't this repo's code (how `git subtree` works, a protocol, a tradeoff), write the doc in `.kanna/tmp/` the same way and open it in a tab:

```sh
kanna open md .kanna/tmp/git-subtree.md --tab --name explain
```

- **Diagrams first, then a short section per idea,** each with its command or example. A table where things compare.
- **Use the user's own case** for the examples (this repo's names, commits and commands), not generic ones.
- In the chat, a line saying the tab is open, then wait. Answer follow-ups by editing the doc (the tab reloads) or with `kanna comment` on it.

## One part at a time

1. Comment on one part (code or doc).
2. Focus the pane `kanna comment` printed: `kanna focus term-129`.
3. In the chat, one or two lines at most: where you are and "tell me when to go on". Don't repeat the comment, don't explain in the chat.
4. Stop and wait. Never comment on several parts in one turn.

When the user asks a question, answer it with a comment on the code or doc that answers it, and a line in the chat.

## At the end

Offer to leave the tabs open or close them, and do what the user picks (`kanna close PANE` for each tab `kanna comment` opened).
