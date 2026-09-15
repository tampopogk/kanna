/**
 * `createCheckoutSession` — the web channel's only purchase entry point.
 *
 * Every refusal here is prevention rather than cleanup
 * (`docs/specs/accounts-and-billing.md`, Decisions 1, 3 and 4): an unverified
 * account cannot activate an entitlement, a comped account must never be
 * allowed to pay, and an App-Store-sourced subscriber must be sent to Apple's
 * settings instead of into a second, parallel subscription.
 */
import { randomUUID } from "node:crypto";
import type { Firestore, Transaction } from "firebase-admin/firestore";
import {
  CheckoutContractError,
  parseCheckoutSessionRequest,
  type CheckoutSessionResponse,
} from "./contract.js";
import { resolveCheckoutConfig } from "./config.js";
import { readBillingState, type BillingState } from "./entitlement.js";
import { BillingRequestError } from "./errors.js";
import { consoleBillingLogger, type BillingLogger } from "./logger.js";
import type {
  StripeCheckoutGateway,
  StripeCheckoutSessionInput,
  StripeCheckoutSessionState,
  StripeCustomerInput,
} from "./stripeGateway.js";
import {
  accountCheckoutPath,
  accountDeletionPath,
  stripeCustomerPath,
  userDocPath,
  type BilledSourceState,
} from "./types.js";

export interface CheckoutCaller {
  uid: string;
  email: string | null;
  emailVerified: boolean;
}

export interface CreateCheckoutSessionDependencies {
  db: Firestore;
  env: NodeJS.ProcessEnv;
  /** Defaults to the live Stripe client built from the configured secret key. */
  gateway?: StripeCheckoutGateway;
  logger?: BillingLogger;
  now?: () => string;
}

function isBlockingStatus(state: BilledSourceState | null): boolean {
  return state !== null && (state.status === "active" || state.status === "grace");
}

