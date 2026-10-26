# Machines Build Order

**Part of:** [your machines](machines.md) and [Kanna Cloud](kanna-cloud.md) · **Build isolation:** [shared build Macs](build-mac-isolation.md)

Each slice is one agent's task and leaves `main` releasable. **Your machines** come first, then **Kanna Cloud**. Slice numbers are the tasks' names (`Fleet 7`, from before the rename), not the order. The tasks' first messages predate the decisions in these specs and their part specs; where a slice below differs from its task, this spec wins (**Changed**).

**Test rig:** two daemons are two instances on one Mac ([instances](instances.md)), or an instance on this Mac and a Linux VM: a lima VM, or an Incus VM on r720-1 standing in for a Linux machine of the user's. r720-1 is Kanna Cloud's first host (Ubuntu 26.04.1).

**Models first.** Each risky part gets a model before its slice: a throwaway prototype in `references/prototypes/<name>/` or a mockup in `references/mockups/`. Its slice starts once the model shows the risk is handled, and its tests then compare the product with the model. Each model is its own task.

## Your machines

The first milestone: your own machines, LAN first, your own relay or a fixed address. One tenant, so no tenant isolation; it neither includes nor depends on build Macs, a Cloud feature.

| Risk | Model | Shows | Before slices |
|---|---|---|---|
| iroh's reach and speed | [fleet-net](../../references/prototypes/fleet-net/README.md): two endpoints in Rust, LAN discovery, a relay on the e2-micro, `kanna/1` streams of each kind | a direct path on the LAN, hole punching from away, relay fallback; echo delay and throughput for each, the standard slice 8 is held to | 2, 3, 8 |
| Pairing and the peer log | `fleet-pairing`: Python, a PAKE over a code, a signed log between three simulated devices | a forged key refused; mutual revocation; a revoked key never re-added | 2 |
| Desks across machines | `references/mockups/fleet.html` | a Desk per project with tasks on several machines; the Machines tab; a card on a relayed or offline machine | 3 |
| The replicated backlog | `fleet-backlog`: Python, N simulated daemons with partitions | the store chosen; two runners asking at once; an owner going away; a scheduler taking over | 10 |
| Transfer | `fleet-transfer`: a script between two home folders | bundle, `reset --soft`, the transcript moved, Claude and Codex carrying on | 11 |
| Shared agent accounts | `agent-sign-in`: probes on two VMs | one setup token on many machines at once; Codex on access tokens from a holder | S1 |

