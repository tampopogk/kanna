# Cross-machine transfer corrections — task 440c7635

## Result and attribution

Implemented corrections for native macOS discovery, LAN provenance, cloud
snapshot delivery, selected-route propagation, relay setup buffering, and fresh
Codex session export. Focused regressions reproduce the defects and verify the
corrections. Acceptance task `641dbb6f` owns the preserved fixture and live
readbacks; helper `074f03cc` owns MBP. Neither operator installation was tested.

The original desktop.21 run at `dc75f7a3be20030009908e41f0d9a63b0366afa4`
failed with **zero moves**. Corrected runs completed four cloud moves. They do
not retroactively pass original C or Linux B. **LAN transfer remains failed,
and the fresh-session export correction still needs its bounded live retest.**
No production acceptance, soak waiver, publication, PR, or stage advance is
claimed. Source intent, retry budgets, idempotency, and ownership rules remain.

Code checkpoints:

| Checkpoint | Change | Live attribution |
| --- | --- | --- |
| `552d55faa` | Native DNS-SD and explicit LAN provenance | Cross-host discovery and ordinary late cloud registration observed; import selected unreachable LAN despite forced cloud |
| `af5cc0569` | Prevent overlapping presence reads starving cloud snapshots | Deterministic producer-through-consumer regression; not proved as old C cause |
| `36b60d29f` | Persist authenticated concrete return route | First cloud-only move and blind recall passed; reverse pull exposed setup-buffer rejection |
| `bd2c8bd2c` | Cloud proxy backpressure and signed pull-route propagation | Cloud-only large-payload pull and cloud with LAN present completed; fresh-run context failed while known-session context passed |
| Fresh-session follow-up in this change | Discover fresh Codex rollout using existing recovery resolver; refuse unidentified empty export | Local regression passes; same-fixture live retest pending |

The `bd2c8bd2c` tree is `0098dc14772796ad87614aa7e2fc652bda3b72f8`.
Acceptance preserves endpoint-native titles, process paths, binary hashes,
account digests, route evidence, and exact transfer IDs in its worktree report
`docs/testing/2026-09-15-studio-mbp-C-transfer-acceptance.md` and evidence files
`2026-09-15-studio-mbp-C-641dbb6f.json` and
`2026-09-15-corrected-transfer-641dbb6f.json`. Their final sections supersede
historical checkpoints. Private nonce values and raw session bodies are omitted.

## Confirmed defects and corrections

### 1. macOS raw multicast and misleading loopback coverage

A bounded mdns-sd 0.19 probe logged EHOSTUNREACH on both active Studio interfaces,
en0 and en1, while loopback succeeded. Strengthening the original discovery test
to require a non-loopback endpoint failed before the correction. Neither a
Bonjour collision nor installed-app interference was established.

macOS transfer discovery now uses libSystem DNS-SD, as general LAN discovery
already did. One owned native connection registers, browses, resolves and tracks
addresses; shutdown wakes its blocking wait and disposes children before the
parent. Interface/generation tracking handles withdrawal and address preference,
including IPv6 scope. No external dependency or polling/retry loop was added.
Other platforms keep mdns-sd. The independent-root native discovery, pairing,
delivery, and withdrawal regression passes. Physical cross-host discovery also
passed on corrected source. [Sanitized initial probe](evidence/2026-09-15-transfer-discovery-440c7635.json).

### 2. Real LAN peers discarded as cloud proxies

Both mDNS peers and cloud proxies carry pid 0. Desktop parsing and pairing used
that value to discard cloud entries and consequently discarded real mDNS peers.
The sidecar now emits explicit `lan_discovered`; desktop consumers use it and
retain the old discriminator only for older replies. The server API already
classified peers by proxy endpoint, so this was not the original API LAN error.

### 3. Cloud identity delivery starvation

The Firestore subscription discarded a completed relay-presence read whenever
a newer read was pending. Continuous updates could prevent any snapshot from
reaching external-peer registration. Delivery now rejects only results older
than an already-delivered result and maps current documents/options. No timer
or retry was added. A real subscription-callback → mapper → synchronizer
regression proves late same-account registration with an empty LAN catalog and
preserves out-of-order delivery protection.

Old C's missing cloud producer edge is still unproven. `/v1/cloud/desktops`
reports relay/LAN presence, not receipt of Firestore transfer identity. Cloud
identity protocol 1 is intentionally distinct from direct peer protocol 5.
The listener shares the live external registry, not a startup snapshot.
Corrected live runs converged without reconnect or manual registration, both
with native discovery and with separate registry-only roots. Readback times
bound convergence; they do not measure exact delivery latency. Server-log
silence does not establish absence of renderer warnings.

### 4. Forced-cloud return traffic selected LAN

Source preflight pinned cloud only in its outgoing reservation. The receiver
stored no route and selected Auto for finalization, artifact fetch, acknowledgment
and refusal notification. At 552, MBP import attempts 1–5 failed NoRoute65 against
the correct Studio LAN endpoint `192.168.1.165:4465` before source finalization.
The initial payload's clean-finalization flag was a placeholder, not source-stop
proof. The one approval returned 200; its scheduled boolean was not retained.

