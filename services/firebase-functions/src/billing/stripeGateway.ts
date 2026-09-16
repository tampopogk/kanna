/**
 * The narrow slice of the Stripe API the billing backend calls.
 *
 * Kept behind an interface so the emulator tests exercise the real
 * `createCheckoutSession` logic — its guards, its customer reuse, the metadata
 * it stamps — without making live Stripe calls in CI.
 */
import Stripe from "stripe";
import { classifyProductOwnership, type CustomerProductScan, type ProductOwnershipVerdict } from "./stripeOwnership.js";

export interface StripePortalGateway {
  createPortalSession(input: {
    customerId: string;
    returnUrl: string;
    configurationId: string;
  }): Promise<{ url: string }>;
}

export function stripePortalGateway(secretKey: string): StripePortalGateway {
  const stripe = new Stripe(secretKey);
  return {
    async createPortalSession(input) {
      const session = await stripe.billingPortal.sessions.create({
        customer: input.customerId,
        return_url: input.returnUrl,
        configuration: input.configurationId,
      });
      return { url: session.url };
    },
  };
}

export interface StripeCustomerInput {
  uid: string;
  email: string | null;
  idempotencyKey: string;
}

export interface StripeCheckoutSessionInput {
  uid: string;
  customerId: string;
  priceId: string;
  successUrl: string;
  cancelUrl: string;
  idempotencyKey: string;
  expiresAt: number;
}

export interface StripeCheckoutSession {
  id: string;
  url: string | null;
}

export interface StripeCheckoutSessionState extends StripeCheckoutSession {
  mode: string | null;
  uid: string | null;
  customerId: string | null;
  status: "open" | "complete" | "expired" | null;
  subscriptionStatus: string | null;
  /** Whether this session's own line items prove it belongs to the Kanna product. */
  productVerdict: ProductOwnershipVerdict;
}

/** Whether a customer's subscriptions block a new purchase, scoped to the Kanna product. */
export type SubscriptionBlockVerdict = "blocked" | "clear" | "ambiguous";

export interface StripeCheckoutGateway {
  createCustomer(input: StripeCustomerInput): Promise<{ id: string }>;
  resolvePriceId(lookupKey: string): Promise<string | null>;
  createCheckoutSession(input: StripeCheckoutSessionInput): Promise<StripeCheckoutSession>;
  retrieveCheckoutSession(sessionId: string): Promise<StripeCheckoutSessionState>;
  listOpenCheckoutSessions(customerId: string): Promise<StripeCheckoutSessionState[]>;
  hasBlockingSubscription(customerId: string): Promise<SubscriptionBlockVerdict>;
  closeCheckoutSession(sessionId: string): Promise<void>;
}

/** Which Kanna account (and, when known, which Stripe customer) a mutation must belong to. */
export interface StripeBillingObjectContext {
  uid: string;
  customerId: string | null;
}

export interface StripeSubscriptionGateway {
  cancelSubscription(subscriptionId: string, context: StripeBillingObjectContext): Promise<void>;
  closeCheckoutSession(sessionId: string, context: StripeBillingObjectContext): Promise<void>;
  closeCustomerBilling(customerId: string, context: { uid: string }): Promise<void>;
}

/**
 * Read-only product-ownership lookups against the shared Stripe account.
 *
 * A webhook or Checkout/Invoice payload cannot be trusted to carry complete
 * line-item data, so ownership is proven by reading the object's items back
 * from Stripe rather than trusting whatever the payload happened to include.
 */
export interface StripeOwnershipLookupGateway {
  /** Distinct product ids across a subscription's line items, paginated. */
  subscriptionProductIds(subscriptionId: string): Promise<string[]>;
  /** Distinct product ids across a checkout session's line items, paginated. */
  sessionProductIds(sessionId: string): Promise<string[]>;
  /** A full scan of every non-deleted subscription a customer holds, distinguishing empty from unresolved. */
  customerProductScan(customerId: string): Promise<CustomerProductScan>;
}

export function stripeOwnershipLookupGateway(secretKey: string): StripeOwnershipLookupGateway {
  const stripe = new Stripe(secretKey);
  return {
    subscriptionProductIds: (subscriptionId) => subscriptionProductIds(stripe, subscriptionId),
    sessionProductIds: (sessionId) => sessionProductIds(stripe, sessionId),
    customerProductScan: (customerId) => customerProductScan(stripe, customerId),
  };
}

/**
 * Every billing object a shared customer holds, not only subscriptions: a
 * one-time or historical invoice for a foreign product, with no subscription
 * of its own, would otherwise be invisible to this scan while still being
 * exposed by a customer-wide Portal session with invoice history enabled.
 */
