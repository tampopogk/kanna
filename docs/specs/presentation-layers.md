# Presentation Layers

**Status:** proposed · **Changes:** [architecture](architecture.md#2-what-the-daemon-keeps-what-the-apps-keep), [layout](layout.md), [app link](app-link.md), [workspaces](workspaces.md) · **Related:** [tasks](tasks.md), [diff viewer](diff-viewer.md), [restarting the daemon](daemon-restart.md)

Kanna's daemon runs headless, and any number of apps (the Mac app now, others such as iOS later) connect to it to view tasks and their workspaces. So what a task *is* and how it's *presented* are split into three layers: the **work** and the **logical presentation** live in the daemon and work with no app; the **physical presentation** lives in each app. Today the layers are mixed, and that's caused real bugs: diff panes go blank after a daemon restart, a restarted task's terminal couldn't be typed into, and Done left ghost windows.

## Contents

1. [The three layers](#1-the-three-layers)
2. [Who changes what](#2-who-changes-what)
3. [IDs](#3-ids)
4. [Content](#4-content)
5. [Where today's code mixes the layers](#5-where-todays-code-mixes-the-layers)
6. [Decisions](#6-decisions)
7. [kmux](#7-kmux)
8. [Glossary](#8-glossary)

## 1. The three layers

```mermaid
flowchart TB
    CLI["kanna CLI, agents"]
    subgraph D["daemon (headless)"]
        W["1 · Work<br/>tasks: worktree, branch, agent, turns,<br/>runners, scripts, history<br/>content: terminals · diffs · files · pages"]
        L["2 · Logical presentation<br/>workspaces · tabs · arrangement<br/>shown tab · focus · zoom · Shelf<br/>planner workspace per project"]
        L -->|"by content ID"| W
    end
    subgraph M["Mac app"]
        PM["3 · Physical presentation<br/>windows · its Desk · panes<br/>sizes · text size · font · focus · dock width"]
    end
    subgraph I["iOS app (later)"]
        PI["3 · Physical presentation"]
    end
    CLI -->|"commands"| W & L
    PM & PI -->|"commands"| W & L
    W & L -.->|"publishes every change"| PM & PI
    classDef work fill:#6b4f2f,stroke:#e0a960,color:#ffffff
    classDef logic fill:#2f6f4f,stroke:#6fcf97,color:#ffffff
    classDef phys fill:#2f4f8f,stroke:#7fa7ff,color:#ffffff
    class W work
    class L logic
    class PM,PI phys
```

| Layer | Lives in | What's in it |
|---|---|---|
| **1 · Work** | daemon | **Tasks** as units of work: worktree, branch, agent, turns, runners, scripts, history, and whether it's queued, running, stopped or done. **Content**, each with an ID of its own: terminals, diffs, files, pages ([4](#4-content)). |
| **2 · Logical presentation** | daemon | Each task's **workspace**: its docked agent and its tabs, and any **extra windows** it asked for, each with tabs of its own; what each tab shows, by content ID; the arrangement as structure (this beside that, a ratio only as a hint); what the task is presenting: the shown tab, the focused content, and the **zoomed** content (`kanna zoom`: one content fills its tab); the **Shelf** and its three-per-repo limit; a **planner workspace** per project for content that's no task's. |
| **3 · Physical presentation** | each app | **Windows**: where each of a workspace's windows is, and how the app spreads tabs across windows; the app's own **Desk** (which Shelf task it shows); **panes** as views of content; sizes, **text size** (⌘+ and ⌘−, a page's or a terminal's scale), font, first responder, the dock's width. |

- **The daemon publishes every change** to layers 1 and 2. An app connects, subscribes, and renders; on every (re)connect it subscribes again and gets the current state, so nothing depends on what the daemon was told before it restarted.
- **Every layer-2 command works with no app.** A task opens a tab, shows a diff, focuses its spec, all in the daemon; an app that opens later shows the workspace as the task left it.
- **Logical windows, physical windows.** A task may ask for an extra window (`open --window new`); that's logical, so every app shows it, and the app realizes it at a default position that the user can then move. Independently, an app may spread a workspace's tabs across more windows of its own (a tab torn off): that's physical, and the task neither sees nor cares.
- **Physical state never leaves the app.** Pane and window IDs, frames, sizes and focus are the app's; the daemon never stores them and the CLI never sees them.
- **Each app has its own Desk.** Pulling a task onto the Desk in one app doesn't move it in another. The Shelf is shared: it's the set of tasks out, and a Desk shows one of them.

## 2. Who changes what

| Change | Made by | How |
|---|---|---|
| Start, stop, done, a turn, a runner, a script | the daemon, on a command from the CLI, an agent or an app | layer 1 |
| Open a tab, close it, rename it, move it to another workspace, focus or zoom content, shelve or defer a task | any client: the CLI, an agent, or an app (a drag, ⌘W) | a command on layer 2, the same one the CLI sends |
| Window frames and positions, tabs torn off into windows, splits' sizes, text size, which Shelf task the Desk shows, first responder | the app alone | local; saved by the app |
| Pull a task onto the Desk, park it | the user in an app, or `kanna task pull` and `park` | layer 3 of one app: the CLI's instance's app (`KANNA_INSTANCE`), started if none is running, as the CLI starts Kanna.app today |
| A terminal's grid size | the app that last resized it (as tmux's `latest`) | `resize`, layer 1; with no app it keeps its size |

- **An app is a client like any other** on layers 1 and 2: dragging a tab to another workspace sends the `move` the CLI would, and the app redraws when the change comes back.
- **Stop** hangs up the task's tab terminals (the daemon finds them in its workspace) and **Start** reruns them, with no app needed. **Done** closes the workspace and its content; every app drops its views of it.
- **Hints, not orders:** a split's ratio and a line to scroll to are kept as hints that each app applies as it can.
- **Within a workspace, moving a tab between windows is the app's;** moving one to another workspace is a command on layer 2.

## 3. IDs

```mermaid
flowchart LR
    T["task-3f9a1c2e"] -->|"has"| WS["ws-3"]
    T -->|"agent"| A["term-41"]
    WS -->|"dock"| A
    WS -->|"tabs"| TB["tab-9 · tab-10"]
    WS -->|"windows"| LW["win-2: tab-12"]
    TB -->|"shows"| C["term-44 · diff-2 · file-7 · page-3"]
    P["p5, w4 (app-local)"] -.->|"a view of"| C
    classDef d fill:#2f6f4f,stroke:#6fcf97,color:#ffffff
    classDef a fill:#2f4f8f,stroke:#7fa7ff,color:#ffffff
    class T,WS,A,TB,LW,C d
    class P a
```

| ID | Of | Made by | Seen by |
|---|---|---|---|
| `task-3f9a1c2e` | a task | daemon ([task IDs](task-ids.md)) | everyone |
| `ws-3` | a workspace | daemon | everyone |
| `win-2` | a workspace's extra window | daemon | everyone |
| `tab-9` | a tab in a workspace | daemon | everyone |
| `term-41`, `diff-2`, `file-7`, `page-3`, `ios-1` | content | daemon; never reused | everyone |
| `p5`, `w4` | a pane, a window on screen | each app | that app only |

- **A restart makes a new terminal with a new ID.** `restart term-41` (or a task starting again) ends it and answers `term-48`; the workspace's tab now shows `term-48`, and an app sees a new content ID, so it makes a new view instead of reusing the exited one.
- **A task keys on its agent's terminal ID and its workspace ID,** both in its record; neither is a pane.
- **`KANNA_TERMINAL`** replaces `KANNA_PANE` in a terminal's environment: the caller a command acts for. `kanna` finds the caller's task and workspace from it.
- **Content commands take content IDs:** `send`, `read`, `send-key` and `restart` take a terminal ID (or its name); pane IDs are no longer accepted. **Layout commands** (`open`, `close`, `focus`, `rename-tab`, `move`, `move-tab`) act on layer 2 and take workspace, tab and content IDs.

## 4. Content

| Kind | Identity | Shared | Ends |
|---|---|---|---|
| Terminal | one program run; `restart` makes a new one | by every tab and app that shows it | closed, or its task done |
| Diff | folder + git args: opening the same diff again gives the same ID | one watcher for all subscribers | when the last subscriber leaves (no grace period) |
| File | path + kind (`md`, `code`) | as the diff | closed from its last tab |
| Page | one browsing session: URL, title, history | by the tabs that show it | closed |
| iOS | a simulator device and app; only a Mac app can show it | — | closed |

- **Diffs:** an app subscribes to `diff-2` and gets the diff and every change; the daemon runs one watcher per folder + git args and stops it when the last subscriber leaves. An app subscribes again on every (re)connect, so a restarted daemon starts the watcher again. A failed diff answers an error that the pane shows, never a blank page.
- **Content in a workspace** appears once per workspace: opening content the workspace already shows shows its tab and focuses it.

## 5. Where today's code mixes the layers

| Today | Mixes | Moves to |
|---|---|---|
| `Pane` (`crates/kanna-mux/src/pane.rs:94`) holds the program (`command`, `cwd`, `pid`, `state`, `exit_code`, `session`, `status`, `agent`, `hung_up`), the page (`url`, `title`, `history`), the file (`path`, `markup`, `comment`), the diff (`diff`), the device (`device`, `app`), and how it's shown (`zoom`, `view`, `line`, `name`) | 1, 2, 3 | content records per kind (layer 1); `name` → the tab (2); `line`, `comment` → hints on open (2); `zoom` (the text size) and `view` → the app (3) |
| `app_link.rs` `updated` (`:362`): the app writes `url`, `title`, `device`, `zoom`, `view`, `state` into the daemon's panes | 1, 3 | a page reports its URL and title as a content command; the text size and `view` stay in the app |
| `app_link.rs` `lend` (`:254`): the app hands its non-terminal panes back on hello, and `gone` (`:288`) drops them when it leaves | 1, 3 | content lives in the daemon whether or not an app is attached; nothing is handed back |
| `app_link.rs` `hello` (`:186`): one app at a time (`app_attached`) | — | any number of apps |
| `AppLayout` (`app_link.rs:532`) and `Core::layout` forward every layout step to the app, and `offline` fails them with `no_app` | 2, 3 | the workspace model runs in the daemon; no layout step goes to an app |
| `kanna-layout` / `LayoutOwner` in the app: the whole model, saved in `layout.json` | 2, 3 | `model.rs`, `owner.rs`, `desk.rs`, `tabs.rs`, `workspace.rs` move into the daemon, saved in `kanna.db`, with a workspace's windows as `win-N` holding tabs and no frame; the app keeps frames, torn-off tabs, its Desk and sizes in its own file |
| Terminals keyed by pane ID (`terminals.rs`, `terminals.json`, `<pane>.snap`, the app link's `attach`, `input`, `resize`, `status`, `folder`) | 1, 3 | keyed by terminal ID |
| `Backlog` (`backlog.rs`): `held` (a pane), `turns`, `quitting`, `setting_up` and status keyed by pane; `tasks.pane` in `kanna.db` | 1, 3 | keyed by the agent's terminal ID; the task records its workspace ID |
| `terminals_of` (`core.rs:349`) asks the app's layout which terminals are in a task, for Stop and Start | 1, 3 | reads the daemon's workspace; works with no app |
| Core finds a task by its agent pane (`core.rs:919`, `filter(id != task)`) | 1, 3 | by task ID → agent terminal |
| Diff watchers per pane (`diffs.rs`, started by `host.rs` only for panes the daemon opened) | 1, 3 | one watcher per folder + git args, with subscriptions |
| `Place` (`db.rs:75`): `Backlog`, `Shelf`, `Desk`, `Done` mixes queued-to-run with where it's shown | 1, 2, 3 | layer 1: queued, running, stopped, done; layer 2: shelved or not; layer 3: which Shelf task each app's Desk shows |
| A workspace's `where` (`{desk}`, `shelf`, `backlog`) and a Desk window's `shows` | 2, 3 | `shelved` in the daemon; `shows` per app |
| Free windows, the planning terminal as a Desk's `lead` | 2, 3 | content goes in a task's workspace or its project's planner workspace; every window is a workspace's |
| The app's terminal `settings` decide the daemon's colour replies | 1, 3 | the last app to send them, as today; kept on disk |

## 6. Decisions

1. **The daemon runs headless** and publishes every change to work and logical presentation; apps subscribe and render.
2. **Three layers** as in [1](#1-the-three-layers); an app changes layers 1 and 2 only through commands, like any client.
3. **Each app has its own Desk;** the Shelf and its limit are shared.
4. **Terminals:** a restart makes a new terminal with a new ID; `KANNA_TERMINAL` replaces `KANNA_PANE`.
5. **Diffs:** one watcher per folder + git args, shared, stopped when the last subscriber leaves.
6. **CLI:** content commands take content IDs, layout commands act on layer 2 with no app; pane and window IDs never reach the CLI.
7. **Stop and Start** hang up and rerun a task's tab terminals from its workspace; **Done** closes it.
8. **Ruled out:** the app as the only owner of layout (a task could then open tabs only by starting the app); the daemon storing workspaces as an opaque app blob (something still has to understand a layout to change it); a grace period for diff watchers.
9. **`kanna task pull` and `park`** act on the CLI's instance's app, starting it if none runs.
10. **kmux gets the same layers** ([7](#7-kmux)).
11. **A workspace has logical windows** (`win-N`, asked for by a task, realized by each app at a default position, then moved freely); an app may also spread tabs across windows of its own without the task knowing.
12. **Logical state is saved in `kanna.db`** with the tasks, so Done closes a task and its workspace in one transaction.

## 7. kmux

kmux gets the same split, through [the kmux subtree](kmux-subtree.md) as a kmux PR: its server keeps the content and the logical presentation, with content IDs in its protocol and CLI, and its app keeps windows and panes. kmux stays the reference model ([kmux is the reference](kmux-reference.md)), so its protocol cases change with it and Kanna passes the new ones.

## 8. Glossary

| Term | Meaning |
|---|---|
| **Content** | Something a tab shows that has an ID of its own in the daemon: a terminal, a diff, a file, a page, an iOS device. |
| **Zoomed** | Logical: one content fills its tab (`kanna zoom`), as the task presents it; every app shows it so. |
| **Text size** | Physical: how large one app draws a page or a terminal (⌘+, ⌘−); local to that app. |
| **Pane** | One app's view of content; local to that app. |
| **Planner workspace** | A project's workspace for content that's no task's: the planning terminal and what's opened from it. |
| **Subscribe** | An app asks the daemon for something's current state and every later change. |
