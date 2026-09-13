# Task reference UI verification

Task `0e269a87` implements the accepted optional-adjacency direction from
consultation `4be3a1a5`. The fork is `1f580ab8e`, which already includes terminal
editor PRs #1469 and #1473. Their settled Edit label and native save/quit contract
remain intact. Manager-curated sidebar planning in task `dbd6d34b` is separate.

## Durable review terms — owner directive

Independent review may proceed now. **HOLD final approve/merge until the owner
completes a bounded test-drive of this new split UI**, covering physical
iframe-click focus and split-pane usability. Synthetic WebDriver pointer events
and explicit WebKit focus calls do not establish either physical-click behavior
or owner usability acceptance. Record the owner's outcome in the durable task
review record before releasing that hold; an agent's test result is not a
substitute.

This directive does not reopen the already-approved terminal editor, require a
broader feature scope, or create a blanket full validation gate. Commit-post
success records repository bookkeeping only and does not imply owner acceptance
or merge authorization.

## Boundaries

Desktop tab descriptors persist through the existing preference/server path;
no DB migration, daemon protocol, workflow state, or mobile navigation changes.
Selected view and reading positions are task/workspace scoped. Remote file
snapshots remain excluded from persistence. Editors keep their recorded session
and original workspace. Visibility and shortcut focus are separate for adjacent
terminals and diff readers.

Embedded preview resolves the task through the existing local-only task-detail
API and checks its current workspace and claimed port. It uses the existing
local HTTP target; no relay or remote preview transport was added. A bounded
five-page cache preserves in-page state during switches, discarding closed or
superseded workspaces. Restart reloads pages. Browser access remains available.

## Actual native evidence

Started only this worktree with `./kd dev up`, test DB
`kanna-test-ui-0e269a87.db`, and the existing E2E test-SQL/foreground options.
Before interaction and after webview reload, the isolated WebDriver endpoint
`http://127.0.0.1:4475` verified:

- Native title: `Kanna — task 0e269a87 (0.0.68 @ 1f580ab8e)`.
- Build task, branch and worktree: `task-0e269a87` / `0e269a87`.
- Version `0.0.68`, base commit `1f580ab8e`, with this worktree's frontend changes.

`tests/e2e/real/task-reference.test.ts` exercises the native webview, real server,
and daemon PTYs. Harmless `/bin/cat` supplies agent-terminal bytes; `/usr/bin/vim`
performs an actual file edit/save in the original fixture workspace. It checks:

- Full-width initial agent, optional adjacency, explicit full width and narrow
  single-view layout.
- File and diff reading positions across task switches and webview reload.
- Return-to-agent focus, explicit WebKit iframe focus ownership, and unchanged
  agent/editor PIDs across task switches.
- Vim saving and editor Cmd+S remaining outside the workflow-advance context,
  including no stage-post dispatch.
- Preview routing through an API-claimed task port and no iframe reload across
  a task switch.

Screenshots and machine-readable evidence are under `.tmp/task-reference/` in
this worktree. Foreground verification confirmed painted terminal text; earlier
background captures had blank xterm rendering because WebKit reported the window
hidden. These are native screenshots, not mock renders. No installed production,
staging, or owner test-drive window was used. Fixture PTYs and HTTP server
were cleaned up, then `./kd dev down --kill-daemon` stopped the owned stack;
ports 1433, 4475 and 48133 had no listeners and the task tmux socket was gone.

The new regression found and fixed two ownership gaps: hidden diff offsets
could be overwritten during reload, and editor Cmd+S could dispatch a post while
the stage still appeared unchanged. The strengthened checks cover both.

The WebDriver plugin implements pointer actions as synthetic MouseEvents, which
do not transfer browser focus into a cross-origin iframe. The iframe check uses
WebKit's focus API explicitly; physical mouse entry into the embedded page and
subjective split-pane feel remain owner test-drive checks.

This evidence is automated native verification, not human acceptance. Reload
coverage is a webview restart plus persisted preferences and surviving daemon
sessions, not an OS reboot. Provider-specific TUI behavior, physical mobile
behavior and off-network preview transport were not claimed or expanded.

## Targeted checks