export async function createCheckoutSession(
  request: unknown,
  caller: CheckoutCaller | null,
  deps: CreateCheckoutSessionDependencies
): Promise<CheckoutSessionResponse> {
  const logger = deps.logger ?? consoleBillingLogger;
  const now = deps.now ?? (() => new Date().toISOString());

  if (!caller) {
    throw new BillingRequestError(
      "unauthenticated",
      "sign_in_required",
      "Sign in before starting a subscription."
    );
  }
  if (!caller.emailVerified) {
    throw new BillingRequestError(
      "failed-precondition",
      "email_verification_required",
      "Verify your email address before subscribing."
    );
  }

  let parsedRequest: ReturnType<typeof parseCheckoutSessionRequest>;
  try {
    parsedRequest = parseCheckoutSessionRequest(request);
  } catch (error) {
    if (error instanceof CheckoutContractError) {
      throw new BillingRequestError("invalid-argument", error.reason, error.message);
    }
    throw error;
  }
  const { plan } = parsedRequest;

  let config: ReturnType<typeof resolveCheckoutConfig>;
  try {
    config = resolveCheckoutConfig(deps.env);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logger.error("createCheckoutSession is not configured", { message });
    throw new BillingRequestError("internal", "not_configured", message);
  }

  const gateway = deps.gateway ?? (await liveGateway(config.secretKey));
  const base = config.portalBaseUrl.replace(/\/+$/, "");
  try {
    let attempt = await admitCheckout(deps.db, caller, base, now());
    if (attempt.sessionId) {
      const existing = await gateway.retrieveCheckoutSession(attempt.sessionId);
      assertSessionOwner(existing, caller.uid, attempt.customerId);
      if (!isRetired(existing)) {
        return await checkoutResponse(deps.db, gateway, caller.uid, attempt, existing, plan, now());
      }
      // Stripe, not a local deadline or a delayed webhook, proves the previous
      // session can never be paid again. Only one caller can replace its owner.
      attempt = await admitCheckout(deps.db, caller, base, now(), attempt.id);
    }

    if (!attempt.customerId) {
      assertReplayWindow(attempt, now());
      const customer = await gateway.createCustomer(attempt.customerInput);
      attempt = await updateAttempt(deps.db, caller.uid, attempt.id, now(), (current, transaction) => {
        if (current.customerId) return current;
        transaction.set(deps.db.doc(userDocPath(caller.uid)), {
          stripeCustomerId: customer.id, updatedAt: now(),
        }, { merge: true });
        transaction.set(deps.db.doc(stripeCustomerPath(customer.id)), {
          uid: caller.uid, stripeCustomerId: customer.id, updatedAt: now(),
        }, { merge: true });
        return { ...current, customerId: customer.id };
      });
    }
    const customerId = attempt.customerId;
    if (!customerId) throw reconciliationRequired();

    if (!attempt.checkoutInput && !attempt.sessionId) {
      // Old ledgers may already have issued a URL. Inspect every recorded
      // session before migrating; never expire/cancel a session to make room.
      const previous = await Promise.all(attempt.previousSessionIds.map((id) => gateway.retrieveCheckoutSession(id)));
      // Older error paths released admission without recording an uncertain
      // response. Include open sessions on this customer, not just ledger ids.
      const open = await gateway.listOpenCheckoutSessions(customerId);
      const candidates = [...new Map([...previous, ...open].map((session) => [session.id, session])).values()];
      for (const session of candidates) assertSessionOwner(session, caller.uid, customerId);
      const outstanding = candidates.filter((session) => !isRetired(session));
      if (outstanding.length > 1) throw reconciliationRequired();
      const [existing] = outstanding;
      if (existing) {
        attempt = await recordSession(deps.db, caller.uid, attempt.id, existing.id, now());
        return await checkoutResponse(deps.db, gateway, caller.uid, attempt, existing, plan, now());
      }
      // Incomplete/unpaid/paused subscriptions can still collect money even
      // though they don't grant access. Entitlement expiry is not retirement.
      if (await gateway.hasBlockingSubscription(customerId)) throw alreadySubscribed();
      const priceId = await gateway.resolvePriceId("cloud_monthly");
      if (!priceId) throw new Error("No active Stripe price has lookup_key cloud_monthly");
      attempt = await updateAttempt(deps.db, caller.uid, attempt.id, now(), (current) => {
        if (current.checkoutInput || current.sessionId) return current;
        return { ...current, checkoutInput: {
          uid: caller.uid, customerId, priceId,
          successUrl: current.successUrl, cancelUrl: current.cancelUrl,
          idempotencyKey: `checkout-${current.id}`,
          expiresAt: Math.floor(Date.parse(now()) / 1000) + 24 * 60 * 60,
        }, checkoutStartedAt: now() };
      }, true);
    }

    if (!attempt.sessionId) {
      assertReplayWindow(attempt, now());
      if (!attempt.checkoutInput) throw reconciliationRequired();
      const session = await gateway.createCheckoutSession(attempt.checkoutInput);
      // An error or process exit before this transaction leaves the immutable
      // request owned by this attempt. A new invocation replays the same key.
      attempt = await recordSession(deps.db, caller.uid, attempt.id, session.id, now());
    }
    if (!attempt.sessionId) throw reconciliationRequired();
    const session = await gateway.retrieveCheckoutSession(attempt.sessionId);
    assertSessionOwner(session, caller.uid, customerId);
    logger.info("Resolved a Stripe checkout session", { uid: caller.uid, plan, sessionId: session.id });
    return await checkoutResponse(deps.db, gateway, caller.uid, attempt, session, plan, now());
  } catch (error) {
    // Never release an uncertain external operation or cancel a possibly-paid
    // session on an ordinary error. The existing deletion ledger retains it.
    if (error instanceof BillingRequestError) throw error;
    const message = error instanceof Error ? error.message : String(error);
    logger.error("Stripe checkout requires retry or reconciliation", { uid: caller.uid, message });
    throw new BillingRequestError("internal", "stripe_error", "Could not start checkout. Please try again.");
  }
}

