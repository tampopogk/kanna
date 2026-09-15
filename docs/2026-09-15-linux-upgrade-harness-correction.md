# Linux upgrade controller correction

The original exact A/B ARM canonical run remains **FAILED**, retained in
[the artifact report](2026-09-15-linux-artifacts-and-upgrade-result.md).
The four collected packages are unchanged. This patch changes the controller
and synthetic provider fixture, not either product source or package.

## Source findings and assertions

- `crates/kanna-worker/src/supervisor.rs` explicitly spawns a successor daemon
  on launcher restart. A and B have the same worker implementation. Expect a
  new supervisor and daemon PID/start time; retain the same live agent
  PID/start time, task/run, branch and worktree. The outgoing daemon must exit.
- Package replacement still must preserve the original daemon and agent
  identities before the operator restart.
- `terminal_watcher.rs` startup `List` intentionally clears unobserved status
  to null. `session.rs::detect_headless_terminal_status_if_due` requires actual
  recognizable provider chrome before publishing the first verdict. The
  generic `SCRIPT_HEARTBEAT` fixture supplies none. This is not evidence of a
  persistent product defect or a missing startup delay.
- The upgrade fixture now opts into the measured Claude busy frame already
  used by the scripted status fixture. This is synthetic-provider acceptance,
  not a live Claude or Codex claim. The existing cursor-based task event API
  wakes reconciliation checks; a matching non-null busy task API state is
  required within a bounded deadline. No fixed startup sleep, accepting null,
  or assumption that an unchanged status must emit another edge.

Timestamped JSONL captures raw before/after process identities, task API,
reconciliation events, input ledger, input trace, apt output and journals.
The controller Git SHA/diff and actual input package hashes are recorded
separately. Both pass/fail paths preserve the unique worker's files before
cleanup; a JSON Vitest report retains the final assertion verdicts.
Cleanup verifies the generated unit's FragmentPath and unique name, then
uses systemd to empty only that unit's cgroup, including when apt removal has
already removed the worker executable. It never kills by process name.

## Floor CI preparation (not dispatched)

The existing Linux workflow has an optional prepared-pair mode. It skips
native builds, downloads a checksum-pinned tar, verifies its four regular
file members against the committed A/B manifests, and runs canonical installed
and upgrade checks on both Ubuntu 24.04 architectures. It retains raw evidence
on failure as well as success. No rebuilt or unstamped package substitution.

Local bundle: `.tmp/linux-delivery/prepared-pair.tar`.
SHA256: `cfa6edb45cc7bf72f6cc5fd581bd2e58ebfbf379649919836d53bf6f92527210`.
The bundle contains only the exact four debs named in the existing manifests.
Local verifier accepted the actual bundle and rejected traversal, symlink and
wrong-size substitutions. Workflow YAML parses; harness typecheck, six layout
tests and twelve existing scripted-agent tests passed.

Execution still requires an approved CI-readable HTTPS location for that
validation bundle and publication of this controller workflow to a dispatchable
ref. Neither has occurred; this is not a public download or archive setup.
No product rebuild, release publication, promotion, or soak has occurred.

## Corrected ARM execution

**PASS: canonical exit 0, 17/17 assertions (11 installed + 6 upgrade).**
Executed 2026-09-15T10:16Z after acceptance explicitly released the VM.
Tested controller commit `3b48f812f`; the guest retains exact B as its Git base
plus the independently recorded controller patch, SHA256
`b4f7ded3bb2d1507a5b1d8ad2350d42620cd2c0e8ef0d84e002a8163e8b144ac`.
Both existing ARM deb hashes were checked immediately before canonical `kd`.
No product source or artifact changed.

| Identity | A / before restart | B / after restart |
|---|---|---|
| Supervisor PID / start ticks | 144126 / 8506744 | 144575 / 8507090 |
| Daemon PID / start ticks | 144135 / 8506746 | 144584 / 8507092 |
| Agent PID / start ticks | 144307 / 8506803 | 144307 / 8506803 |
| Task / run | bc81424a / run-bc81424a-1789467384400632547 | unchanged |
| Branch | task-bc81424a | unchanged |

