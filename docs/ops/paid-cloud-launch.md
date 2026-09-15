# Paid-cloud launch handoff

Local evidence, 2026-09-14, task `f467d701`, parent `ee3c91ab`.
**Production Stripe is NOT SET UP. This document is not Ship authorization.**
No deployed sandbox transaction, production configuration, release, store action,
device operation or public publication was performed. Android (`8bea27a8`), its
registration/document availability, and Discord are not dependencies here.

This is the integration/evidence sheet for the existing
[billing contract](../specs/accounts-and-billing.md),
[relay operations](../relay-vm-operations.md),
[Auth email runbook](account-email.md), and
[release procedure](../dev/release.md); use those procedures rather than a second
set of deployment commands. The parent's latest durable plan and all delivered
inputs were read. Owner authorization permits local technical work, leaving
prices, trials, refunds, beta eligibility and the iOS purchase model unchanged.

## Integrated source and evidence

The exact integrated baseline is main
`8d734ec40ed55ebd4eb37bbb2c84afc075558d20`. This task's starting integration
commit `05fdb9453` had the same tree; local merge `e38524686702d0aec5b668c074888ead16e7fae2`
adds the actual merged-main ancestry. This task adds tests and handoff documents,
with no production behavior change.

| Prerequisite | Integrated source | Durable evidence reused for unchanged code |
|---|---|---|
| Deployment policy `647dcf8d`, PR #1497 | `93c415fd90a1046f0953a6798689ae340464ac7e` merge; reviewed `c077c05dce0b17b39c10be28212041d473986301` | Independent 70 focused tests and typecheck; both environment policies remain off |
| Portal lifecycle `ce9475d4`, PR #1498 | `32da19e6d5a8b4f4bceb1ba39664d3ac008c409b` merge; final `c818c82c9a77466785eced3050b3a48769771773` | 67 reviewer-rerun tests; 73 emulator/Auth checks and package typechecks reused; final kd manifest correction covered by 70 kd checks |
| Checkout coordination `96a0a462`, PR #1499 | `3d560118cf7a0f33c4cab3b39ba7a38a16cb8e36` merge; final `1d5af26a09815aeedb0b9428bd7a841ac9767c9c` | 72 independently rerun emulator/core/gateway tests, typecheck reused; no Functions HTTP or hosted Stripe evidence in that result |
| Access recovery `88a3c8de`, PR #1500 | `8d734ec40ed55ebd4eb37bbb2c84afc075558d20` merge; final `1678511980693b89ebe2a832b64ef5666284c0a1` | Final review: 41 relay tests and relay typecheck; earlier 304 focused passes reused. Patch-equivalent rebase from reviewed `bb012e4bdf8a724dcaa84cfa672118c62614e74a` |

Counts overlap; do not sum them as independent coverage. Full sibling results
and inputs are available through `kanna_get_task` and `kanna_task_inputs`.

New verification: `pnpm --dir services/relay exec vitest run test/entitlement.integration.test.ts --maxWorkers=1`
passed **19/19**, including one added cross-service journey. Relay
`pnpm exec tsc --noEmit` and a strict no-emit check explicitly including that
test and its helper passed. Logs are task-local `.tmp/paid-cloud-relay.log` and
`.tmp/paid-cloud-test-types.log`; durable evidence is this table and the task result.

| Boundary | What the evidence actually proves |
|---|---|
| Fresh identity → verification | Disposable Auth emulator signup, rejected unauthenticated/unverified checkout, OOB verification code applied, refreshed verified token. No delivered email proof |
| Verified identity → checkout | Actual exported `onCall` HTTP handler authenticates the token and runs real checkout/Firestore coordination; injected Stripe gateway returns a fixture session. Repeat checkout reuses it; no entitlement is granted by a returned URL |
| Webhook → entitlement → relay | Actual exported `onRequest` HTTP handler receives original bytes and signed hand-built fixtures. Invalid signature is rejected. Same desktop and phone protocol sockets observe activation and can publish/invoke; renewal, payment failure, timed grace expiry, recovery, cancel-at-period-end and subscription deletion change access correctly |
| Ordinary management vs deletion | Real portal callable resolves the authenticated customer's billing session through injected I/O. Cancellation retains the account until/after expiry; deletion cancels via injected gateway, deletes Auth/account/credentials and leaves the tombstone. Old signed events cannot resurrect it. Unknown-customer events return acknowledged `unresolved_account` |
| Free access and older peers | Existing cases in the same 19-test suite cover account push without entitlement, comp script → reducer → relay, enforcement off, old peers without unsolicited frames, and tunnel revocation. No APNs/FCM delivery claim |
| Actual application consumers / LAN | Reuse access sibling's server publication/settings, desktop account projection, mobile transport/account-switch/foreground recovery and LAN-preservation checks. New sockets are protocol fixtures, not a running native desktop or phone; desktop credential registration is a fixture |

