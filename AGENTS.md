# Working in kanna-v4

Rules for every agent working here, the planning agent and task agents alike.

## Code

- No code comments.
- Provenance of code copied from kmux (MIT) goes in `NOTICE`, not in file headers.

## Design

- Take the technically better architecture over the expedient one. Offer a stopgap only as a step towards it, and say what the better design costs.

## Commits and branches

- No AI attribution in commits: no `Co-Authored-By` trailer for an AI.
- No PRs: a branch merges straight into main.
- Unrelated work gets its own task (`kanna task new`), even when it comes up mid-task.

## Running inside Kanna

- You may be running inside one of the user's own Kannas: production, staging or the main checkout's dev build, each with its own folder in `~/Library/Application Support/` (`kanna-prod`, `kanna-staging`, `kanna-dev`; see `docs/specs/build-channels.md`). Your terminal's `KANNA_SOCKET` says which.
- Never kill or restart `kanna-keeper`: it holds every terminal, the user's sessions included.
- Never `pkill` or `killall` anything Kanna or kmux. Stop only processes you started, by PID.
- Plain `kanna` (no `KANNA_SOCKET`) talks to the user's instance. Run tests against a private instance and socket, as `scripts/e2e-kanna.sh` does, never the user's.
- The Desk, Shelf and focus are shared with the user, who works in them at the same time. Never assume a change there was yours, and never undo one you didn't make; ask.

## Tests

- Start GUI instances in the background (`--bg` or `KANNA_BG=1`; kmux's `--bg` or `KMUX_BG=1`) so windows don't jump in front of the user. The product's default stays foreground.
- Many agents run at once in parallel worktrees and may share the system `TMPDIR`. Write build and test logs to a path unique to you (in your worktree, or a `mktemp -d` dir), never a fixed name like `$TMPDIR/build.log`.

## Rules

- Prefer writing or updating a skill (`.agents/kanna-skills`) or this file to using an agent's memory.
