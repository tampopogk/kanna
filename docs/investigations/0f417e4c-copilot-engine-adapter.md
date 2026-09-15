# Copilot engine notice: public enqueue without native system-role claims

2026-09-15, task `0f417e4c`, following preflight commit `500b47e1f`.

**Choose Copilot's host-owned extension as the next adapter.** The installed
SDK's missing `source` convenience field is not a blocker. Its public
`joinSession()` / `session.send({prompt, mode: "enqueue"})` path carries explicit
Kanna engine wording separately from the terminal composer. Kanna, not Copilot's
model role, owns the reserved durable `engine` source. No native system-role
claim, private RPC, SDK replacement or composer save/restore is necessary.

OpenCode has the same distinction: `prompt_async` synthetic text is still a
user-role message. Its metadata is not a system-role guarantee either. Both
can use a labelled notice with server-owned provenance. Copilot is the narrower
next step because actual same-session attachment is already demonstrated;
OpenCode still needs its authenticated existing-TUI listener/session binding
resolved. No additional OpenCode research was performed for this decision.

## What was implemented and tested offline

An unregistered compatibility prototype lives in
`tests/cli-contract/fixtures/copilot-engine-wake/adapter.mjs`. It builds the
explicit `[Kanna supervisor]` notice with a stable task/run/native-session/
subscription/batch marker, refuses a mismatched native session, calls only
public `send({prompt, mode: "enqueue"})`, and distinguishes admission from
uncertainty. It has no terminal handle, retry, acknowledgement or DB authority.
It is not loaded by any product launch.

The test driver imports the **actual installed SDK** into a Node child and
connects its stdin/stdout to a fake JSON-RPC parent. It never runs Copilot or a
model, inherits no account/provider configuration, and uses no network listener.
The parent answers only connection, join, send and history operations; unexpected
operations fail the test. Each owned child exits and is awaited; a deadline
kills it on failure. No credentials/config or background processes remain.

Command (replace the paths with the local installed artifacts):

```sh
KANNA_TEST_COPILOT_SDK_PATHS='["/Users/jeremyhale/Library/Caches/copilot/pkg/darwin-arm64/1.0.64/copilot-sdk/extension.js","/Users/jeremyhale/Library/Caches/copilot/pkg/darwin-arm64/1.0.83/copilot-sdk/extension.js"]' \
  pnpm --dir tests/cli-contract test tests/offline/copilot-engine-wake.test.ts
```

**13 tests passed**, 1.52 seconds. One shared binding/wording test and six cases
per SDK: exact public join/enqueue wire request; rejected, lost and malformed
receipts; reconnect/history without resend; and mismatched session refusal.
The live preflight's hashes still identify the artifacts:

| Runtime artifact | SDK SHA-256 |
| --- | --- |
| 1.0.64 | `c4d7588911b6feb44522489bb77712120a7bdedc44a82e32f58e280bfebdd9c1` |
| 1.0.83 | `7c6498e44e5e6d7718bdfb14ffa3b03b0eb07f51e2fc178c9a1771a6482b945d` |

This is wire/prototype evidence, **not real host queue or composer evidence**.
The fabricated `user.message` history event tests positive matching only; the
real host's history shape and when queued messages enter history still need
measurement. Missing history does not prove non-admission and never triggers a
retry. Matching text alone must not authorize an engine record: reconciliation
also needs the server's prepared attempt, run/session binding and native event
identity. An ambiguous match stays uncertain. No mailbox ack is inferred.

## Concrete implementation boundary

The causal collision remains the automatic `input` adapter's use of the same
logical terminal-input path as explicit human sends. A label changes provenance
wording, not that transport's composer interaction. The Copilot adapter instead
needs this bounded path:

1. Existing subscription observer admits and durably retains the ordered batch.
   Before sending, persist an attempt for its exact task/run/native-session/
   subscription/batch binding. A replaced run or disabled/unregistered extension
   is unavailable, never delivered. Registration must come from the verified
   host-owned extension and match the server's current run, not just a cwd.
2. Kanna supplies the complete notice to that extension. The extension enqueues
   through its joined public session. It does not accept arbitrary callers
   declaring `engine`, mutate the composer, launch another runtime or acknowledge
   the batch. Bind replies to the connection epoch and prepared attempt.
