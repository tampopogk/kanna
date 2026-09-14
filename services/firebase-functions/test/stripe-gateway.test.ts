import { beforeEach, describe, expect, it, vi } from "vitest";

const stripeMocks = vi.hoisted(() => ({
  customersCreate: vi.fn(),
  pricesList: vi.fn(),
  sessionsCreate: vi.fn(),
  sessionsRetrieve: vi.fn(),
  sessionsList: vi.fn(),
  subscriptionsList: vi.fn(),
  portalCreate: vi.fn(),
}));

vi.mock("stripe", () => ({
  default: class MockStripe {
    billingPortal = { sessions: { create: stripeMocks.portalCreate } };
    customers = { create: stripeMocks.customersCreate };
    subscriptions = { list: stripeMocks.subscriptionsList };
    prices = { list: stripeMocks.pricesList };
    checkout = {
      sessions: { create: stripeMocks.sessionsCreate, retrieve: stripeMocks.sessionsRetrieve, list: stripeMocks.sessionsList },
    };
  },
}));

import { stripeCheckoutGateway, stripePortalGateway } from "../src/billing/stripeGateway.js";

describe("Stripe checkout gateway", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    stripeMocks.sessionsCreate.mockResolvedValue({
      id: "cs_test_3ds",
      url: "https://checkout.stripe.com/c/pay/cs_test_3ds",
    });
  });

  it("explicitly requests 3D Secure on the Checkout Session", async () => {
    const gateway = stripeCheckoutGateway("sk_test_mocked");

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
    await stripeCheckoutGateway("sk_test_mocked").createCustomer({
      uid: "owner", email: "owner@example.test", idempotencyKey: "customer-attempt",
    });
    expect(stripeMocks.customersCreate).toHaveBeenCalledWith({
      email: "owner@example.test", metadata: { firebase_uid: "owner" },
    }, { idempotencyKey: "customer-attempt" });
  });

  it("retrieves session ownership and current expanded subscription status", async () => {
    stripeMocks.sessionsRetrieve.mockResolvedValue({
      id: "cs_owned", url: null, mode: "subscription", customer: { id: "cus_owned" }, client_reference_id: "owner",
      status: "complete", subscription: { id: "sub_owned", status: "incomplete" },
    });
    await expect(stripeCheckoutGateway("sk_test_mocked").retrieveCheckoutSession("cs_owned")).resolves.toEqual({
      id: "cs_owned", url: null, mode: "subscription", customerId: "cus_owned", uid: "owner", status: "complete", subscriptionStatus: "incomplete",
    });
    expect(stripeMocks.sessionsRetrieve).toHaveBeenCalledWith("cs_owned", { expand: ["subscription"] });
  });

  it("pages only the mapped customer's open sessions and preserves ownership", async () => {
    stripeMocks.sessionsList.mockImplementation(async function* () {
      yield { id: "cs_first", url: "https://checkout.example.test/first", mode: "subscription",
        status: "open", customer: "cus_owned", client_reference_id: "owner" };
      yield { id: "cs_later_page", url: "https://checkout.example.test/later", mode: "payment",
        status: "open", customer: "cus_owned", metadata: { firebase_uid: "other" } };
    });
    const sessions = await stripeCheckoutGateway("sk_test_mocked").listOpenCheckoutSessions("cus_owned");
    expect(sessions).toMatchObject([{ id: "cs_first", uid: "owner" }, { id: "cs_later_page", uid: "other", mode: "payment" }]);
    expect(stripeMocks.sessionsList).toHaveBeenCalledWith({ customer: "cus_owned", status: "open", limit: 100 });
  });

  it.each(["active", "trialing", "past_due", "unpaid", "incomplete", "paused"])("blocks %s billing even after terminal subscriptions on earlier pages", async (status) => {
    stripeMocks.subscriptionsList.mockImplementation(async function* () {
      yield { status: "canceled" };
      yield { status: "incomplete_expired" };
      yield { status };
    });
    await expect(stripeCheckoutGateway("sk_test_mocked").hasBlockingSubscription("cus_owned")).resolves.toBe(true);
    expect(stripeMocks.subscriptionsList).toHaveBeenCalledWith({ customer: "cus_owned", status: "all", limit: 100 });
  });

  it("permits replacement only when all subscriptions are terminal", async () => {
    stripeMocks.subscriptionsList.mockImplementation(async function* () {
      yield { status: "canceled" };
      yield { status: "incomplete_expired" };
    });
    await expect(stripeCheckoutGateway("sk_test_mocked").hasBlockingSubscription("cus_owned")).resolves.toBe(false);
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