interface CheckoutAttempt {
  id: string;
  startedAt: string;
  customerInput: StripeCustomerInput;
  customerId: string | null;
  successUrl: string;
  cancelUrl: string;
  checkoutInput: StripeCheckoutSessionInput | null;
  checkoutStartedAt: string | null;
  sessionId: string | null;
  previousSessionIds: string[];
}

interface CheckoutCoordination {
  creating?: boolean;
  sessionIds?: string[];
  attempt?: CheckoutAttempt;
}

export function assertCanSubscribe(state: BillingState): void {
  if (state.sources.comp?.active) {
    throw new BillingRequestError("failed-precondition", "comp_active",
      "This account has complimentary Kanna Cloud access and does not need a subscription.");
  }
  if (isBlockingStatus(state.sources.app_store) || state.sources.app_store?.paymentOutstanding) {
    throw new BillingRequestError("failed-precondition", "app_store_active",
      "This account is subscribed through the App Store. Manage it in Apple's subscription settings.");
  }
  if (isBlockingStatus(state.sources.stripe)) throw alreadySubscribed();
}

function alreadySubscribed(): BillingRequestError {
  return new BillingRequestError("failed-precondition", "already_subscribed",
    "This account already has a subscription or a payment awaiting confirmation. Review your account before subscribing again.");
}

function reconciliationRequired(): BillingRequestError {
  return new BillingRequestError("failed-precondition", "checkout_reconciliation_required",
    "An earlier checkout needs reconciliation. Contact support before starting another payment.");
}

function assertReplayWindow(attempt: CheckoutAttempt, now: string): void {
  // Stripe may prune keys after 24h. This is a fail-closed replay bound, not
  // a lock lease: elapsed time NEVER authorizes a new customer or session.
  const startedAt = attempt.customerId ? attempt.checkoutStartedAt : attempt.startedAt;
  const age = Date.parse(now) - Date.parse(startedAt ?? "");
  if (!Number.isFinite(age) || age < 0 || age >= 23 * 60 * 60 * 1000) throw reconciliationRequired();
}

export function assertSessionOwner(session: StripeCheckoutSessionState, uid: string, customerId: string | null): void {
  if (session.mode !== "subscription" || session.uid !== uid || !customerId || session.customerId !== customerId) {
    throw reconciliationRequired();
  }
}

export function isRetired(session: StripeCheckoutSessionState): boolean {
  return session.status === "expired"
    || (session.status === "complete" && (session.subscriptionStatus === "canceled"
      || session.subscriptionStatus === "incomplete_expired"));
}

async function checkoutResponse(
  db: Firestore, gateway: StripeCheckoutGateway, uid: string, attempt: CheckoutAttempt,
  session: StripeCheckoutSessionState, plan: CheckoutSessionResponse["plan"], now: string,
): Promise<CheckoutSessionResponse> {
  if (session.status === "open" && attempt.customerId && await gateway.hasBlockingSubscription(attempt.customerId)) {
    throw alreadySubscribed();
  }
  await updateAttempt(db, uid, attempt.id, now, (current) => current, true);
  if (session.status === "complete") throw alreadySubscribed();
  if (session.status !== "open" || !session.url || !attempt.customerId) {
    throw new BillingRequestError("failed-precondition", "checkout_in_progress",
      "Checkout is no longer open. Please try again to check its current status.");
  }
  return { sessionId: session.id, url: session.url, customerId: attempt.customerId, plan };
}

