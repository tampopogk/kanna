# Machines Build Order

**Part of:** [your machines](machines.md) and [Kanna Cloud](kanna-cloud.md) · **Build isolation:** [shared build Macs](build-mac-isolation.md)

Each slice is one agent's task and leaves `main` releasable. **Your machines** come first, then **Kanna Cloud**. Slice numbers are the tasks' names (`Machines 6`, `Cloud 7`), not the order. The tasks' first messages predate the decisions in these specs and their part specs; where a slice below differs from its task, this spec wins (**Changed**).

**Test rig:** two daemons are two instances on one Mac ([instances](instances.md)), or an instance on this Mac and a Linux VM: a lima VM, or an Incus VM on r720-1 standing in for a Linux machine of the user's. r720-1 is Kanna Cloud's first host (Ubuntu 26.04.1).

**Models first.** Each risky part gets a model before its slice: a throwaway prototype in `references/prototypes/<name>/` or a mockup in `references/mockups/`. Its slice starts once the model shows the risk is handled, and its tests then compare the product with the model. Each model is its own task.

## Your machines

The first milestone: your own machines, LAN first, your own relay or a fixed address. One tenant, so no tenant isolation; it neither includes nor depends on build Macs, a Cloud feature.

| Risk | Model | Shows | Before slices |
|---|---|---|---|
| iroh's reach and speed | [fleet-net](../../references/prototypes/fleet-net/README.md): two endpoints in Rust, LAN discovery, a relay on the e2-micro, `kanna/1` streams of each kind | a direct path on the LAN, hole punching from away, relay fallback; echo delay and throughput for each, the standard slice 8 is held to | 2, 3, 8 |
| Pairing and the peer log | `fleet-pairing`: Python, a PAKE over a code, a signed log between three simulated devices | a forged key refused; mutual revocation; a revoked key never re-added | 2 |
| Desks across machines | `references/mockups/fleet.html` | a Desk per project with tasks on several machines; the Machines tab; a card on a relayed or offline machine | 3 |
| The replicated Shelf | `fleet-backlog`: Python, N simulated daemons with partitions | the store chosen; two runners asking at once; an owner going away; a scheduler taking over | 10 |
| Transfer | `fleet-transfer`: a script between two home folders | bundle, `reset --soft`, the transcript moved, Claude and Codex carrying on (the product moves a handoff instead) | 11 |
| Shared agent accounts | `agent-sign-in`: probes on two VMs | one setup token on many machines at once; Codex on access tokens from a holder | S1 |

