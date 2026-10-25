# Machines: Setting Up a Linux Machine

**Status:** built in slice 1 (`scripts/server-setup.sh`), updates in slice 6 · **Part of:** [your machines](machines.md) · **Used by:** [Kanna Cloud's provisioning](kanna-cloud.md#3-provisioning)

How any Linux machine becomes one of your machines: a box of your own, or a VM Kanna Cloud makes for you. The script and the unit are the same in both; only who runs them differs.

## Contents

1. [The machine](#1-the-machine)
2. [The daemon unit](#2-the-daemon-unit)
3. [Running the setup](#3-running-the-setup)
4. [Joining and updates](#4-joining-and-updates)

## 1. The machine

- **The user's account** with `loginctl enable-linger`; git, build tools and Node for the agent CLIs; Kanna at `/opt/kanna/<commit>`, with `current` a symlink; state in `~/.local/share/kanna-prod/`. `scripts/server-setup.sh` does all of this and can be re-run safely.
- **Stock Ubuntu (26.04):** setup installs the latest Kanna release on a stock image, so there's no image per version.

## 2. The daemon unit

The keeper stays the daemon's child, as on the Mac, so `KillMode=process`, or a restart kills every shell:

```ini
# ~/.config/systemd/user/kanna-daemon.service
[Service]
Environment=KANNA_SOCKET=%h/.local/share/kanna-prod/kanna.sock
Environment=KANNA_KEEPER=/opt/kanna/current/kanna-keeper
ExecStart=/opt/kanna/current/kanna-daemon
Restart=always
KillMode=process
[Install]
WantedBy=default.target
```

## 3. Running the setup

As the user Kanna runs as (with sudo), until releases have a Linux tarball (slice 6):

```sh
sudo apt-get install -y git       # a stock cloud image has none
git clone https://github.com/tampopogk/kanna.git ~/kanna && cd ~/kanna
scripts/server-setup.sh            # --agents claude,codex (default) | none; --no-packages; --no-start
claude                             # sign in once, until agent sign-in
kanna task list                    # ~/.local/bin/kanna, on PATH from the next login
```

1. **Packages** with apt (git, build tools, sqlite3, jq, Node and npm, and bubblewrap and socat for the agents' sandboxes: Ubuntu's AppArmor blocks the user namespaces a bundled `bwrap` needs, and only the system `/usr/bin/bwrap` has a profile allowing them), Rust with rustup if `cargo` is missing, and the agent CLIs with npm into `~/.local`.
2. **Builds** from the checkout: `scripts/build-ghostty-vt.sh` (on Linux always `-Dcpu=baseline`; Zig downloads for Linux too), then `KANNA_CHANNEL=production cargo build --release -p kanna -p kanna-daemon -p kanna-keeper`, so the state folder is `kanna-prod`.
3. **Installs** the three binaries, `planner.sh` (a remote Desk's planner runs it, [links §4](machine-links.md#4-projects-desks-and-claims)) and Kanna's skills (`kanna-skills/`, which the Mac has in the app's `Resources`) in `/opt/kanna/<commit>` (`<commit>-dirty` for local changes), owned by the user so a daemon can update itself later; switches `current` with a rename, pointing `previous` at the one it replaced, and removes the others; an unchanged build isn't copied again; links `~/.local/bin/kanna`.
4. **The unit** (§2), with the socket in the state folder (`$XDG_DATA_HOME` if set), plus `KANNA_AGENT_SKILLS` and a `PATH` with `~/.local/bin`, where npm puts the agent CLIs, so the daemon and every terminal it starts find them without a login shell. Ubuntu's `~/.profile` adds `~/.local/bin` only to login shells, so a plain `ssh host kanna …` needs `~/.local/bin/kanna` (or `ssh -t host bash -l`); `loginctl enable-linger`; enabled, and restarted unless the running daemon is `current`'s binary and the unit is unchanged. It's started when it answers a request, not when its socket appears. The keeper survives the restart and is upgraded in place by the new daemon ([keeper upgrade](keeper-upgrade.md)), so terminals and agents keep running.
5. **`kanna agents setup`** when the agents aren't set up yet (not when you said no), since no planning terminal is there to ask.

The daemon logs to the journal (`journalctl --user -u kanna-daemon`).

**Tested** on r720-1 (Ubuntu 26.04.1 on the host, x86_64; a first run takes about 5 minutes) and in an Ubuntu 26.04 lima VM on an Apple Silicon Mac (arm64, and x86_64 cross-built with `gcc-x86-64-linux-gnu` and `libc6-dev-amd64-cross`, run under Rosetta): a backlog runner starts a task in a worktree, `kanna task start` and `stop` work, real claude tasks commit their work and are stopped when their turn ends (claude asks once per machine to trust the project and to accept bypass mode), a working claude carries on through a daemon restart, a terminal's process and screen survive `systemctl --user restart kanna-daemon` and each update, and re-runs restart the daemon only when its binary or unit changed (A → B → B keeps A as `previous`; a dirty rebuild installed with `--no-start` is started by the next run). r720-1 is Kanna Cloud's first host; here it only stood in for a Linux machine of the user's.

## 4. Joining and updates

- **Joining:** you pair the machine by code ([identity §2](machine-identity.md#2-pairing)), set up its git credentials ([tasks §4](machine-tasks.md#4-repos-and-credentials)), then give it your agent accounts ([agent sign-in](agent-sign-in.md)). A Kanna Cloud VM joins by token instead ([Cloud §4](kanna-cloud.md#4-enrollment-by-token)).
- **Updates:** `scripts/release.sh` also builds a Linux tarball (x86_64, `baseline` CPU, so older Xeons such as the R720s' Ivy Bridge run it). A daemon updates itself when a newer release is out (fetched from a peer that has it, or the release site): it switches `current`, runs `kanna-keeper --upgrade` ([keeper upgrade](keeper-upgrade.md)) and restarts; terminals survive ([restarting](daemon-restart.md)). `kanna fleet update` does it now, everywhere. No order is needed while versions overlap.
