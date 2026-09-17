# Product Behavior

The user-facing surface of Kanna: task workflows, close semantics, sidebar
state, the diff viewer, keyboard shortcuts, and preferences. Read this when
changing UI flows or task lifecycle behavior.

For Kanna's purpose, current audience, product principles, decision rights,
and the boundary between current and proposed behavior, start with
[Product Context](product-context.md).

The *contracts* an agent must not break — core concepts, workflow and stage
semantics, and the MCP task-management rule — stay in the repo-root
[`AGENTS.md`](../../AGENTS.md).

### Workflows

**Create a task:**
1. ⇧⌘N → enter prompt (choose an available agent provider)
2. The frontend calls `kanna-server`, which creates the git worktree
   (`{repo}/.kanna-worktrees/task-{uuid}`)
3. …runs `.kanna/config.json` setup scripts if present (e.g., `pnpm install`)
4. …and asks the daemon to spawn the agent CLI in the worktree
5. Agent starts working. User watches in real-time terminal.

**Review and merge:**
1. Agent finishes → task marked as unread (bold in sidebar)
2. User selects task, presses Cmd+D → the general-purpose Diff tab shows all branch changes
3. Optionally Cmd+P → file picker → file tab, Cmd+O → open in IDE, or Cmd+J → shell tab in the worktree
4. Cmd+S → advance the workflow (commit post runs in-session; the pr-stage agent creates the GitHub PR and reports its URL)
5. Human reviews the PR through its preserved PR link, then uses the ordinary task stage-advance action. When the task's pinned workflow ships the `approve` post, that post hands approved work to the merge queue/master; pinned workflows without the post only advance. The workflow's existing single-flight and completion semantics remain unchanged.

**A task can grow its own workflow.** A workflow does not have to be chosen in
full before the task starts. The owner opens a `consultation`; when they choose
an outcome and ask to proceed, the task manager appends a manual `plan` stage to
**that same task** with `kanna_replace_task_workflow` and advances it. The
planning agent then chooses the review depth, agents, providers, and revision
budget the work actually warrants, and publishes the delivery stages in the same
`kanna_complete_stage` call that records the plan — the two are written in one
transaction, so a recorded plan is never visible without the stages it chose.
The task parks at its manual `plan` gate either way; appending stages runs
nothing.

- *Only what has not happened yet.* The stages already recorded must survive
  byte-for-byte, and the published suffix must follow an existing recipe —
  `in progress` (+`commit`) → `pr` (+`approve`), or the same with a `review`
  stage between. Each stage's agent and provider are the planner's choice, which
  is how one recipe covers both the ordinary `review` agent and the
  `qa-dispatcher` panel. This is a linear engine, not a workflow language.
- *The plan stays readable.* Kanna stamps `plan_context` onto the pinned
  definition and binds its result to the reserved `$PLAN_RESULT` prompt
  variable for every published stage and post — unlike `$PREV_MAIN_RESULT`,
  which each later main stage overwrites. It survives revisions, resume, and
  recovery because it lives in the pinned workflow, not in run history. An
  ordinary workflow edit carries the stamp forward and may not author or change
  it.
- *Callers fence on what they read.* `kanna_advance_stage` and the combined
  completion both take `expected_definition`; a tail that moved under the caller
  is a conflict, never a silently different workflow. The desktop projects a
  task's next stage from the task's own pinned definition, not from the repo
  file its workflow name resolves to — a grown task would otherwise read as one
  stage long and appear to close on the next advance.

**Revisions.** Sending a task back for revision follows these contracts
(engine code: `crates/kanna-server/src/task_creator/{stages,resume}.rs`):

- *Feedback is required — for every caller.* An agent-originated request with
  an empty prompt is refused (400) at the API boundary, before a revision
  round is spent or the review run is closed, so the reviewer can resend its
  findings. Every other caller — the human path included — falls back to the
  terminating run's recorded `feedback`, then to its result `summary`; if
  neither holds anything to act on, preparation is refused rather than
  started empty (and any claimed round is handed back).
- *Agent verdicts are bound to their review run.* The request adapter injects
  the immutable `stage_run` id stamped into the reviewer's spawn context.
  The server refuses a task/run mismatch, a stale or already-finished review
  run, or a missing id on a newly bound run before it closes a review or
  spends a revision round. Pre-binding legacy runs keep their compatibility
  path; an explicit human instruction can still be relayed through the agent
  tool path for recovery.
