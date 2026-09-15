# Apple subscription operations

Implementation track: task `8140ed16`, approved September 15, 2026. Native
Apple billing and web/Stripe billing grant the same Firebase account's
`cloud_access`. This document authorizes no live setup, purchase, refund,
agreement acceptance, deployment or store submission.

## Catalog and identity

- Production bundle: `build.kanna.app`; one product:
  `build.kanna.cloud.monthly`, one month, one Kanna Cloud subscription group.
- Monthly targets: ¥500 / US$5 / CA$5 / AU$5 / €5 / £5. Record actual
  storefront price-point IDs before activation; bring a real unavailable-price
  exception to Jeremy. No annual plan, trial, offers or Family Sharing.
- Owner/reviewer comp remains. Ten **additional** beta places exclude the
  existing two; this implementation issues no grants or enrollments.
- Numeric ASC app ID, group ID, key ID/issuer, agreement, banking/tax,
  provisioning and storefront availability remain unverified operational inputs.
  Small Business Program participation/commission is not assumed.

## Configure and deploy, when authorized

1. Confirm existing app/product/group entries before creating anything. Account
   Holder handles the Paid Applications Agreement; authorized humans complete
   tax/banking. Record six storefront prices and reviewer metadata/screenshot.
2. Provision a scoped In-App Purchase server key. It is separate from the ASC
   upload key. Install `APP_STORE_PRIVATE_KEY` through Secret Manager tooling;
   never paste it into chat, commands, source or logs. Set public Functions
   parameters `APP_STORE_APP_ID`, `APP_STORE_GROUP_ID`, `APP_STORE_KEY_ID`,
   `APP_STORE_ISSUER_ID`. Blank committed defaults disable admission gracefully.
3. Deploy through `./kd cloud deploy --functions` and the canonical rules/portal
   workflow. Registration alone binds the Apple key; admission binds the Stripe
   API key for existing-payment checks; notifications bind neither key. Missing
   Apple configuration does not prevent Stripe module initialization. An absent
   required Secret Manager entry prevents deployment of its new callable.
4. Set **both production and sandbox V2 notification URLs** to the
   `kanna-build` project's `appStoreNotifications` endpoint. TestFlight uses
   production Firebase and Apple's sandbox, **not** `kanna-staging`.
   Use Apple's `requestTestNotification` and status lookup before acceptance.
5. Record Billing Grace Period settings. Proposed: 16 days, paid-to-paid,
   sandbox first. Confirm the setting before enabling production; signed
   deadlines also work when grace is disabled.
6. Deliver a new native candidate through `kd` under separate release authority.
   Native pins: `expo-iap` 5.6.2 and Apple server library 3.1.0. Runtime bumps:
   dev 2.2.7 → 2.2.8, staging/prod 2.2.6 → 2.2.7. Existing accepted binaries
   cannot receive this JS by OTA. Do not reinterpret this as reapproval of
   earlier staging evidence or as permission to publish.
7. Confirm live Terms, Privacy/EULA and purchase-data privacy disclosures.
   Mobile links are `https://kanna.build/terms` and `/privacy`; legal approval
   and live URL acceptance remain release inputs. Initial private support:
   `support@tampopomyoko.com`, owned by Jeremy. Provide a reviewer comp account,
   separate non-comp purchase account and disposable reachable desktop.
   First subscription submission accompanies the app version.

## Sources, support and deletion

Read `users/{uid}/billing/stripe` **and** `/app_store`, even if the entitlement
source is `comp`. Entitlement `source` is the currently honored access source,
not the complete inventory. Apple source records retain original transaction
ID and verified `production`/`sandbox` environment; Stripe keeps its customer
and subscription IDs. The private `appStoreSubscriptions` ledger retains each
environment/original separately. Sandbox is excluded from revenue reporting.