303 tests across 18 Vitest files pass, covering tab persistence, workspace invalidation, sidebar reasons/filtering,
port routing, active-view shortcut ownership, local and remote terminals, editor
attachment, and diff visibility/scroll behavior. Desktop `tsc --noEmit` and
`vue-tsc --noEmit` pass. `git diff --check` is clean. The native scenario is registered in the unattended real
E2E tier for future regression runs; explicit foreground painting checks are
optional under the existing `KANNA_E2E_NO_ACTIVATE=0` test setting.

## Owner-directed revision, 2026-09-13 (not merge approval)

The owner requested removal of launch provider/model metadata and Next stage
model, a + menu in place of Diff/Files/Shell buttons, an Agent dropdown for
earlier-stage output, and user-created splits with movable tabs. This supersedes
the evaluation-only/no-code limit for those changes. It does not approve merge.
Manager-curated sidebar task `dbd6d34b` remains separate.

The local layout revision gives every pane its own tab bar; supports side-by-side
and top/bottom splits, draggable tabs (including Agent), resizable dividers,
joining panes, and task-scoped persistence of geometry and tab membership.
Content nodes stay mounted in one flat host while their geometry changes, so
moving an iframe does not reparent/reload it and moving a terminal does not
recreate its session. Narrow windows show a combined tab bar and one view.

History investigation found task `09b6a82a`, “Separate terminal and agent output
per stage,” closed after a recorded approval/handoff on September 11. Its PR
#1432 remains open at `200e535f7920c5a57e392a3e2b59694d1a8ba343`; it adds durable
per-attempt terminal records and archived final frames. It has been inspected
read-only, not merged or copied wholesale. At the investigation baseline this
branch had saved stage results, not that archive API. The owner resolved the choice on September
13: one Agent tab selects Latest or a previous stage attempt's full terminal
scrollback, with the actual recorded termination status. Historical views must
not attach, restart, resize, or send input to an ended session. No summary is
presented as a terminal transcript and no mobile stage navigation is included.

### Confirmed archive gap and bounded extraction proposal

Read-only inspection of the current source and held head `200e535f` found:

- Current `http_api/task_logs.rs::render_persisted_stage_run_logs` renders
  results/feedback, not terminal bytes. `db/stage_runs.rs::list_stage_runs_for_task`
  provides run identity but no archived VT or process exit code.
- Current daemon `connection.rs::Command::Snapshot` reads a live or recovery
  snapshot by session ID. It does not accept a stage attempt identity. Reusing
  that task-level ID cannot reliably retrieve each previous attempt.
- Held PR #1432 has `db/terminal_sessions.rs` attempt records with stage-run ID,
  nullable exit code and archive availability; its archive route checks task
  ownership and prefers the terminal record ID over the reused daemon ID.
- Its `terminal_watcher.rs::archived_terminal_frame_over` imposes a 256 KiB
  trailing slice on serialized VT. Copying that helper unchanged would lose
  retained scrollback and can begin within a VT sequence. It does not satisfy
  the owner's request for the full retained terminal history.

The bounded backend proposal is to extract only agent-attempt archive identity,
capture-before-replacement and natural-exit capture from that work, with the
associated ownership-checked list/read routes. Preserve the complete snapshot
already retained by the existing headless terminal instead of the 256 KiB slice;
this is retained terminal scrollback, not a promise of unlimited process output.
Persist the observed exit code or explicit unknown termination, never infer zero
from a successful stage result. Bind capture to the outgoing attempt before a
replacement can reuse its daemon ID; retries must remain separate entries.
Old attempts whose bytes were never archived remain explicitly unavailable.

The desktop slice then needs a read-only snapshot renderer inside the existing
Agent pane and a stage/attempt selector with Latest returning to the current
terminal. It must have no session creation, stream attachment or input path.
Do not extract setup/teardown separation, mobile navigation, additional agent
tabs, or sidebar changes. This was a concrete backend prerequisite, not an
unresolved owner preference. The implementation below supersedes this proposal.

Targeted verification for that extraction must cover two attempts reusing one
session ID, natural exit and replacement capture, nonzero/unknown exits, missing
archives, output exceeding 256 KiB, archive task ownership, and switching back
to the same live terminal without historical input or restart. No new tests or
UI interactions were performed during this read-only gap investigation. Existing
workspace-layout test evidence below remains separately scoped.

