# Published Linux B: additional local system acceptance

Product: `linux-v0.2.0-staging.2`, source
`a9df2f1d46fb08bcf53200b738e0fdc3c127b636`, tree
`3415e82f8a273b6ff94494d367139038eb026a62`. No product rebuild or
publication was performed. Controllers are separate task-owned TypeScript scripts;
their hashes and measured identities are in
[evidence](evidence/2026-09-15-linux-bootstrap/local-system/results.json).

## Measured on Ubuntu 26.04.1 ARM64

- **Public apt consumer PASS:** isolated apt sources/lists/cache verified the
  published signing key, fetched signed staging metadata, selected and downloaded
  `0.2.0~staging.2-1`. Actual deb SHA256
  `ab5dec3dc5b4d6902449dad490a8d38bd3a1fa6d759682b7963c7dc4ea077c3d`,
  43,811,392 bytes. No global apt source configuration changed.
- **Install PASS:** existing canonical `installedWorker.ts` package helper ran
  real apt as UID0 through the documented UTM guest agent. Initial package absent;
  installed version verified. Actual free space 2,427,277,312 bytes exceeded the
  conservative 600,000,000-byte allowance.
- **Native live Codex worker restart PASS:** real jeremy UID1000 systemd manager,
  installed B worker, fresh private DB and synthetic repo. Task `802e5786` kept
  agent PID154377/start12690501 and its task/run/workspace/committed and dirty
  contents across supervisor154143→155936 and daemon154152→155945. Codex recalled
  its conversation-only nonce after restart; the new input occurred once in the
  durable ledger. Immediate HTTP-ready task runtime was null, retained honestly;
  subsequent input became busy and completed. `/quit` exited and owned cgroup
  cleanup passed. This is worker restart, not GUI desktop restart.
- **Local revision assertions PASS, whole controller FAILED:** task `22352d80`
  advanced to review and revised to in-progress in fresh workspaces, preserving
  old tracked/untracked changes and carrying only committed boundaries. Revision
  round1, new run, Codex/Luna selection verified. Final `/quit` was recorded once
  at22:02:24Z but did not settle within60s; the controller failed and performed
  verified owned-unit/cgroup cleanup. Do not describe this run as a passing gate.
  Terminal history matching can observe an earlier response during asynchronous
  startup; the exact cause of this final input outcome is not established.

The first revision controller attempt (`d213ccbe`) failed because it observed the
old task stage immediately after advance, while latestRun already named review.
Its evidence is retained. The changed attempt waited for authoritative new
stage/run/workspace and allowed whitespace in the fixture acknowledgment; it
was not an unchanged rerun. A future shutdown check must fence acknowledgment to
that run's actual ready provider, rather than reuse task-wide terminal history.
No product code was changed based on these controller observations.

## Reused evidence and limits

Exact prepared A/B Ubuntu24.04 installed/upgrade **17/17 each architecture** from
run34959684410 remains valid and was not rerun. Acceptance641's prior installed B
GUI/live Codex build and review fork evidence is distinct and reused. This new
ARM26.04 run is not an additional x86 or Ubuntu24.04 claim.

Full `system/both` acceptance remains **missing**. Exact-B second endpoint is not
currently live/authenticated; no B cross-machine transfer was attempted. Smallest
next system action: allocate a reachable isolated exact-B counterpart, establish
normal staging sign-in on both endpoints, then measure routes, both-direction
push/pull, ownership and live context. This prerequisite is independent of whether
newer desktop C fixes pass. GUI desktop restart and the remaining viewer/mobile
surfaces are not claimed here. Elapsed soak cannot fill these gaps.

## Cleanup and retained evidence

Restored original package-absent state using canonical package removal, no
`autoremove`. Fresh scan: no `/usr/lib/kanna-staging` processes, no owned test
units; root user manager/runtime inactive, jeremy user manager active. VM lane
explicitly released to641. Other tasks' fixtures/projects unchanged.

Raw private controller evidence and fixtures remain under
`/home/jeremy/.tmp/kanna-d3ce8dec-system-1`; private state/credentials were not
copied into the repository. Local scripts remain `.tmp/linux-B-system` with
hashes in the committed evidence. No promotion, release, build, notification,
new cloud configuration or soak override occurred. Ship task remains open.

### Bounded shutdown diagnosis and corrected result (Batch377)

Retained review/revised terminal snapshots still contained the **initial** nonce
while new Codex banners showed `loading`. The readiness expression searched the
whole task transcript, so `/quit` was delivered before the revised provider was
ready. This establishes a controller error, not evidence of provider shutdown
failure. Corrected readiness requires a newly emitted response nonce after each
new authoritative stage/run/workspace identity; old transcript responses cannot
satisfy it.

Only the failed local lifecycle check was repeated, using the same verified B
package. Task `65a78fb9` produced distinct native responses in all three runs,
passed the forward/revision commit/dirty preservation assertions, and processed
exactly one `/quit`: sent22:07:57.332Z, exited observed22:07:57.627Z. Controller
exit0; cleanup22:07:57.885Z. Prior failed attempts remain retained. A preliminary
launch before guest apt completion failed ENOENT without creating a task; this
was reconciled against actual apt exit0/version/executable before launch.

Final package absent, no installed Kanna processes or owned units; root manager
and runtime-dir inactive, jeremy manager active. VM released to641 again.
Install/restart/floor passes were reused, not rerun. This resolves the local
revision/shutdown controller failure only; full system/both still lacks exact-B
cross-machine acceptance. Structured correction evidence is appended to the
existing evidence file above; no product changes or publication occurred.