- *Revisions resume by default, provider-neutrally.* `request_revision`
  reopens the target stage's previous PTY agent session in that run's **own
  worktree** — Claude, Copilot, Codex, and OpenCode all resume when their
  recorded session/transcript preconditions hold (each CLI keys transcripts
  differently; the engine checks the recorded session id against the
  provider's own store and the run's cwd). Antigravity and headless SDK
  sessions cannot resume, and any failed precondition (missing transcript,
  worktree gone, tip diverged from the committed one) falls back to a fresh
  fork — with the reason recorded durably on the replacement run
  (`stage_run.resume_fallback_reason`).
- *Rounds are budgeted.* A workflow's `revision_limit` defaults to **5**
  (`0` = unlimited) and counts only *agent-requested* revisions. Once spent,
  an agent's `request_revision` starts nothing: the review verdict is still
  recorded, the task parks `unread` at its current stage, and the response
  carries `revisionBudget.exhausted: true`. A *human* revision bypasses the
  budget and **resets** the count — but only the budget: it is still subject
  to feedback resolution and every other preparation precondition.
  The human origin is a caller declaration, not authenticated identity, and an
  agent may use it only to relay an explicit human instruction from its
  terminal. The desktop shows the exhausted state but has no reset action.
  This is also the documented recovery when a human decides a task should get
  another pass after an automatic round was consumed incorrectly; no separate
  counter-repair operation exists.
- *The task's terms live in its original prompt and durable delivered-input
  history.* Later review stages assess the branch against the prompt plus the
  owner, manager, and reviewer directives recorded by `kanna_task_inputs`.

**Manual intervention:**
1. Cmd+J → shell tab opens in the task's worktree
2. Run tests, inspect files, debug
3. Close shell → focus returns to agent terminal
4. Type in the agent terminal to send input to the running provider CLI

### Task reference views

A task opens with its provider TUI using the full work area. Each pane has its
own tab bar, directly above its contents. **+** opens a menu for diff,
terminal, file explorer or commit graph. Right-click a pane’s tab strip (or
focus it and press Shift+F10) for splitting side by side or top and bottom,
with a pane-specific **×** to close that pane. Splitting a pane with multiple tabs moves its selected reference tab into the
new pane; splitting the Agent view or a single-tab pane opens an empty destination. Drag any tab,
including Agent, onto another pane's tab bar or an empty pane. Dropping before a
tab reorders it. Empty source panes collapse after a tab moves or closes.
Dividers resize with the pointer or arrow keys. A pane’s **×** removes only
that pane, moving its tabs to its adjacent sibling while preserving other splits
and terminal sessions. The last pane cannot be removed. Tabs use pointer dragging
with insertion markers; stage dropdowns and close buttons do not initiate drags.
The **+** menu shows each content command’s existing keyboard shortcut.
Open a file with **⌘P**; it has no separate + menu item.
The Agent tab shows its stage name followed by a dropdown arrow; tab labels
cannot be text-selected. Modified bracket shortcuts cycle in displayed pane/tab
order after moves. Right-click a tab for **Close all right**, which closes only
reference tabs to its right in that pane and preserves Agent. Insertion markers
also show the append position after the last tab and in empty panes.

Narrow work areas (under 800 CSS pixels) show one readable view and a combined
tab bar without discarding the saved split layout. Select the **Agent** tab
to return to the provider terminal. The focused view owns shortcuts, including
the editor's existing save/quit semantics. Task id, owning machine and branch
remain visible. Launch provider/model metadata and the Next stage model control
are absent from this header. An older editor tab labels its original workspace
when the task has moved on.
Selection, pane membership, tab ordering and split proportions are stored per task,
separately from repository and app tabs. File reading offsets and diff scope,
branch inclusion, and reading offsets restore on task return and app restart
for the same workspace. Positions are best effort if content changes. A new
workspace resets those reading positions. Live terminal continuity comes from
surviving daemon sessions; unavailable sessions and stale remote file snapshots
are not restored by tab persistence.

The Agent tab's stage selector offers Latest and dated historical attempts.
Earlier attempts show read-only terminal-retained scrollback (including the
final alternate screen) and the observed exit status, or an explicit unknown
status. Repeated attempts remain separate. Uncaptured legacy history is marked
unavailable; saved stage summaries do not stand in for terminal output. Selecting
history never starts or attaches to an ended process. Latest returns to the
same current terminal. Continued posts share a process's launch identity;
retries and fallback launches receive distinct identities.