Production iOS says “Managed outside the App Store” for web subscriptions and
offers no external purchase/portal link. The web account page identifies Stripe
explicitly and offers its hosted portal. Apple management always goes through
Apple; refunds are requested at <https://reportaproblem.apple.com>. Kanna's
24-hour **direct Stripe refund-request** window neither applies to Apple nor
promises Apple approval. No automatic duplicate cancellation or refunds.

Deletion is immediate and Auth-last/retryable. It cancels Kanna-owned Stripe
billing but **cannot cancel Apple billing**. Users may manage Apple first or
delete immediately after the warning. Deletion removes Apple token mappings,
subscription and notification records; the minimal account-deletion tombstone
remains. Deleted/unknown tokens cannot be rebound, including to a new account
registered with the same email. Support must not silently reassign purchases.

## Monitor and reconcile

Monitor `appStoreNotifications` 400/503 rates, unresolved binding diagnostics,
Apple delivery failures and reconciliation failures. Logs contain event IDs,
environment and outcomes, not JWS payloads, tokens or keys. A 503 is retryable
and writes no dedupe marker. Unknown/deleted account bindings are acknowledged
without grants. Registration retrieves current verified status; a device's old
signed purchase alone never grants access.

Use one bounded repair with operator credentials supplied securely in the
process environment. Build first, then:

```sh
pnpm --filter @kanna/firebase-functions build
node services/firebase-functions/scripts/reconcile-app-store.mjs \
  --project kanna-build --uid ACCOUNT_UID --environment production \
  --original-id ORIGINAL_TRANSACTION_ID --days 7
```

Default is dry run. `--apply` replays up to 30 days of paginated notification
history and then current status through the same transactional writer. Select
sandbox explicitly when appropriate. The script checks existing ownership;
it cannot relink. A concurrent-write fence fails the operation; rerun to
refetch. Earlier replay writes are durable/idempotent. Output identifies
provider/environment/statuses and only a subscription suffix.

No periodic poller was added. Active relay records intentionally survive
period-end notification latency, so missed notifications need this repair.
Rollback must preserve renewal/refund/deletion processing. Disable new purchase
admission in the `beginAppStorePurchase` callable if necessary (a scoped code
deployment, retaining registration/notification configuration); never blank the
shared verifier parameters to accomplish this. Never roll back to a writer unaware of existing Apple
subscriptions.

## Acceptance packet (pending live execution)

Use the exact production-identity TestFlight binary, real ASC product, sandbox
Apple account and same verified Kanna uid. Select an isolated disposable
desktop with relay enforcement on, never the operator's installed desktop.
Follow [the production mobile QA gate](../testing/mobile-production-qa-gate.md)
at external TestFlight/submission time.

Record source SHA, bundle/version/build/runtime, Functions project/revision,
device and relay/desktop identities, environment and sanitized notification IDs.
Prove purchase sheet → Apple-signed registration + real notification → WAN
controls; restart/reinstall restore; Stripe/comp no repurchase; renewal;
renewal-off retains paid time; accelerated expiry/grace/recovery; refund/revoke;
deletion while Apple billing persists. Record unsupported/uninduced cases as
gaps. Local fixtures and StoreKit configuration are not this acceptance.

Primary references checked September 15, 2026:

- [Apple server library](https://github.com/apple/app-store-server-library-node)
- [Maintained Expo adapter](https://github.com/hyodotdev/openiap/tree/main/libraries/expo-iap)
- [Apple sandbox testing](https://developer.apple.com/documentation/storekit/testing-in-app-purchases-with-sandbox)
- [Notification URLs](https://developer.apple.com/help/app-store-connect/configure-in-app-purchase-settings/enter-server-urls-for-app-store-server-notifications/)
- [Subscription pricing](https://developer.apple.com/help/app-store-connect/manage-subscriptions/manage-pricing-for-auto-renewable-subscriptions)
- [Apple account deletion](https://developer.apple.com/support/offering-account-deletion-in-your-app/)
