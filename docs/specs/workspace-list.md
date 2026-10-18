# kanna workspace list

**Status:** built · **Builds on:** [workspaces](workspaces.md), [presenting](presenting.md), [task IDs](task-ids.md), [CLI](cli.md) · **Replaces:** `kanna list`

**An agent presenting its work needs to see its own workspace:** its tabs, the panes in each, which tab is shown, what's focused and whether it's zoomed, as the user will see it when they pull the task. `kanna workspace list` prints that, for the caller's pane (`$KANNA_PANE`, which every Kanna terminal has), with no argument. `kanna list` goes: nobody read its text, and it had no clear purpose.

```mermaid
flowchart LR
    P["$KANNA_PANE<br/>(read by the CLI)"] --> WS["its workspace<br/>ws-3"]
    T["task-1dfb12c2"] -->|has a| WS
    WS --> D["dock · tabs · panes<br/>shown tab · focus · zoom"]
    WS --> OW["its windows<br/>(w40, hidden with it)"]
```

- [A task has a workspace](#a-task-has-a-workspace)
- [The output](#the-output)
- [What changes](#what-changes)
- [kmux](#kmux)
- [Decisions](#decisions)

## A task has a workspace

```mermaid
flowchart LR
    subgraph before["Before"]
        BW["workspace task-1dfb12c2<br/>task: task-1dfb12c2"] -.->|"names"| BT["task task-1dfb12c2"]
    end
    subgraph after["After"]
        AT["task task-1dfb12c2<br/>workspace: ws-3"] -->|"has a"| AW["workspace ws-3<br/>(knows no tasks)"]
    end
```

- **A workspace has an ID of its own,** `ws-1`, `ws-2`, …, made by the layout. The dash keeps it from being read as a window (`w1`).
- **The task's record names its workspace** (a `workspace` key in its `doc` in `kanna.db`, so no migration). The daemon sets it from the workspace a start makes, and keeps it right from each listing, by the workspace its agent is docked in. The layout knows nothing of tasks: no `task` field, no lookup by a task's ID; the daemon turns a task given to `window assign` into its workspace.
- **A workspace can have no task,** as a docked agent's (`kanna open --dock`) does.
- **Replies** name the workspace by its ID; the daemon adds the task and its name (`task`, `name`) when one has it: `opened p5 in ws-3 (task-1dfb, on the Shelf), tab t9: running`.

## The output

`kanna workspace list`, from a task's agent or any pane of its workspace:

```text
ws-3  task-1dfb (Fix login), in the backlog
  dock: p407 (Fix login)  left 2/5  frozen
  focus: p400 (diff), zoomed
  *t211 "diff"      p400 (diff)
   t212 "login.md"  p5:1/2 | p6:1/2
  window w40, hidden with it
    *t30 "Tab 1"    p9
```

`*` is the shown tab. From a pane in a free window, it's that window; from a pane with no window (Kanna.app closed), it says so.

```text
w45  free
  focus: p411 (notes)
  *t247 "notes"  p411 (notes)
```

`kanna workspace list --all`: every window, then every workspace as above, then free windows and panes in no window.

```text
Windows
*w31  Desk: ws-1 (task-1dfb, Fix login), planning terminal p280
 w45  free

ws-1  task-1dfb (Fix login), on Desk w31
  …
ws-3  task-22a4 (Skills show diffs), on the Shelf
  …

Free windows
w45
  *t247 "notes"  p411 (notes)
```

`--json` prints the daemon's list reply: the caller's workspace, or with `--all` everything, as `kanna list --json` did (windows, panes, workspaces), with each workspace's shown tab, focus, zoom and its windows' tabs added. e2e scenarios and `tests/cli.rs` read `kanna workspace list --all --json`.

## What changes

| Where | Change |
|---|---|
| kanna-mux | workspaces get `ws-N` IDs and lose `task`; the list reply gives each workspace its shown tab, focused and zoomed pane, and its hidden windows' tabs (`hiddenWindows`); the saved layout is version 3 (version 1 and 2 layouts load, their task-named workspaces given `ws-N` IDs, their windows following) |
| daemon | a task's `doc` names its workspace, set when it starts and from each listing; replies add the task and its name |
| CLI | `workspace list [--all] [--json]`; `list` goes |
| Skills | kanna-tasks "Present your work" ends with `kanna workspace list`; kanna-quiz finds its window with it |
| e2e, tests | `kanna workspace list --all --json` |
| Specs | this; [workspaces](workspaces.md) (the model, IDs, replies), [database](database.md), [CLI](cli.md), [commands](commands.md); the old `kanna list` output spec is removed |

## kmux

The same change goes to kmux ([kmux subtree](kmux-subtree.md)), so the CLI stays kmux's: `kmux list` goes, `kmux workspace list [--all]` joins `new`, `hide`, `show` and `close`, read from `$KMUX_PANE`, and kmux's workspace IDs become `ws-1` (its saved `s1` IDs still load). A kmux branch and PR, as the subtree says.

## Decisions

| | Decided | Why |
|---|---|---|
| `kanna list` | removed | no skill or script read its text; e2e read `--json` |
| Context | the caller's pane, read from `$KANNA_PANE` by the CLI | every Kanna terminal has it, as `open` already uses it |
| Task and workspace | a task has a workspace; the layout doesn't know tasks | a workspace is presentation, a task is work; one can exist without the other |
| IDs | `ws-1`, `ws-2` | apart from task IDs; can't be read as a window |
| Everything | `--all`, also for scripts (`--all --json`) | one command, not a second one for dumps |
| kmux | backported | kanna's CLI is kmux's |
