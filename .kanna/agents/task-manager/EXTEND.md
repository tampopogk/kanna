## Keep Small Tasks Small

Owner feedback (2026-09-10): a terminology rename and compact MCP response
must not become hours of repeated builds, visual matrices, and review churn.
Set a bounded acceptance bar from the requested behavior. Reuse exact-head or
patch-equivalent evidence, and require a concrete reason before repeating a
broad gate or adding another review round. Routine naming changes need focused
definition/compatibility checks; API projections need affected route/consumer
checks. Preserve necessary integration proof without making every task run
every repository lane.

When verification or scope grows beyond the request, intervene immediately:
identify the remaining defect or proof, remove unrelated work and redundant
checks, and carry the task through review and merge. Do not create additional
consultations or tasks merely to explain procedural delay. Distinguish an
agent working on the requested result from one repeatedly verifying unchanged
code. Use observed runs, revisions, and diff growth to audit wasted work;
do not invent token totals.

## Planning Model Selection

For future planning runs, prefer the coherent `codex` / `gpt-6-astra` / `high`
selection while Fable availability is constrained. `claude` / `fable` with an
explicit supported effort remains an allowed planning alternative when it is
actually available or explicitly selected. Respect real provider availability,
keep provider, model, and effort together, and never compose Fable with Codex.
Do not use Sol for planning absent a later owner instruction. This applies only
to future planning; do not rerun or rewrite existing tasks or plans, and do not
change builder, reviewer, or consultant selection for it.

## Desktop UI Targeting

Owner direction (2026-09-10): dev windows carry the task id in their native
title. Require that exact id plus worktree/build identity before any agent UI
interaction or visual evidence, and recheck after target/session changes.
Generic `Kanna` / `build.kanna` lookups can launch production before inspection;
forbid them for test selection. Use only an explicitly identified running
worktree window or canonical isolated WebDriver endpoint. Missing/mismatched
identity stops that UI path; it does not authorize an installed-app fallback.
Carry this rule into active implementation/review directives immediately;
future agents inherit the canonical rule in `AGENTS.md` once merged. On an
incident, stop owned automation, preserve actual actions/identity evidence,
and leave operator processes untouched. Installed production/staging testing
requires a separate explicit human request naming the environment.

## Kanna Desktop Release Policy

For this repository, never run `./kd release ship` directly in the manager session. Create and shepherd the Ship task, whose repo-local `ship` extension owns the release runbook and flag semantics. After any manual publish, run `./kd release status` and verify that the channel version actually moved.
