# Delivery status — tasks, sessions and structured workflows

Coordinating parent: task `482a02db`. Specification: `docs/specs/tasks-sessions-structured-workflows.md` at `9a4198da8`.
This manifest is the parent's durable record of component children, their prerequisite commits, reviewed results and
integrated commits, so a resumed or revising parent can reconcile without duplicating children.

Child workflow: `plan-build-review`, edited per child after its plan settles to
`plan[M] -> in progress[a]+commit -> review[M, final]` (no `pr`/`approve`). The parent operates each child's plan and
final review gates as manager; children return reviewed local commits only.

| Task | Child id | Scope / current checkpoint | Base / prerequisites consumed | State | Reviewed commit(s) | Integrated as |
|---|---|---|---|---|---|---|
| T0 ledger bridge | `1dc3da40` | full card | `task-482a02db-3` @ `9a4198da8` | plan | — | — |
| T6 local artifacts | `ed3533d4` | first increment (publish/open by tree id); result binding after T0.review | `task-482a02db-3` @ `9a4198da8` | plan | — | — |
| T8 provenance | `d06e30e1` | first increment (channel identity capture); account boundary later | `task-482a02db-3` @ `9a4198da8` | in progress (plan accepted by parent) | — | — |
| T1 workflow contract | — | — | needs T0.review | not created | — | — |
| T2 stage workspaces | — | — | needs T0.review | not created | — | — |
| T3 commit transitions / roleless gates | — | — | needs T1, T2 review | not created | — | — |
| T4 stage dependency edges | — | — | needs T1, T2 review | not created | — | — |
| T5 subtask joins | — | — | needs T4 review | not created | — | — |
| T7 artifact remote | — | — | needs T6, T8 review | not created | — | — |
| T9 transfer | — | — | needs T1–T5, T8 (T6) review | not created | — | — |
| T10 definitions | — | — | needs T3, T5, T6 review | not created | — | — |
| T11 task/session UI | — | — | needs T0 (first), T2–T4 review | not created | — | — |
| T12 artifact viewer | — | — | needs T6 (T7, T8) review | not created | — | — |
| T13 disk authority / migration | — | — | needs T1–T12, T14 review | not created | — | — |
| T14 release workflow | — | — | needs T3, T8 review | not created | — | — |

## Log

- 2026-09-23: first wave created from parent tip `9a4198da8`.
- 2026-09-23: T8 plan accepted (channel-identity type in `mutation_provenance.rs`; additive SQL provenance on runs/inputs/events); workflow edited to qa-dispatcher final manual review; advanced to build.