| # | Task | Slice | After |
|---|---|---|---|
| 1 | Fleet 1: Kanna on Linux, headless | **Built.** Daemon, keeper and CLI on Linux x86_64; `server-setup.sh` and the unit ([setup](machine-setup.md)). | — |
| 2 | Fleet 2: iroh endpoint and pairing | **Built** (`crates/kanna-net`): [identity §5](machine-identity.md#5-interfaces). Relays stay off until slice 8 sets one up. | 1 |
| 3 | Fleet 3: The app connects to a remote daemon | **Built** ([one link per machine](machine-links.md); tested against a second daemon and an Ubuntu 26.04 VM). A link per machine through `kanna-net`, with the app's certified endpoint key; `app.hello` with `client`, `protocols`, `commit` (the `cert` in the stream header); `other_protocol`; views keyed by (machine, ID); Desks on a project, with the planner on any machine that has it; the Machines tab and path indicator, matching `references/mockups/fleet.html` without its build-job scene. **Changed:** iroh, not TCP; no machine picker or per-machine Desks. | 2, e044 |
| 4 | Fleet 4: Files and threads through the daemon | `file.read`, `markup.*`, screenshot bytes ([machines §7](machines.md#7-terminals-and-panes)). Unchanged. | 3 |
| 5 | Fleet 5: Port tunnels for web panes | `port.open`, a `port` stream per tunnelled connection. **Changed:** streams on the one connection, not a second connection. | 3 |
| 6 | Fleet 6: Fleet update | **Built** ([updates](machine-update.md); tested from this Mac to an Ubuntu 26.04 VM): the Linux tarball, `/opt/kanna/<commit>`, `kanna machine update` (in code `kanna fleet update` until the rename), the app's `other_protocol` message. **Changed:** a range of protocols, not one, so mixed versions work. | 1, 2 |
| 6b | Fleet: daemons update themselves | A daemon updates itself from a peer's release or the release site ([setup §4](machine-setup.md#4-joining-and-updates)). | 6 |
| 8 | Fleet 8: Remote terminals measured | Bounded snapshots, reconnect by offset, an echo-delay table from the LAN and through the relay. **Changed:** also sets up the user's own `iroh-relay` on the GCP e2-micro and the setting that names it ([machines §3](machines.md#3-reach-lan-first-relay-second)). | 3 |
| 10 | Fleet 10: A replicated backlog | [tasks §1–2](machine-tasks.md): every daemon holds every record (iroh-docs, per the model), hybrid-clock order, owners that grant, placement preferences. Done: two daemons share a backlog; two runners asking at once and an owner going away are tested. **Changed:** it needs only 3. | 3 |
| 11 | Fleet 11: Task transfer | [tasks §3](machine-tasks.md#3-transfer): between any daemons, Mac included; a git bundle over the link, not a push; no `/kanna/repos`; resume by session ID; durable on both ends. Done: a stopped claude task and a stopped codex task each move between two machines with different home folders and carry on their conversation. | 10 |
| S1 | *(new: needs a task)* Fleet: agent sign-in | [Agent sign-in](agent-sign-in.md) for your own machines: accounts added on the Mac, a lasting token supplied on joining, the Codex holder over app-server or the per-machine fallback, removal and rotation. Split out of slice 9. | 3 |

**After the first milestone, not yet tasks:** more than one app per daemon, then the phone on the LAN.

## Kanna Cloud

After your machines' first milestone. Its first stage has the operator as the only tenant; a second tenant needs the tenant isolation models to pass.

| Risk | Model | Shows | Before slices |
|---|---|---|---|
| The xcodebuild shim | `xcodebuild-shim`: a Python shim from a Linux VM to a Mac | a dirty worktree built, diagnostics mapped, a console UI test; the owner-account experiment proves forwarding only | 7, 12 |
| Shared build-Mac isolation | `build-mac-isolation`: two tenant-exclusive macOS VMs, per [isolation spec](build-mac-isolation.md) | concurrent guest UI tests; host/cross-tenant access refused; safe artifact handling, guest reassignment and resource limits | 7, 12 |
| Linux tenant VMs | *(new: needs a task)* `cloud-tenant-vms`: two users' Incus VMs on r720-1 | neither reaches the other, the host's management or the LAN, enforced on the host for IPv4 and IPv6; a released VM leaves nothing; a `hosted` key admits no one | C3 |

| # | Task | Slice | After |
|---|---|---|---|
| 9 | Fleet 9: Provisioning | [Cloud §3–4](kanna-cloud.md#3-provisioning): after [the host install](cloud-provisioning.md), Incus, `kanna cloud provision` (a VM for one user made, set up and enrolled by token), the `hosted` mark in the peer log, per-VM git credentials, release destroying the VM. **Changed:** Cloud operator work, not the user's LAN; agent sign-in is S1, which it uses. | 6, S1 |
| C1 | *(new: needs a task)* Cloud: directory and relays | Accounts, the directory publishing peer logs and relaying the PAKE, Kanna's relays ([Cloud §5](kanna-cloud.md#5-directory-and-relays)). | 9 |
| 7 | Fleet 7: xcodebuild redirect to a build Mac | The shim's first version ([build Macs §6](build-macs.md#6-build-order)): worktree sync over a `sync` stream, a durable supervisor, tenant-exclusive macOS VMs on this MBP and later the mini, no host execution, mapped diagnostics, `kanna build -- SCRIPT`. **Changed:** Cloud's; the isolation model must pass first. | 2, both build-Mac models |
| 12 | Fleet 12: Build jobs and iOS from remote tasks | A job queue across a pool of build Macs matched by capability (arch, macOS, Xcode), slots, Simulator leases, tenant-exclusive macOS VM slots behind a restricted host VM manager, the `.app` fetched to the MBP (no ssh to replace). **Changed:** Cloud's. | 7, 5 |
| C2 | *(not yet a task)* Cloud: placement by capability | A task's `needs:` against the capabilities in each machine's peer log entry, for runners asking and the scheduler ([Cloud §5](kanna-cloud.md#5-directory-and-relays)). | 10, C1 |
| C3 | *(not yet a task)* Cloud: other tenants | The host-enforced tenant network, quotas, billing. | 9, C1, Linux tenant model |

**Later, not yet tasks:** the encrypted backlog copy, push, the phone away from the LAN.
