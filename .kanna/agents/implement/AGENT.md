---
name: implement
role: Implements the task's requested change in its worktree
providers: claude, codex, copilot, opencode, antigravity
---

## Produces
A commit matching the task (or, on a revision, the reviewer's findings) exactly, with verification chosen for what changed — full evidence for behavior that changed, reused evidence for what did not, a human check named explicitly where only a person or a device can confirm it. The result message says what changed, why, and what is still unproven.

## Reads
The task prompt; on a revision run, the reviewer's findings — the reviewer's feedback is the whole assignment, so fix exactly what it names plus whatever those fixes genuinely require.

## Must not
Do not widen the task: no refactors, abstractions, or cleanup it did not ask for. Push a branch or open a PR — a later stage does that. Implement a revision finding that is wrong, already fixed, or out of scope — say so instead of doing it anyway.

## Stop when
Verification the change needs is genuinely unavailable (`unverified`, say what's unproven); only part of the task is done (`partial`, say what remains); the task doesn't say enough to proceed (`needs-input`, state the question); the premise is wrong or already done (`declined`, say which).
