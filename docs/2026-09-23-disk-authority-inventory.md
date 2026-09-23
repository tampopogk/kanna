# Disk authority inventory (spec §11, §16.11, T13)

Date: 2026-09-23. Schema: migrations through `103_disk_state_records`.

This is the input to the T13 cutover. It lists every durable table in
`crates/kanna-server/src/db` and says, for each, which disk record is its
authority. Tables that hold statistics or transient live state are
exceptions: they need no disk authority, and a database rebuilt from disk
starts them empty.

SQLite is still authoritative. The first increment (T13a) wrote this
inventory and an offline rebuild; the second (T13b) put every other
durable fact of a task on disk, so the rebuild now restores all of it.
Neither changes who is authoritative: no writer was retired and nothing
reads the disk records at runtime except the rebuild.

The code is the list: `crate::db::task_state::CARRIED_TABLES` (with each
column carried or left out for a reason) and `NOT_CARRIED_TABLES`. A test
(`every_table_and_column_is_classified`) fails when a table or column
appears in neither, so a later schema change has to decide where its
authority is.

## Disk records

| record | where | holds |
|---|---|---|
| **task directory** | `~/.kanna/repos/<repo-id>/tasks/<task-id>/` | `task.json` (a replaceable snapshot, now with `state`: the task's rows of every carried table) and the immutable `ledger/` entries: `result` (including engine-observed endings), `input`, `transition`, `plan` |
| **repo directory** | `~/.kanna/repos/<repo-id>/` | `repo.json` (registration and sidebar order), `artifacts.git` (T6) |
| **local config** | the repo's `.kanna/config.json` / `config.local.json`, the app's local config | repo policy, workflow and agent definitions |
| **protected secret store** | the OS keychain / protected credential files | pairing secrets, device tokens, account credentials. No carried column holds a secret; an incoming transfer's claim token is left out |

`task.json`'s `state` is `{ "version": 1, "tables": { <table>: [ <row>, ... ] } }`.
Each row is the table's carried columns verbatim (integers and reals as
numbers, text as strings, NULL as null) plus its SQLite `rowid`, because
several readers order runs, intents and transfers by it. Tables with no
rows are omitted.

`state.reflects_through` is the highest ledger sequence whose effects the
rows already hold: the highest committed entry, published or still pending,
read in the same SQLite transaction as the rows (every entry commits with
the mutation it records). `state.unreflected_reservations` lists sequences
below it that were only reserved then, whose effects are not in the rows
yet. `ledger.published_through` stays the publication watermark only; the
rows can be ahead of it.