The extraction was authorized in the subsequent owner coordination. Further
lifecycle tracing found a narrower contract decision that the held mechanism
does not settle:

1. At held head `200e535f`, `capture_finished_agent_attempt` asks for a snapshot
   by session ID and then reads `latest_stage_run`. `RecoveryManager::start_session`
   removes the archive for that same ID; `Command::Snapshot` prefers a live
   session. An independently initiated replacement can therefore make an old
   Exit's lookup return a new process's screen/run. Capturing before the watcher's
   own completion handler prevents its automatic transition from racing, but
   does not fence an independent replacement. `Event::Exit` has no incarnation
   or immutable archive identifier to check. This is a source-level race finding,
   not a claim that a native reproduction was run.
2. Current `output.rs` uses `session.try_wait().await.unwrap_or(0)`.
   `pty.rs::try_wait` returns None for adopted children, already-reaped children,
   and unavailable wait status. Consequently the existing Exit code alone
   cannot distinguish observed zero from unknown. Copying it into an archive
   would violate truthful termination for those paths.

Proposed narrow extension: capture an immutable attempt-addressed final archive
from the exiting session itself, retain nullable observed process status in that
archive, and carry an archive/attempt identity across the daemon-to-server read
boundary so the server cannot substitute the current session. Bind the archive
to the launch identity before spawn; injected posts sharing a process share that
attempt, while a fallback spawn or retry receives a distinct identity. Preserve
existing live-terminal and completion semantics; this metadata is for historical
attribution. Unsupported/legacy evidence must be reported as unavailable/unknown.
This requires extending the held mechanism's daemon archive contract, rather
than copying its session-ID-only lookup. The manager subsequently accepted architect consultation `65077f33` and
released this implementation hold; the full verdict was read before editing.

Prior QA applies to `9b8252b1b`, not these edits. New targeted native evidence
uses the same verified development window, title:
`Kanna — task 0e269a87 · task-0e269a87-5 · persistent-task-reference-split (0.0.68 @ 9b8252b1b)`.
That stamp names the native base; the window is serving the local frontend
revision from this worktree through Vite. The native preparation explicitly
reloads and re-verifies identity after HMR before driving the new controllers.
The disposable task `8ebe0aa7` and preview on port 5189 remain available for the
owner's evaluation. The production/staging apps were not used for UI testing.

`.tmp/split-eval/check-panes.ts` exercises the + menu, creates nested panes,
drags Agent through browser DragEvents, resizes through the layout controller,
and persists the result. It checks unchanged iframe element identity and equal
real daemon session inventories/PIDs before and after movement. The inspected
native screenshot shows preview on the left, Agent upper right, file lower
right, each with its own tab bar. Synthetic DragEvents prove handler wiring,
not physical trackpad/mouse behavior. Pointer capture resizing and subjective
usability still need owner evaluation. These checks do not replay the earlier
full fixture or claim fresh Vim-save, mobile, or remote-transport verification.

Owned stack inventory remains `.kanna/kd-state/process-inventory.json`, tmux
`kanna-task-0e269a87-5`, WebDriver 4475, development server 48133, and preview
Python PID 10115 / exec session 18713. Leave it available under the owner's
explicit instruction; stop the owned preview and use `./kd dev down --kill-daemon`
in this worktree when the owner finishes. The revised UI must be independently
reviewed and owner-evaluated before PR #1477 may merge.

The new targeted set passes 103 tests across eight files (pane model, tab and
layout persistence, MainPanel focus wiring, TaskHeader, + menu/drag scoping,
preview cache continuity, and keyboard actions). Desktop `tsc --noEmit` and
`vue-tsc --noEmit` pass. A native reload check caught an initial Agent focus
no-op creating a scope before hydration; that no-op now leaves storage
restoration untouched and the persistence test covers it. The repeated native
check confirms equal saved layout before/after reload, all three tabs reachable
in a narrow single-view layout, and three panes restored on widening.
The isolated fixture dismisses its startup shortcuts overlay via the dev bridge;
that setup step is not evidence of shortcut-overlay keyboard behavior.