async function customerProductScan(stripe: Stripe, customerId: string): Promise<CustomerProductScan> {
  const ids = new Set<string>();
  let unresolved = false;
  for await (const subscription of stripe.subscriptions.list({ customer: customerId, status: "all", limit: 100 })) {
    const productIds = await subscriptionProductIds(stripe, subscription.id);
    if (productIds.length === 0) {
      // Nothing resolvable on a listed subscription is not the same as no
      // subscription at all: it could be a foreign product this lookup
      // simply failed to identify.
      unresolved = true;
      continue;
    }
    for (const id of productIds) ids.add(id);
  }
  for await (const invoice of stripe.invoices.list({ customer: customerId, limit: 100 })) {
    const productIds = await invoiceProductIds(stripe, invoice.id);
    if (productIds.length === 0) {
      unresolved = true;
      continue;
    }
    for (const id of productIds) ids.add(id);
  }
  return { productIds: [...ids], unresolved };
}

function productIdOf(product: unknown): string | null {
  if (typeof product === "string" && product.length > 0) return product;
  if (product && typeof product === "object" && "id" in product && typeof (product as { id: unknown }).id === "string") {
    return (product as { id: string }).id;
  }
  return null;
}

function isResourceMissing(error: unknown): boolean {
  return error instanceof Stripe.errors.StripeInvalidRequestError && error.code === "resource_missing";
}

async function subscriptionProductIds(stripe: Stripe, subscriptionId: string): Promise<string[]> {
  try {
    const ids = new Set<string>();
    for await (const item of stripe.subscriptionItems.list({ subscription: subscriptionId, limit: 100 })) {
      const id = productIdOf(item.price?.product);
      if (id) ids.add(id);
    }
    return [...ids];
  } catch (error) {
    if (isResourceMissing(error)) return [];
    throw error;
  }
}

async function sessionProductIds(stripe: Stripe, sessionId: string): Promise<string[]> {
  try {
    const ids = new Set<string>();
    for await (const item of stripe.checkout.sessions.listLineItems(sessionId, { limit: 100 })) {
      const id = productIdOf(item.price?.product);
      if (id) ids.add(id);
    }
    return [...ids];
  } catch (error) {
    if (isResourceMissing(error)) return [];
    throw error;
  }
}

/** Distinct product ids across an invoice's line items, paginated. */
async function invoiceProductIds(stripe: Stripe, invoiceId: string): Promise<string[]> {
  try {
    const ids = new Set<string>();
    for await (const item of stripe.invoices.listLineItems(invoiceId, { limit: 100 })) {
      const id = productIdOf(item.pricing?.price_details?.product);
      if (id) ids.add(id);
    }
    return [...ids];
  } catch (error) {
    if (isResourceMissing(error)) return [];
    throw error;
  }
}

/** A session's own line items plus its Stripe-recorded customer and claimed uid, read fresh from Stripe. */
interface StripeObjectSnapshot {
  productIds: string[];
  customerId: string | null;
  metadataUid: string | null;
  /** True when Stripe confirms the object itself no longer exists (resource_missing on the object, not just its items). */
  missing: boolean;
}

function missingSnapshot(): StripeObjectSnapshot {
  return { productIds: [], customerId: null, metadataUid: null, missing: true };
}

async function subscriptionSnapshot(stripe: Stripe, subscriptionId: string): Promise<StripeObjectSnapshot> {
  let subscription: Stripe.Subscription;
  try {
    subscription = await stripe.subscriptions.retrieve(subscriptionId);
  } catch (error) {
    // Confirmed gone: distinct from an existing object whose ownership could
    // not be resolved. A deletion retry hitting an already-canceled/removed
    // object must be an idempotent no-op, not a thrown ambiguity.
    if (isResourceMissing(error)) return missingSnapshot();
    throw error;
  }
  const productIds = await subscriptionProductIds(stripe, subscriptionId);
  return {
    productIds,
    customerId: typeof subscription.customer === "string" ? subscription.customer : subscription.customer?.id ?? null,
    metadataUid: typeof subscription.metadata?.firebase_uid === "string" ? subscription.metadata.firebase_uid : null,
    missing: false,
  };
}

async function sessionSnapshot(stripe: Stripe, sessionId: string): Promise<StripeObjectSnapshot> {
  let session: Stripe.Checkout.Session;
  try {
    session = await stripe.checkout.sessions.retrieve(sessionId);
  } catch (error) {
    if (isResourceMissing(error)) return missingSnapshot();
    throw error;
  }
  const productIds = await sessionProductIds(stripe, sessionId);
  return {
    productIds,
    customerId: typeof session.customer === "string" ? session.customer : session.customer?.id ?? null,
    metadataUid: session.client_reference_id
      ?? (typeof session.metadata?.firebase_uid === "string" ? session.metadata.firebase_uid : null),
    missing: false,
  };
}

