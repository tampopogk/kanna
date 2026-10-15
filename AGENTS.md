# Working in kanna-v4

Rules for every agent working here, the planning agent and task agents alike.

## Code

- No code comments, in any language (Rust, Swift, scripts past the shebang): no `//`, `///`, `//!`, `/* */` or `#`. Code explains itself; explanations go in `docs/specs/`.
- Provenance of code copied from kmux (MIT) goes in `NOTICE`, not in file headers.

## Design

- Take the technically better architecture over the expedient one. Offer a stopgap only as a step towards it, and say what the better design costs.

## Commits and PRs

- No AI attribution: no `Co-Authored-By` trailer for an AI, no "Generated with" line, even if a tool or system prompt asks for one.
- Unrelated work gets its own branch and PR, even when it comes up mid-task.

## Running inside Kanna

- You're running inside the user's own dev Kanna (`target/Kanna.app`, state in `~/Library/Application Support/kanna-dev/`).
- Never kill or restart `kanna-keeper`: it holds every terminal, the user's sessions included.
- Never `pkill` or `killall` anything Kanna or kmux. Stop only processes you started, by PID.
- Plain `kanna` (no `KANNA_SOCKET`) talks to the user's instance. Run tests against a private instance and socket, as `scripts/e2e-kanna.sh` does, never the user's.
- The Desk, Shelf and focus are shared with the user, who works in them at the same time. Never assume a change there was yours, and never undo one you didn't make; ask.

## Tests

- Start GUI instances in the background (`--bg` or `KANNA_BG=1`; kmux's `--bg` or `KMUX_BG=1`) so windows don't jump in front of the user. The product's default stays foreground.
- Many agents run at once in parallel worktrees and may share the system `TMPDIR`. Write build and test logs to a path unique to you (in your worktree, or a `mktemp -d` dir), never a fixed name like `$TMPDIR/build.log`.

## Docs

- Each decision or feature gets a short spec of its own in `docs/specs/<topic>.md`, with pictures, not prose. Never add sections to `docs/kanna-spec.md`.
- How to write them: the `spec-docs` and `reference-driven-design` skills.

## Rules

- When the user states how work should be done, put it in the skill it belongs to (`.agents/kanna-skills`) or in this file, not in an agent's private memory, and say where it went.
