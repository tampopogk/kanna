# Shelving at Once

**Status:** built · **Changes:** [tasks](tasks.md#8-how-its-built), [each app's Desk and Shelf](app-desk.md) · **Builds on:** [presentation layers](presentation-layers.md)

Clicking a stopped task in the backlog (Move to Shelf, `backlog shelve`) moves it to the Shelf **the moment it's clicked**, and its card can be pulled onto the Desk right away. Its agent and tabs start behind it. Before, the Shelf changed only once the daemon had restarted the agent and every tab terminal and run `git branch --show-current` on its main thread, so a stopped task with tabs took a noticeable while to appear.

## Contents

1. [Order of a shelve](#1-order-of-a-shelve)
2. [Rules](#2-rules)
3. [How it's built](#3-how-its-built)
4. [Tested](#4-tested)
5. [Not yet](#5-not-yet)

## 1. Order of a shelve

```mermaid
sequenceDiagram
    participant A as app
    participant D as daemon (main thread)
    participant G as git thread
    participant K as keeper
    A->>D: backlog shelve {task}
    D->>D: Shelf limit, claim for the app
    D->>D: off the backlog, on the Shelf (kanna.db)
    D-->>A: backlog and layout notices: the card is on the Shelf
    Note over A: the card is clickable, and a pulled pane shows a spinner
    D->>K: next turn: restart the agent, then its tabs
    K-->>D: running
    D-->>A: folder of the agent's new terminal, then its output
    D->>G: git branch --show-current
    G-->>D: branch
    D-->>A: reply {task, pane, folder, branch, app}
```

## 2. Rules

- **The Shelf changes first.** The daemon checks the limit, claims the task for the app, takes it off the backlog and records it on the Shelf, and publishes that, before it starts anything.
- **No starting state.** The card looks like any Shelf task's while its agent starts.
- **Pull it at once.** Pulling and parking are the app's, so a card can be pulled while its terminals start. A pane whose terminal hasn't started yet shows a spinner until it has.
- **A failed start stays on the Shelf.** The reply carries the error, and the Desk's note shows it (`noted` in `Desk.swift`), as it does for a refused shelve. Nothing goes back to the backlog.
- **The reply waits for the work:** `kanna task shelve` still answers once the agent and tabs run, with the agent's new terminal and the branch, so the CLI reports a failed start.
- **The daemon's main thread never runs git** for a shelve or a start: the branch comes from a thread, as the worktree does (`make_worktree`).

## 3. How it's built

- **`release`** (a task held in the backlog): after the claim, `started` takes it off the backlog, saves it on the Shelf and publishes it. The agent's restart (`revive`: the agent, then `core.wake` for its tabs) runs on the daemon loop's **next turn** (`Event::Turn`), so the layout's flush at the top of the loop sends the Shelf change before the restart's keeper round trips.
- **A fresh or moved task** (`launch`) opens its agent with `wait: false`, publishes, then looks up the branch only if `make_worktree` didn't already give it.
- **Branch lookups** go to a thread and come back as `Event::Branch`; the reply waits on them, the Shelf doesn't.
- **A restarted terminal's folder:** a restart gives the agent a new terminal ID ([IDs](presentation-layers.md#3-ids)), and the app files Shelf cards by their terminal's folder. The daemon sends the app the new ID's folder as the ID changes, not once the terminal has started, so the card is filed under its project at once.
- **The spinner:** a terminal pane the app has no started terminal for yet (`PaneView.waiting`) shows a small spinner, gone once its terminal attaches or the pane shows a message.
- **daemon.log lines have a time** (`2026-10-19 16:33:53.542 kanna-daemon: …`, local time to the millisecond): the daemon's `log!` stamps each line, and a panic's first line is stamped too. A shelve logs `TASK is on mac's Shelf, and its agent and tabs start`, then `TASK's agent and tabs started`.

## 4. Tested

- **Daemon** (`backlog.rs`, `a_stopped_task_is_on_the_shelf_before_its_agent_has_started_again`): a stopped task held in the backlog is shelved while its agent's terminal is kept starting; it's off the backlog, on the Shelf and claimed by the app, while the agent is still starting and the reply is still waiting, which comes once the agent runs.
- **e2e `shelve_at_once`:** a stopped task with two terminal tabs, clicked in the app's backlog, is on the app's Shelf within 200ms (about 50ms here). Its card, pulled at once, puts it on the Desk with its agent resuming and both tabs running, and no note. daemon.log's lines carry the time.
- With the mock agent, restarting an agent and two tabs takes about 8ms on a quiet machine, so the e2e also passes on the old order. The daemon test is what pins the order.

## 5. Not yet

- **Keeper spawns are synchronous:** every terminal start is a request to the keeper that the daemon's main thread waits on. It's normally a few milliseconds, and it now comes after the Shelf change. Making spawns asynchronous is its own task.
- **A task with no worktree yet** reaches the Shelf once `git worktree add` (on a thread) is done, since its agent needs its folder to start in.
- **Other crates' log lines** (`kanna-mux:`, `kanna-detect:`) aren't stamped: they print with `eprintln!` from their own crates.
