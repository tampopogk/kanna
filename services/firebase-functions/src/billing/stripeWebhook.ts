/**
 * `stripeWebhook` — the Stripe source's only writer.
 *
 * It writes `users/{uid}/billing/stripe` and never the entitlement doc; the
 * reducer derives that from every source (`docs/specs/accounts-and-billing.md`,
 * Decision 3). Correctness rests on three things the spec names explicitly:
 * signature verification, dedupe on `stripeEvents/{event.id}`, and out-of-order
 * resolution by the event's own `created` timestamp.
 *
 * Every outcome that is not a signature or configuration failure answers 200.
 * Stripe retries non-2xx responses, and an event Kanna cannot resolve — an
 * unknown customer, an event type it does not handle — will never resolve on a
 * retry either; it is logged and acknowledged instead of retried forever.
 */
import type { Firestore, Transaction } from "firebase-admin/firestore";
import { resolveWebhookConfig } from "./config.js";
import { applyEntitlement, readBillingState } from "./entitlement.js";
import { consoleBillingLogger, type BillingLogger } from "./logger.js";
import { StripeSignatureError, verifyStripeSignature } from "./stripeSignature.js";
import type { StripeOwnershipLookupGateway } from "./stripeGateway.js";
import { classifyProductOwnership, type ProductOwnershipVerdict } from "./stripeOwnership.js";
import {
  interpretStripeEvent,
  isHandledStripeEventType,
  parseStripeEventEnvelope,
  stripeEventCreatedAt,
  type StripeEventEnvelope,
  type StripeSourcePatch,
} from "./stripeEvents.js";
import {
  accountDeletionPath,
  billingSourcePath,
  stripeCustomerPath,
  stripeEventPath,
  userDocPath,
  type BilledSourceState,
  type BillingEnvironment,
  type EntitlementRecord,
} from "./types.js";

export interface StripeWebhookRequest {
  /** The exact bytes Stripe sent; a re-serialized body will not verify. */
  rawBody: Buffer | string;
  signature: string | undefined;
}

export type StripeWebhookOutcomeCode =
  | "applied"
  | "duplicate"
  | "stale"
  | "ignored"
  | "unresolved_account"
  | "deleted_account"
  | "invalid_payload"
  | "invalid_signature"
  | "not_configured"
  | "mode_mismatch"
  | "foreign_product"
  | "ambiguous_ownership"
  | "ownership_unresolved";

export interface StripeWebhookOutcome {
  httpStatus: number;
  code: StripeWebhookOutcomeCode;
  eventId: string | null;
  uid: string | null;
  entitlement: EntitlementRecord | null;
  entitlementWritten: boolean;
  message?: string;
}

export interface StripeWebhookDependencies {
  db: Firestore;
  env: NodeJS.ProcessEnv;
  logger?: BillingLogger;
  /** Injectable clock so tests can pin `updatedAt`. */
  now?: () => string;
  /** Read-only product-ownership lookups; defaults to the live Stripe client. */
  ownership?: StripeOwnershipLookupGateway;
}

