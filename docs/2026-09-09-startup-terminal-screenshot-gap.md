# The startup terminal's rendered screenshot is not captured — 2026-09-09

## What is not verified

Task 09b6a82a gives every launch a startup terminal of its own and shows it in
the desktop as a `terminal` tab beside the agent session. AGENTS.md asks for a
change that affects rendered UI to be verified by running the real app,
exercising the changed states, and capturing and inspecting screenshots. The
screenshots were not captured.

## Why not

`screencapture` on this Mac Studio returns an all-black image: the session
driving the work has no Screen Recording entitlement, and AppleScript's
`System Events` reports `0 windows` for every `kanna-desktop` process, so the
documented fallback in `dev-window-screenshots-catch-the-staging-app` — read
both windows' bounds, move the dev window somewhere the staging app cannot
cover, then verify the captured PNG — has no way to run either. Nothing about
the change is at fault; the capture path itself is unavailable from here.

## What was verified instead, and how

The rendering is proved by a driven E2E test rather than by a picture:
`apps/desktop/tests/e2e/mock/main-tabs.test.ts` now drives the real app through
WebDriver and asserts, against the real Vue tab bar and the real server:

- `GET /v1/tasks/{id}/terminals` reports the launch's `setup` terminal as a
  session distinct from the agent's, and still names the agent session by task
  id;
- selecting the task opens `terminal:setup-<task>-1` beside `agent`, and the
  tab's rendered label reads `Startup · in progress`;
- it does not steal focus — `agent` stays the active tab;
- closing it hides the view and leaves the server's record intact, so reopening
  shows the same terminal.

The server side was also exercised against a running `./kd dev up` instance: a
task created in a repository with `setup` commands answered its create request
immediately, recorded `setup-<task>-1` as a live `setup` terminal alongside the
`agent` one, and `kanna_list_task_terminals` / `kanna_open_terminal` appeared in
the instance's advertised tool list.

## What would close this

Screen Recording permission for the session driving the capture (or a human
running the app and taking the screenshots). What is worth looking at with eyes
is the tab bar with a startup terminal open, a startup terminal mid-run, and the
finished-terminal banner in `TaskTerminalPanel.vue` — none of which the driven
test judges visually.
