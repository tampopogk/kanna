# Claude Channels compatibility experiment — task 0f417e4c

The native channel preserves an unfinished human composer, including its cursor,
and keeps engine provenance separate. The prototype is **not ready to enable**:
startup can lose its one-shot probe, and a busy-turn notification can be consumed
by Claude without the model reading the mailbox. Neither case acknowledged or
removed the durable event. Return for a scoped implementation decision; this is
an experiment result, not completion of the original input-lifecycle fix.

## Exact environment and isolation

- Worktree/branch: `task-0f417e4c`; base head
  `9ef7f71b6d40c83e3a951c1a7ef825af8c95c8e3`.
- Installed executable: `/Users/jeremyhale/.local/bin/claude`, resolving to
  `/Users/jeremyhale/.local/share/claude/versions/2.1.270`.
- Model: **`claude-haiku-4-5-20251001`**, pinned explicitly, verified on every
  recorded assistant API message. No fallback model, subagent, or other model.
- Existing authentication: first-party claude.ai Max account. Authentication
  secret read into the disposable process environment only. No policy change.
- MCP protocol negotiated by the actual CLI: `2025-11-25`.
- Capability: `experimental: {"claude/channel": {}}`; notification method:
  `notifications/claude/channel`; server name `kanna-mcp`. No permission relay.
- Synthetic session: `90b83807-8f8d-4c73-b132-ff0f5189fcaa`.
- Real PTY via existing `tests/cli-contract/helpers/pty-bridge.py`, 120x40;
  rendered snapshots decoded using disposable `pyte` installation.
- Real newly built `kanna-mcp` relay and real server routes/subscription service
  in the existing Rust `Fixture`. Its isolated test database contained only
  synthetic `child-c` / `manager-run` and `child-a`. Ephemeral server
  `http://127.0.0.1:61828`, test control `127.0.0.1:61829`.
  Synthetic subscription `watch-1789412171988045000-0`.
- No desktop UI, production/staging app, owner composer, live subscription,
  manager-terminal input, new durable task, or deployment was used.

The MCP proxy forwarded the prototype's real notifications and HTTP-backed tool
calls unchanged. It reduced tools/list to confirmation and mailbox read/ack,
shortened their descriptions, and added a harmless eight-second `experiment_hold`
tool to exercise busy delivery. It logged the wire protocol. The fixture's
control socket only emitted synthetic events, inspected the mailbox, disconnected
the synthetic channel, and stopped the fixture. It never injected PTY input.

## Invocation

The runner substituted absolute worktree paths for `$EXPERIMENT` below; its cwd
was `$EXPERIMENT/enabled-account`, under this worktree's `.tmp/claude-channel-live`.
The same CLI ran without the development-channel flag for the negative case.

```sh
claude --model claude-haiku-4-5-20251001 \
  --session-id 90b83807-8f8d-4c73-b132-ff0f5189fcaa \
  --setting-sources '' \
  --settings "$EXPERIMENT/enabled-account/settings.json" \
  --strict-mcp-config --mcp-config "$EXPERIMENT/enabled-account/mcp.json" \
  --tools '' --disable-slash-commands \
  --system-prompt 'Disposable compatibility test. Use only supplied MCP tools. Confirm channel probes immediately. On event wake read the mailbox once, say OBSERVED, and do not acknowledge until the human says ACK. On ACK acknowledge the last observed batch. On BUSY call experiment_hold then say DONE. Otherwise reply OK. Never launch agents. Keep replies below ten words.' \
  --dangerously-load-development-channels server:kanna-mcp
```

Process-local environment: isolated `CLAUDE_CONFIG_DIR`, existing
`CLAUDE_CODE_OAUTH_TOKEN` held only in memory, `CLAUDE_CODE_MAX_OUTPUT_TOKENS=256`,
`MAX_THINKING_TOKENS=0`, `DISABLE_AUTOUPDATER=1`, `TERM=xterm-256color`.
Inherited Kanna/Claude/Anthropic/Codex variables were excluded. The MCP child alone
received `KANNA_CLAUDE_CHANNELS=1`, `KANNA_CLAUDE_CHANNEL_HOST=1`,
`KANNA_TASK_ID=child-c`, `KANNA_STAGE_RUN_ID=manager-run`, and the fixture URL as
`KANNA_SERVER_BASE_URL`. It launched `.build/debug/kanna-mcp serve`.

Settings disabled thinking and allowed only the three synthetic-test MCP tools.
No machine-global settings were changed. The disposable trust dialog and native
local-development confirmation were accepted under the owner's explicit scope.
A temporary ignored Rust fixture wrapper had a 900-second maximum; the PTY
controller had a 600-second accept timeout. Both were explicitly stopped sooner.
The temporary include was removed from product test sources afterward.

## Observed results

1. **Unregistered channel:** without CLI channel opt-in, initialize advertised
   the capability and the MCP relay emitted a probe. Claude never confirmed it.
   Batch 1 remained `pending`, with the explicit unconfirmed-channel error. No
   owner input or model turn occurred, and nothing was acknowledged.
