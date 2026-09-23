# setup Contract

Maintainer documentation. This file is never resolved into the agent's prompt — the agent follows `AGENT.md` plus any repository `EXTEND.md` — so every rule below must also be stated there; this file must not be the only home of one.

The `setup` role configures a repository by composing built-in Kanna roles and tested flavor variants.

Required behavior:

- It must inspect the repository before asking questions, including git remote URL, available GitHub auth through `gh auth status`, existing CI configuration, and existing `.kanna/` files, and ask only for decisions that inspection cannot determine safely.
- It must write `.kanna/config.json` selections, and repo-local `EXTEND.md` files only for behavior that does not match stock flavors. It must not write copied stock `AGENT.md` files for roles such as `pr` or `merge`.
- For the stock GitHub flow, it must select a built-in workflow (`no-review`, `single-reviewer`, or `specialized-reviewers`) plus `merge@github`, not author a workflow file of its own, and must not select `pr@draft-pr`.
- Its answers must compose: `pr@push-only` must never be paired with a built-in workflow; manual merge omits the `approve` post; `pr@draft-pr` with a merge agent writes a repo-local `.kanna/agents/approve/EXTEND.md` that readies the draft before signaling.
- Machine-specific choices go in ignored `.kanna/config.local.json`; it preserves an existing local config bootstrap if valid and does not mandate installing one.
- It runs `kanna_doctor` against the candidate worktree after edits and validates changed JSON files, without treating doctor as proof that commands work.
- It explains activation (shared definitions resolve from origin's recorded default-branch snapshot) rather than publishing or inventing a remote.
- It must finish with `kanna_complete_stage` status `success`, or `failure` when setup is blocked.
