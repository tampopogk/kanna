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
| T6 local artifacts | `ed3533d4` | first increment (publish/open by tree id); result binding after T0.review | `task-482a02db-3` @ `9a4198da8` | increment 1 reviewed + integrated; parked at review awaiting T0 for checkpoint 2 | `dc9c09bfa` | `20fd89157` |
| T8 provenance | `d06e30e1` | first increment (channel identity capture); account boundary split to T8b | `task-482a02db-3` @ `9a4198da8` | increment 1 reviewed (round 2), integrated, closed | `65e1943e5`, `7c06a0de4` | `a609c5aec` |
| T8b same-account boundary | `216cf1c2` | account-boundary enforcement + T8 follow-ups (build-first, specialized-reviewers; build gate manual) | parent `e679678d0` (T6+T8 integrated) | reviewed (round 2), integrated, closed | `7863ff1f0`, `a00800e67` | `bab127907` |
| T1 workflow contract | — | — | needs T0.review | not created | — | — |
| T2 stage workspaces | — | — | needs T0.review | not created | — | — |
| T3 commit transitions / roleless gates | — | — | needs T1, T2 review | not created | — | — |
| T4 stage dependency edges | — | — | needs T1, T2 review | not created | — | — |
| T5 subtask joins | — | — | needs T4 review | not created | — | — |
| T7 artifact remote | `a07d574f` | increment 1: two homes publish/fetch/comment via one Git remote (CLI/API) | parent `01f5c8972` (T6 dc9c09bfa, T8 7c06a0de4, T8b a00800e67) | in progress | — | — |
| T9 transfer | — | — | needs T1–T5, T8 (T6) review | not created | — | — |
| T10 definitions | — | — | needs T3, T5, T6 review | not created | — | — |
| T11 task/session UI | — | — | needs T0 (first), T2–T4 review | not created | — | — |
| T12 artifact viewer | `6d3d22f8` | increment 1: local open-by-hash desktop/mobile, anchors, isolation; remote + §14 walkthrough after T7.review | parent `01f5c8972` (T6 dc9c09bfa, T8/T8b) | in progress | — | — |
| T13 disk authority / migration | — | — | needs T1–T12, T14 review | not created | — | — |
| T14 release workflow | — | — | needs T3, T8 review | not created | — | — |

## Log

- 2026-09-23: first wave created from parent tip `9a4198da8`.
- 2026-09-23: T8 plan accepted (channel-identity type in `mutation_provenance.rs`; additive SQL provenance on runs/inputs/events); workflow edited to qa-dispatcher final manual review; advanced to build.
- 2026-09-23: T6 plan accepted (bare artifacts.git outside worktree, parentless retained content refs, metadata-only ref, loopback read-only preview listener, six catalog tools); same workflow edit; advanced to build.
- 2026-09-23: T0 plan accepted (store at ~/.kanna/repos/<repo-id>/tasks/<task-id>/, task.json snapshot + immutable ledger/<seq>-*.{md,json}, SQL outbox in originating txn, publication barrier before announce/dispatch, KANNA_TASK_LEDGER_PATH + triggering result in preamble, reserved declared_role/channel_identity/artifacts); same workflow edit; advanced to build.
- 2026-09-23: T8 review round 1 failed (security: LAN machine-invoke identity must record the account the bearer secret was verified under, not a fresh dispatch-time read); migration/compat passed; 181 scoped tests pass. Auto revision 1/5 to build. Non-blocking follow-ups: forged-header extractor coverage beyond loopback; tolerant unknown-kind ChannelIdentity decoding before a typed peer consumer exists.
- Environment note: specialty reviewers could not compile on Darwin 27 (zig libcxx break); T8's dispatcher ran scoped tests via a MacOSX26.5 xcrun shim.
- 2026-09-23: T6 increment 1 passed panel review at `dc9c09bfa` (compat/perf pass; security/concurrency/migration findings judged non-blocking; 65 scoped server tests + 55 catalog + 29 TS config + CLI round trip). Merged into parent as `20fd89157` (tree = reviewed commit + manifest only). T6 stays open for checkpoint 2 (result binding, retention) after T0.review; carry these follow-ups into it: case-insensitive `.git` component check; bounded preview drain after close/expiry; persisted monotonic version sequence instead of wall clock; preview session cap; navigation-to-control-port browser test; document config.local downgrade rejection of `artifacts`.
- 2026-09-23: T8 increment 1 passed panel review round 2 at `7c06a0de4` (security fix: LAN invoke records the account verified with the secret; 19 scoped tests). Merged as `a609c5aec` alongside T6; shared files http_api.rs, router.rs, http_api/tests.rs, main.rs, task_creator/mod.rs auto-merged — combined build/test in progress. Contract note: dispatch_authenticated_lan_http_invoke gained verified_account_uid. Account-boundary enforcement split into follow-on child T8b (separate contract), plus T8 follow-ups; ledger channel_identity fill waits for T0.review.
- 2026-09-23: combined T6+T8 check at `a609c5aec` (xcrun MacOSX26.5 shim): 239 scoped tests passed, 0 failed (artifact, local_config, every_registered_http_route, mutation_provenance, lan_listener, peer_legacy, lan_machine, http_api actions/input/workflow_switch); clippy -D warnings clean except 3 pre-existing `nonminimal_bool` errors in http_api/desktop_views.rs, unchanged since base 9a4198da8 (not in delivery scope). T8 review gate operated and task closed.
- 2026-09-23: T8b created from parent tip `e679678d0`, claude opus medium, tail edited to qa-dispatcher final manual review.
- 2026-09-23: T0 review round 1 failed at `c91542a2c`: concurrency — 3 blocking findings in outbox/continuation recovery; migration and compat passed; 354 scoped Rust + 46 prompt-builder tests pass. Auto revision 1/5 to build.
- 2026-09-23: T8b build committed `7863ff1f0` (account_boundary module; relay/LAN-machine/sealed-peer same-account enforcement; legacy pin diagnostics + re-pair; federation unreachable owner → 503; tolerant ChannelIdentity decode). 178 scoped tests pass; 4 full-suite failures reported as base/TMPDIR artifacts (to be confirmed in review). Advanced to panel review. Out-of-scope follow-up: daemon socket mode follows process umask and daemon input has no uid check (crates/daemon/src/socket.rs).
- 2026-09-23: T8b review round 1 failed: compat — stream-client does not treat `peer_account_boundary` as a connection refusal (silent reconnect loop instead of the re-enrollment diagnostic). Security ABA race and pre-existing plaintext claim error judged non-blocking; 325 scoped tests pass. Auto revision 1/5. Follow-ups: routing-generation snapshot for relay_confirms_sibling; structured claim_pairing_offer error body; document one-sided re-enrollment; old-reader/new-writer peer_trust fixture.
- 2026-09-23: T8b passed round 2 at `a00800e67` (stream-client treats peer_account_boundary as refusal; 86 stream-client tests, tsc clean; 325 scoped server tests from round 1). Merged as `bab127907` (tree = reviewed + manifest). T8b closed. Remaining T8 work: write channel_identity into T0's ledger envelope after T0.review (parent-owned small step or T0 follow-up).
- 2026-09-23: T7 (`a07d574f`) and T12 (`6d3d22f8`) created from parent `01f5c8972` after T6/T8/T8b review; build-first on specialized-reviewers, claude opus medium, tails edited to panel final manual review.
