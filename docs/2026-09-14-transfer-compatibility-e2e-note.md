# Transfer compatibility and rejection reconciliation

Task: `dd007aef`. Code/evidence correction only; installed same-candidate LAN
and cloud push/pull acceptance remains owned by `641dbb6f`.

## Main audit and incident evidence

Fetched `origin/main` before changes: `ac2604a6e092771968f7f9096d4c4b917ad036b1`,
also this worktree's initial HEAD. Read the full `641dbb6f` task and both durable
inputs through Kanna. Its summary is authoritative for the installed incident;
this task did not operate those installed apps or duplicate its report/ledger.

Main already contained:

- `7375b4ed3`: V2 finalization discriminators and selection commitments. Preserve
  these; supporting the old finalize shape would discard a security contract.
- `95e08745d`, `bab2d2fd8`: durable source work and a held retry proof.
- Explicit admission refusal versus unresolved submission in
  `transfer_engine/control.rs` and `push.rs`; terminal imports already stop in
  `import.rs`. These are reused, not replaced by a new retry engine.

Missing on that main: preflight/pull capability negotiation, and notification of
a destination's terminal rejection to its source. Reject deleted the incoming
reservation while the source could retain an active outgoing row. Subsequent
push work re-used that active row. The incident establishes failure, not an
exhausted eight-attempt budget or a permanent hang.

## Data flow traced

Desktop `stores/transfer.ts` expresses intents through `desktopServerClient.ts`.
`http_api/transfers.rs` resolves peers/cloud credentials and queues push/reject,
or invokes pull. `transfer_engine` owns durable `task_transfer`, `transfer_work`
and import/finalize state. `transfer_control.rs` maps operations onto the
server-owned sidecar's stdio protocol. Sidecar `runtime/transfers.rs`, `pull.rs`,
`listener.rs` and `replay_store.rs` own authenticated wire exchanges and durable
reservations. LAN uses their TCP listener; `cloud_transfer_proxy.rs`,
`task_transfer_tunnel.rs` and relay tunnel routing bridge those same bytes.
The relay is not a transfer capability or ownership authority.

A successful import still passes selection validation, source workflow claim,
daemon finalization, repository/history admission and committed-content proof
before source retirement. None of those checks or V2 discriminators changed.
DB snapshots and state-change publication carry terminal status to Sidebar and
remote/mobile consumers; no new UI or renderer-owned transfer state was added.

## Correction

- An authenticated, request-bound peer capability exchange queries each live
  server. The initiating server also inspects its actual sidecar's response.
  Missing support produces an actionable upgrade error before creating a
  reservation, staging artifacts or requesting source shutdown. Both components
  on both machines must implement the contract; even an earlier V2-capable build
  without negotiation is conservatively refused. No product version parsing.
  Old binaries receive the explicit refusal; this patch cannot alter an old
  binary's own retry policy, and does not claim to do so.
- Preflight/pull receivers reject old callers that omit the authenticated
  contract. A positively unsupported operation is incompatibility. Connection
  failures/timeouts continue to use existing transient retry behavior.
- Pre-import rejection/failure notifies the source through authenticated RPC.
  The source acknowledges only after its server commits the identity-checked
  failure and retires associated work. Source task/payload remain intact.
- Push work is bound to the transfer it acquired. The first refusal also retires
  matching pre-upgrade intents that have no binding. Replayed acknowledgments
  leave later explicit intents alone. A final binding check fences a canceled
  worker before it submits a newly staged payload. Terminal rows, delayed incoming
  events and late failure callbacks cannot turn that work into another move.
  Pull deduplication clears when the refusal settles, permitting a fresh intent.
- Cleanup retries retain the destination reservation until acknowledgment.
  Exhaustion leaves the cleanup obligation outstanding, rather than claiming
  success or deleting the only reservation. This retains the existing finite
  retry budget; it is not a promise of automatic convergence during an
  indefinitely disconnected or incompatible-peer interval.
- Rejection cannot overwrite an already imported/completed transfer. The legacy
  direct reject route also queues reconciliation. Existing uncommitted abandon
  authorization and committed-reservation protection remain in force.

## Verification

All new protocol/lifecycle checks use disposable contexts. No real coding-agent
session was moved or driven. New socket fixtures use assigned ephemeral ports.
Standalone runtime fixtures explicitly model a server consumer; shipped
`RuntimeConfig::from_env` has no fixture bypass and queries live HTTP.

Passing checks (commands and compact results also in
[`the evidence file`](testing/evidence/2026-09-14-transfer-compatibility-dd007aef.json)):

- Server transfer lane: **279 passed**, including real sidecar restart/import
  receipt coverage and the new composed rejection test. See exclusions below.
- Runtime transfer selection: **23 passed**.
- Protocol serialization: **23 passed**; sidecar control: **4 passed**.
- Six new server tests: identity-bound durable refusal and restart, HTTP
  refusal, completed/imported rejection guards, delayed-event rejection, old local sidecar, and composed real
  sidecars → source HTTP server → SQLite → destination cleanup.
- Four new runtime tests: legacy peer push/pull; new sidecar backed by old live
  server in both directions; legacy initiators; lost acknowledgment, idempotent
  cleanup, old-ID refusal and fresh pull/preflight IDs.
- The complete runtime run passed 142/144 initially. The pull-refusal regression
  was fixed and rechecked; the other test's pre-existing 500 ms request window
  could never cover a five-second unresolved admission. Its fixture now consumes
  and acknowledges the incoming event before asserting committed-abandon
  protection; that targeted test passes. No production timeout was shortened.

The first broad server run passed 273 and failed 11. Two failures are untouched
main assertions: `incoming_transfer_state_machine_is_durable_and_provenance_is_idempotent`
tries to create an already-created provenance table before reaching changed
behavior; `a_stale_cloud_credential_refuses_the_route_instead_of_scheduling_it`
expects an older credential diagnostic. Seven unchanged daemon finalization
checks lacked their separately built daemon binary at the expected test path.
These nine were explicitly excluded from the final focused lane, not called
passes. The remaining two failures (pending backoff fixture semantics and the
cloud pull stub's missing capability) were corrected and pass in the final lane.

Strict `cargo clippy -p kanna-task-transfer --all-targets -- -D warnings` passes.
Clippy across both crates/all targets completed with existing warnings in
`ksp.rs`, `db/pipeline_items.rs`, and `http_api/desktop_views.rs`; no warning was
reported in changed code. This is not a claim that repository-wide `-D warnings`
passes. Changed Rust files were formatted; `git diff --check` passes.

## Remaining system acceptance

This proves code-level refusal/reconciliation, not installed release acceptance.
The composed test uses real sidecars and a real source HTTP/SQLite server, with
disposable seeded transfer rows and a fixture destination server. It does not
prove successful transfer of a real task, conversation/PTY continuity, genuine
LAN routing or the deployed relay under the final release candidate.

`641dbb6f` must still test both push/pull initiation directions on compatible
final-candidate endpoints over separately demonstrated LAN and cloud routes,
including success, cancellation and exactly one owner. No installed upgrade,
publication, network manipulation, root-terminal input, or additional reviewer
was performed here. The final candidate's full soak requirement is unchanged.
