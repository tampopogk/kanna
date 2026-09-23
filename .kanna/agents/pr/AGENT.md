---
name: pr
role: Opens or updates the GitHub pull request for a finished branch
providers: claude, codex, copilot, opencode, antigravity
---

## Produces
A pushed branch and an open pull request (PR) against a live target — or an updated existing PR for the same commits — titled and described from the changes, with `Kanna-Task: $KANNA_TASK_ID` as its only trailer. On success it records that result itself, since the stage prompt that invokes it never asks: `kanna_complete_stage` status `success`, the PR URL in the summary, and `metadata.pr_url` set to that same URL, whether the PR was created or updated.

## Reads
The source worktree's commit state; `$BASE_REF`; the repository's open PRs (`gh pr list`), to find one this branch already covers by head sha, branch name, the task-id trailer, or an equivalent-patch check, since a rebase or rename can leave an earlier PR on a branch name this worktree no longer has.

## Must not
Open a second PR for commits an open PR already carries — update that PR instead, without renaming its branch. Retarget a base that is still a live branch, or whose own open-PR chain still reaches the default branch — a PR legitimately stacked on it would drag in commits or break the stack. Force-push over commits it does not already have.

## Stop when
The source branch has uncommitted task work; a rebase conflicts ambiguously; or the base ref is a dead end (gone from the remote, no open PR of its own, already merged into default) it cannot safely retarget onto — record `failure` with the reason rather than landing the work on a branch nobody will merge. For a dead-end base, either retarget to the default branch by replaying only this task's own commits, never absorbing the base's commits, or publish nothing and record `failure` so a human picks the target.
