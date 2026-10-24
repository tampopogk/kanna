# Fleet Build Order

**Part of:** [Fleet](fleet.md) · **Build isolation:** [shared build Macs](build-mac-isolation.md)

Each slice is one agent's task and leaves `main` releasable. The tasks' first messages predate the decisions in Fleet and its part specs; where a slice below differs from its task, this spec wins (**Changed**). **Test rig:** two daemons are two instances on one Mac ([instances](instances.md)) or an instance on this Mac and r720-1 (installed, Ubuntu 26.04.1).

**Models first.** Each risky part gets a model before its slice: a throwaway prototype in `references/prototypes/<name>/` or a mockup in `references/mockups/`. Its slice starts once the model shows the risk is handled, and its tests then compare the product with the model. Each model is its own task.

| Risk | Model | Shows | Before slices |
|---|---|---|---|
| iroh's reach and speed | [fleet-net](../../references/prototypes/fleet-net/README.md): two endpoints in Rust, LAN discovery, a relay on the e2-micro, `kanna/1` streams of each kind | a direct path on the LAN, hole punching from away, relay fallback; echo delay and throughput for each, the standard slice 8 is held to | 2, 3, 8 |
| Pairing and the peer log | `fleet-pairing`: Python, a PAKE over a code, a signed log between three simulated devices | a forged key refused; mutual revocation; a revoked key never re-added | 2 |
| Desks across machines | `references/mockups/fleet.html` | a Desk per project with tasks on several machines; the fleet page; a card on a relayed or offline machine | 3 |
| The replicated backlog | `fleet-backlog`: Python, N simulated daemons with partitions | the store chosen; two runners asking at once; an owner going away; a scheduler taking over | 10 |
| Transfer | `fleet-transfer`: a script between two home folders | bundle, `reset --soft`, the transcript moved, Claude and Codex carrying on | 11 |
| The xcodebuild shim | `xcodebuild-shim`: a Python shim from a Linux VM to a Mac | a dirty worktree built, diagnostics mapped, a console UI test; the owner-account experiment proves forwarding only | 7, 12 |
| Shared build-Mac isolation | `build-mac-isolation`: two tenant-exclusive macOS VMs, per [isolation spec](build-mac-isolation.md) | concurrent guest UI tests; host/cross-tenant access refused; safe artifact handling, guest reassignment and resource limits | 7, 12 |
| Shared agent accounts | `agent-sign-in`: probes on two VMs | one setup token on many machines at once; Codex on access tokens from a holder | 9 |

| # | Task | Slice | After |
|---|---|---|---|
| 1 | Fleet 1: Kanna on Linux, headless | Daemon, keeper and CLI build and run on Linux x86_64. `server-setup.sh` and the unit (§9). A real agent runs a task with runners; terminals survive a daemon restart. **Changed:** Ubuntu 26.04, on r720-1 (an Incus VM there, or the host until slice 9). | — |
| 2 | Fleet 2: TCP listener and pairing | **Changed, renamed "iroh endpoint and pairing":** confirm iroh's version and stability first. Then: the endpoint in the daemon with `device.key`, iroh's defaults off, LAN discovery, `kanna fleet pair` with a PAKE, the signed peer log, `identity_mismatch`, revocation, `kanna fleet addr`, `kanna --host` through the local daemon, daemon-to-daemon connections, and the stream convention (§5). No `KANNA_LISTEN`, `fleet.json` or signed hello. Done: `kanna --host r720-1 task list` from the Mac; an unpaired key is refused; a stream of each kind opens between two daemons. | 1 |
| 3 | Fleet 3: The app connects to a remote daemon | A link per machine through `kanna-net`, with the app's certified endpoint key; `app.hello` with `client`, `cert`, `protocols`, `commit`; `other_protocol`; views keyed by (machine, ID); Desks on a project, with the planner on any machine that has it; the fleet page and path indicator, matching `references/mockups/fleet.html`. **Changed:** iroh, not TCP; no machine picker or per-machine Desks (§2, placement). | 2, e044 |
| 4 | Fleet 4: Files and threads through the daemon | `file.read`, `markup.*`, screenshot bytes (§7). Unchanged. | 3 |
| 5 | Fleet 5: Port tunnels for web panes | `port.open`, a `port` stream per tunnelled connection. **Changed:** streams on the one connection, not a second connection. | 3 |
| 6 | Fleet 6: Fleet update | The Linux tarball, `/opt/kanna/<commit>`, `kanna fleet update`, the app's `other_protocol` message. **Changed:** daemons update themselves; a range of protocols, not one, so mixed versions work. | 1, 2 |
| 7 | Fleet 7: xcodebuild redirect to the Mac mini | The shim's first version ([build Macs §6](build-macs.md#6-build-order)). **Changed:** worktree sync over a `sync` stream, not rsync over ssh; a durable supervisor from the start; tenant-exclusive macOS VMs on this MBP and later the mini; no host execution; the isolation model must pass first; mapped diagnostics; `kanna build -- SCRIPT`. | 2 |
| 8 | Fleet 8: Remote terminals measured | Bounded snapshots, reconnect by offset, an echo-delay table from the LAN and through the relay. **Changed:** also sets up `iroh-relay` on the GCP e2-micro. | 3 |
| 9 | *(new: needs a task)* Fleet 9: Provisioning | After [provisioning](fleet-provisioning.md) installs the host: Incus, `kanna fleet provision` (a VM for a user made, set up and enrolled), git credentials on enrollment (§9), and [agent sign-in](agent-sign-in.md). | 6 |
| 10 | Fleet 10: A home per repo | **Changed, renamed "A replicated backlog":** [tasks §1–2](fleet-tasks.md): every daemon holds every record (iroh-docs, Automerge or Loro: choose and say why), hybrid-clock order, owners that grant, `needs:` against capabilities, placement preferences. Done: two daemons share a backlog; two runners asking at once and an owner going away are tested. The task waits for 6 and 3; it needs only 3. | 3 |
| 11 | Fleet 11: Teleport between Linux machines | **Changed, renamed "Task transfer":** [tasks §3](fleet-tasks.md#3-transfer): between any daemons, Mac included; a git bundle over the link, not a push; no `/kanna/repos`; resume by session ID; durable on both ends. Done: a stopped claude task and a stopped codex task each move between two machines with different home folders and carry on their conversation. | 10 |
| 12 | Fleet 12: Build jobs and iOS from remote tasks | **Changed:** a job queue across a pool of build Macs matched by capability (arch, macOS, Xcode), slots, Simulator leases, tenant-exclusive macOS VM slots behind a restricted host VM manager, with guest console sessions and the isolation model required, the `.app` fetched to the MBP (no ssh to replace). | 7, 5 |

**After the first milestone, not yet tasks:** Kanna Cloud's account, directory and relays (provisioning by token, the encrypted backlog copy, push); more than one app per daemon, then the phone; Kanna-run machines for other tenants (tenant VM pools, billing).
