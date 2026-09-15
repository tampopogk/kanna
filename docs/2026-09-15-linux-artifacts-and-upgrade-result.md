# Linux A/B artifacts complete; ARM upgrade gate failed

Ship task `d3ce8dec`. No publication, public archive/key setup or soak started.
Supersedes the earlier preparation/configuration blocker reports.

## Exact prepared sources

Both canonical `./kd release prepare --platform linux --ref <SHA>
--staging-iteration <N>` commands completed successfully:

- **A**, accepted new unpublished bootstrap predecessor:
  `5cefcfdfec4ba35b400ff32ae7992639257211a6`, tree
  `b9dc0f445cca4a4c9771c858c28b70f044df1d89`, committed VERSION `0.2.0`,
  iteration 1. Root accepted precisely two stamp-support files, +5/-1, over
  product source `79f1c8225`; no product/version change. The new SHA is never
  represented as the historical SHA. Patch, identity and reconstructible Git
  bundle are in `docs/evidence/2026-09-15-linux-bootstrap/`.
- **B**, explicit reviewed main:
  `a9df2f1d46fb08bcf53200b738e0fdc3c127b636`, tree
  `3415e82f8a273b6ff94494d367139038eb026a62`, committed VERSION `0.2.0`,
  iteration 2. This task's controller merges/docs did not enter either source.

Outputs: `.build/linux-prepared/<source-SHA>-staging.<N>/`, each with
`manifest.json`, both debs and adjacent `.deb.json` measured reports.
Committed manifests/reports and independent verification are under
[`docs/evidence/2026-09-15-linux-bootstrap/`](evidence/2026-09-15-linux-bootstrap/).

| Package | Bytes | SHA-256 |
| --- | ---: | --- |
| A `kanna-staging_0.2.0~staging.1-1_amd64.deb` | 44438078 | `3c70fa85ecd2c5478896b85dfc918cbc194aa9968c7c12e9a775cfed1ab6b794` |
| A `kanna-staging_0.2.0~staging.1-1_arm64.deb` | 43735536 | `7948b9609928cf8ab35cb152fd6e61d58ed2059cd4dfa3f40efa922879a6e913` |
| B `kanna-staging_0.2.0~staging.2-1_amd64.deb` | 44493092 | `2dca964a748a2058c7b52fc453f6907788ba03e8e02f26836dfd8ee830aacee4` |
| B `kanna-staging_0.2.0~staging.2-1_arm64.deb` | 43811392 | `ab5dec3dc5b4d6902449dad490a8d38bd3a1fa6d759682b7963c7dc4ea077c3d` |

Independent reading of all four actual Debian archives verified manifest/report
hashes, revision/tree stamps, control version/architecture, all **32 executable
hashes**, and zero reported audit findings. No unstamped CI package was imported.
At the last live check, CI `34951214464` at `f88d1467b…` remained in progress:
ARM build, both apt interoperability and prerequisite jobs passed; x86 build
was running. It is separate validation, not these collected package bytes.

## Actual ARM installed/upgrade run

Existing UTM VM `AFA4B3BF-1874-4CB2-AACD-82C47B8C2EC5`, Ubuntu 26.04.1,
ARM64/glibc 2.43: supplemental coverage, **not Ubuntu 24.04 floor proof**.
The pre-copy budget was 2,520,542,716 bytes versus 4,098,658,304 available:
87,546,928 compressed deb bytes, 256,858,112 bytes for twice the larger
Installed-Size, 28,654,028 exact-source archive bytes and 2 GiB dependencies/
transient allowance. Actual filtered harness dependency installation fit.
No earlier task/owner files were cleaned.

Artifacts and exact B source/dependencies remain under
`/home/jeremy/.tmp/kanna-d3ce8dec/`. Both guest deb hashes matched before testing.
Existing guest-agent root execution started the previously inactive real
`user@0.service`; no sudoers/auth/linger changes. The canonical command ran as
root with `HOME=/root`, actual `/run/user/0` bus, default root unit lookup, and
task-owned TMPDIR/cache. The initial manager-starting observation stopped before
testing; systemd's own readiness wait then admitted its settled degraded state.

