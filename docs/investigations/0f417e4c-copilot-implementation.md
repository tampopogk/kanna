# Copilot supervisory wake implementation

2026-09-16 UTC, task `0f417e4c`. Implemented in the task worktree on base head
`27e08702524dbec3314071c0fd3a8bf6f3a083c4`. This report describes **the scoped
Copilot fix**, not a completed fix for every harness. Per the latest instruction,
the workflow owns committing, independent review and PR creation; these changes
have not been deployed or applied to an owner's running session/subscription.

## Cause and resulting boundary

The subscription observer admitted a durable event batch and selected the
`input` harness adapter. That adapter called `send_engine_wake`, which used the
server's ordinary logical-input path and the daemon's text-plus-submit operation.
The provider received those bytes at its live composer. Its unfinished human
draft and the automatic notice therefore became one submitted message. An
`engine` database label cannot undo that composition at the provider boundary.

Copilot now uses the proven host-owned extension and public
`session.send({prompt, mode: "enqueue"})`. The extension joins the native session
assigned to the stage run. Kanna bundles its source into a content-addressed
local plugin and passes `--experimental --plugin-dir` on new Copilot PTY launches.
There is no second model runtime, package installation or global plugin change.

The observer's existing admission, cursor, batch ordering and read/ack contract
stay authoritative. Default Copilot subscriptions select `copilot_extension`;
legacy Copilot rows naming `input` dispatch to the same native-only adapter
without rewriting their durable mailbox. Explicit `send_task_input` still uses
its existing always-submit PTY semantics. No daemon/composer code changed.

## Durable lifecycle

1. The local SSE registration validates task, latest run, provider and native
   session. SQLite retains the registration epoch, but only the current live
   server-side stream permits a new attempt. A transient session-mutation lease
   returns retryable 503; a foreign or retired binding returns 409.
2. Migration `086_copilot_wake` adds registration and attempt tables. An attempt
   is reserved transactionally before the send frame, with unique subscription
   and batch identity, original stage/run and exact server-authored text.
3. The extension calls native enqueue once. Queue acceptance is separate from
   model execution, mailbox read and acknowledgement. The receipt endpoint
   admits neither caller-authored input text nor caller-selected provenance.
4. A confirmed native queue ID or a unique matching native history event records
   the captured notice as reserved `engine`, atomically with the receipt and
   `task.input_delivered`. Native queue IDs and history event IDs remain distinct.
   Copilot's native role is still **user**, with explicit engine wording; this
   implementation makes no claim of native system-role semantics.
5. Lost receipts, disconnects and server restart retain the attempt and batch.
   Reconnect issues **inspect**, never send, for an existing unresolved attempt.
   A retained queue ID or positive exact history evidence reconciles it once.
   Missing/ambiguous history stays visibly uncertain; a later native message
   event can resolve a notice that was still queued. No blind resend or PTY
   fallback exists. Even a crash between preparation and transmission remains
   uncertain rather than guessing whether a send happened.
6. Receipt waits hold no ordinary-input lease. Acknowledgement is still explicit
   and permits the next ordered batch. A late receipt records its original run
   without restoring an acknowledged batch or changing the replacement run.

The stream retries broken connections with bounded backoff and SSE keepalives;
it does not poll events. Provider stdin closure disposes bridge listeners/timers
and stops reconnecting. A missing/disabled extension leaves an actionable pending
mailbox instead of claiming delivery. An unresolved notice cannot lock later
ordinary input. A paused legacy mailbox retains its uncertainty diagnostic when
resubscribed.

## Changed files

The [source manifest](../evidence/0f417e4c-copilot-implementation-files.json)
pins every changed implementation/test file by SHA-256 against the base head.

- `crates/kanna-server/src/db/copilot_wake.rs`, DB module/migration, test schema
  and migration expectation: persisted registration, attempts and atomic receipts.
  `db/task_inputs.rs` exposes the existing preview helper internally.
- `http_api/copilot_wake.rs`, router and AppState: local stream/receipt boundary
  and live connection lifecycle. `harness_wake.rs` and `event_subscriptions.rs`:
  Copilot adapter selection, legacy mailbox preservation and observer integration.
- `resources/copilot-wake/{extension,bridge}.mjs`: provider-owned SDK bridge,
  transport reconnect and positive-history recovery.
- `task_creator/{environment,mod}.rs`: bundled plugin materialization and launch
  flags. No runtime dependency on a build-machine package installation.
- `http_api/tests/task_events/copilot_wake.rs`, task-event test registration,
  spawn assertion and the two CLI offline test files: boundary/recovery coverage.
- Tool catalog delivery vocabulary, `AGENTS.md` and server boundary documentation.