The new HTTP host is Express with the **real exported Functions SDK handlers**,
Auth and Firestore emulators, and mocked Stripe gateway factories. It is neither
the Firebase CLI Functions worker nor deployed Functions. It adds no runtime
test toggle to production code. The existing portal `test:integration` instead
uses a **checkout callable stub**: that result proves portal/Auth wiring only.
Existing billing-emulator tests exercise backend cores and injected gateways.
None of these are hosted Stripe, real 3DS, mail delivery or deployed-sandbox proof.
The existing relay suite compiles Functions for its comp fixture; no broad build
or native admission was run.

## Compatible independently shipped surfaces

All rows below name **source contracts at the integrated baseline**, not a claim
that any installed or deployed artifact contains them. Root/Ship must record a
final candidate containing these patches and the test commit, then capture each
artifact's own identity; a desktop release does not deploy the other services.

| Surface | Required contract / later evidence to pin |
|---|---|
| Functions (`services/firebase-functions/src/index.ts`) | Node 24, `us-central1`; exports `createCheckoutSession`, `createPortalSession`, `deleteAccount`, `stripeWebhook`. Checkout accepts `{plan:"monthly"}`; portal accepts `{}` and trusts only server customer mappings. Callable errors retain `details.reason`. Record deployed revision/SHA, region, public invoker and secret bindings separately |
| Billing + rules | `users/{uid}/billing/{stripe,comp,app_store}` → reducer → `users/{uid}/entitlements/cloud_access`; `accountCheckouts`, `stripeCustomers`, `stripeEvents`, `accountDeletions` ownership/fences must remain compatible. Pin `firestore.rules` and indexes with Functions; explicit Functions/portal-only deploy targets do not include rules |
| Portal (`apps/web-portal`) | Same Firebase project/uid as desktop and phone; observed entitlement, grace deadline and pending/read-failure states; reset/resend/refresh verification; `/account`, `/subscribe`, `/billing/success`, `/billing/canceled`. Return URL alone never proves payment. Record hosting target, source/build and return origin |
| Relay (`services/relay`) | `auth.access_updates:true` opts into repeated `auth_ok`; `capabilities.accessUpdates.version=1`, `tunnelServices:[ksp,task-transfer]`, snapshot and desktop-routing version 2, free mobile-notification version 2. `entitlement` carries active/status/period/grace/reason; 4402 denies paid work/tunnels. Older peers do not receive unsolicited frames. One account observer owns live updates/grace deadline; read failure reports unknown and fails open. Pin image digest/SHA, health commit, project and actual enforcement |
| Server + desktop | Matching `relay_client.rs`/`relay.rs` support repeated capabilities, suspend/restart task publication, project entitlement through server settings and desktop store into Preferences. Match `userId` to active account; omitted enforcement and unknown reads are not false denials. Package compatible server/daemon with the signed desktop and record version/SHA/architectures |
| Mobile (iOS acceptance outstanding) | `relayClient.ts`/tunnel access control handles 4402 and recovery; auth refresh/account changes discard stale results; account/foreground refresh, LAN selection and pairing remain independent. Baseline source release 1.0.2, staging/prod runtime 2.2.5, dev runtime 2.2.6; this work adds no native/runtime change. Record final iOS bundle id, version/build, archive tag/SHA, runtime, channel, update ID/source and installed-device evidence |

Source environment mapping (`.firebaserc`, mobile environment registry, kd registry):

| Environment | Firebase / Auth domain | Portal return origin | Relay | Registry enforcement |
|---|---|---|---|---|
| Staging | `kanna-staging` / `kanna-staging.firebaseapp.com` | `https://kanna-staging-account.web.app` | `wss://relay-staging.kanna.build` | off |
| Production | `kanna-build` / `kanna-build.firebaseapp.com` | `https://kanna-build-account.web.app` | `wss://relay.kanna.build` | off |

