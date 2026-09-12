# OpenCode model selection verification — 2026-09-12

Task 7e983334 implements the owner's accepted model integration in its existing
worktree. It does not repeat the local inference demonstrations or publish any
changes. Model serving remains external to Kanna.

## Launch coordination

Task e218c437 is a separate, unmerged ordinary-launch repair. Its full detail,
empty input ledger, source patch and
`docs/2026-09-12-opencode-launch-e2e-note.md` were read on September 12. That
candidate fixes the same unsupported `--auto` and MCP `env` serialization,
but proposes a native catch-all allow for default/dontAsk and was held on that
permission decision. This integration retains the schema/flag corrections
and **does not adopt the catch-all grant**. Native permissions are retained;
questions are possible. The coordination instruction explicitly did not grant
permission to override configured denies. No other worktree was edited, no PR
was duplicated, and neither candidate has been published by this task.

Review overlap explicitly: `kanna-agent-protocol/src/mcp.rs`, `opencode.rs`,
and `kanna-server/src/task_creator/commands.rs` plus their contracts. Merge the
semantics, not both patches mechanically. This task is not a report that the
other task's proposed permission behavior was approved.

## Focused coverage

- Adapter tests cover native model IDs, auxiliary-model/provider restriction,
  initial/resume config, MCP `environment`, and absence of permission grants.
- An opt-in installed-CLI contract feeds the **actual generated Rust config**
  to OpenCode 1.4.3's `--pure debug config`. A task-local config supplies a
  catch-all deny and command-specific deny/allow rules; those survive the
  MCP/model overrides. No inference occurs.
- Server contracts cover PTY initial/resume argv/config, native inventory
  parsing, credential redaction, and an HTTP request that executes a
  repository-local fixture CLI. Model/override persistence uses existing
  stage-run fields, with provider/model added to the latest-run projection.
- Vue contracts cover local metadata, readiness wording, manual model entry,
  discovery errors/stale responses, initial task submission, one-time stage
  selection and its outbound request. Post-driven transitions use a separate
  save action through the existing pinned-workflow replacement API: tests
  preserve prompts/posts, assert no advance, retain compare-and-set input and
  operator source, reject ambiguous IDs, and surface stale edits without retry.
  Final-stage closes do not offer model selection.
- Installed CLI flag/config checks run without inference. Prior e218c437
  generated-launch evidence establishes the unchanged corrected argv/MCP
  schema's no-help TUI startup; it does not establish permission equivalence.

On the owner's reconciliation request, one narrow canonical isolated native
check was added in `tests/e2e/mock/opencode-model-selection.test.ts`. It exercises
the actual WebKit selector with fixture inventory and the real workflow API:
local connection/readiness text, changing to a cloud ID, preserving the pinned
post and prompts, and saving without advancing or creating a stage run. The
server test `opencode_saved_next_stage_model_is_resolved_after_post_completion`
separately verifies the saved selector is resolved when that post completes.
This is not an end-to-end live-agent stage transition or inference test.

Commands and results are recorded in the task's `.tmp/opencode-*.log` files.
No task-owned model servers were started for this implementation.

## Check results and limits

The installed OpenCode config contract also verifies that the selected provider
replaces a different configured provider allowlist and a cloud helper model,
while native permission denies survive. No grants are injected for default or
dontAsk. An unconfigured spawn leaves inherited inline config alone; Kanna's
MCP/model payload still owns OPENCODE_CONFIG_CONTENT when present, as documented
in the setup guide. This does not promise arbitrary inline-config composition.

Strict clippy encountered the pre-existing unused
`auth_ok_frame_with_terminal_geometry` function in `crates/kanna-server/src/ksp.rs`.
That file is unchanged. It is outside this integration and has not been edited.
The diagnostic clippy run records any further warnings separately; this is not
reported as a clean strict-clippy gate. No broad repository build or GUI matrix
was added to work around it.

Initial focused results before native verification: 115 desktop component/store/client tests, 31 OpenCode
server tests, 10 existing workflow replacement/switch contracts, 11 adapter
contracts (including installed OpenCode config parsing), and 10 installed CLI
flag/config checks passed. Desktop `tsc --noEmit` and `vue-tsc --noEmit` passed.
Diagnostic clippy for the two changed crates completed with only the baseline
unused-function warning above; strict `-D warnings` remains unclean for that
reason. No local-model coding run, cloud inference, model-server startup,
publication, task advance, or other task/worktree mutation occurred.

## Local handoff reconciliation

Rebased the integration onto fetched `origin/main`
`92554f25e` (September 12). The sole conflict was two independent API additions
at the end of `desktopServerClient.ts`; both the merged terminal-editor APIs
and the OpenCode APIs were retained. MainPanel, router, main and store changes
merged without conflicts. The diff against this base contains no editor or
shortcut removals. No other checkout was edited.

After reconciliation and the native findings below, 116 desktop tests, 31 server OpenCode tests, 11
installed/config adapter contracts, 10 installed CLI checks, and 11 workflow
replacement contracts were rerun successfully. The added post-resolution test
also passed. Both desktop TypeScript checks passed again. Strict clippy was
rerun and still fails only on the unchanged unused function named above.
`cargo fmt --all` ran; its unrelated baseline formatting changes were restored.
`git diff --check` passed. No broad suite or visual matrix was run.

All four experiment receipts and model artifacts remain under `.tmp`, with
child tasks retained. This integration is hosted-authored product code, not
output from the earlier local-model experiments. The requested integration
verdict supersedes the old comparison failure as the current stage result;
it does not change that experiment's recorded outcome. Local commit only:
independent review remains necessary before any publication.

The narrow native check found and drove fixes for two product gaps: snapshot
refreshes closed the selector even when task/stage identity was unchanged, and
the workflow schema rejected OpenCode's slash-bearing native IDs. The watcher
now compares the task/stage values individually; only OpenCode's schema branch
adds native ID punctuation. Regression tests cover both. The real API also
canonicalizes workflow selectors into arrays; repeated saves now use its
returned snapshot as the concurrency fence. Server and component tests exercise
that second save. No permission policy changed during these fixes.

The first native attempt failed before interaction on the harness's fixture
path guard: explicitly placing a fixture under the live worktree was refused.
The test now uses the framework's standard managed fixture directory without
weakening that guard; receipt/log/screenshot artifacts remain in task `.tmp`.
Subsequent failing native attempts and red/green regression logs are retained
in `.tmp/handoff`, not relabelled as passing runs.

Final native result: **1/1 passed** via
`pnpm --dir apps/desktop test:e2e mock/opencode-model-selection.test.ts`.
Receipt: `.tmp/handoff/native-e2e-verified.log`, `native-identity.json`,
`native-model-selector.png`. Verified native title:
`Kanna — task 7e983334 (0.0.68 @ 1214b3509)`, worktree/branch
`task-7e983334`, WebDriver `http://127.0.0.1:25976`.
The tested working tree contained the final runtime fixes above; subsequent
commit changes the Git stamp, not those tested sources. Screenshot inspected:
the selector and saved choice are readable. The fixture intentionally has no
agent session, so its terminal reports session-not-found; this is not a model
launch failure. Both saves use the real isolated server API; only inventory is
stubbed. The runner stopped its own app/server/daemon, and its listener ports
were clear afterward. Installed Kanna and other tasks were left alone.

Remaining UI limits: no live-agent post/next-stage execution, native New Task
submission, or datalist-popup keyboard interaction was exercised in this check.
New Task forwarding is covered by component/composable contracts; post model
resolution and launch config by server/installed-CLI contracts. No broad native
matrix, model inference, or new owner approval gate was added.
