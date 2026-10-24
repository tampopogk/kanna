# Restarting the Daemon

**Status:** built ([plan](../plans/mux-build.md) M2) · **Builds on:** [architecture](architecture.md), [the keeper](../keeper.md) · **Related:** [daemon build](daemon-build.md), [upgrading the keeper](keeper-upgrade.md)

The daemon can stop, crash or be upgraded without losing a terminal. The keeper holds every pty, so the programs keep running and their output waits in the pty's kernel buffer; the daemon keeps its terminals on disk, so the next one knows their IDs, names and screens. On start it takes every pty back from the keeper (`adopt`) and rebuilds its terminals from both.

## Contents

1. [Files on disk](#1-files-on-disk)
2. [Stopping](#2-stopping)
3. [Starting](#3-starting)
4. [Rules](#4-rules)
5. [Checked by](#5-checked-by)

## 1. Files on disk

In `$KANNA_STATE_DIR/terminals/` (0700; a symlink there is refused), each written atomically (a new file, then renamed), mode 0600:

| File | What it holds | Written |
|---|---|---|
| `terminals.json` | `{kanna: "terminals", version: 3, counters: {pane, terminal}, terminals: [{id, pane, name, type, cmd, cwd, state, exitCode, pid, keeper, cols, rows}]}`: each terminal's record (`id` its terminal ID, `term-41`; `pane` the same ID, the pane that shows it; `state` `running` or `exited`; `keeper` the id the keeper knows its pty by; `cols` and `rows` the size it last had) and the pane and terminal counters. A version 1 or 2 file (panes `pN`) is rewritten once at start as version 3: each terminal's pane takes its terminal's ID (`term-N` for a version 1 file's pane `pN`, whose screen file is renamed to match) | on every change (open, close, restart, start, exit), from the daemon's main loop; an exit before the daemon acknowledges it to the keeper |
| `<terminal>.snap` | the terminal's screen as a GHOSTSNP snapshot, as `attach` sends it | by a saver thread, at most once a second per terminal and only after its screen changed; all of them on a clean stop |
| `settings.json` | the app's last terminal `settings` ([terminal](terminal.md#2-the-config)) | when the app sends them |

- **Snapshots** are taken under the lock the pty reader feeds the screen with, without touching what the app is sent. If one can't be encoded (as on [attach](app-link.md#3-rules)), or the write fails, the last good file stays and the saver tries again a second later.
- **Bounded:** a snapshot file is at most 16 MiB (one over isn't written), `terminals.json` is refused past 4 MiB when read; one snapshot per terminal; the keeper holds at most 256 ptys. Snapshots of terminals that are gone are deleted when they close, and at start.
- **A file it can't trust** (bad JSON, another version, two terminals with one ID, keeper id or name) is moved aside as `terminals.json.bad`, and the daemon starts as if it had none (it still takes back every pty the keeper has).
- **One daemon per folder:** the daemon holds `flock` on `daemon.lock` while it runs; a second waits 5 s for it, then exits 1 ("another daemon (pid …) uses …"). Its keeper is that folder's only, so no daemon ever adopts or closes another's terminals.

## 2. Stopping

On SIGTERM, SIGINT or SIGHUP the daemon stops reading every pty (whatever is unread stays in the kernel buffer for the next daemon), writes `terminals.json`, saves every screen that changed since its last save, and exits; its terminals keep running in the keeper. The signal also sets `alarm(5)`, so a daemon whose main loop is stuck still ends 5 s later, as a crash. A crash (SIGKILL) skips that: up to a second of screen changes are lost from the saved screens, and a full-screen program redraws them when it gets SIGWINCH. To stop the terminals and the keeper too, use `kanna instances stop` ([stopping an instance](instance-stop.md)).

## 3. Starting

The daemon connects to the keeper (pending exits arrive as events), adopts every pty it holds, and matches them with `terminals.json` by keeper id:

```mermaid
sequenceDiagram
    participant D1 as old daemon
    participant K as keeper
    participant S as shell
    participant D2 as new daemon
    participant A as Kanna app
    Note over D1: SIGTERM: stops reading, saves terminals.json and the screens<br/>(SIGKILL: what it saved up to a second before)
    D1--xA: link closes; the app keeps trying to reconnect
    S->>S: output waits in the pty buffer
    D2->>D2: flock daemon.lock
    D2->>K: connect, hello (unacknowledged exits arrive as events)
    D2->>K: adopt
    K->>D2: each pty: {id, pid, cmd, cwd, exit?} + master
    D2->>D2: read terminals.json: same IDs, names, cmd, cwd; counter continues
    D2->>D2: each screen from <terminal>.snap (if the pid matches), sized to the pty (TIOCGWINSZ)
    D2->>S: TIOCSWINSZ, then SIGWINCH to the pty's foreground group
    D2->>S: reads what waited in the buffer into the screen, then carries on
    D2->>K: ack exits that happened meanwhile (after saving them)
    A->>D2: app.hello
    D2->>A: ok, panes (the same IDs: the saved layout still matches)
    A->>D2: attach term-1
    D2->>A: snapshot term-1, grid term-1, then output
```

| The keeper has the pty | `terminals.json` has it | The daemon |
|---|---|---|
| yes, running | yes | takes it back: same ID, name, command, pid; its saved screen (blank if the file is missing or unreadable, or the record's pid isn't the pty's); the waiting output; SIGWINCH |
| yes, exited meanwhile (unacknowledged) | yes | as above, then reads its last output, records the exit code, acknowledges it: `list` shows `exited` with the program's own code |
| yes | no (a crash before the record was written, or the file lost) | **takes it back** with a blank screen, no name, its command (from the keeper's `/bin/sh -c CMD`, else none: a shell) and cwd, under the keeper's id if that's a pane ID not in use, else a new one; the app places it as a new pane |
| no | yes, `exited` | keeps it as an exited terminal with its saved screen and exit code, until it's closed or restarted |
| no | yes, `running` | drops it (its program ended and the keeper lost its exit: the keeper stopped, or idled out) and logs it |

## 4. Rules

- **Unknown ptys are adopted, not closed:** a pty the keeper holds is a program the user started (perhaps an agent mid-task); closing it would lose work. It costs only a pane without a name, which the user can close.
- **Pane and terminal IDs never repeat:** each counter continues from the larger of the saved counter and every ID on disk and in the keeper. A terminal started again gets a new terminal ID. Only losing `terminals.json` while the keeper has also lost the newest terminals can bring an old ID back.
- **Exit codes** come from the keeper, the process's parent, so an exit while no daemon runs keeps its code (`exit 7` → `exitCode: 7` after the restart). On macOS a program can't finish exiting while its last output is unread; the new daemon reads it, and the exit arrives then. That holds for `sh` and `bash`; zsh exits without waiting, so output a zsh program wrote just before exiting while no daemon ran is lost (its exit code is not).
- **Sizes:** the pty keeps the size the last daemon set; the new daemon sizes each screen to it and sets it again, and the app resizes once it attaches. A terminal started again after the restart (`restart`, a task resumed) starts at the size in its record, even if its screen's file is gone ([pane sizes](pane-sizes.md)).
- **Upgrades:** a relaunched app of another build makes the daemon stop as on SIGTERM and starts its own, which takes everything back as above; a rebuild that didn't change the daemon keeps it running ([daemon build](daemon-build.md)).
- **The app** sees its link close, keeps trying to reconnect, and on reconnecting says hello and attaches again ([app link](app-link.md)); the hello reply lists the same IDs, so its layout still matches and nothing moves ([layout](layout.md#3-saving-and-restoring)).

## 5. Checked by

`crates/kanna-daemon/tests/restart.rs` (a running shell keeps its pid and screen, output written while the daemon was down appears, and the app gets all of it on attach; a full-screen program redraws once after SIGWINCH; exits while the daemon is down, with their codes; a SIGKILLed daemon's screens; IDs don't repeat; a pty the disk doesn't know is taken back and a record the keeper doesn't know dropped; a second daemon on the folder is refused; an app of another build stops the daemon and the next keeps its terminals; a terminal started again keeps its size without its screen), the store's unit tests, and `daemon_restart` (with the app attached: same PIDs, screen and layout, output written meanwhile, the app's view), `daemon_upgrade` and `pane_sizes` (programs on the Desk, the Shelf and in the backlog see only the Desk's size across a relaunch and a restart) of `scripts/e2e-kanna.sh`.