/**
 * The live gateway.
 *
 * `client_reference_id` and `subscription_data.metadata.firebase_uid` are what
 * make every later webhook resolvable to an account without a lookup race: the
 * subscription events carry the uid themselves rather than depending on the
 * checkout event having landed first.
 *
 * `expectedProductId` scopes every newly resolved price to the Kanna Cloud
 * product on the shared Stripe account; a price on any other product resolves
 * to no price at all rather than being handed to a caller. Every session and
 * subscription read is likewise scoped: `productVerdict`/`hasBlockingSubscription`
 * only ever report a Kanna-owned object as adoptable or blocking, so a foreign
 * object on a shared customer can neither be handed back to a caller nor block
 * their purchase, and an ambiguous one is surfaced rather than guessed at.
 */
export function stripeCheckoutGateway(secretKey: string, expectedProductId: string): StripeCheckoutGateway {
  const stripe = new Stripe(secretKey);
  return {
    async createCustomer(input) {
      const customer = await stripe.customers.create({
        ...(input.email ? { email: input.email } : {}),
        metadata: { firebase_uid: input.uid },
      }, { idempotencyKey: input.idempotencyKey });
      return { id: customer.id };
    },
    async resolvePriceId(lookupKey) {
      const prices = await stripe.prices.list({ active: true, lookup_keys: [lookupKey], limit: 1 });
      const price = prices.data[0];
      if (!price) return null;
      return productIdOf(price.product) === expectedProductId ? price.id : null;
    },
    async createCheckoutSession(input) {
      const session = await stripe.checkout.sessions.create({
        mode: "subscription",
        customer: input.customerId,
        client_reference_id: input.uid,
        line_items: [{ price: input.priceId, quantity: 1 }],
        success_url: input.successUrl,
        cancel_url: input.cancelUrl,
        expires_at: input.expiresAt,
        payment_method_options: {
          card: { request_three_d_secure: "any" },
        },
        subscription_data: { metadata: { firebase_uid: input.uid } },
        metadata: { firebase_uid: input.uid },
      }, { idempotencyKey: input.idempotencyKey });
      return { id: session.id, url: session.url };
    },
    async retrieveCheckoutSession(sessionId) {
      const session = await stripe.checkout.sessions.retrieve(sessionId, { expand: ["subscription"] });
      const productIds = await sessionProductIds(stripe, sessionId);
      return checkoutSessionState(session, classifyProductOwnership(productIds, expectedProductId));
    },
    async listOpenCheckoutSessions(customerId) {
      const sessions: StripeCheckoutSessionState[] = [];
      for await (const session of stripe.checkout.sessions.list({ customer: customerId, status: "open", limit: 100 })) {
        const productIds = await sessionProductIds(stripe, session.id);
        sessions.push(checkoutSessionState(session, classifyProductOwnership(productIds, expectedProductId)));
      }
      return sessions;
    },
    async hasBlockingSubscription(customerId) {
      let sawAmbiguous = false;
      for await (const subscription of stripe.subscriptions.list({ customer: customerId, status: "all", limit: 100 })) {
        if (subscription.status === "canceled" || subscription.status === "incomplete_expired") continue;
        const verdict = classifyProductOwnership(await subscriptionProductIds(stripe, subscription.id), expectedProductId);
        if (verdict === "owned") return "blocked";
        if (verdict === "ambiguous") sawAmbiguous = true;
      }
      return sawAmbiguous ? "ambiguous" : "clear";
    },
    async closeCheckoutSession(sessionId) {
      await closeStripeCheckoutSession(stripe, sessionId, expectedProductId, null);
    },
  };
}

function checkoutSessionState(session: Stripe.Checkout.Session, productVerdict: ProductOwnershipVerdict): StripeCheckoutSessionState {
  return {
    id: session.id, url: session.url, mode: session.mode,
    uid: session.client_reference_id ?? session.metadata?.firebase_uid ?? null,
    customerId: typeof session.customer === "string" ? session.customer : session.customer?.id ?? null,
    status: session.status,
    subscriptionStatus: typeof session.subscription === "object" ? session.subscription?.status ?? null : null,
    productVerdict,
  };
}

