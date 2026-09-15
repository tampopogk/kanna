import { readFileSync } from "node:fs";
import { AppStoreServerAPIClient } from "@apple/app-store-server-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createAppleGateway } from "../src/billing/appStoreGateway.js";
import { appleEnv, appleJws, fixtureVerifier, renewalClaims, transactionClaims } from "./support/appleFixtures.js";

const env = { ...appleEnv, APP_STORE_KEY_ID: "TESTONLY", APP_STORE_ISSUER_ID: "test",
  APP_STORE_PRIVATE_KEY: readFileSync(new URL("./fixtures/apple/leaf.key", import.meta.url), "utf8") };
const verifier = fixtureVerifier();
const gateway = () => createAppleGateway(env, verifier);
const tx = () => verifier.transaction(appleJws(transactionClaims()));
function response(overrides = {}) {
  return { bundleId: "build.kanna.app", environment: "Sandbox", data: [{ subscriptionGroupIdentifier: "12345",
    lastTransactions: [{ originalTransactionId: "100000001", status: 5,
      signedTransactionInfo: appleJws(transactionClaims(undefined, { revocationDate: Date.now() })),
      signedRenewalInfo: appleJws(renewalClaims()) }] }], ...overrides };
}
afterEach(() => vi.restoreAllMocks());
describe("Apple current-status gateway", () => {
  it("verifies provider JWS and returns current revocation instead of the old device purchase", async () => {
    const api = vi.spyOn(AppStoreServerAPIClient.prototype, "getAllSubscriptionStatuses").mockResolvedValue(response());
    const state = await gateway().current(await tx());
    expect(api).toHaveBeenCalledWith("100000001");
    expect(state[0]).toMatchObject({ status: 5, transaction: { revoked: true, environment: "sandbox" } });
  });
  it("rejects mismatched response identity and ownership within signed current status", async () => {
    const api = vi.spyOn(AppStoreServerAPIClient.prototype, "getAllSubscriptionStatuses").mockResolvedValue(response({ bundleId: "another.app" }));
    await expect(gateway().current(await tx())).rejects.toMatchObject({ reason: "invalid_apple_transaction" });
    const foreign = response();
    foreign.data[0]!.lastTransactions[0]!.signedTransactionInfo = appleJws(transactionClaims("33333333-3333-4333-8333-333333333333"));
    api.mockResolvedValue(foreign);
    await expect(gateway().current(await tx())).rejects.toMatchObject({ reason: "invalid_apple_transaction" });
  });
  it("leaves missing current status and provider outages retryable", async () => {
    const api = vi.spyOn(AppStoreServerAPIClient.prototype, "getAllSubscriptionStatuses").mockResolvedValue(response({ data: [] }));
    await expect(gateway().current(await tx())).rejects.toMatchObject({ reason: "apple_retry_required" });
    api.mockRejectedValue(new Error("provider unavailable"));
    await expect(gateway().current(await tx())).rejects.toMatchObject({ reason: "apple_retry_required" });
  });
  it("reads every notification-history page in the explicit scope", async () => {
    const api = vi.spyOn(AppStoreServerAPIClient.prototype, "getNotificationHistory")
      .mockResolvedValueOnce({ hasMore: true, paginationToken: "second", notificationHistory: [{ signedPayload: "first" }] })
      .mockResolvedValueOnce({ hasMore: false, notificationHistory: [{ signedPayload: "second" }] });
    const values = [];
    for await (const payload of gateway().notificationHistory("sandbox", "100000001", 100, 200)) values.push(payload);
    expect(values).toEqual(["first", "second"]);
    expect(api.mock.calls).toEqual([[null, { startDate: 100, endDate: 200, transactionId: "100000001" }],
      ["second", { startDate: 100, endDate: 200, transactionId: "100000001" }]]);
  });
});
