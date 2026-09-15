# Stage terminal geometry

## Source audit

Task `121f9e32` started at current `origin/main` `fa13c1956`.
The installed staging tag `v0.3.0-staging.20` points to `3ce0847d0`.
Both contain `789629609` (intentional viewer gestures), `e690ea40a`
(resize ordering/stability), and the post-attach foreground activation in
`terminalSessionLifecycle.ts`. This is a separate stage-replacement defect.

The supplied installed-daemon evidence says the replacement `dd007aef` PTY
spawned at 80 columns by 24 rows at 22:57:03.664769 on September 14 (UTC−06).
At 22:58:00.884764 its first `activate_viewer` applied 279 by 78. The prior
stage had also applied 279 by 78. These logs establish a default-size spawn
and a later active-view claim; they do not identify the frontend gesture that
caused that claim. Only the named log was inspected; the owner task and
installed application were not manipulated.

## Data flow and correction

Stage preparation creates a PTY spawn with an 80 by 24 fallback.
`spawn_prepared_stage_run_for_api` snapshots the outgoing terminal, kills it,
seeds primary-screen history, and spawns the same session id in the next
workspace. Previously it ignored the snapshot's dimensions for that spawn.

The task remains the same UI slot and `TerminalView` key. `session_created`
invalidates the persistent host's attachment; the lifecycle fits the pane,
registers its proposal, attaches to the replacement, and checks foreground
eligibility. KSP registration and resize are passive. Only an eligible
active-view edge claims geometry. Thus a rebind without a foreground edge
could leave the replacement at 80 by 24 indefinitely, until a later real
viewing gesture.

The server now uses the outgoing snapshot's positive dimensions for the
replacement's initial PTY grid. Geometry is retained even for a blank terminal
or one with only alternate-screen content. History flattening is separate.
A missing snapshot keeps the existing fallback. No viewer identity, activity
sequence, or ownership is inherited; current last-active-viewer arbitration
is unchanged.

There was also a tuple-order error in `RegisterViewer`: `rows_cols()` fed a
state constructor expecting `(cols, rows)`. This explains the logged
`previous=(24, 80)` despite an explicit 80 by 24 spawn. The corrected conversion
and first-claim log assertion keep diagnostic tuples consistent.

## Verification

See the focused regression in
`apps/desktop/tests/e2e/real/terminal-stage-geometry.test.ts`. It uses the
canonical isolated native runner and a disposable repository whose local
Codex executable reports kernel `stty size` on startup and SIGWINCH instead
of invoking a model. Provider discovery is isolated and selects Codex.
Native identity is checked before window operations. The journey covers a new
foreground task without terminal input, a hidden-pane stage replacement,
foreground return, and a continuously foregrounded stage replacement.

### Results (2026-09-14, task worktree `task-121f9e32`)

Before the fix, the lifecycle socket regression sent `(80, 24)` despite an
outgoing `(279, 78)` snapshot. The real native stage transition independently
reported `stty size` as `24 80` at replacement startup for a measured 114 by 40
pane. Its later daemon snapshot was already 114 by 40, demonstrating why a
final-size-only assertion misses the defect. The daemon first-registration
regression separately failed on `previous=(24, 80)`.

After the fix, the native journey passed in 5.07 seconds on fixture task
`daabdc0f`. The checked title was
`Kanna — task 121f9e32 (0.0.68 @ fa13c1956)`; the worktree and build stamp also
matched (the build carries the base commit stamp plus this worktree's fix).
The runner's WebDriver endpoint was `http://127.0.0.1:34754`.

| Observation | Result |
| --- | --- |
| New foreground task: measured pane and daemon grid | 114 columns × 40 rows |
| First stage replacement's initial kernel report | `40 114` |
| Foreground return after advancing from the diff tab | 114 columns × 40 rows |
| Continuously foregrounded second replacement | initial report `40 114`, no other reported size |
| Post-replacement attachment | existing active-view trace reached `sent` |
| Terminal-input frames across the whole journey | 0 |

The first advance starts with the pane hidden, but the app can reveal it as
it reconciles the stage change. This test does not claim it remained hidden;
its startup-size oracle is independent of any later activation. Existing
eligibility and stream-client tests retain coverage of passive/hidden viewers.
The single one-second observation after the second rebind checks for trailing
resizes; it is not a timing workaround in application code.

Checks:

- `pnpm --dir apps/desktop test:e2e real/terminal-stage-geometry.test.ts`: passed.
- `cargo test -p kanna-server --bin kanna-server stage_spawn_inherits_last_applied_geometry_without_requiring_history`: passed (history, blank, alternate-only, missing snapshot).
- `cargo test -p kanna-server --bin kanna-server stage_carryover_flattens_the_outgoing_terminal_into_the_replacement_seed`: passed.
- `cargo test -p kanna-daemon --test reconnect test_terminal_geometry_changes_are_logged_by_a_running_daemon`: passed.
- Desktop `useTerminal` and `terminalLayout`: 54 tests passed; unchanged frontend.
- Shared stream client: 80 tests passed; unchanged ownership/ordering behavior.
- E2E runner plan and tier classification: 14 tests passed.
- Rust formatting and `git diff --check`: passed.
- `cargo clippy -p kanna-server -p kanna-daemon --bins -- -D warnings`:
  blocked by five existing warnings in unchanged files: unused
  `auth_ok_frame_with_terminal_geometry` (`ksp.rs:773`), argument count
  (`db/pipeline_items.rs:271`), and three boolean expressions
  (`http_api/desktop_views.rs:518–526`). These are outside this task.

Local execution artifacts: `.tmp/stage-geometry/before.json`,
`.tmp/stage-geometry/evidence.json`, `.tmp/stage-geometry-before.log`,
`.tmp/stage-geometry-after3.log`, and the focused check logs under `.tmp/`.
The first native setup attempts exposed fixture-only issues: a nested-repo
safety refusal, an unavailable window-hide permission, non-returning setup
before PTY spawn, and the legacy `loadItems` startup helper racing selection.
The final fixture uses a normal snapshot refresh and a repository-local
executable. The orphaned setup reporter was identified by its exact fixture
cwd and terminated; the native runner cleaned up its other resources.

No release contains this new correction yet. Publishing is outside this task.

