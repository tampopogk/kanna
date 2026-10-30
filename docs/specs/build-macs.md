# Build Macs and the xcodebuild Shim

**Status:** planned · **Part of:** [Kanna Cloud](kanna-cloud.md#6-build-macs) · **Isolation:** [shared build Macs](build-mac-isolation.md) · **Research:** [xcodebuild shim](../research/xcodebuild-shim.md)

A task on Linux works on Apple code as if Xcode were local. Its `xcodebuild`, `xcrun`, `simctl`, `codesign` and project-chosen `swift` calls run on a **build Mac** against the agent's current worktree. Output and the exit code stream back, and test results, screenshots and built apps arrive where the agent expects them. Any number of Macs, Intel and Apple Silicon, form the **pool**. Build Macs are a [Kanna Cloud](kanna-cloud.md#6-build-macs) feature: [your machines](machines.md) aren't designed around them, though a self-hoster can run them, for example in their own Kanna Cloud.

## Contents

1. [Rules](#1-rules)
2. [A job](#2-a-job)
3. [Paths and artifacts](#3-paths-and-artifacts)
4. [The pool](#4-the-pool)
5. [iOS for remote tasks](#5-ios-for-remote-tasks)
6. [Build order](#6-build-order)

## 1. Rules

- **The shim is a PATH entry:** `/opt/kanna/current/shim` comes first on a Linux task's PATH. Each tool there is a link to `kanna build`, which does the remote work, so the agent, `scripts/build.sh` and anything Xcode-shaped reach it without knowing.
- **Transparent only as far as PATH reaches.** A script that checks `uname`, calls `/usr/bin/xcodebuild`, or runs a path from `xcrun --find` isn't caught. For those, `kanna build -- scripts/build.sh` runs the whole script on the Mac.
- **Swift is per project:** `swift build` on Linux stays Linux's unless the repo's `.kanna` settings send it to a Mac. The two platforms keep separate build folders.
- **Requests are structured:** arguments are passed as a list and spawned directly on the Mac, never pasted into a shell command.
- **The Mac never writes source back.** Files a build generates come back only as declared outputs.
- **A macOS VM is mandatory for remote builds:** several mutually untrusted users can share a physical Mac. Project code runs only in a tenant-exclusive guest; host accounts are not the tenant boundary. The host retains its `device.key` and never falls back to executing a build itself. See [isolation](build-mac-isolation.md) for credentials, networking, VM lifecycle and the required proof. The previous owner-account MBP run is a forwarding experiment, not the product execution mode.
- **UI tests use the guest console:** each tenant VM has its own logged-in job-user Aqua session and Simulator state. Concurrent tenants use separate guests; UI jobs within one guest serialize. Guest capacity is admitted against host resources and applicable licensing, not an assumed unlimited pool.

## 2. A job

```mermaid
sequenceDiagram
    participant AG as agent on r720-2
    participant K as kanna build (shim)
    participant D2 as daemon r720-2
    participant DM as host daemon + VM manager
    participant S as supervisor in tenant macOS VM
    AG->>K: xcodebuild -scheme App test
    K->>D2: job: argv, cwd, env allowlist, needs
    D2->>DM: pick a Mac (capabilities), open streams
    D2->>DM: sync the worktree snapshot
    DM->>S: run in the mirror, lock held
    S-->>DM: stdout, stderr, exit code (persisted)
    DM-->>D2: live output, then artifacts
    D2-->>K: raw output + exit code, mapped diagnostics
    K-->>AG: as if xcodebuild ran here
```

- **The snapshot:** the worktree's files as they are (modified, untracked, deleted, executable bits, symlinks), minus `target/`, `.build/`, `node_modules/` and the repo's excludes. Changed blocks only, rsync's algorithm, sent over a stream on the daemon link. Kanna takes it at submit time: the agent is blocked on the shim meanwhile, the files are copied to a staging folder first, and the snapshot's hash is recorded with the job. The live worktree is never read later.
- **The mirror** on the Mac is keyed by (machine, repo, task): a Mac-local checkout of the same commit with the snapshot laid over it, since a linked worktree's `.git` file is meaningless there. Commits the Mac lacks (unpushed ones) travel with the snapshot as a git bundle, so the Mac needs no git credentials. It keeps DerivedData and SwiftPM's scratch folder outside the source, keyed by task and Xcode version, so rebuilds stay warm.
- **The lock** covers sync, run and artifact staging for one mirror. Other mirrors run in parallel, within the Mac's slots (§4).
- **The supervisor** persists each job's request ID, status, logs and artifact manifest. A dropped connection reattaches to the running job instead of starting another. Cancelling signals the job's process group and waits before releasing the lock.
- **The exit code** is the build's. A transport failure is a distinct error from Kanna, never a made-up exit code. Build status and artifact status are reported separately.
- **Xcode is pinned per job** with `DEVELOPER_DIR`, never `xcode-select`. The job records Xcode, Swift, SDK, arch and Simulator runtime.

## 3. Paths and artifacts

| | Linux | Mac guest |
|---|---|---|
| Source | `~/repos/app/.kanna-worktrees/task-1dfb/` | `~kanna-build/mirrors/r720-2/app/task-1dfb/src/` inside the tenant VM |
| Caches | — | `~kanna-build/mirrors/r720-2/app/task-1dfb/derived/<xcode>/` |
| One job's results | the path the agent named, or `.kanna/build/<job>/` | a fresh folder per job |

- **The working directory** keeps its path relative to the worktree. Known path arguments are mapped both ways: `-project`, `-workspace`, `-derivedDataPath`, `-resultBundlePath`, `-archivePath`, SwiftPM's `--scratch-path`. A file outside the worktree must be declared, never guessed.
- **Output is passed through raw.** A second, mapped stream of diagnostics (`file:line:col` rewritten to Linux paths) goes to the agent, so it can open the files named. stdout is never rewritten wholesale, since it may be JSON.
- **Coming back:** logs, the `.xcresult` summarised on the Mac by its own `xcresulttool` (JSON plus exported attachments), and screenshots, always. `.app`, archives and dSYMs come back on request or by repo policy, archived on the Mac to keep permissions and symlinks. Each has a checksum, and its path on the Mac is recorded.

## 4. The pool

- **Each build Mac advertises** admitted guest capacity, guest arch/macOS, installed Xcode versions, Simulator runtimes, cores, RAM and free disk. A job states what it needs (`xcode: 26.x`, `arch: arm64`); Kanna picks a compatible tenant-owned guest with the shortest queue, preferring one that already has the task's mirror.
- **Slots:** one heavy build and one UI-test slot per guest to start, with a host-wide admission cap based on measured memory/CPU pressure and allowed VM capacity. Xcode's own test parallelism is bounded as well.
- **Simulators are leased** by (guest, UDID) across a job's install, launch and screenshot steps, never `booted`. UI tests run through a LaunchAgent in the guest job user's console session.
- **Disk:** mirrors and DerivedData have a quota per Mac and are evicted least recently used first. Five servers' tasks fill one mini quickly.
- **A build Mac's daemon runs from launchd** (`kanna machine serve`), as a LaunchDaemon running as its service user, since no app may be open there ([machines §5](machines.md#5-the-app-link-over-the-network)).
- **Signing:** Simulator and ad-hoc signing for agent jobs. Distribution identities stay out of ordinary jobs, and a signed build uses a temporary keychain made for that job.

## 5. iOS for remote tasks

The Simulator runs on the MBP. A remote task's `.app`, built on a build Mac, is fetched over the link and installed with `simctl`, and the `ios-N` content lives on the MBP's daemon in the existing iOS pane ([app panes](app-panes.md)). Streaming a build Mac's Simulator screen comes later.

## 6. Build order

[Build order](machines-build-order.md#kanna-cloud), Cloud's slices 7 and 12:

- **7, first version:** one physical Mac, this MBP first then the mini, with builds inside a tenant macOS VM after the [isolation model](build-mac-isolation.md#5-proof-before-product) passes:
  - the shim and `kanna build`, and `kanna build -- SCRIPT`;
  - the snapshot sync, the mirror and lock, the supervisor;
  - raw output and mapped diagnostics, the exit code, artifacts.

  Acceptance covers files that are dirty, untracked and deleted; a warm rebuild; two tasks at once; cancelling; a dropped connection; a failed test's attachments; a screenshot coming back. Done when a Linux task builds Kanna.app and runs a Swift test.
- **12, the pool:** several Macs with tenant-isolated guest slots, matched by guest capability, guest-scoped Simulator leases, the queue shown in the app, the `.app` fetched to the MBP. Never pool different tenants inside one guest.
