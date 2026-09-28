# Hosted frontend contract

Run `python3 tests/hosted-frontend/run.py http://127.0.0.1:<reserved-port>`
against an isolated development instance started with `./kd dev up --db ...
--daemon-dir ... --transfer-root ...`. Build sidecars first. The runner refuses
non-development servers and creates its own local fixture repository, origin,
provider executable and tasks under the current worktree's `.tmp/` directory.
It closes its tasks even on failure. Stop the development instance afterward
with `./kd dev down --kill-daemon` and the same isolation arguments.

The provider uses file gates to make startup, turn completion, approval and the
unacknowledged-write crash reproducible. Assertions cross the actual HTTP
server, daemon, PTY, bundled frontend and provider process. Evidence is retained
under the printed `.tmp/hosted-contract-*/` directory. This supplements the real
Claude/Codex acceptance recorded in `docs/agent-tui/hosting.md`.
