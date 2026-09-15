# Linux Bazel product and package boundary

Task `25c25a1b`, based on `ac2604a6e092771968f7f9096d4c4b917ad036b1`.
Implementation commit: `bd5fa5a64c6e33bfe4b96f5a6b8a0392b8253ccd`.
The evidence-only follow-up changes no product input.

This slice implements product/package ownership. It grants no publication
permission and does not complete the Linux release.

## Build surface

`./kd build linux-package --channel staging --architecture arm64` (or
`x86_64`) builds the corresponding declared Bazel package. The graph declares execution support
for Apple Silicon macOS, Linux ARM64 and Linux x86-64, using the existing
pinned Zig/Rust/Noble sysroot graph. No Cargo fallback is used.

The labels are `//packaging/linux:products_{production,staging}_{arm64,x86_64}`
and `//packaging/linux:deb_{production,staging}_{arm64,x86_64}`. Each product
collection transitions the desktop, worker and all six sidecars to its explicit
Linux platform. Package targets produce a `.deb` and JSON audit report. `kd`
queries those outputs, verifies their identity/hash, and copies them to
`.build/linux-package/out/` using the existing Debian filename convention.

`VERSION` supplies the package and desktop version. `--version` must agree with
it; a caller must stamp source before building a different version.
`--staging-iteration N` maps to the Bazel integer setting
`--//packaging/linux:staging_iteration=N`. `--skip-build` requires Bazel's
up-to-date check; an old output directory does not satisfy it.
`--allow-audit-findings` is rejected on this path. The retained direct assembler
is explicitly marked `builder: prototype` and cannot certify Bazel provenance.

## Integration changes

- The worker owns a BUILD target in the existing server dependency universe;
  its server-process library uses that same universe so Tokio types agree.
  Its crate version matches Cargo for the server bootstrap configuration.
- Tauri's execution helpers support Linux execution hosts and take the target
  triple independently of their execution platform. Linux context/config
  generation uses the requested channel and `VERSION`; Darwin keeps its
  existing config path.
- Ghostty's Linux archive merge invokes the pinned Zig archiver. Its build
  always specifies the approved glibc target, including on native Linux.
  Each native action creates private Zig caches and links the pinned SDK's
  `libc++.a` and `libc++abi.a`. The Mac SDK wrapper does not inject the Darwin
  sysroot into a Linux-targeted Zig build. Unused Ghostty shared outputs are
  discarded before Rust consumes the static build outputs.
- The full server link exposed an OpenSSL archive-index integration defect:
  host Apple `ranlib` had reduced the vendored ELF archives to empty 96-byte
  archives even though the dependency build action succeeded. Linux OpenSSL
  now indexes through the declared target archiver (`zig ar s`).
- Package assembly reuses `linux-package.ts`'s audited installation layout,
  built-in resources, icons, launcher, control metadata and maintainer scripts.
  An execution-platform Python tool reads ELF facts and writes Debian ar/gzip/
  tar containers. Target executables are never executed on the cross builder.
  Ownership, archive ordering and timestamps are fixed; host `dpkg`, `ar`,
  `readelf`, compressor and development libraries are not assembly inputs.
- The existing runtime policy derives `Depends`. In addition, the product
  package gate requires the matching interpreter and refuses direct dynamic
  libc++/libc++abi/libunwind dependencies. Its report contains every executable's
  measured closure and hash, plus the final package hash.
- The existing hosted Ubuntu lanes now build these targets and retain their
  audit reports alongside the packages. Install-only checks still exercise
  the real systemd user manager and installed worker. C++ checks now assert
  static closure instead of requiring the old prototype's dynamic exceptions.

## Verification

Executed locally on Darwin ARM64 with Bazel 9.0.1 and the repository's pinned
Rust/Zig/sysroot graph. Both final artifacts were collected by the canonical
commands below, including the worker's Cargo-compatible crate version:

```sh
./kd build linux-package --channel staging --architecture arm64
./kd build linux-package --channel staging --architecture x86_64
```

Both are `kanna-staging` version **`0.2.0~staging.1-1`**, built from the
implementation commit above. These are local validation artifacts, not a
published release candidate. They remain in `.build/linux-package/out/` in this
worktree, with adjacent `.deb.json` reports.