A local task's claimed port opens a reference preview after resolving its
current workspace and port against the owning server. **Open in browser**
retains full browser and DevTools access. The task server must be running and
allow embedding. All visible previews and a bounded cache of hidden previews remain mounted during task/view switches,
retaining their page state; app restart or cache eviction reloads the page.
Only the port name is persisted, never a page snapshot or credential. Embedded
previews are local desktop only; mobile and remote preview transport are
unchanged.

The compact sidebar offers **New task**, **Unread**, and **Questions** over the
existing task list. Unread uses the existing read state; Questions requires a
positively detected waiting prompt. Row tooltips explain each reason. Filters
retain repo pins, nesting and stage order, and create no new lifecycle state.

### Editing a local task file

File previews offer **Edit**, followed by a visible terminal-editor choice.
Preferences → **Terminal Editor Command** pins an installed terminal tool (for
example `nvim` or `emacs -nw`); empty means detect available editors. Commands
accept quoted arguments, without shell expansion. `VISUAL` and `EDITOR` are
hints only when they name a recognized terminal editor. Graphical editors keep
using the separate **IDE Command** / **Open in IDE** action. No editor is bundled
or downloaded, and missing/invalid choices are explained in the picker.

Each editor is a separate daemon-backed terminal, leaving the agent TUI intact.
Use the editor's native save/quit commands; Cmd+S does not save or advance while
an editor tab is active. Use the existing tabs to return to the agent, where
Cmd+S retains its existing stage action. Preview/citation navigation remains
read-only, including `kanna_open_view`.

Switching tasks, hiding a tab, and restarting the app reattach the same session.
A stage change leaves the editor in its original workspace; it never moves into
the new stage. Only committed changes cross stage boundaries. Closing the task
ends all of its editor sessions, including hidden
ones and older workspaces, and loses unsaved buffers. Save and quit first.
An ended/missing editor is not automatically restarted; open it explicitly from
a file preview again. The editor owns buffers and saving. Kanna does not inspect
unsaved buffers or prevent simultaneous agent/human writes.

Editing is local desktop only. Remote previews are labelled read-only and do
not open against matching local paths. Editors in a task transferred away are
not remotely transported or resumed by this integration.

### Viewing a terminal from more than one device

The PTY has one authoritative grid. The first measured visible viewer seeds a
session that has no active owner, without becoming an owner itself. After that,
the terminal viewer with the most recent deliberate terminal activity owns
sizing. Typing/direct input and active scrolling (wheel/trackpad, touch,
scrollbar, or Shift+PageUp/PageDown) claim or refresh that viewer. Merely opening
or focusing a task/window/app does not, and key-window blur does not withdraw a
still-rendered desktop viewer. Output auto-scroll and replay never claim
control. Rotation, keyboard visibility, resize, and layout also do not claim;
they can only update the existing owner's grid. Hidden, inactive, or zero-size
viewers cannot steal sizing. A reconnect re-registers and rehydrates from the
authoritative snapshot without stealing control.

**Multi-repo:** Import repos via sidebar. Each repo has its own task list. Cmd+Opt+Up/Down navigates tasks in sidebar order.

### Closing a task (⇧⌘⌫)

Close is refused (409) while the task has open subtasks — close or detach them
first. Otherwise:

1. Kills the agent PTY, shell, and all task editor sessions in the daemon
2. Runs workspace teardown commands best-effort when configured
3. Sets `closed_at` in the DB
4. Snapshots dirty state in each of the task's worktrees with a local `WIP at task close` commit, then removes those worktrees with `git worktree remove --force --force` and prunes worktree registrations
5. Deletes the task's `worktree` table rows but keeps the task row and all branches. Branches are never deleted by close.
6. Selects the next task in the sidebar
7. Tasks with `closed_at` are hidden from the sidebar. The sidebar shows tasks whose `closed_at` is null.

Close also delivers blocker-close instructions to dependent tasks' sessions
and starts dependents the close unblocked. Managers observe the durable
`task.closed` event through `kanna_wait_events`; close does not inject input
into another task's terminal.

