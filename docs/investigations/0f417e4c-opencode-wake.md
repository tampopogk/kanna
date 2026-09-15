REVISE: OpenCode has a promising composer-independent session API, but it is not a verified drop-in wake channel for Kanna's existing interactive sessions. Recommend a capability-gated `opencode_session` adapter only after authenticated endpoint/session ownership and the focused live contract below are proved. Keep unsupported deliveries pending and explicit; never substitute terminal input.

## Evidence verified

Investigated 2026-09-15 for parent **0f417e4c**, child **298fd01c**. Inspected Kanna head **ffb3b7cf0d58a2cea9e4a911443d87a3b1e7d048**, branch `task-298fd01c`, initially clean. Read parent's full task detail and all three durable inputs: original objective is draft-safe automatic supervision, preserving mailbox/ack/order/provenance and ordinary explicit input. The child prompt explicitly authorizes these investigations; that latest instruction is not among the parent's three recorded input rows. No other worktree was inspected. The [Claude report](../2026-09-14-claude-channel-live-compatibility.md) and its [JSON evidence](../evidence/0f417e4c-claude-channels-live.json) concern a separate harness and uncommitted prototype; their hashes do not identify committed implementation here.

Installed `/Users/jeremyhale/.opencode/bin/opencode` reports **1.4.3**, SHA-256 `79affb37bc30a206aec8229dbec3562688d01b25fc74b199910d37911e6a847c`. Ran only version/help (`default`, `attach`, `run`, `acp`) and read embedded source fragments. Official tag **v1.4.3** resolves to **877be7e8e04142cd8fbebcb5e6c4b9617bf28cce**; relevant bundled fragments corroborate its TUI transport, async route, and runner implementation. Downloaded source under `.tmp/opencode-research`. Checked current [server docs](https://opencode.ai/docs/server/), [SDK docs](https://opencode.ai/docs/sdk/), [CLI docs](https://opencode.ai/docs/cli/), [MCP docs](https://opencode.ai/docs/mcp-servers/) and [ACP docs](https://opencode.ai/docs/acp/).

**Evidence level:** documented API + installed CLI/help/bundled-source inspection + tagged source/tests read. No new mock execution, provider inference, live session, composer experiment, build, or test gate ran. Historical [OpenCode integration evidence](../2026-08-08-opencode-live-idle-detection-e2e-gap.md) proves ordinary TUI input/idle behavior, not this HTTP wake path. Its recorded versions are not a compatibility guarantee for this installed binary.

## Affected producers/consumers/lifecycle owners

- Kanna DB event writes → [subscription collector/admission](../../crates/kanna-server/src/http_api/event_subscriptions.rs) → persisted [mailbox](../../crates/kanna-server/src/db/event_subscriptions.rs) → [harness adapter](../../crates/kanna-server/src/http_api/harness_wake.rs). One pending batch blocks subsequent collection until matching acknowledgement; read alone does not acknowledge. Admission and compare-and-swap protect delivery/ack races.
- Current OpenCode delivery uses `input` → [engine input handler](../../crates/kanna-server/src/http_api/task_input.rs) → daemon `SubmitInputIfSession` → [logical writer](../../crates/daemon/src/connection.rs) / [framing and submission boundary](../../crates/daemon/src/session.rs). This writes text plus Enter regardless of draft. The confirmed delivery gets [task_input source=engine](../../crates/kanna-server/src/db/task_inputs.rs); unknown delivery does not. Desktop/mobile human input shares the PTY, causing the collision.
- [Launch commands](../../crates/kanna-server/src/task_creator/commands.rs) start the default TUI with `--prompt`; resumed launches seed via `run --session`, then open the TUI sequentially. [MCP/model configuration](../../crates/kanna-agent-protocol/src/mcp.rs) enters through `OPENCODE_CONFIG_CONTENT`. Fresh OpenCode IDs are not assigned by [task creation](../../crates/kanna-server/src/task_creator/mod.rs), so the native adapter needs an additional live binding, not just a historical session lookup.
- OpenCode TUI owns the draft, worker owns its session runner/MCP connections, and OpenCode persists conversation messages. Kanna owns subscription truth, admission, task/run binding and delivery records. These process lifecycles differ: Kanna-server can restart while the daemon and TUI survive; the TUI worker exits with its TUI. Stage replacement/transfer must invalidate endpoint credentials and bindings, not carry a localhost endpoint to another machine.

## Invariants and failure modes

### Exact native route and ownership

**Conditional yes:** `POST /session/{ses_id}/prompt_async?directory={URL-encoded absolute worktree}` can append text to the same session without calling the composer. `/session/{ses_id}/message` invokes the same operation but waits. `noReply: true` persists input without starting the loop, so it is not an idle wake. Use a direct HTTP client, or SDK **`createOpencodeClient`**, not `createOpencode` (which starts a server). [Route implementation][routes], [SDK client][sdk].

Candidate JSON (identifiers are placeholders):

```json
{
  "messageID": "msg_<native ascending identifier>",
  "agent": "<current session agent>",
  "model": { "providerID": "<current provider>", "modelID": "<current model>" },
  "parts": [{
    "id": "prt_<native ascending identifier>",
    "type": "text",
    "synthetic": true,
    "text": "[Kanna supervisor] Engine notice, not owner speech: subscription WATCH batch BATCH is pending. Read it, reconcile, then separately acknowledge.",
    "metadata": { "source": "engine", "subscriptionId": "WATCH", "batchId": 1 }
  }]
}
```

Carry the current variant where applicable; do not accidentally select the default agent/model or override permissions with `tools`. Generate native sortable IDs at delivery, persist them before sending, and retain them for reconciliation; a batch hash merely prefixed `msg_` is not safe because OpenCode uses message-ID ordering. [Identifier implementation][ids].

**Default TUI is not necessarily a listening server.** Without explicit network flags/config, 1.4.3 uses worker RPC at `http://opencode.internal`. `opencode --port PORT --hostname 127.0.0.1` makes that *same worker* listen, and the TUI uses its returned URL. Current Kanna commands do not arrange this. There is no verified retrofit command to expose an already-running private worker. Never start `serve` against the same storage and mistake it for that worker: runner ownership is process-local. [TUI entrypoint][thread], [worker][worker], [run state][state].

For any eligible session, bind **task ID + stage-run ID + daemon session incarnation + actual provider process + endpoint credential + canonical cwd + exact OpenCode session ID** at launch/attachment. Verify authenticated `/global/health`, `/path`, `/session/{id}` and expected root identity. A session list or matching cwd alone does not prove the TUI's selected session. Missing/ambiguous binding or a TUI session switch is unsupported until reconciled. Reserve a unique endpoint; do not guess port 4096, scan installed sessions, or recreate a missing conversation.

**Authentication is a concrete 1.4.3 caveat.** Server supports Basic auth via process-local `OPENCODE_SERVER_PASSWORD` (username defaults `opencode`). Use a task-scoped secret, restrictive local storage if needed for server recovery, Authorization headers, loopback only, and no mDNS. CORS is not authorization. Tagged source and embedded TUI code show external default-TUI startup passing no auth headers; its SDK client does not add them. The private worker-fetch path adds auth, but is bypassed in external mode. This predicts a broken password-protected default TUI and must be reproduced offline before choosing that launch shape; it is **source evidence, not a measured live failure**. `attach` explicitly passes Basic auth with username `opencode`. Do not “fix” the problem by exposing an unauthenticated control server. [Server][server], [worker][worker], [thread][thread], [SDK client][sdk], [attach][attach].

### Runtime behavior and provenance

| State | Verified mechanism / limit |
|---|---|
| Idle, no registered wait | API stores a message and starts the existing session runner. No wait registration is required. |
| Busy/model streaming | Message is stored first; `ensureRunning` joins the existing run rather than launching a second runner or cancelling it. The next loop reads persisted messages. This is not a guarantee of immediate attention or a separately scheduled turn per POST. |
| Shell running | Runner queues a run after shell completion. It does not interrupt the shell. |
| Human draft | API avoids prompt append/submit/clear handlers. Prompt text is local TUI state. Source supports safety; rendered text/cursor/focus still require an isolated live test. |
| Permission/question pending | API does not answer it. The runner can remain blocked, delaying the notice. Keep the human decision intact; immediate service is unsupported. |
| Abort/error/late busy completion | A joined run can finish without processing the new notice. Receipt cannot imply mailbox read; keep pending and reconcile at lifecycle edges. |

Sources: [runner][runner] and its [unit tests][runner-tests], [prompt pipeline][prompt], [composer][composer], [session view][view].

`synthetic: true` is meaningful but **not an engine role**. It is persisted, hidden from ordinary TUI user-text rendering, and excluded from the busy-turn “user sent” wrapper. The message envelope remains `role: user`; model conversion sends its text as user content and drops metadata. Preserve explicit engine wording in the text and reserved engine provenance in Kanna's durable delivery record. `metadata.source` is only a client-authored tag, not authenticated authority. Do not advertise Claude-style native channel provenance. [Message schema/model conversion][messages], [prompt pipeline][prompt], [session view][view].

### Receipt, recovery and ordering

`204` only confirms route admission: the promise can subsequently fail and publish `session.error`. Reconcile the expected message **and text part** through message reads/SSE before declaring native persistence; message and parts are saved separately. Neither persistence, SSE reception nor assistant output acknowledges the Kanna mailbox. Only the agent's matching read/reconcile/ack advances it. [Routes][routes], [session persistence][sessions].

Retain one admitted notice per batch, native IDs, run binding and delivery state. Reconnect SSE then reconcile retained IDs and mailbox state; `/event` is a live bus, not replay with a durable cursor. Server crash after dispatch is uncertain. Do not blindly POST again: a caller-supplied message ID is not a documented exactly-once execution key. Missing/partial records require an explicit recovery outcome, not a fabricated receipt. Recovery must also distinguish **not read** from **read but deliberately unacknowledged** before re-waking. Pending notices must never acquire a lock on later human input. [SSE implementation][events], [Kanna mailbox contract](../kanna-server-boundary.md).

Gate on measured CLI version and endpoint capabilities; no minimum supporting version was established. Unknown schema/version, lost authentication, retired run, dead worker, unavailable model or unknown delivery leaves mailbox pending with diagnostics. Restart/transfer cannot silently resume a second writer. Current admission/backpressure must remain authoritative; no blanket polling or scheduler redesign.

## Alternatives and tradeoffs

1. **Direct native session adapter:** best candidate for an idle wake with no draft mutation. Requires ownership/authentication work and live validation; synthetic user-role content has weaker native provenance than a channel. Not ready for existing Kanna sessions.
2. **Existing MCP wait completion:** safe text delivery into an outstanding tool call, bypassing the composer. OpenCode awaits MCP `callTool`; it does not expose a generic unsolicited MCP notification that starts an idle turn. A wait must already be registered, allowed and within transport timeout. Verify actual call timeout: 1.4.3 passes configured MCP timeout to `callTool` as well as discovery; a long Kanna wait cannot assume that connection/discovery documentation proves its lifetime. Kanna's concurrent MCP dispatch does not prove OpenCode has a persistent background model thread. Fable/another model choosing and maintaining waits is model/tool-use behavior, separate from harness transport. A missed notice is not evidence that the scheduler must be replaced. [MCP implementation][mcp], [Kanna MCP](../../crates/kanna-mcp/src/main.rs).
3. **Mailbox-only delivery:** smallest safe fallback for unsupported sessions, but cannot claim timely unattended wake without an established wait/scheduler. Preserve the pending batch and expose this limitation; parent owns whether that temporary product limitation is acceptable.
4. **One owned `serve` + `attach` from session creation:** authenticated attachment is supported and gives explicit session IDs. This is a possible *future launch architecture*, with one engine and one TUI client, not a repair helper for an existing default TUI. It adds lifecycle/cleanup/recovery obligations. `run --attach` is unsuitable: it handles permission events by automatically rejecting them, or approving with its bypass flag. ACP starts its own server and stdio lifecycle; it is not attachment to this TUI. [Attach][attach], [run][run], [ACP command][acp].

Retaining automatic PTY input, `/tui/append-prompt`, `/tui/submit-prompt`, or clearing/retyping drafts fails the owner's objective.

## Acceptance criteria

- Add only a version/capability-gated native wake adapter with verified authenticated live binding; resolve the default-TUI auth limitation before enabling that topology. Do not broaden explicit send-input semantics or alter the protected live subscription.
- Preserve agent/model/variant, engine text/record provenance, ordered batches, separate receipt/read/ack, unknown-delivery state, and run fencing. No duplicate engine process, implicit permission response, or human-input lock.
- Refuse unsupported sessions explicitly. Existing non-listening sessions cannot gain this capability merely by changing a subscription enum. Parent decides future launch topology and temporary no-wake policy.

## Required E2E coverage

Use the repository's smallest layers in [testing conventions](../dev/testing.md): focused server subscription fixtures with a scripted HTTP peer for malformed/auth/unknown/version/run/receipt/ack races; fake-provider/daemon integration proving **zero wake PTY bytes** while explicit input still submits. These can run without inference. Do not claim those prove the real OpenCode composer.

Later, separately authorized live CLI-contract test should extend [existing helpers](../../tests/cli-contract/helpers/opencode.ts) and the PTY bridge with rendered terminal capture. Minimal candidate launch (not run here), from a disposable directory under this worktree's `.tmp/`, with isolated XDG/config paths, sanitized inherited environment, fixture-only MCP, disabled auto-update and auxiliary model also pinned:

```sh
opencode --pure --port "$OC_TEST_PORT" --hostname 127.0.0.1 \
  --model opencode/big-pickle
```

First test startup with a disposable `OPENCODE_SERVER_PASSWORD` **without sending a prompt**. If the predicted authentication failure occurs, stop that topology; do not remove auth and proceed. An authorized alternative test must create one owned authenticated server and `opencode attach "$OC_TEST_URL" --dir "$OC_TEST_DIR" --session "$OC_TEST_SESSION"`, with the password in process environment, never a second server for a live session.

Pin **`opencode/big-pickle`**, currently advertised free and already used by repository CLI fixtures; pin both `model` and `small_model`, restrict enabled providers, and record actual returned model/usage. This is a concrete transport/tool-use test candidate, not an immutable underlying model revision or guaranteed availability. If unavailable, stop without cost escalation. [Current Zen model/pricing](https://opencode.ai/docs/zen/).

Required evidence: same PID/worker/session throughout; synthetic draft with mid-text cursor survives idle and busy notices plus reconnect; later human Enter submits exactly the human text; permission wait remains untouched; registered MCP wait returns tool output; no-wait API wake runs; busy-to-idle race and abort retain unread batch; missing auth/version/endpoint fail closed; lost HTTP response is reconciled without duplicate turn; batch 2 waits for ACK1; native receipt/read leaves pending until ACK. Record wire traffic, rendered frames/cursor, persisted synthetic parts, fixture input/mailbox rows and exact versions. Stop all owned processes. **This real-harness gap blocks enabling the adapter**, and is explicitly recorded here rather than hidden by the source inspection.

## Scope/exclusions

Only this investigation note changed. No product code, tests, configuration, rollout, production/staging/owner composer access, subscription mutation, provider inference, additional tasks/agents, or manager messages. No background process was started. Parent owns synthesis and implementation; this advisory REVISE is not a stage advance or a request to implement here.

[routes]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/server/routes/session.ts
[thread]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/cli/cmd/tui/thread.ts
[worker]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/cli/cmd/tui/worker.ts
[state]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/session/run-state.ts
[runner]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/effect/runner.ts
[runner-tests]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/test/effect/runner.test.ts
[prompt]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/session/prompt.ts
[messages]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/session/message-v2.ts
[composer]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/cli/cmd/tui/component/prompt/index.tsx
[view]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/cli/cmd/tui/routes/session/index.tsx
[server]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/server/server.ts
[sdk]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/sdk/js/src/v2/client.ts
[attach]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/cli/cmd/tui/attach.ts
[run]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/cli/cmd/run.ts
[acp]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/cli/cmd/acp.ts
[mcp]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/mcp/index.ts
[ids]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/id/id.ts
[sessions]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/session/index.ts
[events]: https://github.com/anomalyco/opencode/blob/877be7e8e04142cd8fbebcb5e6c4b9617bf28cce/packages/opencode/src/server/routes/event.ts
