# Supervisory wake transport comparison

Reconciled 2026-09-15 for parent task `0f417e4c`. All three investigation
children have finished. Their advisory `failure` results identify missing
compatibility evidence or necessary design changes; they do not authorize PTY
injection, disabling ordinary input, rollout, or additional human attention
badges. No product behavior changes are made by this reconciliation.

Update: the owner subsequently authorized the bounded Copilot no-inference
preflight. [Its result](0f417e4c-copilot-preflight.md) proves same-session
extension attachment on the runtime actually launched, 1.0.64, and finds that
both cached 1.0.64 and 1.0.83 high-level SDKs omit `source` from `send()`.
This rules out claiming native system-role semantics through those serializers,
not the composer-independent transport. The subsequent
[offline adapter proof and recommendation](0f417e4c-copilot-engine-adapter.md)
uses explicit engine wording plus Kanna's reserved durable source instead.
No live send, inference, rollout or ordinary-input change occurred.

## Findings and evidence level

| Harness | Best identified route | Evidence and remaining limitation |
| --- | --- | --- |
| Codex | Existing opt-in app-server adapter, targeting the live root thread with tool output | Existing Kanna implementation; these children did not revalidate its live behavior. It is not a generic adapter for another harness. |
| Claude | Native MCP Channels | Isolated 2.1.270 / pinned Haiku test proved synthetic draft and middle-cursor preservation, native engine labelling, reconnect replay, and separate acknowledgement. A startup probe went unconfirmed; a busy notice reached the model without a mailbox read. The latter is not proof that Fable cannot supervise or that Kanna needs another scheduler. |
| OpenCode | Direct `POST /session/{id}/prompt_async` on the **existing TUI worker** | Installed 1.4.3 plus tagged/embedded source support a composer-independent route. Current Kanna sessions lack a verified listening endpoint/native-session binding. Source predicts a password-authentication problem in default TUI external-HTTP mode; not yet reproduced. Live draft/cursor and uncertain-delivery behavior remain untested. |
| Copilot | CLI-owned extension: `joinSession()` then `session.send({prompt: labelledEngineNotice, mode: "enqueue"})` | Actual 1.0.64 TUI passed synthetic draft/cursor, busy enqueue and positive-history receipt recovery with a scripted local provider. Native user role; Kanna owns engine provenance separately. Both cached SDKs passed offline wire tests. Process reconnect, durable product integration and real tool-read/ack remain unproved. |
| Antigravity | No verified unsolicited local route to the same live TUI | `agy` absent from bounded local discovery. Documented cloud Remote Control, headless input and official ACP do not establish attachment to an existing terminal session. An already-outstanding MCP call is a possible bounded experiment, not a proven unsolicited wake channel. |

The source-cited child notes are [OpenCode](0f417e4c-opencode-wake.md),
[Copilot](0f417e4c-copilot-wake.md), and
[Antigravity](0f417e4c-antigravity-wake.md). The prior
[Claude experiment](../2026-09-14-claude-channel-live-compatibility.md) is live
evidence only for that harness and configuration.

## Consequences for the bounded fix

- Keep the durable observer, batching/admission, mailbox and explicit read/ack
  contract shared. Select a transport by a measured capability of the current
  run, not by model name, documentation alone, historical session ID or cwd.
- An outstanding tool wait provides a composer-independent result path while
  the invocation survives. It does not prove autonomous wake after cancellation,
  disconnect, or when no wait was registered. `kanna_wait_events` also does not
  wait for an admitted subscription batch; do not casually replace mailbox
  semantics with a second observer.
- A model maintaining a wakeable background thread is useful behavior layered
  on a harness facility. Do not infer an additional scheduler from the Haiku
  busy-turn result. First establish native readiness and preserve observability
  of transport receipt, mailbox read, and explicit acknowledgement.
- Unavailable or uncertain native delivery keeps the batch pending. Do not
  append a fallback nudge to the human composer, start a second writer against
  the same conversation storage, or acknowledge an undelivered notice.
- Mailbox-only retention protects data but does **not** meet the original timely
  unattended supervision requirement by itself. Antigravity's evidence gap
  remains explicit; no eligibility or ordinary-input policy has been changed.
- Ordinary explicit `send_task_input` remains a separate surface. A pending
  notification must never lock it. The live manager subscription
  `watch-1789157553830857000-0` remains unchanged.

## Concrete recommendation and smallest next experiment

Keep one durable mailbox with harness-specific native delivery. The Codex and
Claude paths remain the existing candidates; pursue **Copilot's provider-owned
extension next**, because it offers a narrower same-runtime path than changing
OpenCode's launch ownership. Do not migrate all sessions to a custom SDK UI or
invent a universal PTY fallback. OpenCode stays a credible follow-up once its
authenticated listener topology is resolved. Antigravity stays explicitly
unverified; that is not a decision to disable ordinary Antigravity input.

The authorized no-inference preflight has now answered the attachment question
for actual runtime 1.0.64, and the offline SDK tests establish the supported
labelled enqueue contract for both cached SDKs. Native system-role semantics
are not required for Kanna's reserved engine provenance.

The [current adapter recommendation](0f417e4c-copilot-engine-adapter.md) specifies
the server-owned durable receipt boundary and the next experiment: one disposable
Copilot TUI using a scripted local provider, no paid inference, testing actual
draft/cursor preservation, busy enqueue and uncertain-receipt reconciliation.
The owner subsequently authorized that one synthetic live-send experiment;
[it passed the exercised cases](0f417e4c-copilot-synthetic-tui.md) in 6.571 seconds.
This is not process-reconnect or product-integration proof. No rollout or
ordinary-input change follows. Antigravity's advisory evidence gap remains
separate and creates no blanket human gate. The next scoped work is the Copilot
adapter's durable server/extension boundary, not another broad provider survey.

## Durable child provenance

All children inspected base `ffb3b7cf0d58a2cea9e4a911443d87a3b1e7d048`, which
contains the Claude report but not the parent's unfinished prototype sources.
Read full child detail and instruction history before reconciling each result.
The copied notes are byte-identical to their original committed blobs:

| Child | Advisory result | Original note commit |
| --- | --- | --- |
| `298fd01c` — OpenCode | REVISE | `34595053a6cc9994f263d872a477a26951a1cb09` |
| `e1702091` — Copilot | REVISE | `68d0336ad4a90de055f3eae1e99158eb79dabad7` |
| `c481df3a` — Antigravity | STOP-and-escalate | `ee68ff6720c41af9b44db8609010433902d975b8` |

Report-only validation: original-blob equality, local Markdown link targets,
and staged whitespace checks. No build or product tests are warranted for this
comparison. Original implementation work remains unfinished and uncommitted;
the consultation results are not an independent review of that implementation.
