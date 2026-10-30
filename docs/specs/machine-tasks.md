# Machines: Tasks Across Machines

**Status:** planned · **Part of:** [your machines](machines.md) · **Builds on:** [Shelf runners](shelf-runners.md), [tasks](tasks.md), [each app's Desks](app-desk.md)

Where a task runs when you have several machines: one Shelf replicated to all of them, exactly one machine allowed to start each task, and tasks moving between any two daemons at a boundary. All of it works on your own machines with no account; [Kanna Cloud](kanna-cloud.md#5-directory-and-relays) adds an always-on copy.

## Contents

1. [The replicated Shelf](#1-the-replicated-shelf)
2. [Owners and grants](#2-owners-and-grants)
3. [Transfer](#3-transfer)
4. [Repos and credentials](#4-repos-and-credentials)

## 1. The replicated Shelf

Every one of your daemons holds every task's record, synced peer to peer over iroh, LAN first.

| Part | Consistency |
|---|---|
| A task's record: name, first message, agent, `after`, state, owner | eventual: last writer wins per field, ordered by a hybrid logical clock ("oldest first") |
| Who runs it: the **owner** | one machine; changes only by a grant from the owner (§2) |

The [backlog model](../../references/prototypes/fleet-backlog/README.md) chooses **iroh-docs**, with immutable field updates projected by hybrid logical clock; it compares Automerge and Loro and records the tradeoffs. Slice 10 follows its ownership, placement and partition scenarios, then tests real storage and crash recovery. The Python model simulates the storage contract, not the library.

## 2. Owners and grants

```mermaid
sequenceDiagram
    participant R4 as runner on r720-4
    participant O as owner (r720-1)
    participant X as runner on r720-2
    R4->>O: ask for task-1dfb
    X->>O: ask for task-1dfb
    O-->>R4: granted: you own it
    O-->>X: refused: owned by r720-4
    Note over R4: transfer (§3), then worktree, agent, workspace
    Note over O,X: the record replicates: owner r720-4
```

- **A task starts on its owner,** at first the machine where it was made. Who grants is a policy too: a scheduler (a load balancer over many machines, or [Kanna Cloud's](kanna-cloud.md#5-directory-and-relays)) can own every waiting task and assign them centrally ([machines §2](machines.md#2-machines-and-roles)).
- **Asking:** a runner with free slots, within the task's placement preferences, asks the owner for the oldest task waiting. Matching a task's `needs:` to machines' capabilities is [Kanna Cloud's](kanna-cloud.md#5-directory-and-relays). The owner grants it to exactly one runner, and that runner becomes the owner.
- **Two meanings, two words:** "ask" and "grant" are between machines. A **claim** stays what it is in [app-desk](app-desk.md): an app having a task on its Desk, now named by its device (`claimed by: mbp`).
- **An offline owner's tasks wait.** Leases that expire would allow a double start, so add them only if waiting hurts.
- **Task IDs** stay random 32 bits ([task IDs](task-ids.md)). Apps key views by (machine, ID), and an ID that collides with one in the replica is refused when the task is made.

## 3. Transfer

A task moves between any two daemons, Macs included, at a boundary: not started, or stopped at a turn's end.

```mermaid
sequenceDiagram
    participant S as sender
    participant R as receiver
    S->>S: agent quits by its keys (Tasks::end_agent), session ID kept
    S->>S: WIP commit on the task's branch
    S->>R: git bundle of the branch (objects the receiver lacks)
    S->>R: the transcript, from where the agent keeps it
    R->>R: fetch into its own clone, new worktree, reset --soft to before the WIP commit
    R->>R: transcript into its own session folder
    R->>R: agent resumes by session ID, tabs reopened
    R-->>S: done: the conversation carried on
```

- **No push needed:** the branch travels as a git bundle over the link, so repos without a remote work too, and WIP commits never reach origin.
- **The dirty tree comes back:** `reset --soft` leaves the agent's uncommitted work as it left it, staged.
- **Any path:** the receiver uses its own clone and worktree paths. Agents resume by session ID from any path: checked 2026-10-21 with Claude Code 2.1.305 and Codex 0.163, and v2 relied on it.
- **Durable:** each step is recorded on both ends and resumes after either side restarts. The sender keeps the task until the receiver says done.
- **v2's lessons:** copy the transcript from where the agent really keeps it (v2 copied a lock file and lost the transcript without noticing), never move a running agent, and test that the conversation carries on, not only that the transfer "completed".

## 4. Repos and credentials

- **Clones** live wherever a machine likes (`~/repos/<name>` on servers). A repo is identified across machines by its remote URL, or, for one with no remote, by the ID of its first commit.
- **Git credentials** are per machine: a deploy key or `gh auth login` with a fine-grained token on each machine. Your personal key never leaves the Mac.
- **Agent logins** are set up when a machine joins ([agent sign-in](agent-sign-in.md), [setup §4](machine-setup.md#4-joining-and-updates)).
