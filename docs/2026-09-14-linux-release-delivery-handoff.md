# Linux release/download delivery: blocked at product integration

Task: `d3ce8dec`. Requested by Jeremy Hale: “can we like release the linux app and then put it on [kanna.build]”. No publication occurred.

## Exact source and runtime

After `git fetch origin --tags`, this task's clean `HEAD` and `origin/main` both
resolved to `ac2604a6e092771968f7f9096d4c4b917ad036b1`. There was no delta from
the manager-verified source. This document is the only subsequent task change.
`kanna_info` identified the effective connection as `http://127.0.0.1:48121`,
server staging `0.3.0-staging.20`, Jeremy’s Mac Studio; the separately advertised
LAN endpoint was `0.0.0.0:48121`. This is not the owner's MacBook Pro signing host.

## What actually exists

- PR #1430 (`4e6275041`) supplies Zig 0.15.2, pinned Noble sysroots, glibc 2.39
  toolchains and architecture canaries. PR #1444 (`965cd3fc`) adds the eight
  Rust universes and GTK/WebKit/OpenSSL dependency build-script closure.
  The final proof boundary in
  [the cross-release inventory](2026-09-10-linux-phase3-cross-release-remainder.md)
  explicitly excludes product executables, packages and release integration.
- Current `MODULE.bazel` really includes both Linux triples. Describing the
  entire graph as Darwin-only is stale. However, `packaging/linux/BUILD.bazel`
  has tools/tests and no package target; `crates/kanna-worker` has no BUILD
  target. The next product boundary remains unimplemented.
- `tools/kd/src/runtime/linux-release-build.ts` still builds all eight
  executables through Cargo. These prototype packages cannot be published under
  `.kanna/agents/ship/EXTEND.md`'s Linux contract.
- `release-platform.ts` defines independent `desktop-linux-staging` /
  `desktop-linux` channels, `linux-v…` tags and `release/linux/X.Y` branches.
  Its platform-selection functions have no production call sites. The generic
  CLI parser accepts `--platform`, but the release input schemas and ship handler
  do not implement platform selection; the handler invokes macOS notarization
  and `shipRelease`. Do not attempt a Linux dry-run through that handler.
- The apt transaction/signature modules exist, but no concrete production
  storage adapter or release-command integration calls `publishAptArchive`.
  Test-key interoperability is not owner-key custody or a live signed archive.

## Existing artifacts and acceptance

[Linux Release Check 34878212138](https://github.com/tampopogk/kanna/actions/runs/34878212138)
succeeded at `9d01905f7e29d7113d2f84ec054f2741b4928148`: both builds,
both install-only jobs and both apt-interop jobs passed. There is no delta from
that source to this task's main in the workflow, packaging/linux,
linux-release-build.ts or tests/linux-installed. This is compatible prior
evidence, not certification of the exact current-main product.

CI logs show `kanna-staging_0.0.68~staging.1-1_amd64.deb` and
`kanna-staging_0.0.68~staging.1-1_arm64.deb` installed successfully through apt.
GitHub reported these unexpired CI archives during inspection:

| Architecture | CI artifact ID | Archive SHA-256 (not the contained deb checksum) |
| --- | --- | --- |
| x86_64 / amd64 | 10361884945 | 3e7763589bf2e967b48ff97d81ac4e6b16872cfd7a1268f96974d38a4b2916a9 |
| arm64 | 10361948939 | 51cef7f97c9870c093319c6a19bb6c9f297259873d7b276483530d5b811192d9 |

These are prototype Cargo CI archives with 14-day retention, not website
downloads. They were inspected through metadata/logs, not downloaded or
independently hashed in this task. The workflow runs only
`installed.e2e.test.ts`; it explicitly omits `upgrade.e2e.test.ts` because no
distinct old/new package pair exists. Earlier x86 install failures must not be
carried forward as unresolved. Two-version live-upgrade proof remains absent.
Task `9823e28e` was a development GUI test drive, not installed release proof.
No duplicate CI dispatch, build or VM interaction was needed for this finding.

## Commands and disposition

`./kd release status` exited 0, reporting macOS production `0.2.0` and staging
`0.3.0-staging.20`, with an active `release/0.3` freeze. At inspection,
`promotion.allowed` was false: the candidate had soaked 23.23 of 24 hours.
This state is unrelated to Linux eligibility and was not changed.
`git ls-remote origin 'refs/tags/linux-v*' 'refs/tags/desktop-linux*'
'refs/heads/release/linux/*'` returned no refs.

There is no failed build/publish output to report: no ship command was run,
because source inspection proves it cannot select a Linux publisher. No macOS
dry-run, release, promotion, recut, reset, OTA, cloud change or notification
was performed. No owned background process remains.

## Smallest next executable correction

1. Implement the already-defined product/package slice: Linux desktop, worker
   and six sidecar BUILD targets for both architectures, static Ghostty/C++
   closure, and Bazel outputs feeding the existing package layout and ELF audit
   through `kd`. Retain an explicit barrier against publishing Cargo outputs.
2. Wire platform selection through release schemas, registry, status, lineage,
   build and publication; reject unsupported selectors before any macOS
   credential/build path. Connect the apt transaction to real storage and
   owner-host key preflight. Update the ship runbook to describe implemented
   Linux behavior rather than the obsolete all-Darwin statement.
3. Produce both architectures at one version/revision and distinct predecessor
   packages; run `./kd test linux-installed --old-artifact <deb>
   --new-artifact <deb> --channel staging` on each matching acceptance host.
   Preserve audit and install/live-upgrade evidence against actual package hashes.
   The existing UTM VM can supplement graphical acceptance; its dev build
   cannot replace this gate. Preserve user projects and use isolated identities.
4. Before real publication, select the independent Linux channel explicitly.
   A production operation still requires a named-human request explicitly saying
   production. Provision/verify apt key custody on the owner's trusted MacBook
   Pro and the intended archive storage through the completed canonical procedure.
   Do not solve this by promoting the macOS candidate.

The first action is implementation of item 1, not another generic launch audit
or rebuilding the known prototype. This shipping task stopped at the missing
implementation boundary required by its execution contract.

## kanna-web handoff

### Latest owner acceptance requirement

The owner requires “we need to have the 24 hours soak before we release to
production” and vigorous system tests including task transfers. No soak override
or reset is authorized. Linux must complete its own canonical staging candidate,
tests and full 24-hour soak before production; the macOS `.20` candidate's age
does not certify Linux or newer source. `release-policy.json` already declares
Linux's 24-hour policy, but the platform-specific execution path is not wired.
Acceptance task `641dbb6f` owns cross-machine/system tests. Its evidence must name
the actual source, artifact hashes, architecture and installed version on both
ends of each transfer. Development/prototype tests can establish bounded system
behavior but cannot certify the missing supported release artifacts. Rebuilding
a changed candidate requires acceptance and soak for that candidate.

**Publishable Linux version, artifact URL and deb checksums: unavailable.**
Required architectures remain x86_64/amd64 and arm64. Do not create a download
button pointing at these CI archives or an inferred future release URL.

After the real release, hand over the immutable release URL, exact version and
source SHA, per-architecture deb URLs and SHA-256 values, audit/installed-upgrade
results, apt base URL and channel, public key URL/full fingerprint, and verified
Signed-By bootstrap/install/update instructions. The intended local-deb install
shape is `sudo apt install ./<verified-package>.deb`; this is not presently a
complete public install instruction or an apt update subscription. Final user
guidance must cover repository bootstrap, updates, launcher restart/session
survival, removal and the measured support limits. Website publication must
wait for those actual verified artifacts.
