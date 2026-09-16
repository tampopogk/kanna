/**
 * The narrow slice of the Stripe API the billing backend calls.
 *
 * Kept behind an interface so the emulator tests exercise the real
 * `createCheckoutSession` logic — its guards, its customer reuse, the metadata
 * it stamps — without making live Stripe calls in CI.
 */
import Stripe from "stripe";
import { classifyProductOwnership } from "./stripeOwnership.js";

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
}

export interface StripeCheckoutGateway {
  createCustomer(input: StripeCustomerInput): Promise<{ id: string }>;
  resolvePriceId(lookupKey: string): Promise<string | null>;
  createCheckoutSession(input: StripeCheckoutSessionInput): Promise<StripeCheckoutSession>;
  retrieveCheckoutSession(sessionId: string): Promise<StripeCheckoutSessionState>;
  listOpenCheckoutSessions(customerId: string): Promise<StripeCheckoutSessionState[]>;
  hasBlockingSubscription(customerId: string): Promise<boolean>;
  closeCheckoutSession(sessionId: string): Promise<void>;
}

export interface StripeSubscriptionGateway {
  cancelSubscription(subscriptionId: string): Promise<void>;
  closeCheckoutSession(sessionId: string): Promise<void>;
  closeCustomerBilling(customerId: string): Promise<void>;
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
  /** Distinct product ids across every non-deleted subscription a customer holds. */
  customerProductIds(customerId: string): Promise<string[]>;
}

export function stripeOwnershipLookupGateway(secretKey: string): StripeOwnershipLookupGateway {
  const stripe = new Stripe(secretKey);
  return {
    subscriptionProductIds: (subscriptionId) => subscriptionProductIds(stripe, subscriptionId),
    sessionProductIds: (sessionId) => sessionProductIds(stripe, sessionId),
    async customerProductIds(customerId) {
      const ids = new Set<string>();
      for await (const subscription of stripe.subscriptions.list({ customer: customerId, status: "all", limit: 100 })) {
        for (const id of await subscriptionProductIds(stripe, subscription.id)) ids.add(id);
      }
      return [...ids];
    },
  };
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
 * to no price at all rather than being handed to a caller.
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
      return checkoutSessionState(session);
    },
    async listOpenCheckoutSessions(customerId) {
      const sessions: StripeCheckoutSessionState[] = [];
      for await (const session of stripe.checkout.sessions.list({ customer: customerId, status: "open", limit: 100 })) {
        sessions.push(checkoutSessionState(session));
      }
      return sessions;
    },
    async hasBlockingSubscription(customerId) {
      for await (const subscription of stripe.subscriptions.list({ customer: customerId, status: "all", limit: 100 })) {
        if (subscription.status !== "canceled" && subscription.status !== "incomplete_expired") return true;
      }
      return false;
    },
    async closeCheckoutSession(sessionId) {
      await closeStripeCheckoutSession(stripe, sessionId, expectedProductId);
    },
  };
}

function checkoutSessionState(session: Stripe.Checkout.Session): StripeCheckoutSessionState {
  return {
    id: session.id, url: session.url, mode: session.mode,
    uid: session.client_reference_id ?? session.metadata?.firebase_uid ?? null,
    customerId: typeof session.customer === "string" ? session.customer : session.customer?.id ?? null,
    status: session.status,
    subscriptionStatus: typeof session.subscription === "object" ? session.subscription?.status ?? null : null,
  };
}

/** Cancels only when the subscription's own line items prove Kanna ownership. */
async function cancelStripeSubscription(stripe: Stripe, subscriptionId: string, expectedProductId: string): Promise<void> {
  const verdict = classifyProductOwnership(await subscriptionProductIds(stripe, subscriptionId), expectedProductId);
  if (verdict !== "owned") return;
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
 * `expectedProductId` is checked before every mutation, including a
 * directly-referenced subscription/session id from Kanna's own records: a
 * shared Stripe customer may also hold Kanji Kongbu's subscriptions and open
 * sessions, and deleting a Kanna account must never touch them.
 */
export function stripeSubscriptionGateway(secretKey: string, expectedProductId: string): StripeSubscriptionGateway {
  const stripe = new Stripe(secretKey);
  return {
    async cancelSubscription(subscriptionId) {
      await cancelStripeSubscription(stripe, subscriptionId, expectedProductId);
    },
    async closeCheckoutSession(sessionId) {
      await closeStripeCheckoutSession(stripe, sessionId, expectedProductId);
    },
    async closeCustomerBilling(customerId) {
      const sessions = stripe.checkout.sessions.list({
        customer: customerId,
        status: "open",
        limit: 100,
      });
      for await (const session of sessions) {
        await closeStripeCheckoutSession(stripe, session.id, expectedProductId);
      }
      for await (const subscription of stripe.subscriptions.list({
        customer: customerId,
        status: "all",
        limit: 100,
      })) {
        if (subscription.status !== "canceled" && subscription.status !== "incomplete_expired") {
          await cancelStripeSubscription(stripe, subscription.id, expectedProductId);
        }
      }
    },
  };
}

/** Closes/expires only when the session's own line items prove Kanna ownership. */
async function closeStripeCheckoutSession(stripe: Stripe, sessionId: string, expectedProductId: string): Promise<void> {
  const verdict = classifyProductOwnership(await sessionProductIds(stripe, sessionId), expectedProductId);
  if (verdict !== "owned") return;
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
      await cancelStripeSubscription(stripe, subscriptionId, expectedProductId);
    }
  } catch (error) {
    // Closing an already-closed or removed session is idempotent. A completed
    // session is handled above by canceling the subscription it created.
    if (isResourceMissing(error)) return;
    throw error;
  }
}
