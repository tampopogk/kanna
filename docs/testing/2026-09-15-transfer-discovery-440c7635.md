# Transfer discovery correction — task 440c7635

## Attribution

The final acceptance outcome at desktop.21 source
`dc75f7a3be20030009908e41f0d9a63b0366afa4` is **FAIL, zero moves**.
Acceptance task `641dbb6f` owns its preserved Studio fixture and rerun; helper
`074f03cc` owns MBP. This implementation starts at `5c1ca1b3b`; the traced
discovery, external registry, and desktop synchronization code is unchanged
from the failed source. No installed operator app was tested or changed.

## Confirmed defects and changes

1. **Raw multicast fails while loopback discovery passes.** A six-second
   `mdns-sd 0.19.0` probe in this worktree logged `EHOSTUNREACH` sending to
   `224.0.0.251:5353` on both active Studio interfaces, en0 and en1.
   The existing same-host transfer test passed because it allowed loopback.
   The strengthened test fails against the original discovery code on its new
   non-loopback assertion. This establishes the failing raw-socket boundary;
   neither a service-name collision nor installed-app interference is proven.
   [Sanitized probe evidence](evidence/2026-09-15-transfer-discovery-440c7635.json).

   macOS transfer advertisement, browse, resolve, and address events now use
   libSystem DNS-SD, as general LAN discovery already does. No external
   executable or developer-installed library is required. One native connection
   owns the operations; shutdown wakes its blocking event wait, deallocates
   children before their parent, and clears observations. Observations retain
   interface identity and generations, prefer routable addresses, preserve IPv6
   scope, and withdraw independently. There is no new polling or retry timer.
   Other platforms retain mdns-sd.

2. **The desktop mistakes real mDNS peers for cloud proxies.** Both carry
   `pid: 0`, but `parseLanTransferPeers` and the Pair Machine filter used that
   value to exclude cloud peers. The sidecar now emits `lan_discovered` for the
   actual listed endpoint; the desktop consumes that field, with the old
   discriminator retained only for older sidecar replies. This fixes the
   desktop's LAN selection/pairing path. The server API already classified by
   proxy endpoint, so this is **not** the cause of the original API LAN refusal.

3. **Overlapping presence reads can starve cloud identity delivery.** The
   Firestore subscription previously discarded a completed relay-presence read
   whenever a newer read was pending. With task updates arriving faster than
   those reads finish, no snapshot reaches the transfer synchronizer. A
   producer-callback → mapper → synchronizer regression now reproduces a late
   same-account peer remaining unregistered despite a completed online-presence
   result, with LAN unavailable. It fails before the correction and passes after.
   Delivery now rejects only results older than an already-delivered result,
   preserving ordering without waiting for the update stream to become quiet.
   Mapping uses current documents and local options. No timer or retry was added.
   This establishes a product defect, **not** that it caused the old C incident.

4. **Forced-cloud return traffic silently selected LAN.** The corrected-source
   live run reached a real outgoing/incoming transfer, then MBP's import work
   failed repeatedly with `No route to host (os error 65)` before source
   finalization. Its selected source endpoint was the intended
   `192.168.1.165:4465`, discovered over LAN. Source preflight pinned cloud only
   in its outgoing reservation; the destination stored no route, and
   finalization, artifact fetch, import acknowledgment, and refusal notification
   selected `Auto`, which preferred LAN. The initial payload's clean-finalization
   flag was a placeholder, not proof the source agent had stopped.

   Preflight now carries the concrete route inside its authenticated payload.
   The destination persists that route and uses it for return traffic with the
   existing route-specific trust checks. Current cloud proxies are resolved at
   use, allowing normal proxy/credential refresh. Removing cloud trust fails
   closed instead of falling back to LAN. A real-runtime regression, with
   runtime-produced LAN entries and a counted forwarding cloud proxy, fails
   before this change and passes after: preflight/commit → destination restart →
   finalization → artifact bytes → import acknowledgment all use cloud.
   No intent IDs, attempt budgets, source ownership, or idempotency rules changed.

   Legacy incoming reservations and older senders have no route field and keep
   their existing automatic selection; a binary update cannot reconstruct their
   original choice. Both endpoints must run corrected code for a newly signed
   route to survive the full handoff. The retained fixture can use the supported
   `KANNA_TRANSFER_DISCOVERY=registry` configuration with its distinct existing
   per-instance roots to establish cloud with LAN unavailable, without seeding
   registries or changing the intent. That is legacy recovery evidence, separate
   from new-route persistence. The underlying LAN socket rejection has not been
   attributed to a wrong address, Bonjour collision, or a specific OS policy.

