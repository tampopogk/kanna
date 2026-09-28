# agent-tui

An interactive terminal client for **Claude Code** or **Codex**. You pick the
harness at launch; agent-tui then runs one live, multi-turn conversation with
it over the harness's own bidirectional JSON protocol:

- Claude: `claude -p --input-format stream-json --output-format stream-json`
  with the SDK control protocol on stdio (`--permission-prompt-tool stdio`).
- Codex: `codex app-server` (JSON-RPC over stdio).

```
agent-tui claude|codex [--model M] [--effort E] [--cwd DIR] [--skin NAME] [--bin PATH] [-- HARNESS ARGS…]
```

Examples:

```sh
agent-tui claude --model haiku --effort low
agent-tui codex --effort low -- -c approval_policy="on-request" -c sandbox_mode="read-only"
agent-tui claude --skin duke
```

Arguments after `--` are passed to the harness unchanged. agent-tui never
chooses a bypass permission mode; your harness configuration decides what
needs approval.

## Screen

- **Header**: harness, model, effort and state (Starting, Ready, Working,
  Needs approval, Needs input, Stopping, Disconnected, Degraded) with elapsed
  time. Values come from the harness's structured metadata. A value the
  harness doesn't report shows *Not reported*. A value you only asked for on
  the command line is marked *(requested)*.
- **Transcript**: user messages have a shaded background and an accent bar,
  and assistant text is plain. There are no speaker labels. Each turn's tool
  calls are collapsed into one line, e.g. `Tools · 3 completed · 1 failed ·
  1 running`. Expand it, then expand a call to see its input/output or diff.
  MCP calls also show server/tool, call id, duration, status and result, plus
  separate *Raw input* and *Raw output* JSON sections.
- **Request cards**: approvals and questions from the harness appear as cards
  offering the choices the harness supports. A new card never takes focus, so
  an Enter meant for your draft cannot answer it.
- **Composer**: always editable. While a turn runs, Enter keeps the draft and
  tells you why nothing was sent. There is no silent queue.
- **Footer**: key hints and the skin selector share one row.

## Keys

| Key | Action |
| --- | --- |
| Enter | Send (only when Ready) |
| Ctrl+J (and Shift+Enter where the terminal reports it) | Newline |
| Tab / Shift+Tab | Move focus: composer → pending request → transcript |
| ↑ ↓, PgUp PgDn, Home End | Select and scroll in the transcript; Enter or Space expands |
| Ctrl+C | Stop the current turn (shows Stopping, then “■ Turn stopped”) |
| Ctrl+Q | Quit; while a turn runs it asks *Stop & quit* / *Keep working* |
| Ctrl+F | Search the transcript (Enter / Shift+Enter or ↓ / ↑ between matches, Esc closes) |
| Ctrl+R | Raw JSON for the selected item, or the recent event log |
| Ctrl+L | Jump to latest and resume following live output |
| F1 / F2 | Help / skin picker |

On a focused card: ←/→ choose, Enter confirms, Esc leaves it pending. On a
question card, ↑/↓ choose an option, typing enters a free-text answer, and
Space toggles options on multi-select questions.

Scrolling up pauses live following. New output then shows
`↓ Jump to latest · N new` and never moves your view or focus.

## Slash commands

Type `/` at the start of the composer. ↑/↓ or Ctrl+N/P select (wrapping),
Tab or Enter completes, the next Enter runs, Esc closes and keeps your text.

- Local: `/help`, `/status`, `/theme [skin]`, `/stop`, `/new`, `/quit`.
- Claude: the commands Claude lists in its `initialize` response, labelled
  `claude`. They run by being sent as the message text; this was verified
  with `/context`. While a turn runs they show *when ready*.
- Codex: its app-server interface has no command list, so only local
  commands appear.

An unknown `/command` is never sent as a prompt. To send text that starts
with `/`, begin it with a space.

## Skins

Graphite, Matrix, Dracula, Nord, Solarized Light and Duke Nukem 3D
(`--skin graphite|matrix|dracula|nord|solarized|duke`, `/theme`, or F2).

Each skin has its own welcome text. The quote is picked once when the skin is
selected and then stays fixed; there is no rotation. The Duke skin draws the
owner-supplied sprite (`assets/duke-sprite.png`) with Unicode half-blocks: at
full size on the welcome screen and compact in the header. Below 60 columns
the header uses the `›_` badge. Colors are truecolor when `COLORTERM`
advertises it, otherwise the nearest 256-color values.

## Capabilities and known gaps

Verified against claude 2.1.283 and codex-cli 0.157.1 (September 2026). Each
row was checked with recorded live transcripts (`tests/fixtures`) and with
the opt-in live tests below.

