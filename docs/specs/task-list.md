# Task list

**Status:** built · **Builds on:** [Desk](desk.md#5-cli), [tasks](tasks.md#7-cli), [blocked tasks](task-dependencies.md), [agent messaging](agent-messaging.md), [task scripts](task-scripts.md) · **Code:** `crates/kanna/src/output.rs` (the text), `crates/kanna-daemon/src/tasks.rs` (`merged`, `replied`)

`kanna task list` is how the planning agent, and you, see what's on the Desk and what waits on the Shelf. It reads the Desk first, then the Shelf, each under its own heading, with one short row per task, so the Desk's state reads at a glance. Its STATE says when a task's branch is already in main (`merged`) and when the task has said something to the planner that the planner hasn't had yet (`replied`), so a finished task never reads as one still to merge.

## Contents

1. [The output](#1-the-output)
2. [Rows](#2-rows)
3. [STATE](#3-state)
4. [Which tasks](#4-which-tasks)
5. [--json](#5---json)
6. [How it's built](#6-how-its-built)
7. [Rejected](#7-rejected)

## 1. The output

Run in the kanna repo, with Codex as one Shelf task's agent and every other task on the default:

```text
$ kanna task list
Desk
task-ffb6  Fix the flaky keeper take-back restart test  idle     just now
task-d1dc  Ink 1: threads with marks                    working  2m ago
task-66b7  Crash capture and restart                    merged   3h ago

Shelf
task-7804  Reload changed images in markdown            stopped
task-9387  Agent permission prompts setting             replied
task-2d07  Shelf history                                stopped  codex
task-3300  Desks are windows to ⌘` and the CLI          blocked  waits for "Next Window skips minimized windows"
task-aa69  Cloud 12: Build jobs and iOS from remote     blocked  waits for "Cloud 7: xcodebuild redirect to a build Mac"
```

With `--all`, every repo's tasks, and a row of another repo names it:

```text
$ kanna task list --all
Desk
task-ffb6  Fix the flaky keeper take-back restart test  idle     just now

Shelf
task-7804  Reload changed images in markdown            stopped
task-151a  send-key                                     stopped  ~/work/kmux
```

An empty Desk says so under its heading (`(empty)`); with a QUERY, a heading with no match is left out, and nothing matching says `no task matches "QUERY"`.

## 2. Rows

- **Columns:** the task's ID (shortened, [task IDs](task-ids.md)), its name, its STATE, then a tail of what else matters, in this order: its agent, its repo, what it waits for or when it was last active.
- **No column headings, no WHERE column:** the Desk and Shelf headings say where.
- **Agent only when it isn't the default:** a row whose agent isn't `tasks.agent` ([choosing the agent](tasks.md#6-choosing-the-agent)) names it (`codex`); a row on the default says nothing.
- **Repo only when it isn't the caller's:** never on the default list; with `--all`, a row of another repo shows its folder (`~/work/kmux`).
- **What it waits for, by name:** a blocked row says `waits for "NAME"`, several separated by commas, each by its ID instead when it has no name.
- **When it was last active:** Desk rows only (`2m ago`). A Shelf row doesn't say when it was made.
- **Order:** as before, the Desk's tasks oldest first, the Shelf's in its own order.

## 3. STATE

```mermaid
flowchart TD
    A{setting up, tearing down or a script failed?} -- yes --> S[that, as now]
    A -- no --> B{running and working or needs you?}
    B -- yes --> W[working / needs you]
    B -- no --> M{branch in main, with commits of its own?}
    M -- yes --> MERGED[merged]
    M -- no --> R{a message to the planner not yet handed over?}
    R -- yes --> REPLIED[replied]
    R -- no --> O[blocked / stopped / idle / …]
```

- **`merged`:** the task's worktree has moved past the commit it started from (`kanna-base`, [task scripts](task-scripts.md)), and that commit is in the repo's main checkout's `HEAD` (`git merge-base --is-ancestor`), so the branch has nothing main lacks. A worktree too old to have `kanna-base` counts as moved when its `HEAD` reflog has a commit. A task with no worktree yet, or no commit, is never `merged`. Uncommitted changes don't count: what the planner needs is whether the work is in main.
- **`replied`:** the task sent the planner a message ([agent messaging](agent-messaging.md)) that hasn't been handed to it yet. The planner gets it at the end of its turn; until then the list says the task has something to say.
- **Working and needs you win:** a merged task that's still working says `working`, so it's never read as finished too early.

## 4. Which tasks

- **The caller's repo by default:** the repo the CLI runs in (its git common dir, as `kanna task new` finds it). Outside a git repo, every repo's.
- **`--all`:** every repo's, as the list was before.
- **QUERY** filters as before ([Shelf search](shelf-search.md#cli)).

## 5. --json

The same object as before, filtered the same way (the caller's repo unless `--all`), with new fields:

| Field | On | Is |
|---|---|---|
| `merged` | each Shelf task and Desk task | `true` when STATE says `merged` |
| `replied` | each Shelf task and Desk task | `true` when a message to the planner waits |
| `agent` | the reply | the default agent (`tasks.agent`) of the caller's repo |
| `repo` | each Desk task | its repo, as Shelf tasks have |

## 6. How it's built

```mermaid
sequenceDiagram
    participant CLI as kanna task list
    participant App as app (Desk rows)
    participant D as daemon tasks
    CLI->>App: task list
    App-->>CLI: Desk panes (name, status, leftAt)
    CLI->>D: tasks list {repo, merged: true}
    D->>D: git per worktree, on a thread
    D-->>CLI: shelf, desk (repo, merged, replied), agent
    CLI->>CLI: name Desk panes, keep the caller's repo, print
```

- **The daemon works out `merged`,** since it knows each task's worktree and runs where the repo is, also for a task on another machine. It does it only when asked (`merged: true`: the CLI's other `tasks list` calls, for IDs, don't), on a thread of its own, one thread per worktree, so the list isn't held up by git.
- **`replied`** is read from the messages not yet delivered whose recipient is `planner`.

**Tested** by `output.rs`'s `list_tests` (the layout, the agent and repo only where they differ, waits by name), `task_scripts.rs`'s `a_worktree_is_merged_once_its_own_commits_are_in_main`, `tests/cli.rs` (the caller's repo, `--all`, QUERY), and e2e `task_shelf` (a Desk task reads `replied` while its message to the planner waits, stays so after an unmerged commit, then `merged` once main merges it, and `--json` says `merged`).

## 7. Rejected

- **A `merged` check in the CLI:** the CLI may talk to another machine's Kanna, whose worktrees it can't see.
- **`merged` for any branch that's an ancestor of main:** a task that has started but not committed is one too, and would read as finished.
- **Column headings and a WHERE column under the section headings:** they repeated what the headings say.