| Debian architecture | Bytes | Package SHA-256 |
| --- | ---: | --- |
| arm64 | 43,735,536 | `297f2c164b0431b0d1cc19208b0039d7dd54bf9763e3efdb4bb6cbd5efe077de` |
| amd64 | 44,438,078 | `b19f7992045de085eca527befb1370fa21c25903ae226370392a7a94515a1abd` |

Committed measured reports contain the individual hashes and ELF facts for all
16 executables: [ARM64](evidence/2026-09-14-linux-products/arm64.json) and
[x86-64](evidence/2026-09-14-linux-products/x86_64.json). Both have zero findings,
zero conditional dependency exceptions and no RPATH/RUNPATH. Every executable
has its architecture's approved loader. Only the desktop directly needs the
GTK/WebKit stack; the seven headless executables need glibc/libm only. All GLIBC
requirements are at or below 2.39. There is no direct dynamic Ghostty, OpenSSL,
libc++, libc++abi or libunwind dependency.

- Focused `kd` suites: **76 tests pass** across Linux release build, package
  layout, ELF audit, CI workflow and release Cargo locks. `kd` TypeScript
  checking passes.
- Both canonical `--skip-build` repeat collections pass Bazel freshness checks
  with zero build actions and preserve the package hashes. Collection replaces
  read-only Bazel copies atomically; that real repeat-run failure now has a
  regression test.
- `//packaging/linux:artifact_tool_test`: **3 Python tests pass** under Bazel's
  pinned execution runtime (ELF parsing and deterministic archive metadata).
- Native macOS ARM64 regression builds pass for
  `//apps/desktop/src-tauri:full_staging_context_rust_support` and
  `//crates/daemon:kanna_daemon`. `otool -L` on that daemon lists only Apple
  system libraries/frameworks. No macOS bundle, signing, promotion or GUI
  acceptance was performed.
- Independent inspection uses Apple's libarchive and LLVM `objdump` to compare
  the actual packaged executable bytes and ELF facts against the reports.
  All 16 hashes, direct dependencies and GLIBC versions match. Archive ownership,
  modes and timestamps and the installed launcher symlink are checked.
- Extracting each package, changing executable timestamps, and reassembling
  with the pinned Python tool produces byte-identical output for **both**
  architectures. This proves deterministic **assembly from the same inputs**,
  not independent full-build reproducibility across hosts.
- The broader CLI suite has one unrelated existing failure at
  `tools/kd/tests/cli.test.ts:1725`: cloud deploy parsing returns `dryRun: false`
  beyond the assertion's expected object. It was left unchanged. The focused
  Linux CLI selection passes (17 tests, 33 unrelated tests excluded).

Local logs, independent inspection outputs, extracted verification trees and
reassembled archives are preserved under `.tmp/linux-product/`. The canonical
package outputs and reports remain under `.build/linux-package/out/`; the
declared Bazel outputs remain under `bazel-bin/packaging/linux/`.

No native Linux execution, installed result, new hosted CI run, two-version
upgrade proof or release eligibility is claimed by these local cross builds.
The older run 34878212138 remains valid **prototype** install-only evidence; it
does not validate these new Bazel products.

## Remaining integration

Ship task `d3ce8dec` retains canonical Linux status/ship/promote/channel/lineage
routing, source/candidate provenance, archive storage and the final
release/download handoff. The generic
release `--platform` parser is not a Linux publisher; do not route Linux through
the existing macOS release handler.

The hosted install lane requires this reviewed source to reach GitHub CI.
This task has no permission to push a branch or open a PR. A local cross-built
artifact is not hosted native-build or installed evidence.

There is no real two-version Bazel package pair yet. This slice does not relabel
one build as two versions or substitute the historical Cargo prototype as a
supported predecessor. The existing `kd test linux-installed --old-artifact …
--new-artifact … --channel staging` lane remains the upgrade/active-session
boundary once distinct source/version packages exist. Updating the historical
previous-daemon fixture still requires a real Linux-capable release tag.

Task `641dbb6f` owns system/cross-machine and transfer acceptance. Final Linux
candidate acceptance and the owner's full 24-hour Linux candidate soak remain
mandatory before production. No macOS soak, package build, test-key apt result,
or this document substitutes for those gates. No publication, signing-key
provisioning, remote apt mutation or website download link belongs to this slice.
