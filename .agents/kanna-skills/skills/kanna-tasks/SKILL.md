---
name: kanna-tasks
description: Use when you've been told you're a task's agent in Kanna, as soon as you know what the work in this session is, and whenever the user plans work with you or you split off work for later.
---

# Kanna tasks

In Kanna, each agent's work is a **task**: one agent in one repo, shown on a Desk. Every task is made with a name (`kanna task new NAME`), and that name is what the user sees in the title bar, on the Desk and on the Shelf: keep it.

## One project, one Desk

A project (a repo) has **one Desk**: its window, which holds up to three of the project's tasks, one of them **active**. The active task shows with its agent docked on the left and its tabs on the right; the Desk's other tasks wait in the strip along the bottom of the same window, still running. ⌘. toggles between the active task and the **Desk view**: the planning agent, the Desk's tasks, and the **Shelf**, where the project's other tasks wait.

- **New tasks go on the Shelf** (`kanna task new`). Running and place are separate: `kanna task start` runs a task where it is (a Shelf task gets its agent and stays on the Shelf, and any number can run there), `kanna task stop` ends its agent and leaves it where it is, and `kanna task pick` moves a Shelf task to the Desk, starting it if it's stopped. Pick one when the Desk has room (three tasks per project) or when the user asks. When the user asks and the Desk is full, make room by moving a Desk task back to the Shelf (`kanna task shelve TASK`; it stays as it is, running or stopped). The user makes a Desk task active when they want it.
- `kanna task activate TASK` makes a Desk task the active one; `kanna task park` shows the Desk view (the task stays active). Only when the user asks.
- Never make another Desk for a project (`--window new`): there's one.
- **The Desk, Shelf and focus are shared:** the user may change them at the same time as you, so don't assume a change was yours, and don't undo one you didn't make; ask.
- **Don't search the whole home folder** (`find ~`, `ls ~/*`, `grep -r ~`): macOS asks the user, in Kanna's name, once for every protected folder it walks into (Desktop, Documents, Downloads, Photos, Music, other apps' data). Look where the project or the user points, or ask where something is.

If you've been told you're the Kanna planning agent, you're a window's **planning agent**, not a task: use the `kanna-planning` skill instead, and skip naming a branch.

## Name your branch

A task works in a worktree of its own named after its ID (`.kanna-worktrees/task-3f9a1c2e`), on a branch of that name (`task-3f9a1c2e`). In a new project the branch has no commits yet: your first commit is the project's first. Once you know the work, rename the branch, from the worktree:

```sh
git branch -m fix-login-redirect
```

- **Short kebab-case,** what the work is; follow the repo's branch prefixes if it has any.
- **Only the branch:** leave the worktree's folder where it is, since you and your terminals are in it.
- **Before you push,** so the remote branch has the real name.
- **Other worktrees you make** (to check main, say) go in `.worktrees/` with your task's ID in the name (`.worktrees/3f9a1c2e-main-check`): builds are kept by the worktree folder's name, so a name another task also picks shares its builds.

## Add tasks to the Shelf

When you and the user plan work, or you find work that belongs in its own task, add it to this repo's Shelf rather than doing it now:

```sh
kanna task new "Add rate limiting" "Add rate limiting to the login endpoint: ..."
```

- **Search first.** Before you make a task, look for one that already covers the work: `kanna task list WORDS` with two or three words for the work, then again in other words ("send enter", then "submit codex"), and `kanna task list WORDS --include-done` to see whether it was done already. If a task covers it, tell the user and add to that one instead of making another: tell a running one with `kanna task send TASK "..."`; a Shelf task that hasn't started can't have its first message edited, so make a new one with both and end the old one (`kanna task done TASK`). If one overlaps, say so in both, or make the new one wait for it (`--after`).
- **The first message stands alone.** The agent that gets it starts fresh in its own worktree and hasn't seen this conversation: say what to do, why, where (files, functions), and how to know it's done.
- **Its first message can't change.** Only a Shelf task's name can (`kanna task name TASK NAME`). To change what it says, make a new task and take the old one off with `kanna task done TASK`.
- **The agent is the user's default** (`tasks.agent`, `claude` unless set). When the user asks for another one for a task ("give that to codex"), add `--agent codex` (or `claude`, `opencode`, `agy`).
- **It starts by itself.** Shelf tasks run by themselves, unattended, a few at a time (`shelf.runners`, 3 by default), oldest first, so write a first message an agent can do in one go: settle the open decisions with the user before you make it, name the files and how to test it, and say what to do when stuck. When it needs the user, it asks and ends its turn: Kanna stops it there, and the user resumes it on the Desk.
- **Say what it waits for.** If it builds on other tasks' work, add `--after TASK,...` (IDs from `kanna task list`, like `task-3f9a`; yours too: your pane works). It's **blocked** until each of them is ready or done, and its Shelf row shows it. Once they're ready, it starts on their branches (stacked); see [Ready and stale](#ready-and-stale).
- **Call a task by its name, never its ID:** Kanna shows no task or pane IDs (`task-1dfb`, `term-5`), so tell the user "Add rate limiting" waits for "Fix login redirect", not for task-1dfb. IDs are only for commands (`--after task-3f9a`), and a command you show may keep them. When two tasks' names are alike, say where each one is (the Desk, the Shelf).
- **Start or pick one only when the user asks, or pick when there's room.** `kanna task pick` moves it to the Desk if the project has fewer than three tasks there; otherwise leave it on the Shelf, unless the user asks, and then make room by moving a Desk task they're not using to the Shelf (`kanna task shelve`). `kanna task start` runs it on the Shelf instead, with no limit.

## Ready and stale

- **Mark your work ready** with `kanna task ready set` once it's finished, committed, and its tests pass: tasks waiting for yours can then start on your branch. Don't mark it before: they build on what's there. Uncommitted changes are refused.
- **Your branch moving un-readies it:** a new commit, a merge, a rebase. Every task built on yours then goes stale, so mark it ready again when you're done.
- **When you're stale,** you get a message from `kanna` saying which task you build on moved. Merge its branch into yours (the message names it), rerun your tests, commit, then `kanna task ready set`. If the message says what you build on is stale itself, wait: you get another once it's ready again. `kanna task ready set` is refused until you've merged what moved.
- **Don't merge into main while `kanna task list` says your task `merges after` another:** the work you build on isn't in main yet. When it lands, you get a message to merge main, and you're an ordinary task again.

## Events

Kanna tells you when something your work depends on changes, with a message from `kanna` at the end of your turn. Every task hears these from the start:

| Event | You hear |
|---|---|
| `main-conflicts` | main moved and now conflicts with your branch: merge or rebase main, rerun your tests, then carry on with what you were doing. You aren't told again about the same files until you've merged |
| `base-merged` | a stacked task's base landed in main (see [Ready and stale](#ready-and-stale)) |
| `base-moved` | a stacked task's base moved, or is ready again |

- **Subscribe to `main-moved`** (`kanna task subscribe main-moved`) when you need every move of main, conflict or not: you're building on code others are changing right now, or the user asks you to keep up with main. Each message lists the commits your branch hasn't merged. Unsubscribe when that part of the work is over.
- **Leave the others on** unless the user asks: without `base-moved` you still go stale, you just aren't told.
- **Check with `kanna task events`:** what you hear, and what you were sent.

## Show it in a tab

When the user asks you to show them something (a diff, a file, a doc, a page, a log), open it in a new tab of your own window, not in the chat:

| To show | Run |
|---|---|
| Your branch's changes | `kanna open diff --merge-base main --tab` |
| A command's output | `kanna open --tab --cmd 'CMD \| less -R'` |
| A markdown doc | `kanna open md FILE --tab` |
| A source file | `kanna open code FILE[:LINE] --tab` |
| A page | `kanna open web URL --tab` |

- **Run it from your own terminal, without `--window`:** the tab opens in your task's workspace, even while it's hidden on the Shelf or on the Desk but not active, and the reply names your task and where it is (`opened term-5 in ws-3 (task-1dfb, on the Desk), tab-9: running`). `--window new` opens a window of your task's, shown and hidden with it.
- **Don't put a mermaid diagram in the chat when the chat is a terminal:** it shows there as source. A diagram, or an explanation longer than a few lines, goes in a markdown doc in `.kanna/tmp/` that you open in a tab, as the `kanna-explain` skill says.
- **A tab, not a split or a new window,** so your pane keeps its size.
- **Reuse a tab that already shows it:** `kanna workspace list` lists your workspace's tabs and panes. If the file is open there, bring that pane forward (`kanna focus PANE`), or point it elsewhere (`kanna navigate`), or turn comments on in it (`kanna markup PANE`), instead of opening a second copy.
- **Refer to it by the ID the reply prints** (`term-5`, `diff-2`). Pane names are unique across Kanna, so a `--name` another task already uses fails with `name_taken`.
- In the chat, a line saying what the tab shows.

## Messages

`kanna task send TASK TEXT` leaves a message for another task's agent, and `kanna task send planner TEXT` for this repo's planning agent. Kanna hands it to their agent once their turn is over, as a prompt `[message from YOUR-TASK "Name"] TEXT`, so it never lands mid-turn, a stopped Shelf task runs for it, and one that hasn't started gets it with its first message. Only Claude and Codex agents take messages: for any other, `kanna task send` warns and the message waits.

- **Send one only when the user asks you to.** When you're done or need the user, say so in your chat and end your turn: the Desk shows them the task is done or needs them, and they read your summary there.
- **A message you get** starts with `[message from SENDER]`: answer it once, briefly, with `kanna task send SENDER "..."` (or `planner`), since the sender can't see your chat. A message from `user` came from the user: answer in the chat as usual.
- **Keep it to what they need,** in one message: they read it in the middle of their own work.
- **A message from `phone`** came from the user on their phone, who can't see this terminal. Answer only with `kanna phone`: `kanna phone say TEXT` (bold, inline code, lists), `kanna phone show FILE:LINE[-LAST]` for code (from your `HEAD`; `--hot N-M` marks lines), and `kanna phone ask TEXT --choice 'LABEL|WHY'…` for a question, which returns at once: end your turn and the tap or their words come as a new message. Never ask with a prompt tool: they can't answer it from the phone. Keep it short: it's a small screen.

## Present your work

Set up the review view **once**, when the work is first ready to show the user. You may have run unattended on the Shelf: whatever you leave open, focused and zoomed is what the user sees when they make the task active, maybe hours later. Kanna keeps it through the stop.

Reuse those tabs during review; do not repeat the focus/zoom setup after each reply or revision. **Once the user approves the model or work, leave their tabs, focus and zoom alone** unless they ask for a change. Recording approval and committing do not need another presentation setup.

1. **Open what explains the change best,** as tabs ([Show it in a tab](#show-it-in-a-tab)): the spec you changed, with its changes marked (`kanna open md FILE --diff main`; `--diff A..B` for what changed between two commits), or a new one plain (`kanna open md FILE`), since a diff of a new file only marks every line, the mockup (`kanna open web`), comments on the key code (`kanna comment`, as the `kanna-explain` skill says; a change across layers starts with its call tree in `.kanna/tmp/`), a diagram in a doc in `.kanna/tmp/`, the diff (`kanna open diff --merge-base main`: everything the branch changed, committed or not, following the worktree; not a terminal, so it outlasts the stop).
2. **If you wrote or changed a diagram, check it draws:** fix every `FILE:LINE: message` that `kanna open md` (or `kanna diagrams check FILE`) prints, then take `kanna screenshot PANE` of the doc, look at the PNG, and fix labels that overlap or are cut off.
3. **Close tabs that don't help** (`kanna close PANE`): scratch shells, finished builds.
4. **Then focus the tab the user should see first** (`kanna focus PANE`), and zoom it if one pane says it all (`kanna zoom PANE`). Do it last: otherwise the tab you opened last is what shows when the task is made active. It works the same from the Shelf or the Desk.
5. **Check it with `kanna workspace list`:** your workspace's tabs and panes, the shown tab, what's focused and zoomed, as the user will see it.
6. **End your turn with a short summary in the chat:** what changed, why, what's in the tabs, and what's left for the user (merge, a decision, a check).

**Nothing worth a review tab** (a small fix, a question in words)? Leave the current view alone and summarize in the chat.

If you stop to ask the user something before presenting the work, use that first presentation to show what the question is about. If it is already open for review, reuse it without changing focus or zoom. Terminals you leave running are stopped with you and run again when the task starts; what they were in the middle of is lost.

## Schedules

When the user asks for an agent to be prompted at set times ("remind me every morning to…", "rerun the benchmark every 2h"), make a schedule rather than waiting yourself: `kanna schedule add RULE TARGET PROMPT --name NAME`. RULE is two words in local time (`daily 09:00`, `weekdays 18:00`, `mon,thu 10:30`, `every 2h`); TARGET is a task (your own ID for you, as `kanna task list` shows it) or `planner`, the repo's planning agent. At each time Kanna leaves that agent a message, as `kanna task send` does, which it gets when its turn is over as `[message from schedule NAME] PROMPT`. A message from `schedule NAME` is one of these: do what it says, as if the user had asked. When there's a result worth seeing, leave it once, briefly, with `kanna schedule reply NAME TEXT`: the user reads it beside that firing on the Desk's Schedules tab, and a schedule can't take a `kanna task send`.

## Commands

| Command | Does |
|---|---|
| `kanna task name [TASK] NAME` | Rename a task, running or waiting on the Shelf (only when the user asks) |
| `kanna task new NAME [FIRST-MESSAGE] [--agent CMD] [--after TASK,...]` | Add a task to this repo's Shelf; `--agent`: run this agent (`codex`, …) instead of `tasks.agent`; `--after`: it can't start until those tasks are ready or done |
| `kanna task edit TASK --after TASK,...\|none` | Change what a Shelf task waits for (only when the user asks to unblock it) |
| `kanna task ready set [TASK]` | Mark your work finished and tested, at your branch's head: the tasks waiting for it can start on it |
| `kanna task ready clear [TASK]` | Take that back (only when the user asks) |
| `kanna task list [QUERY] [--all] [--include-done]` | This repo's tasks, the Desk's then the Shelf's: ID, name, state (working, idle, needs you, stopped, merged, replied, ready, stale with what it's behind, or blocked with what it waits for; `merges after` a task whose work isn't in main yet), an agent other than the default; `--all`: every repo's; QUERY keeps those where every word starts the task's ID or is its agent's pane ID, or is in its name, first message or branch, and a done task's ID alone says it's done, and when; `--include-done` adds done tasks under Done, newest first: with QUERY every one that matches, without it the 5 done last |
| `kanna task start TASK` | Run a task where it is: a Shelf task gets its worktree, branch and agent with the first message, and stays on the Shelf; a stopped task resumes its conversation |
| `kanna task stop [TASK]` | End a task's agent and hang up its terminals, leaving it where it is with its tabs as they are (only when the user asks) |
| `kanna task pick TASK` | Move a Shelf task to the Desk, starting it if it's stopped (three per project; refused when the Desk is full) |
| `kanna task activate TASK` | Make a Desk task the active one and show it (only when the user asks) |
| `kanna task park [TASK]` | Show the Desk view of the task's Desk; the task stays active (only when the user asks) |
| `kanna task shelve [TASK]` | Move a Desk task back to the Shelf as it is, running or stopped (only when the user asks) |
| `kanna task send TASK\|planner TEXT` | Leave a message for another task's agent or this repo's planning agent; it gets it when its turn is over (only when the user asks, or to answer a message) |
| `kanna task messages [TASK]` | A task's messages, sent and received, and whether each was delivered |
| `kanna task subscribe\|unsubscribe EVENT [TASK]` | Hear an event or stop hearing it ([Events](#events)); from your terminal, your task |
| `kanna task events [TASK]` | What a task hears, and every event message it was sent |
| `kanna task done [TASK]` | End a task, or take it off the Shelf; says which blocked tasks can start now |
| `kanna schedule add RULE TARGET PROMPT [--name NAME]` | Leave a task, or `planner`, a message at set times |
| `kanna schedule list` / `remove NAME\|ID` | List the schedules, or remove one |
| `kanna schedule pause NAME\|ID` / `resume` / `run` | Pause one, resume it (a time missed while paused fires once), or leave its message now |
| `kanna schedule reply NAME\|ID TEXT` | Leave a result beside the firing you were last delivered |

Run them from your own terminal, in the repo: a new task belongs to the repo you're in.