Preflight now signs the concrete selected transport. The receiver persists it,
resolves the current proxy at use, and enforces route-specific trust for every
return step. Cloud trust withdrawal fails closed. A counted-proxy runtime test
covers real preflight/commit, receiver restart, finalization, artifact bytes and
acknowledgment while LAN is also available. Legacy reservations without the
field keep Auto; an upgrade cannot reconstruct their historical route choice.

### 5. Pull request lost its route before source work

The pull request used its selected transport but omitted it from the signed
payload and source work event. The resulting push therefore selected Auto.
The concrete transport now travels through the authenticated request, listener,
production IPC conversion and existing durable push work. Older absent fields
retain legacy behavior. The producer-through-consumer test verifies a counted
cloud proxy is used despite an available LAN route. This defect did not cause
the registry-only reverse pull resets, where Auto had no LAN alternative.

### 6. Relay setup buffer incorrectly acted as a request-size limit

The proxy closed a local socket after reading more than 64 KiB while asynchronous
relay setup was pending. Corrected code stops reading at the existing bound and
lets TCP backpressure hold the remainder until setup finishes. Memory, setup
deadline, authentication, cancellation and connection limits remain bounded.

The live reverse pull `949973fa…e333` had a 79,523-byte stored JSON payload before
encryption/base64. Owned MBP proxy logs explicitly reported `local transfer
request exceeded the pre-setup buffer limit` immediately before reset54 on
attempts 1–5 (20:44:26, 20:44:27, 20:44:33, 20:45:03 and 20:47:03 UTC).
The earlier empty warning search was incomplete and superseded. The deterministic
192 KiB pre-authentication test failed before and passes after, preserving all
bytes and the reverse acknowledgment. The later fresh bd2 pull passed this
large-payload boundary in the live cloud-only setup.

### 7. Fresh Codex session silently exported without context

Fresh bd2 pull `994d942e…496cd` completed, but its one value-free blind recall
returned UNAVAILABLE. Both final payloads carried Codex/PTY,
`resume_session_id=null`, `artifacts=[]`, and clean finalization. The destination
launched the original prompt without resume. This locates the loss at source
export, before materialization. The source had previously fallen back to a fresh
run because the earlier transcript was unavailable; the fresh nonce belonged to
that new run, so the earlier recovery failure does not explain away its loss.

`SourceSession::resolve` intentionally keeps provider and ID from the same latest
run instead of borrowing a stale task-row ID. Codex only records its new ID from
the exit footer. The transfer planner treated absent ID as nothing to export,
although local recovery already discovers a rollout by its metadata's worktree.
Finalization and the terminal watcher consume exit events independently; the
retained live evidence does not distinguish missing ID capture from late DB
persistence. No DB was read directly to fill that gap.

The correction reuses the existing cwd-based Codex recovery resolver when a PTY
transfer has no recorded ID. It stages the discovered rollout before promising
resume, and the existing finalization downgrade guard preserves that discovery
across shutdown. If no rollout can be identified for the source worktree, the
transfer fails before stopping the source rather than shipping an empty
conversation. Known recorded IDs and other providers retain their behavior.
The regression starts with fresh rollout metadata and no recorded ID, then
stages, validates and materializes the artifact on a separate destination home,
verifying its bytes and resume eligibility. A wrong-worktree test refuses export.
No live model fixture was added by this task.

## Live results and setup distinctions

| Live leg | Result | Limits |
| --- | --- | --- |
| Original C, dc75/desktop.21 | Zero moves, missing transfer discovery/cloud source peer | Historical cloud cause remains unproven |
| 552 retained push `4bd2c772…e2a5` | Discovery positive; import failed NoRoute65 | Return-route defect confirmed; no completed move |
| 36 legacy continuation of `4bd…` | Failed missing finalization reservation | Diagnostic/rebuild pause exceeded existing 900-second TTL; both records terminal, natural cleanup empty, source retained |
| 36 fresh Studio cloud push `4cee32ed…af42` | Completed 20:35:46; known-session blind recall passed | Registry-only, no competing LAN route |
| 36 reverse Studio cloud pull `949973fa…e333` | Proxy setup-limit resets | Helper bundle-parser delay crossed TTL before bd2 startup; attempts 7–8 failed expired commit reservation; terminal 21:17:03, source retained, cleanup empty |
| bd2 fresh Studio cloud pull `994d942e…496cd` | Completed 21:26:42; large-payload transport passed | Fresh-session export/context FAILED; correction above awaits live retest |
| bd2 MBP cloud pull `d3b6726d…a58b84`, LAN present | Completed 21:39:13; native known-session blind recall passed | Strongest new-nonce context evidence with competing routes |
| bd2 MBP cloud push `59023c1d…8e351d`, LAN present | Completed 21:45:40; same session resumed | Return recall passed but Studio already held that session/nonce, so it cannot prove later MBP-only turns |
| bd2 MBP explicit LAN pull | One prequeue HTTP502, NoRoute65 at 21:36:25.769683 | No move; no retry; LAN remains unaccepted |

