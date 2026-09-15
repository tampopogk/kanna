# Exact Linux A/B floor acceptance

**PASS on both Ubuntu 24.04 architectures: 17/17 tests each.**
[Run34959684410](https://github.com/tampopogk/kanna/actions/runs/34959684410)
completed successfully at controller
`2b5382265c583bf07aafade5e64b0b752d30ee72`. Only the isolated fetch and two
prepared-upgrade jobs ran; native builds were skipped. The later evidence
commit changes no tested controller or product bytes.

## Actual product inputs

Both candidates use committed VERSION0.2.0. A is the accepted **new unpublished
bootstrap** source `5cefcfdfec4ba35b400ff32ae7992639257211a6`, iteration1; B is
reviewed product source `a9df2f1d46fb08bcf53200b738e0fdc3c127b636`, iteration2.
Source/tree stamps and report hashes remain in the committed A/B manifests.

| Architecture | A deb SHA256 | B deb SHA256 |
|---|---|---|
| x86_64 | `3c70fa85ecd2c5478896b85dfc918cbc194aa9968c7c12e9a775cfed1ab6b794` | `2dca964a748a2058c7b52fc453f6907788ba03e8e02f26836dfd8ee830aacee4` |
| arm64 | `7948b9609928cf8ab35cb152fd6e61d58ed2059cd4dfa3f40efa922879a6e913` | `ab5dec3dc5b4d6902449dad490a8d38bd3a1fa6d759682b7963c7dc4ea077c3d` |

TarSHA256: `cfa6edb45cc7bf72f6cc5fd581bd2e58ebfbf379649919836d53bf6f92527210`.
Both jobs rechecked the tar/four manifests before apt, and the installed harness
independently recorded the actual two package hashes per architecture.

## Jobs and retained assertions

| Job | ID | Result |
|---|---|---|
| Isolated trusted draft fetch | 104350045831 | PASS |
| x86_64 Ubuntu24.04 | 104350136081 | 17/17 PASS |
| arm64 Ubuntu24.04 | 104350136080 | 17/17 PASS |

Both actual hosts reported Ubuntu24.04.5LTS, glibc2.39, kernel6.17.0-1022-azure,
and real UID1001 user managers. Apt used real passwordless sudo. These hosted
runners contain developer tools; this is not clean-machine or kernel6.8 proof.

| Identity | x86_64 | arm64 |
|---|---|---|
| Task | 00ce161a | 67b65c23 |
| Preserved agent PID/start ticks | 3788 / 60765 | 3863 / 59974 |
| Old → new daemon PID | 3675 → 4181 | 3769 → 4321 |

Each run proves apt replacement leaves the original live daemon/agent intact;
operator restart creates the expected successor generation while preserving
agent PID/start, task/run, branch and worktree. Observed runtime is busy after
restart; input is delivered once and recorded once against the same run;
completion emits the durable run.finished event. The eleven installed tests
and six upgrade tests, including ownership-checked cgroup cleanup, all pass.
This uses the measured scripted provider; live authenticated cross-machine
Codex transfer is separate and remains with acceptance641dbb6f.

## Evidence and transport cleanup

Committed raw JSONL API/events/input ledger and process identities, final Vitest
JSON reports, journals/unit records, run/job/step outcomes and Actions artifact
metadata: `docs/evidence/2026-09-15-linux-bootstrap/floor-ci-34959684410/`.
Private runtime files and generated credentials were not uploaded or committed.

Actions artifacts: verified tar10391619684 (7-day retention), x86 evidence
10392332961 and arm64 evidence10393130558 (14-day retention). Actions artifact
archive digests differ from the inner tar digest; the exact metadata is retained.
Local tar and both manifests/debs remain in this Ship worktree for delivery.

After both results were downloaded and retained, the authorized temporary draft
release389052859 and its sole asset565472294 were removed. Pre-removal metadata
verified the exact private draft/asset/hash; post-removal lookup returns404.
It was never published, latest, a Linux channel candidate, or a soak event.

The original generic-fixture ARM failure and the contents-read draft-fetch
HTTP403 run34958441800 remain retained. The latter is an **input-access failure**,
not an upgrade failure. The separate corrected ARM26.04 17/17 result is also
preserved. No result has been relabelled retrospectively.

## Delivery boundary

PR1513 can proceed to the requested ordinary Merge Master handoff; no additional
controller review round is required by root's execution instruction. Ship stays
open. Public Linux archive/key/host setup, authenticated system acceptance,
staging publication and its full24-hour tested-candidate soak, and explicit
production authorization remain distinct and incomplete. There is still no
public verified Linux download URL for kanna-web.
