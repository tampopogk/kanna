---
name: kanna-tasks
description: Use when you've been told you're a task's agent in Kanna, as soon as you know what the work in this session is, and whenever the user plans work with you or you split off work for later.
---

# Kanna tasks

In Kanna, each agent's work is a **task**: one agent in one repo, shown on a Desk. Every task is made with a name (`kanna task new NAME`), and that name is what the user sees in the title bar, on the Shelf and in the backlog: keep it.

## One project, one Desk

A project (a repo) has **one Desk**: its window. The Desk shows one task at a time, its agent docked on the left and its tabs on the right; the project's other tasks wait on the **Shelf** along the bottom of the same window, still running. "The Desk" always means that window.

- **New tasks go in the backlog** (`kanna task new`). Running and place are separate: `kanna task start` runs a task where it is (a backlog task gets its agent and stays in the backlog, and any number can run there), `kanna task stop` ends its agent and leaves it where it is, and `kanna task shelve` moves a backlog task to the Shelf, starting it if it's stopped. Shelve one when the Shelf has room (three tasks per project, counting the Desk's) or when the user asks. When the user asks and the Shelf is full, make room by moving a Shelf task back to the backlog (`kanna task defer TASK`; it stays as it is, running or stopped). The user brings a Shelf task onto the Desk when they want it.
- `kanna task pull TASK` swaps a task onto the Desk, parking the one there; `kanna task park` moves the Desk's task to the Shelf. Only when the user asks.
- Never make another Desk for a project (`--window new`): there's one.

If you've been told you're the Kanna planning agent, you're a window's **planning agent**, not a task: use the `kanna-planning` skill instead, and skip naming a branch.

## Name your branch

A task works in a worktree of its own named after its ID (`.kanna-worktrees/task-3f9a1c2e`), on a branch of that name (`task-3f9a1c2e`). In a new project the branch has no commits yet: your first commit is the project's first. Once you know the work, rename the branch, from the worktree:

```sh
git branch -m fix-login-redirect
```

- **Short kebab-case,** what the work is; follow the repo's branch prefixes if it has any.
- **Only the branch:** leave the worktree's folder where it is, since you and your terminals are in it.
- **Before you push,** so the remote branch has the real name.

## Add tasks to the backlog

When you and the user plan work, or you find work that belongs in its own task, add it to this repo's backlog rather than doing it now:

```sh
kanna task new "Add rate limiting" "Add rate limiting to the login endpoint: ..."
```

- **The first message stands alone.** The agent that gets it starts fresh in its own worktree and hasn't seen this conversation: say what to do, why, where (files, functions), and how to know it's done.
- **Its first message can't change.** Only a backlog task's name can (`kanna task name TASK NAME`). To change what it says, make a new task and take the old one off with `kanna task done TASK`.
- **The agent is the user's default** (`tasks.agent`, `claude` unless set). When the user asks for another one for a task ("give that to codex"), add `--agent codex` (or `claude`, `copilot`, `opencode`, `agy`).
- **It starts by itself.** Backlog tasks run unattended, a few at a time (`backlog.runners`, 3 by default), oldest first, so write a first message an agent can do in one go: settle the open decisions with the user before you make it, name the files and how to test it, and say what to do when stuck. When it needs the user, it asks and ends its turn: Kanna stops it there, and the user resumes it on the Shelf.
- **Say what it waits for.** If it can't start until other tasks are done (it builds on their merged work), add `--after TASK,...` (IDs from `kanna task list`, like `task-3f9a`; yours too: your pane works). It's **blocked**: it can't start until those tasks are done, and its backlog row shows it so. It starts from main, so it gets their work only once they're merged and done.
- **Call a task by its name, never its ID:** Kanna shows no task or pane IDs (`task-1dfb`, `p248`), so tell the user "Add rate limiting" waits for "Fix login redirect", not for task-1dfb. IDs are only for commands (`--after task-3f9a`), and a command you show may keep them. When two tasks' names are alike, say where each one is (the Desk, the Shelf, the backlog).
- **Start or shelve one only when the user asks, or shelve when there's room.** `kanna task shelve` puts it on the Shelf if the project has fewer than three tasks out (the Desk's and the Shelf's); otherwise leave it in the backlog, unless the user asks, and then make room by deferring a Shelf task they're not using. `kanna task start` runs it in the backlog instead, with no limit.

## Show it in a tab

When the user asks you to show them something (a diff, a file, a doc, a page, a log), open it in a new tab of your own window, not in the chat:

| To show | Run |
|---|---|
| Your branch's changes | `kanna open diff --merge-base main --tab` |
| A command's output | `kanna open --tab --name NAME --cmd 'CMD \| less -R'` |
| A markdown doc | `kanna open md FILE --tab` |
| A source file | `kanna open code FILE[:LINE] --tab` |
| A page | `kanna open web URL --tab` |

