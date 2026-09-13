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
