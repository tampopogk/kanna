# Software factory: build order

**Status:** proposed · **Part of:** [software factory](software-factory.md), [pipeline language](pipeline-language.md)

The tasks that build the factory, in order; each builds on the ones before and is testable alone. Nothing here is built yet.

For the planner, in order; each builds on the ones before and is testable alone.

1. **Pipeline language crate** (`kanna-pipe`): parser, validator, worst case, `pick`, revisions; port of kojo's `factory_spec.py` with the licence settled and `NOTICE` written; `kanna pipeline check` and `list`. Tests on the three example files and every rule.
2. **Runs in the daemon, sequential stages:** `runs` tables and migration; `kanna task new --pipeline`; `runs.rs` starts agent and `run` stages in the task's pane, reads answers from session files, picks arrows, bounds loops, `progress`, `reset`, the budget, `merge main`; `kanna run list`, `show`, `log`, `stop`. e2e `pipeline_run` with the mock agent.
3. **Gates and steering:** `ask` stages, waiting and needs you, `kanna run answer`, `pause`, `resume`, `skip`, `retry`; typed prompts recorded as interventions; stop and start of a running task mid-stage; daemon restart mid-stage.
4. **The Run tab:** the fixed tab in the app: graph, numbers, revision, visits; its buttons and the `ask` card; the header, Desk view row and strip saying the stage; `kanna open run`.
5. **Fan-out and join, the child-task executor:** `each`, child tasks from a stage's named output, `merge STAGE`, conflicts as a fail, `xN by`, `guard`; `on` parsed, resolved to `here` only, and the visit's `machine` recorded.
6. **Revisions:** `emits` blocks merged and validated, `kanna run edit` in a code pane, revisions on the Run tab with diffs.
7. **Metrics:** tokens, turns, tool calls and cost from Claude's and Codex's session files per visit; lines and files; findings and failures parsed; `kanna stats pipelines`; the Pipelines section of the Stats tab with the dataviz rules (after "Stats tab on the Desk"); the quality columns joined by merge commit (after "Code quality metrics").
8. **Stages on other machines** (after the Machines tasks for the replicated Shelf and task transfer): `on` resolved against the peer log and roles, the child task on a peer through transfer, its record and commits back, an offline placement waiting; e2e `pipeline_remote` between two daemons as `machine.rs`'s tests pair them.
9. **Dogfood:** run `feature.pipe` on a real feature and `bugfix.pipe` on a real bug, with `scripts/check.sh` written for stage use; compare with the numbers; tune the prompts and arrows; write up what changed and why in the pipeline files' comments.