5. **A pull loses its selected route before the source sends the task.** The
   request itself used the selected transport, but its authenticated payload and
   emitted source work event omitted it. The source engine therefore chose
   `Auto` for the resulting push. The request now signs its concrete selected
   route and the listener carries it through the production sidecar event into
   existing durable push work. Older requests/events without the field retain
   their automatic behavior; explicit `Auto` on the signed wire is rejected.
   A real requester → authenticated listener → production IPC conversion →
   source-preflight regression with both routes available proves the resulting
   push uses the counted cloud proxy. It failed before the change. This defect
   is separate from the live registry-only pull reset: that run had no LAN route
   for `Auto` to choose.

6. **The cloud proxy rejects valid requests while relay setup is pending.**
   The proxy treated its 64 KiB setup-buffer bound as a request-size limit,
   closing the socket when another chunk arrived before authentication/tunnel
   setup completed. The transfer protocol allows larger commit payloads. The
   proxy now stops reading when that buffer is full, letting TCP backpressure
   retain the remaining bytes until setup completes. The memory bound, setup
   deadline, cancellation, authentication, and connection limit remain intact.
   A deterministic real-TCP/WebSocket regression sends 192 KiB before relay
   authentication, then verifies every byte and the reverse acknowledgment. It
   failed before the fix with an incomplete WebSocket handshake, and passes
   after. The live reverse pull's stored payload measured 79,523 UTF-8 bytes,
   already above the limit before encryption/base64. The later recovered owned
   MBP proxy logs directly attribute all five live resets to this limit, with
   each rejection immediately preceding its corresponding engine error.

## Original cloud peer absence — cause remains open

The retained MBP `/v1/cloud/desktops` result proves relay/LAN presence, **not**
receipt of a Firestore desktop transfer identity by its renderer. That endpoint
contains machine IDs and names; it does not expose transfer identities.
Consequently, its disagreement with `/v1/transfers/peers` does not establish
whether publication, subscription, eligibility, or registration failed.

The identity producer intentionally publishes cloud protocol **1** independently
of direct peer protocol **5**. No version change is warranted. Listener contexts
share the live external registry; they do not take a startup snapshot.

The initial checkpoint `552d55faa` passed two focused regressions without changing
cloud behavior:

- Authenticated desktop subscription callback → cloud mapping →
  `setCloudMachines` → proxy/external registration accepts a peer arriving after
  destination startup with an empty LAN catalog, and withdraws it on removal.
  Firebase and proxy operations are test doubles; this is not deployed-cloud
  acceptance.
- Separate runtime roots have no mutual LAN entries. A source arriving after
  the destination listener starts reproduces `peer not found: peer-source`
  before registration, remains untrusted, then completes cloud preflight and
  an authenticated cloud pull request after live external registration. No
  listener restart, discovery seed, durable pairing, or trust bypass is used.
  The transport is exercised over real loopback sockets, not a deployed relay.

Do not infer that these tests repair or explain the MBP incident. The cloud
failure remains an acceptance investigation until its missing producer edge is
captured. Credential provisioning and the old dev-restart listener failure are
separate setup/lifecycle findings; recovery-stage prompt correction is outside
this task.

## Focused verification

- Original discovery + strengthened mDNS regression: expected failure at the
  non-loopback assertion (`.tmp/mdns-regression-before.log`).
- Corrected mDNS regression: pass; independent roots, native publication and
  discovery, non-loopback endpoint, explicit LAN provenance, real pairing and
  transfer delivery, and withdrawal (`.tmp/mdns-native-final.log`).
- Native TXT callback and multi-observation removal: 2 passing tests.
- Transfer protocol and address conversion: 26 passing tests.
- Late cloud runtime registration and preflight/pull: pass.
- Desktop cloud index, machine merge, and synchronization lifecycle: 46 passing
  tests. Desktop `vue-tsc --noEmit`: pass.
- Overlapping presence reads added to the late-peer regression: expected failure
  before the cloud delivery correction (`.tmp/cloud-emission-before.log`); all
  46 focused desktop tests pass after (`.tmp/cloud-emission-after.log`). Existing
  out-of-order completion coverage still prevents rollback by older results.
