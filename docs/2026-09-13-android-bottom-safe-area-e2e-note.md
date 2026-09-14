# Android bottom safe-area verification — task 69234f81

## Scope

Use the existing React Navigation safe-area provider, without migrating the
React Native shell or modal SafeAreaViews. FloatingToolbar consumes its existing
navigator insets; MainTabsRoute uses useSafeAreaInsets to reserve the same Android
bottom inset in each tab scene. This preserves the lists' existing 140-point
scroll clearance. TaskScreen lifts its separate composer above the Android
navigation inset, retaining its 14-point resting gap and using the keyboard
position when that is higher. Its existing measured composer position continues
to determine terminal reading clearance.

The iOS branches retain the original toolbar offset, scene style, and composer
position. Focused component checks cover those branches with a nonzero iOS
inset, as well as Android zero/nonzero insets and keyboard positioning. The shell,
native configuration, dependencies, and runtimeVersion are unchanged in this diff;
no iOS simulator run was needed for these Android-only layout changes.

## Physical evidence

On the authorized Samsung SM_A156W, serial R5CX42N3NLK, Android 16 / API 36,
1080×2340, 450 dpi, three-button navigation, package build.kanna.app.staging:

- Tasks: all tab labels and both utility buttons visibly clear the system bar.
- At the list's lower scroll limit, the final card (b4f14d88) is completely above
  the floating toolbar. A further upward swipe leaves it at the same position.
- Task detail: the separate Reply/Send composer and its utility buttons clear
  the system bar. Live terminal output rendered; no agent input was sent.
- The existing paired staging desktop and its tasks restored without rescanning
  or resetting pairing. The pre-existing top/status-bar overlap in task detail
  remains outside this bottom-only fix.

Screenshots in this task's worktree:
`.tmp/android/screens/tasks-list.png`, `tasks-last-card.png`, and `task-detail.png`.
The detail capture is the restored pairing task 0d72b7af. Navigation actions were
only its Back button and Tasks-list swipes. Every adb command named the serial.
Keyboard positioning was tested at component level, not by typing on the phone.

## Installation dependency and incident

The initial `./kd mobile run --android-device R5CX42N3NLK --staging --install`
completed before the owner's coordination message. That candidate lacked the
unmerged pairing-native implementation and used staging runtime 2.2.4; after
installation it showed a blank screen. No app data or pairing was cleared.
This first install is not valid safe-area acceptance evidence.

The corrected build temporarily included the exact four production files from
pairing commit e3a2e935b9bd16b8af5bfb886da12fe01a5a9b99: withKannaBonjour.js,
bonjour.ts, machinePairing.ts, and mobileEnvironments.json. Their bytes were
checked against that commit before rebuilding with the same kd command. Generated
native output was checked for KannaBonjourModule, MainApplication registration,
the scoped .local network policy, and staging runtime 2.2.5. The corrected
installation restored the paired session and produced the evidence above.

These install-only dependency changes were then reversed; they are not part of
this task's diff. Future device installs must include that pairing work or its
compatible successor. The generated build is retained under
`.tmp/android-native-build/`; candidate identity/source hashes and APK SHA-256
are under `.tmp/android/evidence/`. Build logs are `.tmp/android/build.log` and
`.tmp/android/build-with-pairing.log`. Nothing was published.

## Automated coverage

121 tests passed across FloatingToolbar.test.tsx, TaskScreen.test.tsx,
RootNavigator.component.test.tsx, and RootNavigator.terminalInput.integration.test.tsx.
`pnpm --dir apps/mobile typecheck` passed. These tests cover the layout contracts
and preserve existing navigation/terminal wiring, but mocked safe-area values
cannot prove Android native inset delivery. The physical screenshots supply that
check for this device. A repeatable automated Android regression would need a
serial-fenced device UI harness with controlled three-button/gesture navigation
and a seeded list; the existing mobile Appium suite targets iOS.
