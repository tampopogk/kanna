# Disk authority inventory (spec §11, §16.11, T13)

Date: 2026-09-23. Schema: migrations through `104_disk_divergence`.

This is the input to the T13 cutover. It lists every durable table in
`crates/kanna-server/src/db` and says, for each, which disk record is its
authority. Tables that hold statistics or transient live state are
exceptions: they need no disk authority, and a database rebuilt from disk
starts them empty.

The first increment (T13a) wrote this inventory and an offline rebuild;
the second (T13b) put every other durable fact of a task on disk, so the
rebuild restores all of it. The third (T13c) makes the authority a
persisted, per-installation mode: `sql` (the default, as before) or `disk`,
switched at a checkpointed quiescent boundary and rolled back the same
way ("Storage authority" below). The fourth (T13d) retires the SQL-first
write path in `disk` mode: each mutation writes its task directory records
first and SQLite commits after them ("Disk-first writes"); it closes the
rollback window to older builds, verifies every rollback, and migrates
legacy commit posts ("Legacy retirement"). SQLite stays the default
authority; `disk` stays an explicit opt-in.

The code is the list: `crate::db::task_state::CARRIED_TABLES` (with each
column carried or left out for a reason) and `NOT_CARRIED_TABLES`. A test
(`every_table_and_column_is_classified`) fails when a table or column
appears in neither, so a later schema change has to decide where its
authority is.

## Disk records

