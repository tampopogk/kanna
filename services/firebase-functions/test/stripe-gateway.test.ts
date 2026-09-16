import { beforeEach, describe, expect, it, vi } from "vitest";

const stripeMocks = vi.hoisted(() => ({
  customersCreate: vi.fn(),
  pricesList: vi.fn(),
  sessionsCreate: vi.fn(),
  sessionsRetrieve: vi.fn(),
  sessionsList: vi.fn(),
  sessionsExpire: vi.fn(),
  sessionsListLineItems: vi.fn(),
  subscriptionsList: vi.fn(),
  subscriptionsCancel: vi.fn(),
  subscriptionsRetrieve: vi.fn(),
  subscriptionItemsList: vi.fn(),
  invoicesList: vi.fn(),
  invoicesListLineItems: vi.fn(),
  portalCreate: vi.fn(),
}));

vi.mock("stripe", () => {
  class MockStripe {
    billingPortal = { sessions: { create: stripeMocks.portalCreate } };
    customers = { create: stripeMocks.customersCreate };
    subscriptions = {
      list: stripeMocks.subscriptionsList,
      cancel: stripeMocks.subscriptionsCancel,
      retrieve: stripeMocks.subscriptionsRetrieve,
    };
    subscriptionItems = { list: stripeMocks.subscriptionItemsList };
    invoices = { list: stripeMocks.invoicesList, listLineItems: stripeMocks.invoicesListLineItems };
    prices = { list: stripeMocks.pricesList };
    checkout = {
      sessions: {
        create: stripeMocks.sessionsCreate,
        retrieve: stripeMocks.sessionsRetrieve,
        list: stripeMocks.sessionsList,
        expire: stripeMocks.sessionsExpire,
        listLineItems: stripeMocks.sessionsListLineItems,
      },
    };
  }
  class StripeInvalidRequestError extends Error {
    code: string;
    constructor(raw: { code: string }) {
      super(raw.code);
      this.code = raw.code;
    }
  }
  return {
    default: Object.assign(MockStripe, { errors: { StripeInvalidRequestError } }),
  };
});

import Stripe from "stripe";
import {
  stripeCheckoutGateway,
  stripeOwnershipLookupGateway,
  stripePortalGateway,
  stripeSubscriptionGateway,
} from "../src/billing/stripeGateway.js";

const PRODUCT_ID = "prod_kanna_test";
const OTHER_PRODUCT_ID = "prod_kanji_kongbu";

function asyncIterable<T>(items: T[]): AsyncIterable<T> {
  return {
    async *[Symbol.asyncIterator]() {
      for (const item of items) yield item;
    },
  };
}

const CONTEXT = { uid: "owner", customerId: "cus_owned" };

beforeEach(() => {
  vi.clearAllMocks();
  stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([]));
  stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([]));
  stripeMocks.subscriptionsRetrieve.mockResolvedValue({ customer: CONTEXT.customerId, metadata: { firebase_uid: CONTEXT.uid } });
  stripeMocks.sessionsRetrieve.mockResolvedValue({ customer: CONTEXT.customerId, client_reference_id: CONTEXT.uid, status: "open" });
  stripeMocks.invoicesList.mockImplementation(() => asyncIterable([]));
  stripeMocks.invoicesListLineItems.mockImplementation(() => asyncIterable([]));
});

