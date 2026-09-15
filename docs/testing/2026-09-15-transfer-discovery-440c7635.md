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

## Cloud investigation — cause remains open

The retained MBP `/v1/cloud/desktops` result proves relay/LAN presence, **not**
receipt of a Firestore desktop transfer identity by its renderer. That endpoint
contains machine IDs and names; it does not expose transfer identities.
Consequently, its disagreement with `/v1/transfers/peers` does not establish
whether publication, subscription, eligibility, or registration failed.

The identity producer intentionally publishes cloud protocol **1** independently
of direct peer protocol **5**. No version change is warranted. Listener contexts
share the live external registry; they do not take a startup snapshot.

Two focused regressions currently pass without changing cloud behavior:

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
- `cargo clippy -p kanna-task-transfer --all-targets -- -D warnings`: pass.

This is focused implementation evidence, not physical cross-machine or release
acceptance. The same-host network test requires a usable physical interface.

## Bounded rerun handoff

Before any normal fixture startup, acceptance must reconcile the retained work
`push:5d2e372c:C-CLOUD-leg1-641` through supported orchestration. Stopping did
not cancel it. Keep task `5d2e372c`, its review workspace, committed head
`7365949699b6fb5816ade47d6027760e87e3c144`, tracked/untracked bytes, DB, and
provider history. Do not queue a duplicate intent or reuse old C identities.

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
