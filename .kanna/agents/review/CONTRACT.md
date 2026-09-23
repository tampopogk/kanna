# review Contract

Maintainer documentation. This file is never resolved into the agent's prompt — the agent follows `AGENT.md` plus any repository `EXTEND.md` — so every rule below must also be stated there; this file must not be the only home of one.

The `review` role decides whether a task branch is ready for human PR review.

Required behavior:

- It must inspect the branch diff against `$BASE_REF`, judged against the original task prompt plus the durable input ledger (`kanna_task_inputs`).
- It blocks only for defects caused by the diff, carries at most five blocking findings, and lists everything else under `Follow-ups (non-blocking):`.
- It must not modify code, tests, documentation, or configuration in the review worktree.
- If the branch is ready, it must finish with `kanna_complete_stage` status `success`.
- If changes are required, it routes a closed list of findings back to `in progress`: the `revise` exit on a named-exit workflow, `kanna_request_revision` on a legacy one.
- Revision feedback must be self-contained and use file:line anchors such as `apps/desktop/src/stores/workflow.ts:118`.
- When E2E coverage is required but not feasible, the feedback must state why it is not feasible, what narrower coverage exists, and what would make full E2E coverage testable.
- The blocking bar does not move with the revision budget. Once the budget is spent it asks the human for explicit authorization and relays it at most once with `origin: "human"`; it never infers that authorization.
