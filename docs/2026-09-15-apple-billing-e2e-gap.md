# Apple billing: implementation evidence and hosted acceptance gap

Task `8140ed16`, September 15, 2026. Baseline
`5c1ca1b3b904d364bb231d591daed63911200a68` plus this task's implementation diff.
This is code/test evidence, **not Apple launch readiness**.

## Automated evidence

- Functions: final canonical emulator run passed 231 tests across 18 files,
  including 4 current-status/history gateway cases, 15 verifier cases (including
  retryable certificate-status failure), and 8 Apple emulator cases covering
  outstanding Stripe payment and concurrent checkout admission. The final run
  includes expired-versus-retrying Apple admission assertions. No live provider
  calls or credentials were used.
- The actual Apple verifier processes test-only ES256 certificate chains and
  nested JWS signatures. Deployed verification uses packaged Apple roots and
  online certificate checks. Test roots/local StoreKit signatures are rejected
  by the production factory. Functions build and compiled-root packaging passed.
- Relay: 20 entitlement integration tests passed, including the new Apple journey
  through exported callable/notification HTTP handlers, Auth/Firestore emulators,
  the real source reducer and already-connected desktop/phone relay sockets.
  Provider transport/trust fixtures cover purchase, renewal, renewal-off,
  grace deadline denial (4402), recovery, revoke/reversal, duplicate delivery
  and deletion replay. Existing free push/LAN and Stripe cases remain green.
- Mobile: 100 focused tests passed across account/app components, controller,
  source-aware billing card, environment/config and E2E runner. They cover
  localized metadata pricing, no production web CTA, comp and both providers,
  cancellation/pending, finish-after-acceptance, retry/restore and account changes.
  A subsequent controller case also passed for Apple management before deletion
  when Kanna email verification is unavailable.
- Portal: 45 tests passed (2 emulator suites skipped in that package run),
  including provider management, duplicate/comp presentation and account-switch
  listener cleanup. Functions, mobile and portal TypeScript checks passed.

## Native boundary

Pinned `expo-iap` 5.6.2 compiled successfully with this checkout's Expo 57/RN
0.86.2 through `./kd mobile run`. Candidate: dev bundle `build.kanna.app.dev`,
version 1.0.4, build 1, runtime 2.2.8; Xcode 26.6; dedicated iPhone 16 simulator,
iOS 18.4, UDID `EC4A74ED-DB85-4986-B063-D39E8D89010A`.

The opt-in [StoreKit lane](../apps/mobile/e2e/storekit/README.md) passed on this
binary: native pending/Ask-to-Buy approval, transaction delivery, finish only
after the fake durable acknowledgment, explicit restore, native connection
restart and full app-process restart restore. No cloud entitlement was granted.
The E2E runner exited 0; local logs are in `.tmp/storekit-e2e-final.log`.

The separate uninstall/reinstall diagnostic failed: the local simulator
returned no purchase after reinstall, even with `clearTransactions` skipped
on relaunch. This is not a passing reinstall test. It remains reproducible with
`KANNA_STOREKIT_CHECK_REINSTALL=1`; real sandbox reinstall evidence is required.
Purchase-sheet cancellation also remains a device acceptance case; its
controller path has unit coverage. Local success cannot prove Apple certificate
trust, hosted delivery or WAN capability.

Harness fixes during execution: pin the selected simulator UDID instead of
letting Appium create a similarly named device; include Debug-only developer
framework runpaths for StoreKitTest; make the test result visible and read its
accessibility marker directly. The initial failed launches touched only this
task's disposable simulator, never an installed operator desktop app.

## Why hosted acceptance is still pending

No live ASC catalog/agreement/key setup, secrets inspection, purchase/refund,
deployment or TestFlight submission was authorized or performed. A local
StoreKit signature is not a real App Store signature; local HTTP handler tests
are not a deployed Functions worker. Accepted older staging delivery is preserved
and does not constitute IAP acceptance. Native dependency/runtime changes require
new matching binaries; never OTA this JS to the older runtimes.

The release operator must complete the [Apple operations packet](ops/apple-subscriptions.md):
confirm identifiers/agreements, six storefront price points, grace settings,
server key and notification URLs for production **and sandbox on kanna-build**,
legal URLs and reviewer accounts. These inputs are operational prerequisites,
not an unresolved native-versus-web business decision.

## Required acceptance before launch

Run [production mobile QA](testing/mobile-production-qa-gate.md) at the delivery
checkpoint on the exact production-identity TestFlight binary, sandbox Apple
account, non-comp verified Kanna account and isolated disposable desktop with
relay enforcement. Record source SHA, binary bundle/version/build/runtime,
Functions revision/project, notification environment and sanitized IDs,
device/desktop/relay identity.

Prove real purchase sheet → Apple-signed registration and V2 notification →
same-uid cloud access → WAN controls; cancellation/pending; restart/reinstall
restore; no repurchase for comp or Stripe; renewal and renewal-off paid time;
accelerated expiry/grace/recovery; refund/revocation; deletion while Apple billing
continues. Verify Apple-only, Stripe-only, both-provider and comp-plus-paid
management, including account switching. Keep any uninduced Apple scenario named
as a gap; fixtures must not be substituted for it.

One ordinary review should focus on Apple trust/account binding, ordering and
deletion races, existing Stripe/entitlement compatibility, and this native
evidence. No unrelated system audit is needed.
