# Linux staging setup execution (in progress)

Reviewed controller: PR1515 merge
40cc4453d01e5bb2c992af9870d2f9b10c77f28c,
tree8b27250f7e98e21ae713e69ca2111e9b6a7fee74.
Product B remains a9df2f1d46fb08bcf53200b738e0fdc3c127b636,
VERSION0.2.0/staging iteration2. Desktop staging.21/dc75f7a3 and its desktop
soak timestamp are separate and do not certify or start Linux soak.

## Actual reviewed MBP inspect/plan

Both canonical commands exited0 from MBP task6a8d78d0's clean isolated
`.tmp/linux-controller`, with existing adminjeremyhale, existing private
identity path `/Users/jeremyhale/.ssh/google_compute_engine`, and committed
staging-host-key.pub. No apply was attempted. DNS passed, noAAAA/CNAME;
layout supported, unmanaged, accountUidnull. The >=1GiB available-space gate
passed; exact freeBytes was deliberately omitted by the controller and is not
claimed. Both snapshots report **relayConnections1 / proxyChangetrue**.

Plan path on MBP:
`/Users/jeremyhale/.kanna/repos/kanna-7/.kanna-worktrees/task-6a8d78d0/.tmp/linux-controller/.tmp/linux-archive-plan.json`.
Plan confirmation digest:
`a9bce1b2d64425a74b0fc9f255cca1ee86b7ea98e05c67c8c2a66d712693bb97`.
File SHA256:
`792fa249b2e8b0787961777e2f53172a02be4b59bc3fc3933ad850cdb25743f8`.
**This nonzero plan is evidence only, not an apply input.**

Observed staging relay container:
`382f9683342dcb43a0f18784ba167e41cd0ec736ff96b1adb605c83a04ed504a`,
image`sha256:0c6fa6942f24e6e5538b978c00a831e3ffacfadbc627fefbeb600958b194d3ea`,
started2026-09-14T04:43:20.351809162Z.
Caddy container:
`d663c9bcd9349b5fd071460a5208c5b8c3ad345bc32d495afdfd560981974ead`,
image`sha256:5f5c8640aae01df9654968d946d8f1a56c497f1dd5c5cda4cf95ab7c14d58648`,
started2026-07-15T09:23:21.20464501Z.

## Maintenance coordination

Acceptance641dbb6f confirms Studio C stopped14:45:23Z, source5d2e372c quit
once/exit0, no in-flight transfers, canonical dev down exit0 and no owned
listeners/processes. Private fixtures/DB/nonce/dirt are retained. MBP C cleanup
by helper074f03cc is confirmed complete; both endpoints remain paused. Its
disclosed disposable transfer identity is retired and must not be reused. No operator clients were touched and no
current global-zero claim is made. A fresh zero-count plan and immediate
canonical pre-replacement recheck are required after cleanup confirmation.
Acceptance stays paused until explicit release.

The fresh post-pause plan at 14:50:31Z still reported count1 with the same
plan digest. Private authenticated stats identify deployed commit3ce0847d01af,
**seven open sockets and seven live rows**, with no disposable C IDs. The
aggregate1 counts paired user buckets, not sockets. Source at that deployed
commit confirms both socket fields use the same open-account registry.

A bounded controller correction replaces the health user count with authenticated
socket stats, records the deployed commit, refuses missing/mismatched operator
visibility, and checks again immediately before proxy recreation. Late arrivals
restore staged config without restarting Caddy. Ten focused setup tests pass,
including unbound sockets with zero users, auth/malformed stats refusals and
arrivals at both apply boundaries; kd typecheck passes. An accidentally broad
initial test invocation was stopped; no full-suite result is claimed.

Owner-controlled maintenance must pause the installed Studio/MBP and phone
relay clients; Ship will never quit them to force zero. Hosting approval remains
granted. No apply on the old controller or count-only plan, even if user count
later reaches zero. A new corrected zero-socket plan is required.

## Retained bytes on the release host

Live GitHub API confirms unexpired Actions artifact10391619684,
`verified-prepared-pair`, run34959684410/controller2b5382265. This is the
previously verified exact stamped tar, not native CI's unstamped build output.
Archive digest`sha256:bfbd8c8ec1b918e70533c34ed5c9768258f3a9de9f36886d9bfbf3534e0f1522`;
inner pair.tar SHA256
`cfa6edb45cc7bf72f6cc5fd581bd2e58ebfbf379649919836d53bf6f92527210`.
Read-only download plus the existing four-manifest verifier can recover B's
original debs on the MBP without any rebuild/upload/private-key transfer.
The committed B manifest and reports complete its local prepared input.
After explicit no-download reconciliation, a new bounded instruction completed
one download and all four exact-package checks on the MBP. B manifest and two
reports are byte-identical to committed evidence. Local manifest:
`/Users/jeremyhale/.kanna/repos/kanna-7/.kanna-worktrees/task-6a8d78d0/.tmp/linux-controller/.tmp/prepared-pair-34959684410/B/manifest.json`.
Manifest SHA256 `40c09771fe5b09f07436dc70966cfe16626ecdfd325675f0887a33914c64439b`.
Portable preparation result is adjacent at `../preparation-result.json`.
No package rebuild, upload or signing occurred.

No real Linux key, release selector, host account, Caddy configuration,
publication receipt, package URL or soak was changed by inspect/plan. Hosting
and DNS approvals are already granted; no repeated approval is requested.
Ship remains open for actual setup, HTTPS/RPC readback and authorized B staging
publication, followed by verified artifact delivery to website76e93c62.

## Explicit interruption approval and canonical orchestration

Root asked Jeremy, "Can we do the brief staging interruption now?" Jeremy
replied "ok"; root directed Ship to proceed while leaving owner apps and PTYs
running. Timing approval is resolved and must not be requested again.

Fresh MBP inspect/plan at merged6a03281d5f1514c33f04e0a136f4152f389ceead
exited0; authenticated deployed3ce0847d01af still has seven sockets/live rows.
Plan digest94263575264d3943816662ab32e64c8a282e9bd3e9b47f94ebf1b09bb801c8ce.
This controller cannot arrange disconnect/reconnect while apps remain open.
The bounded correction adds explicit plan-bound `--disconnect-relay`, stopping
only Caddy, observing actual socket drain with public admission closed, and
restoring Caddy in success/failure cleanup. It preserves zero sockets, source,
config ownership and rollback checks. Public staging relay health/source is
verified before apt-key HTTPS/RPC/env-selector handoff. No actual stop/apply has
occurred yet. Both C test endpoints remain paused; B bytes on MBP unchanged.
