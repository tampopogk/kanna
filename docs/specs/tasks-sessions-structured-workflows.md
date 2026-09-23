# Specification: tasks, sessions, and structured workflows (target architecture)

Status: owner-directed target design, 2026-09-22, produced by research task `482a02db`. It describes what Kanna should be, not what it is. The planning stage that follows decomposes it into buildable tasks and decides what existing code is reused; nothing here prescribes that. Statements marked **Owner** were decided by the owner in this task's discussion or earlier and are not open. Everything else is the design the owner accepted as the basis for planning.

## 1. Goals

1. Structured workflows: a task moves through stages; each transition is manual or automatic; humans read at few, deliberate points (design accepted, stakeholders satisfied, PR brief) and agents do the work between.
2. One agent per stage in V1. Parallel work is subtasks, each a task with its own session. **Owner.** (Deferred, not precluded: several agents in one stage's workspace, one writing and others reviewing the same worktree.)
3. Artifacts pass from stage to stage and can be shared with other users, including users on other machines and accounts.
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
| The ledger is stored on disk under `~/.kanna/repos/<repo-id>/tasks/<task-id>/` and indexed locally | Whether ledger text is additionally committed on the task branch |
| Artifacts are identified by hash; a result's hashes must resolve while the task is open | Where artifact bytes live (separate artifact git repo by default), retention (keep, 30 days, discard on close) |
| Dependencies are edges from a task-at-stage to a stage; readiness is computed by the engine | What happens when an upstream result is superseded: notify (default), hold, or re-attempt |
| A gate records who operated it (declared role plus verified channel) | Who may operate which gate |
| Parallel work is subtasks; the parent's session is not ready to continue until its subtasks' results exist | How many subtasks, which specialties |
| Agent definitions resolve as bundled base, repo layer, person layer | The content of each layer |
| One owning machine per task; actions from elsewhere are messages to it | Transfer trigger and destination |

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
- Commit-before-transition: a stage whose work must be committed before it can leave (implement) has its agent commit as part of finishing its work, per the agent definition; the engine records the committed sha on the result. There is no separate post mechanism.

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
- Ledger storage: `~/.kanna/repos/<repo-id>/tasks/<task-id>/` holding `task.json` (prompt, template, links) and an ordered `ledger/` of entries: `NNNN-result.md`, `NNNN-transition.json` (from, to, operator, declared role, verified channel), `NNNN-plan.json` (replacement, source), `NNNN-input.md` (tool-delivered input with provenance; terminal typing is in the transcript, not here). Policy may additionally commit the ledger on the task branch.
- Binding: `$RESULT` is the previous stage's latest success result; `$RESULT[stage]` the latest success result of a named stage on this task (this replaces any stamped plan context); `$LEDGER` is the ledger path, for sessions that need history beyond the last result.

## 8. Artifacts

- Identity is the content hash. A new version is a new hash with a `previous` link. A decision or comment names the hash it was about.
- Types: commit (sha, a branch tip), document (markdown, html), mockup (html by default; **Owner:** text on a PNG is an anti-pattern, put the text in HTML), media (png, video), report (test results), pr (url + head sha), decision (who, what, about which hash), comment (anchored by position and excerpt).
- Storage default: one **artifact repository**, a git repo outside the working repo, per working repo (`~/.kanna/repos/<repo-id>/artifacts.git` or a policy-named remote). Each artifact revision is a commit. Binaries never enter the working repo. **Owner.** Text artifacts may additionally be committed in the working repo by policy.
- Sharing: sending an artifact to a person is fetching the artifact repo over the existing peer channel and opening it for them at the hash. Their comments and decisions come back as artifacts in the same repo, next to what they are about.
- Retention: keep, 30 days after the task closes, or discard on close; policy. A result whose artifact hash no longer resolves reads as "produced, no longer retained."

## 9. Dependencies, subtasks, readiness

- Edge: `(task A, stage X) → (task B, stage Y)`. B's stage Y is not ready until A has left stage X with a success result (for A's final stage, until A is closed). B's session forks from the sha in that result when the edge is the stage's base (stacked branches); several incoming commits are merged by B's session, not the engine.
- Subtasks: a session may create child tasks from its current sha (each a task with its own session, worktree, branch); the parent task is blocked until every child has recorded a result, then the parent's session continues with the children's results as inputs and combines them (merge, choose, aggregate verdicts). Best-of-N is N children from the same sha and a parent that picks. The engine computes readiness; the combination is the parent session's job. **Owner:** task and session stay one-to-one; parallelism is subtasks.
- Staleness: when an upstream stage records a newer success result after a downstream stage consumed the old one, the engine marks the edge stale and notifies; holding or re-running is policy (default: notify only).
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
  - A review panel is a review stage whose session dispatches subtasks; not a separate public workflow.

## 11. State ownership and machines

- **Database**: local to the machine; an index of that machine's tasks (id, current stage, readiness, live session registry, pending transitions) plus statistics (time per stage, idle time, transitions, provider usage). Everything except live session state and statistics is rebuildable by scanning the task directories. **Owner:** the database must not mean anything at the organizational level.
- **Disk**: repo `.kanna/` for definitions and config (parts tracked, parts untracked as the repo chooses); `~/.kanna/repos/<repo-id>/tasks/<task-id>/` for the ledger; the artifact repo for artifacts.
- **One owning machine per task.** Actions from another machine or account (operate a gate, send input, add an edge, share an artifact) are messages delivered to the owner and recorded there with the sender's verified channel identity. No cross-machine action while the owner is unreachable.
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

Two accounts on two of the owner's machines (one signed in as each identity) forming one organization. What must work: pairing across accounts; the artifact repo fetched over the peer channel; a mockup opened on the second machine at its hash; a comment and a decision recorded back; a gate on the owning machine operated by the second identity through a message and recorded with that identity. Out of scope: shared task state, offline cross-machine actions, org-level storage.

## 15. Retired by this specification

The input ledger as it exists, recording tool-delivered input only (**Owner**; replaced by the task ledger of §7); reusing an earlier branch on a loop (**Owner:** an incrementing counter instead, in the same workspace); the origin prompt as the task's authority (**Owner**); posts as a distinct concept (committing is part of the agent's work); a stamped plan context and hard-coded stage recipes (replaced by `$RESULT[stage]` and plan replacement); separate resume, rerun and revision mechanisms (all are a new session of a stage, with resume opportunistic); the engine-level distinction between a blocker and a stage dependency.

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
9. Multi-user iteration one (§14).
10. Migration of open tasks and the decision of what existing code each component reuses.

## 17. Open questions (do not block planning)

- Default for a stale dependency beyond notify.
- Whether setup output can be shared across worktrees so subtask fan-out is not N full setups.
- Whether the ledger is committed on the task branch by default in this repo.
- Whether `specialized-reviewers` remains a public workflow or becomes a review stage whose session dispatches subtasks.
