# Build Store

**Status:** built · **Tested:** `scripts/test-store.sh` (locking, publish, pruning, eviction against a fake `ROOT`), `scripts/test-teardown.sh` · **Builds on:** [build targets](build-targets.md), [build speed](build-speed.md) · **Why:** each task's build folder on `build.targets` grew to ~17 GB, and one merge-main round rebuilt ~40 of them, filling the external SSD to 98%

A task worktree's build folder holds only what that task builds itself: one Rust profile and its Swift build. Everything pinned (Zig, Ghostty's source, GhosttyKit, libghostty-vt) is built once per pin into a shared, read-only **store** that worktrees link to; Rust dependencies come from a shared compile cache; and build folders are caches that are evicted, least recently used first, when the volume runs low.

## Contents

1. [What goes where](#1-what-goes-where)
2. [The store](#2-the-store)
3. [One Rust profile per worktree](#3-one-rust-profile-per-worktree)
4. [The compile cache](#4-the-compile-cache)
5. [Eviction](#5-eviction)
6. [Measured](#6-measured)
7. [Rejected](#7-rejected)
8. [Glossary](#8-glossary)

## 1. What goes where

```mermaid
flowchart LR
    subgraph W["task worktree"]
        T["target (link)"]
    end
    subgraph V["build.targets (ROOT)"]
        subgraph F["kanna/task-1dfb12c2"]
            D["debug/ (the one Rust profile)"]
            K["kanna-build/ (SwiftPM), Kanna.app"]
            GK["ghosttykit (link)"]
            GV["ghostty-vt/HOST (link)"]
        end
        subgraph S["_shared (read-only)"]
            SK["ghosttykit/KEY"]
            SV["ghostty-vt/KEY"]
            SZ["zig/KEY, ghostty-source/KEY"]
            SC["sccache/"]
        end
        M["kanna/_main/ (release, …)"]
    end
    T --> F
    GK --> SK
    GV --> SV
    D -. "dependencies from" .-> SC
    SZ -. "build" .-> SK
    SZ -. "build" .-> SV
```

| Piece | Before, per worktree | Now |
|---|---|---|
| Zig toolchain, Ghostty source | 419 MB + 531 MB | `_shared/zig/KEY`, `_shared/ghostty-source/KEY`, once per pin |
| GhosttyKit, its Zig cache, libghostty-vt | 350 MB + 2.8 GB + 245 MB | `_shared/ghosttykit/KEY`, `_shared/ghostty-vt/KEY`, once per pin; the Zig cache is deleted after publish |
| Rust | `debug/` 3.6 GB **and** `release/` 4.4 GB | `debug/` only, deps optimised without debug info |
| Rust dependencies' compiles | per worktree | from `_shared/sccache` |

The main checkout keeps its `release/` profile and real, internal copies of GhosttyKit and libghostty-vt (cloned from the store), so its app still builds with the volume unplugged ([build targets](build-targets.md#2-rules)).

## 2. The store

- **Where:** `ROOT/_shared/NAME/KEY`, `ROOT` being `build.targets`. With `build.targets` unset or its volume unmounted, it's `MAIN/target/_shared` on the internal disk (`MAIN` the main checkout).
- **Names and keys:** a key is the first 16 hex digits of the SHA-256 of what decides the output, so a Ghostty bump (or a change to a build script) is a new key and one build, and an old key is never overwritten.

| Name | Key from | Holds |
|---|---|---|
| `zig` | Zig version, host OS and architecture | the toolchain |
| `ghostty-source` | fork URL, pinned commit, Zig version | a checkout at the commit, with every Zig package it can need fetched into `zig-pkg/` (the entry is read-only, so Zig can't fetch later) |
| `ghosttykit` | pinned commit, Zig version, `KANNA_GHOSTTY_OPTIMIZE`, `build-ghosttykit.sh` | `GhosttyKit.xcframework`, `ghostty.rev` |
| `ghostty-vt` | the same with `build-ghostty-vt.sh`, and the Rust triple | `include/`, `lib/libghostty-vt.a`, `ghostty.rev` |

```mermaid
sequenceDiagram
    participant A as task A's build
    participant B as task B's build
    participant S as _shared/ghosttykit
    A->>S: KEY exists? no
    A->>S: mkdir KEY.lock (pid A)
    B->>S: KEY exists? no
    B->>S: mkdir KEY.lock: taken by pid A (alive), wait
    A->>S: build into KEY.tmp-A, make its contents read-only
    A->>S: mv KEY.tmp-A KEY (atomic), chmod a-w KEY, rm KEY.lock
    B->>S: lock free, KEY exists: use it
    A->>A: ln -s _shared/ghosttykit/KEY target/ghosttykit
    B->>B: ln -s _shared/ghosttykit/KEY target/ghosttykit
```

- **Built once, under a lock:** `kanna_store NAME KEY BUILDER` in `scripts/store.sh` returns the entry, building it first if it's missing. The lock is a folder, `KEY.lock`, made with `mkdir` (atomic) and holding the builder's pid. Others wait for it, saying who they wait for, then look again. A lock whose pid is dead is stale and is taken over.
- **Atomic publish:** the builder writes `KEY.tmp-PID`, its contents are made read-only, one `mv` makes it `KEY`, then `KEY` itself is made read-only (macOS won't rename a folder you can't write). A failed build removes its temp folder and the lock and exits non-zero; nothing half-built is ever at `KEY`.
- **Worktrees link** `target/ghosttykit` and `target/ghostty-vt/TRIPLE` to their entries, and `touch` an entry each time they use it. The main checkout clones them into real folders instead, marked with the key. Zig and the source aren't linked: only the store's builders use them.
- **Read-only:** entries are `a-w`, so a build can't change what other worktrees use. A worktree that builds Ghostty from its own source (`GHOSTTY_SOURCE_DIR`) builds into its own `target/`, as before.
- **Pruned** in the eviction pass ([§5](#5-eviction)): an entry no link under `ROOT` points to and unused for a day is deleted, under its lock. `sccache/` is never pruned; it bounds itself.

## 3. One Rust profile per worktree

| Checkout | Profile | `cargo test` | `scripts/build.sh`'s app |
|---|---|---|---|
| task worktree | `dev` (`target/debug`) | same profile | same profile |
| main checkout | `release` | `dev` | `release` |
| `scripts/release.sh` | `release`, without the compile cache | | `release` |

- **`kanna_rust_profile`** in `scripts/env.sh` says which: `debug` in a worktree, `release` in the main checkout; `KANNA_RUST_PROFILE=release|debug` overrides it. `kanna_rust_dir` is its folder (`target/[TRIPLE/]PROFILE`), which `build.sh`, `build-kanna-app.sh` (as `KANNA_RUST_DIR` for SwiftPM), `e2e-kanna.sh` and `run-kanna.sh` use instead of `target/release`.
- **The `dev` profile is tuned** in `Cargo.toml` so one profile serves both: our crates at `opt-level = 1` with line tables, dependencies at `opt-level = 3` with no debug info. Debug assertions stay on, which the keeper's test hooks need.
- **`Package.swift`** links `libkanna_ffi.a` from `KANNA_RUST_DIR`, or else `target/debug` in a worktree and `target/release` in the main checkout, so `swift test` works in both.
- **Borrowing** ([build speed](build-speed.md#1-what-a-fresh-worktree-borrows)) clones the main checkout's folder of the same profile, when it has one.
- **Release builds stay clean:** `release.sh` sets `KANNA_RUST_PROFILE=release` and an empty `RUSTC_WRAPPER` (which overrides `.cargo/config.toml`), so every crate is compiled by `rustc` itself, and builds in the main checkout, whose Ghostty copies are real folders.

## 4. The compile cache

`.cargo/config.toml` sets `build.rustc-wrapper` to `scripts/rustc-wrapper.sh`, so every `cargo` run, scripted or not, goes through it:

```mermaid
flowchart TD
    A["rustc call"] --> B{"KANNA_SCCACHE=0, or<br/>no sccache on PATH?"}
    B -->|yes| R["rustc"]
    B -->|no| C{"sccache already failed<br/>for this cargo (marker)?"}
    C -->|yes| R
    C -->|no| D["sccache rustc …<br/>SCCACHE_DIR = store/sccache, port 4227"]
    D -->|"exit 2: sccache itself failed"| E["write the marker"] --> R
    D -->|"anything else"| X["its result"]
```

- **Why sccache:** it is the one maintained cache that shares Rust compiles between different target folders (by hashing a crate's inputs, not its path), it's already on this Mac (Homebrew), and it caches registry dependencies, which are most of a worktree's cold build. Cargo itself can't share a build across workspaces (`build-dir` only moves a workspace's own intermediates).
- **What it doesn't cache:** our own crates (incremental), binaries, proc-macros and build-script runs. Those are compiled per worktree.
- **Its own server and folder:** port 4227 (not the default 4226), `SCCACHE_DIR` the store's `sccache/`, `SCCACHE_CACHE_SIZE` 30G unless set. The server starts with the first compile and stops after 10 idle minutes.
- **Degrades to plain `rustc`:** with sccache missing, `KANNA_SCCACHE=0`, or sccache unable to run (a sandbox that forbids its server's socket exits 2 before compiling). The marker (`$TMPDIR/kanna-sccache-off.PID`, per cargo process) keeps a failing sccache from being tried once per crate. Builds are as fast as before, never broken.

## 5. Eviction

Run at the start of every build, by `kanna_targets` (`kanna_evict` in `scripts/store.sh`):

```mermaid
flowchart TD
    A["build starts"] --> P["prune _shared"]
    P --> B{"free on the volume<br/>below build.free (100 GB)?"}
    B -->|no| Z["build"]
    B -->|yes| C["candidates: ROOT/REPO/* except _main, .offline,<br/>_* and this worktree's"]
    C --> D["drop running tasks': a process's cwd in<br/>the worktree, or its command naming it"]
    D --> E["oldest .used first; folders of worktrees<br/>that no longer exist before all"]
    E --> F["rm -rf one, then check free again"]
    F --> B
```

- **Why at build start:** that's when space is used, on every build, not once per task. Teardown only ever frees the task's own folder, and only on Done. Setup runs once per worktree, so a long task that builds a lot never triggers it. The daemon doesn't know a repo's build layout ([task scripts](task-scripts.md#5-rejected)); this repo's scripts do. The check is one `df` when there's room.
- **Running** means some process has its working folder in the worktree, or names the worktree or its build folder in its command line (an agent, a terminal, a `--bg` app). A stopped task has no processes: its terminals are hung up. If `lsof` or `ps` fails, nothing is evicted.
- **Last used** is `ROOT/REPO/WORKTREE/.used`, touched by every build.
- **Never evicted:** `_main`, `.offline`, `_shared` (only pruned), the building worktree's own folder, a running task's.
- **One at a time:** eviction holds `ROOT/_shared/.evict.lock`; a build that finds it taken skips eviction.
- **An evicted worktree rebuilds** on its next build: its `target/` link is left dangling, and `kanna_targets` makes the folder again.
- **`build.free`** in your `~/.kanna-v4/config.json` sets the threshold in GB ([settings](settings.md#3-keys)).

## 6. Measured

Fresh worktrees of main, built with `scripts/build.sh` then `cargo test --workspace --no-run`, on the external SSD:

| | Before | After |
|---|---|---|
| A task's build folder | MEASURE | MEASURE |
| A clean task's build (`build.sh` + tests) | MEASURE | MEASURE |
| A second task's build | MEASURE | MEASURE |
| A Ghostty bump, first task / others | each worktree builds its own | MEASURE / links |

## 7. Rejected

- **A shared `CARGO_TARGET_DIR`:** one cargo lock for every worktree serialises their builds, and their different sources keep invalidating each other's fingerprints.
- **Keeping both profiles, only sharing Ghostty:** `release/` and `debug/` are each 4 GB; tests can't run in `release` because `cargo test` uses `dev` unless every agent remembers `--release`, and the keeper's test hooks need debug assertions.
- **Eviction in the daemon:** it would have to know this repo's `build.targets` layout.
- **Hard links instead of symlinks into the store:** a folder can't be hard-linked, and a link shows where an entry came from.

## 8. Glossary

| Term | Meaning |
|---|---|
| **Store** | `ROOT/_shared`: pinned builds, one per key |
| **Key** | A hash of what decides an entry's output |
| **Entry** | One `NAME/KEY` folder in the store |
| **Profile** | Cargo's build settings and output folder: `dev` → `target/debug`, `release` → `target/release` |
| **sccache** | Mozilla's compiler cache, run as `rustc`'s wrapper |
