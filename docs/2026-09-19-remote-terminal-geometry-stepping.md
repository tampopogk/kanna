# Remote terminal geometry: the pane measured the grid it was rendering

Date: 2026-09-19. Task 60970060, reported by the owner against staging
`0.4.0-staging.2` on `desktop-aa43ab36-e634-4ae9-b629-e8c8a91f7bff` (Mac Studio,
port 48121) with remote task viewing newly working from Gu's MacBook Pro over the
staging relay at `40d25b76`.

Owner's words: *"did we add some kind of filter to the terminal size fight between
two machines? the terminal size just slowly reduced from Apple Studio Display size
to MBP screen size...."* and *"the stepping down to the right terminal size also
kind of feels like network ripple (delay in the network bounce back and forth)."*

## Verdict

A **measurement feedback loop inside the remote viewer**, not a daemon problem.

The daemon's controller election is correct and snaps, exactly as designed: when
the MacBook Pro's pane became the active controller the PTY moved to its grid in
one resize, and when the Studio's pane took ownership back it moved back in one
resize. What stepped was the MacBook Pro's *own measurement of itself*.

A remote pane hydrates at the owner's authoritative grid —
`applyTerminalSnapshot` deliberately restores a snapshot's recorded dimensions
before replaying its bytes, so a full-screen TUI is not reflowed. It then
re-measures with `FitAddon.proposeDimensions()` and, as controller, registers
that measurement with the daemon. `proposeDimensions()` reads the **computed
width of the terminal's parent element**, so the measurement is only about the
pane if the pane cannot widen to the grid. It could:

```css
/* CloudTerminalCache.vue */
.cloud-terminal-cache        { display: flex; flex: 1; min-height: 0; }
.cloud-terminal-cache-entry  { display: flex; flex: 1; min-height: 0; }
/* CloudTerminalView.vue */
.cloud-terminal-shell        { position: relative; flex: 1; min-height: 0; }
```

All three are flex items in a **row** container, and a row flex item's automatic
minimum size is its `min-content` width. `.xterm-screen` is an in-flow child
carrying an explicit `cols × cellWidth` inline width, so none of these boxes could
shrink below the rendered grid. `min-height: 0` protected the block axis and
nothing protected the inline axis — which is why **rows never moved and columns
crept**.

So each round was: daemon applies N → `publish_resize_snapshot` broadcasts a
snapshot at N → the viewer hydrates its xterm at N → the pane widens to N cells →
`proposeDimensions()` returns N − 2 → the controller registers N − 2 → daemon
applies N − 2. The loop ends when the grid finally fits the real pane, at which
point the pane stops being content-sized and the true fit wins.

The −2 is the FitAddon's scrollbar reservation: it subtracts
`options.scrollbar?.width ?? 14` px from the parent width, and at this font's
~7.5 px cell that is two columns per round.

**The owner's "network ripple" instinct was half right.** The relay is not the
cause — the loop is entirely inside one viewer and needs no second machine to
close. What the relay supplied was the *period*: one round trip per step, ~200 ms,
which is what turned an instantaneous convergence into a visible descent.
Of the hypotheses considered, this is #1's mechanism (a resize the viewer feeds
back upstream) with #3's locality (no network required). It is **not** election
thrash — the controller never changed during a descent — and **not** the
`min()` legacy fallback, which never ran.

## Evidence

All read-only. The owner's live sessions were not resized and no input was
injected into them.

1. **The daemon's own log, twice.** `kanna-daemon_99924_rCURRENT.log` in
   `~/Library/Application Support/build.kanna.staging/Kanna/`, session `75bcca47`.
   Two complete descents, identical in shape:

   | | run A | run B |
   |---|---|---|
   | start | 07:34:18.634 | 07:39:27.500 |
   | end | 07:34:30.112 | 07:39:37.365 |
   | duration | 11.5 s | 9.9 s |
   | applied resizes | 49 | 49 |
   | path | 240×78 → 238×46 → … → 143×46 | identical |

   Every step is `cause=register_viewer`, same `writer=31751001664`, same
   `viewer=Some("terminal-viewer-1")`, same `generation=Some(1)`,
   `controller=Some("terminal-viewer-1")->Some("terminal-viewer-1")`,
   `owner_changed=false`, `viewers=2 legacy_viewers=0`. Every column step is
   exactly **−2**; rows are pinned at 46 from the first step to the last. The
   final step is **−1** (144 → 143): the grid has finally fitted inside the pane,
   the pane stops tracking it, and the true fit takes over. Median step interval
   ≈ 200 ms.

2. **It settles, and the end state is correct.** 143×46 is the MacBook Pro pane's
   real fit, and it holds. At 07:34:33 and 07:39:40 the Studio's viewer
   (`writer=31751002384`) re-activates and the PTY returns to 240×78 in a single
   applied resize — the election snapping exactly as designed.

3. **The first proposal was already contaminated.** The activation that opened
   run A proposed `238×46` against `previous=(240, 78)`. 238 is not any
   measurement of a MacBook Pro pane; it is 240 − 2. Rows were right immediately.
   Width was grid-derived from the very first frame, height never was.

4. **Reproduced in the real app, with the same signature.** A new mock-lane E2E
   (`apps/desktop/tests/e2e/mock/remote-terminal-geometry.test.ts`) opens a
   remote pane in a real WKWebView, measures it, hydrates the xterm at an owner
   grid of `117 × 2 + 40 = 274` columns, and re-measures. Before the fix:

   - the pane's own width went **936 px → 2145 px** — it stretched to the grid;
   - the registered proposal went **117 → 272** — that is 274 − 2, the same
     two-column signature the daemon logged.

   After the fix both are unchanged by hydration.

5. **The local path was never affected, and says why.** The Studio's own viewer
   held 240×78 throughout. Its chain (`.main-panel` → `.work-area` →
   `.main-tab-panel` → `.agent-live-content` → `.terminal-panel`) is
   column-direction flex with `min-width: 0` and `overflow: hidden` already
   declared on the row-direction boxes. The cloud chain is the one place that
   convention was not followed.

## Fix

`min-width: 0` on the three row-flex boxes, and `overflow: hidden` on the shell so
an oversized grid is clipped rather than painted past the pane. This matches the
convention already used one component up in `MainPanel.vue`. The viewer's
measurement is now a property of its pane alone, so the loop cannot form: an
election costs exactly one resize.

No daemon change. `crates/daemon/src/client.rs` behaved correctly throughout, and
`test_terminal_geometry_changes_are_logged_by_a_running_daemon` in
`crates/daemon/tests/reconnect.rs` already pins that behaviour through the real
socket protocol — a wide local viewer and a narrow remote one handing geometry
back and forth, each handoff landing the PTY directly on the new controller's
grid. It passes before and after this change, which is consistent with the daemon
not being the defect; a new daemon test could not have caught a viewer that keeps
sending it new sizes.

## Regression coverage

`apps/desktop/tests/e2e/mock/remote-terminal-geometry.test.ts` asserts the
invariant that was violated: **a remote viewer's registered measurement, and the
pane it measures, must not change when the terminal is hydrated at an
authoritative grid larger than that pane.** It runs in the mock E2E lane because
the defect is a CSS layout property — jsdom and happy-dom do no layout, so no
unit lane could observe it. Verified failing before the fix (272 ≠ 117,
2145 px ≠ 936 px) and passing after.
