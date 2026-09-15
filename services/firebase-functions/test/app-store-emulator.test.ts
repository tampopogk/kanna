import { randomUUID } from "node:crypto";
import type { Firestore } from "firebase-admin/firestore";
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { beginAppStorePurchase, registerAppStoreTransaction, type AppleDependencies } from "../src/billing/appStorePurchase.js";
import { handleAppStoreNotification } from "../src/billing/appStoreNotifications.js";
import { applyAppleEvidence, AppleRevisionConflict, type AppleSubscription } from "../src/billing/appStoreEvents.js";
import { createCheckoutSession } from "../src/billing/checkout.js";
import { firestoreAccountDeletionStore } from "../src/accountDeletion.js";
import { appStoreSource, compSource, stripeSource } from "../src/billing/fixtures.js";
import { billingSourcePath, entitlementPath } from "../src/billing/types.js";
import { appleEnv, appleJws, fixtureVerifier, notificationJws, renewalClaims, transactionClaims } from "./support/appleFixtures.js";
import { clearFirestoreEmulator, emulatorFirestore, hasFirestoreEmulator, shutdownEmulatorFirestore } from "./support/emulator.js";
import type { AppleEvidence } from "../src/billing/appStoreVerification.js";
import type { StripeCheckoutGateway } from "../src/billing/stripeGateway.js";

const caller = { uid: "apple-owner", email: "apple@example.test", emailVerified: true };
const verifier = fixtureVerifier();
const now = () => new Date().toISOString();
let clock = Date.now();
async function evidence(token: string, changes: Record<string, unknown> = {}, renewal: Record<string, unknown> = {}, status = 1): Promise<AppleEvidence> {
  const signedDate = ++clock;
  const pair = await verifier.pair(appleJws(transactionClaims(token, { signedDate, ...changes })),
    appleJws(renewalClaims({ signedDate, ...renewal })), changes.environment === "Production" ? "production" : "sandbox");
  return { ...pair, status, signedDate };
}

