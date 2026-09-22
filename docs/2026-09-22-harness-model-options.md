# Harness model options — 2026-09-22

## Provenance and scope

Task `733656c1`, branch/worktree `task-733656c1`. Before edits, HEAD,
`origin/main`, their merge base, and `git ls-remote origin refs/heads/main`
all resolved to `fd0eb9c55f95a31e02514060bdcefcd0a3f08aea` (0 ahead/behind).

`packages/core/src/agent-models.ts` owns the static model picker catalog.
Desktop `useAgentMessageView` / `AgentMessageView.vue` and the live CLI-contract
tests consume it. This base has no mobile model/effort picker or API model
allowlist. Mobile uses the generated harness registry and carries literal
model/effort strings; server selection validation lives in
`crates/kanna-agent-protocol/src/providers.rs`. Claude's existing effort
vocabulary and Codex's model-specific pass-through already support these
releases. No protocol shape changed, so no generated files need regeneration.

## Added options and native evidence

| Harness | Picker label | Native identifier | Supported native efforts |
| --- | --- | --- | --- |
| Claude Code | Opus 5.5 | `claude-opus-5-5` | low, medium, high, xhigh, max |
| Codex | GPT-6 Astra | `gpt-6-astra` | low, medium, high, xhigh, max, ultra |
| Codex | GPT-6 Sol | `gpt-6-sol` | low, medium, high, xhigh, max, ultra |
| Codex | GPT-6 Luna | `gpt-6-luna` | low, medium, high, xhigh, max |

Read-only installed metadata, without inference:

- `codex --version`: `codex-cli 0.155.1`. Its `~/.codex/models_cache.json`,
  fetched `2026-09-22T18:26:52.081701Z`, lists these three exact slugs, display
  names `GPT-6-Astra`, `GPT-6-Sol`, `GPT-6-Luna`, and the efforts above in
  `supported_reasoning_levels`. All three have `visibility: list` and default
  reasoning level `medium`. Picker labels use the documentation's spacing.
- `claude --version`: `2.1.280 (Claude Code)`.
  `claude --help` lists `--effort` values `low, medium, high, xhigh, max`.
  The installed binary's bundled model record has `id: "claude-opus-5-5"`,
  `display_name: "Opus 5.5"`, the same first-party ID, `effort`, `xhigh_effort`
  and `max_effort` capabilities, `default_effort: "medium"`, and native 1M
  context. This establishes more than the `/model` display label alone.

The official [Codex models page](https://learn.chatgpt.com/docs/models)
(the redirect target of `developers.openai.com/codex/models/`) documents native
`codex -m` commands for Astra, Sol, and Luna, Sol's six reasoning choices, and
Luna's exclusion of Ultra. Ultra is Codex orchestration; the
[OpenAI API Astra model page](https://developers.openai.com/api/docs/models/gpt-6-astra)
lists API reasoning only through `max`. No `codex-6` slug or API-only effort
was inferred.

The official [Claude Code model configuration](https://code.claude.com/docs/en/model-config)
documents the five Opus 5.5 effort levels, minimum CLI 2.1.280, and pinned
`claude-opus-5-5` versus moving `opus`. The
[provider model inventory](https://platform.claude.com/docs/en/models/overview)
also confirms the explicit ID. The 1M window does not require a new suffix.
Existing aliases are left for the harness to resolve; the catalog uses explicit
release IDs. Claude's separate `ultracode` workflow setting is not a new model
effort and is outside this catalog update.

## Compatibility

The prior picker entries remain: Opus 4.8, Sonnet 4.6, Haiku 4.5; GPT-5.5,
GPT-5.4, GPT-5.4 mini, and GPT-5.3 Codex Spark. This does not re-certify old
models' availability for every account/sign-in method. Historical fixtures,
stored selections, defaults, live sessions, repo agent assignments, and other
providers are unchanged. Desktop tests prove that an older running model stays
selected and no model-change command is sent until the user picks one.

Structured candidates preserve explicit model and effort strings, including
custom IDs and the recorded `claude-opus-5`. The legacy compact selector syntax
is unchanged: use a structured candidate or sibling `effort: ultra` for Codex
Ultra, not a new `-ultra` suffix that could reinterpret a custom model ID.
Codex continues to delegate model-specific validity to its CLI; Kanna does not
introduce an allowlist. Claude, Copilot, and Antigravity still reject `ultra`.

## Verification

Passed, with no paid inference calls or UI/dev servers:

- `pnpm --filter @kanna/core exec vitest run src/agent-models.test.ts src/config/agent-providers.test.ts src/config/repo-config.test.ts --maxWorkers=2`: 38 tests.
- Desktop `AgentMessageView.test.ts`: 17 tests; `agentCommand.test.ts`: 13 tests,
  using `pnpm --filter @kanna/desktop exec vitest run <paths> --maxWorkers=2`.
  Covers native release IDs, launch efforts, labels, and existing selections.
- `cargo test -p kanna-agent-protocol --test providers`: 15 tests, including
  shared structured-selection fixtures and harness effort boundaries.
- `pnpm --filter @kanna/mobile exec vitest run src/lib/api/agentProviders.test.ts --maxWorkers=2`: 8 tests.
- `pnpm --filter @kanna/core exec tsc --noEmit` and
  `pnpm --filter @kanna/desktop exec vue-tsc --noEmit`: both exit 0.
- `git diff --check`: clean.

An additional run of `src/workflow/agent-loader.test.ts` passed 42 tests and
failed one existing assertion at line 467: it expects the review extension to
contain `Kanna Repository Test Requirements`, but the unchanged extension starts
with `Verification Proportional to the Change`. The loader, test, and extension
have no diff against the base; this unrelated mismatch is left alone.
