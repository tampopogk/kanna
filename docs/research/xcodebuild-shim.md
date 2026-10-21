# Redirecting xcodebuild to a build Mac

> 2026-10-21. Research by Codex, read-only: prior art, three designs, a first version. Copied from the planner's scratchpad into the repo; what was decided is in the [fleet spec](../specs/fleet.md). The prompt it answered: a `/kanna/bin/xcodebuild` on a Linux task's PATH calling `kanna build xcodebuild`, the worktree synced to a mirror on the Mac, a lock per mirror, products back, later a job queue in the Mac's daemon.

**Kanna should keep the proposed PATH shim and rsync-over-SSH design, backed by a small Mac-side execution supervisor from the first version.** Add consistent source snapshots, explicit path and artifact mappings, host-wide resource limits, and durable job status. A full queue can follow without changing the agent-facing commands.

This assessment reflects public documentation available on **21 October 2026**. Implementation recommendations below are engineering judgments; this research did not modify files or install anything.

The main qualification is “transparently”: forwarding supported commands is achievable. Making every Linux shell script behave as though it runs on macOS is not. Kanna should support both individual command forwarding and explicit execution of an entire script on the Mac.

The relevant prior art spans several different layers:

| Prior art | What it does and moves | Implication for Kanna |
|---|---|---|
| **Mainframer / SSH wrappers** | Mainframer pushes project files, executes a remote command, and pulls results back using SSH and rsync. Its published README still describes 2.x as stable and 3.x as future development; that wording alone does not establish active maintenance. | This is the closest architectural precedent. Borrow the workflow; assess maintenance separately before adopting its implementation. [Mainframer](https://github.com/buildfoundation/mainframer) |
| **Agent-oriented Xcode wrappers** | `xcode-remote` builds on a Mac build host, transfers the `.app` to another machine through rsync/scp, launches it, and returns structured build/runtime feedback. Its documented direction differs from Linux-source-to-Mac mirroring. | Structured results and runtime feedback are useful precedents, but this is not a complete substitute for Kanna’s source transport. [xcode-remote](https://github.com/blackpaw-studio/xcode-remote) |
| **Bazel Apple/Swift rules and Buck2** | Bazel represents builds as actions with declared inputs and outputs. Apple/Swift rules supply platform-specific build logic. BuildBuddy publishes a concrete Linux-host-to-macOS-executor example. Buck2 also supports Bazel’s Remote Execution API. | Linux orchestration of Apple builds is practical today, but requires a compatible build graph and Mac execution environment. [Linux-to-Mac demo](https://github.com/buildbuddy-io/linux-to-mac-rbe-demo), [Swift rules](https://github.com/bazelbuild/rules_swift), [Buck2 remote execution](https://buck2.build/docs/users/remote_execution/) |
| **`rules_xcodeproj` and `xcode-build-server`** | The former generates Xcode projects from Bazel targets. The latter implements the Build Server Protocol for SourceKit-LSP, including extracting compilation information from Xcode build logs. Neither supplies general source mirroring and remote command execution. | They address IDE integration. Returning build diagnostics does not automatically give a Linux editor Apple SDK-aware language services. [rules_xcodeproj](https://github.com/MobileNativeFoundation/rules_xcodeproj), [xcode-build-server](https://github.com/SolaWing/xcode-build-server) |
| **Tuist** | Current Tuist supports module caching with `.xcframework`s and remote sharing of Xcode compilation-cache artifacts. Its documentation identifies compilation caching as an Xcode 26 feature. | Useful later for cache reuse; a remote cache does not execute an arbitrary `xcodebuild` command for Linux. [Xcode cache](https://tuist.dev/en/docs/guides/features/cache/xcode-cache), [module cache](https://tuist.dev/en/docs/guides/features/cache/module-cache) |
| **GitHub self-hosted runners / Buildkite** | Mac agents receive jobs, obtain source through checkout steps, execute commands, report logs/status, and publish selected artifacts. Buildkite documents multiple agents and launchd integration on macOS. | Strong precedents for scheduling and lifecycle management. Commit-oriented checkout needs an additional snapshot mechanism for dirty agent worktrees. [GitHub runners](https://docs.github.com/en/actions/reference/runners/self-hosted-runners), [Buildkite macOS](https://buildkite.com/docs/agent/self-hosted/install/macos), [artifacts](https://buildkite.com/docs/pipelines/configure/artifacts) |
| **distcc** | Distributes individual C-family compilation jobs; linking remains local. Ordinary mode sends preprocessed input, while pump mode can move source/header dependencies. | Its compiler-wrapper pattern is relevant, but its execution model does not cover Swift, Xcode project evaluation, signing, or Simulator tests. [distcc manual](https://github.com/distcc/distcc/blob/master/man/distcc.1) |
| **idb / `simctl`** | idb separates a Mac companion from a client that can run elsewhere, providing remote device/Simulator automation. Apple documents `simctl io <device> screenshot <output.png>`. | Use `simctl` for initial lifecycle and screenshots; consider idb for richer interaction. idb’s private-framework dependencies warrant compatibility testing against pinned Xcode versions. [idb](https://github.com/facebook/idb), [Apple Simulator reference](https://devstreaming-cdn.apple.com/videos/wwdc/2019/418o9bbtoe880sauh/418/418_getting_the_most_out_of_simulator.pdf) |

**Design (a), a PATH shim with a persistent mirror, best matches the immediate requirement.** `/kanna/bin/xcodebuild` forwards to `kanna build xcodebuild ...`; the client transfers current source, invokes the Mac tool, streams output, and retrieves selected outputs. Stable source and cache directories preserve Xcode and SwiftPM incremental state.

Rsync offers an explicit synchronization boundary and delta transfers. Mutagen can keep a mirror warm continuously, but continuous updates must not modify source during a build. Its one-way-replica mode also overwrites remote changes, so it needs carefully excluded output directories and a flush/freeze or staging boundary. Start with rsync; add Mutagen only if measured synchronization latency justifies the extra state. [rsync manual](https://rsync.samba.org/ftp/rsync/rsync.1), [Mutagen synchronization](https://mutagen.io/documentation/synchronization), [flush behavior](https://mutagen.io/documentation/introduction/getting-started)

**Design (b), a Mac job queue, changes admission and durability more than compilation.** A request contains a commit or snapshot identifier, arguments, working directory, toolchain, resource requirements, and artifact declarations. The Mac materializes it into a task-specific workspace and runs the same executor.

A Git ref alone misses unstaged modifications, deletions, and untracked files. Accept either an immutable commit identifier for clean CI-style work or a pushed snapshot containing the actual working files. Queue submission must capture those files then—not read the live worktree when the job eventually starts. Stable per-task workspaces preserve incremental caches; fresh checkout paths for every job undermine that benefit. The queue provides fairness, reconnectable logs, cancellation, and explicit Simulator leases, but requires more lifecycle machinery.

**Design (c), REAPI, is appropriate when the project already uses Bazel/Buck or action-level distribution becomes a deliberate investment.** Clients upload content-addressed input trees and commands; workers execute actions; output trees and logs return through content-addressed storage. Results include exit codes, and optional ByteStream endpoints support live stdout/stderr. Execution may occur more than once, so externally visible side effects require care. [REAPI specification](https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/remote_execution.proto)

Its incremental model is action-cache reuse rather than a shared mutable DerivedData directory. Execution platforms select matching macOS/Xcode workers; relative execution-root paths and compiler path normalization improve portability. Simulator resources and signing still need separate scheduling and secret handling. Wrapping an entire stateful `xcodebuild` invocation in one REAPI action is possible in principle, but forfeits much of the benefit while retaining substantial protocol complexity.

For the first implementation, the following details determine whether the shim feels dependable.

1. **Define a bounded compatibility contract.**

   Forward `xcodebuild`, selected `xcrun` commands, `codesign`, and project-configured Swift commands. Preserve argument boundaries using a structured request and direct process spawning; avoid constructing shell commands from interpolated arguments. Include task ID, request ID, relative working directory, environment allowlist, toolchain selection, and artifact rules.

   Offer `kanna build -- scripts/build.sh` to run the entire script remotely. A Linux script may inspect `uname`, invoke `/usr/bin/xcodebuild`, or execute a path returned by `xcrun --find`; PATH interception cannot transparently handle all these cases. Remote `--show-sdk-path` output also names a Mac filesystem location, not a locally usable SDK.

   Make Swift forwarding project-specific. Linux-native Swift builds/tests remain useful, and ordinary Mac-built binaries cannot execute on Linux. Keep the two platforms’ build directories separate. [Swift platform and artifact guidance](https://docs.swift.org/latest/documentation/swiftlinux/building/)

2. **Transfer the actual worktree consistently.**

   Identify mirrors by repository identity plus globally unique task/worktree ID—not branch name or basename. Include modified tracked files, untracked build inputs, executable bits, symlinks, and deletions. Exclude caches and artifacts, but do not assume every ignored file is irrelevant: generated configuration may be required.

   A linked worktree’s `.git` file points into another repository’s administrative directory. Copying it verbatim produces invalid Mac metadata. Maintain a Mac-local checkout when scripts need Git, overlay the working snapshot, and record the original commit and dirty state separately. Handle submodules, LFS contents, and local package dependencies explicitly. [Git worktree internals](https://git-scm.com/docs/git-worktree.html)

   Hold the mirror lock across synchronization, execution, and artifact staging. Separately ensure consistent capture on Linux: briefly pause task writes or use a filesystem snapshot. A remote lock cannot stop local edits during rsync. Reject or explicitly map symlinks escaping the workspace and detect filename collisions on case-insensitive destinations.

3. **Keep paths stable and map them deliberately.**

   A workable mapping is:

   ```text
   Linux: /worktrees/<task>/...
   Mac:   /Users/kanna/workspaces/<repo>/<task>/src/...
   ```

   Preserve the relative current directory. Translate recognized path arguments such as project/workspace paths, `-derivedDataPath`, `-resultBundlePath`, archive paths, and SwiftPM scratch paths. File arguments outside registered roots need declared uploads; arbitrary absolute-path rewriting is unsafe.

   Preserve raw logs and command stdout. Produce a separate mapped diagnostic view so `/Users/kanna/.../Foo.swift:42:7` opens the corresponding Linux file. Do not globally substitute stdout: it may contain JSON, binary output, or paths consumed by another command.

   Retain the mapping and snapshot identity with each job. Generated files and dependency diagnostics need their own retrievable mappings. Debugger paths, index stores, and paths embedded in `.xcresult` remain distinct concerns; text-log rewriting cannot fix them.

4. **Separate source, caches, and results.**

   Keep persistent DerivedData and SwiftPM scratch storage outside the source mirror, keyed by task and toolchain/platform profile. Use a fresh result directory per invocation. Honor explicitly supplied paths through mapping rather than silently replacing them.

   Pin `DEVELOPER_DIR` per job; record Xcode, Swift, SDK, architecture, and Simulator runtime versions. Avoid changing global `xcode-select` while jobs run. Keep package resolution reproducible and dependency credentials on the Mac with limited scope.

   Preserve unchanged-file timestamps and avoid unconditional clean builds. Do not copy Mac `.build` or DerivedData into Linux caches. Apply disk quotas and retention: five Linux servers can produce considerably more persistent state than one mini can comfortably retain.

5. **Add host-wide scheduling alongside mirror locks.**

   Mirror locks prevent two commands from corrupting one task’s build state. They do not prevent ten tasks from exhausting RAM or launching competing Simulator sessions. Start with one heavy build slot and one UI-test slot; increase build concurrency after measuring warm builds and memory pressure.

   Assign explicit Simulator UDIDs and lease them across related install, launch, interaction, and screenshot commands. Avoid the global `booted` selector in multi-task operation. Bound Xcode’s own test-worker parallelism as well as Kanna job concurrency.

   Treat the dedicated logged-in GUI session as a shared resource. Run GUI-dependent work through a user LaunchAgent with required permissions. Serializing UI jobs initially is a reliability policy, not an assertion that Xcode cannot parallelize Simulator UI tests: Apple supports such parallel testing. [Buildkite launchd setup](https://buildkite.com/docs/agent/self-hosted/install/macos), [Apple parallel testing](https://developer.apple.com/videos/play/wwdc2020/10221/)

6. **Make completion survive transport failure.**

   Stream stdout/stderr without allocating a PTY by default, preserving binary-safe channels and keeping Kanna status messages separate. Return the remote process exit code when transport and required artifact delivery succeed. SSH itself returns the remote status or 255 for an SSH error, so transport failure needs distinct metadata. [OpenSSH manual](https://man.openbsd.org/cgi-bin/man.cgi/OpenBSD-current/man1/ssh.1)

   The Mac supervisor should persist request IDs, process status, logs, and an artifact manifest. Reconnection attaches to the existing job rather than blindly launching another. Forward cancellation to the process group and wait for termination before releasing locks.

   Store build status and artifact-transfer status separately. A successful build with missing required artifacts must not appear wholly successful; a failed build should still return available diagnostics.

7. **Return useful artifacts without synchronizing source backward.**

   Retrieve logs, structured test summaries, and requested screenshots automatically. Return `.xcresult`, `.app`, archives, dSYMs, and other large products according to policy, always recording their remote locations and checksums.

   Parse `.xcresult` on the Mac with its matching `xcresulttool`, then return JSON and exported attachments for Linux agents. Apple documents summary, test-detail, build-result, and attachment-export commands. [Xcode release notes](https://developer.apple.com/de/documentation/xcode-release-notes/xcode-16_3-release-notes)

   For `simctl ... screenshot shot.png`, create the remote file and retrieve it to the expected local path before returning success. For `.app` bundles, preserve executable permissions and symlinks; a Mac-created archive avoids lossy cross-platform copying. A returned app is an artifact, not something Linux can launch.

   Build-generated source changes should come back as explicit outputs or patches with conflict checking. Bidirectional source synchronization could overwrite edits made while the build ran.

8. **Keep signing and trust boundaries explicit.**

   Use simulator/ad-hoc signing where appropriate for development. Keep distribution identities out of ordinary agent jobs. For authorized signed builds, use a dedicated signing context with explicit keychain selection, noninteractive access, provisioning profiles, and cleanup. GitHub’s documented temporary-keychain workflow is a useful starting point. [Signing on Mac runners](https://docs.github.com/en/actions/how-tos/deploy/deploy-to-third-party-platforms/sign-xcode-applications)

   An `xcodebuild` allowlist is not a sandbox: projects, package plugins, and tests execute code. A shared Mac user provides little isolation between tasks and accessible secrets. Restrict the first version to mutually trusted repositories; introduce isolated users or Mac VMs when trust differs. Queues and REAPI do not automatically remove this risk. [GitHub runner security](https://docs.github.com/en/actions/reference/security/secure-use)

For networking, prefer direct tailnet connections and avoid sending LAN builds through GCP. Tailscale supports direct, peer-relay, and DERP paths; use its connection diagnostics to verify which is active. GCP can host a peer relay when needed away from home. Large artifacts should remain downloadable separately from interactive logs. [Tailscale connection types](https://tailscale.com/docs/reference/connection-types)

Clarify “Tailscale SSH” in the proposal: its macOS server support requires the open-source `tailscaled` variant. Ordinary macOS OpenSSH over Tailscale is also suitable, with host verification and restricted credentials. [Tailscale SSH support](https://tailscale.com/docs/features/tailscale-ssh)

**Ship the shim, rsync snapshot boundary, persistent task caches, path-aware artifact return, and minimal supervisor first.** Defer full queue policy, Mutagen, shared compilation caches, richer idb interaction, and REAPI until measurements justify them. The acceptance exercise should cover dirty/untracked/deleted inputs, warm rebuilds, concurrent tasks, cancellation, dropped connections, mapped diagnostics, failed-test artifacts, and screenshot retrieval. The largest risks are inconsistent source capture, incomplete path semantics, shared Simulator/signing state, and Mac memory/disk saturation—not the SSH invocation itself.