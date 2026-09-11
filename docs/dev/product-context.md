# Product Context

Read this page first when a task depends on why Kanna exists or how a product
decision should be made. It is a short orientation, not a feature inventory or
a roadmap. Follow the links for exact behavior and implementation contracts.

## Evidence and status language

This page distinguishes three kinds of statement:

- **Owner-confirmed** means an explicit product choice, initially recorded in
  Kanna task `05df80b8` on 2026-09-11.
- **Current** means behavior verified in source or an executable definition at
  the initial review baseline, commit `ed77fd2a52a8eb77fbee7eed8f5d8dbde409102c`.
  It proves what exists, not that every detail is intended forever.
- **Proposed** or **open** means a design, exploration, or unanswered question.
  A document in `docs/specs/` is not a commitment merely because it is detailed.

Future-direction consultation task `534d9967` is deliberately not a source of
settled product facts. Its teams-and-integrations work remains exploration until
the owner makes a decision.

## What Kanna is for

Kanna is an operator's workspace for running and steering multiple coding-agent
tasks against software repositories. It gives each task an isolated place to
work, keeps the task and its agent session durable across app and process
lifecycle changes, and moves the work through an explicit workflow whose human
and agent decision points can differ by stage.

The confirmed current fit is **solo developers**. Kanna helps one developer:

- run independent agent tasks in parallel without sharing a checkout;
- keep long-running terminal sessions alive and return to them;
- see which work needs attention without watching every terminal continuously;
- inspect changes and move work through implementation, review, revision, pull
  request, and merge handoff; and
- delegate bounded coordination and review work to agents while retaining
  explicit human gates and authorities.

Teams of 10 or 100 people, multi-user collaboration, and broader integrations
are **exploration, not an approved roadmap**. Do not invent team roles,
permissions, shared-state semantics, integrations, or delivery commitments when
a task depends on them. Raise the missing product decision instead.

## Experience principles

These are owner-confirmed unless marked otherwise.

1. **The agent TUI is the main interaction.** The operator works with the real
   provider CLI in its terminal rather than through a Kanna-authored imitation
   of the agent conversation.
2. **The GUI supplies useful visibility and lifecycle controls.** Desktop and
   mobile should make parallel work legible and provide the surrounding actions
   that are valuable there. They do not need to duplicate every action an agent
   can perform in its TUI or through Kanna's tools.
3. **Mobile is a first-class companion.** It is a real client for observing and
   steering work away from the desk, not a notification-only accessory. This
   does not establish feature-for-feature parity with desktop; that boundary is
   still a product choice.
4. **Authority must be explicit.** An agent result, an idle terminal, a stage
   advance, or a raw key is not a substitute for a human approval. Existing
   examples include production release authorization and the human instruction
   that queues a reviewed pull request. See the root
   [`AGENTS.md`](../../AGENTS.md) for the binding contracts.
5. **Current architecture: task continuity outlives process continuity.** A
   task keeps its identity and history while stages can fork new workspaces and
   start new agent runs. The daemon keeps live PTY sessions independent of the
   desktop app. These are current behaviors, not a new product principle
   inferred from the code; their exact contracts are in
   [Architecture](architecture.md) and
   [`crates/daemon/SPEC.md`](../../crates/daemon/SPEC.md).

## Product model

| Concept | Product meaning |
|---|---|
| Repository | The project boundary. It supplies configuration, workflow and agent definitions, setup/teardown commands, and the task list. |
| Task | One durable work item: its prompt, identity, workflow state, run history, delivered instructions, blockers, and eventual PR link. |
| Workspace | A task stage's branch and git worktree. A stage transition forks a fresh workspace from the task's latest committed tip; only committed work crosses that boundary. |
| Workflow | Ordered stage policy: which role runs, whether a transition is manual or automatic, and whether tail work runs as a post. The definition is pinned to the task. |
| Stage run | One execution of a stage or post, with its agent/provider, session, result, and provenance. Revisions add runs; they do not create a replacement task. |
| Parent and blocker links | Two different graph relationships: parentage groups true subtasks; blockers express execution dependencies. Neither is generic ownership. |
| Runtime, read state, and activity | Runtime says what the agent process is doing; read state says whether a person has seen the latest output; activity is a blended display value. They answer different questions. |
| Agent provider | The installed external CLI Kanna launches for a run. Provider authentication remains with that CLI. |

