# Specification: tasks, sessions, and structured workflows (target architecture)

Status: owner-directed target design, 2026-09-22, produced by research task `482a02db`. It describes what Kanna should be, not what it is. The planning stage that follows decomposes it into buildable tasks and decides what existing code is reused; nothing here prescribes that. Statements marked **Owner** were decided by the owner in this task's discussion or earlier and are not open. Everything else is the design the owner accepted as the basis for planning.

## 1. Goals

1. Structured workflows: a task moves through stages; each transition is manual or automatic; humans read at few, deliberate points (design accepted, stakeholders satisfied, PR brief) and agents do the work between.
2. Plural sessions: more than one agent can work a task's stage in parallel, and people can attach their own sessions, without the engine reconciling their outputs.
3. Artifacts pass from stage to stage and can be shared with other users, including users on other machines and accounts.
4. Dependencies between tasks, including a task depending on another task at a specific stage.
5. Front-loaded design: iterate with an agent on a mockup, get stakeholders to agree, then let agents implement and test so the PR comes out right.
6. Simple, formulaic agent definitions; sensible defaults; policy overridable per repo and per user.
7. The database stays local to one machine and never has to converge with another. **Owner.**

Non-goals for this iteration: a cloud or organizational database, CRDTs, engine-computed merging of parallel sessions' outputs, drift detection at gates (**Owner:** try it, fix it if it is a problem), transcript resume across machines as something the design depends on (**Owner:** transfer is unreliable; resume is opportunistic).

## 2. Vocabulary

| Word | Meaning |
|---|---|
| **Task** | A durable unit of intent: id, title, origin prompt, **current goal**, stage chain, links (parent, dependencies), owning machine, PR link. |
| **Stage** | One step of a task with a role. Has attempts. Entered manually or automatically. Exits when its working session records a **result**. |
| **Attempt** | One execution of a stage. A loop back to a stage is a new attempt. Attempts per stage are budgeted. |
| **Working session** | The one agent session that is doing a task's current attempt. Owns a worktree and a branch. Writes the handoff. At most one per task at a time. |
| **Auxiliary session** | A session attached to a task or artifact that produces no handoff: the operator's shell or editor, a stakeholder viewing a mockup, a PR reviewer briefing a human. |
| **Handoff** | The stage result: status plus a structured message. The only thing a stage passes to the next stage or to a dependent task. |
| **Artifact** | An immutable, hashed, typed thing a stage produced and that can be shared: mockup, plan, brief, test report, video, PR reference. Commits are artifacts identified by sha. |
| **Dependency** | An edge from one task's stage handoff to another task's stage. Within a task, stage N feeds N+1 implicitly. |
| **Gate** | A manual transition. Operated by a human or by an agent; the record says which. Automatic transitions are not gates. **Owner.** |
| **Workflow** | A template that stamps a task's initial stage chain: for each stage its role, entry (manual/auto), session policy, budget. |
| **Policy** | Configuration: repo `.kanna/config.json` (team), `config.local.json` (person), agent definition layering, storage and retention choices. |

## 3. Structure versus policy

The engine guarantees structure. Everything in the right column is configurable and has a sensible default. **Owner:** make sure the structure is there; the implementation is up to the policy of the user.

| Structure (engine) | Policy (repo, person, defaults) |
|---|---|
| A task has stages; each stage has attempts; every attempt exits through one result call that produces a handoff | Which stages a workflow has; which transitions are manual |
| One working session per task at a time; it owns exactly one worktree and one branch | Provider, model, effort; setup and teardown commands |
| Branch counter `task-<id>-<n>` is monotonic per task and never reused | Whether finished worktrees are kept, and for how long |
| Handoffs are stored on disk under `~/.kanna/repos/<repo-id>/tasks/<task-id>/` and indexed locally | Whether handoff text is additionally committed on the task branch |
| Artifacts are identified by hash; a handoff's hashes must resolve while the task is open | Where artifact bytes live (separate artifact git repo by default), retention (keep, 30 days, discard on close) |
| Dependencies are edges from a handoff to a stage; readiness is computed by the engine | What happens when an upstream handoff is superseded: notify (default), hold, or re-attempt |
| A gate records who operated it (declared role plus verified channel) | Who may operate which gate |
| Parallel work is subtasks; the parent stage is not ready until its subtasks' handoffs exist | How many subtasks, which specialties, best-of-N or shard |
| Agent definitions resolve as bundled base, repo layer, person layer | The content of each layer |
| One owning machine per task; actions from elsewhere are messages to it | Transfer trigger and destination |