- `cargo clippy -p kanna-task-transfer --all-targets -- -D warnings`: pass.
- Return-route correction: 6 cloud-route tests, 4 finalization tests, 3 incoming
  reservation tests, 2 destination acknowledgment/finalization tests, and replay
  compatibility tests pass. The new full return-route regression failed before
  the correction (`.tmp/cloud-return-before.log`) and passes after, including
  trust withdrawal (`.tmp/cloud-return-final.log`). Final clippy passes.
- Reverse-pull follow-up: setup-buffer regression fails before the correction
  (`.tmp/cloud-setup-buffer-before.log`); all 14 proxy tests pass after, including
  timeout, cancellation, credential refresh, connection cap, and byte-preserving
  large-request coverage (`.tmp/cloud-setup-buffer-after.log`).
- Pull route regression fails before the correction
  (`.tmp/cloud-pull-route-before.log`); 7 cloud runtime tests pass after
  (`.tmp/cloud-pull-route-after.log`). Ten pull authentication/idempotency tests
  and the pull wire roundtrip pass (`.tmp/cloud-pull-compatibility.log`). Updated
  task-transfer clippy with warnings denied passes (`.tmp/cloud-pull-clippy.log`).

This is focused implementation evidence, not physical cross-machine or release
acceptance. The same-host network test requires a usable physical interface.

## Bounded rerun handoff

Before any normal fixture startup, acceptance must reconcile the retained work
`push:5d2e372c:C-CLOUD-leg1-641` through supported orchestration. Stopping did
not cancel it. Keep task `5d2e372c`, its review workspace, committed head
`7365949699b6fb5816ade47d6027760e87e3c144`, tracked/untracked bytes, DB, and
provider history. Do not queue a duplicate intent or attribute the corrected
runtime to the old C source/build.

Pin the corrected source/tree, binary hashes, ports, and exact native title
containing `641dbb6f` on both isolated endpoints through canonical kd. Same
staging account and Codex only. Capture this early cloud checkpoint before a
move: source local `cloud_transfer_identity_v1` → authenticated desktop document
transfer fields → destination renderer `cloudSnapshot.transferMachines` →
external peer and proxy catalog. Record registration errors with peer IDs,
never tokens. Start the destination first to cover late source arrival.

Then exercise explicit LAN and forced-cloud push/pull, recording actual route,
intent and transfer IDs, source ownership, and finalization. Forced-cloud
coverage must remain valid without LAN discovery. Acceptance owns a fresh blind
recall nonce; do not place it in any prompt. Keep results distinct from old C
and Linux B. No production acceptance, publication, or soak waiver follows
from this implementation.

### Retained-intent reconciliation and corrected owner direction

The corrected code checkpoint is local commit `552d55faa58510dc556cd23f648f5e8d0f9cac6a`,
tree `d1b51a1e623c0e0f09b1ae57daa0691579f931c2`. Acceptance verified its portable
bundle with prerequisite `dc75f7a3`; SHA-256:
`ed827692d4c595c22034bbbc895ec7779f86a0d6fc03d142b72de210ba543bc4`.

Read-only source tracing found no supported offline inspect/cancel/pause control
for the retained pre-record push. `runtime.rs` unconditionally starts
`transfer_engine::run`, which recovers and drains pending work before an API
caller can intervene. The retry budget in `db/transfer_work.rs` is eight, so
five observed failures do not establish exhaustion. The public rejection and
cleanup operations require a transfer ID, which this failed push never created.

The initial interpretation that reconciliation required a new offline queue
control was incorrect. Root explicitly authorized **controlled continuation of
this same disposable acceptance intent**: start and verify the corrected
destination first, then start the corrected source and observe its remaining
automatic attempts. Keep the existing intent ID and attempt trace; issue no
duplicate push. This fulfills the reconciliation requirement without changing
queue semantics or requiring another feature/task.

Task `641dbb6f` coordinates `074f03cc` and owns that bounded rerun. The scope-choice
attention was cleared. The earlier failure record and gate assertion describe
the superseded interpretation, not a remaining authorization requirement. The
original cloud cause remains open until the four-boundary checkpoint and live
results establish it. No DB was opened or edited, and no installed operator
app was tested. All local test/probe processes have exited; no stage advance or
publication was performed.

## Corrected-source live checkpoint (552d55faa)

Acceptance restarted the verified MBP destination first, then Studio at
19:45:38Z with the retained intent and identities. Attempt 6 failed at
19:46:10.668Z because the **destination** peer was not yet in Studio's local
catalog. This is distinct from old C's missing **source** peer at MBP.

