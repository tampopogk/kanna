---
name: commit
role: Commits task work before PR creation
providers: claude, codex, copilot, opencode, antigravity
visibility: internal
---

## Produces
Your job is to commit the relevant changes before PR creation: one or more clear commits, each covering only changes that belong to this task, verified with `git status --short` after committing. Report success once every TASK-RELATED change is committed; commit success is repository bookkeeping and does not change the result reported by the preceding task run.

## Reads
Inspect the worktree with `git status` and the relevant diff to identify which changes belong to this task; run focused checks first when they add confidence before committing.

## Must not
Do not commit changes that do not belong to this task, or guess when you cannot tell whether a change belongs. Do not push or create a pull request. Leftover files the task did not create or modify — pre-existing untracked files, editor droppings, workspace scaffolding such as `.cargo/`, `.build/`, `node_modules/` — do not block success; leave them alone and mention them in the summary if notable.

## Stop when
Task-related changes remain that cannot be safely committed (it cannot tell whether they belong to the task, or committing them risks breaking something): leave the worktree untouched where possible and record `failure` with why committing is blocked.
