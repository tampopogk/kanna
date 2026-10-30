# Build Targets

**Status:** built · **Tested:** `scripts/test-teardown.sh` · **Builds on:** [build speed](build-speed.md), [settings](settings.md) · **Why:** each checkout's `target/` grows to 10 GB, and 57 task worktrees filled the internal disk until a task stopped

Build outputs can live on another volume, such as an external SSD: every task worktree's, and all of the main checkout's except the app it runs. Sources stay where they are.

## Contents

1. [Where `target/` lives](#1-where-target-lives)
2. [Rules](#2-rules)
3. [Removing a task](#3-removing-a-task)

## 1. Where `target/` lives

Set `build.targets` in your `~/.kanna-v4/config.json` ([settings](settings.md#3-keys)) to a folder on that volume.

```json
{ "build": { "targets": "/Volumes/VHS/kanna-targets" } }
```

```mermaid
flowchart LR
    subgraph I["internal disk: ~/work/kanna-v4/target/"]
        A["Kanna.app<br/>(the app, daemon, keeper run here)"]
        G["ghosttykit/, ghostty-vt/<br/>(built Ghostty, 560 MB)"]
        O[".offline/release, .offline/kanna-build<br/>(dependencies, 850 MB)"]
        M["release, kanna-build, ghostty-source,<br/>ghosttykit-build, debug, … (links)"]
        W1["…/.kanna-worktrees/task-1dfb12c2/target (link)"]
    end
    subgraph V["/Volumes/VHS/kanna-targets/kanna-v4"]
        MM["_main/release, kanna-build, …"]
        T1["task-1dfb12c2/"]
    end
    M -->|symlink each| MM
    W1 -->|symlink| T1
    MM -->|"build-kanna-app.sh copies the binaries"| A
    MM -->|"rsync after each main build"| O
    MM -.->|"borrow: APFS clone"| T1
    G -.->|"worktrees link to it"| T1
```

| Drive | Main builds | A fresh worktree borrows |
|---|---|---|
| plugged in | on the volume; then `.offline` syncs (0.2s when dependencies didn't change) | main's `release/` by clone (0.2s) |
| unplugged | in real folders, `release/` and SwiftPM's packages cloned from `.offline` (0.3s): an incremental build, not a cold one | from `.offline`, by clone |

## 2. Rules

- **A worktree's whole `target/`** becomes a symlink to `ROOT/REPO/WORKTREE` (the main checkout's folder name, then the worktree's).
- **The main checkout keeps on the internal disk** `target/` itself, its `*.app` bundles, built Ghostty (`ghosttykit/`, `ghostty-vt/`) and `.offline/`; every other folder in it becomes a symlink to `ROOT/REPO/_main/NAME`, including ones a later build makes. `build-kanna-app.sh` already builds `target/Kanna.app` by copying the binaries out of `release/`, so the app, daemon and keeper always run from the internal disk, and unplugging the drive never takes the keeper and its terminals with it.
- **Built Ghostty stays internal** because rebuilding it takes its 3.1 GB source and a 2.6 GB Zig build, and every worktree links to it; it changes only with the pinned commit. The source and Zig cache live on the volume.
- **`.offline/` is a copy of main's dependencies:** after each main build with the volume, `kanna_targets_offline` syncs `release/` and SwiftPM's `repositories/`, `checkouts/` and `workspace-state.json` into it with `rsync -a --delete`. With the volume unplugged, main clones them back into real folders, and a worktree borrows from it when main's `release/` isn't there.
- **The `kanna` on your PATH** should be `target/Kanna.app/Contents/Helpers/kanna`, not `target/release/kanna`, which is on the volume.
- **Unset, nothing changes:** `target/` is a folder in each checkout.
- **Set by `kanna_targets`** in `scripts/env.sh`, which `build.sh` and each `build-*.sh` run first. It reads only your file (`$KANNA_AGENTS_HOME/.kanna-v4/config.json` when that's set): the drive belongs to this Mac, not the repo, so a repo's `.kanna/config.json` can't set it.
- **A missing volume falls back to the internal disk,** in the main checkout too. When the root is under `/Volumes/NAME` and `NAME` isn't mounted (absent, or an empty folder left on the internal disk), the build says so, removes the links that point at the missing volume, and builds in real folders. Nothing builds cold: Ghostty is already internal, and `release/` and the packages come from `.offline/`. A root that isn't an absolute path falls back the same way.
- **The next build with the volume moves them there.** A real folder moves over (`mv` to `DEST.moving`, rename, link), keeping what it built. If the volume still has an older copy from before it was unplugged, the moved one replaces it. A link to another root moves the same way.
- **Borrowing clones on the volume.** With main's `release/` on the same volume, `kanna_borrow_cargo` clones it (0.2s for 830 MB, against 6.5s to copy it from the internal disk), and the clone shares disk blocks with main's. It resolves main's link first, since `cp -R` copies a symlink named on its command line instead of following it. When the two sides are on different volumes (one fell back), `kanna_clone` copies with `cp -R -p`. A fresh worktree copying from the internal disk built in 31s (20.7s with everything internal, [build speed](build-speed.md#4-measured) case (a)); a no-op rebuild in 2s.
- **Everything reads it through the link:** cargo, SwiftPM (its `workspace-state.json` paths and module cache keep the worktree's `target/` path), Zig's `--cache-dir`s, `e2e-kanna.sh`, and `--bg` instances launched from the worktree's `target/Kanna.app`. Launch Services runs the app from the volume's real path, so `e2e-kanna.sh` stops an app by the state folder it's tied to, not by its path ([e2e cleanup](e2e-cleanup.md)). `.gitignore` ignores `/target` as well as `target/`, since git doesn't count a symlink as a folder.

## 3. Removing a task

`git worktree remove` and `rm -rf target` remove only the link, so Done's teardown removes the folder it points to. The repo's `.kanna/config.json` sets `tasks.teardown` to `scripts/teardown.sh` ([task scripts](task-scripts.md#3-teardown-and-cleanup)), which runs on every Done, also when the worktree is kept: the folder is a cache, and the next build makes it again.

```mermaid
flowchart TD
    A["target/ in the worktree"] -->|"a real folder, or none"| K["leave it: git worktree remove takes it"]
    A -->|a link| B{"build.targets set,<br/>its volume mounted?"}
    B -->|no| K2["leave it"]
    B -->|yes| C{"links to exactly<br/>ROOT/REPO/WORKTREE?"}
    C -->|"no: another worktree's, _main, .offline, elsewhere"| K2
    C -->|yes| D["rm -rf the folder (not a link there), then the link"]
```

- **Only the worktree's own folder:** the link must read exactly `ROOT/REPO/WORKTREE` as `kanna_targets` makes it (`ROOT` from your `build.targets`, `REPO` the main checkout's folder name, `WORKTREE` this worktree's), and that path must be a folder, not a link. It never follows anything else, and never touches the main checkout, `_main/`, `.offline/` or another worktree's folder.
- **Nothing to remove exits 0:** a real `target/`, no `target/`, `build.targets` unset, its volume unmounted, a link anywhere else, a folder already gone (then it removes the dangling link). A failed teardown keeps the task from being Done, so only a failed `rm` exits non-zero.
- **The tracked `.kanna/config.json`** carries it to every clone. It runs main's copy, `"$KANNA_REPO/scripts/teardown.sh"`, so a worktree made before the script existed is torn down too. `.gitignore` ignores the rest of `.kanna/`; your `planning.agent` lives in your own `~/.kanna-v4/config.json`.
- **Tested** by `scripts/test-teardown.sh`, against a fake `ROOT` and `KANNA_AGENTS_HOME`.
