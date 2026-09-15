REVISE: Replace automatic Copilot PTY nudges with a capability-gated, local
session extension; do not enable it until the installed runtime passes the
isolated contract below. There is a credible same-session native path, but **no
live evidence here proves draft/cursor preservation on Copilot 1.0.83**. Retain
unserved mailboxes visibly; mailbox-only delivery cannot satisfy timely wakeups
when no wait is registered.

## Evidence verified

Investigation date: 2026-09-15. Child `e1702091`, branch `task-e1702091`, inspected
HEAD `ffb3b7cf0d58a2cea9e4a911443d87a3b1e7d048`, initially clean. Read parent
`0f417e4c` through `kanna_get_task` and all three durable inputs through
`kanna_task_inputs`. Its original objective excludes ordinary explicit input;
the later Claude test authorization does not authorize a Copilot live test.
This child's prompt supplies the subsequent investigation authorization.

Read the committed [Claude report](../2026-09-14-claude-channel-live-compatibility.md)
and [evidence](../evidence/0f417e4c-claude-channels-live.json). Their draft and
busy-turn observations are Claude-only. Their prototype hashes are not committed
Copilot implementation. No parent worktree was inspected.

Local executable `/Users/jeremyhale/.local/bin/copilot`: **1.0.83**, SHA-256
`5d27ea2257d8056e0e8063845e5d41f4e8fc37ae52a4052b62347b746299ca7a`.
Executed only version/help commands, including config, environment, commands,
experimental commands, and plugin help. They advertise `--acp`, `--connect`,
`--remote`, session resume/assignment, `--experimental`, `--plugin-dir`, and
`--extension-sdk-path`; ordinary help does not advertise `--ui-server` or a local
send-to-TUI command. Experimental command help also omits `/extensions` despite
current official documentation listing it. This is a compatibility warning,
not proof that extensions are absent. Binary string inspection did not recover
readable runtime implementation. No provider session, mock provider, or inference
was started; there are **no Copilot live or synthetic transport results**.

