import { beforeEach, describe, expect, it, vi } from "vitest";

const stripeMocks = vi.hoisted(() => ({
  customersCreate: vi.fn(),
  pricesList: vi.fn(),
  sessionsCreate: vi.fn(),
  portalCreate: vi.fn(),
}));

vi.mock("stripe", () => ({
  default: class MockStripe {
    billingPortal = { sessions: { create: stripeMocks.portalCreate } };
    customers = { create: stripeMocks.customersCreate };
    prices = { list: stripeMocks.pricesList };
    checkout = {
      sessions: { create: stripeMocks.sessionsCreate },
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
    }));
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