describe("Stripe checkout gateway", () => {
  beforeEach(() => {
    stripeMocks.sessionsCreate.mockResolvedValue({
      id: "cs_test_3ds",
      url: "https://checkout.stripe.com/c/pay/cs_test_3ds",
    });
  });

  it("explicitly requests 3D Secure on the Checkout Session", async () => {
    const gateway = stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID);

    await gateway.createCheckoutSession({
      uid: "firebase-user",
      idempotencyKey: "checkout-attempt",
      expiresAt: 1800000000,
      customerId: "cus_test_3ds",
      priceId: "price_test_jpy",
      successUrl: "https://portal.kanna.build/billing/success",
      cancelUrl: "https://portal.kanna.build/billing/canceled",
    });

    expect(stripeMocks.sessionsCreate).toHaveBeenCalledOnce();
    expect(stripeMocks.sessionsCreate).toHaveBeenCalledWith(expect.objectContaining({
      payment_method_options: {
        card: { request_three_d_secure: "any" },
      },
      expires_at: 1800000000,
    }), { idempotencyKey: "checkout-attempt" });
  });
  it("uses the durable customer idempotency key without putting it in metadata", async () => {
    stripeMocks.customersCreate.mockResolvedValue({ id: "cus_owned" });
    await stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).createCustomer({
      uid: "owner", email: "owner@example.test", idempotencyKey: "customer-attempt",
    });
    expect(stripeMocks.customersCreate).toHaveBeenCalledWith({
      email: "owner@example.test", metadata: { firebase_uid: "owner" },
    }, { idempotencyKey: "customer-attempt" });
  });

  it("resolves a price only when it belongs to the expected Kanna product", async () => {
    stripeMocks.pricesList.mockResolvedValue({ data: [{ id: "price_kanna", product: PRODUCT_ID }] });
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).resolvePriceId("cloud_monthly"))
      .resolves.toBe("price_kanna");
  });

  it("refuses a price that resolves to a different product on the shared account", async () => {
    stripeMocks.pricesList.mockResolvedValue({ data: [{ id: "price_kongbu", product: OTHER_PRODUCT_ID }] });
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).resolvePriceId("cloud_monthly"))
      .resolves.toBeNull();
  });

  it("retrieves session ownership, current expanded subscription status and product verdict", async () => {
    stripeMocks.sessionsRetrieve.mockResolvedValue({
      id: "cs_owned", url: null, mode: "subscription", customer: { id: "cus_owned" }, client_reference_id: "owner",
      status: "complete", subscription: { id: "sub_owned", status: "incomplete" },
    });
    stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).retrieveCheckoutSession("cs_owned")).resolves.toEqual({
      id: "cs_owned", url: null, mode: "subscription", customerId: "cus_owned", uid: "owner", status: "complete",
      subscriptionStatus: "incomplete", productVerdict: "owned",
    });
    expect(stripeMocks.sessionsRetrieve).toHaveBeenCalledWith("cs_owned", { expand: ["subscription"] });
  });

  it("marks a session naming a foreign product as not Kanna's, without changing ownership fields", async () => {
    stripeMocks.sessionsRetrieve.mockResolvedValue({
      id: "cs_kongbu", url: null, mode: "subscription", customer: { id: "cus_shared" }, client_reference_id: "owner",
      status: "open", subscription: null,
    });
    stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([{ price: { product: OTHER_PRODUCT_ID } }]));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).retrieveCheckoutSession("cs_kongbu"))
      .resolves.toMatchObject({ productVerdict: "foreign" });
  });

  it("marks a session with no resolvable line items as ambiguous, never as owned", async () => {
    stripeMocks.sessionsRetrieve.mockResolvedValue({
      id: "cs_unclear", url: null, mode: "subscription", customer: { id: "cus_shared" }, client_reference_id: "owner",
      status: "open", subscription: null,
    });
    stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([]));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).retrieveCheckoutSession("cs_unclear"))
      .resolves.toMatchObject({ productVerdict: "ambiguous" });
  });

  it("pages only the mapped customer's open sessions, each with its own product verdict", async () => {
    stripeMocks.sessionsList.mockImplementation(() => asyncIterable([
      { id: "cs_first", url: "https://checkout.example.test/first", mode: "subscription",
        status: "open", customer: "cus_owned", client_reference_id: "owner" },
      { id: "cs_later_page", url: "https://checkout.example.test/later", mode: "payment",
        status: "open", customer: "cus_owned", metadata: { firebase_uid: "other" } },
    ]));
    stripeMocks.sessionsListLineItems.mockImplementation((id: string) =>
      asyncIterable([{ price: { product: id === "cs_first" ? PRODUCT_ID : OTHER_PRODUCT_ID } }]));
    const sessions = await stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).listOpenCheckoutSessions("cus_owned");
    expect(sessions).toMatchObject([
      { id: "cs_first", uid: "owner", productVerdict: "owned" },
      { id: "cs_later_page", uid: "other", mode: "payment", productVerdict: "foreign" },
    ]);
    expect(stripeMocks.sessionsList).toHaveBeenCalledWith({ customer: "cus_owned", status: "open", limit: 100 });
  });

  it.each(["active", "trialing", "past_due", "unpaid", "incomplete", "paused"])("blocks %s billing on a Kanna-owned subscription, even after terminal subscriptions on earlier pages", async (status) => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([
      { id: "sub_terminal_1", status: "canceled" },
      { id: "sub_terminal_2", status: "incomplete_expired" },
      { id: "sub_live", status },
    ]));
    stripeMocks.subscriptionItemsList.mockImplementation(({ subscription }: { subscription: string }) =>
      asyncIterable(subscription === "sub_live" ? [{ price: { product: PRODUCT_ID } }] : []));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).hasBlockingSubscription("cus_owned")).resolves.toBe("blocked");
    expect(stripeMocks.subscriptionsList).toHaveBeenCalledWith({ customer: "cus_owned", status: "all", limit: 100 });
  });

  it("permits replacement only when all subscriptions are terminal", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([
      { id: "sub_terminal_1", status: "canceled" },
      { id: "sub_terminal_2", status: "incomplete_expired" },
    ]));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).hasBlockingSubscription("cus_owned")).resolves.toBe("clear");
  });

  it("does not block on a live subscription proven to belong to another product on the shared account", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([{ id: "sub_kongbu", status: "active" }]));
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: OTHER_PRODUCT_ID } }]));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).hasBlockingSubscription("cus_shared")).resolves.toBe("clear");
  });

  it("reports ambiguous rather than clear when a live subscription's items cannot be resolved", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([{ id: "sub_unclear", status: "active" }]));
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([]));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).hasBlockingSubscription("cus_shared")).resolves.toBe("ambiguous");
  });
});

