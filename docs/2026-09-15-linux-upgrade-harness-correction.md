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

Pending explicit VM handoff from acceptance `641dbb6f`. Its live fixture must
not be interrupted. Exact A/B artifacts already on the guest will be reused;
controller changes will be applied and recorded independently of B source.
