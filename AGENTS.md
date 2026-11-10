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
- A fix of something already on main names the commit that brought the bug in a `Fixes: COMMIT` trailer, when you know it: the Stats tab traces fixes by it ([code quality metrics](docs/specs/code-quality-metrics.md#5-fixes)).
- Unrelated work gets its own task (`kanna task new`), even when it comes up mid-task.

## Running inside Kanna

- You may be running inside one of the user's own Kannas: production, staging or the main checkout's dev build, each with its own folder in `~/Library/Application Support/` (`kanna-prod`, `kanna-staging`, `kanna-dev`; see `docs/specs/build-channels.md`). Your terminal's `KANNA_SOCKET` says which.
- Never kill or restart `kanna-keeper`: it holds every terminal, the user's sessions included.
- Never `pkill` or `killall` anything Kanna or kmux. Stop only processes you started, by PID.
- Plain `kanna` (no `KANNA_SOCKET`) talks to the user's instance. Run tests against a private instance and socket, as `scripts/e2e-kanna.sh` does, never the user's.
- The Desk, Shelf and focus are shared with the user, who works in them at the same time. Never assume a change there was yours, and never undo one you didn't make; ask.

## Trying and shipping a change

- The user works in Kanna Staging, on the MacBook Pro and on the Studio (its served daemon). A change reaches them by a staging release, not by rebuilding the dev app. `scripts/release.sh` runs on the MacBook Pro, the machine with the release credentials (`~/.kanna/.env.release.local`), and Sparkle updates both machines' Kanna Staging from it.
- A staging release is a release candidate ([release candidates](docs/specs/release-candidates.md)): it soaks for 3 days, then `scripts/release.sh --promote` releases the same commit to production. While a version's candidates soak, `--staging` builds its `release/X.Y.Z` branch, so main's other changes wait for the next version.
- The planner offers a candidate (`scripts/release.sh --staging`) when the user wants one: when no version is open and main has moved since the last release, or when the release branch has fixes the last candidate lacks (`scripts/release.sh --status` says which). When the user names a fix on main for the soak, it runs `scripts/release.sh --pick COMMIT`. After a promote it makes the task that merges `origin/release/X.Y.Z` back into main.
- The planner never promotes a candidate (`--promote`) or fails one (`--fail`) without being told to, and never passes `--soak`, `--no-soak` or `--yes` unless the user said so.
- `scripts/build.sh dev` in the main checkout is for trying a change on main before it ships, in Kanna Dev; a task tries its own change in its worktree's dev build.
- Moving a Kanna's work between folders on one Mac: `scripts/move-instance.sh` ([instance move](docs/specs/instance-move.md)). Never on the user's own Kanna unless they ask; show them `--dry-run` first.

## Tests

- Start GUI instances in the background (`--bg` or `KANNA_BG=1`; kmux's `--bg` or `KMUX_BG=1`) so windows don't jump in front of the user. The product's default stays foreground.
- A new e2e scenario goes in its area's file under `scripts/e2e/`, with no list to edit ([e2e scenarios](docs/specs/e2e-scenarios.md)).
- Many agents run at once in parallel worktrees and may share the system `TMPDIR`. Write build and test logs to a path unique to you (in your worktree, or a `mktemp -d` dir), never a fixed name like `$TMPDIR/build.log`.

## Rules

- Prefer writing or updating a skill (`.agents/kanna-skills`) or this file to using an agent's memory.