3. A native message-id receipt is queue acceptance, not model consumption. Keep
   that receipt separately from mailbox read/ack. On confirmed acceptance the
   server records the exact notice as reserved `TaskInputSource::Engine`, with
   the original run/stage and native receipt identity. Persist receipt plus input
   record atomically/idempotently so reconnect cannot create a second record.
4. Do **not** call the existing `record_task_input` blindly from a late callback:
   it selects the currently running stage and its documented contract requires
   PTY confirmation. Add a narrowly named native-receipt recording boundary
   with explicit captured run/stage and truthful queue-acceptance semantics.
   An additive receipt/attempt in the existing subscription JSON can preserve
   idempotency without redesigning the task workflow. Confirmed old-run delivery
   must never be stamped onto the new run.
5. No task-input/composer lock spans the SDK wait. Missing/rejected/malformed
   receipt leaves the same batch pending/uncertain; no PTY fallback or automatic
   resend. Reconnect first reconciles the persisted attempt against native
   history. An absent queued message stays unresolved, with visible diagnostic.
   Read and explicit batch acknowledgement remain separate; later events stay
   ordered behind that batch. Ordinary explicit input remains available.

Steps 1–5 describe the product integration still required, not code shipped by
this experiment. The prototype proves the send/inspection portion only. In
particular, pending retention alone does not satisfy timely supervision; real
enqueue execution and recovery must pass before enabling this adapter.

The existing server source already reserves `engine` from caller-declared input
sources and records it in the durable `task_input` row. The focused
`caller_declared_input_sources_are_a_closed_set` test passed. The existing
`subscription_wakes_manager_through_fenced_input_once_per_pending_batch` test
also passed (2.21 seconds), checking the exact engine text/record, retained batch,
and separate acknowledgement. Both ran with
`cargo test -p kanna-server --bin kanna-server <test-name>` against this worktree,
including its earlier uncommitted prototype changes. They exercise the existing
server/daemon fixture, not native Copilot receipt persistence. No full build or
suite was repeated.

## Smallest remaining experiment and authorization boundary

Request **one disposable Copilot 1.0.64 TUI, at most 90 seconds, using a scripted
loopback OpenAI-compatible provider, no paid inference or account credentials**.
Reuse the preflight's task-local config, UUID identity checks, offline mode and
PTY bridge. Pin the actual runtime/artifact; stop rather than silently switching
runtime or contacting a remote provider. The configured model is a synthetic
fixture identifier, with canned responses only, not a claimed available model.

The test must invoke live `session.send`, which the current authorization
explicitly excludes. Hence this next experiment has **not run**. The owner's
Claude/Haiku authorization does not cover it. This is the concrete experiment
decision, not a new broad implementation or rollout gate.

Use the same labelled notice and public SDK tested here. Start with no draft;
then type a disposable synthetic draft and move its cursor into the middle.
Enqueue a notice and compare draft text/cursor before and after; insert a marker
at that cursor and manually submit only the synthetic draft. Capture the local
provider's request to prove the notice and draft are separate messages. Hold one
scripted response to test busy enqueue ordering. Drop one receipt, reconnect the
extension to the same UUID, inspect real history/queue visibility, and verify
that no second send is made while uncertainty remains. A disabled/unregistered
extension must make no send. Use synthetic mailbox IDs, never the manager's
subscription, and do not infer an event acknowledgement from any transport result.
Stop all owned processes and delete disposable config/session state afterward.

This deliberately proves host composition and transport without asking a model
to supervise. A later paid model experiment, if needed to prove actual tool-read/
ack behavior, remains a separate explicit decision. No live sends, account
changes, ordinary-input changes, production subscriptions, rollout, push/PR,
new agents/tasks or manager-terminal messages occurred in the offline work.

The original collision fix is still unfinished. Earlier uncommitted daemon/
Claude prototypes are not validated or enabled by this result, and this note is
not their independent review. The completed attachment preflight is preserved
in [its report](0f417e4c-copilot-preflight.md); historical next-step language in
the comparison is superseded by this bounded recommendation.