2. **Cold disposable authentication setup:** an enabled launch with no cached
   account metadata and nonessential traffic disabled said
   `--dangerously-load-development-channels ignored (server:kanna-mcp)` and
   `Channels are not currently available`. It also retained batch 1. This was
   **not evidence of an organization restriction**: using the existing account's
   metadata in the isolated config and normal feature discovery reached the
   development confirmation and worked. No feature flag or policy was forced.
3. **Startup race:** the relay emitted its initial probe during MCP initialization,
   before the disposable CLI completed its startup/preview UI. No confirmation
   followed. A fixture-forced transport reconnect after startup produced a new
   probe, which Haiku immediately confirmed. The exact CLI drop point was not
   instrumented; the observed missing confirmation is sufficient to disprove
   reliance on a single initialization-time probe.
4. **Draft and cursor:** typed `SYNTHETIC alpha omega`, moved five positions left,
   and received the probe and batch 1. The composer still contained that exact
   unsent text, with cursor x=18 (zero-based, before `omega`). Inserting
   `CURSOR ` landed in the middle. Another disconnect/reconnect replayed batch 1;
   the draft remained `SYNTHETIC alpha CURSOR omega`, cursor x=25. Haiku recognized
   the already-observed batch and did not repeat its read or acknowledge it.
5. **Actual human submission:** only after the channel deliveries, moved to the
   end, appended ` ACK batch 1 only, then BUSY.`, and pressed Enter. The recorded
   user message was exactly
   `SYNTHETIC alpha CURSOR omega ACK batch 1 only, then BUSY.` No channel text was
   concatenated into it. These operations affected only the synthetic composer.
6. **Ordering and busy delivery:** emitted the second event while batch 1 was
   still unacknowledged. It remained behind batch 1. After explicit ACK1, the
   relay sent batch 2 at `2026-09-14T19:00:37.494Z`, during the live turn. Claude
   queued it while the synthetic hold tool ran, then removed it from its native
   queue at `19:00:45.577Z` with reason `absorbed_mid_turn`. The transcript
   contains a separate attachment with `origin.kind=channel`, server
   `kanna-mcp`, and `isMeta=true`. Haiku finished with “Batch 1 acknowledged.
   DONE.” **It did not read batch 2 in this turn.** The mailbox retained it,
   although the prototype called the transport `notified`.
7. **Idle recovery and separate acknowledgement:** another confirmed reconnect
   replayed the same batch 2. Haiku read it and reported observation. It remained
   pending until the separate synthetic `ACK batch 2.` message. Final mailbox:
   `batchId=2`, `pending=null`, `wakeState=idle`, no error.

[Machine-readable evidence](evidence/0f417e4c-claude-channels-live.json) records
mailbox snapshots, composer text/cursors, exact tool arguments, native busy
provenance, model usage and SHA-256 hashes of the uncommitted prototype sources.
Raw PTY/MCP traces and temporary drivers remain under `.tmp/claude-channel-live`
for same-worktree investigation. Copied account configs and all temporary
credential-bearing process environments were removed; all owned experiment
processes exited. The test framework owns cleanup of its isolated database.

## Usage and checks

The transcript contains 15 distinct assistant API message IDs, all pinned Haiku:
26,103 input tokens, 5,989 cache-creation input tokens, 30,504 cache-read input
tokens, and 789 output tokens. Usage was aggregated once from message IDs,
taking the maximum field value when a message had multiple transcript entries.
There were two explicit synthetic human submissions. No separate billing charge
was available; these are recorded token counts, not an inferred invoice.

Focused checks against the prototype:

- `cargo test -p kanna-mcp --bin kanna-mcp claude_channel`: **3 passed**.
- `cargo test -p kanna-server --bin kanna-server claude_channel`: **2 passed**.
  Covers disabled/unconfirmed connection, stale run, lost transport receipt,
  retained pending batch, ordered backlog, confirmed reconnect replay, separate
  acknowledgement, and no fabricated owner `task_input` rows.
- Disposable live Rust fixture: exited successfully after explicit stop.

An earlier fake-provider remote composer test failed during setup because the
new catalog entry omitted `response: json`; this was corrected before the live
experiment and the focused checks above. That remote test and the guarded PTY
fallback are **not claimed verified by this experiment**. No broader gate or
independent review was run in this experiment; the manager owns later selection.

## Scoped decision

Keep Claude Channels opt-in and unshipped. Continue only with a bounded readiness
and consumption contract: retry a probe from actual pending-wake admission once
the CLI can listen, distinguish transport-written from mailbox-read, and arrange
an event-driven follow-up when a busy turn finishes without reading its pending
batch. Test these specific cases; do not infer delivery from stdout or acknowledge
on receipt. Never fall back to composer injection after uncertain native delivery.
No need to yank/retype a person's draft, alter ordinary explicit send-task-input
semantics, or change live subscriptions.

The original collision cause remains the automatic adapter's use of the ordinary
always-submit PTY path. The prototype's guarded fallback and native adapter are
unfinished local work, not a completed fix. Only this experiment report/evidence
is committed for the requested decision; source changes remain in this worktree.

Primary references: [Channels contract](https://code.claude.com/docs/en/channels-reference),
[preview availability](https://code.claude.com/docs/en/channels), and
[model IDs and pricing](https://platform.claude.com/docs/en/models/overview).