```sh
./kd test linux-installed \
  --old-artifact /home/jeremy/.tmp/kanna-d3ce8dec/artifacts/kanna-staging_0.2.0~staging.1-1_arm64.deb \
  --new-artifact /home/jeremy/.tmp/kanna-d3ce8dec/artifacts/kanna-staging_0.2.0~staging.2-1_arm64.deb \
  --channel staging
```

**Exit 1; two reported failures**, both in `upgrade.e2e.test.ts`:

1. Line 205: runtime state immediately after restart was `null`, outside
   `busy|idle|waiting`.
2. Line 210: daemon PID changed `130891 → 131216`, failing the same-PID assertion.

The exact worker source deliberately spawns a new daemon generation and hands
off the old one; acceptance task `641dbb6f` found no worker-source delta A→B.
Therefore the second assertion conflicts with the stated existing contract;
it is **not by itself proof of product loss**. The null runtime observation is
a separate finding requiring temporal API/event evidence. No test correction,
gate override or unchanged rerun was made here.

What survived, and evidence limits:

- In the failed restart test, assertions before line 205 succeeded: new
  supervisor, original agent alive with equal PID/start-time identity, same
  task, running run, branch and worktree. Journal records worker
  `130882 → 131207` and scripted `claude-impl` PID `131023` present across
  restart. This provider is a fixture, not a live Claude or Codex model.
- The input/completion tests were not among reported failures: they assert one
  durable task/run input row, exact once-only submitted text, and completion
  detail/event. The surviving input trace independently contains **one**
  1048-character AFTER-UPGRADE message (SHA-256
  `5578445ea6176b8120705a10082aea8ba6926c306768691bc17d7363c7e008e5`)
  for fixture task/worktree `73b60dc4`.
- Numeric agent start ticks, run ID, raw API/ledger responses and daemon-specific
  log files were not emitted by the harness and its worker-data cleanup removed
  them. Assertion order is supporting evidence, not a substitute for those raw
  values. Do not invent timing or a handoff-protocol success trace. Preserved
  worker/systemd journal records generation startup and agent survival.

Sanitized failure output, journal, launcher, trace summary and cleanup are in
[`arm-gate/`](evidence/2026-09-15-linux-bootstrap/arm-gate/).

## Cleanup and next action

The harness removed `kanna-staging` and its unique units but left a daemon in
its test cgroup. After recording the cgroup and absence of unrelated root user
sessions, Ship stopped its newly started root manager/runtime directory. Both
are inactive; no Kanna process or unique test unit remains. VM ownership was
explicitly released to `641dbb6f` for the later jeremy/live-Codex system pass.
The existing user session/projects and prior task artifacts remain untouched.

A's first attempt was deliberately interrupted when Studio free disk reached
1.2 GiB (then 201 MiB during shutdown). Only the two verified task-owned Bazel
output directories were reclaimed; B deliverables and shared/other-task caches
were preserved. Recovered 113 GiB enabled the successful A-only retry. Logs
remain in `.tmp/linux-delivery/`. Preparation removes isolated source but leaves
large Bazel output bases: this measured cleanup gap deserves a bounded tooling
follow-up, not a publication bypass.

**Next:** root disposes the two upgrade-test findings and routes an instrumented
canonical rerun; x86 installed/upgrade and exact-floor coverage remain absent.
`641dbb6f` owns the distinct live-Codex system/transfer pass with an isolated
exact-B MBP development counterpart (explicit dev provenance, no Mac release
claim). No successful upgrade/system attestation exists yet. Real SSH archive,
Caddy/DNS and protected key setup still await concrete approval under the
committed operations proposal. Only an authorized, tested, publicly verified
candidate starts the full 24-hour soak; production remains separately authorized.
There is still **no public release URL** for kanna-web to link.
