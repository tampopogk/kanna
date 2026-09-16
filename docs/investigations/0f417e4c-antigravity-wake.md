STOP-and-escalate: No verified local transport can deliver an unsolicited engine
notice to the **same running `agy` TUI** while preserving its draft and cursor.
Do not select an Antigravity native-wake adapter on the present evidence.
Research is complete; selecting one requires the isolated compatibility evidence
below. Mailbox-only retention is safe but does not satisfy unattended wake latency.

## Evidence verified

Investigation date: 2026-09-15. Child `c481df3a`, parent `0f417e4c`, branch
`task-c481df3a`; inspected clean base
`ffb3b7cf0d58a2cea9e4a911443d87a3b1e7d048`.

- Read the parent's full `kanna_get_task` and all three durable inputs. The
  original requirement protects drafts from **automatic supervisory events**;
  ordinary explicit input retains its always-submit contract. The inputs contain
  the bounded Claude experiment authorization. The child assignment separately
  supplies the newer investigation-child authorization; it is not an additional
  parent input row.
- Read the committed [Claude report](../2026-09-14-claude-channel-live-compatibility.md)
  and inspected its [evidence structure](../evidence/0f417e4c-claude-channels-live.json).
  It reports Claude 2.1.270/Haiku results, not Antigravity compatibility. Its source
  hashes describe uncommitted prototypes, which are absent here and were not read
  from another worktree.
- **Local availability:** `command -v agy`, Python `shutil.which`, and
  `zsh -lic 'command -v agy || true'` found no executable. Checked the resolver's
  user-install candidates: `~/.opencode/bin`, `~/.local/bin`, `~/.bun/bin`,
  `~/.npm/bin`, `/usr/local/bin`, `/opt/homebrew/bin`, plus this worktree's
  `node_modules/.bin`; no `agy` exists there. This is bounded discovery, not a
  whole-machine inventory or inspection of an installed app bundle. No local
  CLI version/help/source or live transport result is available.
