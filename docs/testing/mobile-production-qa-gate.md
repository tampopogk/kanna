# Mobile Production QA Gate

This gate is required before sending a Kanna mobile production build to
TestFlight external testing and before submitting that build to App Store
review. It does not upload to TestFlight or App Store Connect, and it does not
install, launch, or run Appium against a physical iPhone.

## Automated Gate

Run this from the repo root:

```bash
./kd mobile qa --production --key-path <absolute-private-key-path>
```

The command validates `apps/mobile/src/mobileEnvironments.json` production
identity, then runs:

```bash
pnpm --dir apps/mobile run typecheck
pnpm --dir apps/mobile run test
pnpm --dir apps/mobile run test:e2e:preflight
pnpm --dir apps/mobile run test:e2e:smoke
```

Production embeds the committed OTA signing certificate, so Expo must sign the
development manifest served to the simulator. Select the existing matching
local key with `--key-path`; this sets `KANNA_OTA_PRIVATE_KEY_PATH` only for the
QA subprocesses. The environment variable can be set directly for standalone
E2E runs. The path must be absolute and name a readable regular file. Local dev
E2E remains unsigned and does not require this selector.

The simulator checks use `KANNA_APP_ENV=prod` and default
`KANNA_E2E_DESKTOP_SERVER_URL` to the installed production desktop server at
`http://127.0.0.1:48120`. Start the installed desktop app first. If you need to
target another desktop server, set `KANNA_E2E_DESKTOP_SERVER_URL` before
running the gate. The simulator checks also require the existing Appium
XCUITest setup and a simulator app installed with the production bundle id
`build.kanna.app`; the preflight step fails early when those are missing.

The simulator smoke pairs the app with the exact selected desktop server
before any task-list deadline starts: the runner reads the server's identity,
creates a real pairing session on it (a loopback-only server route, so the
gate runs on the desktop's own machine, and a LAN address of this Mac in
`KANNA_E2E_DESKTOP_SERVER_URL` is rewritten to loopback for that one call),
seeds the app with that exact endpoint, and claims the payload through the
app's own pairing code path, which persists the device secret. The runner then
waits a bounded 15 seconds for the app's sanitized connection marker to show a
device id, that desktop's device secret, and that endpoint before the unchanged
30-second task-row assertion begins. On a failed assertion the error carries
the same sanitized picture — route origins, connection and task-collection
state, and credential-presence booleans, never a credential, email, or content
— so a zero-row result can be classified rather than attributed to latency.
The runner hands the app the exact route as
`EXPO_PUBLIC_KANNA_SERVER_URL`, the app's existing explicit development
endpoint, and treats it as part of the exact Metro environment: a Metro that
was started without that route is not reused, because the claim would then
target whatever Bonjour resolved. The production gate always starts its own
signed Metro. For a dev simulator run that should reuse a kd-managed Metro,
export the same `EXPO_PUBLIC_KANNA_SERVER_URL` before `./kd dev up --mobile`,
or run `./kd dev down` and let the smoke start Metro.

**What a simulator run proves.** Every simulator lane here loads the branch's
JavaScript through the Expo development client and Metro. It validates that
JavaScript against the installed native shell; it does not validate the
JavaScript embedded in a separately archived IPA. The runner's own files
(`apps/mobile/e2e/**`, `tools/kd/**`) are harness-only, but the pairing and
billing lanes also read app-side instrumentation that lives in app source:
the E2E-gated connection marker (`src/e2eConnectionDiagnostics.ts`,
`src/App.tsx`, rendered only with `EXPO_PUBLIC_KANNA_ENABLE_E2E_TRUST_SEED=1`)
and inert `testID` targets on the billing card (`src/components/AppleBillingCard.tsx`,
`src/e2eTestIds.ts`). A candidate IPA archived before those files merged does
not contain them, so a passing simulator run on this branch is evidence about
the branch, not about that IPA. The release task decides whether that
observation-only overlay is acceptable for the candidate or a rebuilt
candidate is required; this document does not make that call.

For releases that touch OTA, relay, Firebase production config, update signing,
or `runtimeVersion`, run the OTA-inclusive gate:

```bash
./kd mobile qa --production --ota --key-path <absolute-private-key-path>
```

