# Kanna hosted agent terminal

Task 93a5a7d0 imports the reviewed agent-tui unchanged at 073f522f3, then
integrates it into Kanna. Source: foobar-24 commit
859dcd7d495e219e17d1cff200f447e25993cef5. Original task 2e6fbb4a and design
artifacts ad68cbd2a1f2927f519089391b77cfac25325448 and
de92bf19dfe5fdec64918f3c3f42b9ed6f0fec9c remain in repo-18d8f429a6316298.

## Configuration and execution

`agentFrontends` maps only `claude` and `codex` to `native` or `agent-tui`.
Omission is native. It is resolved after provider/model selection; Kanna's own
repository enables both hosted frontends. SDK/headless sessions are unchanged.
The bundled sidecar launches the resolved real harness with typed native option
translation, including MCP, permissions, model/effort, initial prompt and resume.
Unknown/incompatible explicit options fail; there is no silent native fallback.

The desktop new-task window offers **Custom TUI mode** for Claude and Codex.
Checked selects `agent-tui`; unchecked selects `native` for that task. The
`agentFrontend` creation option is stored with the task and overrides repository
defaults across later stages, recovery and resume. API callers that omit it
continue to inherit repository configuration.

Claude uses bidirectional stream-json and correlated replayed user UUIDs.
Codex uses initialize, thread/start or thread/resume, and turn/start JSON-RPC.
The original six skins, sprite, cards and keyboard behavior remain; the header
adds queued count and hosted `/new` refuses changing the bound conversation.

## Input and runtime contract

The frontend owns a private Unix socket and fsynced receipt journal, allowing
same-incarnation daemon adoption. An older successor without the hosted protocol
capability is refused while a hosted session exists. Registration uses a per-spawn capability,
peer UID and exact task/run/session/incarnation binding. The harness does not
inherit the capability configuration environment variable. JSON/frame, queue
and receipt limits are enforced. Provider output cannot issue hosting commands.

Logical input is separate from PTY keys and drafts. Acceptance records a durable
FIFO sequence; only the app event loop dispatches at Ready with no pending card.
The submitting journal boundary precedes provider writes. Correlated acceptance
becomes submitted; a crash without proof becomes uncertain and is never blindly
resent. Submitted input plus its delivered ledger entry commit together, once.
Retries must retain their delivery ID and payload. HTTP 202 carries the receipt;
CLI/MCP/mobile preserve queued versus submitted/failed/uncertain outcomes.
Native HTTP 204 behavior remains compatible. Query `/input-deliveries` or
`kanna_task_input_deliveries`; uncertain text and referenced attachments remain.
Migration 105 carries attempts through disk rebuild; unresolved attempts prevent
transfer. Replacement retires old queues rather than forwarding them to a new run.

Structured frontend busy/waiting/idle and composer attestations supersede native
screen detection. Channel loss is unavailable, never inferred idle. Session IDs
and native transcript references are recorded on the exact run. Typed provider
quota/capacity failures feed existing recovery policy. Resume loads bounded
history for display only; old cards/messages are not executed by the reader.

## Hosted acceptance, 2026-09-28

An isolated `./kd dev up` used the reserved port 48122, task-owned database,
daemon directory and fixture repositories under `.tmp`. Both providers launched
through normal Kanna workflow creation, with the real configured kanna-mcp.
Installed versions: Claude 2.1.284; Codex 0.158.0.

| Fixture | Provider and identity | Evidence |
| --- | --- | --- |
| ed36e047 | Claude haiku, low; 9d470ab6-ce3a-4b68-94fd-267f74bce78f | MCP inspect and completion; idle/mid-turn multiline inputs; same-ID retry; preserved draft; interrupt; new-process resume; recall of earlier IDLE/MID inputs and 12-second sleep; post and revision resume |
| 5da7dd6e | Codex gpt-6-sol, low; 01a0e9f5-39bf-7733-856c-ee0f3f047172 | Same checks, native app-server thread retained across restart/resume/post/revision |
| 4c9bfbee | Claude acceptEdits; 26644e03-51f5-4d67-ad2e-da0f42d02c77 | Actual outside-worktree write approval, waiting state, queued text/draft isolation, raw Deny and Allow once |
| cf3eb935 | Codex workspace-write; 01a0e9fa-3622-7c12-80be-b7ef52865908 | Actual escalation approval, waiting state, queued text/draft isolation, raw Deny and Allow once |

