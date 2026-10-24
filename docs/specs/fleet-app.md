# Fleet: Desks Across Machines

**Status:** model ready for user review; product planned · **Part of:** [Fleet §6](fleet.md#6-one-app-several-machines) · **Model:** [fleet.html](../../references/mockups/fleet.html) · **Run and scope:** [README](../../references/mockups/fleet/README.md) · **Before:** Fleet 3

A Desk belongs to a project. Pulling a task changes what the app shows, while its agent and jobs keep running where they are. The model follows the existing [Desk](../../references/mockups/desk.html) style.

## 1. Project, Shelf and backlog

![One project Desk with direct, relayed and offline tasks on its three-place Shelf, and a backlog spanning machines](fleet-desk.png)

- **A Desk is on a project,** whichever machines its tasks run on. A project is known across machines by its remote URL or first commit ([tasks §4](fleet-tasks.md#4-repos-and-credentials)). Its planning terminal runs on a machine that has the project, placed like a task, and its backlog page lists the project's tasks from every machine. The new-Desk prompt and Open Recent show projects, not paths on a machine. Opening an already-open project brings its existing Desk forward.
- **The Shelf stays the app's:** claims in its order, three per project including the task on the Desk, keyed by (machine, task) in `app-state.json`. Pulling and parking preserve claims, order and ownership. Task cards, the Shelf, agent headers and build summaries put the machine and connection path in a tooltip, available on hover or keyboard focus. No host names or Relayed / Offline badges appear in their normal presentation.
- **Claim records name the device:** `claimed by: mbp` (the hello's `client`) replaces `mac` in the protocol. This is internal bookkeeping, not a label in the UI: users work on tasks without tracking the claiming device or execution machine. Runners leave claimed tasks alone, as today.
- **Toasts** held while no app was attached arrive when the app connects, so five servers' finished tasks show up when you open the lid. Toast delivery is outside this model; use the [toast reference](../../references/mockups/toasts.html) for its presentation.
- A task can use several machines: the model's **Keep terminal output** agent runs on `dev-1`, and its macOS UI test job runs on `mbp`. The Mac is this build Mac; no mini is assumed. The task’s UI tests pane shows the job, with its build machine in a tooltip. It does not create another task or claim.

## 2. Fleet and paths

![Fleet shows the local Mac, a direct LAN peer, a relayed peer and an offline peer](fleet-page.png)

Fleet is app-wide: each peer's path **from this Mac**, capabilities and connection state. It has no Work column; task and job details stay with the task. A host such as r720-1 is context for a VM, not an additional peer. The relay is a path, not a task-running machine.

Each row distinguishes **This Mac**, **Direct · LAN**, **Direct · internet**, **Relayed**, and **Offline**. Shelf and Fleet are fixed, non-closeable tabs at the start of the right-hand tab strip, before the current task’s document tabs. Shelf shows the project’s Shelf and backlog; Fleet shows the machine inventory. Both remain available while a task is open. Switching tabs preserves the current agent, task selection, input and claims; it does not park the task. Hovering or keyboard-focusing a path indicator within Fleet reveals its connection explanation as a tooltip. There is no persistent connection-detail panel. Select Shelf or the task’s document tab to return.

Fleet offers no machine picker or manual path switch; the switches above the window are model controls only.

A relay-to-direct transition updates the task's tooltip automatically. It does not move the task, replace its workspace, or create a second Desk. A direct internet path is not labeled LAN or relayed. Example paths and work in the screenshots are fixtures, not measurements or a report of deployed machines.

![The relayed path explains its connection on hover or keyboard focus](fleet-path-tooltip.png)

## 3. Relayed and offline tasks

![A relayed task remains usable with no visible host or path badge](fleet-relay.png)

A relayed task remains interactive. Hover or focus its card, Shelf item or agent header to see **tests-1 · Relayed**. An offline task’s tooltip says **tools-1 · Offline**; direct and local tasks expose their machine and path the same way. Escape dismisses the tooltip. Connectivity is separate from the agent’s working / needs-you state.

![Hovering a task reveals its machine and connection path in a tooltip](fleet-tooltip.png)

![An offline task retains its last view and claim; input is disabled and the received output is marked stale](fleet-offline.png)

An offline machine's task remains on the Shelf. **View last state** opens its last received workspace; output and task state are explicitly stale and input is disabled. There is no implicit restart, transfer or typed-input queue. The user can still park it and work on another task. An offline owner's backlog tasks wait for that owner ([tasks §2](fleet-tasks.md#2-owners-and-grants)).

On reconnection, the same task and panes become interactive again. The claim, Shelf order and Desk selection survive. Each pane is keyed by (machine, content ID); the model deliberately gives its three claimed tasks the same daemon-local terminal ID to expose accidental cross-machine reuse.

## 4. What Fleet 3 should copy

Compare the product against these scenes and transitions, using the model's fixture names and the same viewport (1360 × 850; Fleet page 1360 × 920). The model's URL fragments make each starting scene repeatable.

| Scene / action | Expected result |
|---|---|
| Default / `#picture` | One Kanna Desk, three claims across three peers; one merged project backlog; no machine picker, visible host/path badges or claim-device labels |
| Shelf → Fleet → task document tab | Fixed tabs stay first and have no close controls; agent, task selection, input and claims are unchanged; arrow keys switch tabs |
| File → New Desk / Open Recent → Kanna twice | Existing project Desk comes forward; its task selection survives; no duplicate Desk |
| Open Recipe Box, then Kanna | Separate project Desks with their own Shelf and selection; names, not clone paths |
| Pull **Keep terminal output** then **Review build output** | Correct agent and tabs despite matching terminal IDs; task input cannot leak between peers |
| `#picture,fleet` | Machine and path columns only; capabilities and last-seen information; no task or job listing |
| `#picture,relay-task`, hover or focus its header | Tooltip gives the machine and Relayed state; no persistent badge; Fleet remains available as a fixed tab |
| `#picture,offline-task` | Retained claim and last view; stale labels; disabled input; other tasks remain available |
| Mixed paths → All on LAN | Tooltip changes to Direct · LAN; input returns; task, owner, claim count and order unchanged |
| Internet direct | Direct internet path in Fleet; updated tooltip, no connection badge on the task card |
| Remote offline | Remote planner and remote task input disabled; local tasks and the Fleet inventory remain available |

**Review correction:** execution and claim machines are implementation details throughout the task UI. Machine identity and relay/offline state belong in task tooltips, not persistent labels. Fleet remains the explicit machine inventory. Stale-output notices and disabled input remain visible so loss of reach is not mistaken for a live agent.

**What the model exposed:** a connection failure is not an agent stop. Reusing a plain “working” card during disconnection falsely claims fresh knowledge; clearing the card loses the app's claim. Keep the card and explicitly age its state. Similarly, a build job's machine is not necessarily the agent's machine, so its details belong to that task without multiplying tasks or adding a work inventory to Fleet.

**Limits:** fixed fixture data and timestamps; in-memory views and text input; no real link, peer discovery, repo-identity normalization, disk persistence, background work, transfer, pairing or toast queue. The model switches between project windows inside one HTML page. Fleet 3 must prove those real boundaries separately; this model establishes the presentation and state transitions, not network performance.