describe("Stripe product-ownership lookups", () => {
  it("collects distinct product ids across a subscription's paginated items", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([
      { price: { product: PRODUCT_ID } },
      { price: { product: PRODUCT_ID } },
    ]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").subscriptionProductIds("sub_owned"))
      .resolves.toEqual([PRODUCT_ID]);
    expect(stripeMocks.subscriptionItemsList).toHaveBeenCalledWith({ subscription: "sub_owned", limit: 100 });
  });

  it("reads product ids as plain ids or expanded product objects", async () => {
    stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([
      { price: { product: PRODUCT_ID } },
      { price: { product: { id: OTHER_PRODUCT_ID } } },
    ]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").sessionProductIds("cs_mixed"))
      .resolves.toEqual(expect.arrayContaining([PRODUCT_ID, OTHER_PRODUCT_ID]));
  });

  it("treats a vanished subscription as proving no product", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => {
      throw new Stripe.errors.StripeInvalidRequestError({ code: "resource_missing" } as never);
    });
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").subscriptionProductIds("sub_gone"))
      .resolves.toEqual([]);
  });

  it("unions product ids across every subscription a customer holds", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([{ id: "sub_a" }, { id: "sub_b" }]));
    stripeMocks.subscriptionItemsList.mockImplementation(({ subscription }: { subscription: string }) =>
      asyncIterable([{ price: { product: subscription === "sub_a" ? PRODUCT_ID : OTHER_PRODUCT_ID } }]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_shared"))
      .resolves.toEqual({ productIds: expect.arrayContaining([PRODUCT_ID, OTHER_PRODUCT_ID]), unresolved: false });
  });

  it("marks the scan unresolved when a listed subscription's items cannot be resolved, distinct from a customer with none", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([{ id: "sub_unclear" }]));
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_shared"))
      .resolves.toEqual({ productIds: [], unresolved: true });
  });

  it("marks the scan unresolved when a listed subscription's items lookup hits resource_missing", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([{ id: "sub_gone" }]));
    stripeMocks.subscriptionItemsList.mockImplementation(() => {
      throw new Stripe.errors.StripeInvalidRequestError({ code: "resource_missing" } as never);
    });
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_shared"))
      .resolves.toEqual({ productIds: [], unresolved: true });
  });

  it("reports a genuinely empty customer as clean, not unresolved", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_owner"))
      .resolves.toEqual({ productIds: [], unresolved: false });
  });

  it("includes a foreign one-time invoice with no subscription of its own in the scan", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([]));
    stripeMocks.invoicesList.mockImplementation(() => asyncIterable([{ id: "in_kongbu" }]));
    stripeMocks.invoicesListLineItems.mockImplementation(() => asyncIterable([
      { pricing: { price_details: { product: OTHER_PRODUCT_ID } } },
    ]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_shared"))
      .resolves.toEqual({ productIds: [OTHER_PRODUCT_ID], unresolved: false });
  });

  it("pages every invoice a customer holds, unioning their product ids with any subscription's", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([{ id: "sub_owned" }]));
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    stripeMocks.invoicesList.mockImplementation(() => asyncIterable([{ id: "in_first" }, { id: "in_second" }]));
    stripeMocks.invoicesListLineItems.mockImplementation((id: string) => asyncIterable([
      { pricing: { price_details: { product: id === "in_first" ? PRODUCT_ID : OTHER_PRODUCT_ID } } },
    ]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_shared"))
      .resolves.toEqual({ productIds: expect.arrayContaining([PRODUCT_ID, OTHER_PRODUCT_ID]), unresolved: false });
    expect(stripeMocks.invoicesList).toHaveBeenCalledWith({ customer: "cus_shared", limit: 100 });
  });

  it("marks the scan unresolved when a listed invoice's line items cannot be resolved", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([]));
    stripeMocks.invoicesList.mockImplementation(() => asyncIterable([{ id: "in_unclear" }]));
    stripeMocks.invoicesListLineItems.mockImplementation(() => asyncIterable([]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_shared"))
      .resolves.toEqual({ productIds: [], unresolved: true });
  });

  it("marks the scan unresolved when a listed invoice's line items lookup hits resource_missing", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([]));
    stripeMocks.invoicesList.mockImplementation(() => asyncIterable([{ id: "in_gone" }]));
    stripeMocks.invoicesListLineItems.mockImplementation(() => {
      throw new Stripe.errors.StripeInvalidRequestError({ code: "resource_missing" } as never);
    });
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_shared"))
      .resolves.toEqual({ productIds: [], unresolved: true });
  });

  it("reports a customer with no subscriptions or invoices at all as clean", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([]));
    stripeMocks.invoicesList.mockImplementation(() => asyncIterable([]));
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductScan("cus_owner"))
      .resolves.toEqual({ productIds: [], unresolved: false });
  });
});

