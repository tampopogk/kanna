# Disk authority inventory (spec §16.11, T13 first increment)

Date: 2026-09-23. Schema: migrations through `098_stage_workspaces` (T2).

This is the input to the T13 cutover. It lists every durable table in
`crates/kanna-server/src/db` and says, for each: which disk record will be its
authority, what the T0 ledger already carries, and what is missing. Tables
that hold statistics or transient live state are exceptions: they need no
disk authority, and a database rebuilt from disk starts them empty.

SQLite is still authoritative. Nothing in this increment changes that.

## Disk records that can be authorities

| record | where | holds today |
|---|---|---|
| **task directory** | `~/.kanna/repos/<repo-id>/tasks/<task-id>/` | `task.json` (a replaceable snapshot) and the immutable `ledger/` entries: `result`, `input`, `transition`, `plan` (T0 envelope, T1 exit/budget fields, T2 `session_ref` identity, T6 `artifacts`, T8 `channel_identity`) |
| **repo directory** | `~/.kanna/repos/<repo-id>/` | `artifacts.git` (T6: artifact contents, result bindings, retention and sharing refs). Nothing else yet |
| **local config** | the repo's `.kanna/config.json` / `config.local.json`, the app's local config | repo policy, workflow and agent definitions. Machine-local preferences are not there yet |
| **protected secret store** | the OS keychain / protected credential files | pairing secrets, device tokens, account credentials. No table in this database holds a secret |

## What the offline rebuild does now

`crates/kanna-server/src/task_store/rebuild.rs` reads task directories with a
versioned reader (it refuses a `schema_version` other than 1 and any entry
whose envelope does not match its file name or directory) and projects them
into a new database created with the production migrations. It writes:

- `pipeline_item`: identity, prompt, title, workflow name and pinned definition, stage, branch, base ref, parent, PR url/number, created/updated/closed times (all from `task.json`)
- `task_blocker`: from `links.dependencies`, only onto tasks that have a directory
- `stage_run`: one row per run that recorded a result entry (the newest result wins); status, verdict, summary, metadata, the exit the session named, artifact references, feedback, declared role and verified channel of the result, and T2's workspace id, session branch, session name and transcript reference from `session_ref`
- `task_input`: every input entry with its row id, stage, source, message, delivery time, import origin and channel
- `task_stage_budget`: replayed from each routed result's `budget`, reset by a send-back (an operator exit other than `advance`)
- `task_ledger_entry`: the ledger itself as published outbox rows with the files' exact bytes, so a server started on the result continues each task's sequence
- `task_ledger_snapshot`, `task_ledger_backfill`: marked current, so nothing is owed to publish or backfill
- `repo`: a **placeholder** row per repo id (empty path, name = id), only because `pipeline_item.repo_id` has a foreign key

The fixture round trip (`http_api::tests::disk_rebuild`) runs a migrated
database through the real endpoints and writers: a parked manual gate, an
active run with delivered input, a legacy revision, a legacy custom post, a
named-exit loop until its budget is spent, a dependency-blocked task, an
artifact-referencing result and a pre-ledger task imported by T0's backfill.
It then rebuilds twice and asserts:

- the two rebuilds, and a re-application of the projection, are identical row for row
- every compared fact matches, except exactly the list below

## Facts that cannot yet be rebuilt from disk

The list is `NOT_REBUILT` in `rebuild.rs`; the round trip asserts that each
compared entry actually differs in the fixture and that nothing else does.