The boundary is sound because a ledger sequence is never handed out twice:
`task_ledger_sequence.high_water` records every sequence ever allocated to
the task, in the allocating transaction, and allocation is always above it
(as T2's branch counter is). A released or abandoned reservation is a
permanent gap; readers and the publisher already treat a gap as nothing.
The mark is carried in `state`, and a rebuild raises it to the highest
sequence the directory records, reflects or lists as reserved, so entries
a rebuilt database records are never mistaken for ones an older
`task.json` already reflects.

`repo.json` is `{ schema_version, repo_id, registration: {<repo columns>},
sidebar_order, snapshot_revision }`.

A task or repository deleted from the database (a creation rolled back, a
repository unregistered) has its `task.json`/`repo.json` replaced by a
tombstone `{ schema_version, task_id|repo_id, removed: true }`. Ledger files
are never deleted. The rebuild skips tombstones.

### Same transaction

Once migration `103_disk_state_records` has run, triggers sit on every
carried table (and `repo`, `repo_sidebar_order`, and `pipeline_item`
deletes). Each one
bumps T0's `task_ledger_snapshot.revision` (or the new
`repo_disk_snapshot.revision`, or records a `disk_record_removal`) in the
statement that changes a carried row, so no writer, present or future, can
change carried state without owing a new record. The publisher (T0's, every
five seconds and at startup) rewrites the record from current rows, in one
read snapshot. An update that sets only left-out or quiet columns
(`pipeline_item.updated_at`, `repo.last_opened_at`, the task row's live
columns, a transfer's lease) owes nothing.

Every connection parses the triggers when it opens, so they are one short
statement each (no column-by-column comparison: a write that sets a carried
column to its current value owes an identical record). SQLite checks
triggers when a table is altered or rebuilt, so each migration that runs
drops them first, and they are re-installed to the build's definition after
the last migration (`task_state::sync_disk_state_triggers`; a no-op when
they already match).

### Backfill

The same migration owes every open task a new `task.json` and every
repository a `repo.json`; the publisher writes them. The migration is one
transaction, and publication is the outbox T0 already has, so an
interrupted backfill resumes by publishing what is still owed, and running
it again only rewrites identical records
(`an_interrupted_backfill_resumes_to_the_same_records`). Closed tasks keep
the `task.json` they had.

### Engine-observed endings

A run that closes without a verdict (session interrupted, spawn failure,
quota replacement, rejected resume, lifecycle failure, cancellation) now
writes a `result` entry in the closing transaction, sourced
`stage_run_ending` / `<run>#ending-<n>`, with `status: null`,
`observed_by: "engine"`, `ending {run_status, no_work_termination}`, no
declared role and the server's own channel. It is not a verdict: no session
is told it caused anything, it is never a transition's triggering result,
it resolves no join member, and a dependency edge ignores it (edges read
only `success`). Teardown runs record none. After a transfer has exported
the task's final ledger none is recorded (the task is leaving).

A revision request's result entry now also carries `request.summary` and
`request.findings` beside the joined message.

## What the offline rebuild does

`crates/kanna-server/src/task_store/rebuild.rs` reads `repo.json` files and
task directories with a versioned reader (it refuses an unknown
`schema_version` or `state` version, a table or column it does not carry,
and any entry whose envelope does not match its file name or directory) and
projects them into a new database created with the production migrations:

- `repo`, `repo_sidebar_order`: from `repo.json`; a placeholder only for a
  repository with task directories and no `repo.json` (reported)
- every carried table: from `state`, row for row, keeping rowids, in
  foreign-key order. Rows naming a task or run that is not rebuilt are
  reported and dropped, never invented
- `task_blocker`: `links.dependencies`
- `task_input`: the ledger's input entries
- `task_ledger_entry`: the ledger files as published rows, so a server
  started on the result continues each sequence; `task_ledger_snapshot`,
  `task_ledger_backfill`, `repo_disk_snapshot` marked current

Two rules on top of copying:

- **The ledger wins over a stale `task.json`.** A crash between publishing
  an entry and rewriting `task.json` leaves `state` behind the ledger. The
  entries after `state.reflects_through` (and any reservation it lists as
  unreflected) are applied on top of its rows, and only they can pay an
  owed transition; a `state` without the field falls back to
  `ledger.published_through`:
  a verdict or engine-observed ending closes its run (a run the rows do not
  hold yet is projected from the ledger), a routed result spends its
  budget, a send-back resets it.
- **Owed work is restored, never re-done.** Nothing is executed. An owed
  transition (`task_ledger_continuation`) from a stale `task.json` is
  dropped only when a newer entry paid or replaced it: a transition (the
  dispatch it owed, or any move that fences it stale) or a verdict under
  another operation (a corrected result). After unrelated newer entries
  (inputs, endings, plans) it is restored, with a diagnostic either way.
  Lifecycle intents, commit steps, dependency waits and
  join members are restored as they were; restart reconciliation, which is
  built to be idempotent, resumes them.
- **The branch counter is never below a recorded suffix.** A restored
  counter is raised to the highest `task-<id>-<n>` any row or ledger entry
  of the task names.

A `task.json` without `state` (written before this increment, or by an
older peer) is projected from the ledger as T13a did.

The fixture round trip (`http_api::tests::disk_rebuild`) drives a migrated
database through the real endpoints and writers to: a parked manual gate
with a review context, decision and retry key; an active session that was
lost and recovered, with its workspace, branch number, session identity,
prompt, setup record and provider observations; a legacy revision; a
legacy custom post; a named-exit loop with its budget spent; a
dependency-blocked task; a stage edge into a first stage before it started
and one into a later stage with the completion parked on it; a subtask join
whose uncreated member holds an owed transition; a pending commit step with
its delivery in flight; a claimed incoming transfer with its provenance and
imported state; an outgoing transfer holding a task's workflow with the
ledger fenced; artifact references; a pre-ledger task imported by T0's
backfill; and a task whose creation was rolled back after its `task.json`
was written. It asserts that every carried table is exercised; that no
secret reached disk; that two rebuilds, and a re-application, are identical;
that **every durable row rebuilds exactly**; that the rebuilt database owes
nothing, a flush writes nothing, and the owed transition is still held by
its join; and that the removed task is not resurrected.

## What a rebuilt database lacks

`NOT_REBUILT` in `rebuild.rs`; none of it is compared, and everything else
is.

| fact | kind | why |
|---|---|---|
| statistics | statistics | `activity_log`, `task_activity_interval`, `operator_event`, `provider_token_usage`, `provider_usage_scan`, `provider_usage_discovery`, `task_revision`, `task_pull_request` |
| task live state | transient | the task row's live columns: activity, runtime status, read state, output preview, unsent drafts, port lease, live session mirror, event debounce baselines, the teardown-in-progress marker (its teardown run is carried) |
| session transport | transient | `terminal_session`, `task_port`, `copilot_wake_*`, `claude_channel_*` |
| transfer work queue | transient | `transfer_work`, `transfer_work_phase`; restart recovery re-derives it from `task_transfer` |
| event feed | transient | `task_event`, `task_event_cursor_handle`; the ledger is the record |
| subscription positions | cannot rebuild | `event_subscription`, `task_serviced_watermark` hold `task_event` sequence numbers, and a rebuilt database restarts that feed empty: a restored position would skip or replay events, so subscribers subscribe again |
| run terminal capture | cannot rebuild | `agent_terminal_attempt` is a capture of session output, not task state; the run keeps its transcript reference |
| transfer claim token | cannot rebuild | a capability that never leaves the database, with a 30-second lease expiry that means nothing after a restart; restart recovery re-claims a claimed transfer under a new token |
| machine pairing and preferences | cannot rebuild | `trusted_peer`, `settings` belong to the machine (its pairing store, its local config), not a task |
| publication window | cannot rebuild | a change committed in SQL but not yet written when the database is lost (the publisher writes within seconds and at every startup) |
| history never captured | cannot rebuild | backfilled history's branch, commit, triggering result and channel; numbers a branch-counter reservation spent without leaving a branch, directory or record |

## Table inventory

Legend: **state** = carried in `task.json` `state`; **ledger** = the ledger
entries; **repo.json**; **exception** = statistics or transient; **—** =
not rebuilt, with the reason above.

### Task state

| table | disk record | notes |
|---|---|---|
| `pipeline_item` | state (+ `task.json` top-level fields) | live columns left out (see "task live state") |
| `task_blocker` | `task.json` `links.dependencies` | |
| `stage_run` | state; ledger results | every run, with or without a result; session facts; completion bookkeeping |
| `stage_run_prompt` | state | |
| `workspace_setup_run` | state | |
| `task_input` | ledger inputs | |
| `task_stage_budget` | state; ledger results' `budget` | |
| `transition_commit` (T3) | state | rows a transfer carried (T9) keep their `carried:<task>:<origin run>` keys |
| `worktree` | state | |
| `stage_workspace` (T2) | state | |
| `task_branch_counter` (T2) | state | raised to the highest recorded suffix on rebuild |
| `contextless_completion_attempt` | state | completion retry keys |
| `task_provider_rejection`, `task_provider_capacity_notice` | state | provider observations about runs, used by quota recovery |
| `task_revision` | exception (statistics) | |
| `task_port`, `terminal_session` | exception (transient) | |
| `agent_terminal_attempt` | — | run terminal capture |

### Lifecycle intents and owed work

| table | disk record | notes |
|---|---|---|
| `create_task_intent` | state | |
| `lifecycle_operation_intent` | state | restored as is; restart reconciliation resumes it |
| `task_ledger_sequence` | state | the per-task sequence high-water mark; raised on rebuild to every recorded or reflected sequence |
| `task_ledger_continuation` | state | dropped only when a ledger entry newer than `task.json` paid or replaced it |
| `task_stage_edge`, `task_dependency_wait` (T4) | state (the dependent task's) | an edge whose upstream task is not rebuilt is reported and dropped |
| `task_join`, `task_join_member` (T5) | state (the parent's) | |
| `task_ledger_entry` | the ledger itself | unpublished rows are, by definition, not on disk yet |
| `task_ledger_snapshot`, `task_ledger_backfill` | bookkeeping | marked current on rebuild |

### Review, PR and attention metadata

| table | disk record | notes |
|---|---|---|
| `task_review_context`, `human_review_decision` | state | |
| PR url/number/branch, issue number/title, merge signal, attention, pin, agent defaults, initial workflow, revision rounds, notify target, cloud task id | state (`pipeline_item`) | |
| `task_pull_request` | exception (statistics) | |

### Repository registration and settings

| table | disk record | notes |
|---|---|---|
| `repo` | `repo.json` | `last_opened_at` is carried but does not by itself owe a rewrite |
| `repo_sidebar_order` | `repo.json` (order of the repository's remote) | |
| `repo_disk_snapshot`, `disk_record_removal` | bookkeeping (this database's outbox for `repo.json` and tombstones) | |
| `settings` | — | machine preferences (local config) |
| `schema_migrations`, `sqlite_sequence` | — | describe the database file |

### Ownership and transfer

| table | disk record | notes |
|---|---|---|
| `task_transfer` (rows with a local task) | state | `claim_owner_token` and `claim_expires_at` left out; failed transfers with no local task are machine-level alerts and are not carried |
| `task_transfer_provenance`, `task_transfer_workflow_claim` | state | |
| `transferred_task_context`, `transferred_task_manifest`, `transferred_task_history`, `transferred_task_state` (T9) | state | the source's verbatim files stay under `transferred/<transfer-id>/` and are not read by the rebuild |
| `transfer_ledger_export` (T9) | state | the fence refuses appends only while its transfer holds the task |
| `transfer_work`, `transfer_work_phase` | exception (transient) | |
| `trusted_peer` | — | this machine's paired peers and their public keys: pairing state whose authority is the machine's pairing store, not any task |

### Subscriptions, wakes and managers

| table | disk record | notes |
|---|---|---|
| `event_subscription`, `task_serviced_watermark` | — | positions in the transient event feed |
| `copilot_wake_*`, `claude_channel_*` | exception (transient) | a live session's transport |
| `task_event_cursor_handle`, `task_event` | exception (transient) | |

### Legacy

| table | notes |
|---|---|
| `agent_run` | from the base schema; no reader or writer since `stage_run` replaced it. Left empty by a rebuild |

`queued_task_input` is no longer listed: migration `067_remove_input_hold_state`
dropped it.

## Still open

- **No downgrade fence for any migration.** `schema_migrations` records
  what ran; nothing refuses to open a database that a newer build migrated.
  The task directory has the same exposure: the rebuild's reader refuses an
  unknown `schema_version` or `state` version, the server's own publisher
  and delivery do not. Found in T1's review.
- Making disk authoritative, retiring SQL writers, restart reconciliation
  from disk, a rollback path, peer compatibility, and an entry point for the
  rebuild outside tests.