describe("Stripe subscription gateway ownership gating", () => {
  it("cancels a subscription only when its items prove it belongs to the expected product and matches its context", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    await stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_owned", CONTEXT);
    expect(stripeMocks.subscriptionsCancel).toHaveBeenCalledWith("sub_owned");
  });

  it("never cancels a subscription that belongs to another product on the shared account", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: OTHER_PRODUCT_ID } }]));
    await stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_kongbu", CONTEXT);
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalled();
  });

  it("throws rather than cancels a Kanna-product subscription belonging to a different account (wrong-user same-product)", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    stripeMocks.subscriptionsRetrieve.mockResolvedValue({ customer: "cus_owned", metadata: { firebase_uid: "someone-else" } });
    await expect(stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_owned", CONTEXT))
      .rejects.toThrow(/does not match the expected account/);
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalled();
  });

  it("throws rather than cancels a Kanna-product subscription belonging to a different Stripe customer", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    stripeMocks.subscriptionsRetrieve.mockResolvedValue({ customer: "cus_different", metadata: { firebase_uid: CONTEXT.uid } });
    await expect(stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_owned", CONTEXT))
      .rejects.toThrow(/does not match the expected account/);
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalled();
  });

  it("throws rather than silently skips an ambiguous (unresolvable) direct subscription reference", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([]));
    await expect(stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_unclear", CONTEXT))
      .rejects.toThrow(/Cannot verify Stripe product ownership/);
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalled();
  });

  it("treats a confirmed-vanished direct subscription reference as an idempotent no-op, not ambiguous", async () => {
    // A deletion retry hitting a subscription Stripe already removed must
    // succeed silently: resource_missing on the object itself confirms it is
    // gone, which is a different fact from an existing object whose items
    // could not be resolved.
    stripeMocks.subscriptionsRetrieve.mockImplementation(() => {
      throw new Stripe.errors.StripeInvalidRequestError({ code: "resource_missing" } as never);
    });
    await expect(stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_gone", CONTEXT))
      .resolves.toBeUndefined();
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalled();
    expect(stripeMocks.subscriptionItemsList).not.toHaveBeenCalled();
  });

  it("cancels without a customer check when no customer is known for the context", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    stripeMocks.subscriptionsRetrieve.mockResolvedValue({ customer: "cus_any", metadata: { firebase_uid: CONTEXT.uid } });
    await stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_owned", { uid: CONTEXT.uid, customerId: null });
    expect(stripeMocks.subscriptionsCancel).toHaveBeenCalledWith("sub_owned");
  });

  it("never closes a checkout session with mixed line items", async () => {
    stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([
      { price: { product: PRODUCT_ID } },
      { price: { product: OTHER_PRODUCT_ID } },
    ]));
    await expect(stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).closeCheckoutSession("cs_mixed", CONTEXT))
      .rejects.toThrow(/Cannot verify Stripe product ownership/);
    expect(stripeMocks.sessionsExpire).not.toHaveBeenCalled();
  });

  it("treats a confirmed-vanished direct checkout session reference as an idempotent no-op, not ambiguous", async () => {
    stripeMocks.sessionsRetrieve.mockImplementation(() => {
      throw new Stripe.errors.StripeInvalidRequestError({ code: "resource_missing" } as never);
    });
    await expect(stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).closeCheckoutSession("cs_gone", CONTEXT))
      .resolves.toBeUndefined();
    expect(stripeMocks.sessionsExpire).not.toHaveBeenCalled();
    expect(stripeMocks.sessionsListLineItems).not.toHaveBeenCalled();
  });

  it("throws rather than closes a Kanna-product session belonging to a different account", async () => {
    stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    stripeMocks.sessionsRetrieve.mockResolvedValue({ customer: CONTEXT.customerId, client_reference_id: "someone-else", status: "open" });
    await expect(stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).closeCheckoutSession("cs_wrong_user", CONTEXT))
      .rejects.toThrow(/does not match the expected account/);
    expect(stripeMocks.sessionsExpire).not.toHaveBeenCalled();
  });

  it("touches only Kanna-owned subscriptions and sessions on a shared customer during closeCustomerBilling", async () => {
    stripeMocks.sessionsList.mockImplementation(() => asyncIterable([
      { id: "cs_kanna", status: "open" },
      { id: "cs_kongbu", status: "open" },
    ]));
    stripeMocks.sessionsListLineItems.mockImplementation((id: string) =>
      asyncIterable([{ price: { product: id === "cs_kanna" ? PRODUCT_ID : OTHER_PRODUCT_ID } }]));
    stripeMocks.sessionsRetrieve.mockImplementation((id: string) =>
      Promise.resolve({ status: "open", customer: "cus_shared", client_reference_id: id === "cs_kanna" ? CONTEXT.uid : "kongbu-user" }));
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([
      { id: "sub_kanna", status: "active" },
      { id: "sub_kongbu", status: "active" },
    ]));
    stripeMocks.subscriptionItemsList.mockImplementation(({ subscription }: { subscription: string }) =>
      asyncIterable([{ price: { product: subscription === "sub_kanna" ? PRODUCT_ID : OTHER_PRODUCT_ID } }]));
    stripeMocks.subscriptionsRetrieve.mockImplementation((id: string) =>
      Promise.resolve({ customer: "cus_shared", metadata: { firebase_uid: id === "sub_kanna" ? CONTEXT.uid : "kongbu-user" } }));

    await stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).closeCustomerBilling("cus_shared", { uid: CONTEXT.uid });

    expect(stripeMocks.sessionsExpire).toHaveBeenCalledExactlyOnceWith("cs_kanna");
    expect(stripeMocks.sessionsExpire).not.toHaveBeenCalledWith("cs_kongbu");
    expect(stripeMocks.subscriptionsCancel).toHaveBeenCalledExactlyOnceWith("sub_kanna");
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalledWith("sub_kongbu");
  });

  it("throws out of closeCustomerBilling on a Kanna-product object belonging to a different uid, rather than skipping it", async () => {
    stripeMocks.sessionsList.mockImplementation(() => asyncIterable([]));
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([{ id: "sub_wrong_user", status: "active" }]));
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    stripeMocks.subscriptionsRetrieve.mockResolvedValue({ customer: "cus_shared", metadata: { firebase_uid: "someone-else" } });

    await expect(stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).closeCustomerBilling("cus_shared", { uid: CONTEXT.uid }))
      .rejects.toThrow(/does not match the expected account/);
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalled();
  });
});

it("creates the hosted Portal with only trusted customer, configured features and return URL", async () => {
  stripeMocks.portalCreate.mockResolvedValue({ url: "https://billing.stripe.test/session" });
  await expect(stripePortalGateway("sk_test_mocked").createPortalSession({
    customerId: "cus_owned", configurationId: "bpc_test", returnUrl: "https://account.example.test/account",
  })).resolves.toEqual({ url: "https://billing.stripe.test/session" });
  expect(stripeMocks.portalCreate).toHaveBeenCalledWith({
    customer: "cus_owned", configuration: "bpc_test", return_url: "https://account.example.test/account",
  });
});