| fact | why | where its authority should come from |
|---|---|---|
| `repo.registration` | path, name, default branch, remote are SQL-only | repo directory / local config |
| `task.agent_type`, `task.agent_provider` | task-level agent defaults are not in `task.json` | `task.json` |
| `task.initial_pipeline` | the workflow a task was created with is not recorded; plan entries only name later changes | `task.json` or a creation plan entry |
| `task.revision_rounds` | legacy revision round counter: legacy revision entries carry no round number | result/transition entries (as T1 does for exit budgets) |
| `task.attention_requested` | attention badge is SQL-only | task directory |
| `task.pinned` | sidebar pin is SQL-only | local config (a machine preference) |
| `task.worktree` | the worktree row (path, branch, setup state) | task directory (workspace records) |
| `task.stage_workspace` | T2's stage workspace directory per stage; `session_ref` names only its id | task directory (workspace records) |
| `task.branch_counter` | T2's `task_branch_counter` | not needed on disk: T2 re-seeds it above repository refs and every branch the task's records name. Only numbers spent by attempts that left no branch, directory or record can be reissued |
| `run.exists` | a run with no result entry — running, or ended without a verdict (session exit, spawn failure, teardown) — writes nothing to the ledger | a session-start entry, and an engine-observed result entry |
| `run.session` | agent, provider, model, effort, provider session id, cwd, resume/replace links, entry trigger and channel, no-work termination, T2's `workspace_report` | a session-start entry |
| `run.completion` | `completion_transition`, `completion_bound` (engine bookkeeping) | derivable from the pinned workflow at session start; record it there |
| `run.resolved_prompt` | the prompt a session was started with (`stage_run_prompt`) | a session-start entry |
| `run.summary`, `run.feedback` | a revision request's ledger message joins summary and findings; complete-stage results rebuild exactly | carry summary and findings separately in the revision result entry |
| `run.result_declared_role`, `run.result_channel` | backfilled history records no role and an `unknown` channel by T0's rule; live results rebuild exactly | none — this is deliberate |
| `input.run_id` | an input to a run that recorded no result would dangle, and `task_input.run_id` has a foreign key; projected as NULL | resolved once `run.exists` is |
| `run.started_at`, `run.finished_at` (not compared) | required columns filled with approximations: first ledger mention, and the newest result's `recorded_at` | a session-start entry; the result entry |

Gaps other open follow-ups already found, which this list includes:

- **Engine-observed results write no ledger result entry.** A session that
  exits without a verdict, a spawn failure, a quota replacement or a teardown
  closes its run in SQL only (`no_work_termination`), so the run and why it
  ended are not on disk (`run.exists`, `run.session`). Found in T8c's review.
- **No downgrade fence for any migration.** `schema_migrations` records what
  ran; nothing refuses to open a database that a newer build migrated, so an
  older build reads tables and columns it does not know. Found in T1's review.
  The task directory has the same exposure: the reader added here refuses an
  unknown `schema_version`, the server's own publisher and delivery do not.

## Table inventory

Legend for "ledger carries": **yes** = the rebuild restores it; **part** = some
columns; **no** = nothing on disk. "Exception" rows are statistical or
transient and have no disk authority.

### Task state

| table | authority | ledger carries | missing |
|---|---|---|---|
| `pipeline_item` | task directory | part: identity, prompt, title, workflow pin, stage, branch, base ref, parent, PR url/number, times | agent defaults, initial workflow, revision rounds, attention, pin, notify target, cloud task id, merge signal, PR branch, issue number/title. Live columns are exceptions: activity*, runtime*, unread, preview, composer*, blocked/activity baselines, port offset/env, agent session id, teardown start, blocker revision |
| `task_blocker` | task directory | yes (`links.dependencies`) | — |
| `stage_run` | task directory | part: runs with a result (see above) | runs without a result; session facts; completion bookkeeping |
| `stage_run_prompt` | task directory | no | whole table |
| `task_input` | task directory | yes (input entries) | run reference when the run recorded no result |
| `queued_task_input` | task directory | no | inputs accepted but not yet delivered to a session; a restart loses the queue if the database does |
| `task_stage_budget` | task directory | yes (result `budget`, send-back transitions) | — |
| `transition_commit` (T3) | task directory | part: the settling result entry and its `committed_sha` | the binding itself (run, stage, exit, state). Rows a transfer carried (T9) are keyed `carried:<task>:<origin run>`, never the origin run id, which stays with the machine that ran it |
| `task_revision` | exception (statistics) | — | analytics log of revision requests |
| `worktree` | task directory | no | path, branch, setup pending |
| `stage_workspace` (T2) | task directory | part: id only (`session_ref.workspace_id`) | stage, path, branch |
| `task_branch_counter` (T2) | none needed | no | re-seeded by T2 (see above) |
| `task_port` | exception (transient) | — | port leases are reallocated |
| `terminal_session` | exception (transient) | — | live terminal bindings |
| `agent_terminal_attempt` | task directory (transcript/archive) | no | the final terminal frame of a run |
| `workspace_setup_run` | task directory | no | setup commands, output and exit per run |

### Lifecycle intents and retry keys