At 19:47:10.953Z MBP saw Studio through native LAN discovery, while its renderer
cloud catalog still omitted Studio. By 19:47:46.843Z normal late delivery had
added Studio to the renderer and external catalog: trusted, LAN and cloud
available, cloud route ready. No reconnect, manual registration, or duplicate
push caused that transition. This is positive physical cross-machine discovery
and late cloud registration evidence at `552d55faa`.

Authenticated server reads on Studio (19:49:08.020Z) and MBP
(19:49:25.467Z) matched: `kanna-staging`, database `(default)`,
`firestore.googleapis.com`, TLS, neither cache nor pending local writes. Both
documents contained the expected public identities, protocol 1, and accepting
state. Both renderer accounts had the same UID digest. Relay presence was
recorded separately.

Retained push attempt 7 created transfer
`4bd2c7722756f387fc8cbc07ad57ac81bd04f4a69e476ff48eae35f0d537e2a5`
at 19:56:15Z. MBP's exact owned server log attributed import attempts 1–5 to
`No route to host (os error 65)` at 19:56:16.964894Z, 19:56:17.984148Z,
19:56:23.003971Z, 19:56:53.022995Z, and 19:58:53.658622Z. The source remained
open/live, with input ledger count 5 and no finalization input as of 20:03:19Z;
destination remained claimed with no local task. The one approval returned 200,
but its scheduled boolean was not retained and is unknown. A later empty log
search does not negate the retained attributed errors. No completed move is
claimed. These observations led to the return-route correction above.

The later starvation correction is `af5cc056926f5566f3eb9c2173699d27c6651e2c`,
tree `741f3890cc719610a102e17f4dc19f0479c5fbf7`. It has focused regression and
typecheck evidence, but is not running in this live checkpoint. Native code is
unchanged. Neither the transient absence in this run nor the deterministic
starvation test establishes the cause of old C's cloud failure.

Acceptance owns the timestamped endpoint/binary evidence in its worktree's
`docs/testing/evidence/2026-09-15-corrected-transfer-641dbb6f.json`.

## Cloud-only legacy recovery checkpoint (36b60d29f)

Both isolated endpoints were rebuilt at
`36b60d29fd924e2bff6f25f0e744c02a2ceb092e`, tree
`dd694e542370d6c32b6d171fce1ff2e8d64ecba2`. Acceptance verified the bundle
SHA-256 `c2329ed8b7dbf931281cc6ca86cf6282eecc6586ad43cfe248437b2854f46734`.
The source session was quit through its normal input path and its owned old
allocation stopped before rebuilding; the retained task, transfer, session
history, and existing registry roots were preserved. No reject, fail, abandon,
cleanup override, duplicate push, or registry seed was used.

Canonical startup used `KANNA_TRANSFER_DISCOVERY=registry` on the separate
existing roots. MBP destination was verified first at 20:21:14Z; Studio started
at 20:21:46Z and its native identity was verified at 20:22:40Z. Studio's MBP
catalog entry was cloud ready with LAN unavailable. This run continues the
same legacy reservation, whose original record contains no selected route;
it tests cloud-only recovery rather than the new authenticated route field.

The MBP peer catalog omitted Studio at 20:24:57Z. At 20:26:26Z, authenticated
uncached documents and the MBP renderer contained both expected public transfer
identities, protocol 1, on the same staging account. At 20:27:19.409Z its sidecar
catalog also contained Studio: trusted/transferable, cloud ready, LAN false,
cloud fallback false. These are readback bounds, not measured delivery latency.
No reconnect or manual registration caused the convergence. Renderer warnings
were not captured; their absence in the server log proves nothing about them.
At 20:29:07Z the read-only invoke history confirmed ordinary proxy creation and
external registration for Studio. No earlier rejected peer was established.

Retained import attempt 8 failed at 20:28:57.172559Z with `missing target peer
for outgoing transfer finalization`; destination had no local task. This error
comes from the source's missing **outgoing transfer reservation**, after the
cloud-only request reached it. It is not a missing discovery peer. The existing
reservation lifetime is 900 seconds (`runtime/config.rs`), and source startup
prunes expired outgoing reservations (`runtime/replay_store.rs`). The reservation
created around 19:56:15Z was over 25 minutes old at the 20:21:46Z source restart.
The diagnostic/rebuild pauses outlived the fixture's reservation. No expiry,
intent, or queue semantics were changed to make the test pass.

