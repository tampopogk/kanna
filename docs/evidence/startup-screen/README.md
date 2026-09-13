# Startup screen — real-window evidence

Captured from this worktree's own dev build through the canonical E2E runner
(`pnpm --dir apps/desktop test:e2e mock/app-launch.test.ts`), in the isolated
WKWebView window whose native title was verified first:

```
Kanna — task 8d7d3090 · task-8d7d3090-2 (0.0.68 @ c8f708a4c)
```

No installed production or staging Kanna was launched, activated, or touched.

The screen shows the **mark alone**. The app icon's rounded tile is the shape
macOS puts behind the mark on a home screen; painted here it read as a white
box sitting on the app background, so nothing draws it — the SVG is cropped to
the mark's own bounds and the capsules sit directly on the window.

| File | What it shows |
|---|---|
| `flow-start.png` | The local-service wait, colour field at the start of its loop. |
| `flow-half-period.png` | The same window half a period later: the palette has travelled one and a half rows down and wrapped around. |
| `light-theme.png` | The same state under the light theme. |
| `failure.png` | A real startup failure: motion stopped, the icon's own artwork, restart guidance, no invented retry action. |
| `terminal-focus.png` | The workspace just after the screen lifted, with the restored terminal holding the caret. |

`StartupScreen.test.ts` holds the tile out: no white fill, no tile corner
radius, no stroke, and the viewBox stays the mark's bounds.

`app-launch.test.ts` asserts the rest mechanically: the 14 capsules are the
shipped icon's own geometry, every strip gradient repeats with its first colour
restated last, the segments are fully opaque, and the frame at one full period
is byte-identical to the frame at zero — which is what makes the wrap seamless.

## Terminal focus across the reveal

A terminal restored while the screen was still up asks for focus through an
`inert` ancestor, where a real browser refuses it — and nothing about that
terminal changes when the screen lifts, so without a second ask the revealed
terminal holds no caret until someone clicks it. The readiness edge therefore
re-runs the terminals' own focus owner once `inert` is gone.

`app-launch.test.ts` checks this in the real window: it holds the readiness
edge open after restoration has mounted an agent terminal, lets that terminal's
own focus attempt run and fail, then releases and waits for the caret. With the
re-ask removed the same check reports `{focused: false, inert: false}` after the
reveal — the reviewed race, reproduced — so this is a regression check rather
than a restatement of the fix.

happy-dom cannot stand in for this half: attribute assertions say nothing about
whether a browser honours `inert` for focus. The unit tests cover the ordering
(`App.test.ts`) and the focus owner's own rules (`useTerminalFocusWhenActive.test.ts`).

## Autonomous motion

`autonomous-motion.json` is from a second run of the same test with
`KANNA_E2E_NO_ACTIVATE=0`, which launches this instance with the ordinary macOS
activation policy. The test then calls the native `e2e_activate_current_app`
command — it activates the process that owns this webview and refuses unless
that env var is set, so it resolves no app name or bundle id and cannot reach
an installed Kanna — waits, and samples the animation's own `currentTime`
across 700 ms of wall time without touching it.

Observed: `document.hasFocus()` true and the clock advanced 700 ms — the field
moves by itself, not only when the test drives it.

Two honest caveats the recorded numbers show:

- macOS **declined** the app-level activation (`targetActiveAfter: false`,
  frontmost stayed another app). The window was visible and held document
  focus, which is what let WebKit run the animation, but this was not a
  genuinely frontmost app.
- Because of that, WebKit throttles, and the elapsed animation time varied
  across runs (417–700 ms per 700 ms waited). The test therefore asserts only
  that the clock advanced, never a rate — a rate assertion here would be flaky
  and would also overclaim.

Full-rate motion in a genuinely frontmost window is left to a human look; see
"Owner test-drive" below.

## Owner test-drive

Two ways to look at it live in this task's own window. Neither touches an
installed Kanna, and nothing is left running afterwards.

**1. The ordinary startup (real timings, frontmost, full-rate motion).** From
this worktree:

```sh
./kd dev up
```

The screen is what the window shows while it starts. Confirm the window you are
watching is titled

```
Kanna — task 8d7d3090 · task-8d7d3090-2 (0.0.68 @ c8f708a4c)
```

before judging anything — several Kanna instances run side by side on this
machine. `./kd dev down` when you are done.

**2. A held look, for as long as you want.** In that same window open the
webview inspector (⌥⌘I) and run:

```js
localStorage.setItem("kanna.e2e.startupHold", "1"); location.reload();
```

The window then sits on "Starting local services…" indefinitely. Release it
with:

```js
window.__KANNA_E2E_STARTUP_HOLD__.release();
```

or `window.__KANNA_E2E_STARTUP_HOLD__.fail()` to see the failure state. The
flag is DEV-only and one-shot — it is consumed as it is read, so a launch you
never release cannot wedge the next one.

What is worth your eye rather than an assertion: the speed of the flow, whether
the wrap reads as seamless, and whether the status text sits comfortably beside
it.

## What this evidence does not cover

- **Reduced motion.** `prefers-reduced-motion` cannot be forced in the WebDriver
  window; the static-artwork fallback is covered by
  `src/components/__tests__/StartupScreen.test.ts`.
- **The restore phase.** The DEV-only hold sits before the app mounts, so the
  captures show the pre-mount local-service wait. That the restoring workspace
  is laid out but inert and unreachable is covered by `src/App.test.ts`.
