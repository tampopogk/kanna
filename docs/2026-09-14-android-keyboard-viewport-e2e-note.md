# Android keyboard viewport verification — task c979ef7e

## Cause and ownership

PR #1485 (task 69234f81) remains the owner of keyboard-closed Android system
navigation clearance. `FloatingToolbar`, the tab scenes, and the task composer
continue to consume the bottom safe-area inset in that state.

The keyboard-open defect has a different owner. Expo's generated Android
activity is edge-to-edge and declares `android:windowSoftInputMode="adjustResize"`,
but the React root continues to cover the full display. React Native's Android
keyboard event reports an IME `height` with system bars removed, and on the
Samsung it did not represent the full obstruction created by the keyboard and
its accessory UI. Positioning the absolute composer from that height left its
lower controls behind the IME.

`TaskScreen` now measures Android's actual edge-to-edge obstruction from the
keyboard event's top edge (`viewportHeight - endCoordinates.screenY`) and uses
the larger of that value and the reported height. React Native 0.86 re-emits
`keyboardDidShow` when the IME height changes while remaining visible, so a
Samsung accessory-panel transition refreshes the same measurement without a
timer or blanket offset. Android's closed-keyboard safe-area offset and iOS's
existing keyboard-height positioning are unchanged. The measured composer top
still owns terminal reading clearance, while the resting composer obstruction
still owns the terminal capacity sent to the desktop; keyboard changes do not
reflow the PTY.

## Installed staging inspection

Read-only inspection of Samsung SM_A156W `R5CX42N3NLK` found:

- package `build.kanna.app.staging`, version `1.0.2`, version code `1`;
- native runtime `2.2.5`, OTA channel `staging`, and update URL
  `https://relay-staging.kanna.build/ota/manifest`;
- an available Android OTA manifest for runtime `2.2.5`, update id
  `30775f0e-131a-f7f9-b6e0-a208a6cff846`, created
  `2026-09-14T04:43:49.048Z`; and
- `adjustResize` in the installed activity manifest.

Current main uses staging runtime `2.2.6`. The non-debuggable staging package
does not expose which same-runtime update is currently selected, so the
installed app may still be running embedded or cached JS. That does not explain
this defect in current source: `TaskScreen` still positioned from the incomplete
reported IME height after #1485, and that path is covered directly by this
change. Staging was not launched, modified, reinstalled, or published.

## Physical and automated evidence

`./kd mobile doctor --android-device R5CX42N3NLK` passed, followed by
`./kd mobile run --android-device R5CX42N3NLK`. This installed and launched only
the task-owned `build.kanna.app.dev` identity with the worktree/emulator profile.
Window-manager inspection reported `adjust=resize` for the exact dev activity.
The isolated database had no owner data, so task `680449db` was created against
the current worktree solely as a disposable consultation fixture and opened in
the real `TaskScreen`. No text was entered or submitted and its delivered-input
count remained zero.

On Samsung SM_A156W `R5CX42N3NLK`, the task composer, Reply field, Send control,
and bottom task controls remained fully reachable in each focused state:

- keyboard closed, including restoration after dismissal;
- Samsung keyboard open with its emoji/translation/clipboard/settings toolbar;
- Samsung emoji accessory panel expanded beneath its additional category row.

The open-state window-manager IME frame began at physical y=1332; the composer
ended at y=1310, retaining the intended gap with no duplicated blank inset.
The terminal used the remaining visible area and restored when the IME closed.
Evidence is task-local at `.tmp/task-screen-keyboard-closed.png`,
`.tmp/task-screen-keyboard-open-occlusion-fixed.png`,
`.tmp/task-screen-keyboard-accessory-expanded.png`, and
`.tmp/task-screen-keyboard-closed-restored.png`, with matching UI hierarchy
dumps. The fixture was closed and the task-owned dev stack was stopped after
the check.

Verification:

- `pnpm --dir apps/mobile test -- --run src/screens/taskComposerKeyboard.test.ts src/screens/TaskScreen.test.tsx`
  — 115 passed.
- `pnpm --dir apps/mobile test -- --run src/components/FloatingToolbar.test.tsx src/navigation/RootNavigator.component.test.tsx src/navigation/RootNavigator.terminalInput.integration.test.tsx`
  — 16 passed.
- `pnpm --dir apps/mobile typecheck` — passed.
- `git diff --check` — passed.

Together the focused suites cover 131 passing tests (the earlier 128-test
baseline plus three regression cases for the complete Android occlusion).