At 20:32:35Z acceptance read back both terminal records: incoming failed at
20:28:57Z with no local task, and outgoing failed at 20:28:59Z with the same
missing-target-finalization error. The destination's supported cleanup-candidates
API returned an empty list after natural cleanup; no manual mutation was used.
The source task remained open in review in `task-5d2e372c-2`. Its input ledger
was still 6, unchanged since the operator-directed cleanup `/quit` at 20:14:57Z,
and its latest run remained cancelled at that time. There was no new source
finalization input. Helper-local cleanup/task-absence verification remained
pending at this checkpoint.

This failed legacy attempt does not establish a completed move or validate the
new signed route field. Acceptance is preparing a fresh explicit-cloud operation
on the same corrected source, with fresh provider resume and identity proof;
the terminal legacy ID is not being retried. That fresh operation is the
remaining live test.

## Fresh forced-cloud checkpoint (36b60d29f)

After both legacy records were terminal and natural cleanup returned an empty
list, acceptance scheduled exactly one fresh explicit-cloud push at 20:35:17Z,
intent `C36-CLOUD-fresh1-641`. Transfer
`4cee32ed929f8c7cca81d6a01093a055d2debd8ed75d4064734b3aff8724af42`
was created at 20:35:21Z. Both endpoints remained on `36b60d29f`, using their
unchanged distinct registry roots with registry discovery, cloud ready and LAN
unavailable. The source resumed its same native Codex session with Luna low,
review stage, existing branch, and head `7365949`; ledger count was 7. Private
recall material is omitted. The helper observed automatic import without another
approval or restart.

This fresh reservation exercises the new authenticated selected-route field.
The live setup can establish cloud-only completion; it cannot compare competing
LAN and cloud routes. The focused runtime regression supplies that comparison.
Paired API readback confirmed both transfer records completed at 20:35:46Z.
Destination task
`d0a5045cfdcbc37a95a36b9daec1fa8d6ce493221198e0b67525f62b84ec21e4`
was open in review with Codex Luna and all 7 input-ledger entries; incoming
cleanup candidates were empty. The source run finished at 20:35:32Z and its task
closed at 20:35:46Z. Normal close removed its worktree and preserved its branch
at WIP commit `768ce4f4`, parent `7365949`. Acceptance verified hashes of the
original tracked and untracked bytes on that retained branch.

This is the first completed live move, with cloud-only forward and return
traffic, mDNS disabled, and a fresh signed-route reservation. The helper's
destination native-session/head/history checks and one blind recall remained
pending at this checkpoint. LAN and pull coverage also remained outstanding;
this result is neither a full acceptance matrix nor a production/soak claim.

Post-import native inspection found Codex resuming the original session at a
fixture trust prompt, followed by its “Choose working directory to resume this
session” menu. The default was “Use session directory”; acceptance authorized
the normal trust response and “Current” selection for the exact imported
worktree before the single blind recall. This is a manual resume UX requirement,
not evidence of a model turn: the API's busy state alone did not establish one.
The source provider was absent and only the destination provider was active;
both transfer records remained completed. Recall remained pending. No product
change was requested for this menu.

At 20:43:02Z the single blind recall passed: the destination's 12-character
response hash exactly matched the private fresh source nonce, whose value was
never supplied in model input. Native inspection confirmed the same Codex
session resumed in the exact imported worktree after the normal trust and
current-directory menu selections. The workflow definition was byte-equal,
and hashes of all 7 source input messages matched the destination ledger prefix.
The original successful build history retained its origin; cancelled resume
history was omitted by the existing export filter. The source provider PID was
gone and its task closed, with only the destination provider active.

Acceptance is preparing a Studio-initiated reverse forced-cloud pull of the
same fixture after MBP dirty-state preparation. No source or code changes were
made for that next leg. Pull and LAN results remain outstanding.

## Reverse cloud pull checkpoint (36b60d29f)

Studio sent one explicit-cloud pull at 20:44:23Z for the imported MBP task.
A local response-save `NameError` lost the response body; this was an evidence
capture fault, and acceptance reconciled state without repeating the request.
MBP outgoing transfer
`949973fa2701b189aa6b451edf1d9ab8103f6fdb91e8f656689a1adde414e333`
was created at 20:44:26Z, source peer 37088 to target peer 31533. Its summary
omitted machine IDs. The existing pull event does not carry `targetDesktopId`,
which `push.rs` uses to populate those summary fields; omission alone does not
establish incorrect routing.

Owned MBP server logs attributed attempts 1–5 of the pull-triggered **push** work
to `Connection reset by peer (os error 54)`, at 20:44:26.941456Z,
20:44:27.983251Z, 20:44:33.023935Z, 20:45:03.045830Z, and 20:47:03.105995Z.
The outgoing record remained pending, Studio had no incoming record, and the
source remained open. Both catalogs were trusted/cloud-ready with LAN unavailable
and future credential expiry at 20:46:42Z. No duplicate action, approval, or
reconnect was used.