- Kanna's committed launch preamble cites historical **agy 1.0.14**. Current
  official documentation labels the CLI **1.2.0**; its
  [changelog](https://antigravity.google/changelog) dates remote-control service
  subcommands to 1.2.0, September 10. These are documentation versions, not an
  installed version or a proven minimum for every feature. The public
  [CLI repository](https://github.com/google-antigravity/antigravity-cli) exposes
  documentation/examples/releases, not the TUI implementation needed to inspect
  draft handling. No provider inference, mock transport experiment, or live test
  was run in this child.

### Advertised interfaces and their limits

| Interface | Verified public contract | Relevance to this decision |
| --- | --- | --- |
| Remote Control | `agy remote-control start` registers an OS service; `status` reports its instance name; `stop` unregisters it. Browser access uses the same Google account; the daemon uses CLI credentials. | Cloud dashboard plus a separately managed daemon. No documented local notice endpoint, TUI PID binding, engine-message type, or draft guarantee. Neither enabling nor testing it is authorized here. [Google Remote Control](https://antigravity.google/docs/remote-control/) |
| Headless input | `agy --input-format stream-json --output-format stream-json` accepts `{"event":"user","message":{"content":"…"}}` on its own stdin. `agy -p … --conversation <conversation_id>` starts a new process. `control_request`/`control_response` terminate streaming with an error. | A newly owned headless process is not attachment to an existing interactive process. User events also do not supply engine provenance. No native control envelope should be invented. [Google headless contract](https://antigravity.google/docs/cli/headless) |
| MCP client | Workspace `.agents/mcp_config.json` supports `mcpServers` with local `command`/`args`/`env`, or remote `serverUrl` and authentication. `/mcp` manages connections; unconfigured tools require approval. | Can return a tool result on an outstanding call. The docs do not advertise an unsolicited text-channel extension or a detached MCP wait that wakes an idle root agent. Kanna currently does not register its MCP with agy. [Google MCP](https://antigravity.google/docs/mcp) |
| Official ACP | The [ACP registry](https://raw.githubusercontent.com/agentclientprotocol/registry/main/antigravity-acp/agent.json) lists Google LLC's proprietary **1.1.1** binary, `./agy_acp_server.par` on macOS/Linux, distributed from `dl.google.com/agy-extensions`. Google documents installing Antigravity through [Zed's external-agent registry](https://antigravity.google/docs/ide/extensions/zed). | Official support exists, but this is a different executable from `agy`. Neither source documents attaching it to a running CLI TUI or sharing that TUI's live conversation writer. Its registry version is not the agy version. |
| Third-party ACP wrapper | [`shindgew/agy-acp`](https://github.com/shindgew/agy-acp#architecture) describes one interactive PTY per ACP session, `--conversation` reuse, and reading conversation SQLite for updates. | Not Google's ACP implementation, not proof of a native local channel. It owns its own PTY and can auto-install agy; do not run it for this investigation. |

Google's [background-task guide](https://antigravity.google/docs/cli/subagents/)
advertises concurrent tasks/subagents and continued drafting. That does not
specify unsolicited MCP delivery, root-agent wake scheduling, or exact cursor
preservation on tool completion. A model such as Fable keeping its own wait
thread alive is model behavior atop a harness facility. It does not establish
that `agy` exposes that facility, nor does one missed notice justify redesigning
Kanna's observer or scheduler.

## Affected producers/consumers/lifecycle owners

The deployed path at the inspected head is:

1. Server DB writes produce durable task events. The server's
   [event observer](../../crates/kanna-server/src/http_api/task_events.rs)
   performs scoped local/remote waits; subscription relevance, batching and
   admission belong to [event_subscriptions.rs](../../crates/kanna-server/src/http_api/event_subscriptions.rs).
2. A persisted [subscription](../../crates/kanna-server/src/db/event_subscriptions.rs)
   binds task, stage, branch and run; one pending batch provides backpressure.
   The adapter receives subscription/batch IDs. At this head
   [harness_wake.rs](../../crates/kanna-server/src/http_api/harness_wake.rs)
   offers only `input`, opt-in `codex_app_server`, and mailbox-only `poll`.
3. `input` calls `send_engine_wake` in
   [task_input.rs](../../crates/kanna-server/src/http_api/task_input.rs), checks
   the run under a task-mutation lease, identifies the live daemon session/PID,
   and sends `SubmitInputIfSession`. It does not check composer occupancy.
4. The daemon [connection](../../crates/daemon/src/connection.rs) checks
   incarnation/lifecycle, [session queue](../../crates/daemon/src/session.rs)
   frames a logical message, and [writer](../../crates/daemon/src/output.rs)
   writes text followed by CR with 150 ms compatibility pacing. This uses the
   provider's existing composer and therefore permits the reported collision.
   A label or longer delay cannot make that transport draft-safe.
5. After confirmed PTY delivery, the server records the text as reserved
   `engine` in [task_input](../../crates/kanna-server/src/db/task_inputs.rs).
   Unknown delivery is not recorded as success. Mailbox read is independent;
   only acknowledgement of the current batch advances its cursor.

The [provider registry](../../crates/kanna-agent-protocol/src/providers.rs)
permits PTY, rejects headless and model overrides for Antigravity at this head.
The [launcher](../../crates/kanna-server/src/task_creator/commands.rs) uses
`--prompt-interactive`, effort/permission flags and a worktree alias;
[environment setup](../../crates/kanna-server/src/task_creator/environment.rs)
owns executable resolution and Kanna task/server/token metadata.
[Resume](../../crates/kanna-server/src/task_creator/resume.rs) explicitly
cannot assign/capture an Antigravity conversation ID at PTY spawn. Thus neither
a Kanna task ID nor a cwd is a verified native conversation attachment handle.
Desktop/mobile terminal users share the daemon's input stream; fixing automatic
delivery must not change their ordinary input behavior.

### What a pending tool can actually provide

[MCP `tools/call`](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
has a request ID and a matching result containing text. Returning engine-labelled
data on **that existing connection and call** avoids the PTY input path by
construction. It is not an arbitrary push to a conversation ID. Standard tools
support does not promise a background scheduling/UI policy.

Kanna's existing `kanna_wait_events` reads `/v1/task-events`; it is not a wait for
a subscription's admitted mailbox batch. `kanna_read_event_subscription` is an
immediate read/optional acknowledgement, not a blocking wake registration. A
future mailbox-wait tool would therefore be an explicit small API extension,
not a feature already available by changing agy's configuration. It should wait
on the existing mailbox/admission state rather than duplicate event collection.
The CLI fallback can likewise return output to an already running tool, but does
not establish automatic continuation after a background command exits.

For such an experiment, the provider owns the MCP child and outstanding request;
Kanna owns subscription/run binding. Pass the synthetic task/run and server URL
to a local stdio MCP child, carrying Kanna's local control credential through the
existing token-path mechanism. No cloud account token belongs in a new local
notice protocol. Google provider authentication is separate. Public callers
cannot claim `source: engine`; native tool results need no fabricated owner-input
record. Local/remote MCP authentication options are not evidence of an inbound
control API on agy.

## Invariants and failure modes

The following are **inferences/requirements**, not measured agy behavior:

| Session state | Safe conclusion |
| --- | --- |
| Idle, no pending tool | No verified local wake target. Retain the batch and report unavailable delivery; do not type a nudge. |
| Existing pending MCP wait | Text can be returned as that call's result; actual root continuation and cursor preservation require live proof. |
| Busy | An unrelated running turn is not an outstanding notice call. Do not assume results interrupt it or that receipt means mailbox reconciliation. |
| Human draft exists | Never use the ordinary PTY adapter, clear/retype the draft, or infer safety from a stale empty screen. |
| Permission prompt | Do not send Enter or bypass permission to wake. A wait awaiting permission has not necessarily been registered; unrelated approval may delay consumption. |
| No wait, canceled wait, timeout/disconnect | No recipient is proved. Preserve pending state; a canceled request ID is not reusable on another connection. |

Preserve FIFO batches, CAS protection against late delivery results, current-run
binding, and separate **transport receipt → mailbox read → explicit ack**.
Crash during sending must retain uncertain state, as the committed observer
already does. Reconnect must bind a fresh transport incarnation to the correct
live run and reconcile the same unacknowledged batch before replay; unknown
delivery must never trigger native-to-PTY fallback. Old servers, old agy builds,
unsupported capabilities and absent IDs must fail closed. Waiting for a notice
must never acquire an input lock that strands later human submissions. Cleanup
belongs to the owner of each tool connection/process, without killing the TUI.

## Alternatives and tradeoffs

1. **Keep unconditional supervisory PTY injection:** smallest code change, but
   violates the original objective. Reject.
2. **Mailbox-only with explicit unsupported-wake status:** smallest safe interim
   limitation. Keeps durable ordering/ack and human input usable, but cannot
   promise timely unattended service. This is not completion of the parent fix.
3. **An already-registered wait/tool completion:** best bounded experiment.
   Preserves native tool provenance without a second agent writer. Requires agy
   MCP wiring, a mailbox wait contract, and proof of background/idle behavior.
   It cannot cover sessions where no wait exists merely by assertion.
4. **Daemon-atomic empty-composer admission:** possible separate fallback work,
   only if fresh draft attestation and the actual write are serialized with all
   human input. A server-side check then write is racy. Unknown/draft/permission
   states must defer without locking explicit input; delivery waits for a real
   admissible edge and may be unbounded. This investigation does not validate it.
5. **Remote Control, ACP, or headless rehosting:** larger session-ownership/auth
   change with no established attachment contract. Do not launch a second
   `--conversation` writer or reverse-engineer private cloud RPCs as a fallback.

The manager must decide whether Antigravity is temporarily ineligible for
unattended supervision or whether to seek a separately authorized isolated
experiment. No owner decision relaxing draft safety or wake latency is inferred.

## Acceptance criteria

- Any native selection must identify a pinned installed CLI, supported endpoint
  or pending-call mechanism, authentication, task/run/process binding, and
  readiness/version gate. **No such unconditional agy endpoint is established.**
- Prove draft bytes and cursor survive while engine text reaches the same root
  session, and that the next human submission is unchanged. Prove busy,
  permissions, absent-wait, reconnect, crash, stale-run and uncertain outcomes.
- Preserve the existing observer, batching/admission, receipt/read/ack separation,
  and engine provenance. Test two ordered batches with the first unacknowledged.
- Preserve ordinary `send_task_input`; do not alter the existing subscription
  `watch-1789157553830857000-0`. No provider upgrade, global config, remote service,
  or session migration is an implicit prerequisite accepted by this verdict.

## Required E2E coverage

No live coverage was possible here: agy is absent and inference/install/login are
out of scope. The [test taxonomy](../dev/testing.md) and existing
[Antigravity coverage gap](../2026-07-01-antigravity-cloud-snapshot-e2e-note.md)
make this explicit. Mock protocol success cannot prove TUI draft behavior.

For later implementation, require focused Rust server/daemon integration for
pending-batch retention, ordered ack, reconnect/CAS races, unavailable capability,
and unchanged explicit input; offline CLI contracts for launch/config gates.
Add one isolated fake-provider server→daemon→PTY E2E for the chosen fallback.
A native path additionally needs a deliberate live CLI contract using
[`pty-bridge.py`](../../tests/cli-contract/helpers/pty-bridge.py) and rendered
cursor evidence. Until available, implementation must carry the dated E2E-gap
note required by the repository; this research does not claim replacement tests.

Minimal **future-only** live launch, from an isolated fixture directory beneath
this worktree's `.tmp`, after installed help verifies every flag and isolated
config/auth handling is established:

```sh
/absolute/verified/agy --model gemini-3.8-flash-medium --effort low \
  --prompt-interactive 'Call the fixture wait tool once. On its result say OBSERVED. Do not acknowledge or run other tools.'
```

Use one synthetic MCP server in that directory's `.agents/mcp_config.json`, one
bounded pending call, and release it while a synthetic draft has a middle cursor;
then test absent-wait and one busy/permission case. This is a proposed command,
not a tested invocation. Google's [headless model list](https://antigravity.google/docs/cli/headless)
documents the pinned Flash slug and `agy models`; its
[model availability](https://antigravity.google/docs/models) includes Flash on the
free tier. Flash is the low-cost candidate, but **the cheapest entitled choice and
actual usage cost cannot be verified without the unavailable CLI/account**.
Confirm once before the experiment; stop on unsupported pin or quota, with no
silent model substitution. Record CLI hash/version, selected/actual model, PID,
conversation ID if exposed, MCP request/result trace, draft/cursor before/after,
human submitted text, mailbox/ack states, usage and cleanup. No cloud Remote
Control or ACP process is needed for this first bounded test.

## Scope/exclusions

Only this note is changed. No product/test/config edits, provider launches,
credentials read, installs, new agents/tasks, other-worktree inspection, UI
automation, rollout, broad gates, live subscription changes, or manager-terminal
messages. No owned background processes remain. Parent owns synthesis and any
implementation decision; this verdict does not advance or close a stage.
