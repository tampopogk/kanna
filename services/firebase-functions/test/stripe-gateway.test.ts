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
  subscriptionItemsList: vi.fn(),
  portalCreate: vi.fn(),
}));

vi.mock("stripe", () => {
  class MockStripe {
    billingPortal = { sessions: { create: stripeMocks.portalCreate } };
    customers = { create: stripeMocks.customersCreate };
    subscriptions = { list: stripeMocks.subscriptionsList, cancel: stripeMocks.subscriptionsCancel };
    subscriptionItems = { list: stripeMocks.subscriptionItemsList };
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

beforeEach(() => {
  vi.clearAllMocks();
  stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([]));
  stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([]));
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

  it("retrieves session ownership and current expanded subscription status", async () => {
    stripeMocks.sessionsRetrieve.mockResolvedValue({
      id: "cs_owned", url: null, mode: "subscription", customer: { id: "cus_owned" }, client_reference_id: "owner",
      status: "complete", subscription: { id: "sub_owned", status: "incomplete" },
    });
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).retrieveCheckoutSession("cs_owned")).resolves.toEqual({
      id: "cs_owned", url: null, mode: "subscription", customerId: "cus_owned", uid: "owner", status: "complete", subscriptionStatus: "incomplete",
    });
    expect(stripeMocks.sessionsRetrieve).toHaveBeenCalledWith("cs_owned", { expand: ["subscription"] });
  });

  it("pages only the mapped customer's open sessions and preserves ownership", async () => {
    stripeMocks.sessionsList.mockImplementation(() => asyncIterable([
      { id: "cs_first", url: "https://checkout.example.test/first", mode: "subscription",
        status: "open", customer: "cus_owned", client_reference_id: "owner" },
      { id: "cs_later_page", url: "https://checkout.example.test/later", mode: "payment",
        status: "open", customer: "cus_owned", metadata: { firebase_uid: "other" } },
    ]));
    const sessions = await stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).listOpenCheckoutSessions("cus_owned");
    expect(sessions).toMatchObject([{ id: "cs_first", uid: "owner" }, { id: "cs_later_page", uid: "other", mode: "payment" }]);
    expect(stripeMocks.sessionsList).toHaveBeenCalledWith({ customer: "cus_owned", status: "open", limit: 100 });
  });

  it.each(["active", "trialing", "past_due", "unpaid", "incomplete", "paused"])("blocks %s billing even after terminal subscriptions on earlier pages", async (status) => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([
      { status: "canceled" },
      { status: "incomplete_expired" },
      { status },
    ]));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).hasBlockingSubscription("cus_owned")).resolves.toBe(true);
    expect(stripeMocks.subscriptionsList).toHaveBeenCalledWith({ customer: "cus_owned", status: "all", limit: 100 });
  });

  it("permits replacement only when all subscriptions are terminal", async () => {
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([
      { status: "canceled" },
      { status: "incomplete_expired" },
    ]));
    await expect(stripeCheckoutGateway("sk_test_mocked", PRODUCT_ID).hasBlockingSubscription("cus_owned")).resolves.toBe(false);
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
    await expect(stripeOwnershipLookupGateway("sk_test_mocked").customerProductIds("cus_shared"))
      .resolves.toEqual(expect.arrayContaining([PRODUCT_ID, OTHER_PRODUCT_ID]));
  });
});

describe("Stripe subscription gateway ownership gating", () => {
  it("cancels a subscription only when its items prove it belongs to the expected product", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: PRODUCT_ID } }]));
    await stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_owned");
    expect(stripeMocks.subscriptionsCancel).toHaveBeenCalledWith("sub_owned");
  });

  it("never cancels a subscription that belongs to another product on the shared account", async () => {
    stripeMocks.subscriptionItemsList.mockImplementation(() => asyncIterable([{ price: { product: OTHER_PRODUCT_ID } }]));
    await stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).cancelSubscription("sub_kongbu");
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalled();
  });

  it("never closes a checkout session with mixed or unresolved line items", async () => {
    stripeMocks.sessionsListLineItems.mockImplementation(() => asyncIterable([
      { price: { product: PRODUCT_ID } },
      { price: { product: OTHER_PRODUCT_ID } },
    ]));
    await stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).closeCheckoutSession("cs_mixed");
    expect(stripeMocks.sessionsRetrieve).not.toHaveBeenCalled();
    expect(stripeMocks.sessionsExpire).not.toHaveBeenCalled();
  });

  it("touches only Kanna-owned subscriptions and sessions on a shared customer during closeCustomerBilling", async () => {
    stripeMocks.sessionsList.mockImplementation(() => asyncIterable([
      { id: "cs_kanna", status: "open" },
      { id: "cs_kongbu", status: "open" },
    ]));
    stripeMocks.sessionsListLineItems.mockImplementation((id: string) =>
      asyncIterable([{ price: { product: id === "cs_kanna" ? PRODUCT_ID : OTHER_PRODUCT_ID } }]));
    stripeMocks.sessionsRetrieve.mockResolvedValue({ status: "open" });
    stripeMocks.subscriptionsList.mockImplementation(() => asyncIterable([
      { id: "sub_kanna", status: "active" },
      { id: "sub_kongbu", status: "active" },
    ]));
    stripeMocks.subscriptionItemsList.mockImplementation(({ subscription }: { subscription: string }) =>
      asyncIterable([{ price: { product: subscription === "sub_kanna" ? PRODUCT_ID : OTHER_PRODUCT_ID } }]));

    await stripeSubscriptionGateway("sk_test_mocked", PRODUCT_ID).closeCustomerBilling("cus_shared");

    expect(stripeMocks.sessionsExpire).toHaveBeenCalledExactlyOnceWith("cs_kanna");
    expect(stripeMocks.sessionsExpire).not.toHaveBeenCalledWith("cs_kongbu");
    expect(stripeMocks.subscriptionsCancel).toHaveBeenCalledExactlyOnceWith("sub_kanna");
    expect(stripeMocks.subscriptionsCancel).not.toHaveBeenCalledWith("sub_kongbu");
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
