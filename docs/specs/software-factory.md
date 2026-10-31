# Software factory

**Status:** designed, not built (the build tasks are in [§9](#9-build-tasks)) · **Mockup:** [`references/mockups/pipeline.html`](../../references/mockups/pipeline.html) (Definition, A run on the Desk, waiting on you, Runs compared; `#light`) · **Language:** [pipeline language](pipeline-language.md) · **Why this design:** [research](../research/software-factory.md) · **Builds on:** [tasks](tasks.md), [Shelf runners](shelf-runners.md), [task scripts](task-scripts.md), [agent messaging](agent-messaging.md), [database](database.md), the Stats tab (task "Stats tab on the Desk") and [code quality metrics](code-quality-metrics.md) (task "Code quality metrics")

A **pipeline** says how a change gets built: which stages it goes through (spec, your approval, plan, build, review, tests, merge), who does each one (an agent, a command, you, git), and what happens when a stage fails (a bounded loop back to a fix). A task made with a pipeline is a **run**: the daemon drives it, stage by stage, in the task's worktree and docked pane, so you watch and steer it as you would any task. Every stage's time, turns, tokens, verdicts and your interventions are kept in `kanna.db`, so pipelines can be compared on the Stats tab and tuned.

![A run on the Desk: the stage's agent docked on the left; the Run tab's graph, numbers and stage log on the right](software-factory-run.png)

## Contents

1. [What a run is](#1-what-a-run-is)
2. [Who drives it](#2-who-drives-it)
3. [Fan-out and join](#3-fan-out-and-join)
4. [Seeing and steering a run](#4-seeing-and-steering-a-run)
5. [Metrics](#5-metrics)
6. [How it's built](#6-how-its-built)
7. [CLI](#7-cli)
8. [Rejected](#8-rejected)
9. [Build tasks](#9-build-tasks)
10. [Glossary](#10-glossary)

## 1. What a run is

```mermaid
stateDiagram-v2
    direction LR
    [*] --> Shelf: kanna task new … --pipeline feature
    Shelf --> running: a runner, pick or start
    running --> waiting: ask, stuck, over budget, an agent needs you
    waiting --> running: your answer, retry, skip, edit
    running --> paused: Pause
    paused --> running: Resume
    running --> landed: the merge main stage passes, then done
    running --> stopped: Stop, or no arrow left
    waiting --> stopped: Stop
```

- **A run is a task** ([tasks](tasks.md)) made with `--pipeline NAME`: the same row, worktree, branch, Desk place, Shelf runner, stop and start, messages and Done. What differs is what runs in its docked pane: not one agent for the task's life but **the current stage**, one after another. The task's first message is the run's **request**, given to every agent stage.
- **One run per task, one task per run.** A run's children ([3](#3-fan-out-and-join)) are tasks of their own, marked as the run's.
- **A stage visit runs in the task's pane:** an agent stage starts the agent there with the stage's prompt as its first message, in a fresh session (the pane's command becomes that session's resuming one, so stop and start bring it back, as [which agents resume](tasks.md#which-agents-resume) says); a `run` stage runs its command there, as [setup](task-scripts.md#2-setup) does; an `ask` prints the question there and waits; a `merge` runs git there. The pane's header says the run, the stage, how far along and how long.
- **The turn's end moves the run on.** For an agent stage, the agent's turn ending ([Shelf runners](shelf-runners.md#2-how-its-built): its own report, or idle for 3s) ends the visit: the daemon reads the answer, quits the agent as a runner would, records the visit and picks the next arrow. An agent that stops to ask you makes the run **waiting**; your answer in its pane resumes the stage, and counts as an intervention. On the Shelf, a run keeps its runner from stage to stage; it's stopped only when it waits for you or ends.
- **The budget stops a run** that would exceed its sessions or time before the next stage starts: it waits for you, saying so.
- **A landed run** (its `merge main` stage passed and the flow reached `done`) is Done as a merged task is: teardown, then cleanup ([task scripts](task-scripts.md#3-teardown-and-cleanup)). A run that ends anywhere else stays, stopped, with its branch, for you to pick up by hand: `kanna task start` on it starts a plain agent in the worktree with the run's history as its first message.

## 2. Who drives it

```mermaid
flowchart LR
    CLI["kanna task new --pipeline<br/>kanna run …"] --> D
    APP["Kanna.app<br/>Run tab · Desk view · strip"] --> D
    subgraph D["daemon: runs.rs"]
        PIPE["pipeline: parse, validate, pick the arrow"]
        RUN["the run: current stage, visits, budget"]
        PIPE --> RUN
    end
    RUN -->|"start the stage in the task's pane"| T["tasks.rs: the task's agent or command"]
    T -->|"turn's end · exit code · answer"| RUN
    RUN --> DB[("kanna.db<br/>runs · run_revisions · run_stages · run_events")]
```

- **The daemon, not an agent.** The planning agent makes runs (`kanna task new … --pipeline`) and reads them (`kanna run list`); it never advances one. No agent sees the whole pipeline but through its stage's prompt and `{{STAGE.KEY}}` ([pipeline language §5](pipeline-language.md#5-inputs-and-outputs)); an `emits` stage proposes a revision that the daemon validates ([pipeline language §7](pipeline-language.md#7-changing-a-pipeline-while-it-runs)).
- **Driven by events, not a clock:** a stage ends when its agent's turn ends, its command exits (the keeper's exit code), you answer, or its `within` passes. The Shelf runners' tick (every 2s) is where a run's next stage is started, so a run that was waiting starts within 2s of your answer.
- **A restart of the daemon loses nothing:** the run's state is the rows; a stage mid-visit carries on (its agent still runs in the keeper's pty; a command too), and a visit whose pane is gone is recorded `interrupted` and run again.
- **One stage at a time per run,** except a fan-out's instances. Several runs run at once as tasks do: `shelf.runners` bounds how many on the Shelf, the Desk's three on the Desk.

## 3. Fan-out and join

```mermaid
flowchart LR
    P["plan<br/>answer: parts = [a, b, c]"] --> A["build · a<br/>child task, worktree from the run's branch"]
    P --> B["build · b"]
    P --> C["build · c"]
    A & B & C --> J["join = merge build<br/>in the run's worktree"]
    J -->|"conflict: fail"| F["fix"]
    J -->|"clean"| N["check"]
```

- **`each STAGE.KEY`** makes one **child task** per item: named `RUN'S NAME · ITEM`, on the Shelf, each in a worktree branched from the run's branch at that commit, its agent given the stage's prompt, the request and its item as `# Your part`. They run as Shelf tasks do, `shelf.runners` at a time, and show on the Shelf and the strip under the run's name. A child's turn ending ends its instance; a child that asks you is the run waiting on you.
- **The next stage after a fanned-out one waits for every instance.** `merge STAGE` joins their branches into the run's branch (one merge commit per instance, in item order); a conflict is a `fail` with the conflicting files as feedback, for a `fix` arrow to resolve, and the run's worktree is left at the conflict for it. Children are Done once joined, their worktrees and branches cleaned up as merged tasks'.
- **`xN by STAGE`** fans out the same way (N children named `· attempt K`), scores each with the `run` stage named, keeps the winner's branch as the run's and ends the others, recording every score.
- **Budget:** each instance is a session; a list longer than the sessions left is refused before any child starts, and the stage fails with that as feedback.

## 4. Seeing and steering a run

| The Run tab, waiting on you | Runs compared on the Stats tab |
|---|---|
| ![The approve stage waiting: the question in the agent pane and as a card on the Run tab, with Yes, No and a note](software-factory-gate.png) | ![The Stats tab's Pipelines section: a table by pipeline and revision, dots per run over 30 days, recent runs](software-factory-metrics.png) |

- **The Run tab** is a fixed first tab of a run's task (`kanna open run`, opened when the run starts): the pipeline as a graph with each stage's state and visit count, the run's numbers, the current revision, and the stages in the order they ran with their verdicts, times, turns and tokens. Clicking a visit opens what it left: its answer, the command's output, the diff of its commits.
- **Its buttons** are the CLI's verbs ([7](#7-cli)): **Pause** finishes the current stage and starts no other; **Skip stage** ends the current visit as `pass` by you; **Retry stage** runs it again; **Edit pipeline** opens the run's revision in a code pane; **Stop** ends the run where it is. An `ask` shows as a card: **Yes**, **No** with a note. Each is recorded as an intervention.
- **On the Desk** a run is a task: its header says the stage (`review 7/11 · loop 2/3 · 4m`), its dot is the stage's agent's status (needs you while an `ask` waits), the Desk view's row and the strip item say the stage, children show under it. Nothing else changes.
- **You can type into the stage's agent** as into any task's; what you type is kept with the visit as an intervention, and a stopped agent's session is the visit's.
- **The agent's own view:** a stage's agent presents its work as any task does ([presenting](presenting.md)); its tabs stay through the next stages, so when the run waits for you the tabs show the latest spec, mockup and diff.

## 5. Metrics

Every visit writes a row; everything below is a query over them, shown on the Run tab for one run and on the Stats tab's **Pipelines** section across runs, grouped by pipeline and revision (a pipeline's text hash), so a changed arrow is a new line to compare.

| Per visit | From |
|---|---|
| wall time, time waiting on you | the visit's events |
| agent turns, tool calls, tokens in, out and cached, cost | the agent's session file at the turn's end (Claude's JSONL `usage`, Codex's `token_count` events); the cost from the agent when it reports one, else the pricing table in the repo's settings |
| verdict, feedback size, review findings by severity, failing tests | the verdict; the review's `findings` block; the command's failure lines |
| commits, files and lines changed | git, between the visit's start and end |
| your interventions | answers, pauses, skips, retries, edits, messages, prompts typed into the stage's agent |
| guard before and after, rolled back; attempts and the winner | the stage's record |

| Per run | Is |
|---|---|
| time to land; active time | made to landed; the sum of visits |
| sessions, turns, tokens, cost | sums |
| rework loops | visits of any stage beyond its first, by stage |
| review findings; test failures | sums; `run` visits that failed |
| interventions | the sum, by kind |
| outcome | landed, stopped (why), stuck |
| bugs traced back; changed lines covered; quality delta | from [code quality metrics](code-quality-metrics.md): fix commits on main blamed to the run's merge (SZZ), lines its merge changed that a test executes, erosion and verbosity before and after, each sampled later and joined by the merge commit |

- **Compared by pipeline and revision:** runs, landed share, medians of the rest, and a dot per run over time, so a change to the pipeline shows as a step. The Code quality metrics task's noise caveats (SZZ at F1 about 0.6) are shown with the column.
- **`kanna run show RUN --json` and `kanna stats pipelines --json`** give the same numbers to agents, so a planning agent asked "which pipeline lands faster" can answer.

## 6. How it's built

- **`crates/kanna-pipe`:** the language, a port of kojo's `factory_spec.py` ([research](../research/software-factory.md#1-kojo); provenance in `NOTICE` once kojo has a licence): parse, validate, `pick(stage, verdict, taken, stalled)`, `worst()` for the budget, revisions (merge an emission or an edit, then validate). Pure, tested on the example files and on every error.
- **`runs.rs` in the daemon** owns runs as `tasks.rs` owns tasks: start a stage in the task's pane (an agent command built as a task's is, with the stage prompt written to `.kanna/tmp/stage-N.md` and passed as the first message; a command under the task-script timeout wrapper), watch for the visit's end, read the answer from the session file, pick the next arrow, start children, join, land.
- **Tables** in `kanna.db` ([database](database.md)), the daemon the only writer:

| Table | Columns | Doc |
|---|---|---|
| `runs` | `id` (`run-7f3a1c2e`), `task`, `repo`, `pipeline`, `revision_hash`, `state`, `started_at`, `ended_at`, `outcome` | request, budget, counts |
| `run_revisions` | `run`, `n`, `at`, `by` (`file`, `stage:NAME`, `user`), `hash` | the text, the diff |
| `run_stages` | `id`, `run`, `stage`, `visit`, `instance`, `kind`, `agent`, `started_at`, `ended_at`, `verdict`, `session`, `terminal`, `task` (a child's) | answer path, feedback, tokens, turns, cost, lines, findings, failures, guard, score, interventions |
| `run_events` | `id`, `run`, `at`, `what` (`started`, `stage`, `verdict`, `revision`, `paused`, `resumed`, `answered`, `skipped`, `retried`, `typed`, `stopped`, `landed`), `by` | details |

- **Settings:** `pipelines.default` (a pipeline every `kanna task new` uses unless `--pipeline none`), `pipelines.pricing` (per-model token prices for agents that don't report cost), `tasks.land` (the command that lands a branch when a plain merge isn't the repo's way), in the repo's `.kanna/config.json` ([settings](settings.md)). Pipelines and prompts in a cloned repo run only agents Kanna knows and commands in its worktree, as `tasks.setup` does.
- **The app** draws the Run tab from a `run` message the daemon publishes (the graph from the revision's parsed form, the rows from `run_stages`), and the Stats section from `stats pipelines`.
- **Tested** by `kanna-pipe` unit tests (the parser against the three example files and each rule's error), `runs.rs` tests with the mock agent (a visit's answer read, the arrow picked, a loop bounded, a guard rolled back, a budget stopping a run), and e2e `pipeline_run` on a private instance: a fixture repo with a two-stage pipeline (mock agent, then a command that fails once and then passes), the run landing, its rows and the Run tab's data, a fan-out of two children joined, an `ask` answered from the CLI, a revision emitted and one edited, a daemon restart mid-stage.

## 7. CLI

| Command | Does |
|---|---|
| `kanna task new NAME [MSG] --pipeline NAME` | A task that runs the pipeline when it starts (`--pipeline none`: a plain task when `pipelines.default` is set) |
| `kanna pipeline list` · `kanna pipeline check NAME\|FILE` | The repo's pipelines, with their worst case; validate one, errors as `FILE:LINE: message` |
| `kanna run list [--all]` | The repo's runs: state, stage, how far, how long, outcome |
| `kanna run show RUN [--json]` | The run: its revision, visits with their numbers, children |
| `kanna run answer [RUN] yes\|no [NOTE]` | Answer the waiting `ask` |
| `kanna run pause\|resume\|skip\|retry\|stop [RUN]` | The Run tab's buttons |
| `kanna run edit [RUN]` | The run's current revision in a code pane; saving validates and applies it |
| `kanna run log [RUN]` | The run's events |
| `kanna stats pipelines [--json] [--days N]` | Runs compared by pipeline and revision |

RUN is a run's ID, its task, or the terminal the command runs in.

## 8. Rejected

- **An agent as the orchestrator** (a planner that calls stages as subagents): the plan would live in one context window, the user couldn't see or steer between stages, and every step's cost would be inside one session. The daemon keeps the graph, the records and the gates; agents do stages.
- **Headless agents** (`claude -p`, `codex exec`, as kojo runs them): nothing to watch, no resume, no typing into a stage, no messages. A stage's agent runs in the task's pane like any task's agent, which the rest of Kanna already handles.
- **YAML or JSON pipelines:** the arrows are the point; a flow line reads as the diagram. kojo's grammar is proven on 30 factories.
- **A run that rewrites its own file:** revisions by a declared `emits` stage or by you, validated and bounded ([pipeline language §7](pipeline-language.md#7-changing-a-pipeline-while-it-runs)).
- **Child runs for fan-out:** one change, one run, one graph and one set of numbers; children are tasks.
- **Test count and pass rate as quality:** all tests must pass to land; the quality columns are the Code quality metrics task's.

## 9. Build tasks

For the planner, in order; each builds on the ones before and is testable alone.

1. **Pipeline language crate** (`kanna-pipe`): parser, validator, worst case, `pick`, revisions; port of kojo's `factory_spec.py` with the licence settled and `NOTICE` written; `kanna pipeline check` and `list`. Tests on the three example files and every rule.
2. **Runs in the daemon, sequential stages:** `runs` tables and migration; `kanna task new --pipeline`; `runs.rs` starts agent and `run` stages in the task's pane, reads answers from session files, picks arrows, bounds loops, `progress`, `reset`, the budget, `merge main`; `kanna run list`, `show`, `log`, `stop`. e2e `pipeline_run` with the mock agent.
3. **Gates and steering:** `ask` stages, waiting and needs you, `kanna run answer`, `pause`, `resume`, `skip`, `retry`; typed prompts recorded as interventions; stop and start of a running task mid-stage; daemon restart mid-stage.
4. **The Run tab:** the fixed tab in the app: graph, numbers, revision, visits; its buttons and the `ask` card; the header, Desk view row and strip saying the stage; `kanna open run`.
5. **Fan-out and join:** `each`, child tasks from a stage's named output, `merge STAGE`, conflicts as a fail, `xN by`, `guard`.
6. **Revisions:** `emits` blocks merged and validated, `kanna run edit` in a code pane, revisions on the Run tab with diffs.
7. **Metrics:** tokens, turns, tool calls and cost from Claude's and Codex's session files per visit; lines and files; findings and failures parsed; `kanna stats pipelines`; the Pipelines section of the Stats tab with the dataviz rules (after "Stats tab on the Desk"); the quality columns joined by merge commit (after "Code quality metrics").
8. **Dogfood:** run `feature.pipe` on a real feature and `bugfix.pipe` on a real bug, with `scripts/check.sh` written for stage use; compare with the numbers; tune the prompts and arrows; write up what changed and why in the pipeline files' comments.

## 10. Glossary

| Term | Meaning |
|---|---|
| **Pipeline** | A `.pipe` file in `.kanna/pipelines/` ([pipeline language](pipeline-language.md)) |
| **Run** | A task that runs a pipeline: one change, from request to landed |
| **Stage, visit** | One step of a pipeline; one time it ran |
| **Child** | A task made by a fan-out stage for one item or attempt |
| **Revision** | One version of a run's pipeline |
| **Intervention** | Anything you did to a run: an answer, a pause, a skip, a retry, an edit, a message, a typed prompt |
| **Landed** | The run's branch merged on main by its `merge main` stage |
| **Stuck** | A run with no eligible arrow, waiting for you |