(hasFirestoreEmulator ? describe : describe.skip)("Apple billing persistence", () => {
  let db: Firestore;
  let token: string;
  let deps: AppleDependencies;
  beforeAll(() => { db = emulatorFirestore(); });
  afterAll(shutdownEmulatorFirestore);
  beforeEach(async () => {
    await clearFirestoreEmulator();
    deps = { db, env: appleEnv, verifier };
    token = (await beginAppStorePurchase(caller, deps)).appAccountToken;
  });
  it("creates a stable server token for a fresh verified Auth account and refuses unverified callers", async () => {
    expect((await beginAppStorePurchase(caller, deps)).appAccountToken).toBe(token);
    expect((await db.doc(`appAccountTokens/${token}`).get()).data()).toEqual({ uid: caller.uid });
    await expect(beginAppStorePurchase({ ...caller, emailVerified: false }, deps)).rejects.toMatchObject({ reason: "email_verification_required" });
    await expect(beginAppStorePurchase(null, deps)).rejects.toMatchObject({ reason: "sign_in_required" });
  });
  it("requires current server status on restore and strictly binds the token to its original uid", async () => {
    const current = await evidence(token, { revocationDate: clock }, {}, 5);
    const gateway = { current: vi.fn(async () => [current]), async *notificationHistory() {} };
    const request = { signedTransaction: appleJws(transactionClaims(token)) };
    const result = await registerAppStoreTransaction(request, caller, { ...deps, gateway });
    expect(result.outcome).toBe("inactive");
    expect(result.billing.sources.app_store?.status).toBe("revoked");
    expect(gateway.current).toHaveBeenCalledOnce();
    await expect(registerAppStoreTransaction(request, { ...caller, uid: "other" }, { ...deps, gateway })).rejects.toMatchObject({ reason: "apple_account_conflict" });
    await expect(registerAppStoreTransaction({ signedTransaction: appleJws(transactionClaims()) }, caller, { ...deps, gateway })).rejects.toMatchObject({ reason: "apple_account_conflict" });
  });
  it("atomically dedupes, rejects stale/equal-date grants after revoke, and accepts newer reversal", async () => {
    const active = await evidence(token);
    await applyAppleEvidence(db, caller.uid, [active], { now: now() });
    const revoked = { ...active, status: 5, transaction: { ...active.transaction, revoked: true } };
    const notification = { id: randomUUID(), type: "REFUND", environment: "sandbox" as const, evidence: revoked };
    await applyAppleEvidence(db, caller.uid, [revoked], { now: now(), notification });
    expect((await applyAppleEvidence(db, caller.uid, [active], { now: now() })).outcome).toBe("duplicate");
    expect((await applyAppleEvidence(db, caller.uid, [revoked], { now: now(), notification })).outcome).toBe("duplicate");
    expect((await db.doc(entitlementPath(caller.uid)).get()).data()?.status).toBe("revoked");
    const reversal = await evidence(token, { purchaseDate: active.transaction.purchaseDate });
    await applyAppleEvidence(db, caller.uid, [reversal], { now: now() });
    expect((await db.doc(entitlementPath(caller.uid)).get()).data()?.status).toBe("active");
  });
  it("keeps production and overlapping originals when sandbox expires or an older original is refunded", async () => {
    const production = await evidence(token, { environment: "Production" }, { environment: "Production" });
    const sandbox = await evidence(token);
    await applyAppleEvidence(db, caller.uid, [production, sandbox], { now: now() });
    const expired = await evidence(token, { expiresDate: Date.now() - 1000 }, { autoRenewStatus: 0 }, 2);
    await applyAppleEvidence(db, caller.uid, [expired], { now: now() });
    expect((await db.doc(entitlementPath(caller.uid)).get()).data()?.environment).toBe("production");
    const second = await evidence(token, { originalTransactionId: "200", transactionId: "201", environment: "Production" }, { originalTransactionId: "200", environment: "Production" });
    await applyAppleEvidence(db, caller.uid, [second], { now: now() });
    await applyAppleEvidence(db, caller.uid, [{ ...production, transaction: { ...production.transaction, revoked: true }, status: 5 }], { now: now() });
    expect((await db.doc(entitlementPath(caller.uid)).get()).data()).toMatchObject({ status: "active", appStoreOriginalTransactionId: "200" });
    expect((await db.collection("appStoreSubscriptions").get()).size).toBe(3);
  });
  it("honors paid restore beside Stripe and comp and retains explicit provenance", async () => {
    await db.doc(billingSourcePath(caller.uid, "stripe")).set(stripeSource());
    await applyAppleEvidence(db, caller.uid, [await evidence(token)], { now: now() });
    expect((await db.doc(entitlementPath(caller.uid)).get()).data()?.duplicateSources).toBe(true);
    await db.doc(billingSourcePath(caller.uid, "comp")).set(compSource());
    await applyAppleEvidence(db, caller.uid, [await evidence(token)], { now: now() });
    expect((await db.doc(entitlementPath(caller.uid)).get()).data()?.source).toBe("comp");
    expect((await db.doc(billingSourcePath(caller.uid, "app_store")).get()).data()?.environment).toBe("sandbox");
    expect((await db.doc(billingSourcePath(caller.uid, "stripe")).get()).exists).toBe(true);
    await expect(beginAppStorePurchase(caller, deps)).rejects.toMatchObject({ reason: "comp_active" });
  });
  it("fences stale API status and refetches after an intervening notification", async () => {
    const initial = await evidence(token);
    const gateway = { current: vi.fn(async () => {
      if (gateway.current.mock.calls.length === 1) {
        await applyAppleEvidence(db, caller.uid, [{ ...initial, status: 5, transaction: { ...initial.transaction, revoked: true } }], { now: now() });
        return [initial];
      }
      return [{ ...initial, status: 5, transaction: { ...initial.transaction, revoked: true } }];
    }), async *notificationHistory() {} };
    const result = await registerAppStoreTransaction({ signedTransaction: appleJws(transactionClaims(token)) }, caller, { ...deps, gateway });
    expect(gateway.current).toHaveBeenCalledTimes(2);
    expect(result.billing.sources.app_store?.status).toBe("revoked");
    await expect(applyAppleEvidence(db, caller.uid, [initial], { now: now(), expectedRevision: 0 })).rejects.toBeInstanceOf(AppleRevisionConflict);
  });
  it("does not dedupe transient failure and cannot recreate a deleted account or rebind its token", async () => {
    const payload = { signedPayload: notificationJws(transactionClaims(token), renewalClaims()) };
    const unavailable = { ...deps, verifier: { ...verifier, notification: vi.fn(async () => { throw new Error("offline"); }) } };
    expect((await handleAppStoreNotification(payload, unavailable)).httpStatus).toBe(503);
    expect((await db.collection("appleNotifications").get()).empty).toBe(true);
    expect((await handleAppStoreNotification(payload, deps)).httpStatus).toBe(200);
    const deletion = firestoreAccountDeletionStore(db);
    await deletion.markAccountDeletionStarted(caller.uid);
    await expect(applyAppleEvidence(db, caller.uid, [await evidence(token)], { now: now() })).rejects.toMatchObject({ reason: "apple_account_conflict" });
    await deletion.deleteUserTree(caller.uid);
    await deletion.deleteBillingIndexes(caller.uid);
    expect((await handleAppStoreNotification(payload, deps)).code).toBe("unresolved_account");
    for (const name of ["appAccountTokens", "appleNotifications", "appStoreSubscriptions"]) expect((await db.collection(name).get()).empty).toBe(true);
    expect((await db.doc(`users/${caller.uid}`).get()).exists).toBe(false);
    await expect(beginAppStorePurchase(caller, deps)).rejects.toMatchObject({ reason: "apple_account_conflict" });
  });
  it("blocks both channels during Apple retry, unresolved Stripe creation and provider read failure", async () => {
    await db.doc(billingSourcePath(caller.uid, "app_store")).set({ ...appStoreSource(), status: "expired", paymentOutstanding: true });
    await expect(beginAppStorePurchase(caller, deps)).rejects.toMatchObject({ reason: "app_store_active" });
    await expect(createCheckoutSession({ plan: "monthly" }, caller, { db, env: { STRIPE_SECRET_KEY: "test", KANNA_PORTAL_BASE_URL: "https://example.test" } })).rejects.toMatchObject({ reason: "app_store_active" });
    await db.doc(billingSourcePath(caller.uid, "app_store")).delete();
    await db.doc(`accountCheckouts/${caller.uid}`).set({ creating: true });
    await expect(beginAppStorePurchase(caller, deps)).rejects.toMatchObject({ reason: "already_subscribed" });
    await db.doc(`accountCheckouts/${caller.uid}`).delete();
    await db.doc(`users/${caller.uid}`).set({ stripeCustomerId: "cus_existing" }, { merge: true });
    const stripe: StripeCheckoutGateway = {
      createCustomer: vi.fn(), resolvePriceId: vi.fn(), createCheckoutSession: vi.fn(),
      retrieveCheckoutSession: vi.fn(), closeCheckoutSession: vi.fn(),
      listOpenCheckoutSessions: vi.fn(async () => { throw new Error("provider offline"); }),
      hasBlockingSubscription: vi.fn(async () => false),
    };
    await expect(beginAppStorePurchase(caller, { ...deps, stripe })).rejects.toMatchObject({ reason: "stripe_error" });
    stripe.listOpenCheckoutSessions = vi.fn(async () => []);
    stripe.hasBlockingSubscription = vi.fn(async () => true);
    await expect(beginAppStorePurchase(caller, { ...deps, stripe })).rejects.toMatchObject({ reason: "already_subscribed" });
    stripe.hasBlockingSubscription = vi.fn(async () => {
      await db.doc(`accountCheckouts/${caller.uid}`).set({ creating: true });
      return false;
    });
    await expect(beginAppStorePurchase(caller, { ...deps, stripe })).rejects.toMatchObject({ reason: "already_subscribed" });
    expect(stripe.createCustomer).not.toHaveBeenCalled();
  });
});
