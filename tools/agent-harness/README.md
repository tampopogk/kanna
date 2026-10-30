# Agent harness

Runs Claude Code and Codex in a pseudo-terminal against a local mock model, and records every byte they write, with timestamps. No real model, account or login is used: each run gets its own HOME (and `CLAUDE_CONFIG_DIR` / `CODEX_HOME`) under `out/`, pointed at the mock with `ANTHROPIC_BASE_URL` / a `mock` model provider and the key `mock`.

The mock server comes from Kanna v3's `tools/characterize` (see `NOTICE`).

## Run

```sh
python3 tools/agent-harness/run.py --out tools/agent-harness/out/claude --agents claude \
    --terminals kanna,noprogram --settings default,bell --focus here,away
python3 tools/agent-harness/signals.py tools/agent-harness/out/claude/*-here.jsonl
```

Needs `claude` and `codex` on `PATH`, Python 3.9+, and permission to open a pty and listen on 127.0.0.1 (outside the Claude Code sandbox, or with local binding allowed). A run takes about two and a half minutes; `--jobs` runs several at once.

| Option | Values |
|---|---|
| `--agents` | `claude`, `codex` |
| `--terminals` | `kanna`: the environment a Kanna terminal gives (`TERM=xterm-ghostty`, `COLORTERM=truecolor`, `TERM_PROGRAM=ghostty`, `TERM_PROGRAM_VERSION=1.3.2-dev+4ec3122`); `noprogram`: the same without `TERM_PROGRAM` / `TERM_PROGRAM_VERSION`; `xterm`: `TERM=xterm-256color` only |
| `--settings` | Claude: `default`, `bell`, `ghostty`, `iterm2_with_bell` (`preferredNotifChannel` in the run's `settings.json`). Codex: `default`, `on`, `always`, `always_bel` (`[tui]` in the run's `config.toml`) |
| `--size` | `COLSxROWS` of the terminal (default `120x40`); another size is added to the recording's name |
| `--focus` | `here`: the terminal reports focus-in once and keeps it; `away`: it reports focus-out after each prompt is sent and focus-in before the next |

## What a run does

1. Starts the agent and waits for its prompt (`startup`).
2. Sends `PLAIN_TURN…`; the mock streams a text answer for about 8 s (`plain`, `plain_end`).
3. Sends `TOOL_TURN…`; the mock asks to run `touch kanna-probe.txt` (Claude in `--permission-mode default`; Codex with `on-request` and `require_escalated`), so the agent asks for approval (`tool`, `asking`).
4. Waits 25 s at the approval prompt, then presses Enter to approve (`approve`).
5. Sits at the prompt for 80 s (`idle`), then stops the agent (`end`).

## What it records

- `out/<dir>/<agent>-<terminal>-<setting>-<focus>.jsonl`: one line per event: `o` (output bytes, base64), `i` (keys sent), `answer` (replies to terminal queries: DA1, cursor position, XTVERSION as Ghostty, kitty keyboard, DECRQM, OSC 10/11 colours, window size), `mark` (the phases above), `matched` / `timeout`, `exit`. `t` is seconds since start.
- `out/<dir>/mock-requests.jsonl`: every request the agents made to the mock, with its last user message (`prompt`) and every user message in it (`history`), leaving out `<system-reminder>` blocks. `turn` is what the mock answered an agent's turn with; it's null for a request that offers no tools, such as codex's task title, whose prompt quotes the user's message.
- `out/<dir>/runs/<name>/`: the run's HOME, working folder and script.

Codex starts a detached `codex app-server --managed-daemon` from its CODEX_HOME that outlives the TUI; `run.py` stops any process running from the run's HOME when the run ends.

`signals.py` reads recordings and lists, with time and phase, every BEL, OSC 0/1/2 (title), OSC 9 (notification), OSC 9;4 (progress), OSC 777 (notification), OSC 99, OSC 1337 and focus-reporting request. `--no-titles` leaves titles out; `--json` prints them as JSON; `--text N` prints the last N characters of the screen text instead.

## Files

| File | Is |
|---|---|
| `mock_server.py` | Anthropic Messages, OpenAI Responses and Chat Completions, answering from `scenarios/status.json` |
| `record.py` | Runs one argv in a pty from a JSON script of steps (`wait`, `expect`, `type`, `key`, `focus_in`, `focus_out`, `mark`) |
| `isolate.py` | An agent's isolated HOME and config pointed at the mock; as a command, writes an executable that runs the agent that way (Kanna's e2e `real_agent_tasks` runs real claude and codex tasks with it) |
| `run.py` | Prepares each run's isolated config and runs the matrix |
| `signals.py` | Lists the status signals in recordings |

## Agents as Kanna tasks

`isolate.py` writes an executable that runs an agent against the mock in a HOME of its own, with the parent's `CLAUDE*`, `CODEX*`, `ANTHROPIC*` and `OPENAI*` variables cleared, so Kanna can start it as a task's agent:

```sh
python3 tools/agent-harness/isolate.py --agent codex --home /tmp/h --port 8123 --trust ~/repo --out /tmp/bin/codex
```

The executable is named after the agent, so Kanna treats it as one. An agent installed as a script for an interpreter beside it but not on PATH (codex from nvm, `#!/usr/bin/env node`) gets that folder on its PATH, so it runs from a terminal whose PATH has no nvm. `scripts/e2e-kanna.sh real_agent_tasks` uses it to check that claude and codex tasks come back to their conversation after Move to Backlog, Stop and Start: the mock's log shows the next message carrying the first.
