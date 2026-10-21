# The Daemon Follows the App's Build

**Status:** built · **Builds on:** [the app link](app-link.md), [restarting the daemon](daemon-restart.md) · **Why:** quitting the app leaves the daemon running, so a rebuilt app used to attach to the old daemon and run against its old commands until someone killed it by hand

A relaunched app whose bundled daemon differs from the one running replaces it. Nothing is lost: the keeper holds the ptys, and the next daemon adopts them.

**Only the local daemon follows the app's build.** A daemon on another machine is checked by protocol number and never stopped by an app ([fleet](fleet.md#5-the-app-link-over-the-network)).

## 1. The build ID

The **build ID** is the SHA-256 of the `kanna-daemon` binary, in hex. Three processes work out a build ID, each from a different file, and compare theirs with the daemon's:

| Process | Hashes, for its build ID | When |
|---|---|---|
| the daemon | its own executable | at start, before a rebuild can replace the file |
| the app | `Contents/Helpers/kanna-daemon`, the daemon it would start | once, at its first hello |
| the CLI | the `kanna-daemon` beside it (in the bundle, or `target/release`) | only when a command fails |

- **Why the binary and not the version and commit:** a dev rebuild with uncommitted changes has the same commit and different code, and a commit that doesn't touch the daemon needn't replace it. The hash changes exactly when the daemon does; an app-only rebuild keeps its daemon.
- **Why not a stamp per build:** every rebuild would replace a daemon that hadn't changed.

## 2. Hello from another build

```mermaid
sequenceDiagram
    participant A as rebuilt app
    participant D1 as old daemon
    participant K as keeper
    participant D2 as new daemon
    A->>D1: app.hello {pid, build: B2, panes}
    Note over D1: no app attached, its build is B1
    D1->>A: ok: false, other_build
    Note over D1: stops as on SIGTERM:<br/>saves terminals.json and the screens
    D1--xA: link closes
    Note over A: no 2 s grace: starts Helpers/kanna-daemon<br/>as soon as nothing answers
    A->>D2: spawn (waits for daemon.lock)
    D2->>K: adopt (same ptys, same pids)
    A->>D2: app.hello {build: B2}
    D2->>A: ok, daemon {pid, build: B2}, panes (the same IDs)
```

- **Only with no app attached.** A second app gets `app_attached` as before, whatever its build, so two running apps of different builds can't take turns stopping each other's daemon.
- **A hello without `build`** (an older app, the test apps) is accepted, as before.
- **The app** answers `other_build` by retrying at once rather than backing off, and starts its daemon without the grace it gives a daemon restarted on purpose; at most one start every 10 s still holds, so an app whose bundle changed under it can't loop.

## 3. The CLI meets another build

The CLI never stops a daemon: the app attached to it may be the old build too. It talks to it as before, and when a command fails it asks the daemon its build (`capabilities` answers `build`) and, if it differs from its own, adds a line:

```text
$ kanna send nosuch x
kanna: no pane "nosuch" [not_found]
(the daemon on this socket is from another build than this kanna (a4cb40a16617 here, c9e6b91e5a5d there); quit and relaunch Kanna.app to replace it)
```

## 4. Checked by

`crates/kanna-daemon/tests/restart.rs` (a hello of the same build is accepted with the build in the reply; one of another build is refused with `other_build`, the daemon stops, the shell lives, and the next daemon has it with the same ID and pid), `crates/kanna-build` (the ID is the binary's SHA-256), and `daemon_upgrade` of `scripts/e2e-kanna.sh`: a daemon re-signed as an older build, a terminal opened on it, the CLI's note; the app launched, the old daemon stops, the app's daemon takes over with the terminal's pid and screen, and the note is gone.
