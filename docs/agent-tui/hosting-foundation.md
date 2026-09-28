# Agent TUI hosting foundation — partial implementation

Task `93a5a7d0`, build workspace `task-93a5a7d0-2`, 2026-09-28.
This is **not a completed Kanna-hosted frontend**. Kanna still launches native
provider terminals. The `HostedLaunch` parser and correlated-input methods are
library foundations; the executable does not yet expose `--kanna`.

## Preserved source and build integration

Imported all 75 tracked files from
`/Users/jeremyhale/.kanna/repos/foobar-24` at
`859dcd7d495e219e17d1cff200f447e25993cef5`. Every file matched the source commit
byte for byte before adding the README provenance note. Import commit:
`073f522f3`. Workspace/build integration commit: `1dfdc7453`.

Original task: `2e6fbb4a`, repository `repo-18d8f429a6316298`.
Design artifact: `ad68cbd2a1f2927f519089391b77cfac25325448`.
Sprite artifact: `de92bf19dfe5fdec64918f3c3f42b9ed6f0fec9c`.
The original source, task and artifact stores were read only.

The crate is a Cargo workspace member and a seventh desktop sidecar. Cargo
staging, Tauri packaging, macOS Bazel bundle lists, and Linux product/install
lists include it. The root Cargo lock governs workspace builds; the retained
standalone lock seeds the repository's separate Bazel crate universe.

## Provider contract work

`HostedLaunch::parse` accepts the native argv contract described in the approved
plan, retains argument boundaries, and extracts a nonempty initial prompt without
sending it. Unknown options and ambiguous session bindings fail explicitly.
Claude's transport flags remain adapter-owned; its MCP, permission, tool, budget,
system-prompt, model, effort, autocompact and session flags are preserved.
Codex retains ordered `-c` overrides for app-server, selects `thread/start` or
`thread/resume`, and checks the returned resume identity. An explicit sandbox
does not implicitly disable approval. Explicit `--yolo` maps both bypass settings.

The adapters can produce correlated input receipts. Claude requires a matching
replayed user UUID, content and bound session; Codex requires the matching
`turn/start` result with a nonempty turn ID. A write or unrelated notification
is not acceptance. Codex errors produce rejection receipts; an invalid successful
response degrades the connection without inventing acceptance.

The app's new logical-input method preserves the human composer and focus,
bypasses local slash handling, and refuses dispatch while working or while a
card is pending. **It does not queue input**. A future host event loop must own
queuing, persistence, receipt draining and ready-boundary dispatch.

## Verification performed

- Import comparison: all 75 source files equal before provenance edits.
- Imported baseline: 82 tests passed in the Kanna Cargo workspace.
- Updated crate: 97 tests passed (96 in the full suite, then the final added live
  receipt fixture in the seven-test receipt suite). Existing snapshots unchanged.
- `cargo clippy -p agent-tui --all-targets -- -D warnings`: passed.
- `./kd build sidecars`: built and staged all seven sidecars, including a final
  rebuild after the adapter changes.
- Focused kd packaging tests: 26 passed; desktop sidecar tests: 8 passed.
- `cargo test -p kanna-runtime-defaults linux_install`: 4 passed.
- `git diff --check`: passed.

The inherited shell selected Zig 0.15.2 and carried `CARGO_BUILD_BUILD_DIR` and
`RUSTC_WRAPPER` from another Kanna worktree. Verification unset
`CARGO_TARGET_DIR`/`CARGO_BUILD_BUILD_DIR`, disabled the inherited Rust wrapper for
focused Cargo runs, and selected the already-cached Zig 0.16.0 through PATH for
`kd build sidecars`. All resulting Cargo output is in this workspace's `.build`.
No global tool installation or source change was needed.