The canonical lifecycle definitions and naming rules are in
[`AGENTS.md`](../../AGENTS.md#core-concepts). User-visible details are in
[Product Behavior](product-behavior.md); process and data ownership are in
[Architecture](architecture.md).

## Primary journeys

### Start and steer work

The operator imports or creates a repository, creates a task with a prompt and
workflow/provider choice, and Kanna creates the isolated worktree and launches
the provider CLI. The operator watches and talks to that CLI in its real TUI.
The desktop adds the repo/task sidebar, attention state, diff and file views,
shell access, stage controls, and preferences around that terminal. See
[Product Behavior: Workflows](product-behavior.md#workflows).

### Review, revise, and hand off

The operator inspects the branch diff and advances a manual gate when satisfied.
A workflow may run one reviewer or dispatch specialist review children; a
failed review requests a bounded revision of the same durable task. The PR stage
creates and preserves the pull-request link. Human review authority stays
separate from agent verdicts and ordinary stage advancement. See
[QA Dispatch Review](../specs/qa-dispatch-review.md) and
[PR Review Dispatch](../specs/pr-review-dispatch.md), whose own status labels
identify implemented and proposed portions.

### Let an agent coordinate agents

A manager or dispatcher agent can create children, express blockers, monitor
durable task events, send recorded instructions, and advance work through the
same server API used by other clients. The agent tools are the orchestration
surface; direct database access is not. Kanna's engine executes known workflow
structure, while agents can create runtime structure such as review fan-out.
The human remains responsible for decisions that a workflow or explicit owner
instruction reserves to them.

### Continue from mobile

The phone pairs with a desktop, shows tasks and attention across repositories,
opens the agent view or terminal, sends task input, and previews files and
diffs. It connects directly on a trusted LAN when possible and through the
relay when remote. Pins and Activity dismissals are local to the phone; task
lifecycle state is not. See [Product Behavior: Mobile app](product-behavior.md#mobile-app)
and [Architecture: Mobile app](architecture.md#mobile-app--appsmobile--packagesstream-client).

## Who decides what

| Actor | Decides or owns | Does not imply |
|---|---|---|
| Human operator | Task intent; manual stage gates; human revision recovery; PR review instructions; production authorization; product direction | That every interaction needs a GUI control |
| Pinned workflow | Stage order, role bindings, posts, and automatic versus manual transitions | Human approval beyond the policy explicitly encoded |
| Stage agent | How to perform its bounded role; a reviewer/dispatcher may return a verdict or select necessary specialty checks | Permission to broaden scope, claim human authority, or turn a proposal into roadmap |
| Repository definitions | Repo-specific setup, tests, workflow/agent customization, provider selection, and release extension | A universal Kanna product promise |
| `kanna-server` | Durable task/workflow state, workspaces, stage transitions, task events, and the API used by clients and agents | Product intent inferred from a stored field or endpoint |
| PTY daemon | Live terminal sessions, terminal state, reattach snapshots, and handoff | Workflow or approval decisions |
| Desktop and mobile clients | Presentation and task actions against server-owned state, plus terminal interaction with daemon-owned sessions; some view preferences are deliberately device-local | A second source of task truth |

## Current, partial, and proposed surfaces

- **Current core:** the macOS desktop, PTY daemon, local server, supported agent
  CLIs, isolated task workspaces, workflow stages and revisions, task graphs,
  agent-facing MCP/CLI control, the iOS companion, LAN access, remote relay,
  and machine-to-machine task transfer. The code map is in
  [Architecture](architecture.md).
- **Current but deliberately scoped:** Kanna's implemented collaboration model
  is one operator coordinating agents. The Linux headless worker exists; Linux
  desktop distribution is still partial. See
  [Linux desktop support](../specs/linux-desktop-support.md).
- **Mixed-status designs:** some specs contain both shipped and proposed
  phases. For example, PR review dispatch labels phase 1 implemented and later
  phases proposed, while [Remote Dev Preview](../specs/remote-dev-preview.md)
  labels its LAN slice implemented and its off-network path unbuilt. Read each
  spec's status and dated evidence before describing a capability.
- **Proposed only:** forge independence is explicitly proposed and parked
  behind evidence gates. It is not Kanna's current collaboration model. See
  [Forge Independence](../specs/forge-independence.md).
- **Exploration only:** teams and integrations, including the questions in
  task `534d9967`, have no approved commitment, sequence, or scale target.

## Owner discussion: highest-value open questions

Keep this list short. Resolve a question with owner evidence, record the
decision and date here, and remove the question rather than accumulating a
permanent wishlist.

1. **Audience after solo developers:** What observed job would justify support
   for a 10-person or 100-person team, and who are the users and decision-makers
   in that workflow? No collaboration or integration design is approved yet.
2. **GUI/TUI boundary:** Which actions deserve desktop or mobile affordances
   because they improve visibility, safety, or remote use, and which should
   remain agent-TUI/tool interactions? The principle is settled; the complete
   action-by-action boundary is not.
3. **Mobile scope:** What capabilities are essential for a first-class
   companion, and which may remain desktop-only? “First-class” currently says
   importance, not parity.
4. **Supported workflow lineup:** The README names three public workflows, but
   the current definitions also expose `plan-build-review` and `pr-review` as
   public. Which set should onboarding present as the supported product lineup?
5. **Collaboration source of truth:** If real multi-user demand arrives, which
   existing system should own shared assignments, discussion, review, and
   authorization? Do not assume Kanna must replace the forge or issue tracker.
6. **Platform promise:** Linux has a working headless path and partial desktop
   implementation, but the supported end-user platform and distribution promise
   have not been settled here.