async function admitCheckout(
  db: Firestore, caller: CheckoutCaller, base: string, now: string, replaceId?: string,
): Promise<CheckoutAttempt> {
  const checkoutRef = db.doc(accountCheckoutPath(caller.uid));
  return db.runTransaction(async (transaction) => {
    const [deletion, checkout, user] = await Promise.all([
      transaction.get(db.doc(accountDeletionPath(caller.uid))),
      transaction.get(checkoutRef), transaction.get(db.doc(userDocPath(caller.uid))),
    ]);
    const state = await readBillingState(db, caller.uid, transaction);
    assertNotDeleted(deletion.exists);
    const coordination = checkout.data() as CheckoutCoordination | undefined;
    if (coordination?.attempt && (!replaceId || coordination.attempt.id !== replaceId)) {
      // An unresolved request must be recoverable even if its webhook arrived
      // first. Recovery records the session; the response still checks guards.
      if (!coordination.creating) assertCanSubscribe(state);
      return coordination.attempt;
    }
    assertCanSubscribe(state);
    if (coordination?.creating) throw reconciliationRequired(); // pre-idempotency legacy attempt
    const id = randomUUID();
    const customerId = state.sources.stripe?.stripeCustomerId
      ?? (user.data() as { stripeCustomerId?: string } | undefined)?.stripeCustomerId ?? null;
    const attempt: CheckoutAttempt = {
      id, startedAt: now, customerInput: { uid: caller.uid, email: caller.email, idempotencyKey: `customer-${id}` },
      customerId, successUrl: `${base}/billing/success?session_id={CHECKOUT_SESSION_ID}`,
      cancelUrl: `${base}/billing/canceled`, checkoutInput: null, checkoutStartedAt: null, sessionId: null,
      previousSessionIds: replaceId ? [] : coordination?.sessionIds ?? [],
    };
    transaction.set(checkoutRef, {
      uid: caller.uid, creating: !customerId, sessionIds: coordination?.sessionIds ?? [], attempt, updatedAt: now,
    });
    return attempt;
  });
}

function assertNotDeleted(deleted: boolean): void {
  if (deleted) throw new BillingRequestError("failed-precondition", "account_deleted", "This account has been permanently deleted.");
}

async function updateAttempt(
  db: Firestore, uid: string, id: string, now: string,
  update: (attempt: CheckoutAttempt, transaction: Transaction) => CheckoutAttempt,
  checkGuards = false,
): Promise<CheckoutAttempt> {
  const checkoutRef = db.doc(accountCheckoutPath(uid));
  return db.runTransaction(async (transaction) => {
    const [deletion, checkout] = await Promise.all([
      transaction.get(db.doc(accountDeletionPath(uid))), transaction.get(checkoutRef),
    ]);
    const state = checkGuards ? await readBillingState(db, uid, transaction) : null;
    assertNotDeleted(deletion.exists);
    const coordination = checkout.data() as CheckoutCoordination | undefined;
    if (!coordination?.attempt || coordination.attempt.id !== id) {
      throw new BillingRequestError("failed-precondition", "checkout_in_progress", "Checkout changed. Please try again.");
    }
    if (state) assertCanSubscribe(state);
    const attempt = update(coordination.attempt, transaction);
    transaction.set(checkoutRef, {
      ...coordination, attempt, updatedAt: now,
      creating: !attempt.customerId || (!!attempt.checkoutInput && !attempt.sessionId),
      sessionIds: [...new Set([...(coordination.sessionIds ?? []), ...(attempt.sessionId ? [attempt.sessionId] : [])])],
    });
    return attempt;
  });
}

async function recordSession(db: Firestore, uid: string, id: string, sessionId: string, now: string): Promise<CheckoutAttempt> {
  return updateAttempt(db, uid, id, now, (attempt) => {
    if (attempt.sessionId && attempt.sessionId !== sessionId) throw reconciliationRequired();
    return { ...attempt, sessionId };
  });
}

/** Imported lazily so a missing Stripe key never breaks module load or deploy. */
async function liveGateway(secretKey: string): Promise<StripeCheckoutGateway> {
  const { stripeCheckoutGateway } = await import("./stripeGateway.js");
  return stripeCheckoutGateway(secretKey);
}