Bazel target analysis succeeded and generated `MODULE.bazel.lock`, but execution
is **unverified**. The build failed in rules_rust's own host dependency
`thiserror-impl 1.0.69`, before compiling agent-tui. Disabling the shared disk cache
and rebuilding reproduced it. `codesign --verify` succeeds on that dylib, but
`dlopen` reports `mis-aligned LINKEDIT string pool, fileOffset=0x00167F6C`; rustc
reports E0463 for `thiserror_impl`. Root cause (fixed separately): exec-config
(opt) rustc runs `-Cstrip=debuginfo`, which on Apple rewrites the dylib with its
bundled `rust-objcopy` and can leave the LC_SYMTAB string table only 4-byte
aligned; macOS 27 dyld rejects that. `MODULE.bazel` now passes `-Cstrip=none` via
`extra_exec_rustc_flags`, guarded by `//tools/bazel:exec_proc_macro_macho_test`.
Neither Linux release packages nor an installed desktop
bundle were built.

### Direct protocol probes (not hosted acceptance)

Claude Code `2.1.284`, model `haiku`, effort `low`, stream-json with
`--replay-user-messages`, assigned session
`4adc0e06-42a6-4694-9610-94cfdd4068da`. The input UUID
`f5a383e0-7893-4758-9e42-f438f38240ac` was echoed with the same text and session,
`isReplay: true`, and a subsequent completed result. The actual request/echo is
retained in `crates/agent-tui/tests/fixtures/claude/hosted_ack.transcript` and tested.
Native transcript:
`~/.claude/projects/-Users-jeremyhale--kanna-repos-kanna-6--kanna-worktrees-task-93a5a7d0-2--tmp-claude-probe/4adc0e06-42a6-4694-9610-94cfdd4068da.jsonl`.

Codex CLI `0.158.0`: generated the installed app-server JSON schema; its
`ThreadStartParams` and `ThreadResumeParams` accept `sandbox`, `approvalPolicy`
and `config`, with `threadId` required for resume. The live probe used `gpt-6-sol`,
`model_reasoning_effort="low"`, `read-only`, and `on-request`. It initialized,
started thread `01a0e9a8-6403-73d1-ace6-ae6947951e91`, sent a turn with marker
`HOST-RESUME-93A5`, stopped app-server, reopened the same thread in a new process,
and recalled exactly that marker in a second completed turn. Each turn had a
correlated `turn/start` response with a turn ID. Native transcript:
`~/.codex/sessions/2026/09/28/rollout-2026-09-28T14-15-22-01a0e9a8-6403-73d1-ace6-ae6947951e91.jsonl`.
The official [app-server reference](https://learn.chatgpt.com/docs/app-server)
also documents `thread/resume` by recorded thread ID. Installed schema and live
exchange governed the field choices here.

These probes used task-owned directories under `.tmp/`, not fixture tasks or a
Kanna terminal. They do not prove MCP configuration, task input delivery, workflow
completion, or `kanna_resume_task`. Their process groups were terminated and
waited for. Temporary scripts, logs and schema output remain under `.tmp/`.

## Remaining approved scope

1. Typed `agentFrontends` configuration and resolution across fresh/resume/post/
   revision/recovery launches; explicit bundled-sidecar lookup and `--kanna` mode.
   Enable this repository's frontend map only when integration is ready.
2. Authenticated, versioned host socket with incarnation/run/generation fencing,
   bounded FIFO queue, initial-input ordering, and structured state/identity.
3. Durable server delivery attempts, migration/rebuild/transfer classification,
   idempotency, submitting/uncertain boundaries, status API/CLI/MCP, attachment
   retention, idempotent delivered ledger publication, and 202-aware clients.
4. Daemon routing, reconnection/adoption, queue retirement on run replacement,
   process inventory/cleanup, runtime detection precedence, composer attestation,
   provider rejection recovery, exact-run identity persistence, resume history,
   and hosted `/new` refusal.
5. Deterministic server/daemon/frontend process tests and the applicable Rust gate.
   Real Kanna-hosted tasks for BOTH providers: MCP stage completion, idle/mid-turn
   multiline input, cards/raw keys/drafts/interrupts, post/revision dispatch,
   same-conversation resume, terminal resizing/scrolling/skins and cleanup.

No frontend selection has been enabled. There is no exactly-once delivery claim,
no queue durability claim, and no Kanna-hosted acceptance claim. No branch was
pushed and no PR was opened. The stage must remain `partial`.
