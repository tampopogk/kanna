# Delivery status — tasks, sessions and structured workflows

Coordinating parent: task `482a02db`. Specification: `docs/specs/tasks-sessions-structured-workflows.md` at `9a4198da8`.
This manifest is the parent's durable record of component children, their prerequisite commits, reviewed results and
integrated commits, so a resumed or revising parent can reconcile without duplicating children.

Child workflow: first wave (T0, T6, T8) ran `plan-build-review`, edited after its plan settled to
`plan[M] -> in progress[a]+commit -> review[M, final]` (no `pr`/`approve`). Owner direction 2026-09-23: later children
skip the child plan stage — create on `specialized-reviewers` (panel) or `single-reviewer` (UI/definition-only), building
straight from the card, and edit only the tail to `review[M, final]` with no `pr`. Publication stays in this parent's
single PR stage. The parent operates each child's plan and
final review gates as manager; children return reviewed local commits only.

| Task | Child id | Scope / current checkpoint | Base / prerequisites consumed | State | Reviewed commit(s) | Integrated as |
|---|---|---|---|---|---|---|
| T0 ledger bridge | `1dc3da40` | full card | `task-482a02db-3` @ `9a4198da8` | in progress (plan accepted by parent) | — | — |
| T6 local artifacts | `ed3533d4` | first increment (publish/open by tree id); result binding after T0.review | `task-482a02db-3` @ `9a4198da8` | in progress (plan accepted by parent) | — | — |
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
- 2026-09-23: T6 plan accepted (bare artifacts.git outside worktree, parentless retained content refs, metadata-only ref, loopback read-only preview listener, six catalog tools); same workflow edit; advanced to build.
- 2026-09-23: T0 plan accepted (store at ~/.kanna/repos/<repo-id>/tasks/<task-id>/, task.json snapshot + immutable ledger/<seq>-*.{md,json}, SQL outbox in originating txn, publication barrier before announce/dispatch, KANNA_TASK_LEDGER_PATH + triggering result in preamble, reserved declared_role/channel_identity/artifacts); same workflow edit; advanced to build.
- 2026-09-23: T8 review round 1 failed (security: LAN machine-invoke identity must record the account the bearer secret was verified under, not a fresh dispatch-time read); migration/compat passed; 181 scoped tests pass. Auto revision 1/5 to build. Non-blocking follow-ups: forged-header extractor coverage beyond loopback; tolerant unknown-kind ChannelIdentity decoding before a typed peer consumer exists.
- Environment note: specialty reviewers could not compile on Darwin 27 (zig libcxx break); T8's dispatcher ran scoped tests via a MacOSX26.5 xcrun shim.