On server startup, Kanna reconciles leftovers across all repos, including hidden repos: closed-task worktrees are snapshotted and removed, stale registrations are pruned, and young orphan `task-*` directories without a task row are spared. This bounds registered worktrees by construction to roughly the number of open tasks. The bound matters because each registered git worktree expands sandboxed agent shell spawn profiles; unbounded worktrees can overflow macOS `ARG_MAX` and cause sandboxed shell launches to fail with `E2BIG`.

### Task activity

`activity` is a *display* value that blends two independent dimensions —
what the agent process is doing (`runtimeState`: `busy` | `waiting` | `idle` |
`exited`, the daemon's terminal-state verdict) and whether the latest output
has been read (`readState`: `read` | `unread`). Task detail reports all three;
supervision reads `runtimeState`, because a busy task nobody has read carries
`unread` exactly like a finished one.

| Activity | Meaning | Sidebar display |
|-------|---------|-----------------|
| `working` | The daemon judges the session busy | italic |
| `idle` | Stopped, and read (or selected) | normal |
| `unread` | Latest output not yet read — finished *or* still busy | bold |

Sidebar order: pinned (manual `pin_order`) → unpinned unblocked tasks grouped
by workflow stage in the repo's `stage_order` (default `pr` → `review` →
`in progress` → `plan` → `consultation`; unknown stages last), newest first within each group → blocked
(newest first). Subtasks nest under their parents (suppressed while
searching).

### Pinned tasks

Tasks can be pinned to the top of their repo's task list by dragging above the pin divider. Per-repo scope. Closed tasks disappear regardless of pin state.

**Account-wide singletons are pinned by default, on every machine.** A repo's
Merge Master and Task Manager are one task across the account (see the relay
singleton directory), so every machine's list shows them pinned without the
operator pinning them there. The owning machine stamps the pin on its own
`pipeline_item` row once, when it claims the singleton — top of the pinned
group, shifting the operator's existing pins down rather than renumbering
them. A machine that only *views* the task has no row to stamp, so it derives
the default from the singleton identity the owner publishes (`singletonAgent`,
read back from the `singleton-{agent}` workflow name bound at claim time).

It is a default, not a rule. An explicit unpin always wins and always sticks:
on the owner's machine as the cleared `pinned` column (a reclaim of an existing
singleton never re-stamps it), on a viewing desktop as a `null` entry in the
viewer-local `remoteTaskPins` overlay, and on mobile as an entry in the phone's
own `unpinnedDefaults`. Absence of a pin is not the same as an unpin, which is
why the last two are recorded rather than simply left out.

### Diff viewer

- Main-area tab (Cmd+D), retained with the task's other open views
- Scopes: Branch (all changes since merge-base with the configured base), Working (uncommitted)
- Staged toggle to filter staged-only changes
- Scope remembered per task
- `]` / `[` cycle the Diff scope forward / backward while the Diff tab is active; the platform-mapped modified brackets remain global next / previous tab
- Rendered by `@pierre/diffs` with shadow DOM, syntax highlighting via worker pool

### Keyboard shortcuts