- **Run it from your own terminal, without `--window`:** the tab opens in your task's workspace, even while it's hidden in the backlog or on the Shelf, and the reply names your task and where it is (`opened term-5 in ws-3 (task-1dfb, on the Shelf), tab-9: running`). `--window new` opens a window of your task's, shown and hidden with it.
- **Don't put a mermaid diagram in the chat when the chat is a terminal:** it shows there as source. A diagram, or an explanation longer than a few lines, goes in a markdown doc in `.kanna/tmp/` that you open in a tab, as the `kanna-explain` skill says.
- **A tab, not a split or a new window,** so your pane keeps its size.
- In the chat, a line saying what the tab shows.

## Message the planner or another task

`kanna task send TASK TEXT` leaves a message for another task's agent, and `kanna task send planner TEXT` for this repo's planning agent. Kanna hands it to their agent once their turn is over, as a prompt `[message from YOUR-TASK "Name"] TEXT`, so it never lands mid-turn, and a stopped backlog task runs for it. Only Claude and Codex agents take messages: for any other, `kanna task send` warns and the message waits.

- **Ask another task directly** when you need something only it knows (is its spec final, which branch has the change), instead of asking the user to relay it. Find it with `kanna task list QUERY`.
- **Tell the planner** when you're blocked on a decision it should take to the user, or when you're done and there's something to merge.
- **A message you get** starts with `[message from SENDER]`: answer with `kanna task send SENDER "..."` (or `planner`), not in your chat, which the sender can't see. A message from `user` came from the user: answer in the chat as usual.
- **Keep it to what they need,** in one message: they read it in the middle of their own work.

## Present your work

When the work is done, before you end your turn, set up your window to show the user what you changed and why. You may have run unattended in the backlog: whatever you leave open, focused and zoomed is what the user sees when they pull the task, maybe hours later. Kanna keeps it through the stop.

1. **Open what explains the change best,** as tabs ([Show it in a tab](#show-it-in-a-tab)): the spec you changed (`kanna open md`), the mockup (`kanna open web`), comments on the key code (`kanna comment`, as the `kanna-explain` skill says), a diagram in a doc in `.kanna/tmp/`, the diff (`kanna open diff --merge-base main`: everything the branch changed, committed or not, following the worktree; not a terminal, so it outlasts the stop).
2. **Close tabs that don't help** (`kanna close PANE`): scratch shells, finished builds.
3. **Then focus the tab the user should see first** (`kanna focus PANE`), and zoom it if one pane says it all (`kanna zoom PANE`). Do it last: otherwise the tab you opened last is what shows when the task comes onto the Desk. It works the same from the backlog or the Shelf.
4. **Check it with `kanna workspace list`:** your workspace's tabs and panes, the shown tab, what's focused and zoomed, as the user will see it.
5. **End your turn with a short summary in the chat:** what changed, why, what's in the tabs, and what's left for the user (merge, a decision, a check).

**Nothing worth a tab** (a small fix, a question in words)? Close your tabs and zoom your own pane (`kanna zoom $KANNA_TERMINAL`), so your summary is what the user sees.

If you stop to ask the user something, set up what the question is about the same way first. Terminals you leave running are stopped with you and run again when the task starts; what they were in the middle of is lost.

## Commands

| Command | Does |
|---|---|
| `kanna task name [TASK] NAME` | Rename a task, running or waiting in the backlog (only when the user asks) |
| `kanna task new NAME [FIRST-MESSAGE] [--agent CMD] [--after TASK,...]` | Add a task to this repo's backlog; `--agent`: run this agent (`codex`, …) instead of `tasks.agent`; `--after`: it can't start until those tasks are done |
| `kanna task edit TASK --after TASK,...\|none` | Change what a backlog task waits for (only when the user asks to unblock it) |
| `kanna task list [QUERY]` | Every task: ID, name, where (a Desk, the Shelf, the backlog), state (working, idle, needs you, stopped, or blocked, with what it waits for), agent, repo; QUERY keeps those whose name or first message has every word |
| `kanna task start TASK` | Run a task where it is: a backlog task gets its worktree, branch and agent with the first message, and stays in the backlog; a stopped task resumes its conversation |
| `kanna task stop [TASK]` | End a task's agent and hang up its terminals, leaving it where it is with its tabs as they are (only when the user asks) |
| `kanna task shelve TASK` | Move a backlog task to the Shelf, starting it if it's stopped (three per project) |
| `kanna task pull TASK` | Bring a task onto the Desk, parking the one there (only when the user asks) |
| `kanna task park [TASK]` | Move the Desk's task to the Shelf (only when the user asks) |
| `kanna task defer [TASK]` | Move a task back to the backlog as it is, running or stopped (only when the user asks) |
| `kanna task send TASK\|planner TEXT` | Leave a message for another task's agent or this repo's planning agent; it gets it when its turn is over |
| `kanna task messages [TASK]` | A task's messages, sent and received, and whether each was delivered |
| `kanna task done [TASK]` | End a task, or take it off the backlog; says which blocked tasks can start now |

Run them from your own terminal, in the repo: a new task belongs to the repo you're in.
