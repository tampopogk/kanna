# The agent view was fine; the fixture's agent printed the wrong thing — 2026-09-09

## Resolved

`apps/desktop/tests/e2e/mock/terminal-recovery.test.ts` failed on
`reattaches after a kill+respawn that happened while the task was deselected`,
and the earlier version of this note proposed the wrong cause: that the desktop
mounts a task's agent terminal view during setup, attaches to a session id the
daemon does not have yet, and so misses the agent's first output. That is not
what was happening. The test passes with its assertions unchanged; only the
fixture was wrong.

## What the evidence actually said

Instrumenting the two hops the theory depended on settled it in one run:

- The Tauri event bridge *does* receive and forward `SessionCreated` for every
  session in that lane — the daemon broadcast and `daemon_lifecycle.rs` are
  both fine.
- The agent view attached to the task's session **after** the daemon had it
  (`attach:ok`), received the agent's bytes on the KSP stream, and wrote them
  into its own xterm; a probe reading the buffer straight after the write
  callback found the line present at row 0.

So the view was never stuck. What the test read was `matchingLineCount` for
`^ORIGINAL_READY$`, and the buffer held `ORIGINAL_READYn` — with a literal `n`.
The assertion was right and the rendered terminal genuinely did not contain
what the test required.

## The bug

The fixture generated its fake agent CLI with a single `printf` whose format
string contained the script:

```
printf '#!/bin/sh\n' + "printf 'ORIGINAL_READY\n'; while true; do sleep 60; done" + '\n' > bin/claude
```

The script's own single quotes close the writing `printf`'s quoting, so the
shell re-parsed the rest as arguments — and an unquoted `\n` there is just `n`.
The installed agent printed `ORIGINAL_READYn` with no newline at all. The
marker still appeared in `/v1/tasks/{id}/logs` (a substring match), which is
why the launch looked healthy right up to the rendered-buffer assertion.

The fixture now writes the script one line at a time with `printf '%s\n'`,
passing the marker line as an *argument* rather than folding it into the format
string.

## What stands from the earlier note

`terminalSessionLifecycle` no longer settles on an empty terminal when an
attach is refused because the session is missing: it schedules the same backoff
a refused attach already uses and says which terminal is running meanwhile.
That was landed separately, is correct on its own terms, and is unrelated to
this failure.

The reason a retired terminal cannot be attached to at all — and renders its
archived final frame instead — is in
`docs/kanna-server-boundary.md`, "A Task Owns Several Terminals".