export async function handleStripeWebhook(
  request: StripeWebhookRequest,
  deps: StripeWebhookDependencies
): Promise<StripeWebhookOutcome> {
  const logger = deps.logger ?? consoleBillingLogger;
  const now = deps.now ?? (() => new Date().toISOString());

  let config: ReturnType<typeof resolveWebhookConfig>;
  try {
    config = resolveWebhookConfig(deps.env);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logger.error("stripeWebhook is not configured", { message });
    return outcome({ httpStatus: 500, code: "not_configured", message });
  }

  let verified: unknown;
  try {
    verified = verifyStripeSignature({
      rawBody: request.rawBody,
      signature: request.signature,
      webhookSecret: config.webhookSecret,
    });
  } catch (error) {
    const message = error instanceof StripeSignatureError ? error.message : String(error);
    logger.warn("Rejected Stripe webhook with an invalid signature", { message });
    return outcome({ httpStatus: 400, code: "invalid_signature", message });
  }

  const event = parseStripeEventEnvelope(verified);
  if (!event) {
    logger.warn("Rejected a Stripe webhook whose payload is not an event envelope");
    return outcome({ httpStatus: 400, code: "invalid_payload" });
  }

  if (!isHandledStripeEventType(event.type)) {
    // Not written to the dedupe ledger: it records events Kanna acted on.
    return outcome({ httpStatus: 200, code: "ignored", eventId: event.id });
  }

  const expectedLive = config.environment === "production";
  if (event.livemode !== expectedLive) {
    // A misrouted webhook (test events reaching a live endpoint, or the
    // reverse) will never resolve on retry; drop and log rather than retry
    // forever.
    logger.error("Dropped a Stripe event delivered in the wrong mode for this environment", {
      eventId: event.id,
      eventType: event.type,
      environment: config.environment,
      livemode: event.livemode,
    });
    return outcome({ httpStatus: 200, code: "mode_mismatch", eventId: event.id });
  }

  const interpretation = interpretStripeEvent(event, {
    graceFallbackDays: config.graceFallbackDays,
  });
  if (interpretation.kind === "ignored") {
    return outcome({
      httpStatus: 200,
      code: "ignored",
      eventId: event.id,
      message: interpretation.reason,
    });
  }

  const ownership = deps.ownership
    ?? (await import("./stripeGateway.js")).stripeOwnershipLookupGateway(config.secretKey);
  let ownershipVerdict: ProductOwnershipVerdict;
  try {
    ownershipVerdict = await verifyEventOwnership(ownership, event, interpretation.patch, config.productId);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    logger.error("Could not verify Stripe product ownership; retrying", {
      eventId: event.id,
      eventType: event.type,
      message,
    });
    return outcome({ httpStatus: 503, code: "ownership_unresolved", eventId: event.id, message });
  }
  if (ownershipVerdict === "foreign") {
    // A proven event for another product on the shared account: acknowledge
    // and touch nothing. This is the expected steady state for Kanji Kongbu.
    return outcome({ httpStatus: 200, code: "foreign_product", eventId: event.id });
  }
  if (ownershipVerdict === "ambiguous") {
    logger.warn("Dropped a Stripe event whose product ownership could not be proven", {
      eventId: event.id,
      eventType: event.type,
    });
    return outcome({ httpStatus: 200, code: "ambiguous_ownership", eventId: event.id });
  }

  const uid = await resolveUid(deps.db, interpretation.account);
  if (!uid) {
    logger.warn("Dropped a Stripe event that resolves to no Kanna account", {
      eventId: event.id,
      eventType: event.type,
      customerId: interpretation.account.customerId,
    });
    return outcome({ httpStatus: 200, code: "unresolved_account", eventId: event.id });
  }

  const applied = await deps.db.runTransaction(async (transaction) =>
    applyStripeEvent({
      transaction,
      db: deps.db,
      uid,
      event,
      patch: interpretation.patch,
      customerId: interpretation.account.customerId,
      environment: config.environment,
      now: now(),
    })
  );

  logger.info("Handled Stripe webhook event", {
    eventId: event.id,
    eventType: event.type,
    uid,
    outcome: applied.code,
    entitlementWritten: applied.entitlementWritten,
  });
  return { ...applied, eventId: event.id, uid };
}

interface ApplyStripeEventInput {
  transaction: Transaction;
  db: Firestore;
  uid: string;
  event: StripeEventEnvelope;
  patch: StripeSourcePatch;
  customerId: string | null;
  environment: BillingEnvironment;
  now: string;
}

async function applyStripeEvent(
  input: ApplyStripeEventInput
): Promise<Omit<StripeWebhookOutcome, "eventId" | "uid">> {
  const { transaction, db, uid, event, patch, customerId, environment, now } = input;
  const eventRef = db.doc(stripeEventPath(event.id));
  const userRef = db.doc(userDocPath(uid));
  const deletionRef = db.doc(accountDeletionPath(uid));

  // Every read first: Firestore transactions forbid a read after a write.
  // The durable top-level deletion tombstone is the synchronization boundary.
  // A webhook transaction already in flight conflicts with the tombstone write
  // and retries; later events cannot race recursiveDelete into recreating data.
  const deletionDoc = await transaction.get(deletionRef);
  const userDoc = await transaction.get(userRef);
  const eventDoc = await transaction.get(eventRef);
  const state = await readBillingState(db, uid, transaction);

  if (deletionDoc.exists || !userDoc.exists) {
    return {
      httpStatus: 200,
      code: "deleted_account",
      entitlement: null,
      entitlementWritten: false,
    };
  }

  if (eventDoc.exists) {
    return {
      httpStatus: 200,
      code: "duplicate",
      entitlement: state.previous,
      entitlementWritten: false,
    };
  }

  const eventCreatedAt = stripeEventCreatedAt(event);
  transaction.set(eventRef, {
    eventId: event.id,
    type: event.type,
    createdAt: eventCreatedAt,
    uid,
    receivedAt: now,
  });

  if (customerId) {
    transaction.set(
      db.doc(stripeCustomerPath(customerId)),
      { uid, stripeCustomerId: customerId, updatedAt: now },
      { merge: true }
    );
  }

  const existing = state.sources.stripe;
  if (existing && Date.parse(existing.lastEventAt) > Date.parse(eventCreatedAt)) {
    // An older event arriving late must not walk back newer state.
    return {
      httpStatus: 200,
      code: "stale",
      entitlement: state.previous,
      entitlementWritten: false,
    };
  }

  const next = mergeStripeSourceState({
    existing,
    patch,
    eventId: event.id,
    eventCreatedAt,
    environment,
    now,
  });
  transaction.set(db.doc(billingSourcePath(uid, "stripe")), next);

  const written = applyEntitlement({
    db,
    uid,
    transaction,
    now,
    defaultEnvironment: environment,
    sources: { ...state.sources, stripe: next },
    previous: state.previous,
  });

  return {
    httpStatus: 200,
    code: "applied",
    entitlement: written.entitlement,
    entitlementWritten: written.written,
  };
}