## Accepted attempt archive implementation (September 13)

Architect consultation `65077f33` approved the narrow archive contract and its
manager released the hold. The implementation stays on PR #1477. No new agent,
mobile route/navigation, setup/teardown tab separation, task transfer change,
sidebar revision, approval, or merge is part of this follow-up.

- The server records PTY launch bindings before Spawn. Each new launch uses its
  existing immutable launch-run ID; injected posts keep that process identity.
- The daemon freezes that binding in the PTY and carries it through same-machine
  handoff. Finalization captures from the exact handle under existing reuse
  fences. Archives preserve the terminal-retained VT, geometry and cursor
  metadata; neither of PR #1432's 256 KiB slices is used. Degraded serialization
  produces explicit unavailable evidence, never a lossy replacement snapshot.
- Nullable observed exit status is cached separately from legacy completion
  signals. A List/try_wait observation survives consumption; adopted/unobservable
  exits and synthetic Kill intent do not become successful process exits.
- Attempt-addressed files are atomically published, immutable and idempotent.
  Known-attempt HTTP reads reconcile them into server-owned SQLite with exact
  task/run/session/workspace ownership checks. A daemon copy is released only
  after the DB transaction commits; a lost release retains a safe duplicate
  until owned daemon-directory cleanup. Server archives follow stage-run/task
  deletion via the foreign key and survive ordinary workspace teardown.
- The Agent tab offers Latest and distinct dated attempts. Historical output is
  parsed locally into inert selectable text, including primary scrollback and
  the final alternate screen, with observed/unknown exit below. There is no
  historical spawn, recovery, attach, input or backend resize path. Latest
  preserves the mounted live terminal. Missing legacy history remains explicit.

Focused verification (new code, separate from previous layout QA): real-daemon
same-ID A/B capture above 256 KiB, natural exit 7, Kill with unknown observed
status, adopted PTY identity with unknown exit, cached 0/nonzero observations,
immutable persistence/conflicts/unsafe IDs, DB ownership/reopen/write failure,
and real-daemon -> server DB -> authorized HTTP with delayed consumption after
replacement and reads after server-state recreation/daemon shutdown. Desktop
checks cover repeated stages, zero/nonzero/unknown/missing states, stale fetches,
task changes, both retained terminal buffers above 256 KiB, and the same live
terminal component after Latest. Final commands/results are recorded below.

The initial real-daemon server fixture selected a stale Cargo artifact and
correctly rejected the unsupported archive command; it was corrected to use
this worktree's canonical `.build/debug/kanna-daemon` output, and passed. This
was an isolated test daemon, never installed Kanna or the owner evaluation app.

The owner's evaluation stack was preserved (`./kd dev status`: running). No
native interaction or native restart was performed for this archive revision.
The archive wiring has real daemon/server HTTP and desktop component evidence,
but the revised selector has not yet been exercised end-to-end in the owner's
native window. That window's backend must be refreshed through the owned `kd`
workflow when coordinating the next owner evaluation. Fresh independent review
and owner acceptance still precede merge; earlier QA on `9b8252b1b` is not an
approval of these edits.

Final focused results:

- Daemon archive/status unit tests: 2 passed.
- Real-daemon same-ID/large-output and archive-write-failure tests: 2 passed.
- Real adopted-PTY archive test: 1 passed.
- Server archive DB, ownership routes, legacy-daemon behavior and real-daemon
  HTTP reconciliation tests: 4 passed.
- Before-Spawn durable binding and continued-post identity tests: 2 passed.
- AgentHistoryView, renderTerminalArchive, MainPanel and MainTabBar: 39 Vitest
  tests passed. TypeScript, Vue and desktop Rust compilation passed.
- `git diff --check` passed. Test-owned daemon fixtures shut down through their
  scoped guards. The separately authorized owner evaluation stack stays up.

Commands/logs are retained in `.tmp/archive-work/` (task-local, not committed):
`daemon-unit-final.txt`, `archive-reconnect-final.txt`, `handoff-test2.txt`,
`server-final.txt`, `prebind-test.txt`, `post-test.txt`, `ui-tests-final.txt`,
`ts-check-final.txt`, `vue-check-final.txt`, and `desktop-check.txt`.