## 4. Task

- `id` stable for life. `title` human-facing. `origin_prompt` immutable history.
- `current_goal`: rewritten by every handoff's `goal` field. This is what the next session is told to do. **Owner:** the origin prompt is not the holy word; the goal of a task changes as iterations go.
- Links: `parent` (a genuine subtask relation), `dependencies` (edges, §9), `pr` (url + head sha once a PR exists).
- `owning_machine`: exactly one at a time (§11).
- A task is closed by its final stage's exit or by an explicit close; close is refused while subtasks are open.

## 5. Stage and attempt

- Stage fields (from the workflow template): `name`, `role` (agent definition), `entry: manual | auto`, `session_policy: single | subtasks(n) | human`, `budget` (max attempts, default 5), `prompt` (bound with `$GOAL`, `$HANDOFF`, `$BRANCH`, `$BASE`, `$ARTIFACT[name]`).
- Entry: an `auto` stage starts when the previous stage's handoff has status `success` and every dependency edge into it is satisfied. A `manual` stage waits for a gate operation.
- Exit: the working session calls the result API once: `status` (`success | unverified | partial | needs-input | declined | failure`) plus the handoff message (§7). Only `success` lets an `auto` next stage start. Any other status parks the task at the stage with the message visible.
- Attempts: entering an earlier stage that already ran (a loop) creates a new attempt of that stage. Attempts count against the stage's budget; a human may start an attempt beyond the budget and the budget resets. **Owner:** the two loop shapes are iterate within a stage, and go back to an earlier stage and come forward again (for example mockup → stakeholder review → mockup → stakeholder review); the autonomous implement/review loop is typically three to five attempts.
- Stage ordering is linear per task. Something needed *before* the current stage is a new task from this task's branch with this task's latest handoff as input, never a stage inserted earlier. **Owner (2026-09-19).** Stages may be appended to a task at any time by replacing its remaining plan (§10).

## 6. Sessions

- **Working session**: spawned for an attempt; one per task at a time. Gets a fresh worktree forked from the committed sha named in the input handoff (or the base ref for the first stage), on branch `task-<id>-<n>` with `n` the task's next counter value. Setup runs in the new worktree per policy. The session's provider, model, effort and provider session id are recorded on the attempt.
- **Name**: stage-scoped, derived from the stage template and the current goal (for example `Implement: <goal, first line>`); the agent may rename its session. Task lists show task titles; session views show session names. **Owner.**
- **Resume**: if the provider supports it and the transcript and worktree are present, a new attempt of the same stage may resume the previous working session in its worktree; otherwise it starts fresh from the handoff. Resume is opportunistic. Transfer bundles transcripts best-effort but the design never depends on them.
- **Transcript**: a reference (provider, session id, path) recorded on the attempt. It is searchable by later sessions for a specific fact; it is not an input. **Owner:** do not have review read the whole transcript.
- **Auxiliary sessions**: operator shell and editor in the task's current worktree; stakeholder viewer sessions against an artifact hash (possibly on another machine); PR reviewer briefing sessions. They produce comments and decisions, never a handoff.
- **Worktree lifetime**: a worktree lives while its session is live or resumable; cleanup afterwards is policy. Committed work is the only thing that crosses to the next session; uncommitted work in an abandoned worktree is lost by design.

## 7. Handoff

The stage result is the handoff. One API call carries it; the engine writes it to disk, indexes it, and binds it into the next stage. **Owner:** use the result call as the ledger; the input ledger in its current form is removed.

Message shape (sections, plain text; the first `done` line is the one-line summary surfaces show):

```
goal:        what this task is now, one paragraph; rewrites current_goal
done:        what happened in this attempt
changed:     decisions or directions that changed during the attempt, and why
next:        what the next stage must do, and must not do
branch:      task-<id>-<n> @ <sha>
artifacts:   name → hash (shareable things produced this attempt), optional
transcript:  provider, session id, path (local reference), filled by the engine
subtasks:    ids of subtask handoffs consumed, optional
```

- Written by the working agent that did the work, because it has the context. **Owner.** The engine refuses an exit whose message lacks `goal`, `done`, `next` or `branch`.
- Storage: `~/.kanna/repos/<repo-id>/tasks/<task-id>/stages/<nn>-<stage>/attempt-<k>/handoff.md` plus `attempt.json` (status, provider stamp, session reference, timestamps, gate operator for the entry). Policy may additionally commit `handoff.md` on the task branch.
- Binding: `$HANDOFF` is the previous stage's handoff; `$HANDOFF[stage]` the latest successful handoff of a named stage on this task (this replaces any special plan-result mechanism); `$GOAL` is `current_goal`.

