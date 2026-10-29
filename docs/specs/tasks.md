# Tasks

**Status:** built · **Mockup:** [`references/mockups/desk.html`](../../references/mockups/desk.html) (Simulate: no tasks; ⌘. to see the Shelf; `kanna task new` in an agent's pane) · **Builds on:** [Desk](desk.md) · **From:** Kanna v2's tasks (a worktree per task) and v3's (a task belongs to a repo, and waits until it's pulled)

A **task** is one agent's work in one repo. Agents make tasks: they add them to the repo's **Shelf**, and you move the ones you want to the Desk, where each gets its own agent, branch and worktree. There's no form: you tell the window's [planning agent](planning-agent.md) what you want, and it makes the tasks.

![The Desk view: the planning agent on the left, the Desk's tasks above the Shelf on the right](desk-new.png)

## Contents

1. [What a task is](#1-what-a-task-is)
2. [How tasks are made](#2-how-tasks-are-made)
3. [The Shelf](#3-the-shelf)
4. [Naming](#4-naming)
5. [Stop, Done, or back to the Shelf](#5-stop-done-or-back-to-the-shelf)
6. [Choosing the agent](#6-choosing-the-agent)
7. [CLI](#7-cli)
8. [How it's built](#8-how-its-built)
9. [Open questions](#9-open-questions)
10. [Glossary](#10-glossary)

## 1. What a task is

```mermaid
stateDiagram-v2
    direction LR
    Shelf: on the Shelf
    Desk: on the Desk
    [*] --> Shelf: an agent runs kanna task new
    Shelf --> Desk: Move to Desk · pick (starts it if it's stopped)
    Desk --> Shelf: Move to Shelf · shelve (as it is)
    Shelf --> [*]: Done
    Desk --> [*]: Done
```

- **One repo.** A task belongs to the repo it was made in, so it can later be sent to another machine or person with that repo.
- **A name and a first message.** On the Shelf, before it starts, that's all a task is.
- **Running and place are separate.** A task is in one place (the Shelf, or a Desk, active or not) and either **running** (its agent alive) or **stopped** (no agent; a new Shelf task, or one stopped, its worktree, branch, conversation and tabs kept). `kanna task start` and `kanna task stop` change only whether it runs; Move to Desk, Move to Shelf and making a task active change only where it is, except that Move to Desk starts a stopped task.
- **Started, it runs in its own worktree `<repo>/.kanna-worktrees/<NAME>` on branch `<NAME>`, made from the repo's checked-out commit, and an agent there that gets the first message. NAME is the task's ID (`task-3f9a1c2e`, [task IDs](task-ids.md)), made with the task and checked against the repo's worktrees and branches; the agent renames the branch once it knows the work ([4](#4-naming)). The repo's `info/exclude` keeps `.kanna-worktrees/` and each worktree's `.kanna/tmp/` (an agent's scratch files, such as [code tour](code-tour.md) docs) out of git.
- **Every task works in a worktree.** All development happens in one, never in the repo's own checkout.
- **A new worktree runs the repo's setup** (`tasks.setup`) in the agent's terminal before the agent starts, once per worktree ([task scripts](task-scripts.md#2-setup)).
- **Kanna is git-based: a task needs a git repo.** In a folder that isn't one, making a task is refused; Kanna never runs `git init` itself. An agent sets the repo up with you (in a new, empty project, the planning agent does), and a repo with no commits yet works: its first task's branch starts empty (`git worktree add --orphan`), so the task's first commit is the project's. Kanna never commits for you.
- **A Desk holds three per repo**, the active one among them, running or stopped. Any number run on the Shelf.

## 2. How tasks are made

- **By an agent:** `kanna task new "NAME" "FIRST MESSAGE"` adds a task to the Shelf of the repo it runs in. The **planning agent**, the one Kanna starts in a window's [planning terminal](planning-agent.md), makes them as you plan with it, and a task's agent splitting off its own work does too.
- **By you, by hand:** the same `kanna task new` in the repo, in any terminal.
- **Only these two.** The app never starts a blank task: every task is made with the CLI or by an agent, and started from the Shelf (Move to Desk, or `kanna task start` to run it there). There's no Start a task button or menu item.
- **A window with nothing in it** starts in the Desk view with its planning terminal (pictured above), where Kanna starts the planning agent: when Kanna opens with nothing, on File → New Desk (⌘N, in `~`), and on `kanna PATH` when no Desk is on PATH's project ([new Desk](new-desk.md)).
- **`kanna PATH` brings forward the project's Desk** when one is open (`kanna .` in a repo, a subfolder or one of its worktrees): it comes to the front, its planning terminal or task focused, and the CLI says `brought forward d4 on ~/work/app`. With several Desks on the project (⌘N and `kanna task activate` can make a second), it's the one the app had in front last, never the daemon's pick. Only with none does it open one (`opened d11 on …`). One project, one Desk; ⌘N still always opens a new one ([the CLI acts on its caller](cli-caller.md#2-desks-by-project)).
- **No New Instance.** kmux's Kanna → New Instance (a whole second Kanna with its own daemon, keeper and state) isn't in Kanna's menu: working on another project is what a Desk is for. Instances stay as a developer and testing tool (dev builds, `kanna --instance`), not a user feature ([build channels](build-channels.md)).
- **A new or empty project** (an empty folder, or not a git repo yet): the planning agent helps you decide what it is, sets it up (`git init` and a first commit only when you agree), and turns the plan into Shelf tasks, with Kanna's `kanna-new-project` skill.

## 3. The Shelf

- **The Desk view lists it** under the Desk's tasks: each task's name, its first message, and the task that made it.
- **Move to Desk** (`kanna task pick`) puts a task on the Desk, starting it if it's stopped: it makes the worktree and starts the agent with the first message. When the Desk holds three, it's refused ("Desk full") until one goes back to the Shelf or is Done.
- **`kanna task start` runs a task where it is**: a Shelf task gets its worktree and agent and stays on the Shelf, running, so you can start several and move them to the Desk later. `kanna task list` shows each one's state: working, idle, needs you, or stopped.
- **Oldest first.** The strip under the active task shows how many wait ("N on the Shelf").
- **Search it** with ⌘F in the Desk view, by name or first message: [Shelf search](shelf-search.md).
- **A task can wait for others:** blocked, it can't start until they're Done ([blocked tasks](task-dependencies.md)).
- **Shelf tasks run by themselves,** a few at a time, and Kanna stops each when its agent's turn ends ([Shelf runners](shelf-runners.md)).

## 4. Naming

- **A task's name is the one its maker gave it** with `kanna task new`, kept when it starts. Every task is made by an agent or with the CLI, so every task has a name from the start, and its agent doesn't name it. `kanna task name` renames one when you ask, running or waiting on the Shelf; a Shelf task starts with its new name, even one renamed while it starts. The app has no rename for a Shelf task: it's the CLI's.
- **A Shelf task's first message never changes.** Only its name can. To change what it says, make a new task and take the old one off with `kanna task done`: the agent that starts it gets the words it was made with, and its history says what it was asked.
- **It learns how from Kanna's skills.** [`kanna-tasks`](../../.agents/kanna-skills/skills/kanna-tasks/SKILL.md) names its branch, writes Shelf tasks' first messages that stand alone and leaves starting tasks to you. The planning agent has its own, keyed on being told it's the planner ([agent setup](agent-setup.md#2-how-an-agent-knows-its-in-kanna)): [`kanna-planning`](../../.agents/kanna-skills/skills/kanna-planning/SKILL.md) (say what's running and waiting, plan with you, make Shelf tasks, start or end them only when you ask, never write code), and [`kanna-new-project`](../../.agents/kanna-skills/skills/kanna-new-project/SKILL.md) for a folder that's empty or not a repo yet (decide what it's for, `git init` and a first commit only when you agree, then the first tasks). [`kanna-markup`](../../.agents/kanna-skills/skills/kanna-markup/SKILL.md) answers [review comments](doc-markup.md).
- **Agents in any repo have the skills** once you agree to [agent setup](agent-setup.md).
- **The agent renames its branch** with `git branch -m NEW` in its worktree, from `task-3f9a1c2e` to a short name for the work. The worktree's folder keeps its name: the agent and its terminals are in it.

## 5. Stop, Done, or back to the Shelf

- **Stop ends a task's agent and terminals and leaves it where it is** (`kanna task stop`): on the Desk or the Shelf. Nothing closes: its agent's pane and terminals stay, exited, its other tabs stay with their comments, and the shown tab, focus and zoom stay, as do its worktree and branch. While it's stopped, a pane keeps its last screen with the exited notice. Starting it again (`kanna task start`, or Move to Desk) runs its agent in that pane on a new terminal, in the daemon and the app (the old run's scrollback goes, and so do the modes it left on), resuming its conversation ([which agents resume](#which-agents-resume)), and reruns the terminals that were running when it stopped. A stopped task still takes its place on the Desk.
- **Its agent quits as you'd quit it:** Stop, Done and a [Shelf runner](shelf-runners.md#2-how-its-built) type its quit keys, and keep the session it says as it exits when the task has none of its own; it's hung up only if it has no keys or doesn't quit in 5s.
- **A task presents its work:** before its turn ends, its agent sets up its tabs to show what it changed and why, and that's what you see when you make it active ([presenting](presenting.md)).

- **Done ends the task:** its agent and tabs close, or it leaves the Shelf.
- **Done quits the agent, runs the repo's teardown in its pane, then cleans up a merged task:** the task is Done once teardown succeeds (if it fails, the task stays, its pane saying why); its worktree and branch go when its branch is merged and the worktree has no changes; otherwise they stay, and its events say why ([task scripts](task-scripts.md#3-teardown-and-cleanup)).
- **Move to Shelf puts a task back** (Task menu, or `kanna task shelve`), freeing its place on the Desk: it leaves the Desk and waits on the Shelf again, under its old ID if Kanna started it.
- **It stays as it is there,** running or stopped. A running agent carries on with whatever it was doing, and its tabs stay as they were.
- **Moving it to the Desk again brings it back as it is:** the same agent and tabs, with no restart (a stopped one starts).
- **If its agent's pane closed meanwhile,** starting it opens a new agent in its worktree (a new worktree only if that's gone too), and the agent comes back to its conversation.

### Which agents resume

Kanna gives an agent a session ID of its own when it starts, if the agent takes one, and keeps it with the task; otherwise it finds the task's session in the agent's records by the task's worktree, the newest if there are several, and keeps that ID with the task. Each task has its own worktree, so the folder's newest session is the task's. Once the agent has started, its pane's command is the resuming one, so `kanna restart` on it comes back to its conversation (claude refuses a session ID that already has one). Kanna looks for a session it hasn't found each time the agent's status changes, and when the task starts again; until it's found, codex resumes with `--last` and opencode starts again.

| Agent | Started with | Resumed with | A task Kanna didn't start |
|---|---|---|---|
| claude | `claude --session-id UUID 'FIRST'` | `claude --resume UUID` | `claude --continue` (the folder's latest), until a stop finds the ID it says |
| codex | `codex --yolo 'FIRST'` (it takes no ID), through [its app-server](codex-app-server.md) | `codex --yolo resume UUID`: the newest rollout in `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-*.jsonl` (`~/.codex` by default) whose first line, its `session_meta`, has the folder as `cwd`, looking only at days since the worktree was made, or the one it said as it quit; `resume --last` until one is known (it's codex's newest, which isn't reliably the task's) | the same |
| copilot | `copilot --yolo --session-id UUID -i 'FIRST'` | `copilot --yolo --session-id UUID` | the latest session in `~/.copilot/session-state/*/workspace.yaml` whose `cwd` is the folder (`copilot --continue` isn't the folder's) |
| opencode | `opencode --prompt 'FIRST'` (it takes no ID) | `opencode --session ID`, the latest top-level session in `~/.local/share/opencode/opencode.db` whose `directory` is the folder (`--continue` is the repo's, shared by its worktrees) | the same |
| antigravity | `agy --dangerously-skip-permissions -i 'FIRST'` (it takes no ID) | `agy --dangerously-skip-permissions --conversation ID`, the one it said as it quit, else `--continue` (the folder's last conversation) | the same |

**Tested** by e2e `real_agent_tasks` for claude and codex: the real agents run as tasks against a local mock model (`tools/agent-harness`), come back to their conversation after `kanna restart` on their first start (codex's pane by then resuming its own session's UUID), and after Move to Shelf, Stop (by their quit keys) and Start, the mock sees their next message carry the first, with no second start. Copilot and antigravity only talk to their own services, and opencode (which could use the mock as a provider) isn't covered yet, so only their command lines are tested (`backlog.rs`).

**A planning agent** resumes too, one conversation per project rather than per task: it starts the same way with `You are the Kanna planning agent.`, and since the main checkout isn't a folder of its own, a session it takes no ID for is the first one made there since it started, not the newest ([planner resume](planner-resume.md#2-each-agent)).

| Planner | Started with | Resumed with |
|---|---|---|
| claude | `claude --session-id UUID 'You are the Kanna planning agent.'` | `claude --resume UUID` |
| codex | `codex --yolo 'You are …'` | `codex --yolo resume UUID`: the one it said as it quit, else the first rollout since it started whose `cwd` is the main checkout |
| copilot | `copilot --yolo --session-id UUID -i 'You are …'` | `copilot --yolo --session-id UUID` |
| opencode | `opencode --prompt 'You are …'` | `opencode --session ID`: the first top-level session since it started whose `directory` is the main checkout |
| antigravity | `agy --dangerously-skip-permissions -i 'You are …'` | not resumed: a new conversation each time |

An agent not in the table starts again with its first message, its conversation lost; so do copilot and opencode when no session of theirs is found for the folder. Copilot, opencode and antigravity take the first message as a flag (`-i`, `--prompt`), not an argument.

- **Done on a Shelf task that's still running** ends its agent and tabs too.

## 6. Choosing the agent

- **A task runs the agent it was made with,** kept with it from `kanna task new` to Done: `kanna task list` says which (an AGENT column; `agent` in `--json`). The Desk view's rows don't: their second line is what the task waits for, its first message, its status and when it was made or last active.
- **By default, the `tasks.agent` setting** ([settings](settings.md#3-keys)): `claude` unless you set it. In your `~/.kanna-v4/config.json` it may be any command (`"codex"`, `"claude --model opus"`); a repo's `.kanna/config.json` may only name an agent Kanna knows (`claude`, `codex`, `copilot`, `opencode`, `agy`), so a repo you clone can't make Kanna run a command of its choosing.
- **For one task, `--agent`:** `kanna task new NAME MSG --agent codex`. The planning agent and a task's agent add it when you ask for an agent ("do this one in codex"), as the `kanna-planning` and `kanna-tasks` skills say; otherwise they leave it to the setting.
- **Every agent gets the same:** the first message as it starts, the [skills](agent-setup.md) (Codex reads them from `~/.agents/skills`), [status](agent-status.md) from its screen, `kanna send`, and [resume](#which-agents-resume).
- **Codex asks once per repo whether to trust it,** before its first message, even with `--yolo`: the task shows needs you until you answer, and codex saves the answer. Kanna doesn't answer for you (codex reads trust only from its saved settings, not from `-c`); a repo you plan in with codex is trusted already.

**Tested** by e2e `codex_task`: a codex-like mock agent (`tools/mock-agent` named `codex`) set as `tasks.agent` starts from the Shelf with its first message, renames its branch, goes working, idle and needs you, answers `kanna send`, shows its agent in `kanna task list` and in the app's data (`debug.desk`), and ends with Done; a repo's `tasks.agent` naming a command is ignored. A real codex task was run once by hand (October 2026): it read `kanna-tasks`, renamed its branch, committed, was stopped by the Shelf runner and resumed on the Desk.

## 7. CLI

| Command | Does |
|---|---|
| `kanna task new NAME [FIRST-MESSAGE] [--agent CMD] [--after TASK,…]` | Add a task to this repo's Shelf (the agent defaults to the `tasks.agent` setting, else `claude`, and starts without asking permission; [6](#6-choosing-the-agent)); `--after` makes it wait for others ([blocked tasks](task-dependencies.md)) |
| `kanna task edit TASK --after TASK,…\|none` | Change what a Shelf task waits for |
| `kanna task start TASK` | Run a task where it is: a Shelf task gets its worktree, agent and first message and stays on the Shelf; a stopped task's agent comes back, resuming its conversation |
| `kanna task stop [TASK]` | End a task's agent and hang up its terminals, leaving it where it is with its tabs as they are (default: the one you're in) |
| `kanna task pick TASK` | Move a Shelf task to the Desk of the CLI's instance's app, starting it if it's stopped; refused when that Desk holds three of its project's |
| `kanna task name [TASK] NAME` | Rename a task (default: the one you're in), running or on the Shelf (`task-3f9a`); a Shelf task starts with the new name |
| `kanna task list [QUERY]` | Every task: ID, name, where (Desk or Shelf), how long ago it was last active ("12m ago"), state (working, idle, needs you, or stopped), agent, repo; with QUERY, only those whose name or first message has every word ([Shelf search](shelf-search.md#cli)) |
| `kanna task shelve [TASK]` | Move a task from the Desk to the Shelf as it is, running or stopped (default: the one you're in); refused on a Shelf task, pointing to `pick` |
| `kanna task send TASK\|planner TEXT` | Leave a task's agent or the repo's planner a message, typed in when its turn is over ([agent messaging](agent-messaging.md)) |
| `kanna task messages [TASK]` | A task's messages, or every message |
| `kanna task done [TASK]` | End a task, or take it off the Shelf |
| `kanna agents [status]` | Whether your agents are set up for Kanna, and whether this Kanna has newer skills ([agent setup](agent-setup.md)) |
| `kanna agents setup [--ask]` | Install Kanna's skills and Claude's sandbox entry (`--ask`: say what and ask first) |
| `kanna agents update [--ask] [--force]` | Bring the skills up to this Kanna's, merging your changes (`--force`: Kanna's over yours) |
| `kanna agents remove` | Take back what Kanna installed |

`activate` and `park` are the [Desk's](desk.md#5-cli). The old `defer` and `pull` still work as hidden aliases of `shelve` and `activate` ([Desk view §3](desk-view.md#3-cli-verbs)).

## 8. How it's built

```mermaid
flowchart TB
    CLI["kanna task new / start / name"] --> D
    APP["Kanna.app: Desk view"] --> D
    APP -->|"plan: the window's planning terminal"| D
    D["daemon: the Shelf<br/>ID · name · repo · first message · maker"] -->|"start: git worktree add, open --dock on the Shelf or claimed"| L["layout: workspaces"]
```

- **The daemon keeps the Shelf** (`Backlog` in `backlog.rs`, an internal name kept from before the [rename](desk-view.md#5-internal-names)) in `kanna.db` ([database](database.md)), with every task from made to done and each change of its place or state as an event, so agents add tasks with or without the app.
- **The daemon opens planning terminals** (`backlog` `plan`): it makes the command (the context limit, agent setup, the planner script, then your shell) and opens it as a Desk's planner (a workspace with no task) in its folder ([new Desk](new-desk.md#3-how-its-built)).
- **A running task is its docked agent pane**, as on the [Desk](desk.md): its name is the pane's name, its repo the pane's folder.
- **Only a task's agent docks.** `kanna move PANE --dock` refuses any pane that isn't docked (`not_a_task`), so a task starts only by starting one and the limit holds.
- **A task's agent is frozen** ([docked pane](docked-pane.md#2-rules)): the daemon opens it with `open --dock --frozen`, so it stays in its dock, and nothing is dropped, swapped or docked in its place, which would make another pane the task.
- **A task running on the Shelf is hidden, not closed:** starting a Shelf task opens its agent docked straight onto the Shelf, outside the limit, and a task moved there keeps its window. The layout keeps its workspace (agent docked, tabs, its windows) on the Shelf, saved with the rest, so its panes keep running and survive a relaunch; the Desk, the limit and `activate` skip it. Its Shelf entry names its agent's pane, and moving it to the Desk clears the mark.
- **Moving a task to the Desk** checks the limit, runs `git worktree add` if it has no worktree, opens the agent docked straight onto the Desk in that folder with the first message (or releases its window from the Shelf, starting a stopped agent), names the pane, and drops the Shelf entry. A held task is on the Desk before its agent and tabs start again ([Move to Desk latency](move-to-desk-latency.md)).
- **Stopping** closes nothing: it hangs up the task's running terminals, marking each to run again, then its agent's (SIGHUP to its process group), keeping the panes exited, saved with the rest ([presenting](presenting.md#3-how-its-built)). **Starting a stopped task** restarts the agent's pane with the command that resumes it ([which agents resume](#which-agents-resume)), so it keeps its ID and place, then the marked terminals with their own commands and folders.
- **One table per agent** in the daemon (`AGENTS` in `backlog.rs`) says how to start it without asking permission, how to pass the first message, how to name a new session, how to resume one, how to resume the folder's latest, and where to find its sessions by folder. The lookup runs while the agent runs, each time its status changes, until it finds the session (it may not be written yet when the agent starts), and again when the task starts again.
- **An ID is never handed out twice.** A new task's ID is saved as it's made, so a restart can't reuse one, and a task keeps it from made to done. IDs name tasks only; worktrees and branches get their own random names, so a lost `kanna.db` or another Kanna on the same repo can't collide with them.
- **A task's agent doesn't ask permission.** Starting one adds the agent's own flag for that (`claude --dangerously-skip-permissions`, `codex --yolo`, `copilot --yolo`, `agy --dangerously-skip-permissions`; opencode has none) unless its command already says how to ask (`--permission-mode`, `--ask-for-approval`/`-a`, `--allow-all`/`--allow-all-tools`, `--mode`). The planning agent and the quiz agent start the same way. The task works in its own worktree, and you watch it on the Desk. The task's Shelf entry keeps the command as written.
- **The agent's name the app keeps** (`debug.desk`) is its command's program (`/opt/bin/codex --yolo` is `codex`): from the Shelf entry's command, and for a task on a Desk from its pane's command (`agent` in the layout's workspaces and in `task list`).
- **`$KANNA_AGENT` is gone:** the daemon read it as the default agent, but it was the daemon's environment, not yours, and only the e2e tests set it; they set `tasks.agent` in their own settings now.
- **A Codex task runs attached to an app-server of its own** ([Codex's app-server](codex-app-server.md)), started from the task's terminal, not in Codex's shared background server, which runs every session's commands and hooks with the environment of whatever started it. Codex's commands and hooks get the task's `$KANNA_TERMINAL` and `$KANNA_SOCKET`, as Claude's do, and Kanna can hand it messages.

## 9. Open questions

1. **Task IDs that travel:** IDs ([task IDs](task-ids.md)) are random 32 bits, checked only on this machine; sending tasks elsewhere may want longer ones.
2. **Codex, opencode and antigravity take no session ID up front:** Kanna finds codex's and opencode's afterwards by the task's worktree, and antigravity's from what it says as it quits; each is the task's while each task has its own worktree. If one starts to take an ID, it moves to the ID like claude and copilot.
3. **Codex saves its last reply a moment after showing it:** it writes the reply to its session about a second after it appears, and a stop within that second (SIGHUP, then SIGKILL a second later) loses the reply: started again, it shows the question and "Conversation interrupted". [Shelf runners](shelf-runners.md) wait for the turn to end and quit codex with its own keys; `kanna task stop` still hangs up; `real_agent_tasks` waits for the reply to be saved.

## 10. Glossary

| Term | Meaning |
|---|---|
| **Task** | One agent's work in one repo, shown on a Desk |
| **Desk** | A project's window, and the up to three tasks it holds, one of them active ([Desk](desk.md)) |
| **Shelf** | A repo's tasks off the Desk: a name and a first message each, and the agent of each one started there or moved back running |
| **Running / stopped** | Whether a task's agent is alive; separate from where the task is |
| **Repo** | The git repository a task belongs to |
| **Worktree** | A second checkout of the repo, in `.kanna-worktrees/`, on the task's own branch |
| **Agent setup** | Kanna's skills in your home folder for Claude and Codex, and Claude's sandbox letting agents reach Kanna; installed and updated when you agree ([agent setup](agent-setup.md)) |
| **First message** | What the agent is told when its task starts |