That adds the existing read-only production OTA checks:

```bash
./kd mobile ota status --production
./kd mobile ota doctor --production
```

These require Google Cloud credentials for `kanna-build`. They read production
cloud and relay state but do not publish, roll back, or mutate devices.

## Apple Billing Review Screenshot

The subscription review screenshot comes from a separate, focused lane that
does not depend on the task-list smoke and waives none of the gate above:

```bash
./kd mobile billing-review --production \
  --screenshot-path <task-worktree>/.tmp/app-review/apple-billing.png \
  --key-path <absolute-private-key-path>
```

It runs `pnpm --dir apps/mobile run test:e2e:billing-review` under
`KANNA_APP_ENV=prod` against the production bundle `build.kanna.app` on the
simulator, signs in with the App Review account, opens the account sheet
directly, reads the real `AppleBillingCard` — the localized StoreKit monthly
price, Restore Purchases, and the EULA and privacy links — writes exactly one
screenshot at the caller-controlled path, and prints a report. It never taps
Subscribe or Restore Purchases, and it never substitutes the dev-only StoreKit
configuration lane, which replaces the app UI with `StoreKitTestApp` and is not
a review screenshot.

The reviewer account comes only from the existing protected selectors: an
explicitly exported `KANNA_E2E_CLOUD_EMAIL` / `KANNA_E2E_CLOUD_PASSWORD` pair
(the mobile cloud E2E's own selectors), or otherwise the review demo account
App Store Connect records for the version in `apps/mobile/VERSION`, read with
the same `APP_STORE_CONNECT_API_KEY_ID` / `APP_STORE_CONNECT_API_ISSUER_ID`
key `kd mobile publish` uses. The value reaches only the E2E subprocess
environment and is never printed.

The command succeeds only when the report is `ready`: signed in, email
verified, card rendered, billing confirmed by the production read, a localized
price or an already-covered source, Restore Purchases enabled, and both legal
links present. Anything else is named as a blocker (for example
`billing-read-unconfirmed` when the deployed Firestore rules deny the billing
read, or `storefront-price-unavailable` when StoreKit returns no product) and
the screenshot is still written as evidence. A ready capture does not replace
the full smoke, TestFlight purchase/restore acceptance, or the human
physical-device check.

## Before TestFlight External Testing

1. Build the production iOS candidate through the normal Expo/Xcode production
   release path.
2. Run `./kd mobile qa --production --key-path <absolute-private-key-path>`.
3. If the release changes OTA, relay, Firebase production config, update
   signing, or `runtimeVersion`, run `./kd mobile qa --production --ota --key-path <absolute-private-key-path>`.
4. Human-only physical-device check on the TestFlight build:
   - Install the candidate from TestFlight on an iPhone.
   - Confirm Settings -> Privacy & Security -> Local Network -> Kanna is on.
   - Open Kanna, sign in to a production account, and confirm the profile state.
   - Confirm the app reaches the production relay or installed production
     desktop server.
   - Open the task list, open a task, verify terminal output streams, send input,
     and navigate back to the task list.
   - If OTA is expected, verify the app applies the production OTA and reports
     the expected runtime/channel in the app update UI.

External TestFlight can proceed only after the automated gate passes and the
human physical-device check has no release-blocking issues.

## Before App Store Submission

1. Confirm the App Store candidate is the same build that passed external
   TestFlight, or rerun the full gate for the rebuilt candidate.
2. Run `./kd mobile qa --production --ota --key-path <absolute-private-key-path>` when any OTA, relay, production
   Firebase, signing, or runtime compatibility state is part of the release.
   Otherwise rerun `./kd mobile qa --production --key-path <absolute-private-key-path>`.
3. Repeat the human-only physical-device check on the exact TestFlight build
   selected for App Store submission.
4. Confirm App Store metadata, privacy declarations, screenshots, and release
   notes match the submitted build.
5. Do not submit if production OTA doctor fails, the physical-device check fails,
   or the candidate build differs from the checked build.

## Manual-Only Boundary

Agents may run the repo-side gate and read-only OTA checks. Agents must not
upload builds, submit to App Store Connect, install or launch attached physical
devices, or run physical-device Appium unless a human explicitly asks for that
specific action.