| table | authority | ledger carries | missing |
|---|---|---|---|
| `create_task_intent` | task directory | no | an accepted create not yet finished; replay keys |
| `lifecycle_operation_intent` | task directory | no | post/stage-spawn operations across the daemon boundary (prepared → committed) |
| `contextless_completion_attempt` | task directory | no | completion-attempt keys that make a retried verdict idempotent |
| `task_ledger_continuation` | task directory | no | an owed transition between enqueue and dispatch; restart reconciliation must re-derive it from the latest result and transition |
| `task_ledger_entry` | the ledger itself | yes (published rows) | unpublished rows are, by definition, not on disk yet |
| `task_ledger_snapshot`, `task_ledger_backfill` | the task directory's own bookkeeping | yes (marked current) | — |

### Review, PR and attention metadata

| table | authority | ledger carries | missing |
|---|---|---|---|
| `task_review_context` | task directory | no | the PR a review task is about, head/base shas |
| `human_review_decision` | task directory (a decision is an artifact per spec §8) | no | who decided what about which head, delivery state |
| `task_pull_request` | exception (statistics) | — | forge facts behind the Analytics view |
| `pipeline_item.attention_requested` | task directory | no | see above |

### Repository registration and settings

| table | authority | ledger carries | missing |
|---|---|---|---|
| `repo` | repo directory / local config | no (placeholder only) | path, name, default branch (+source), remote url/hash, hidden, sort order |
| `repo_sidebar_order` | local config | no | sidebar order by remote |
| `settings` | local config | no | user preferences; migrations also stamp analytics coverage start times |
| `schema_migrations` | exception (per-file) | — | describes the database file, not data |

### Ownership and transfer

| table | authority | ledger carries | missing |
|---|---|---|---|
| `task_transfer` | task directory (per task) + repo directory | no | transfer state, claims and leases (`task.json.owning_machine` is reserved and null) |
| `task_transfer_provenance` | task directory | no | which peer/task this one was imported from |
| `task_transfer_workflow_claim` | task directory | no | a finalizing transfer's claim on the workflow |
| `transfer_work`, `transfer_work_phase` | exception (transient queue) | — | in-flight transfer engine work |
| `transferred_task_context`, `transferred_task_manifest`, `transferred_task_history` | task directory | part: imported inputs keep `source.origin` | imported workflow/results/history that predates the destination's ledger (T9) |
| `transferred_task_state` (T9) | task directory | part: carried result/transition/plan entries are re-recorded with `source.origin.ledger_entry`, and the `transfer_import` transition records transfer id, source, ownership generation and fresh-start reason; the rebuild projects carried results under `carried:<task>:<origin run>` | carried links (stage workspaces, settled edges and joins), state digest, and whether the first session resumed; the source's verbatim files are kept under `transferred/<transfer-id>/` but not read by the rebuild |
| `transfer_ledger_export` (T9) | exception (transient) | — | the fence a transfer's final export sets on the source task's ledger (transfer id, exported sequence); it refuses appends only while that transfer holds the task, and means nothing once the source closes or the transfer ends |

### Subscriptions, wakes and managers

| table | authority | ledger carries | missing |
|---|---|---|---|
| `event_subscription` | task directory (the subscriber task) | no | durable mailbox position and filter |
| `copilot_wake_registration`, `copilot_wake_attempt` | task directory | no | native wake transport state |
| `claude_channel_registration`, `claude_channel_attempt` | task directory | no | native channel transport state |
| `task_serviced_watermark` | task directory (the manager's view) | no | what a manager already dealt with |
| `task_event_cursor_handle` | exception (transient) | — | named event cursors, expire |
| `task_event` | exception (event feed, pruned after 14 days) | — | announcements; the ledger is the record |

### Statistics and observations (exceptions)

`activity_log`, `task_activity_interval`, `operator_event`,
`provider_token_usage`, `provider_usage_scan`, `provider_usage_discovery`,
`task_revision`, `task_pull_request` are statistics. `task_provider_rejection`
and `task_provider_capacity_notice` are provider observations used by quota
recovery; they are durable facts about runs and belong in the task directory
once run records are (they are listed here because a lost row only costs a
recovery decision, not task state).

## Not in this increment

Making disk authoritative, removing or rewriting SQL writers, restart
reconciliation cutover, a rollback path, peer compatibility, and an entry
point for the rebuild outside tests.
