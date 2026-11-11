# Codex's app-server

**Status:** built · **Tested:** `cargo test -p kanna-daemon` (codex_remote, messages), e2e `agent_messaging`, `planner_choice`, `shelf_runners` (the mock agent as `codex`), `real_agent_tasks` (the real Codex) · **Builds on:** [tasks](tasks.md), [agent messaging](agent-messaging.md) · **Why:** [research](../research/agent-messaging.md), checked against Codex 0.163.0

Kanna starts every Codex, a task's, the planner's or a quiz's, with an **app-server of its own**, and the Codex TUI attached to it. The server is Codex's door for [agent messaging](agent-messaging.md): Kanna starts a turn on it instead of typing into the prompt.

## Contents

1. [How Codex starts](#1-how-codex-starts)
2. [Delivering a message](#2-delivering-a-message)
3. [Costs](#3-costs)
4. [Glossary](#4-glossary)

## 1. How Codex starts

```mermaid
sequenceDiagram
    participant T as agent terminal
    participant H as kanna-daemon --codex-remote
    participant S as codex app-server
    participant U as codex TUI
    T->>H: kanna-daemon --codex-remote STATE codex --yolo 'FIRST'
    H->>S: codex app-server -c approval_policy="never" -c sandbox_mode="danger-full-access" --listen unix://STATE/codex/codex-PID.sock
    Note over S: the terminal's environment: KANNA_TERMINAL, KANNA_SOCKET
    H->>U: codex -c approval_policy="never" -c sandbox_mode="danger-full-access" 'FIRST' --remote unix://STATE/codex/codex-PID.sock
    Note over U: the TUI as usual, in the terminal
    U-->>H: quits
    H->>S: stop, remove the socket
```

- **The helper is the daemon's own binary** (`codex_remote.rs`), so it needs no shell syntax and works whatever your login shell is.
- **The server gets the terminal's environment,** so the commands Codex runs see the task's `KANNA_TERMINAL` and `KANNA_SOCKET`. That's why Kanna used to start Codex with `--no-daemon`: Codex's shared background server runs every session's commands with its own environment. A server per Codex is the task's own.
- **Permissions go on the server and on a new session's TUI:** `--yolo` becomes `approval_policy="never"` and `sandbox_mode="danger-full-access"` (`-c`) for both. The TUI starts the session's thread with its own config, so without them a [trusted](https://developers.openai.com/codex/config-reference) folder's `on-request` and `workspace-write` win over the server's, and a task's `git commit` (its refs are in the main repo's `.git`, outside the sandbox) asks to escalate.
- **Resume keeps the session's own:** `codex resume ID --remote unix://SOCKET` brings the session back on a new server, with the permissions it was started with; the TUI gets no permission flags or `-c`, since Codex refuses them when resuming against a server ("Permission overrides are not supported when resuming a remote task").
- **One writer at a time:** a thread can be open on one app-server only: while the old server lives, a new one's `thread/resume` fails with `thread ID already has an active writer`, and the TUI shows the conversation read-only ("This conversation is open in another app"). So a restarted terminal starts only once the old terminal's session is empty ([below](#restarting)).
- **The socket** is `codex-PID.sock` in Kanna's state folder's `codex/`, or in `/tmp/kanna-UID` when that path is too long for a Unix socket; a leftover from a helper that's gone is removed at the next start.
- **Only you can reach it:** the server takes any connection without auth, and a turn on it runs with the agent's permissions, so the folder must be yours alone: the helper makes it 0700 (tightening one of yours that others could open) and checks it's a real folder, owned by you, that no one else can open (a `/tmp` name someone else made first, or a symlink, fails), else Codex runs on its own.
- **Quitting:** `/exit` and its quit keys work as before ([Shelf runners](shelf-runners.md#2-how-its-built)); attached, Codex says its session as `codex --remote unix://SOCKET resume ID`, which Kanna reads like `codex resume ID`.
- **If the server won't start,** the helper says so in the terminal and runs Codex on its own (`--no-daemon`): it works, but takes no messages.

### Restarting

`kanna restart` closes the old terminal and starts the new one, and the keeper only signals on a close: SIGHUP to the session, SIGKILL to what's left after a second. Starting at once would race the old server for the thread's writer lock, and lose.

- **The new terminal waits for the old session:** `Terminals` watches the session off the loop, every 20 ms, and starts the pane once no process is left in it. The pane stays `starting` until then.
- **At most 6 s:** past the helper's 5 s. A session that outlives that doesn't hold the pane back.
- **It's every terminal's rule, not Codex's:** a terminal the keeper closes is gone in a second, so a shell's restart waits only as long as its shell takes to go.
- **Stopping or closing the pane while it waits** drops the start, as for one the keeper hasn't answered yet.

## 2. Delivering a message

| Step | JSON-RPC over a WebSocket on the socket |
|---|---|
| Hello | `initialize` with `experimentalApi`, then `initialized` |
| Find the session | `thread/loaded/list`, then `thread/read` each: the one that isn't `ephemeral` (Codex also loads a thread to title the session) |
| Deliver | `thread/queue/add` with the message as text input; `turn/start` at its prompt if Codex refuses the queue |
| Follow | `thread/queue/list`, every 2s while a message is queued; `thread/queue/start` when it's stuck after Esc |

- **Kanna finds the socket** from the arguments of the pane's Codex: the `--remote unix://SOCKET` its TUI was started with.
- **Queued at any time:** on a busy Codex, `turn/start` [drops the answer it's streaming](agent-messaging.md#2-delivery), and the queue doesn't.

## 3. Costs

| | `codex --no-daemon` (before) | App-server and TUI |
|---|---|---|
| Processes per Codex | 1 | 2, plus the helper |
| Memory, idle (macOS footprint) | 182 MB | 191 + 51 = 242 MB |

- **About 60 MB more per running Codex;** a stopped task runs nothing either way.
- **Experimental:** Codex marks the app-server and `--remote` experimental; the protocol is versioned (v2), and the helper's fallback keeps Codex working if they change.

## 4. Glossary

| Term | Meaning |
|---|---|
| **App-server** | Codex's JSON-RPC server: it runs sessions, and TUIs and other clients attach to it |
| **Helper** | `kanna-daemon --codex-remote`: starts the server, runs the TUI attached to it, stops the server after. On SIGHUP (its terminal closed), SIGTERM or SIGINT it sends both SIGTERM, SIGKILLs whichever is left after 5 s, and exits, so no server outlives its terminal. A signal thread waits for those signals (retrying `sigwait` if it fails) and ends the helper with `_exit`, so it never runs `exit` alongside the main thread |
| **Session thread** | The thread a Codex session runs in; a queued message runs on it as a new prompt |
