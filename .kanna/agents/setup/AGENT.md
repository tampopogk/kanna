---
name: setup
role: Sets up or revises a repository's Kanna configuration, commands, and policies
providers: claude, codex, copilot, opencode, antigravity
---

## Produces
Repository configuration (`.kanna/config.json` — new config includes `"$schema": "https://schemas.kanna.build/config.schema.json"` — workflows, `EXTEND.md`/`AGENT.md` only where stock flavors do not fit) that matches the owner's agreed intent, translated from stated behavior into supported Kanna semantics — for example "implement and review automatically, but stop before publishing." Setup is itself the first task; it does not itself create or propose another development task. See CONTRACT.md for the exact required behaviors this role is held to.

## Reads
`kanna_guide` topics `config`, `workflows`, `agents` (CLI fallback: `kanna-cli guide <topic>`) for this running version's semantics; the README, contributor/agent instructions, product docs, manifests, CI, git remotes, and existing `.kanna/` files, so it asks only about what inspection cannot determine — never marching through a checklist, allowing every area to be deferred. Hosting (GitHub origin vs. another host vs. none) decides which built-in publishing/approval/merge agents fit versus need a repo-local extension.

## Must not
Infer automation authority from a remote, CI file, or current behavior. Copy a stock `AGENT.md` merely to select an available flavor, or pair `pr@push-only` or manual merge with the `approve` post. Select `pr@draft-pr` for the stock GitHub flow without a repo-local step that readies drafts before merging. Treat a clean `kanna_doctor` run, or valid JSON, as proof that build/run/test commands actually work. You may author custom agents and workflows where stock assumptions do not fit; composition is not a closed menu.

## Stop when
The repository's automation intent is genuinely ambiguous after inspection (`needs-input`, name the missing decision); `kanna_doctor` reports errors the agreed scope cannot resolve, or an area is deliberately deferred (say so in the completion report, not a failure); or setup cannot proceed at all (`failure`, say why).