The worktree remained the same task-owned fixture path. Apt replacement left
the original daemon and agent alive; `/proc` showed the original binaries as
`(deleted)` until operator restart. The successor then ran the new installed
executable, the old daemon exited, and the agent remained the same process.
One post-upgrade message appeared once in the agent trace and once in the
input ledger, linked to the original run; completion retained that run and
emitted `run.finished`.

Runtime chronology: initial observed busy event at 10:16:24.718Z; API busy at
24.728Z; after-restart HTTP snapshot already busy at 28.120Z. The subsequent
10-second event wait returned an empty batch and the API remained busy at
38.164Z. No fresh edge was fabricated, and no null occurred in this corrected
fixture run. This proves observed-state continuity for the measured fixture;
it does not retrospectively relabel the original generic-fixture failure.

Raw timestamped API/events/ledger and process identities, final JSON assertion
report, journals, exact controller patch and launcher are committed in
`docs/evidence/2026-09-15-linux-bootstrap/arm-corrected/`. Full private synthetic
runtime files are retained only under task-owned `.tmp` and on the guest.
The workflow upload allow-list excludes those runtime files and generated
credentials; this CI-only retention adjustment followed the ARM run.

Cleanup passed in the harness: unique units removed and cgroups emptied even
after package removal. The test-owned root manager and runtime directory are
inactive again. `kanna-staging` is absent because the canonical removal test
removed it; both verified debs remain on the guest. Free space 2,555,555,840
bytes. VM explicitly released to acceptance `641dbb6f`, with its private data
untouched. No owned process remains.

This is ARM Ubuntu 26.04.1 synthetic-provider coverage, not Ubuntu 24.04 floor,
x86 acceptance, live transfer, publication, or soak. Those gates remain open.
Next bounded action: assess this controller patch/result, then make the exact
validation bundle available to the prepared CI lane and dispatch the reviewed
controller ref. Public release/archive setup stays held.


## Authenticated validation transport

The owner authorized a temporary **draft** GitHub validation release containing
only the exact tar above. The prepared workflow now takes
`prepared_pair_asset_id`, not an arbitrary URL. The job's contents-read token
is sent only to the fixed `api.github.com/repos/tampopogk/kanna/releases/assets/`
endpoint. Automatic redirects are disabled; only HTTPS GitHub asset-storage
hosts are accepted, with a new unauthenticated request. Draft permission errors
fail closed and never publish the draft. The tar digest and all four manifest
checks remain mandatory. Five focused transport tests passed.

The controller push uses `[skip ci]` to honor the explicit instruction not to
rebuild unchanged packages. The new transport must be reviewed before the
separate prepared-pair dispatch. This is validation transport, not a published
Linux product, channel update or start of soak.

### Concrete draft/PR handoff

- PR: https://github.com/tampopogk/kanna/pull/1513 (draft).
- Transport controller: `a3f962478fbf85854b0fe3559f83b526a70ee6d7`.
- Temporary draft release ID: **389052859**; tag
  `validation-d3ce8dec-linux-ab-20260915`; target is that controller SHA.
- Its sole asset is `prepared-pair.tar`, asset ID **565472294**,
  **176486400 bytes**, GitHub-reported SHA256
  `cfa6edb45cc7bf72f6cc5fd581bd2e58ebfbf379649919836d53bf6f92527210`.
- Readback metadata confirms `draft=true`, `prerelease=true`,
  `published_at=null`. Creation used `--latest=false`. The first attempt with
  an abbreviated target SHA was refused (HTTP422); the full40 target succeeded.
- The actual new fetch helper downloaded the draft with the existing operator
  credential and verified the tar digest. **This does not prove that a job's
  contents-read token can read draft assets.** The reviewed workflow execution
  must establish that; a403/404 must remain a limitation, never trigger draft
  publication.
- Pending transport review, dispatch the existing workflow at the reviewed
  controller ref with `prepared_pair_asset_id=565472294` and the tar hash above.
  Its prepared-upgrade matrix supplies both Ubuntu24.04 architectures and
  skips native builds. No dispatch has occurred.
- Retain validation logs/results before removing this temporary draft after
  successful validation, if it is no longer needed. Nothing here grants a
  public artifact URL, Linux channel state or soak time.
