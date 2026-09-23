# Specification: tasks, sessions, and structured workflows (target architecture)

Status: owner-directed target design, 2026-09-22, produced by research task `482a02db`. It describes what Kanna should be, not what it is. The planning stage that follows decomposes it into buildable tasks and decides what existing code is reused; nothing here prescribes that. Statements marked **Owner** were decided by the owner in this task's discussion or earlier and are not open. Everything else is the design the owner accepted as the basis for planning.

## 1. Goals

1. Structured workflows: a task moves through stages; each transition is manual or automatic; humans read at few, deliberate points (design accepted, stakeholders satisfied, PR brief) and agents do the work between.
2. One agent per stage in V1. Parallel work is subtasks, each a task with its own session. **Owner.** (Deferred, not precluded: several agents in one stage's workspace, one writing and others reviewing the same worktree.)
3. Artifacts pass from stage to stage and can be sent to other users, including users on other accounts. Sending an artifact is the only thing that crosses an account boundary; machines are never paired across accounts. **Owner.**
4. Dependencies between tasks, including a task depending on another task at a specific stage.
5. Front-loaded design: iterate with an agent on a mockup, get stakeholders to agree, then let agents implement and test so the PR comes out right.
6. Simple, formulaic agent definitions; sensible defaults; policy overridable per repo and per user.
7. The database stays local to one machine and never has to converge with another. **Owner.**

Non-goals for this iteration: several agents sharing one stage's workspace (**Owner:** keep V1 simple, one agent per stage), a cloud or organizational database, CRDTs, engine-computed merging of subtask outputs, drift detection at gates (**Owner:** try it, fix it if it is a problem), transcript resume across machines as something the design depends on (**Owner:** transfer is unreliable; resume is opportunistic).

## 2. Vocabulary

| Word | Meaning |
|---|---|
| **Task** | A durable unit of intent: id, title, origin prompt, ledger, stage chain, links (parent, dependencies), owning machine, PR link. |
| **Stage** | One step of a task with a role. A task is *in* a stage from the transition that entered it until the transition that leaves it. |
| **Transition** | The move from one stage to the next, or back to an earlier one. Manual (someone advances) or automatic (a success result fires it). A manual transition is a gate. **Owner.** |
| **Workspace** | What a stage works in: a worktree, the task's reserved ports, the setup that ran there, and anything else the stage's tooling needs. One workspace per stage; a new stage gets a new workspace forked from the committed sha it starts from. |
| **Session** | The one agent session working the task's current stage, in that stage's workspace, on its own branch. A stage may run more than one session over time (a loop back, a rerun, a resume), never two at once in V1. |
| **Result** | What a session records at the end of its work: a status and a message, optionally naming artifacts. The engine attaches the committed sha and the session reference. |
| **Ledger** | The task's durable, ordered record on disk: results, transitions with who operated them, plan changes, and inputs delivered by tools. What a later stage or a dependent task reads. |
| **Artifact** | Content a session produced for people to look at or for another stage to consume: mockup, plan, brief, test report, video, PR reference. Hashed, shareable, stored outside the ledger. Commits are artifacts identified by sha. |
| **Dependency** | An edge from one task at a stage to another task's stage. |
| **Workflow** | A template that stamps a task's initial stage chain: for each stage its role, entry (manual/auto), budget, prompt. |
| **Policy** | Configuration: repo `.kanna/config.json` (team), `config.local.json` (person), agent definition layering, storage and retention choices. |

### Ledger versus artifacts

The ledger is the record of what happened to a task: small, text, one per task, kept on the task's owning machine, moved with the task. Artifacts are things produced: any size, identified by hash, shared with people, retained by policy, kept in an artifact repository. A result in the ledger may name artifacts by hash; an artifact never contains ledger entries. Comments and decisions people make *about* an artifact travel with the artifact (they are needed wherever it is viewed); a transition someone operated is in the ledger (it is about the task).

## 3. Structure versus policy

The engine guarantees structure. Everything in the right column is configurable and has a sensible default. **Owner:** make sure the structure is there; the implementation is up to the policy of the user.

| Structure (engine) | Policy (repo, person, defaults) |
|---|---|
| A task has stages; every session ends by recording one result; every transition is recorded with its operator | Which stages a workflow has; which transitions are manual |
| One workspace per stage; one agent session in it at a time; each session on its own branch | Provider, model, effort; setup and teardown commands |
| Branch counter `task-<id>-<n>` is monotonic per task and never reused | Whether finished worktrees are kept, and for how long |
| The ledger is stored on disk under `~/.kanna/repos/<repo-id>/tasks/<task-id>/` and indexed locally | None. The ledger is internal message passing between stages; the real outputs are commits, PRs and artifacts. **Owner.** |
| Artifacts are identified by hash; a result's hashes must resolve while the task is open | Where artifact bytes live (separate artifact git repo by default), retention (keep, 30 days, discard on close) |
| Dependencies are edges from a task-at-stage to a stage; readiness is computed by the engine; a superseded upstream result is recorded and notified | What to do about it: the task manager's job, not the engine's. **Owner.** |
| A gate records who operated it (declared role plus verified channel) | Who may operate which gate |
| Parallel work is subtasks, each in its own workspace with its own setup; the parent's session is not ready to continue until its subtasks' results exist | How many subtasks, which specialties. Setup cost is the repo's to make cheap, not the engine's to share. **Owner.** |
| Agent definitions resolve as bundled base, repo layer, person layer | The content of each layer |
| One owning machine per task; actions from the same account's other machines are messages to it; nothing crosses accounts except artifacts | Transfer trigger and destination; artifact remote |

## 4. Task

- `id` stable for life. `title` human-facing. `origin_prompt` kept as history.
- The task's terms are the origin prompt read together with the ledger; where a later result says the work changed direction, the result governs. **Owner:** the origin prompt is not the holy word; the goal of a task changes as iterations go. No separate field is introduced for this; the ledger is the mechanism.
- Links: `parent` (a genuine subtask relation), `dependencies` (edges, §9), `pr` (url + head sha once a PR exists).
- `owning_machine`: exactly one at a time (§11).
- A task is closed by the transition out of its final stage or by an explicit close; close is refused while subtasks are open.

## 5. Stage, session, transition

- Stage fields (from the workflow template): `name`, `role` (agent definition), `entry: manual | auto`, `budget` (agent-initiated loops back into this stage, default 5), `prompt` (bound with `$RESULT`, `$RESULT[stage]`, `$BRANCH`, `$BASE`, `$ARTIFACT[name]`).
- **Sessions record results; transitions exit stages.** **Owner.** A session records its result at the end of its work. That completes the session, not the stage. The stage exits only on a transition:
  - `auto`: the transition fires when a result with status `success` is recorded and every dependency edge into the next stage is satisfied.
  - `manual`: the transition fires when a person or an agent advances the task; the operator is recorded. Until then the task stays in the stage: the person may keep working with the session, send it more input, and have it record again; the latest result is what the next stage receives.
- Any status other than `success` (`unverified`, `partial`, `needs-input`, `declined`, `failure`) leaves the task in the stage with the message visible and never fires an automatic transition.
- Loops: a transition back to an earlier stage (a person going back, or an agent requesting revision with findings) starts a new session of that stage; the findings are a result in the ledger that the new session reads. Agent-initiated loops count against the stage budget; a person may loop beyond it and the count resets. **Owner:** the two loop shapes are iterate within a stage, and go back to an earlier stage and come forward again (mockup → stakeholder review → mockup → stakeholder review); the autonomous implement/review loop is typically three to five rounds.
- Stage ordering is linear per task. Something needed *before* the current stage is a new task from this task's branch with this task's ledger as input, never a stage inserted earlier. **Owner (2026-09-19).** Stages may be appended by replacing the remaining plan (§10).
- **Commit step of a transition.** **Owner.** ⌘S on an implementation stage means "commit, then advance." A stage may declare `exit: commit`. When its transition is requested (manual advance or auto), the engine sends the live session one instruction, commit your work and record your result, in the same session and workspace so nothing is lost; the transition fires on that result. If the session is dead, a short commit session runs in the same workspace instead. This is the only mechanism by which uncommitted work reaches the boundary; it is part of the transition, not a stage.

## 6. Session

- Spawned for a stage; one per task at a time. Entering a new stage creates that stage's workspace: a fresh worktree forked from the committed sha in the previous stage's latest success result (or the base ref for the first stage), with setup run per policy. Every session gets its own branch `task-<id>-<n>`, `n` the task's next counter value, checked out in the stage's workspace. Provider, model, effort and provider session id are recorded on the session.
- **Loop back**: a transition back to an earlier stage reuses that stage's workspace. The new session checks out a new branch there (the counter increments; an earlier branch is never reused) and, where the provider allows, resumes the previous session's conversation, which works because the working directory is unchanged. **Owner.**
- **Name**: stage-scoped, set by the stage template from the task title and stage (for example `Implement: <title>`); the agent may rename its session. Task lists show task titles; session views show session names. **Owner.**
- **Resume**: opportunistic. If the provider supports it and the transcript and workspace are present, the new session resumes; otherwise it starts fresh from the ledger. Transfer bundles transcripts best-effort; the design never depends on them.
- **Transcript**: a reference (provider, session id, path) recorded on the session. Later sessions may search it for a specific fact; it is not an input. **Owner:** review does not read the whole transcript.
- **Workspace lifetime**: a stage's workspace lives while the task can still loop back to that stage or its session is resumable; cleanup afterwards is policy. Committed work is the only thing that crosses to the next stage; uncommitted work left in a workspace is lost when it is cleaned up.
- The operator's shell and editor open in the current stage's workspace; they are tools on the workspace, not sessions of the task. A stakeholder looking at a mockup is an artifact share (§8), not a session of the task.

## 7. Result and ledger

- **Owner:** agents record their result at the end of their work through the existing result call; that record is the ledger entry, and the previous input ledger is retired.
- Result fields written by the agent: `status` (one of six), `message` (first line is the one-line summary surfaces show; the rest is what the agent reports: what was done, what changed direction and why, what the next stage must know), `artifacts` (name → hash, optional). Fields attached by the engine: branch and committed sha, session reference, timestamp.
- The engine refuses a result with an empty message. It does not mandate sections; agent definitions say what a good message for that role contains.
- Ledger storage: `~/.kanna/repos/<repo-id>/tasks/<task-id>/` holding `task.json` (prompt, template, links) and an ordered `ledger/` of entries: `NNNN-result.md`, `NNNN-transition.json` (from, to, operator, declared role, verified channel), `NNNN-plan.json` (replacement, source), `NNNN-input.md` (tool-delivered input with provenance; terminal typing is in the transcript, not here). **Owner:** the ledger is internal message passing between stages and lives only here; nobody outside the machine reads it. The real outputs are commits, PRs and artifacts.
- Binding: `$RESULT` is the previous stage's latest success result; `$RESULT[stage]` the latest success result of a named stage on this task (this replaces any stamped plan context); `$LEDGER` is the ledger path, for sessions that need history beyond the last result.

## 8. Artifacts

- Identity is the content hash. A new version is a new hash with a `previous` link. A decision or comment names the hash it was about.
- Types: commit (sha, a branch tip), document (markdown, html), mockup (html by default; **Owner:** text on a PNG is an anti-pattern, put the text in HTML), media (png, video), report (test results), pr (url + head sha), decision (who, what, about which hash), comment (anchored by position and excerpt).
- Storage default: one **artifact repository**, a git repo outside the working repo, per working repo (`~/.kanna/repos/<repo-id>/artifacts.git` or a policy-named remote). Each artifact revision is a commit. Binaries never enter the working repo. **Owner.** Text artifacts may additionally be committed in the working repo by policy.
- Sharing: **machines are never paired across accounts** (**Owner**). An artifact reaches another person through an **artifact remote** both can reach: a git remote for the artifact repository. Sending is pushing the revision to the remote and telling the person the hash; receiving is fetching and opening it at the hash. Their comments and decisions are pushed back to the same remote, next to what they are about. Hosting the remote is ordinary git hosting: a bare repository on any machine the team can reach over ssh, or a private repository on a git host the team already uses. An organization may later run a Kanna daemon on a server as its artifact host with per-account push permission; that is a product after V1, not a requirement of it. Same-account machines may continue to exchange artifacts over the existing peer channel.
- Retention: keep, 30 days after the task closes, or discard on close; policy. A result whose artifact hash no longer resolves reads as "produced, no longer retained."

## 9. Dependencies, subtasks, readiness

- Edge: `(task A, stage X) → (task B, stage Y)`. B's stage Y is not ready until A has left stage X with a success result (for A's final stage, until A is closed). B's session forks from the sha in that result when the edge is the stage's base (stacked branches); several incoming commits are merged by B's session, not the engine.
- Subtasks: a session may create child tasks from its current sha (each a task with its own session, worktree, branch); the parent task is blocked until every child has recorded a result, then the parent's session continues with the children's results as inputs and combines them (merge, choose, aggregate verdicts). Best-of-N is N children from the same sha and a parent that picks. The engine computes readiness; the combination is the parent session's job. **Owner:** task and session stay one-to-one; parallelism is subtasks.
- Superseded inputs: when an upstream stage records a newer success result after a downstream stage consumed the old one, the engine records it on the edge and emits an event. What to do about it is the task manager's job. **Owner.** The engine takes no action of its own.
- Cycles are refused at edge creation.

