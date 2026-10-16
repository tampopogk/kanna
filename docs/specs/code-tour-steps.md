# Code Tour Steps

**Status:** decided · **Mockup:** [`references/mockups/code-tour-steps.html`](../../references/mockups/code-tour-steps.html) · **Builds on:** the [code tour](code-tour.md)

The agent queues a whole tour at once, over code and markdown, and you step through it at your own pace: each comment has **Back**, where you are (**2 of 5**) and **Next**. Each step is sent to the agent, so it knows where you are and can add a line; a question in the middle becomes a stop of its own, right after the one you're on.

![Stop 2 of 5: Next selects server.rs and scrolls to the comment; the step reached the agent](code-tour-steps.png)

## Contents

1. [The loop](#1-the-loop)
2. [Rules](#2-rules)
3. [CLI](#3-cli)
4. [The skill](#4-the-skill)
5. [How it's built](#5-how-its-built)

## 1. The loop

```mermaid
sequenceDiagram
    participant You
    participant A as agent
    participant D as daemon
    participant App as Kanna.app
    You->>A: how does the keeper hand a terminal to the daemon?
    A->>D: kanna tour doc.md:5 "…" server.rs:92 "…" … (5 stops)
    D->>App: stop 1: the doc, its thread c1 with 1 of 5
    You->>App: Next
    App->>D: tour next
    D->>App: stop 2: server.rs, its comment with 2 of 5
    D->>A: [kanna tour 2/5 · server.rs:92–103]
    A->>You: a line, or nothing
    You->>A: a question
    A->>D: kanna tour add --next client.rs:244 "…"
    D->>App: stop 4 of 6, shown now
    You->>App: Next … Done
    D->>A: [kanna tour done · 6 stops]
```

## 2. Rules

- **A tour is a list of stops,** each a comment as `kanna comment` makes it: on a source file, lines snapped to their scope; on a markdown file, a thread by the agent on the block at the line. Every stop is checked when it's queued (a missing file, a line past the end), and nothing is queued if one fails.
- **One tour per agent.** A new `kanna tour` replaces the agent's last one. The tour belongs to the agent's pane: it comes back after a relaunch and goes when that pane closes.
- **Only the current stop shows.** Showing a stop does what `kanna comment` does: it opens the file's tab or selects the one that shows it, and scrolls to the comment. A tab opens when the tour reaches it, not when it's queued.

![The tour was just queued: stop 1 is the agent's thread on the doc, with Next](code-tour-steps-queued.png)

- **Back, N of M, Next** sit along the bottom of the comment box, or of the thread's card on a markdown stop. Back is off on the first stop; on the last, Next says **Done**.
- **Each step goes to the agent,** as markup comments do, sent and submitted to its terminal: `[kanna tour 2/5 · crates/kanna-keeper/src/server.rs:92–103]` (the path from the agent's folder). Done sends `[kanna tour done · 5 stops]`. The agent answers in a line at most, or not at all: the comment is the explanation.
- **A question in the middle** is answered with a stop after the current one (`kanna tour add --next`), shown at once; the count grows (4 of 6), Next carries on with the tour, and Back returns to where you were.

![You asked at stop 3; the answer is stop 4 of 6, and Next goes on to fdpass.rs](code-tour-steps-question.png)

- **Done ends the tour.** The last stop's comment stays, without its Back and Next; the tabs stay until closed, as any comment's.
- **A closed tab comes back.** If you close a stop's tab, Back or Next to that stop opens it again.
- **A markdown stop's thread is written when it's reached,** and stays in the doc's thread list afterwards, like any thread. Going back to it selects the same thread.
- **Light and dark** follow the system.

![Light mode](code-tour-steps-light.png)

## 3. CLI

| Command | Does |
|---|---|
| `kanna tour FILE:LINE[-LAST] TEXT [FILE:LINE TEXT …]` | a new tour, replacing yours; shows stop 1. Answers `tour of 5 · 1 of 5 · p86 · server.rs:92–103` |
| `kanna tour add [--next] FILE:LINE TEXT …` | adds stops at the end, or with `--next` right after the current one and shows the first of them |
| `kanna tour go next\|back\|N` | shows that stop, as Next and Back do (the app's buttons call it), and tells the agent |
| `kanna tour` | lists the stops, marking the current one |
| `kanna tour end` | ends the tour, as Done does, without telling the agent |

Each TEXT is one argument (quote it). Placement options are `comment`'s, for files that aren't open yet. `kanna list` shows a code pane's comment with `tour: {at, of}` while it's the current stop; `debug.code` and `debug.md` report the footer.

## 4. The skill

`kanna-explain` queues the tour instead of commenting one part at a time:

- **Plan the route, then queue it in one `kanna tour`:** the big-picture doc first, then the code, small parts each.
- **In the chat, a line:** how many stops, and "Next on the comment; ask here any time". Then stop.
- **On `[kanna tour N/M · …]`,** add a line only if there's something the comment couldn't say; otherwise say nothing more than a word.
- **On a question,** answer with `kanna tour add --next` on the code that answers it, and a line.
- **On `[kanna tour done …]`,** offer to leave the tabs or close them.

## 5. How it's built

| Piece | Where | What's new |
|---|---|---|
| CLI | `crates/kanna` | `tour`, its pairs of `FILE:LINE TEXT`; markdown stops go to the daemon too |
| Thread files | `crates/kanna-threads` (new) | the doc thread file (where it lives, reading and writing it safely, `block_at`, adding the agent's thread), moved out of the CLI's `threads.rs` so the daemon writes a markdown stop's thread with the same code |
| Tour | `crates/kanna-mux` (`tour.rs`) | `tour` keeps `{stops, at}` on the agent's pane record; showing a stop reuses `comment` for code and `markup` plus a written thread for markdown; each step `send`s the agent its line |
| Code pane | `apps/kanna/Sources/KannaCode` | the comment box's footer: Back, N of M, Next or Done, calling `tour go` |
| Markdown pane | `apps/kanna/Sources/KannaMarkdown` | the same footer on the card of the thread the pane's record names as the tour's |
| Contents | `apps/kanna/Sources/Kanna/Panes` | passes each pane its stop's `tour: {owner, at, of}` and sends `tour go` for the buttons |

- **The stop's place is on the record of the pane that shows it:** a code pane's `comment` gets `tour: {owner, at, of}`; a markdown pane gets `tour: {owner, thread, at, of}`. Only the current stop has it, so moving on clears the last one's.
- **Checks:** `crates/kanna-daemon/tests/panes.rs` for `tour` (queueing, a bad stop refusing the lot, next, back, add --next, done, the message to the agent, a relaunch, the agent's pane closing); `crates/kanna` unit tests for parsing the pairs; e2e `code_tour_steps`: a tour of a doc and two code files, Next and Back pressed through `debug.code`/`debug.md`, which tab is selected and the footer's N of M, the agent's terminal reading each `[kanna tour …]` line, `add --next`, Done.