The source inserts its outgoing record after staging and immediately before
payload commit. This narrows the unresolved boundary to commit/admission rather
than source finalization. Studio retained several early task-transfer tunnel
dials/closes but none for the later attempts in the inspected window; neither
endpoint retained a proxy/frame-size warning identifying the reset cause.
This failure remains separate from the previous import return-route failure.

An authenticated API-only read at 20:49:56Z measured the pending outgoing
payload JSON at 79,523 UTF-8 bytes; no payload contents were emitted. The first
completed push's final stored payload measured 41,562 bytes, but that final body
is not proof of its earlier commit wire size. The proxy's setup-buffer defect
above is deterministically reproduced and corrected. The retained reverse
reservation was created around
20:44:26Z and reaches the existing 900-second lifetime around 20:59:26Z. A later
expiry must be attributed separately from any proxy or discovery result. Its
normal next automatic attempt was due around 20:57:03Z; no queue/TTL changes or
manual retries were introduced.

At 20:54:25.256666Z the helper recovered the owned MBP proxy warnings from the
same server log. Each says `local transfer request exceeded the pre-setup buffer
limit`, at 20:44:26.940729Z, 20:44:27.981880Z, 20:44:33.022485Z,
20:45:03.044961Z, and 20:47:03.104536Z. Each immediately precedes the matching
engine reset above. This confirms the setup-buffer limit as the live reverse
pull failure cause. The earlier empty warning search was an evidence gap,
superseded by these retained records. The separate pull-route propagation defect
remains regression-established, not the cause of these cloud-only resets.
The corrected test candidate remains `bd2c8bd2c`; this attribution update changes
documentation only.

Studio destination was ready on `bd2c8bd2c`, tree
`0098dc14772796ad87614aa7e2fc652bda3b72f8`, at 20:58:32Z, with exact native
task641/C identity, the same account digest and peer/desktop IDs, and registry
discovery. Acceptance retained its arm64 server and sidecar binary hashes.
MBP source preparation was delayed by the helper searching for an older bundle
message format. The already-delivered durable input was reconciled without a
resend: 10,317 decoded bytes and SHA-256
`0fb071a21d952fdf5503d97431d9cdbfd1c41971f03062748db08f39824582b6`
matched the verified bundle. This is a helper setup fault, not a product defect
or human approval hold. Source cleanup/build remained incomplete as the
20:59:26Z reservation deadline passed. No new transfer or duplicate intent was
created. A subsequent missing-reservation result must remain TTL-attributed and
cannot validate or invalidate the corrected proxy's byte delivery.

Both endpoints were identity-verified on `bd2c8bd2c` by 21:04Z, and paired API
readback at 21:04:34Z showed trusted/cloud-ready catalogs with LAN and fallback
unavailable. The source's old server log was reconciled programmatically:
20:57:03.298126Z was attempt 6 of the retained pull's push work, failing with
reset 54, not the already-terminal legacy import. That attempt ran before the
source upgrade. Its initial misattribution was withdrawn.

The corrected MBP server log then recorded attempt 7 at 21:07:03.373333Z:
`protocol error: missing target peer for transfer commit` for transfer
`949973fa2701b189aa6b451edf1d9ab8103f6fdb91e8f656689a1adde414e333`.
The outgoing reservation had expired before corrected source startup. This
confirms the anticipated TTL boundary and does not test large-payload delivery
through the corrected proxy. The authoritative parent API read at 21:07:44Z
still showed source pending and destination absent; a helper's empty parsed
object was not a transfer status. Acceptance is preserving automatic settlement,
with final attempt 8 expected around 21:17:03Z, before any fresh request. No
manual retry, queue control, or product semantics change was introduced.

The retained pull settled at 21:17:03Z: source outgoing failed with `push gave
up: missing target peer for transfer commit`, and Studio had no incoming record.
At 21:17:20Z parent API readback showed no active outgoing transfer, empty cleanup
candidate lists on both endpoints, and no pending Studio incoming transfer. The
MBP source task remained open in review, with ledger count 9 and its last quit
at 20:57:34Z. Acceptance is resuming that same source session and private recall
fixture under `bd2c8bd2c` before a fresh request. The old TTL failure is retained
separately; settlement used no manual retry or queue/code change.