Auth SDK domains above are distinct from the email sender domains. Staging's
recorded sender is `accounts@auth-staging.kanna.build`; production
`accounts@auth.kanna.build` still requires the separate email runbook handoff.
Do not infer Stripe mode from the Firebase project or entitlement `environment`
stamp: the reducer stamps from the project and does not validate provider mode.
The parent's earlier HTTP/version observations are historical, not fresh readback.

## Final candidate acceptance still required

- Fresh install of the selected signed macOS artifact on a Mac without developer
  tools, plus **0.2.0 → selected candidate** updater install/restart with an
  existing task. Confirm daemon/session survival, new stage spawn, server/DB
  compatibility and same-account remote access. Follow canonical release/soak
  rules; no candidate is selected or approved by this source integration.
- [#1217](https://github.com/tampopogk/kanna/issues/1217) was still open on
  2026-09-14: bundle replacement before restart can invalidate the daemon's
  trusted executable path and silently refuse new spawns. Current
  `useAppUpdate.ts` still calls `downloadAndInstall`, sets `readyToRestart`, and
  waits for explicit `relaunch`; this code does not prove the issue resolved.
  Include the **installed-but-not-restarted** interval in the candidate check,
  followed by restart and a successful new spawn. A reproduced silent failure
  needs root's bounded release disposition/fix; do not weaken daemon trust or
  close the issue from billing evidence.
- [#1317](https://github.com/tampopogk/kanna/issues/1317) was still open on
  2026-09-14: staging .10 cloud transfer failed awaiting `auth_ok`. Current
  `cloud_transfer_proxy.rs` has credential-expiry diagnostics and
  `transfer_targets.rs` refuses/refreshes stale cloud routes before scheduling.
  This is a source mitigation, not reproduction evidence. The proxy does not
  opt into access updates; the relay preserves that older handshake. If the
  intended release promises cross-machine cloud task transfer (a paid capability
  advertised by this source), verify an actual transfer to durable `completed`
  with the selected server/relay pair, including stale-token refresh. Scheduled
  is not moved. If root excludes that promise, record the exclusion; this issue
  does not block a fixture proof of phone access. No automatic fix here.
- iOS: owner first settles purchase/link policy for the intended channel and
  storefronts. Baseline runtime 2.2.5 does not prove App Store 1.0 compatibility;
  the parent's archive 1.0.1(4)/runtime 2.2.2 was historical archive evidence only.
  Verify the actual installed/store/TestFlight binary and platform-specific OTA
  lineage. Native/config/signing-certificate changes require a new runtime and
  compatible binary. Later authorized submission follows the existing
  [production QA gate](../testing/mobile-production-qa-gate.md) and exact
  TestFlight/device acceptance: signup/verification, same-account off-LAN task
  and terminal access, foreground activation/expiry/recovery, account switch,
  ordinary billing/deletion, free LAN and iOS push. No owner-device operation,
  native build, signing or OTA publish is authorized here.

## Redacted production Stripe setup sheet — owner/operator later

**NOT SET UP**, confirmed by owner and reiterated after all sibling merges.
No secret values belong in this sheet or chat. Preserve one `cloud_monthly`
recurring monthly price, USD default, six currency options: JPY **500**;
USD/CAD/AUD/EUR/GBP **500 minor units** (5.00). No currency picker; do not infer
trials, annual pricing, proration, refund terms or beta eligibility.

| Later required record | Acceptance / approval |
|---|---|
| Project, account, mode | Owner confirms `kanna-build`, correct Stripe business account and **live** mode; keep staging account/test mode distinct. Record redacted account identifier and configuration evidence, never credentials |
| Catalog | Active `cloud_monthly`, recurring month, six existing currency options and amounts; record product/price IDs and authorized currency-localization observation. Fixture amounts are historical synthetic data, not a price catalog |
| Webhook | Exact production `stripeWebhook` endpoint/region and selected endpoint API version. Six events: `checkout.session.completed`, `customer.subscription.created`, `.updated`, `.deleted`, `invoice.paid`, `invoice.payment_failed`. Fixtures declare `2026-06-30.preview` and exercise item period ends / invoice parent subscription fields; this is not an instruction to select that version. Validate actual authorized sandbox payloads at the chosen endpoint version before live acceptance |
| Customer Portal | Owner selects cancellation timing, payment-method and invoice features, any plan-change/proration choices; record `STRIPE_PORTAL_CONFIGURATION_ID`. The code requires an explicit configuration and makes no feature-policy choice |
| Receipts / dunning / fraud | Owner confirms Stripe receipt sender/settings, failed-payment mail/retry/terminal-state behavior, support/refund escalation, 3DS/Radar configuration. Existing Checkout requests 3DS `any`; existing grace fallback is 14 days from period end without a next retry. Preserve those technical defaults without declaring commercial terms |
| Secret Manager | Required names only: `STRIPE_SECRET_KEY` bound to checkout, portal and deleteAccount; `STRIPE_WEBHOOK_SECRET` bound only to webhook. Record enabled version identifiers, project and binding/IAM evidence through authorized tooling; never values. No provisioning performed |
| Auth / origin | Record actual allowed Auth domains, sender-domain acceptance, hosting account target and `KANNA_PORTAL_BASE_URL`; same-origin return routes and portal/Functions project must match. No tokens in browser handoff URLs |
| Decisions and approvals | Owner decides beta cohort/comp cutoff, iOS model, commercial/legal/refund terms and support responder. Separate authorization for any sandbox transaction, production setup, enforcement enable, final candidate Ship and public payment CTA. Root routes later work; this sheet grants none |

## Existing operations and incident handoff

Use existing scoped health, `kd relay stats`, relay logs and Functions billing
logs. No new monitoring vendor or alert system is installed. **Alert recipient
and backup are not confirmed**: owner must name who watches the existing
surfaces and handles private support at `support@tampopomyoko.com` before paid
invitations. Root receives this handoff through the durable task result, not
terminal input. Proposed routing below is for that named responder to adopt.

| Signal | Responder's bounded action |
|---|---|
| Health unavailable/wrong commit; error rate or pressure | Check selected environment, relay image/health commit, uptime/restart and existing aggregate stats (connections, byte classes, tunnel buffer/admission pressure). Escalate broad access loss to owner/release responder; `/health` 200 proves liveness only |
| Webhook signature/config failure | `invalid_signature`/HTTP 400 or `not_configured`/500: match endpoint, API/event mode, secret **binding/version**, original-byte handling and recent deploy. Never paste signature/secret values. Pause invitations until authorized correction and a valid replay are observed |
| HTTP 200 but missing access | Search `Dropped a Stripe event that resolves to no Kanna account` with event ID/type/customer ID: `unresolved_account` is acknowledged, not retryable delivery success and not added to dedupe. Route every unexpected drop for owner billing reconciliation; repair verified mapping/event handling only with authorization |
| Payment without access | Privately collect environment, account email/uid, approximate time and redacted checkout/event correlation IDs (no card data). Check verified **same uid** on portal/desktop/phone; resolved Stripe customer/subscription, checkout ledger, webhook `outcome`/`entitlementWritten`, billing source and entitlement, relay project/actual policy, then negotiated client capabilities and 4402/unknown state. A return URL, active UI, stale index or successful health check alone is insufficient. Do not tell a payer to pay again |
| Checkout retry/reconciliation | Same durable attempt/key/payload is reused. `checkout_reconciliation_required` after uncertain creation's 23-hour bound or conflicting legacy sessions requires authorized inspection of provider request/session/subscription records. Time elapsed is not proof of retirement; never clear `creating`, rotate request keys or delete the ledger to unlock payment/deletion |
| Dunning / expired access | Correlate `invoice.payment_failed`, retry/grace deadline, later `invoice.paid` or terminal subscription event. Active records intentionally survive their period timestamp; a missing terminal webhook can leave access active. Check provider event delivery under authorized context; no automatic sweep or billing reconciliation job is present |
| Verification/reset mail | Reuse [email acceptance](account-email.md): fresh disposable Gmail and Microsoft 365 users, verification AND reset delivery, link completion, refreshed verified token/reset sign-in; record Inbox/Junk/quarantine and redacted authentication-header verdicts. Historical staging Gmail Inbox/M365 Junk did not capture headers or establish production/reset acceptance. Emulator OOB codes are not delivered mail |

Operator stats can include uid/desktop connection details; account-token stats
are aggregates only. Keep support cases and correlation identifiers private;
never include stats-token URLs, Auth tokens, pairing secrets or raw terminal
content in public issues. No alert channel/SLA, capacity certification or
automatic failed-webhook alert is claimed from these available logs.

For incident rollback, record the known-good **compatible set** before launch:
Functions/rules, portal build/origin, relay image/policy, desktop/server and iOS
binary/runtime/OTA. Use canonical kd under separate authorization. Review the
environment registry in every rollback source: old deploy code or a source
with `off` cannot silently represent the intended enforcement choice. Prefer a
compatible forward fix for protocol/schema changes; desktop recovery follows
the release runbook, not an invented downgrade. OTA rollback requires matching
platform/runtime. Temporary enforcement disable requires owner approval and
does not authorize refunds/cancellation.

Preserve live billing sources, checkout coordination, reverse maps, event
dedupe and deletion tombstones through deployments/rollbacks; never restore an
old DB snapshot or remove tombstones to “fix access.” **Explicit successful
account deletion is different**: it intentionally removes the user tree,
billing/index/checkout records and Auth, retaining only `accountDeletions/{uid}`
on Kanna's side. Stripe-side financial/customer records are not deleted by that
code. Do not promise a new retention period, refund or entitlement restoration;
re-registering creates a new uid and does not inherit a grant.

## Factual public-site / legal handoff (root forwards later)

Keep this as a compact handoff until deployed behavior and owner/legal approval
are known; no edits to kanna-web/marketing or legal effective dates were made.

| Current copy needing correction | Fact supported by integrated source |
|---|---|
| `docs/legal/support.md`: manual account requests, no account creation | Portal and mobile support self-service registration/verification; portal supports resend and password reset. Public onboarding should link the approved deployed portal, then downloads, same verified account on desktop/phone, desktop running and off-LAN access check |
| Support/privacy: no in-app deletion | Portal and mobile call authenticated `deleteAccount` after explicit confirmation. Ordinary Manage billing retains the account; deletion cancels immediately, removes cloud data/pairings/Auth, preserves local repositories/LAN data, offers no refund/undo. Support remains fallback for failed/ambiguous cases |
| `docs/legal/privacy-policy.md`: missing billing provider/data | Describe Stripe-hosted payment/billing management and the actual uid/customer/subscription/event/entitlement/checkout records, existing Google Auth/Firestore/relay processing and logs. Kanna receives billing references/status, not card details through these callables. Disclose the deletion tombstone and separately handled Stripe financial records. Owner/legal chooses retention/commercial language; do not invent it |
| Paid activation / free beta claims | Return success is pending until entitlement confirms; finite grace can expire; free local/LAN and existing push remain. Existing comp mechanism is preserved; the eligible beta cohort is undecided. Do not publish blanket new beta eligibility, refund/trial terms or an unapproved iOS purchase-link promise |

## Pass / gap at the manual gate

| Area | Status | Next owner |
|---|---|---|
| Integrated local technical behavior | **PASS**: four merged prerequisites; 19 relay integration tests including actual HTTP backend seam; focused typechecks; unchanged sibling evidence reused | Independent task reviewer/root |
| Deployed sandbox and hosted Stripe | **GAP**: no Functions-worker/deployed or hosted checkout/3DS transaction, provider-generated lifecycle, delivered mail or physical WAN client rehearsal in this result | Operator under separately authorized sandbox context |
| Production Stripe setup | **NOT SET UP**; redacted requirements above | Owner/operator |
| iOS purchase model / legal / beta policy | **UNDECIDED**, preserved | Owner/legal |
| Final release/device acceptance | **GAP**: compatible candidate artifacts/readbacks, fresh install/updater (#1217), conditional real cloud transfer (#1317), iOS native/runtime/OTA and device acceptance | Root/Ship + owner |
| Operations / public copy | **HANDOFF READY**; recipient routing, provider settings, mail acceptance and public corrections not applied | Owner/support + website owner through root |
| Publication authorization | **NOT GRANTED HERE**: no push/PR, deployment, payment, store, announcement or Ship | Root/operator's later workflow |


## Apple launch track — 2026-09-15

The approved native Apple channel is implemented alongside the existing web
billing boundary in task `8140ed16`. See [Apple subscription operations](apple-subscriptions.md)
for scoped ASC configuration, notification routing, source-aware support,
reconciliation and pending real sandbox/TestFlight acceptance. The entitlement
source is not the billing inventory: read both provider source records,
including on comp accounts. Existing Stripe activation prerequisites and this
runbook's hosted/payment/device limitations remain. Local signed-fixture
Functions→Firestore→relay evidence does not establish live Apple readiness.