/**
 * Cancels only when the subscription's own line items prove Kanna ownership.
 *
 * A confirmed-gone subscription (`missing`) is a no-op, not a thrown
 * ambiguity: a deletion retry hitting an already-canceled/removed object must
 * stay idempotent. A `context` additionally requires the subscription's own
 * metadata uid (and, when known, its Stripe customer) to match before
 * mutating: a proven-foreign object is left untouched, but a proven-Kanna
 * object that names a different account is an inconsistency, not a foreign
 * object, and must not be silently skipped — it is thrown so the caller's
 * retry contract stays intact.
 */
async function cancelStripeSubscription(
  stripe: Stripe,
  subscriptionId: string,
  expectedProductId: string,
  context: StripeBillingObjectContext | null
): Promise<void> {
  const snapshot = await subscriptionSnapshot(stripe, subscriptionId);
  if (snapshot.missing) return;
  const verdict = classifyProductOwnership(snapshot.productIds, expectedProductId);
  if (verdict === "foreign") return;
  if (verdict === "ambiguous") {
    throw new Error(`Cannot verify Stripe product ownership for subscription ${subscriptionId}`);
  }
  if (context && (snapshot.metadataUid !== context.uid || (context.customerId !== null && snapshot.customerId !== context.customerId))) {
    throw new Error(`Subscription ${subscriptionId} does not match the expected account`);
  }
  try {
    await stripe.subscriptions.cancel(subscriptionId);
  } catch (error) {
    if (isResourceMissing(error)) return;
    throw error;
  }
}

/**
 * Live cancel-at-once gateway used by account deletion.
 *
 * `expectedProductId` and the caller's `context` are both checked before
 * every mutation, including a directly-referenced subscription/session id
 * from Kanna's own records: a shared Stripe customer may also hold Kanji
 * Kongbu's subscriptions and open sessions, or a stale/corrupt reference to
 * another Kanna user's own subscription, and deleting one account must never
 * touch either.
 */
export function stripeSubscriptionGateway(secretKey: string, expectedProductId: string): StripeSubscriptionGateway {
  const stripe = new Stripe(secretKey);
  return {
    async cancelSubscription(subscriptionId, context) {
      await cancelStripeSubscription(stripe, subscriptionId, expectedProductId, context);
    },
    async closeCheckoutSession(sessionId, context) {
      await closeStripeCheckoutSession(stripe, sessionId, expectedProductId, context);
    },
    async closeCustomerBilling(customerId, context) {
      const sessions = stripe.checkout.sessions.list({
        customer: customerId,
        status: "open",
        limit: 100,
      });
      for await (const session of sessions) {
        await closeStripeCheckoutSession(stripe, session.id, expectedProductId, { uid: context.uid, customerId });
      }
      for await (const subscription of stripe.subscriptions.list({
        customer: customerId,
        status: "all",
        limit: 100,
      })) {
        if (subscription.status !== "canceled" && subscription.status !== "incomplete_expired") {
          await cancelStripeSubscription(stripe, subscription.id, expectedProductId, { uid: context.uid, customerId });
        }
      }
    },
  };
}

/**
 * Closes/expires only when the session's own line items prove Kanna
 * ownership, and (when `context` is given) that its claimed uid/customer
 * match. See `cancelStripeSubscription` for the foreign/ambiguous/inconsistent
 * split.
 */
async function closeStripeCheckoutSession(
  stripe: Stripe,
  sessionId: string,
  expectedProductId: string,
  context: StripeBillingObjectContext | null
): Promise<void> {
  const snapshot = await sessionSnapshot(stripe, sessionId);
  if (snapshot.missing) return;
  const verdict = classifyProductOwnership(snapshot.productIds, expectedProductId);
  if (verdict === "foreign") return;
  if (verdict === "ambiguous") {
    throw new Error(`Cannot verify Stripe product ownership for checkout session ${sessionId}`);
  }
  if (context && (snapshot.metadataUid !== context.uid || (context.customerId !== null && snapshot.customerId !== context.customerId))) {
    throw new Error(`Checkout session ${sessionId} does not match the expected account`);
  }
  try {
    const session = await stripe.checkout.sessions.retrieve(sessionId);
    if (session.status === "open") {
      await stripe.checkout.sessions.expire(sessionId);
      return;
    }
    const subscriptionId = typeof session.subscription === "string"
      ? session.subscription
      : session.subscription?.id;
    if (subscriptionId) {
      await cancelStripeSubscription(stripe, subscriptionId, expectedProductId, context);
    }
  } catch (error) {
    // Closing an already-closed or removed session is idempotent. A completed
    // session is handled above by canceling the subscription it created.
    if (isResourceMissing(error)) return;
    throw error;
  }
}