## Verification and preserved failures

| Check | Result |
| --- | --- |
| `cargo check -p kanna-server --bin kanna-server` | Pass. |
| `cargo clippy -p kanna-server --bin kanna-server --tests` | Pass; existing unrelated unused `auth_ok_frame_with_terminal_geometry` warning only. |
| Focused Rust command selecting `copilot_wake`, `send_task_input`, fresh-profile migration and prepared Copilot spawn | 15 passed at that snapshot: five native tests, eight ordinary-input tests, migration and spawn. |
| `cargo test -p kanna-server --bin kanna-server copilot_wake` after adding paused-mailbox coverage | Six passed. Latest diagnostic-preservation change then passed its targeted legacy-subscription test. |
| Production bridge offline tests | Five passed: exactly-once send, retained queue receipt, absent/ambiguous history, late history event, hanging send and disposal. |
| Production extension HTTP/process fixture | One passed: initial 503, fragmented SSE, failed receipt, reconnect, scripted process replacement, history reconciliation without resend, parent stdin EOF cleanup. |
| `cargo test -p kanna-tool-catalog` | 59 passed. |
| JavaScript syntax and `git diff --check` | Pass. |

The Rust fixture uses real loopback HTTP/SSE, SQLite and the subscription
observer. It verifies unavailable registration, wrong binding, stale epoch,
separate read/ack and ordered next batch, transaction rollback, original-run
late receipts, and replacement of all server in-memory state against the same
database. The JavaScript process fixture changes only SDK import resolution to
a scripted public API; the production transport/bridge execute unchanged.

Initial native tests failed because the test-only database schema omitted the
new tables; sharing the migration schema constructor fixed that failure. The
wire fixture initially expected exactly two receipt requests, overlooking the
intentional uncertainty report after the failed receipt; it now checks receipt
meaning and epoch while asserting exactly one native send. These failed checks
were test defects, not successful runs. Clippy also identified two new style
issues, both fixed before its passing run.

The [committed 6.571-second TUI proof](0f417e4c-copilot-synthetic-tui.md), head
`27e08702524dbec3314071c0fd3a8bf6f3a083c4`, remains the actual composition evidence:
Copilot **1.0.64**, empty composer, partly typed draft and insertion cursor,
separate later human submission, busy enqueue and lost-receipt history recovery.
Six canned loopback provider requests, no paid/external inference. It is reused
for the unchanged native API contract rather than rerunning a UI matrix.

A further **registration-only** packaged-plugin check ran for 1.419 seconds in
one disposable sandboxed 1.0.64 TUI. It verified the actual plugin directory,
SDK resolution and exact joined session
`cbeb9ed7-452a-4247-ba5c-9a1b0d2f3c53`. Its only HTTP request was registration;
there were no wake frames, native sends or model requests. Exact command, source
hashes, sandbox, logs, runner and cleanup are in the
[packaged-load evidence](../evidence/0f417e4c-copilot-packaged-load.json).
The parent-stdin cleanup was added afterward and checked by the scripted process
fixture; the load check is not misrepresented as testing those later source bytes.

All owned provider/extension processes stopped, the disposable listener closed,
and its workspace, session/config/XDG/GH/runtime directories were removed.
No credentials were supplied. Temporary synthetic artifacts remain under `.tmp/`.

## Remaining compatibility limits and workflow handoff

- Actual native behavior is proven for **Copilot 1.0.64**. Installed launcher or
  SDK version labels alone are not evidence for other runtime versions.
- A real provider-owned extension process/native stdio restart has **not** been
  exercised. Server restart and a scripted SDK process replacement pass; they
  do not prove Copilot will restart a crashed extension itself. If it does not,
  events stay pending until a later normal launch or explicit mailbox service.
- Permission dialogs, account policy, and a paid model's tool read/ack behavior
  are not covered. The synthetic unauthenticated run reported an unavailable
  permission-service reset; that remains a limitation, not a passing check.
- Existing sessions without this extension cannot gain it through a server
  upgrade alone. Their Copilot automatic notices remain pending, with no unsafe
  composer fallback. An old server still has its old behavior; a new extension
  cannot repair that boundary by itself.
- Other harness adapters are unchanged and remain outside this narrowed change.
  The prior non-Copilot prototype was removed from the working diff and preserved
  under `.tmp/deferred-non-copilot-prototype`; it is not part of this implementation.
- No production subscription, owner composer, current owner session, workflow
  stage, account policy or release channel was modified. No broader compatibility
  experiment or paid inference is needed to review this scoped implementation.

Ready for the workflow's commit and one focused independent input-lifecycle
review. That review has not yet occurred; no rollout or all-harness completion is
claimed. There is no new human approval request or attention badge.