interface MergeStripeSourceStateInput {
  existing: BilledSourceState | null;
  patch: StripeSourcePatch;
  eventId: string;
  eventCreatedAt: string;
  environment: BillingEnvironment;
  now: string;
}

/**
 * Fold one event's known fields onto the stored source state.
 *
 * Different Stripe events know different things — a completed checkout session
 * knows the ids but no period end, a failing invoice knows the retry schedule
 * but not the subscription status — so a patch carries only what its event
 * observed and everything else keeps the value the last event established.
 */
export function mergeStripeSourceState(input: MergeStripeSourceStateInput): BilledSourceState {
  const { existing, patch, eventId, eventCreatedAt, environment, now } = input;
  return {
    source: "stripe",
    status: patch.status ?? existing?.status ?? "expired",
    currentPeriodEndsAt:
      patch.currentPeriodEndsAt !== undefined
        ? patch.currentPeriodEndsAt
        : (existing?.currentPeriodEndsAt ?? null),
    graceEndsAt:
      patch.graceEndsAt !== undefined ? patch.graceEndsAt : (existing?.graceEndsAt ?? null),
    cancelAtPeriodEnd: patch.cancelAtPeriodEnd ?? existing?.cancelAtPeriodEnd ?? false,
    environment,
    stripeCustomerId: patch.stripeCustomerId ?? existing?.stripeCustomerId ?? null,
    stripeSubscriptionId: patch.stripeSubscriptionId ?? existing?.stripeSubscriptionId ?? null,
    appStoreOriginalTransactionId: null,
    lastEventAt: eventCreatedAt,
    lastEventId: eventId,
    updatedAt: now,
  };
}

/**
 * Prove which product an event's subscription (or, before a subscription
 * exists, its checkout session) belongs to.
 *
 * An event that names neither — malformed, or a shape this mapping has never
 * seen — cannot be proven and comes back `ambiguous`.
 */
async function verifyEventOwnership(
  ownership: StripeOwnershipLookupGateway,
  event: StripeEventEnvelope,
  patch: StripeSourcePatch,
  expectedProductId: string
): Promise<ProductOwnershipVerdict> {
  if (patch.stripeSubscriptionId) {
    const productIds = await ownership.subscriptionProductIds(patch.stripeSubscriptionId);
    return classifyProductOwnership(productIds, expectedProductId);
  }
  if (event.type === "checkout.session.completed") {
    const sessionId = event.data.object.id;
    if (typeof sessionId === "string" && sessionId.length > 0) {
      const productIds = await ownership.sessionProductIds(sessionId);
      return classifyProductOwnership(productIds, expectedProductId);
    }
  }
  return "ambiguous";
}

/**
 * Resolve the account an event belongs to.
 *
 * Checkout carries `client_reference_id` and every object Kanna creates carries
 * `metadata.firebase_uid`, so the reverse-lookup doc is the fallback for objects
 * created outside that path (a subscription started from the Stripe dashboard).
 */
async function resolveUid(
  db: Firestore,
  account: { uid: string | null; customerId: string | null }
): Promise<string | null> {
  if (account.uid) return account.uid;
  if (!account.customerId) return null;

  const mapping = await db.doc(stripeCustomerPath(account.customerId)).get();
  const uid = (mapping.data() as { uid?: unknown } | undefined)?.uid;
  return typeof uid === "string" && uid.length > 0 ? uid : null;
}

function outcome(
  partial: Pick<StripeWebhookOutcome, "httpStatus" | "code"> &
    Partial<Pick<StripeWebhookOutcome, "eventId" | "message">>
): StripeWebhookOutcome {
  return {
    eventId: null,
    uid: null,
    entitlement: null,
    entitlementWritten: false,
    ...partial,
  };
}