| record | where | holds |
|---|---|---|
| **task directory** | `~/.kanna/repos/<repo-id>/tasks/<task-id>/` | `task.json` (a replaceable snapshot, now with `state`: the task's rows of every carried table) and the immutable `ledger/` entries: `result` (including engine-observed endings), `input`, `transition`, `plan` |
| **repo directory** | `~/.kanna/repos/<repo-id>/` | `repo.json` (registration, sidebar order, and since T13c the `installation` that wrote it), `artifacts.git` (T6) |
| **authority record** | `~/.kanna/authority/<installation>.json` | the installation's storage authority mode, a requested switch, a switch in progress, and its recent checkpoints (T13c); schema version 2 and `disk_first_since` while the installation is `disk` (T13d) |
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
| publication window | cannot rebuild | `sql` mode only: a change committed in SQL but not yet written when the database is lost (the publisher writes within seconds and at every startup). In `disk` mode there is none: the disk is written before SQLite commits |
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
| `disk_divergence` | bookkeeping (this database's fence on a task whose disk is ahead of it, T13c) | a database rebuilt from disk is not behind it |
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

## Storage authority (T13c)

`crates/kanna-server/src/task_store/authority.rs`. Every installation has
a mode:

- **`sql`** (default): SQLite is authoritative; the task directories are
  its published projection. On disagreement the database wins and the
  publisher rewrites the disk. This is every installation until an
  operator switches it.
- **`disk`**: the task directories are authoritative; SQLite is a
  projection of them. On disagreement the disk wins.

**Installation.** `installation` is derived from the database path (the
first 16 hex digits of its SHA-256). Production and staging both publish
under `~/.kanna`, so each writes its own authority record there, and every
`repo.json` names the installation that wrote it. A rebuild or
reconciliation takes in only the repositories stamped with its own
installation and the task directories under them; the others are reported
as foreign and never read. A `repo.json` an older build rewrote without the
stamp is owed again at the next `disk`-mode startup.

**What `disk` mode does.**

- At startup, after migrations and before any service can schedule,
  dispatch or accept a mutation, every task directory is compared with the
  database. A task whose directory holds what the database never had (a
  ledger entry it lacks or holds with other bytes, a `task.json` at a
  revision the database never reached with other rows, or no row at all)
  has its rows replaced by the projection of its directory, in one
  transaction: rows the directory does not hold are removed, rows it holds
  are written, ledger rows become its files, published, and committed
  entries the directory contradicts are dropped and reported. The branch
  counter and the ledger sequence high-water mark are never lowered.
  Nothing is executed: owed transitions, lifecycle intents, commit steps,
  edge waits, join members and transfer claims come back as rows, and the
  restart reconciliation that follows resumes them as after a crash (a
  claimed incoming transfer is re-claimed under a new token; its token
  never reaches disk). A task the disk tombstones is removed. A record the
  disk lost (a ledger file or `task.json` the database published) is owed
  and written again.
- A missing database is rebuilt from the installation's records before the
  server opens it (a WAL or shared-memory file left beside it is removed
  first). What a rebuilt database lacks is the table below.
- At runtime, when the publisher finds a ledger file already holding other
  bytes, or a `task.json` at a revision beyond the database's, it writes
  nothing over it: the task is flagged and reconciled from its directory
  under the task's mutation lease (deferred while an operation holds a
  ledger reservation on it). The flag is a row of `disk_divergence`
  (migration `104`), so it survives a restart; until the repair deletes it
  the task publishes nothing, and the repair takes its differing
  `task.json` as the disk's whatever the revisions say. A repair aborts if
  the task's rows changed after they were compared, never lowers a counter,
  keeps a transfer (and its workflow claim) that still owns the task by
  T9's rule, never writing a disk value over it; while the database holds
  an effective claim on the task, no ownership column
  (`TRANSFER_OWNERSHIP_COLUMNS`: the claim, the transfer's association and
  status, the ledger-export fence, an import's ownership generation) is
  taken from disk, and never rewrites another task's input: an input whose id a
  restored older database handed out again moves to a new id, and the join
  member naming it follows.

**Switching to `disk`.** Ask for it; the next server start performs it:

```sh
kanna-server storage-authority disk      # or KANNA_STORAGE_AUTHORITY=disk
kanna-server storage-authority status    # mode, request, switch, last checkpoint
```

The subcommand reads the server's config (`KANNA_SERVER_CONFIG`, else the
app's `server.toml`) and writes only the authority record. At startup the
switch records a checkpoint after each step:

| checkpoint | after | mode |
|---|---|---|
| `to_disk.begin` | the switch is recorded | sql |
| `to_disk.drained` | stale reservations released, pre-ledger history imported, a record owed for every task never published (closed tasks that predate the disk records) and every repository (restamped), the outbox drained with nothing left owed | sql |
| `to_disk.repo_verified` | one per repository: each of its tasks has a current `task.json` whose `state` and dependencies equal its rows, ledger files equal its published rows byte for byte, and `repo.json` equals the registration | sql |
| `to_disk.verified` | every repository verified, no task directory or tombstone the database does not account for | sql |
| `to_disk.commit` | the mode is `disk` | disk |

A kill at any point resumes at the next start by re-running the steps, all
idempotent. Any difference refuses the switch (`to_disk.refused`, with the
problems in the checkpoint and the log): the installation stays `sql`, the
request stays, and the next start tries again once the cause is gone.

**Rolling back to `sql`.** `kanna-server storage-authority sql` (or
`KANNA_STORAGE_AUTHORITY=sql`), then restart:

| checkpoint | after | mode |
|---|---|---|
| `to_sql.begin` | the rollback is recorded | as before |
| `to_sql.reconciled` | from `disk` mode only: the database reconciled from disk and the outbox drained, so it holds everything the disk does | disk |
| `to_sql.commit` | the mode is `sql` | sql |

From a switch that never committed, nothing changed who was authoritative,
so the rollback only records itself. From `disk` mode, see "The rollback
window" below: since T13d the rollback is verified, and it is the only way
back to a build older than T13d.

`KANNA_STORAGE_AUTHORITY` is applied at every start of a server that has
it in its environment; while it is set, a subcommand request the other way
is overridden at the next start. The packaged desktop never passes an
inherited value on: it removes `KANNA_STORAGE_AUTHORITY` from the
environment of the `kanna-server` it launches (T13d), so a stray variable
in a login shell or launch agent cannot switch a production installation.
For a desktop-launched server the subcommand is the switch.

**Peers and clients.** Nothing on the wire changes. Transfer, federation,
the mobile and desktop APIs, and a peer that knows nothing of disk mode
behave exactly as against a `sql` installation.

**Exceptions.** Secrets stay in their protected stores (no carried column,
authority record or `repo.json` holds one). Statistics and transient
live-session state stay SQL-only, and a rebuilt database starts them empty.

**Cost.** A `disk`-mode startup reads every task directory of the
installation (as a rebuild does) and compares it with the rows. A switch
publishes a `task.json` for every task never published, closed ones
included, once.

## Disk-first writes (T13d)

`crates/kanna-server/src/task_store/disk_first.rs`, with the connection's
commit gate in `crates/kanna-server/src/db/disk_first.rs`.

In `disk` mode every transaction that owes a disk record (a trigger or an
enqueue touched `task_ledger_snapshot`, `task_ledger_entry`,
`repo_disk_snapshot` or `disk_record_removal`) writes it from its own
uncommitted rows, holding SQLite's write lock, before SQLite commits. For
each task it touched:

1. **Refuse** when the disk holds what this database did not write: the task
   is fenced (`disk_divergence`, or a failed commit in this process), a
   ledger file at one of its unpublished sequences holds other bytes, or
   `task.json` is at a revision the database never published. Nothing is
   written, the transaction rolls back with `disk-first write refused: ...`,
   and the task is reconciled from its directory (under its mutation lease,
   as in T13c). Until then every write to the task is refused.
2. **Write `task.json`**, the commit point. It holds the rows as the
   transaction leaves them and, under `ledger.in_flight`, the exact bytes of
   every committed entry that is not yet a file, and `ledger.readable_through`,
   the task's ordering watermark (below). One rename makes the whole
   mutation durable on disk.
3. **Publish the entry files** in sequence order, whatever reservation
   another operation holds below them: an open reservation never holds back
   another commit's durability. A failure here does not undo the mutation
   (it is in `task.json`); the publisher writes the file later.
4. **SQLite commits.** If that fails, the disk already holds the mutation:
   the task is fenced and reconciled from disk, so the mutation stands even
   though its caller saw an error.

**Durability and ordering are separate.** Files are durable as their
commits make them. What consumers read in order is the watermark
`ledger.readable_through` (`disk` mode only), under one invariant: every
sequence at or below it is an entry whose file is already synced, or a gap
(a released or abandoned reservation). It stops below the first open
reservation and below any entry whose file is not written yet.

- Writers: the commit point (step 2) records the watermark as it stood;
  the entry files are written and synced (file and directory); only then is
  `task.json` rewritten with the watermark over them, before SQLite commits.
  A file that fails to write keeps the watermark below it until a later
  flush writes it. Filling or releasing a reservation (including startup's
  release of reservations a dead process left) moves it on the same way.
- Readers (session delivery's triggering result) read the watermark once,
  then only files at or below it: entry files are immutable, so that view
  is consistent however commits interleave.
- A stored watermark is never trusted: every `disk`-mode start compares it
  with the one the ledger gives (after a rebuild no reservation survives,
  so the ledger's gaps are closed) and rewrites `task.json` when they
  differ, behind or ahead. The rebuild and the reconciler read
every durable file; a reconciliation publishes in-flight files only for the
tasks it reconciles. (`sql` mode is unchanged: its publisher still writes
files in order and stops at a reservation.)

Owed `repo.json` records and tombstones are written the same way. A
repository's tombstone names the installation that removed it; a `disk`-mode
start applies such a tombstone the database does not reflect (the removal's
commit died after its tombstone) by removing the repository and its tasks,
which is what a rebuild from the same disk holds. A tombstone without the
stamp (written before T13d) is still only reported. A crash
before step 2 leaves no trace of the mutation; a crash after it leaves it on
disk, and the next start publishes the in-flight entry files and reconciles
the database from the directory
(`a_disk_first_commit_killed_at_every_write_step_rebuilds_identically`: the
mutation is there whole or not at all, and the restarted database equals a
rebuild from the directories alone).

Every process that opens the database takes the installation's persisted
mode before its first connection can commit: the server at startup, and a
process that holds only the database path (the `worktree-cleanup`
subcommand) by finding the installation's authority record under the
override root, `~/.kanna`, or the database's own root. A `disk` installation
is gated in every process; an unreadable record is taken as `disk`.

Every write path reaches a commit through the gate: `with_immediate_transaction`
and the connection's own transactions publish before `COMMIT`, an
autocommit statement runs inside such a transaction in `disk` mode, and any
other commit that touches the outbox in `disk` mode (a raw `COMMIT`, a
prepared statement in autocommit) is refused by SQLite's commit hook. In
`sql` mode nothing about a commit changes. The fixture round trip runs every
representative endpoint with disk authority from the start and asserts no
commit was refused and nothing is left for the publisher.

Cost: in `disk` mode each mutation that owes a record rewrites that task's
`task.json` (all of its rows) and its new entry files, synced, while holding
SQLite's write lock.

## The rollback window

- **Through this build: open, verified.** `kanna-server storage-authority
  sql` and a restart reconciles the database from disk, drains it, and runs
  the same exact comparison that gates `to_disk.verified`. Only if the
  database holds exactly what the disk does is `to_sql.reconciled` recorded
  and the mode set to `sql`. Otherwise the rollback is refused
  (`to_sql.refused`, with the differences in the checkpoint, the log and
  `storage-authority status`), the installation stays `disk`, the request
  stays, and the next start tries again once the cause is fixed.
- **By starting an older build: closed from this build's first `disk`-mode
  start.** Before any disk-first write, the authority record is rewritten as
  schema version 2 with `disk_first_since` and a note. The persisted mode is
  in force before that write is attempted, and if it cannot be written the
  server refuses to start (`storage authority is disk but the disk-first
  fence could not be written ...`) rather than serve the installation. A
  switch or rollback whose checkpoint fails to save leaves the mode the
  record holds in force. A T13c build accepts
  only version 1 and refuses to start on the installation (its error names
  the record and its version), so no older build runs SQL-first over
  records it may not hold (such as `ledger.in_flight` entries). A completed
  rollback rewrites version 1, which reopens older builds. Builds older than
  T13c do not read the record at all: never start one on a `disk`
  installation.

Operator sequence to leave disk authority, or to downgrade:

```sh
kanna-server storage-authority sql       # with a T13d or newer build
# restart that build; check:
kanna-server storage-authority status    # mode: sql, no "disk-first writes since"
# only now start an older build, if that is the goal
```

## Legacy retirement (T13d)

- **Commit posts become commit steps.** At startup, after recovery, each
  open task whose pinned workflow commits through a post named `commit` is
  migrated, under its mutation lease, to that stage's `exit_commit` (T3's
  commit step, now valid under legacy routing too). It happens only at a
  quiescent boundary (no running run, no owed transition, lifecycle
  operation or commit step, not parked at a post, no transfer holding its
  workflow), through the ordinary replacement validator (recorded runs keep
  their stages, no run is superseded), fenced on the exact definition read,
  and recorded as a workflow replacement with `source`/`declared_role`
  `engine` on the server's channel. A task whose commit post already has
  recorded runs keeps the post; any other task not quiescent is left as it
  was until a later start.
- **Kept as adapters:** every other post (`approve`, custom posts) and the
  request-revision API for tasks not on named-exit routing.
- **Result prompt variables** (`$PREV_RESULT`, `$PREV_MAIN_RESULT`,
  `$PLAN_RESULT`) are deprecated, never removed: still substituted,
  documented as deprecated in the workflows guide, and noted once per
  repository file by the loader (a log line) and per use by `kanna_doctor`
  (a warning, never an error). The legacy bundled workflows (`no-review`,
  `single-reviewer`, `plan-build-review`) and the plan agent still use them.

## Still open

- **No downgrade fence for any migration.** `schema_migrations` records
  what ran; nothing refuses to open a database that a newer build migrated.
  The authority record now fences builds before T13d off a `disk`
  installation, but not off a `sql` one.
- Input-only ledger reads.
- The switch is not performed while the server runs; it waits for the next
  start.
- Making `disk` the default authority is a later rollout, after `disk` has
  run on staging (owner, 2026-09-23).
