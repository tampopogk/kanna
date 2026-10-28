# Agent Status

**Status:** built (no dots yet: the Desk isn't built) · **Tested:** e2e `agent_status`, `real_agent_program_status`; `cargo test -p kanna-vt`, `-p kanna-daemon` · **Used by:** [Desk](desk.md) (the agent header and the Shelf) · **Mockup:** the Desk's ([`references/mockups/desk.html`](../../references/mockups/desk.html)), the dots beside each task · **From:** kanna v2/v3's detection rules (`kanna3-detect`) and herdr's agent-detection manifests

**Agent status** tells you at a glance whether an agent is busy, waiting for you, or done, so you can leave it running and come back when it needs you. An agent that reports its own status ([OSC 7501](#4-the-agents-own-report-osc-7501), as Claude Code does) is taken at its word. For the others, Kanna reads the bottom of the agent's screen and matches it against **rules**: patterns for each agent, kept as data that Kanna updates on its own, so a new Claude Code or Codex release never needs a new Kanna.

![The dots: the task on the Desk is working (blue); on the Shelf, one needs you (amber) and one is idle (green)](desk.png)

## Contents

1. [States](#1-states)
2. [Rules](#2-rules)
3. [Detection rules](#3-detection-rules)
4. [The agent's own report (OSC 7501)](#4-the-agents-own-report-osc-7501)
5. [CLI](#5-cli)
6. [How it's built](#6-how-its-built)
7. [Glossary](#7-glossary)

## 1. States

```mermaid
stateDiagram-v2
    direction LR
    [*] --> working: starts
    working --> needs_you: a question or approval on screen
    needs_you --> working: answered
    working --> idle: back at its prompt
    idle --> working: a new turn
    working --> exited: program ends
    idle --> exited
    needs_you --> exited
```

| Status | When |
|---|---|
| working | the agent reports working, or a busy rule matches (spinner, "esc to interrupt", a busy title or progress) |
| needs you | the agent reports blocked, or a waiting rule matches: an approval, a question or a trust prompt is on screen |
| idle | the agent reports idle, done or error, or is at its prompt, or no rule matches |
| exited | the program has ended (the pane is `exited`) |

## 2. Rules

- **Known agents only:** a terminal whose foreground program is an agent Kanna has rules for (Claude Code, Codex, …) has a status; other terminals show only `exited`.
- **Needs you is strict:** it shows only when the screen matches a known question or approval. A prompt Kanna hasn't learned shows as idle, never as needs you by guesswork.
- **Settled screens only:** Kanna judges a screen once it stops changing mid-frame, so a half-drawn repaint never flips the status.
- **The agent's report, else the screen:** an [OSC 7501](#4-the-agents-own-report-osc-7501) report wins whenever there is one; the bell and the agents' own notifications (OSC 9, OSC 777) aren't used. They depend on each agent's settings and, for Codex, on focus reports ([evidence](../research/agent-status-signals.md)); the screen doesn't.
- **Shown in Kanna only:** status is the dots in Kanna's windows; it doesn't badge the Dock icon or post macOS notifications.
- **Same for every agent:** each agent differs only in its rule file.
- **Rules are data: they can't choose what Kanna runs.** Which programs are agents is Kanna's own code; the version probe runs only the agent's own executable, as the OS reports it, with `--version`; a rule file gives only the pattern that reads the answer.
- **Unread is separate:** an agent that goes from working to idle while you aren't looking at it is **unread** until you do. Unread is the app's mark of what you've seen, not an agent status: the status stays idle, and the dot shows both.

## 3. Detection rules

```mermaid
flowchart LR
    S["the screen's last 10 lines<br/>+ title + progress"] --> E["rule engine"]
    R["rules for this agent<br/>(override › newer of remote and bundled)"] --> E
    E --> V["working · needs you · idle"]
    H["rules site"] -->|"checked every 30 min,<br/>hot-loaded"| R
```

- **One file per agent**, in JSON: a `version`, and rules each with a status, a priority, the agent versions it applies to, and a test. The highest-priority match wins.
- **What a test reads:** the screen's last 10 lines by default (or another region: last non-empty lines, lines below a divider), the terminal title and progress reports. It checks with `contains`, `regex`, and `all`/`any`/`not`.
- **Agent versions:** Kanna asks each agent binary its version once (`<its executable> --version`, nothing from the rule file) and uses only rules whose range includes it, so a team on different releases is each judged right.
- **Hot-loaded, three sources:**
  1. **Bundled** in Kanna.
  2. **Remote:** Kanna checks the rules site, verifies each file's hash against the site's index, and swaps the newer rules in without a restart.
  3. **Local override** in `~/.config/kanna/agent-rules/<agent>.json`, reloaded when it changes, which always wins.
- **Every rule file is checked whole before it's used**, wherever it came from: at most 256 KiB, 256 rules, 64 vocabulary lists and tests nested 24 deep; patterns capped and compiled with size limits; regions at most 200 lines; known statuses, tests, regions and agents only. One that fails is refused and logged, and the rules in use stay. From the site, an entry must name a known agent (`a-z`, `0-9`, `-`), its file must be on the site's own host (redirects too) and match the index's hash before it's read, and a file no newer than the one Kanna has is ignored.
- **Last 10 lines** means the last 10 rows of the screen once blank bottom rows are dropped, so an agent whose prompt sits halfway down a tall pane is read where it is. A region (`screen`, `lastLines`, `lastNonEmptyLines`, `firstNonEmptyLines`, `linesAbove`/`linesBelow` an anchor, `between` two anchors) is taken from the whole screen.
- **Which programs are agents** is Kanna's code: `claude`, `codex`, `opencode`, `copilot`, `agy`/`antigravity`, and `mock-agent` (tools/mock-agent, for tests), matched against the foreground process group's executable, with symlinks resolved (its file name, or `<agent>/versions/<n>` as Claude Code installs), else its name or a script an interpreter runs (`node …/codex.js`). Only an agent known by its resolved executable is asked its version, and that exact path is what runs; one known only by name has no version. An agent's rule file is `agent-rules/<agent>.json`.
- **Proved on a corpus:** recorded screens from real Claude Code and Codex, driven by a mock model so no tokens are spent, each labeled with its status. A rule change ships only when the whole corpus passes.

### When status changes

- **Settled:** no output for 60 ms and no synchronized update (mode 2026) open; output that never pauses is judged once a second anyway, and a synchronized update open longer than a second is ignored.
- **Working → idle waits 300 ms:** an idle verdict right after working must still hold 300 ms later, so a frame between two steps of a turn doesn't flash idle. Every other change shows at once.
- **Unread** (the app's): an agent that goes from working to idle while its pane isn't on screen in the window Kanna has in front ([physical layout §3](physical-layout.md#3-the-window-in-front-in-the-app)) (in its active tab, zoomed pane or dock), with Kanna active, is unread until it is; leaving idle clears it.
- **Names on the wire:** `working`, `needs-you`, `idle`, `exited`.

## 4. The agent's own report (OSC 7501)

The [Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status) (draft rev 0.4) lets a program tell its terminal what it's doing: `ESC ] 7501 ; state=working|blocked|idle|done|error[:kind=…][:progress=N][:id=…][:app=…][:title=b64][:msg=b64] ST`. Claude Code sends it (since 2.1.295) once its terminal answers the probe `OSC 7501 ; ?`; Codex (since 0.163.0) sends it without asking, whenever its output is a terminal. Kanna takes reports whether or not the program asked. Agents that don't report (OpenCode, Copilot, older releases) keep the rules.

```mermaid
flowchart LR
    A["agent<br/>(Claude Code)"] -->|"OSC 7501 ; ?"| VT["daemon's terminal<br/>(libghostty-vt in kanna-vt)"]
    VT -->|"OSC 7501 ; ?  (the answer)"| A
    A -->|"OSC 7501 ; state=…"| VT
    VT --> REC["the pane's records<br/>one per id"]
    PG["foreground process group<br/>when the report came"] --> REC
    REC -->|"root record, from the<br/>program in the foreground"| D{"a record?"}
    S["screen + rules"] --> D
    D -->|yes| ST["status"]
    D -->|no| ST
```

- **Precedence:** the pane's root record (no `id`) decides whenever there is one: working is working, blocked (any kind) is needs you, idle, done and error are idle. With no record, the [rules](#3-detection-rules) decide as before. It shows at once: no settling and no 300 ms hold.
- **Only the daemon's terminal answers the probe,** so the agent gets one answer; the app's Ghostty parses the reports and ignores them.
- **Records** are kept per pane, one per id, each report replacing its record whole (children such as `build/test` too; `state=clear` removes a record and those beneath it; one without an `app` takes its nearest ancestor's). At most 256; the one updated longest ago goes.
- **When records go:** working and blocked ones when a new prompt starts (OSC 133 A) or the program ends (another process group takes the terminal's foreground, or the pane exits); all of them at a full reset (RIS). A record counts only while the process group that reported it is in the foreground, so a `done` left by one program never speaks for the next.
- **A turn's end:** a record going from working to idle or done ends a [backlog task's turn](backlog-runners.md) at once, alongside the 3s rule.
- **Untrusted text:** `msg`, `title` and `app` are shown only by `kanna status explain`, with control and invisible formatting characters taken out, and never sent back to the program.

## 5. CLI

| Command | Does |
|---|---|
| `kanna workspace list --all --json` | Adds `status` to each agent pane |
| `kanna status explain PANE` | The agent, its version, where its status came from (`source: OSC 7501` with the record, or `source: rules`), the rules file and its source, the matching rule and the text it matched |
| `kanna status explain --file SCREEN --agent claude` | The same for a saved screen, for writing rules |
| `kanna status update` | Check the rules site now |

`KANNA_AGENT_RULES_URL` sets the rules site (`none` turns it off; the tests do), `KANNA_AGENT_RULES_DIR` the override folder.

## 6. How it's built

- **In the daemon**, which already keeps every terminal's screen ([app link](app-link.md#3-rules)), so status works with the app closed and `list` reads it directly. The daemon knows each terminal's foreground program, which picks the agent. The code: `crates/kanna-detect` (the rules, their sources and the foreground program, from v3's `kanna3-detect`), `crates/kanna-daemon/src/status.rs` (one thread judging settled screens) and the `status` command; rule files and the corpus in `agent-rules/`. Checked by `cargo test -p kanna-detect` (the corpus and the rule store), `crates/kanna-daemon/tests/status.rs` and `agent_status`.
- **The app** gets a `status {pane, status}` notice on every change, and one per pane after `app.hello`, and keeps status and unread in its state (`debug.status`).
- **Rules site:** the repo's GitHub Pages, `https://jemdiggity.github.io/kanna-v4/agent-rules/index.json`: the rule files and an `index.json` (agent, URL, version, hash), built by `scripts/build-agent-rules-site.sh` and published by `.github/workflows/agent-rules.yml` with `cargo test -p kanna-detect` (the corpus) as the gate. A changed rule file needs a higher `version`, or Kannas that have the old one ignore it.
- **The corpus and harness:** `tools/agent-harness` (the mock model, from v3's characterization tools) records sessions; `kanna-corpus` (`crates/kanna-corpus`) replays them through libghostty-vt, samples the settled screens by the harness's phases and writes them into `agent-rules/corpus/<agent>/`. `cargo test -p kanna-detect corpus` prints a confusion matrix per agent.
- **Seeded** from v3's rules (the user's own, via v2); herdr (Apache-2.0) is a reference for behaviour only, and none of its rule text is copied.

## 7. Glossary

| Term | Meaning |
|---|---|
| **Unread** | An idle agent you haven't looked at since it finished; kept by the app, apart from the status |
| **Rule** | A pattern on an agent's screen, title or progress that means one status |
| **Rules site** | Where Kanna fetches updated rule files from |
| **Corpus** | Recorded agent screens, each labeled with its true status, that every rule change is tested against |
| **Mock model** | A local stand-in for the AI provider that drives real Claude Code and Codex without spending tokens |
| **OSC 9;4** | The escape sequence a program uses to report progress to its terminal |
| **OSC 7501** | The Program Status Protocol: a program's own report of what it's doing, kept as one record per id |
| **Root record** | The record a report with no `id` makes: the program itself |