| # | Task | Slice | After |
|---|---|---|---|
| 1 | Machines 1: Kanna on Linux, headless | **Built.** Daemon, keeper and CLI on Linux x86_64; `server-setup.sh` and the unit ([setup](machine-setup.md)). | — |
| 2 | Machines 2: iroh endpoint and pairing | **Built** (`crates/kanna-net`): [identity §5](machine-identity.md#5-interfaces). Relays stay off until slice 8 sets one up. | 1 |
| 3 | Machines 3: The app connects to a remote daemon | **Built** ([one link per machine](machine-links.md); tested against a second daemon and an Ubuntu 26.04 VM). A link per machine through `kanna-net`, with the app's certified endpoint key; `app.hello` with `client`, `protocols` (now one `protocol`), `commit` (the `cert` in the stream header); `other_protocol`; views keyed by (machine, ID); Desks on a project, with the planner on any machine that has it; the Machines tab and path indicator, matching `references/mockups/fleet.html` without its build-job scene. **Changed:** iroh, not TCP; no machine picker or per-machine Desks. | 2, e044 |
| 4 | Machines 4: Files and threads through the daemon | **Built** ([files and threads](files-and-threads.md)): `file.read`, `markup.*`, screenshot bytes, the diff base and `app.snap`'s text ([machines §7](machines.md#7-terminals-and-panes)); local panes read through the daemon too. **Changed:** the PNG comes back in the app's reply, not on a stream of its own. | 3 |
| 5 | Machines 5: Port tunnels for web panes | **Built** ([port tunnels](port-tunnels.md)). `port.open`, a `port` stream per tunnelled connection. **Changed:** streams on the one connection, not a second connection. | 3 |
| 6 | Machines 6: Machine update | **Built** ([versions](machine-versions.md)): the Linux tarball, the app's `other_protocol` message. **Changed:** `kanna machine update` and daemons updating themselves (6b) were removed: you install Kanna on your Linux machines yourself, and Kanna Cloud's servers get an internal tool. The range of protocols became one protocol per build, and a machine of another shows as Incompatible. | 1, 2 |
| 8 | Machines 8: Remote terminals measured | Bounded snapshots, reconnect by offset, an echo-delay table from the LAN and through the relay. **Changed:** also sets up the user's own `iroh-relay` on the GCP e2-micro and the setting that names it ([machines §3](machines.md#3-reach-lan-first-relay-second)). | 3 |
| 10 | Machines 10: A replicated Shelf | **Built** ([tasks §1–2](machine-tasks.md)): every daemon holds every record, hybrid-clock order, owners that grant, `--on` preferences; tested with two and three daemons on a Mac (a shared Shelf oldest first, two asks at once, a grant lost on the way, a runner and an owner away) and between two Ubuntu 26.04 VMs. **Changed:** it needs only 3; records are an op log in `kanna.db` over `sync` streams, not iroh-docs; no home machine, and no `needs:` (Cloud's C2). | 3 |
| 11 | Machines 11: Task transfer | **Built** ([task transfer](task-transfer.md)): `kanna task move TASK MACHINE` between any daemons; a git bundle over a `transfer` stream, not a push; no `/kanna/repos`; the handoff lists the tabs that were open, none reopened (2026-11-03); durable on both ends. Tested with two daemons with different home folders (mock claude and codex, a receiver gone before it answers, a move back). **Changed:** the agent carries on from a handoff its predecessor wrote, not by resuming its session (decided with the user, 2026-11-02), so done is a stopped claude task and a stopped codex task each carrying on their work on another machine: done with real agents from a MacBook Pro to a Mac Studio (2026-11-03). | 10 |
| S1 | Machines: agent sign-in | **Built** ([agent sign-in](agent-sign-in.md)): accounts added on the Mac (`kanna accounts`, the Kanna menu), kept in the Keychain; tokens and keys supplied by version on adding and joining; the Codex holder through each Codex's own app-server, which the terminal command uses; `kanna accounts sign-in` as the fallback; removal everywhere, revocation taking back what a machine was given. Split out of slice 9. | 3 |

**After the first milestone, not yet tasks:** more than one app per daemon, then the phone on the LAN.

## Kanna Cloud

After your machines' first milestone. Kanna Cloud is Kanna's product ([Cloud](kanna-cloud.md)): customers ask from their own Kanna, Kanna provisions with its internal `kanna-cloud`. Its first stage has Kanna as the only tenant; a second tenant needs the tenant isolation models to pass.

| Risk | Model | Shows | Before slices |
|---|---|---|---|
| The xcodebuild shim | `xcodebuild-shim`: a Python shim from a Linux VM to a Mac | a dirty worktree built, diagnostics mapped, a console UI test; the owner-account experiment proves forwarding only | 7, 12 |
| Shared build-Mac isolation | `build-mac-isolation`: two tenant-exclusive macOS VMs, per [isolation spec](build-mac-isolation.md) | concurrent guest UI tests; host/cross-tenant access refused; safe artifact handling, guest reassignment and resource limits | 7, 12 |
| Linux tenant VMs | [cloud-tenant-vms](../../references/prototypes/cloud-tenant-vms/README.md): two users' Incus VMs on r720-1 | neither reaches the other, the host's management or the LAN, enforced on the host for IPv4 and IPv6; a released VM leaves nothing; a `hosted` key admits no one | C3 |

| # | Task | Slice | After |
|---|---|---|---|
| 9 | Cloud 9: Provisioning | **Built** (2026-11-03, R720 backend). Kanna's internal `kanna-cloud` CLI ([how Kanna provisions](cloud-vms.md)): host setup (Incus, the tenant network, the pool), making a VM for a Cloud task with Kanna running in it (server setup), release that leaves nothing; nothing in the customer's `kanna` and nothing in their peer log. **Changed:** provisioning is Kanna's, never a customer command; Cloud VMs never join the customer's machines, so the `hosted` mark and token enrollment were dropped (review, 2026-11-03). | 6 |
| C1 | Cloud: accounts, link, directory and relays | Cloud accounts joined from the CLI (`kanna cloud join`, an emailed code) and the account key in the Mac's Keychain; the customer's daemon's link to Kanna Cloud (its pinned key, signed in with the account key); the service that takes a Cloud task and has `kanna-cloud` provision it; vouchers for task VMs, accepted for their one task only; the directory publishing peer logs and relaying the PAKE; Kanna's relays ([Cloud §3, §6–7](kanna-cloud.md#6-the-link-to-kanna-cloud)). | 9 |
| C5 | Cloud: Cloud tasks | `kanna task new … --cloud` and `kanna task move TASK --cloud`: a VM per task, reached through the link and its voucher, never in the customer's machines; the repo sent as a git bundle and branches back the same way, no git credential on the VM; the VM destroyed when the task is done ([Cloud §5](kanna-cloud.md#5-cloud-tasks)). | C1, 11 |
| C6 | Cloud: the Shelf in the cloud | Cloud holds a replica of the customer's Shelf and runs it as a runner with its own limit (`shelf.cloud`), each task on its own VM, laptop open or closed; the project's planner on a cloud VM, the only holder of opt-in git access (a GitHub App), fetching, pushing and opening PRs for the Shelf; messages as Shelf records delivered wherever the recipient runs; a copy of each repo kept by the customer's machines' bundles and the agent account held by the service; finished branches wait as bundles for the customer's machines ([Cloud §5](kanna-cloud.md#5-cloud-tasks)). | C5, 10 |
| 7 | Cloud 7: xcodebuild redirect to a build Mac | The shim's first version ([build Macs §8](build-macs.md#8-build-order)): worktree sync over a daemon stream, a durable supervisor in the guest (`kanna-guest`), tenant-exclusive macOS VMs (`kanna-vm`, `kanna guests`) on the Mac Studio, later the mini, no host execution, mapped diagnostics, `kanna build -- SCRIPT`. **Changed:** Cloud's; a `job` stream, not `sync`; whole changed files, not changed blocks; it goes ahead for the operator as the only tenant while the isolation model's guest-root probes are still to run, and they gate a second tenant (decided with the user, 2026-11-03); each guest gets a data disk for its builds. Kanna.app and a Swift test built from a Linux VM on 2026-11-03 ([results](build-macs.md#8-build-order)). | 2, both build-Mac models |
| 12 | Cloud 12: Build jobs and iOS from remote tasks | A job queue across a pool of build Macs matched by capability (arch, macOS, Xcode), slots, Simulator leases, tenant-exclusive macOS VM slots behind a restricted host VM manager, the `.app` fetched to the MBP (no ssh to replace). **Changed:** Cloud's. | 7, 5 |
| C2 | Cloud: placement by capability | A task's `needs:` against the capabilities in each machine's peer log entry, for runners asking and the scheduler ([Cloud §7](kanna-cloud.md#7-directory-and-relays)); a Cloud task's `needs: xcode` sends its builds to a build Mac. | 10, C1 |
| C3 | *(not yet a task)* Cloud: other tenants | Quotas, billing per account (the payment provider's hosted checkout and portal, opened by `kanna cloud billing`), the network check on every host. | C1, Linux tenant model |

**Later, not yet tasks:** push, the phone away from the LAN.
