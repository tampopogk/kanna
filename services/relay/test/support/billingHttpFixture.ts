import type { AppleEvidence } from "../../../firebase-functions/src/billing/appStoreVerification.js";
export const appleFixture = { current: [] as AppleEvidence[] };
export const appleGatewayFixture = { createAppleGateway: () => ({
  current: async () => appleFixture.current,
  async *notificationHistory() {},
}) };
/** Local HTTP hosting for the actual exported Functions handlers, never Stripe. */
import express from "express";
import type { StripeCheckoutGateway, StripeCheckoutSessionInput, StripePortalGateway, StripeSubscriptionGateway } from "../../../firebase-functions/src/billing/stripeGateway.js";

export const billingFixture = {
  checkoutInput: null as StripeCheckoutSessionInput | null,
  checkoutCalls: 0,
  portalCustomer: null as string | null,
  canceledSubscriptions: [] as string[],
  closedCustomers: [] as string[],
};

const checkout: StripeCheckoutGateway = {
  async createCustomer() { return { id: "cus_launch_fixture" }; },
  async resolvePriceId(key) {
    if (key !== "cloud_monthly") throw new Error(`Unexpected lookup key: ${key}`);
    return "price_launch_fixture";
  },
  async createCheckoutSession(input) {
    billingFixture.checkoutInput = input;
    billingFixture.checkoutCalls++;
    return { id: "cs_launch_fixture", url: "https://checkout.stripe.test/launch-fixture" };
  },
  async retrieveCheckoutSession(id) {
    if (!billingFixture.checkoutInput || id !== "cs_launch_fixture") throw new Error("Unknown fixture session");
    return {
      id, url: "https://checkout.stripe.test/launch-fixture", mode: "subscription",
      uid: billingFixture.checkoutInput.uid, customerId: billingFixture.checkoutInput.customerId,
      status: "open", subscriptionStatus: null,
    };
  },
  async listOpenCheckoutSessions() { return []; },
  async hasBlockingSubscription() { return false; },
  async closeCheckoutSession() {},
};
const portal: StripePortalGateway = {
  async createPortalSession(input) {
    billingFixture.portalCustomer = input.customerId;
    return { url: "https://billing.stripe.test/launch-fixture" };
  },
};
const subscription: StripeSubscriptionGateway = {
  async cancelSubscription(id) { billingFixture.canceledSubscriptions.push(id); },
  async closeCheckoutSession() {},
  async closeCustomerBilling(id) { billingFixture.closedCustomers.push(id); },
};

// The suite replaces only these external I/O factories. Auth, callable error
// encoding, checkout coordination, webhook signatures and Firestore stay real.
export const billingGateways = {
  stripeCheckoutGateway: () => checkout,
  stripePortalGateway: () => portal,
  stripeSubscriptionGateway: () => subscription,
};

export async function startBillingHttpFixture(port: number) {
  if (!process.env.FIREBASE_AUTH_EMULATOR_HOST?.startsWith("127.0.0.1:")
    || !process.env.FIRESTORE_EMULATOR_HOST?.startsWith("127.0.0.1:")
    || process.env.GCLOUD_PROJECT !== "kanna-local") {
    throw new Error("Billing HTTP fixture requires isolated loopback emulators");
  }
  const functions = await import("../../../firebase-functions/src/index.js");
  const app = express();
  app.use(express.json({ verify(request, _response, bytes) {
    Object.assign(request, { rawBody: Buffer.from(bytes) });
  } }));
  for (const [name, handler] of Object.entries(functions)) {
    app.post(`/${name}`, (request, response) => {
      if (!("rawBody" in request) || !Buffer.isBuffer(request.rawBody)) throw new Error("Missing raw body");
      return handler(Object.assign(request, { rawBody: request.rawBody }), response);
    });
  }
  const server = app.listen(port, "127.0.0.1");
  await new Promise<void>((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  return {
    url: `http://127.0.0.1:${port}`,
    close: () => new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve())),
  };
}