## 8. Artifacts

- Identity is the content hash. A new version is a new hash with a `previous` link. A decision or comment names the hash it was about.
- Types: commit (sha, a branch tip), document (markdown, html), mockup (html by default; **Owner:** text on a PNG is an anti-pattern, put the text in HTML), media (png, video), report (test results), pr (url + head sha), decision (who, what, about which hash), comments (anchored by position and excerpt).
- Storage default: one **artifact repository**, a git repo outside the working repo, per working repo (`~/.kanna/repos/<repo-id>/artifacts.git` or a policy-named remote). Each artifact revision is a commit. Binaries never enter the working repo. **Owner.** Text artifacts may additionally be committed in the working repo by policy.
- Sharing: sending an artifact to a person is fetching the artifact repo over the existing peer channel and opening an auxiliary session for them against the hash. Their comments and decision come back as artifacts in the same repo.
- Retention: keep, 30 days after the task closes, or discard on close; policy. A handoff whose artifact hash no longer resolves reads as "produced, no longer retained."

## 9. Dependencies, subtasks, readiness

- Edge: `(task A, stage X) → (task B, stage Y)`. B's stage Y is not ready until A's stage X has a `success` handoff. A whole-task blocker is an edge from A's final stage. B's first working session forks from the sha in that handoff when the edge is the stage's base (stacked branches); multiple incoming commits are merged by B's working session, not the engine.
- Subtasks: a stage with `session_policy: subtasks` creates child tasks from its current sha (each with its own working session, worktree, branch); the parent stage's own working session is blocked until every child has a handoff, then runs with the children's handoffs as inputs and combines them (merge, choose, aggregate verdicts). Best-of-N is N children from the same sha and a parent that picks. The engine computes readiness; the combination is the parent session's job. **Owner:** task = session stays one-to-one; parallelism is subtasks.
- Staleness: when an upstream handoff gains a newer successful attempt after a downstream stage consumed it, the engine records the edge as stale and notifies; holding or re-attempting is policy (default: notify only).
- Cycles are refused at edge creation.

## 10. Workflows and plan replacement

- A workflow is a JSON template: `name`, `description` (a context sentence saying when to use it), `budget` default, `stages[]` with the fields in §5. Templates live in the bundled defaults and in repo `.kanna/workflows/`, with `config.json`/`config.local.json` choosing which applies.
- A task's **remaining plan** is the list of stages not yet entered. It is replaced wholesale through one call, fenced on the plan the caller read; the only validation is that the current stage survives with its role and that every stage resolves. Recorded attempts are history on disk and are not part of the plan, so nothing else needs protecting. A plan stage publishes the stages it chose by the same call in the same exit as its result.
- Initial lineup by pattern of work (descriptions are context sentences; stage roles in order; `M` manual entry, `A` auto):
  - **mechanical**: implement(M start) → pr(M) — the test suite is the reviewer.
  - **shaped**: implement → review(A) → pr(M); implement/review loop under budget.
  - **planned**: plan(M gate after) → implement(A) → review(A) → pr(M); review may loop to plan or implement.
  - **designed**: mockup(M) → stakeholder(M) → plan(A) → implement(A) → review(A) → pr(M) → pr-review(M). Mockup and stakeholder iterate; humans read at the mockup, the stakeholder outcome, and the PR brief. This is the front-loaded flow.
  - **research**: research(M) — parks with a brief; the plan stage is appended when the owner chooses.
  - Review panels are a review stage with `session_policy: subtasks`; not a separate public workflow.

## 11. State ownership and machines

- **Database**: local to the machine; an index of that machine's tasks (id, current stage and attempt, readiness, live session registry, gate operations) plus statistics (time per stage, idle time, transitions, provider usage). Everything except live session state and statistics is rebuildable by scanning the task directories. **Owner:** the database must not mean anything at the organizational level.
- **Disk**: repo `.kanna/` for definitions and config (parts tracked, parts untracked as the repo chooses); `~/.kanna/repos/<repo-id>/tasks/<task-id>/` for task state and handoffs; the artifact repo for shareable artifacts.
- **One owning machine per task.** Actions from another machine or account (operate a gate, send input, add an edge, share an artifact) are messages delivered to the owner and recorded there with the sender's verified channel identity. No cross-machine action while the owner is unreachable.
- **Transfer** moves a task: its directory, branches, and transcripts best-effort; the destination becomes owner; the source record closes.
- **Provenance**: every gate operation, result, input and plan replacement records a declared role (operator, manager, agent, auto) and a verified channel (local process, paired device, peer desktop, relay-attested account). The two are never merged; absent verification is recorded as unknown, never as the owner.