| Capability | Claude | Codex |
| --- | --- | --- |
| Multi-turn prompts with retained context | ✓ | ✓ |
| Streaming text deltas | ✓ | ✓ |
| Model in header | ✓ reported (from `system/init` after the first prompt; before that `--model` is shown as *requested*) | ✓ reported (`thread/start`) |
| Effort in header | ✗ not reported by Claude: shows `--effort` as *(requested)* or *Not reported* | ✓ reported (`reasoningEffort`) |
| Tool calls, output, diffs | ✓ | ✓ (commands, file changes, MCP, web search) |
| MCP calls with raw input/output | ✓ | ✓ |
| Approvals (allow / deny by request id) | ✓ | ✓ commands, file changes, MCP tool elicitations |
| Questions | ✓ AskUserQuestion | schema only: `item/tool/requestUserInput` was not observed live |
| Interrupt | ✓ | ✓ (sent as soon as the turn id is known) |
| Harness slash commands | ✓ | ✗ none exposed |

Other gaps:

- Codex `item/permissions/requestApproval` is implemented from the generated
  schema only. MCP elicitations that ask for form fields can only be declined
  or cancelled.
- codex 0.157 omits `decline` from a command approval's `availableDecisions`
  but accepts it (see fixtures), so agent-tui always offers a *Deny* that
  lets the turn continue, alongside *Deny and stop turn*.
- Harness requests agent-tui does not support (e.g. Codex dynamic tool calls,
  Claude hook callbacks) are answered with a protocol error and shown as a
  diagnostic. They are never approved.
- Shift+Enter needs the kitty keyboard protocol (kitty, WezTerm, recent
  iTerm2/Ghostty). It isn't available in tmux. Ctrl+J always works.
- No mouse support.
- Not in this version: viewing saved logs, concurrent sessions, switching
  provider, resuming across launches, and remembering the skin between
  launches.

## Robustness

- **Framing**: stdout is framed as JSONL. Records split across reads, CRLF,
  blank lines and a final line without a newline are all handled. A malformed
  line becomes a visible diagnostic, keeps its raw text and line number, and
  later records keep flowing.
- **Degraded state**: if a malformed record looks like a control, approval or
  completion record, the session shows **Degraded** and sending is disabled
  until a turn completion re-establishes state. You can also stop with Ctrl+C
  or restart with `/new`.
- **Disconnects**: EOF or process exit gives **Disconnected**. The
  transcript and draft are kept, the stderr tail is shown, and nothing is
  resent. `/new` starts a fresh session.
- **Untrusted text**: provider text is sanitized. Control and escape
  sequences are shown as visible symbols (`␛[31m`), never executed.
- **Bounded memory**: the transcript keeps 5,000 entries, the raw log keeps
  20,000 records, and records or tool output over 1 MB are truncated for
  display with a notice. Trimming the display never resets the harness's
  context.
- **Process cleanup**: the harness runs in its own process group. On quit,
  agent-tui interrupts a running turn, closes stdin, waits up to 3 s, then
  signals the group, so no tool subprocess outlives the client. SIGHUP (terminal
  closed), SIGTERM and SIGINT take the same path and restore the terminal.

Logs go to `~/Library/Caches/agent-tui/agent-tui.log` (macOS) or
`$XDG_CACHE_HOME/agent-tui/agent-tui.log` (default `~/.cache`). Set
`AGENT_TUI_LOG=debug` to include the harness's stderr.

## Development

```sh
cargo test                     # unit, fixture replay, behavior and render snapshot tests
cargo test --features live --test live -- --ignored --test-threads=1
```

The live tests need signed-in `claude` and `codex` on PATH, and use a small
model at low effort. For each harness they check that:

1. the header shows the reported model (and effort for Codex);
2. a prompt gets a streamed reply;
3. a follow-up uses the first turn's context;
4. a command needing approval can be denied and then allowed on retry;
5. interrupting a long turn ends with “■ Turn stopped” and returns to Ready;
6. quitting leaves no process in the harness's process group.

The last run passed all six checks for both harnesses.

`tests/fixtures/*/*.transcript` are sanitized recordings of live spike
sessions: `<` lines were received and `>` lines were sent. `tests/replay.rs`
reproduces every sent record through UI actions and checks the app emits an
equivalent message. `tests/fixtures/codex/schema` holds the relevant part of
`codex app-server generate-json-schema` output.

## Kanna import provenance

Imported from `/Users/jeremyhale/.kanna/repos/foobar-24`, approved commit
`859dcd7d495e219e17d1cff200f447e25993cef5`, for successor task `93a5a7d0`
of task `2e6fbb4a`. All tracked files were compared byte for byte before
this provenance note and subsequent integration changes.

Design authority: document artifact `ad68cbd2a1f2927f519089391b77cfac25325448`.
Duke sprite authority: artifact `de92bf19dfe5fdec64918f3c3f42b9ed6f0fec9c`.
Both artifacts remain in original repository `repo-18d8f429a6316298`; these
references do not relocate or modify the original task or artifact store.
The original Cargo.lock is retained as provenance; Kanna's workspace lockfile
governs integrated builds.

### Hosting integration status

Kanna builds and bundles this crate, but does not yet launch it as a task
frontend. `HostedLaunch` and the correlated-input adapter methods are library
foundations; the executable has no `--kanna` mode yet. The standalone live checks
above are upstream evidence, not Kanna-hosted acceptance. See
[the partial implementation and verification record](../../docs/agent-tui/hosting-foundation.md)
for completed work and the remaining approved scope.
