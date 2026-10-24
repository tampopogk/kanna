# Fleet: Kanna Across Machines

**Status:** planned · **Parts:** [identity and pairing](fleet-identity.md), [tasks across machines](fleet-tasks.md), [build Macs](build-macs.md), [provisioning a server](fleet-provisioning.md), [agent sign-in](agent-sign-in.md) · **Builds on:** [architecture](architecture.md), [app link](app-link.md), [each app's Desk and Shelf](app-desk.md), [backlog runners](backlog-runners.md) · **Research:** [fleet](../research/fleet.md), [scenarios](../research/multi-machine-and-people.md), [xcodebuild shim](../research/xcodebuild-shim.md)

One person's Kanna runs on several machines: a MacBook Pro as the client, Linux servers that run tasks, Macs that build, and a relay for when the LAN isn't there. The same software later becomes **Kanna Cloud**, a cheap private cloud rented to users, who bring their own machines, rent Kanna's, or both. Kanna develops any project, not only Kanna.

The first milestone is the user as the only tenant on their own machines, built with the network and enrollment code a customer would use. Accounts, other tenants and billing come after it.

## Contents

1. [Rules](#1-rules)
2. [Machines and roles](#2-machines-and-roles)
3. [Reach: LAN first, relay second](#3-reach-lan-first-relay-second)
4. [Identity and pairing](#4-identity-and-pairing)
5. [The app link over the network](#5-the-app-link-over-the-network)
6. [One app, several machines](#6-one-app-several-machines)
7. [Terminals and panes](#7-terminals-and-panes)
8. [Where tasks run](#8-where-tasks-run)
9. [Server setup and updates](#9-server-setup-and-updates)
10. [Build order](#10-build-order)
11. [Rejected](#11-rejected)
12. [Glossary](#12-glossary)

## 1. Rules

- **You work on projects, not machines.** Kanna decides where a task's work runs (§2), and one task can use several machines at once. The app shows a machine only where it matters: a slow path, an offline machine, the fleet page.
- **LAN first.** Machines connect directly whenever they can. A relay is only for when you're away or the LAN fails, and the app shows which path each machine is on.
- **Free unless it needs infrastructure.** Whatever works on the LAN with no server of Kanna's is free, as long as being free doesn't make it worse. A MacBook and a mini pair with no account and get everything except reach from away. Trust never depends on an account.
- **Cloud is cheap, about $5 a month,** and covers only what needs Kanna's servers: reach from away, mobile access away from the LAN, push notifications, the always-on backlog copy, automatic enrollment.
- **End to end.** Relays and Kanna's own service carry only ciphertext and can't add a device. A machine that runs your agent sees your code, whoever operates it.
- **Every daemon can send and receive tasks**, Macs included, at a boundary: not started, or stopped at a turn's end.
- **Versions are compatible, not identical.** Daemons and apps of different releases work together over a range of protocol versions, so machines update on their own schedule.
- **Bring your own key.** Agents run on each user's own Claude or Codex subscription or API key. An account is added once and every machine can use it ([agent sign-in](agent-sign-in.md)).

## 2. Machines and roles

Every machine runs the same daemon and keeper. They differ in what each is for, advertised as **capabilities**: OS, architecture, cores, RAM, Xcode versions, simulators, display.

```mermaid
flowchart LR
    subgraph MBP["MacBook Pro · mbp"]
        APP["Kanna.app"] --- LD["daemon + keeper<br/>local projects"]
    end
    subgraph LAN["LAN"]
        subgraph R["r720-1 … r720-5 · Ubuntu 26.04 + Incus"]
            VM["your VMs<br/>daemon + keeper<br/>runners · repos"]
        end
        MACS["build Macs: mini + others<br/>Intel and Apple Silicon<br/>daemon · build jobs"]
    end
    GCP["GCP e2-micro<br/>iroh relay, static IP<br/>(later: Kanna Cloud)"]
    APP <==>|"direct"| VM & MACS
    VM ==>|"build jobs"| MACS
    APP -.->|"away: relayed"| GCP -.- VM
    classDef kanna fill:#2f6f4f,stroke:#6fcf97,color:#ffffff
    classDef os fill:#4a4a55,stroke:#9a9aa8,color:#ffffff
    class APP,LD,VM,MACS kanna
    class GCP os
```

| Machine | Role | Peer |
|---|---|---|
| `mbp` | the client; still a Kanna machine for local projects | yes |
| R720 hosts | run users' VMs (Incus), as many per user as wanted | no: each VM is a peer |
| build Macs | `xcodebuild` and Simulator jobs for Linux tasks; Mac tasks too | yes ([build Macs](build-macs.md)) |
| GCP e2-micro | an iroh relay at a stable IP; later Kanna Cloud's directory | no |

**Placement: where work goes.**

- **Placement is a policy, not a place.** By default, any machine with free slots and the capabilities a task `needs:` asks for the oldest waiting task, so a mini, ten VMs or a hundred share one backlog without a scheduler ([tasks §2](fleet-tasks.md#2-owners-and-grants)). Preferences narrow who asks (e2e on the mini, this project's agents on those VMs). A scheduler or load balancer that assigns tasks centrally can take over from asking without changing the rest.
- **Steps go where they can run.** A step that needs another machine's capability goes there as a **job**: `xcodebuild` through the shim, an e2e suite with `kanna build -- scripts/e2e.sh` on a Mac with a display, while Linux VMs do the agent work ([build Macs](build-macs.md)).
- **Taking a task to the Desk doesn't move it.** It stays where it runs and is shown over the link. To try a Mac or iOS app it built, the `.app` is fetched to the MBP ([build Macs §5](build-macs.md#5-ios-for-remote-tasks)).

## 3. Reach: LAN first, relay second

Networking is built into the daemon with **[iroh](https://github.com/n0-computer/iroh)** (Rust). An iroh endpoint *is* an Ed25519 key: you dial a key, not an address.

1. **On the LAN:** machines find each other with iroh's local discovery and connect directly at wire speed.
2. **Away, with a relay:** a relay is iroh's meeting point for hole punching. iroh connects directly through NAT when it can, and otherwise sends the traffic through the relay, which forwards QUIC it can't read.
3. **Away, with an address:** `kanna fleet addr NAME HOST` dials a peer at an address you give it, such as one on a VPN you already have. That's free, with no relay ([identity §3](fleet-identity.md#3-the-peer-log)).

**Local turns off iroh's defaults,** which use n0's public relays and DNS discovery. Kanna then depends on no third party's terms, and reach from away is either your own relay, your own address, or Cloud.

| | **Local** (free, no account) | **Cloud** (about $5 a month) |
|---|---|---|
| Discovery | LAN | Kanna's directory, anywhere |
| Away from the LAN | your own `iroh-relay` (the user's GCP e2-micro runs one) or an address you give (a VPN) | Kanna's relays |
| Pairing | a one-time code per new machine | automatic once you're signed in |
| Phone (after the first milestone) | on the LAN | anywhere, with push notifications |
| Backlog | replicated among your machines | plus an always-on encrypted copy |
| Remote Desks, terminals, panes, port tunnels, transfer, build Macs | all of it | the same, from away too |

**Most traffic never touches Kanna's servers:** iroh connects directly whenever it can, and moves a connection to a direct path by itself as soon as one exists, so using the LAN takes no action from the user.

Each machine's row in the app shows its path (direct on the LAN, direct over the internet, or relayed), so LAN-first can be seen, not just assumed.

## 4. Identity and pairing

In short ([detail](fleet-identity.md)):

- **The daemon has `device.key`**, an Ed25519 key that is its iroh endpoint and signs for the device. The app has its own endpoint key, certified by `device.key`. The CLI goes through the local daemon.
- **Pairing** is a PAKE over a one-time code, run directly between the two devices. On the LAN you type the code. In Cloud, provisioning puts the token in the new machine's setup, and the directory only relays the exchange, so it can't put its own key in.
- **The peers form a signed log**, replicated to every peer. A revocation beats any later add, and a name that comes back with another key is refused. Signing a peer takes a gesture in the app.

## 5. The app link over the network

A remote link is an iroh connection to the daemon's endpoint. Its first stream carries the [app link](app-link.md)'s JSON lines unchanged: snapshot, output, input, resize, layout, diffs, toasts, `debug.*`. Bulk transfers get **streams of their own** on the same connection (`file.read`, screenshot bytes, port tunnels, worktree syncs, transfers), so a keystroke never waits behind 20 MB.

```mermaid
sequenceDiagram
    participant A as Kanna.app (mbp)
    participant D as daemon on r720-1
    A->>D: connect (iroh: the app's endpoint key)
    A->>D: app.hello {pid, app, client: "mbp", cert, protocols: 3..5, commit}
    alt no protocol in common
        D-->>A: ok: false, other_protocol {mine: 1..2, commit}
        Note over A: "r720-1 is too old: it updates itself", and the daemon keeps running
    else in common
        D-->>A: ok, protocol: 4, daemon {machine, commit, capabilities}, panes, layout
    end
```

| | Local daemon | Remote daemon |
|---|---|---|
| Transport | unix socket | iroh, through `kanna-net` in `kanna-ffi` for the app |
| Version check | binary hash; another build is replaced ([daemon build](daemon-build.md)) | the highest protocol both support; `other_protocol` only when there is none |
| Who starts it | the app; on a Mac that serves the fleet, launchd | systemd (Linux), launchd (Macs) |
| Apps | one (`app_attached`) | one; lifting the limit is its own slice, needed before the phone |

- **One link per machine.** Every view is keyed by (machine, content ID), since `term-41` exists on every daemon.
- **The CLI:** `kanna --host NAME` (or `KANNA_HOST`) sends any command to that machine: the local daemon dials it and forwards the command. Inside a remote terminal nothing changes: the CLI there uses its own unix socket.
- **A Mac that serves the fleet** (a build Mac, or a Mac whose tasks you reach from elsewhere) runs its daemon from launchd (`kanna fleet serve`), pointed at the installed app's `Contents/Helpers/kanna-daemon`: a LaunchAgent on a Mac you use, and on a build Mac a LaunchDaemon running as its service user ([build Macs §1](build-macs.md#1-rules)). The app's replace rule still holds: on `other_build` the daemon exits, and launchd starts it again from the updated bundle.
- **Streams:** one ALPN, `kanna/1`. Each stream opens with a header line naming its kind (`control`, `file`, `port`, `sync`, `job`, `transfer`), and its bytes follow.

## 6. One app, several machines

- **A Desk is on a project,** whichever machines its tasks run on. A project is known across machines by its remote URL or first commit ([tasks §4](fleet-tasks.md#4-repos-and-credentials)). Its planning terminal runs on a machine that has the project, placed like a task, and its backlog page lists the project's tasks from every machine. The new-Desk prompt and Open Recent show projects, not paths on a machine.
- **The Shelf stays the app's:** claims in its order, three per project, keyed by (machine, task) in `app-state.json`. A card shows its machine only when that machine is relayed or offline.
- **Claims name the device:** `claimed by: mbp` (the hello's `client`) replaces `mac`. Runners leave claimed tasks alone, as today.
- **Toasts** held while no app was attached arrive when the app connects, so five servers' finished tasks show up when you open the lid.
- **What it looks like** (the fleet page with each machine's path and what it's running, a card on a relayed machine) is drawn in `references/mockups/fleet.html`, made before slice 3 (§10).

## 7. Terminals and panes

**Terminals keep the byte stream** of snapshot plus output. v2's lag came from its units, not the stream: whole scrollbacks re-sent on every reconnect, base64 through one Node loop, and a resize loop in the viewer ([research §2.5](../research/fleet.md#25-terminal-latency)).

- **Bounded snapshots:** `attach {scrollback: N}` sends the screen and a page; older scrollback comes on demand.
- **Reconnect by offset:** `output` carries a running byte offset, and `attach {from}` replays only the gap.
- **The 16 / 64 MiB limits stay**, and resizing stays the daemon's.
- **Local echo** (mosh-style prediction) is decided after slice 8 measures echo delay through the relay.

| Content | Remote task |
|---|---|
| Terminal, diff | over the link already |
| Markdown, code | through the daemon (task e044); images and relative links with `file.read` |
| Doc markup threads | `markup.*` through the daemon |
| `kanna screenshot` | bytes come back over a stream; the daemon writes the file where the agent asked |
| Web | **port tunnel:** the dev server stays on the server's loopback; the app listens on `127.0.0.1:LOCAL` and carries each connection over a stream. The origin stays localhost; bodies are never rewritten. |
| iOS | the MBP runs the Simulator from the `.app` a build Mac made ([build Macs](build-macs.md#5-ios-for-remote-tasks)) |

## 8. Where tasks run

In short ([detail](fleet-tasks.md)):

- **The backlog is replicated** to every one of your daemons, eventually consistent, ordered oldest first by a hybrid logical clock. Cloud adds an always-on encrypted copy.
- **Each task has one owner.** A runner elsewhere with free slots and the right capabilities asks for it, and the owner grants it to exactly one.
- **Transfer** moves a task between any two daemons at a boundary. The branch goes as a git bundle over the link, the transcript is copied, and the agent resumes by session ID at the receiver's own paths.

## 9. Server setup and updates

- **R720 host:** installed from bare metal through its iDRAC ([provisioning](fleet-provisioning.md); r720-1 runs Ubuntu 26.04.1). Then Incus, with users' VMs (Ubuntu 26.04) bridged onto the LAN, so peers reach them directly. A user can have any number, on one host or across several, and each is a peer like any machine: for capacity, for a project kept apart, or one per task when a task needs isolation. A VM never holds two users.
- **In the VM:** the user's account with `loginctl enable-linger`; git, build tools and Node for the agent CLIs; Kanna at `/opt/kanna/<commit>`, with `current` a symlink; state in `~/.local/share/kanna-prod/`. `scripts/server-setup.sh` does all of this and can be re-run safely. A VM starts from the stock Ubuntu cloud image, and setup installs the latest Kanna release, so there's no image per version.
- **The daemon unit:** the keeper stays the daemon's child, as on the Mac, so `KillMode=process`, or a restart kills every shell:

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

- **Enrolling** pairs the VM (§4), sets up its git credentials ([tasks §4](fleet-tasks.md#4-repos-and-credentials)), then gives it your agent accounts ([agent sign-in](agent-sign-in.md)), with nothing to sign in on the VM.
- **Updates:** `scripts/release.sh` also builds a Linux tarball (x86_64, `baseline` CPU for the R720s' Ivy Bridge). A daemon updates itself when a newer release is out (fetched from a peer that has it, or the release site): it switches `current`, runs `kanna-keeper --upgrade` ([keeper upgrade](keeper-upgrade.md)) and restarts; terminals survive ([restarting](daemon-restart.md)). `kanna fleet update` does it now, everywhere. No order is needed while versions overlap.

## 10. Build order

[Models first and implementation slices](fleet-build-order.md), including the mandatory shared build-Mac isolation model before slices 7 and 12.

## 11. Rejected

- **Tailscale or Headscale underneath:** customers would need a Tailscale account, or Kanna would run Headscale for them. Either way, sign-in and pairing, the product's front door, would be someone else's design.
- **A TCP listener with Kanna's own signed hello** (the research's plan): that relied on a WireGuard mesh for reach. iroh gives reach and encryption in one piece.
- **The local daemon proxying the app's remote links:** it would put the MBP's daemon in every keystroke's path and make one app link carry several daemons. The CLI goes through it only because a short-lived process shouldn't register with a relay.
- **n0's public relays by default:** free reach from away, but on a third party's terms and capacity.
- **A fixed `/kanna/repos` path:** it needs root on users' Macs, and agents resume by ID from any path anyway.

## 12. Glossary

| Term | Meaning |
|---|---|
| **Peer** | One machine's Kanna, known by its device key: the MBP, a user's VM, a build Mac |
| **iroh** | A Rust library for direct, encrypted QUIC connections between keys, with hole punching and relays |
| **ALPN** | Application-Layer Protocol Negotiation: the name a connection asks for when it opens (`kanna/1`), so both ends agree what protocol it speaks before any data flows |
| **Relay** | A server that forwards encrypted packets between peers that can't connect directly |
| **PAKE** | Password-authenticated key exchange: proves both sides typed the same short code, without sending it |
| **Job** | One step of a task (a build, an e2e suite) run on another machine that has the capability |
| **Owner** | The machine that starts a task's agent, so it never starts twice; it can grant that to a runner that asks |
| **Hybrid logical clock** | A timestamp that stays ordered across machines whose clocks drift |
| **Local / Cloud** | Kanna free with no account (LAN, your own relay) / with an account for about $5 a month (directory, Kanna's relays, phone away, push, backlog copy) |