## 10. Workflows and plan replacement

- A workflow is a JSON template: `name`, `description` (a context sentence saying when to use it), `budget` default, `stages[]` with the fields in §5. Templates live in the bundled defaults and in repo `.kanna/workflows/`, with `config.json`/`config.local.json` choosing which applies.
- A task's **remaining plan** is the list of stages not yet entered. It is replaced wholesale through one call, fenced on the plan the caller read; the only validation is that the current stage survives with its role and that every stage resolves. Recorded history is in the ledger, not in the plan, so nothing else needs protecting. A plan stage publishes the stages it chose by the same call when it records its result.
- Initial lineup by pattern of work (descriptions are context sentences; stage roles in order; `M` manual entry, `A` auto):
  - **mechanical**: implement → pr(M). The test suite is the reviewer.
  - **shaped**: implement → review(A) → pr(M); implement/review loop under budget.
  - **planned**: plan → implement(M) → review(A) → pr(M); review may loop to plan or implement.
  - **designed**: mockup → stakeholder(M) → plan(M) → implement(A) → review(A) → pr(M) → pr-review(M). Mockup and stakeholder iterate; humans read at the mockup, the stakeholder outcome, and the PR brief. This is the front-loaded flow.
  - **research**: research — parks at its manual gate with a brief; the plan stage is appended when the owner chooses.
  - **specialized-reviewers** stays a public workflow: shaped, with a review stage whose session dispatches specialty review subtasks. Verified 2026-09-22: the dispatcher creates child tasks (222 on the owner's machine under 15 parents), and the same dispatching review stage has also run inside single-reviewer and no-review tasks, so the workflow and "a review stage that dispatches" are the same thing; the name is what makes it selectable. **Owner.**

## 11. State ownership and machines

- **Database**: local to the machine; an index of that machine's tasks (id, current stage, readiness, live session registry, pending transitions) plus statistics (time per stage, idle time, transitions, provider usage). Everything except live session state and statistics is rebuildable by scanning the task directories. **Owner:** the database must not mean anything at the organizational level.
- **Disk**: repo `.kanna/` for definitions and config (parts tracked, parts untracked as the repo chooses); `~/.kanna/repos/<repo-id>/tasks/<task-id>/` for the ledger; the artifact repo for artifacts.
- **One owning machine per task.** Actions from the same account's other machines (operate a gate, send input, add an edge) are messages delivered to the owner and recorded there with the sender's verified channel identity. No cross-machine action while the owner is unreachable. **Nothing crosses an account boundary except artifacts** (§8): another account never operates a gate, sends input, or reads a ledger; their decision arrives as an artifact and the owner, or the owner's task manager, acts on it.
- **Transfer** moves a task: its directory, branches, and transcripts best-effort; the destination becomes owner; the source record closes.
- **Provenance**: every transition, result, input and plan replacement records a declared role (operator, manager, agent, auto) and a verified channel (local process, paired device, peer desktop, relay-attested account). The two are never merged; absent verification is recorded as unknown, never as the owner.

## 12. Agent definitions

- Layers: bundled default → repo `.kanna/agents/<name>/` (team) → person (`config.local.json`). Repo config selects which agent runs which stage.
- Formula, four sections, 15 to 40 lines, nothing mechanical:

```
---
name: <role>
role: <one sentence>
providers: <ordered candidates>
---
## Produces      the result message and artifacts this role leaves behind, and whether it commits
## Reads         what it takes as input; what it may search but not read
## Must not      three or four prohibitions
## Stop when     the conditions for a non-success status and what to say
```

- Rule: if the engine changing would make a line wrong, the line is not policy and does not belong in a definition. Waiting, joining, budgets, result binding and naming rules move to the engine or to the single environment preamble Kanna injects.

## 13. Interfaces (semantics, not wire shapes)

- `create_task {repo, prompt, workflow | plan, base, parent?, dependencies?}`
- `advance {task, to?, source}` — operate a manual transition, forward or back; records operator provenance.
- `record_result {task, status, message, artifacts?}` — ends the session's work; the engine attaches sha and session; may carry `remaining_plan` for a plan stage.
- `replace_plan {task, expected, remaining_plan, source}`
- `create_subtasks {task, specs[]}` and `add_dependency {from: (task, stage), to: (task, stage)}`
- `publish_artifact {task, type, bytes | path}` → hash; `share_artifact {hash, to: identity}`; `record_decision {hash, decision, by}`
- `send_input {task, text, source}` — typed into the session; recorded in the ledger with provenance because tool-delivered input is the only input not visible in the terminal.
- `wait {task | repo, until}` — returns when a result is recorded, a gate becomes operable, a dependency becomes ready, or a task needs a human.
- `transfer_task {task, to}`

## 14. Multi-user, iteration one

Two accounts on two of the owner's machines (one signed in as each identity), no pairing between them. What must work: both machines configured with the same artifact remote (a bare git repository the owner hosts, or a private repository on a git host); a mockup produced in a stage on machine A, pushed to the remote, opened on machine B at its hash; a comment and a decision recorded on B and pushed back; machine A fetching them, showing them against the mockup, and the owner operating the gate on A. Out of scope: cross-account pairing or task control, cross-account notifications through the relay (the sender tells the recipient the hash), shared task state, org-level storage.

## 15. Retired by this specification

The input ledger as it exists, recording tool-delivered input only (**Owner**; replaced by the task ledger of §7); reusing an earlier branch on a loop (**Owner:** an incrementing counter instead, in the same workspace); the origin prompt as the task's authority (**Owner**); posts as a stage-like concept (the commit step is a property of a transition, §5); a stamped plan context and hard-coded stage recipes (replaced by `$RESULT[stage]` and plan replacement); separate resume, rerun and revision mechanisms (all are a new session of a stage, with resume opportunistic); the engine-level distinction between a blocker and a stage dependency.

## 16. Independent components for the planner

Listed so the planning stage can see what can proceed in parallel; no order or estimate is implied.

1. Task directory and ledger: result call contract, ledger entry files, `$RESULT`, `$RESULT[stage]`, `$LEDGER` binding.
2. Workspace and session model: workspace per stage (worktree, ports, setup), one session at a time, branch per session with a monotonic counter, loop-back reuse of the stage's workspace, opportunistic resume, session naming, transcript reference.
3. Artifact repository: hash identity, types, publish/share/decision/comment records, retention policy, peer-channel fetch.
4. Dependency edges, subtask readiness, staleness notification, cycle refusal.
5. Workflow template schema, plan replacement with the single validation rule, the initial lineup, transition semantics (result completes a session; transition exits a stage).
6. Database reduction to index plus statistics; rebuild-from-disk; provenance pair on every mutation.
7. Agent definition formula applied to every bundled definition; environment preamble absorbing the mechanics.
8. UI: session names in a sessions view, gate screen showing the latest result and artifacts, dependency and staleness display, artifact viewer with comments.
9. Artifact remote configuration and the two-account iteration (§14).
10. Migration of open tasks and the decision of what existing code each component reuses.

## 17. Questions resolved by the owner, 2026-09-22

- Superseded upstream results: the engine records and notifies; the task manager handles it. No hold or re-run machinery.
- Subtask setup: no sharing of setup output across workspaces; each subtask has its own workspace; repos are expected to make their setup cheap.
- Ledger: lives under `~/.kanna` only; it is internal message passing between stages and its exact form does not matter. The real outputs are commits, PRs and artifacts.
- `specialized-reviewers`: stays public; a review stage that dispatches subtasks is the same thing.
- Cross-account: no pairing; artifacts only, through a shared artifact remote.
- Commit before a boundary: the transition's commit step instructs the live session in place (§5).