## 12. Agent definitions

- Layers: bundled default → repo `.kanna/agents/<name>/` (team) → person (`config.local.json` selection or a person-level override directory). Repo config selects which agent runs which stage.
- Formula, four sections, 15 to 40 lines, nothing mechanical:

```
---
name: <role>
role: <one sentence>
providers: <ordered candidates>
---
## Produces      the handoff and artifacts this role must leave behind
## Reads         what it takes as input; what it may search but not read
## Must not      three or four prohibitions
## Stop when     the conditions for a non-success status and what to say
```

- Rule: if the engine changing would make a line wrong, the line is not policy and does not belong in a definition. Waiting, joining, budgets, verdict binding, ledger reading and naming rules move to the engine or to the single environment preamble Kanna injects.

## 13. Interfaces (semantics, not wire shapes)

- `create_task {repo, prompt, workflow | plan, base, parent?, dependencies?}`
- `start_stage {task, stage?, source}` — operate a manual gate or start an attempt; records operator provenance.
- `record_result {task, attempt, status, handoff}` — the exit; writes the handoff; may carry `remaining_plan` for a plan stage.
- `replace_plan {task, expected, remaining_plan, source}`
- `create_subtasks {task, stage, specs[]}` and `add_dependency {from: (task, stage), to: (task, stage)}`
- `publish_artifact {task, attempt, type, bytes | path}` → hash; `share_artifact {hash, to: identity}`; `record_decision {hash, decision, by}`
- `send_input {task, text, source}` — typed into the working session; recorded as an input with provenance (this is the residue of the old ledger: tool-delivered inputs remain recorded because they are the only ones not visible in the terminal).
- `wait {task | repo, until}` — returns when a stage exits, a gate becomes operable, a dependency becomes ready, or a task needs a human.
- `transfer_task {task, to}`

## 14. Multi-user, iteration one

Two accounts on two of the owner's machines (one signed in as each identity) forming one organization. What must work: pairing across accounts; the artifact repo fetched over the peer channel; a stakeholder auxiliary session on the second machine against a mockup hash; a comment and a decision recorded back; a gate on the owning machine operated by the second identity through a message and recorded with that identity. Out of scope: shared task state, offline cross-machine actions, org-level storage.

## 15. Retired by this specification

The input ledger as a separate record (**Owner**); reusing an earlier branch on a loop (**Owner:** an incrementing counter instead); the origin prompt as the task's authority (**Owner**); posts as a distinct concept (a commit step is a stage whose session policy continues the previous session, or part of the implementing agent's own exit); a stamped plan context and hard-coded stage recipes (replaced by `$HANDOFF[stage]` and plan replacement); separate resume, rerun and revision mechanisms (all are attempts, with resume opportunistic); the engine-level distinction between a blocker and a stage dependency.

## 16. Independent components for the planner

Listed so the planning stage can see what can proceed in parallel; no order or estimate is implied.

1. Task directory, handoff storage and the result call's message contract; `$HANDOFF`, `$HANDOFF[stage]`, `$GOAL` binding; current-goal rewriting.
2. Working session model: worktree per session, monotonic branch counter, opportunistic resume, session naming, transcript reference.
3. Artifact repository: hash identity, types, publish/share/decision/comment records, retention policy, peer-channel fetch.
4. Dependency edges, subtask readiness, staleness notification, cycle refusal.
5. Workflow template schema, plan replacement with the single validation rule, the initial lineup.
6. Database reduction to index plus statistics; rebuild-from-disk; provenance pair on every mutation.
7. Agent definition formula applied to every bundled definition; environment preamble absorbing the mechanics.
8. UI: sessions view with session names, gate screen showing the handoff and artifacts, dependency and staleness display, artifact viewer with comments.
9. Multi-user iteration one (§14).
10. Migration of open tasks and the decision of what existing code each component reuses.

## 17. Open questions (do not block planning)

- Default for a stale dependency beyond notify.
- Whether setup output can be shared across worktrees so subtask fan-out is not N full setups.
- Whether `handoff.md` is committed on the task branch by default in this repo.
- Whether `specialized-reviewers` remains a public workflow or becomes a review stage with subtasks only.
