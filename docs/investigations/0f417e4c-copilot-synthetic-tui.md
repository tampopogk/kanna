# Copilot 1.0.64 synthetic TUI compatibility result

2026-09-15, task `0f417e4c`, tested from `c1b6e0632846b99d6106c41fb0f10601c115e8fc`.
**The exercised composition, busy enqueue and positive-history recovery cases
passed.** One disposable TUI ran for **6.571 seconds** within the authorized
90-second bound. Six requests reached a scripted loopback provider; no model
inference ran. This establishes a viable composer-independent Copilot transport,
not a completed Kanna adapter or rollout approval.

## Exact execution and isolation

The owner explicitly authorized this one synthetic `session.send` experiment,
superseding the previous no-live-send restriction for this test only. Cleared
the attention badge before execution. No owner session, production subscription,
account/global configuration, extra task, external provider or rollout was used.

| Identity | Observed value |
| --- | --- |
| Copilot runtime | **1.0.64**, confirmed by startup log and loaded SDK |
| PTY bridge / TUI / host-owned extension PIDs | `76555` / `76557` / `76917` |
| Launch, environment and joined session UUID | `91d56906-3b22-4fb5-a03c-4b8d3d4f6dcd` in all three |
| Native SDK | cached 1.0.64 `copilot-sdk/extension.js` |
| SDK SHA-256 | `c4d7588911b6feb44522489bb77712120a7bdedc44a82e32f58e280bfebdd9c1` |
| Configured model identifier | `kanna-synthetic-local`, a fixture name, not an account model |
| Provider | scripted OpenAI-compatible loopback `/v1/chat/completions`, canned SSE replies |

The existing `tests/cli-contract/helpers/pty-bridge.py` launched
`sandbox-exec -f "$FIXTURE/sandbox.sb"` with:

```sh
/Users/jeremyhale/.local/bin/copilot -C "$FIXTURE/workspace" \
  --no-auto-update --no-remote-export --no-custom-instructions \
  --disable-builtin-mcps --experimental \
  --session-id 91d56906-3b22-4fb5-a03c-4b8d3d4f6dcd \
  --model kanna-synthetic-local --log-dir "$FIXTURE/logs" --log-level debug
```

`COPILOT_OFFLINE=true`, provider URL/type/wire API, `COPILOT_HOME`, XDG, GH config
and TMPDIR were explicitly isolated under
`.tmp/copilot-live-synthetic-0f417e4c`. Inherited agent, provider and account
environment variables were filtered. No credential was supplied or copied.
The sandbox denied all network operations except outbound to the fixture's
loopback port, and denied file writes outside the fixture and `/dev`. Before
launch, probes verified the allowed socket connected while another listening
loopback port and an external address both failed with `EPERM`. External-provider
fallback was therefore blocked independently of Copilot's configuration.

An initial sandbox-profile validation **failed before any TUI launch** because
macOS requires `localhost`, not `127.0.0.1`, in that policy expression. Preserved
the failure, corrected the expression, and verified the restriction before the
single TUI run. Runtime diagnostics also retain a denied shared-cache lock-file
write, unavailable permission-service reset, missing authentication/remote, and
unknown synthetic model warning. These are recorded limitations, not passing
permission/account tests. The fixture used no tools requiring permission.

## Observed behavior

| Case | Concrete evidence |
| --- | --- |
| Empty composer | Native `send({prompt, mode:"enqueue"})` returned a message ID and caused the first local provider request without terminal input. |
| Partly typed draft | `KANNA_SYNTHETIC_DRAFT_LEFT_RIGHT` remained unchanged while batch 2 was submitted and answered. The draft was absent from both provider requests made before human submission. |
| Cursor preservation | Terminal cursor remained `[29,36]` before/after the notice. Typing `Z` afterward produced `KANNA_SYNTHETIC_DRAFT_LEFT_ZRIGHT`, proving the actual insertion position, not just screen coordinates. |
| Explicit synthetic human submission | Only the later Enter submitted that edited draft. Native history and the local provider request contain it as its own user message, separate from the engine notice. |
| Busy enqueue | The provider held `KANNA_SYNTHETIC_BUSY_HOLD`; batch 3 received an admission receipt and appeared as `Queued (1)` in the TUI. It did not reach the provider until the held response was released, then ran as the next message. |
| Lost extension-to-Kanna receipt | The extension deliberately withheld batch 4's admission receipt. A subsequent public `getEvents()` call returned exactly one matching `user.message`; the send count stayed one. No resend, PTY fallback or event acknowledgement occurred. |

The six native user-history entries and the six local requests agree on order:
batch 1, batch 2, edited human draft, held busy prompt, batch 3, batch 4. Engine
notices are native **user-role** messages with explicit Kanna wording. Nothing
in this result claims native system-role delivery or owner authorship.

For recovery, the suppressed queue receipt's message ID was
`b585c28d-aa06-40a7-8cd2-deeef1854ffb`, while the matching history event ID was
`532e24e9-604c-4e9c-a668-840bdc7f3c39`. They are **different identifiers**. A real
adapter must not join them by equality. Its persisted prepared attempt, exact
notice/binding and positive native history evidence must drive reconciliation.

The fixture's `reconnect-history` command advances a simulated Kanna transport
epoch and reads history through the existing host-owned SDK connection. It
**does not restart the extension process or reconnect native stdio**. This is
positive-history recovery after a lost extension-to-Kanna receipt, not proof of
process-restart recovery, loss of the host SDK response, or visibility of a
still-queued message in history. No live disabled/unregistered-extension case
ran. The earlier offline tests cover absent history and failed receipts only at
the fake-parent/prototype boundary; they do not fill those live evidence gaps.

## Cleanup, durable evidence and remaining work

All recorded owned PIDs are absent, the loopback listener is closed, and the
disposable repo, session store, config and XDG/GH/runtime directories were
removed. Synthetic traces and runner remain under `.tmp`; no credentials existed
to retain. Copilot reported zero-token/zero-cost usage for the canned responses;
those are synthetic telemetry, not a paid-account usage check.

[Machine-readable evidence](../evidence/0f417e4c-copilot-synthetic-tui.json)
preserves exact command/environment, binary and SDK hashes, sandbox probes
including the failed first profile, process identity, draft/cursor/queue screens,
ordered user messages, provider request excerpts, receipt/history IDs, fixture
source, raw artifact hashes, diagnostics and cleanup assertions. Post-run checks
independently verified all six messages/order, draft separation, unique history
match, PID absence and closed listener. No second TUI or repeated live run was
launched. A post-run verification assertion incorrectly expected 12 controller
actions and failed; the preserved log has 10. The corrected check passed for
exactly five SDK sends, four synthetic key writes and one history read. This
verification-script error and the initial sandbox failure remain in the evidence.

The next work is the scoped Copilot product adapter described in the
[implementation boundary](0f417e4c-copilot-engine-adapter.md): server-bound
extension registration, durable prepared attempts and native receipts, correct
original-run `engine` input recording, idempotent recovery and separate mailbox
read/ack. No ordinary-input lock may span native delivery. Missing or uncertain
delivery must retain the batch without PTY fallback. Disabled registration,
stale runs, process reconnect, absent history and late receipts need focused
integration tests when those boundaries exist. This experiment did not create
a Kanna subscription or test real model tool-read/ack behavior.

The original multi-harness collision fix remains unfinished. The earlier dirty
daemon/Claude prototype is unchanged by this experiment and has not received
the requested focused independent review. No additional paid inference or broad
provider experiment is needed or authorized by this report. No new attention
badge is set: the requested decision was supplied, and implementation work remains.
