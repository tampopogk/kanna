# The agent's first output now arrives as stream, not snapshot — 2026-09-09

## What changed

A launch runs the repository's setup in a startup terminal and starts the agent
only when that shell exits cleanly, so a task exists for as long as setup takes
before its agent session does. The desktop mounts the task's agent terminal view
when the task appears — during setup — and attaches to a session id the daemon
does not have yet. That attach is *accepted* rather than refused: the stream
binds, and everything the agent later prints reaches the view as streamed
output. What the view never receives is a **snapshot containing that output**,
because the snapshot it took was of a session that had not started.

Before this change the agent's first bytes were printed by its own bootstrap
shell at spawn, so they were already in the daemon's headless terminal when the
view attached and arrived *in* the snapshot.

## The observable consequence

`apps/desktop/tests/e2e/mock/terminal-recovery.test.ts` fails on
`reattaches after a kill+respawn that happened while the task was deselected`.
The fixture's `ORIGINAL_READY` is in the daemon's terminal for that session and
in `/v1/tasks/{id}/logs`, the perf log records its 15 bytes delivered to the
webview and drained to xterm — and `buffer.active` for that session is 40 blank
lines, so the assertion never sees it.

**Whether a visible window renders it was not verified.** The driven lane's
window is hidden, and `screencapture` on this machine returns black (see
`docs/2026-09-09-startup-terminal-screenshot-gap.md`), so the one observation
that would separate "hidden-window rendering" from "the view is genuinely stuck"
could not be made. Treat this as unresolved in that direction rather than as a
proven product defect *or* a proven harness artefact.

## What was ruled out

- **Not the new terminal tabs.** Disabling `useTaskTerminalTabs` entirely
  changes nothing.
- **Not the server or the launch.** `GET /v1/tasks/{id}/terminals` shows the
  startup terminal retired with exit 0 and the agent terminal live, the daemon
  lists the agent session `Active`/`busy`, and the task's logs carry the marker.
  The same launch shape against a real `./kd dev up` instance puts the marker in
  the agent terminal within seconds.
- **Not "wait for the agent before selecting".** The view mounts when the task
  is created, not when it is selected, so waiting for the agent's output before
  selecting does not change which snapshot it took.
- **The `session_created` rebind, which exists for exactly this, never fires.**
  A probe logging every `session_created` payload the view sees recorded none at
  all in that lane, for any session. The daemon broadcasts `SessionCreated` on
  spawn and `daemon_lifecycle.rs` forwards it, so something between them is not
  delivering it there. That is worth finding on its own: it is the mechanism the
  desktop already relies on for stage-swap respawns.

## What was tried and taken back out

Two shapes, both reverted rather than landed looking fixed:

1. Keying the agent `TerminalView` on a generation counter the terminals
   reconciliation bumps when a task's agent session first appears. The
   reconciliation only re-reads when the task's row changes, so the bump can
   arrive after the window it matters in.
2. Holding the agent view unmounted (a "Running startup…" placeholder) until the
   terminals endpoint reports a live agent, re-checking on a 1s timer while it
   is pending. It did not make the test pass either, which is why it is not in
   the tree: a placeholder and a poll are real UX and real risk to buy nothing
   demonstrated.

## What did land

`terminalSessionLifecycle` no longer settles on an empty terminal when an attach
is *refused* because the session is missing: it schedules the same backoff a
refused attach already uses, and says which terminal is running meanwhile. That
covers the refusal path — it is correct on its own terms and is not what this
note is about, because here the attach is accepted.

## What would close it

First, decide the open question: run the app visibly and watch a task with
`setup` in its `.kanna/config.json` — does the agent tab fill in when setup
finishes? If it does, this is a harness limitation and the test should observe
the agent's output through a source that does not depend on a hidden window
rendering streamed bytes. If it does not, the desktop needs the agent view to
attach only once the session exists; the server now reports which terminals a
task has and whether each is live, so the fact is available, and finding why
`session_created` is not reaching the webview would give the event-driven
version of the same fix.