Desktop/server restart replaced the daemon (85106 to 81973) while approval was
pending. Frontend PIDs, draft, card, queued delivery and provider identities
survived adoption. Kanna post-check runs recorded success via MCP with the prior
conversation marker; revision runs reopened the original provider IDs.

`python3 tests/hosted-frontend/run.py http://127.0.0.1:48122` exercises the real
server/daemon/frontend against the checked-in controlled provider. It passed:
delayed initialization and initial-first dispatch; concurrent FIFO inputs;
same-ID retry and changed-payload conflict; draft/approval isolation; explicit
deny; one confirmed input entry each; provider crash before acknowledgement as
uncertain; queue bound and retirement of all accepted pending entries.

Native transcript and ledger audit found exactly one IDLE and one MID message
per provider and exactly one corresponding delivered input entry each, in order.
Raw `/new` retained the bound conversation; skin-picker and page keys reached the
frontend through Kanna raw input.

The Bazel blocker was fixed in task 96755839, imported as 31bccff19. The target
`bazel build //crates/agent-tui:agent_tui` now passes. It preserves host-tool debug
information to avoid a macOS 27 Mach-O stripping defect; shipped target stripping
is unchanged. Sidecar builds stage all seven executables.

## Verification and remaining limits

- Imported frontend suite: 105 passing tests, including the original replay,
  rendering and process tests plus hosting coverage.
- CLI: 127 unit tests and 25 integration tests pass. Daemon: 350 library and
  376 binary unit tests pass; current-version handoff integration passes.
- Server: the broad run passed 2,415 tests. Five new migration/route/rebuild
  fixture failures were fixed and rerun successfully. Focused reruns passed
  380 task-creator tests, 55 task-input tests, 15 raw-input tests, four durable
  delivery tests, both disk rebuild cases, attachment tests, the route audit,
  engine-wake receipt/authorization tests and the two timing cases that failed
  in the broad run.
- Core config: 30 tests pass. Mobile transport receipts: 114 tests pass and
  mobile TypeScript checking passes. Protocol type generation and desktop
  frontend build passed through `./kd test rust`.
- Runtime/packaging contracts: 69 tests pass. Catalog/schema contracts: 68
  tests pass.
- Bazel frontend build and the macOS proc-macro regression test pass.
  `./kd build sidecars` stages all seven executables.
- Focused clippy passes for frontend, daemon, server and CLI with the existing
  `nonminimal_bool` and `items_after_test_module` lints allowed. Final daemon
  clippy also passes without allowances.

The full Rust gate is **not green**: it stops at existing clippy findings in
`desktop_views.rs`, `workspace_setup_logs.rs`, and the desktop mobile-command
await-lock test. The broad server run also fails six LAN/relay tests with
"desktop relay has not connected" and the KSP request-saturation timeout; the
latter still fails alone. The daemon's eight shipped-v2 handoff cases cannot
build their fixture because this checkout lacks `v0.1.0-staging.1`. These are
reported limitations, not claimed passes. No unrelated fixes were made.

Live native desktop visual inspection remains **unverified**. Computer-use
inventory exposes the installed Kanna apps but cannot bind the isolated,
unbundled development window; selecting the installed app showed a blank
window. Real hosted PTY output, keys, cards, state and transcript behavior were
observed through Kanna, but a person still needs to inspect resize/scroll and
skin rendering in the actual development agent terminal. No standalone terminal
demo is offered as a substitute.

The final controlled-provider rerun passed on the final sidecars (evidence in
`.tmp/hosted-contract-gcmt1uc3/`). A second daemon adoption, 81973 to 37826,
retained all four real-provider sessions. All fixture tasks are closed,
`./kd dev down --kill-daemon` completed, and checks found no remaining owned
processes, frontend sockets, or listeners on the reserved desktop/API ports.
Private receipt journals remain as reconciliation evidence. A restart
initially refused authentication because Cargo test binaries in `.build/debug`
shadowed the staged target-triple sidecars; moving those generated artifacts
aside restored the pinned executable paths. No authorization check was relaxed.

## Per-task checkbox follow-up

The new-task checkbox was verified with 114 desktop component, creation and
request tests, a clean `vue-tsc --noEmit`, and all 186 task-creator core tests.
The Rust coverage includes explicit native/agent-tui/inherited settings,
fresh-task persistence and database reload after creation-intent removal.
`./kd build sidecars` rebuilt all seven bundled executables. The real
server/daemon/frontend contract passed with the fixture repository defaulting
to native and each task explicitly requesting agent-tui, proving that the
per-task choice reaches the actual launch. Existing original-acceptance limits
above remain separate from these checkbox checks.
