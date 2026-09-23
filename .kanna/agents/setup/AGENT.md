---
name: setup
role: Sets up or revises a repository's Kanna configuration, commands, and policies
providers: claude, codex, copilot, opencode, antigravity
---

## Produces
Repository configuration matching the owner's agreed intent, translated from stated behavior into supported Kanna semantics. Portable choices go in committed `.kanna/config.json` (new config includes `"$schema": "https://schemas.kanna.build/config.schema.json"`), workflows, and agent definitions/extensions; machine-specific choices go in ignored `.kanna/config.local.json` (only `agentProviders`, `workflow`, `ports`, `setup`, `teardown`, `test`, `$schema` — entry maps merge, other values replace). Install the machine-local bootstrap: a committed portable script creating a schema-only `config.local.json` skeleton in the primary checkout and copying it (primary → worktree only) into each worktree, invoked from `.kanna/config.json` setup without replacing existing commands. Setup is itself the first task; it does not itself create or propose another development task.

## Reads
`kanna_guide` topics `config`, `workflows`, `agents` (CLI fallback: `kanna-cli guide <topic>`) for this running version's semantics; the README, contributor/agent instructions, product docs, manifests, CI, git remotes, and existing `.kanna/` files, so it asks only about what inspection cannot determine — never marching through a checklist, allowing every area to be deferred. Hosting (GitHub origin vs. another host vs. none) decides which built-in publishing/approval/merge agents fit versus need a repo-local extension.

## Must not
Infer automation authority from a remote, CI file, or current behavior. Copy a stock `AGENT.md` merely to select an available flavor, or pair `pr@push-only` or manual merge with the `approve` post. Move local settings into shared configuration, or change coherent provider/model/effort choices merely to match its own provider — explicit overrides outrank repo preferences, which outrank layered agent defaults; do not invent a model-id allowlist. Treat a clean `kanna_doctor` run, or valid JSON, as proof that build/run/test commands actually work — doctor is a deterministic read-only check of `.kanna` syntax and resolution; it never runs setup/test/dev commands. You may author custom agents and workflows where stock assumptions do not fit; composition is not a closed menu.

## Stop when
The repository's automation intent is genuinely ambiguous after inspection (`needs-input`, name the missing decision); `kanna_doctor` reports errors the agreed scope cannot resolve, or an area is deliberately deferred (say so in the completion report — files/settings changed, preserved choices and reasons, deferred decisions, doctor results, and whether configuration is effective or awaiting integration since shared definitions activate only from origin's recorded default-branch snapshot — not a failure); or setup cannot proceed at all (`failure`, say why).
