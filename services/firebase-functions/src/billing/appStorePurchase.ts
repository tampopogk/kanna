import { randomUUID } from "node:crypto";
import type { Firestore } from "firebase-admin/firestore";
import { appStoreConfig } from "./appStoreConfig.js";
import { requireEnv, STRIPE_PRODUCT_ID_ENV, STRIPE_SECRET_KEY_ENV } from "./config.js";
import { assertCanSubscribe, assertSessionOwner, isRetired, type CheckoutCaller } from "./checkout.js";
import { readBillingState } from "./entitlement.js";
import { BillingRequestError } from "./errors.js";
import { createAppleGateway, type AppleGateway } from "./appStoreGateway.js";
import { applyAppleEvidence, appleConflict, AppleRevisionConflict } from "./appStoreEvents.js";
import { createAppleVerifier, signedInput, appleRetry, type AppleVerifier } from "./appStoreVerification.js";
import { stripeCheckoutGateway, type StripeCheckoutGateway } from "./stripeGateway.js";
import { accountCheckoutPath, accountDeletionPath, appAccountTokenPath, userDocPath } from "./types.js";

export interface AppleDependencies {
  db: Firestore; env: NodeJS.ProcessEnv; verifier?: AppleVerifier; gateway?: AppleGateway;
  stripe?: StripeCheckoutGateway; now?: () => string;
}
export function requireAppleCaller(caller: CheckoutCaller | null): CheckoutCaller {
  if (!caller) throw new BillingRequestError("unauthenticated", "sign_in_required", "Sign in to your Kanna account first.");
  if (!caller.emailVerified) throw new BillingRequestError("failed-precondition", "email_verification_required", "Verify your email address first.");
  return caller;
}

/** Read-only provider admission, followed by a Firestore recheck. This is not a
 * cross-provider payment lock; a delayed Apple approval may still overlap Stripe. */
export async function beginAppStorePurchase(caller: CheckoutCaller | null, deps: AppleDependencies) {
  const { uid } = requireAppleCaller(caller);
  const config = appStoreConfig(deps.env);
  const { db } = deps;
  const [checkout, user] = await db.getAll(db.doc(accountCheckoutPath(uid)), db.doc(userDocPath(uid)));
  const state = await readBillingState(db, uid);
  assertCanSubscribe(state);
  const coordination = checkout.data();
  if (coordination?.creating || (coordination?.attempt?.checkoutInput && !coordination.attempt.sessionId)) throw outstanding();
  const customerId: string | null = state.sources.stripe?.stripeCustomerId ?? user.data()?.stripeCustomerId ?? null;
  const sessionIds: string[] = [...new Set<string>([...(coordination?.sessionIds ?? []),
    ...(coordination?.attempt?.sessionId ? [coordination.attempt.sessionId] : [])])];
  if (customerId || sessionIds.length) {
    const stripe = deps.stripe ?? stripeCheckoutGateway(
      requireEnv(deps.env, STRIPE_SECRET_KEY_ENV),
      requireEnv(deps.env, STRIPE_PRODUCT_ID_ENV),
    );
    try {
      const sessions = [...await Promise.all(sessionIds.map(id => stripe.retrieveCheckoutSession(id))),
        ...(customerId ? await stripe.listOpenCheckoutSessions(customerId) : [])];
      // A session proven to belong to another product on the shared account
      // is not Kanna billing and cannot block this purchase; one that could
      // not be proven either way is not dropped, so assertSessionOwner still
      // rejects it into reconciliation below.
      for (const session of sessions.filter((session) => session.productVerdict !== "foreign")) {
        assertSessionOwner(session, uid, customerId);
        if (!isRetired(session)) throw outstanding();
      }
      if (customerId) {
        const blocking = await stripe.hasBlockingSubscription(customerId);
        // A verdict that cannot prove ownership either way is treated the
        // same as the sessions loop above: it must not silently pass.
        if (blocking === "ambiguous") throw new Error("Cannot verify existing Stripe billing ownership");
        if (blocking === "blocked") throw outstanding();
      }
    } catch (error) {
      if (error instanceof BillingRequestError) throw error;
      throw new BillingRequestError("internal", "stripe_error", "Could not confirm existing billing. Please try again before purchasing.");
    }
  }
  return db.runTransaction(async transaction => {
    const [deletion, currentCheckout, currentUser] = await transaction.getAll(db.doc(accountDeletionPath(uid)),
      db.doc(accountCheckoutPath(uid)), db.doc(userDocPath(uid)));
    const currentState = await readBillingState(db, uid, transaction);
    if (deletion.exists) throw appleConflict();
    assertCanSubscribe(currentState);
    if (!(checkout.updateTime ? currentCheckout.updateTime?.isEqual(checkout.updateTime) : !currentCheckout.updateTime)
      || customerId !== (currentState.sources.stripe?.stripeCustomerId ?? currentUser.data()?.stripeCustomerId ?? null)) throw outstanding();
    const token: string = currentUser.data()?.appAccountToken ?? randomUUID();
    const mapping = await transaction.get(db.doc(appAccountTokenPath(token)));
    if ((mapping.exists && mapping.data()?.uid !== uid) || (currentUser.data()?.appAccountToken && !mapping.exists)) throw appleConflict();
    // Fresh Auth accounts need no preexisting profile document.
    transaction.set(db.doc(userDocPath(uid)), { appAccountToken: token }, { merge: true });
    transaction.set(db.doc(appAccountTokenPath(token)), { uid });
    return { appAccountToken: token, productId: config.productId };
  });
}
const outstanding = () => new BillingRequestError("failed-precondition", "already_subscribed",
  "An existing subscription or checkout may still collect payment. Manage it before purchasing again.");

export async function registerAppStoreTransaction(request: unknown, caller: CheckoutCaller | null, deps: AppleDependencies) {
  const { uid } = requireAppleCaller(caller);
  const jws = signedInput((request as { signedTransaction?: unknown } | null)?.signedTransaction);
  const verifier = deps.verifier ?? createAppleVerifier(deps.env);
  const tx = await verifier.transaction(jws);
  const token = await deps.db.doc(appAccountTokenPath(tx.token)).get();
  if (token.data()?.uid !== uid) throw appleConflict();
  const gateway = deps.gateway ?? createAppleGateway(deps.env, verifier);
  // Bounded conflict retry refetches Apple, never blindly reapplies a stale API
  // snapshot. External calls stay outside Firestore's retryable transaction.
  for (let attempt = 0; attempt < 3; attempt++) {
    const before = await readBillingState(deps.db, uid);
    const evidence = await gateway.current(tx);
    try {
      return await applyAppleEvidence(deps.db, uid, evidence, {
        now: deps.now?.() ?? new Date().toISOString(), expectedRevision: before.sources.app_store?.revision ?? 0,
      });
    } catch (error) {
      if (!(error instanceof AppleRevisionConflict)) throw error;
    }
  }
  throw appleRetry();
}