| Shortcut | Action |
|----------|--------|
| ⇧⌘N | New task |
| ⌘N / ⌘W | New window / close active tab (or window when no view tab is open) |
| ⌘D | Open or focus Diff tab |
| ⌘J | Open or focus task shell tab |
| ⇧⌘J | Shell at repo root |
| ⌘P | File picker |
| ⌥⌘P | Toggle file preview |
| ⌘O | Open in IDE |
| ⌘L | Open latest file link |
| ⌘S | Advance stage (runs the stage's post first; blocked while a post is running) |
| ⇧⌘⌫ | Close task |
| ⌥⌘↑/↓ | Navigate tasks |
| ⇧⌘↑/↓ | Navigate repos |
| ⌘U / ⇧⌘U | Oldest unread task (repo / all repos) |
| ⌘R / ⇧⌘R | Oldest read task (repo / all repos) |
| ⌘F | Focus search |
| ⌘I / ⇧⌘I | Create repo / import repo |
| ⌘B | Toggle sidebar |
| ⌘G | Commit graph |
| ⇧⌘P | Command palette |
| ⇧⌘E | Tree explorer |
| ⇧⌘Enter | Toggle maximize |
| ⇧⌘A | Analytics |
| ⇧⌘[ / ⇧⌘] | Previous / next main-area tab |
| [ / ] | Previous / next Diff scope (Diff tab only) |
| ⌘/ | Keyboard shortcuts |
| ⌘, | Preferences |
| ⌃- / ⌃⇧- | Back / Forward |
| Escape | Dismiss the top dialog or active non-terminal view tab |

The registry in `apps/desktop/src/composables/useKeyboardShortcuts.ts` is the
single source of truth for app-level shortcuts. View-local shortcuts are
listed by their active Diff, file, tree, or graph context in the shortcut help.
Main-area tabs stay mounted while hidden, so only the active view may consume
its local keys; registered app shortcuts continue to work while a terminal has
focus.

#### Linux

The table above is authored for macOS. `shortcutPlatform.ts` transforms it for
Linux — ⌘X becomes Ctrl+Shift+X, ⇧⌘X becomes Ctrl+Alt+X — and holds a named
exception table for the chords that transform cannot reach, because on a stock
GNOME desktop something else already owns them. Every exception below was
verified against `gsettings list-recursively` on Ubuntu 26.04 and then pressed
on the running app.

| Action | Linux chord | Why it is an exception |
|---|---|---|
| Previous / next main-area tab | Ctrl+Page Up / Ctrl+Page Down | The transform would give ⇧⌘[ / ⇧⌘] an unguessable Ctrl+Alt+[ / ]; this is what GNOME Terminal, Firefox and VS Code all cycle tabs with. Listed in the shortcuts modal on Linux, unlisted on macOS. |
| Back / Forward | Alt+← / Alt+→ | ⌃- / ⌃⇧- carry across unchanged onto the webview's zoom-out chords. Alt+Arrow is what a GTK app is expected to answer anyway. |
| Oldest unread task (repo / all repos) | Ctrl+Alt+U / Ctrl+Alt+Shift+U | Ctrl+Shift+U is IBus' Unicode code-point entry. It never reaches the webview — pressing it types a literal "u" — so the app cannot win it back by handling it. |
| Previous / next task | Ctrl+↑ / Ctrl+↓ | The transform's Ctrl+Alt+↑/↓ is GNOME's workspace switcher, and both other candidates are dead: keys injected below the compositor for Alt+↑/↓ and for Ctrl+Shift+↑/↓ deliver their modifiers to the webview and never the arrow. Ctrl+↑/↓ arrives and nothing claims it. |
| Previous / next repo | Alt+Shift+↑ / Alt+Shift+↓ | Same three dead chords. This keeps repo navigation one modifier above task navigation, the way ⇧⌘↑/↓ sits above ⌥⌘↑/↓. |

Three further rules keep the Linux keymap honest, all enforced by
`shortcutPlatform.test.ts`:

- No binding takes a plain Ctrl+letter. Those belong to the agent session's
  PTY — Ctrl+C is SIGINT, Ctrl+V is readline's quoted-insert — which is why the
  terminal's clipboard chords are Ctrl+Shift+C / Ctrl+Shift+V.
- No binding claims a chord GNOME has already taken (Ctrl+Alt+Arrow switches
  workspaces, Ctrl+Alt+Backspace zaps the X server), and none sits on a zoom
  chord.
- No ↑/↓ binding sits on Ctrl+Alt, Ctrl+Shift or plain Alt. `gsettings` names
  an owner only for the first; the other two were measured not to arrive, which
  settles the question either way — a chord the desktop does not deliver cannot
  be bound, whoever is taking it.

**Selection chords belong to the text field, not to the app.** The global
listener captures keydown before anything else and calls `preventDefault()`, so
any chord it claims is gone from every input, textarea and contenteditable in
the app — which is how Ctrl+Shift+← , *the* word-selection chord, stopped
selecting anything in the task search field. `belongsToTextEditing` concedes a
keystroke to the field it landed in when Shift is held over a caret key. Only
that: the deletion keys are not caret keys, so Ctrl+Shift+Backspace still
closes a task from a focused composer rather than silently doing nothing;
Ctrl+Shift+S means nothing inside an input and still advances a stage from one, and xterm's hidden helper textarea is deliberately not an
editable element — what is typed into it goes to the PTY, and navigating tasks
from a focused agent terminal has to keep working.

Ctrl+Shift+←/→ is pane focus, which does nothing without a split; its modal
label says "(split view)" so the list cannot be read as a promise about tabs.

Plain Ctrl+↑/↓ is deliberately *not* conceded to a focused text field even
though GTK moves the caret by paragraph with it. It is task navigation here,
and typing two characters into the sidebar search and then walking the results
is what the field is for. Nothing is selected, so nothing is lost.

Which Shift chords a field owns is a platform question, and conceding one it
does not own is not free: the agent view's composer holds the caret nearly all
the time, so a chord conceded there does nothing in the view a person spends
the day in — which is how repo navigation measured dead on Linux even after it
moved to a chord the desktop delivers. A GTK or WebKit field builds a selection
out of Shift and Ctrl only, so on Linux `belongsToTextEditing` concedes
Shift+caret while neither Alt nor Super is held: Ctrl+Shift+← still selects a
word, and Alt+Shift+↑/↓ still changes repo from inside the composer. macOS is
unchanged and concedes every Shift+caret chord, because ⌥⇧← and ⇧⌘↑ really are
selection there.

`apps/desktop/tests/e2e/linux/` drives these chords into the running app as
real key events, which is the only layer that can tell "the app ignored it"
apart from "the desktop ate it" — see that directory's README for how to run it.

### Preferences

| Setting | Default |
|---------|---------|
| Suspend After (minutes) | 5 |
| Kill After (minutes) | 30 |
| IDE Command | code |
| Terminal Editor Command | empty (detect installed terminal editors) |
| Locale | en |
| Default Agent Provider | claude |

Stored in SQLite `settings` table.

## Mobile app

The connection model and data paths are in
[Architecture](architecture.md#mobile-app--appsmobile--packagesstream-client);
this is the user-facing surface.

**Structure.** Three tabs — **Tasks** (repo-scoped, with a repo chip row),
**Activity** (unread, not-locally-dismissed tasks across repos), and **More**
(repo commands) — behind a floating toolbar that also carries Search and
"Add task". Tapping a row opens the full-screen task detail: agent view or
terminal, a composer with quick replies and photo attachment, and file/diff
previews. Account, the Machines list, and the Quick Replies editor live in a
modal account sheet; there is no settings tab.

**Task cards** are tinted by workflow stage from the app-icon palette
(`in progress` orange, `review` purple, `pr` green, `consultation` blue;
custom stages hash onto a fixed sub-palette; blocked is a rose badge on top of
the stage color). A short task id sits beside the truncating title; unusually
long IDs middle-ellipsize in their bounded metadata column so the title remains
legible. Up to three lines of the task's latest output snippet render on the card — live via
the KSP task-summary stream while connected, falling back to the resting
(Firestore) snippet on disconnect.

**Pins and dismissals are phone-local.** Swiping a row left and releasing past
the threshold commits the action (pin/unpin in Tasks, dismiss in Activity);
releasing inside the threshold cancels — there is no revealed-button state.
Pins order to the top of the repo list; a dismissal hides the task from
Activity until newer activity arrives. Neither is published to the desktop,
and a mobile dismiss never marks the task read for the desktop or
supervisors. Account-wide singletons are pinned by default here too, above
the phone's own pins — see "Pinned tasks" above.

**Pairing.** A desktop is added by scanning its QR code (or typing the 6-char
code): the phone finds the desktop via Bonjour on the LAN and claims the
pairing session over HTTP, receiving a device credential that is independent
of the signed-in account (manually paired machines survive sign-out; signing
out does drop the account's machines). After pairing, the app immediately
loads the new machine's work and shows the wait. Machines shows every desktop
merged from the account, manual pairing, and live LAN discovery, grouped by
availability.

**Photo attachments.** The composer can attach one photo (library or camera)
to a task input; images are resized/re-encoded to JPEG within a 3 MiB budget.
The control appears only when the task's own desktop advertises attachment
support (asked once per task and route), and permission denials explain
whether to retry or open Settings. The desktop stores the image outside the
worktree and appends `[Attached image: <path>]` to the injected input.

### Agent-requested task attention

An agent can place one `!` before a desktop sidebar task title using Kanna's
set/clear attention tools. The tooltip and accessible label give the short human
action requested. The existing task row opens the task as usual. The annotation
does not affect sorting, pins, unread state, detected questions, or navigation.
It persists across restarts and stage changes until an agent explicitly clears
it, including on owner request. Selecting a task does not dismiss it.
