# Isolated StoreKit native boundary

This lane uses the real `expo-iap` adapter and purchase controller with Apple's
local StoreKit configuration. Its fake durable acknowledgment does **not**
contact Functions or grant cloud access. Local JWS signatures are never accepted
by deployed Functions.

Use a dedicated disposable simulator. Do not point this lane at an existing
personal test device: it clears its local StoreKit transactions and reinstalls
the dev app. Build through the canonical runner:

```sh
EXPO_PUBLIC_KANNA_STOREKIT_TEST=1 ./kd mobile run --simulator SIMULATOR_UDID
./kd dev down --kill-daemon
KANNA_IOS_SIMULATOR_NAME=DEDICATED_SIMULATOR_NAME \
  EXPO_PUBLIC_KANNA_STOREKIT_TEST=1 pnpm --dir apps/mobile test:e2e storekit
```

The stop between build and test gives the E2E runner ownership of Metro with
its exact environment. Assigned task ports are inherited. The runner pins the
selected simulator UDID and uses only `build.kanna.app.dev`.

The opt-in Expo config plugin adds a Debug/simulator-only StoreKitTest bridge,
configuration resource and developer framework paths. It is refused for staging
or production. Normal release archives use canonical clean prebuild without this
flag. Do not reuse this instrumented generated iOS project for a release.

The lane checks native pending approval, transaction delivery, deferred finish
until acknowledgment, explicit restore, connection restart and process restart
restore. It neither buys a real subscription nor contacts App Store Connect.

`KANNA_STOREKIT_CHECK_REINSTALL=1` replaces the process restart check with an
explicit reinstall diagnostic. It uses a temporary copy of the exact installed
binary under `.tmp` and removes that copy afterward. This diagnostic failed on
the tested iOS 18.4 simulator: no simulated purchase survived uninstall, despite
skipping `clearTransactions` on relaunch. It remains a named sandbox acceptance
gap, not a passing local test. Cancellation is separately covered by the
controller tests; actual purchase-sheet cancellation also remains in
sandbox/device acceptance.

See [the evidence and remaining acceptance](../../../../docs/2026-09-15-apple-billing-e2e-gap.md)
and [the operational handoff](../../../../docs/ops/apple-subscriptions.md).