Completed known-session hops preserved workflow definitions, source input-ledger
prefixes (7, then 13 and 14 entries), verified file hashes/head, sole destination
provider ownership, source closure and empty cleanup candidates. Source close
preserved WIP branches before removing worktrees. Normal Codex trust and
current-directory resume menus required manual selection; API busy was not
proof of a model turn. The failed old pull's transferring icon converged to the
expected failure marker by 21:19:30; no persistent UI defect was established.

The response-save NameError and helper's old bundle-message parser were test
setup faults. No duplicate request, queue edit, registry seed, intent reset, or
artificial TTL extension was used. An earlier invented cancellation/approval
gate was explicitly superseded by root's authorization to continue the retained
disposable intent; that gate is not a remaining requirement.

Both acceptance allocations are now stopped with owned listeners/processes
cleared. The final Studio fixture `c4f27c…3b3a5` is retained for the already
authorized bounded fresh-run retest; fixtures were not closed as cleanup.
No further bd2 move or new model task is planned. Acceptance owns final binary
and cleanup evidence; this fix task started no live allocations.

## LAN diagnosis and precise next check

Native DNS-SD found Studio `192.168.1.165:4465` and MBP `192.168.1.207:4502`,
with protocol 5, pid 0 and explicit LAN provenance. CLI TCP connected both ways.
The sidecar uses direct Tokio `TcpStream::connect` against that endpoint, with
no custom interface bind. Native direct connections nevertheless returned
NoRoute65; cloud through loopback proxies worked.

Owned MBP evidence pins sidecar 77802 → server 77412 → desktop 77030 on macOS
26.2 (25C56). Desktop binary SHA-256 was
`71f228e3349aed78a33dd257753c344b3fa203660feb8bdc1cc25aff8c2e6081`,
code-sign identifier `kanna_desktop-f542b7356c14e0fb`, TeamIdentifier unset.
Its embedded Info.plist contains the usage description and Bonjour declarations
but no CFBundleIdentifier. No signing-flag inference is made from the partial
readback. Tauri's dev codegen embeds the supplied plist and name/version without
inserting the configured bundle ID. The bounded OS-log slice found no explicit
local-network denial. These facts establish an attribution limitation, not a
proved permission denial or a proved socket implementation defect.

[Apple TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)
explains that helper access follows the responsible app, Terminal/SSH tools are
exempt, and reliable identity tracking requires Apple-issued signing. Therefore
CLI TCP success is not a native-app permission check. **Next:** acceptance must
identify the exact isolated app's responsible code and Local Network permission,
preferably with an attributable signed isolated development bundle, then make
one LAN protocol request from its owned sidecar. Capture a policy/path diagnostic
if the permitted app still fails. Do not toggle a generic installed Kanna entry,
reset system privacy, bypass trust, or rewrite sockets based only on errno65.
The current evidence cannot conclusively classify this as an environment fault.

## Focused verification

- Native non-loopback regression: red before, green after; independent roots,
  pairing/delivery/withdrawal. Two native callback/observation tests pass.
- Protocol/address conversion: 26 tests pass. Desktop cloud producer, machine
  merge and lifecycle: 46 tests plus vue-tsc pass. Starvation regression is
  red/green; unchanged frontend evidence reused afterward.
- Return-route correction: cloud, finalization, reservation/replay and
  acknowledgment tests pass; counted route and trust-withdrawal regression is
  red/green. Task-transfer clippy with warnings denied passes.
- bd2: all 14 proxy tests, 7 cloud runtime tests, 10 pull authentication/idempotency
  tests and pull wire roundtrip pass. Setup-buffer and pull-route tests are both
  red/green. Logs: `.tmp/cloud-setup-buffer-{before,after}.log`,
  `.tmp/cloud-pull-route-{before,after}.log`, `.tmp/cloud-pull-compatibility.log`.
- Fresh-session follow-up: regression red before (`.tmp/fresh-codex-before.log`),
  then 17 session tests, 23 source-transfer tests and 2 existing provider-recovery
  tests pass. Logs: `.tmp/fresh-codex-after.log`,
  `.tmp/fresh-codex-source-regression.log`, `.tmp/fresh-codex-recovery-regression.log`.
- Changed-file rustfmt and git diff checks pass. Server clippy is blocked by five
  warnings in unchanged files: unused function in `ksp.rs`, argument count in
  `db/pipeline_items.rs`, and three boolean simplifications in
  `http_api/desktop_views.rs` (`.tmp/fresh-codex-clippy.log`). They were not changed.

No broad build/visual matrix or production gate was substituted for these
focused checks. Live fresh-session proof and LAN acceptance remain outstanding.