Official SDK source inspected at
[`9553d5224c73df2d02aee5ec01eeb8353acc736a`](https://github.com/github/copilot-sdk/tree/9553d5224c73df2d02aee5ec01eeb8353acc736a/nodejs)
(`0.0.0-dev`, CLI pin `1.0.84-8`), plus pre-release snapshot
[`81ffc2a77c2454a923f8dae3e74663bc49bfbd9f`](https://github.com/github/copilot-sdk/tree/81ffc2a77c2454a923f8dae3e74663bc49bfbd9f/nodejs)
(2026-09-03). Neither establishes the exact SDK embedded in the local binary.
Typed public message provenance landed on
[2026-09-08](https://github.com/github/copilot-sdk/commit/d8bbc9dd7a6167d4806780f405d8ce74add1cc7c),
after the [1.0.83 release](https://github.com/github/copilot-cli/releases/tag/v1.0.83).
The older generated RPC schema already has `source`, but marks it internal.

## Affected producers/consumers/lifecycle owners

Verified committed path:

1. Server event writes feed the subscription observer. Its
   [collector/admission worker](../../crates/kanna-server/src/http_api/event_subscriptions.rs)
   persists one pending batch, binds it to task/run/stage/branch, and calls the
   adapter after admission. Only matching mailbox acknowledgement advances the
   pending batch's cursor; reading alone does not acknowledge it.
2. [harness_wake.rs](../../crates/kanna-server/src/http_api/harness_wake.rs)
   has `input`, `codex_app_server`, and `poll`; no Copilot native adapter.
   `input` calls `send_engine_wake` in
   [task_input.rs](../../crates/kanna-server/src/http_api/task_input.rs), using
   reserved `engine` provenance and the expected run.
3. The server checks a live daemon PTY and submits `SubmitInputIfSession` with
   its PID. [Daemon connection](../../crates/daemon/src/connection.rs) checks the
   incarnation; [session input](../../crates/daemon/src/session.rs) writes text
   and its submission boundary without a composer check. Consequently automatic
   input can consume a draft just like explicit input. A confirmed write creates
   a [task_input record](../../crates/kanna-server/src/db/task_inputs.rs);
   uncertain delivery does not pretend to be confirmed.
4. [Task creation](../../crates/kanna-server/src/task_creator/mod.rs) assigns a
   Copilot UUID; [commands](../../crates/kanna-server/src/task_creator/commands.rs)
   launch the actual TUI with `--session-id`, `-i`, and
   `--additional-mcp-config` (or `--resume` for recovery).
   [Environment](../../crates/kanna-server/src/task_creator/environment.rs)
   supplies Kanna identity/endpoints and the local credential path. The daemon
   owns the PTY lifetime; Copilot owns its composer and agent loop.
5. [kanna-mcp](../../crates/kanna-mcp/src/main.rs) handles concurrent ordinary
   `tools/call` requests and returns results on their request IDs. Its initialize
   advertises tools/list changes, not MCP Tasks. Desktop/mobile and later stages
   consume task/input/event projections; they need no new composer logic.

### Exact native candidate

Copilot [extensions](https://docs.github.com/en/enterprise-cloud%40latest/copilot/concepts/agents/copilot-cli/about-cli-extensions)
are experimental child processes of the interactive CLI, discovered from
project/user/plugin directories. A per-launch trusted plugin is preferable to
machine-global installation. Extensions require experimental mode and an enabled
extension mode; disabled extensions are stopped. These are availability gates,
not settings this investigation changed.
For an already-running Kanna TUI without this extension, no safe external
retrofit command was verified. Loading/reloading an extension is a separate
capability to test; do not type configuration commands into its live composer.

[joinSession source](https://github.com/github/copilot-sdk/blob/9553d5224c73df2d02aee5ec01eeb8353acc736a/nodejs/src/extension.ts)
uses host-provided `SESSION_ID` and a parent-process connection. Its
[transport](https://github.com/github/copilot-sdk/blob/9553d5224c73df2d02aee5ec01eeb8353acc736a/nodejs/src/client.ts)
is JSON-RPC over the extension's private stdin/stdout pipes, using
`vscode-jsonrpc` framing. It does not launch another agent runtime. The join
resumes the session **on that parent connection**, suppresses the resume event,
and defaults to no permission decision. Keep those defaults: no replacement
model/system prompt, permission handler, user-input handler, or foreground switch.
Logs must use stderr, not protocol stdout.

Candidate API, once verified on the bundled SDK:

```js
import { joinSession } from "@github/copilot-sdk/extension";
const session = await joinSession();
// First bind session.sessionId to Kanna's task/run/provider UUID and bridge epoch.
const messageId = await session.send({
  prompt: "[Kanna supervisor] Subscription <id>, batch <n> is pending. Read and reconcile it, then acknowledge. Engine notice; not owner speech.",
  source: "system",
  mode: "enqueue",
});
```

[session.send](https://github.com/github/copilot-sdk/blob/9553d5224c73df2d02aee5ec01eeb8353acc736a/nodejs/src/session.ts)
sends `session.send {sessionId,prompt,source,mode}` and returns `messageId`.
The [public contract](https://github.com/github/copilot-sdk/blob/9553d5224c73df2d02aee5ec01eeb8353acc736a/nodejs/README.md#sendoptions-messageoptions-promisestring)
describes queue acceptance, not reconciliation. `source: system` is provenance
on a message, not a privileged system-role instruction or a free billing flag.
Default enqueue is preferable to undocumented interruption assumptions about
`immediate`. On the older bundled SDK, high-level `send` may omit `source`;
inspect its wire output and resulting event before accepting this adapter.
Do not silently downgrade provenance or use private RPCs as a compatibility fix.

The host's authenticated account remains the inference authority; a child pipe
needs no separate GitHub login or cloud export. Kanna still needs a private,
per-run bridge endpoint and credential/nonce, validated against the live
task/run/provider UUID and process incarnation. Do not expose a writable public
port or accept a caller's `engine` label as authority. Extension environment
filtering means token inheritance must be verified; prefer a scoped bridge
credential over forwarding a GitHub token. Stop the bridge when its host exits;
server reconnect must not kill the TUI.

## Invariants and failure modes

| State | Supported conclusion / required behavior |
| --- | --- |
| Idle, no draft | Extension send is an advertised queue path to the same parent session. Idle wake execution remains unmeasured locally. |
| Busy | Enqueue must retain the notice and run it in order. Do not equate a successful send with model consumption or mailbox read. |
| Human draft/cursor | No PTY bytes are required by the extension path; actual TUI state preservation still needs evidence. Never clear, save/retype, submit, or probe the owner's composer. |
| Permission/question pending | Bridge must leave the prompt unresolved. Do not assume enqueue bypasses it or guarantees timely execution while human approval is outstanding. |
| No wait registered | Extension can send independently of a model tool wait; ordinary MCP tool completion cannot. Missing/disabled extension means no proven native wake. |
| Reconnect/crash | Rebind only to the same verified live incarnation. Preserve pending batch and receipt evidence. An interrupted RPC may have queued the notice: mark uncertain, reconcile before replay, never fall back to PTY input. |
| Version skew | Reject unsupported framing/protocol/provenance/capabilities. Version/help alone is insufficient. A recovered transcript ID alone is not proof of a live owner. |

Keep three facts separate: transport acceptance, mailbox read, and explicit
batch acknowledgement. Bind receipts to `(task, run, provider session, bridge
epoch, subscription, batch)` and preserve the provider message ID. Record
confirmed native notice provenance durably as `engine`, with truthful receipt
semantics; do not falsely claim a PTY write. A failed record must not trigger
another send. Keep one pending batch and existing admission/order rules. Replays
must be idempotent at the mailbox consumer; `messageId` is not a documented
client-supplied idempotency key. A pending engine notice must never lock the
daemon input queue against later ordinary human messages.

## Alternatives and tradeoffs

- **Registered MCP wait:** An outstanding `kanna_wait_events` result reaches
  its original invocation without typing. This is the smallest safe option
  while that invocation survives, but it is not an unsolicited text endpoint,
  nor a replacement for subscription read/ack. After cancellation/disconnection,
  no invocation remains to wake. The model choosing to maintain a background
  wait (Fable or any other model) is separate from the harness's ability to
  complete it and resume processing. One missed read does not justify a
  scheduler redesign.
- **MCP Tasks:** The official
  [changelog, 1.0.41](https://github.com/github/copilot-cli/blob/main/changelog.md#1041---2026-05-05)
  announces experimental `taskSupport: "required"` tools as background agents,
  tracked by `list_agents`/`read_agent`. This is a real advertised feature, not
  the older issue alleging no support. Kanna would need task negotiation,
  lifecycle/result methods, and completion wiring; it currently implements
  none of that. It still needs an initiated tool task. The older generated
  schema's MCP `notifications` allow-list and the vague historical “tool
  notifications” changelog do not establish a documented unsolicited wake
  method, payload, or idle/draft guarantee.
- **SDK server/attach:** An existing runtime URI plus connection token can
  support SDK attachment. SDK docs mention `--ui-server`; generated discovery
  entries include PID, host/port, session ID, cwd, version, and token. No supported
  automatic local endpoint for the ordinary Kanna-launched TUI was verified.
  Starting `copilot --server` then resuming its storage is not proof of joining
  the current TUI. SDK TCP connection authenticates with `connect`; its legacy
  fallback can drop the token, so authenticated capability checks must fail
  closed. The extension avoids registry selection and that second-writer risk.
- **Internal notification RPC:**
  [Pre-release generated source](https://github.com/github/copilot-sdk/blob/81ffc2a77c2454a923f8dae3e74663bc49bfbd9f/nodejs/src/generated/rpc.ts)
  contains `session.sendSystemNotification {sessionId,message,kind?,options?}`.
  It explicitly belongs to the internal API, with unspecified passive policy
  and a void result. It is a useful upstream lead, not a supported Kanna adapter
  or receipt contract.
- **ACP:** [`copilot --acp --stdio` or `--port`](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server)
  starts an alternate server with `session/new`, load, and `session/prompt`.
  It is not documented as attaching to an already-running default TUI.
- **Remote:** [`--remote` / `/remote on`](https://docs.github.com/en/copilot/how-tos/copilot-cli/use-copilot-cli/steer-remotely)
  and `--connect=<session-or-task-id>` advertise live control, but GitHub-mediated
  control needs the same account and applicable policy. Events leave the
  machine; [remote commands are polled through GitHub](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/about-remote-control).
  No documented local REST send endpoint or engine-provenance guarantee was
  established. Do not enable cloud export to fix this local defect.
- **Mailbox-only:** Safely retains events when no adapter is available, but
  autonomous timely delivery is unsupported. State that limitation explicitly.
  **Retaining automatic PTY injection is not viable.** A custom SDK UI would
  replace launch ownership, daemon event/control adaptation, permissions,
  transcript presentation, desktop/mobile interaction, and recovery—at least
  those six surfaces. The older [SDK proposal](../2026-06-21-copilot-sdk-structured-agent-proposal.md)
  concerns that separate migration; it is unnecessary to investigate an extension.

## Acceptance criteria

1. Select only a verified same-session native capability for automatic Copilot
   notices. No automatic PTY fallback, including after uncertainty. Preserve
   ordinary `send_task_input` behavior.
2. Bind the bridge before admitting delivery; reject stale run/session/epoch,
   wrong credentials, disabled extension, and unsupported source/protocol.
   Verify the actual CLI-bundled SDK; establish a tested minimum version rather
   than assuming 1.0.83 or the newer SDK pin is sufficient.
3. Preserve admission, ordered batches, explicit acknowledgement, truthful
   receipt/provenance and restart reconciliation. Resume pending delivery on
   verified readiness/reconnect events; no blanket polling or duplicate turns.
4. Expose unsupported/no-wait and uncertain cases without acknowledging or
   discarding the mailbox. Keep human input independent. The real unresolved
   decision is authorization for the isolated compatibility experiment below;
   successful research is not rollout approval.

## Required E2E coverage

Use the repository's [test taxonomy](../dev/testing.md). Focused Rust server
fixture + daemon fake-provider integration must prove observer → admission →
bridge → receipt → durable input/mailbox wiring, separate reads/acks, two ordered
batches, restart-after-send, wrong binding, missing capability, and ordinary input
remaining usable. Offline CLI contracts should pin launch/config and actual RPC
fixtures. The existing [Copilot helper](../../tests/cli-contract/helpers/copilot.ts)
uses ignored stdin for `-i`; it cannot prove a live TUI draft. Use the real PTY
helper for that test. Do not run broad gates for this research-only note.

**Later, explicitly authorized live check (not run):** prepare a disposable
directory under this worktree's `.tmp/`, an isolated `COPILOT_HOME`, a task-owned
plugin with the bridge extension, and a fixture-only MCP config. Use existing
authorized authentication without login or global changes. Minimal TUI launch
shape, after preparing those files and a fresh UUID:

```sh
COPILOT_HOME="$EXPERIMENT/copilot-home" copilot \
  -C "$EXPERIMENT/workspace" --no-auto-update --no-remote-export \
  --no-custom-instructions --disable-builtin-mcps --experimental \
  --plugin-dir "$EXPERIMENT/plugin" \
  --additional-mcp-config "@$EXPERIMENT/mcp.json" \
  --session-id "$EXPERIMENT_SESSION_ID" --model mai-code-1.1-flash \
  --max-ai-credits 30 --usage-output-file "$EXPERIMENT/usage.json" \
  -i 'Disposable test. On a Kanna notice read the synthetic mailbox once; acknowledge only on ACK. Keep replies under ten words. Never launch agents.'
```

Pin **`mai-code-1.1-flash`**: local model help lists it, and current
[GitHub pricing](https://docs.github.com/en/copilot/reference/copilot-billing/models-and-pricing)
places it at the lowest listed uncached input/output rates ($0.20/$1.20 per
million tokens), suitable for this small tool test. Actual account availability
is unverified; stop if unavailable rather than switch models. The CLI's minimum
30-credit soft cap is not a hard spend bound; also bound turns/time and record
actual usage/model. No inference is needed for the initial extension handshake.

Capture parent PID/session UUID, bundled SDK version and wire frames; type a
synthetic draft with the cursor in its middle; deliver while idle and during a
held tool; verify exact draft/cursor and the later separately submitted human
message. Hold a permission prompt and prove no automatic answer. Test disabled
extension, no registered wait, disconnect before/after queue acceptance, bridge
restart, and stale run. Separate queue receipt, actual notice/model consumption,
mailbox read, and ACK in evidence. Stop every owned process. If cross-boundary
implementation lands before live E2E is possible, add the convention-required
dated `docs/*-e2e-gap.md` with narrower tests and exact remaining gap; this
investigation does not claim to close that gap.

## Scope/exclusions

Only this note is changed. No product code, tests, settings, live subscriptions
(including `watch-1789157553830857000-0`), owner composers, other worktrees, new
agents/tasks, deployments, or messages to a manager terminal were touched. No
background processes remain. Parent owns synthesis, implementation, experiment
authorization, and any stage/merge decision.
