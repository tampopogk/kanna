import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { Firestore } from "firebase-admin/firestore";
import { createPortalSession } from "../src/billing/portal.js";
import { accountDeletionPath, billingSourcePath, stripeCustomerPath, userDocPath } from "../src/billing/types.js";
import { clearFirestoreEmulator, emulatorFirestore, hasFirestoreEmulator, shutdownEmulatorFirestore } from "./support/emulator.js";

const KANNA_PRODUCT_ID = "prod_test_kanna_cloud";
const env = {
  STRIPE_SECRET_KEY: "sk_test_injected", KANNA_PORTAL_BASE_URL: "https://staging.example.test/",
  STRIPE_PORTAL_CONFIGURATION_ID: "bpc_test", STRIPE_PRODUCT_ID: KANNA_PRODUCT_ID,
};
const gateway = { createPortalSession: vi.fn(async () => ({ url: "https://billing.stripe.test/owned" })) };
const ownershipGateway = { customerProductIds: vi.fn(async () => [KANNA_PRODUCT_ID]) };

describe.skipIf(!hasFirestoreEmulator)("Customer Portal ownership against Firestore", () => {
  let db: Firestore;
  beforeAll(() => { db = emulatorFirestore(); });
  afterEach(async () => { await clearFirestoreEmulator(); vi.clearAllMocks(); });
  afterAll(shutdownEmulatorFirestore);

  it("requires authentication before reading records or contacting Stripe", async () => {
    await expect(createPortalSession({}, null, { db, env, gateway, ownershipGateway })).rejects.toMatchObject({ code: "unauthenticated" });
    expect(gateway.createPortalSession).not.toHaveBeenCalled();
  });

  it.each([{ customerId: "cus_other" }, { uid: "other" }, { returnUrl: "https://evil.test" }, { configuration: "bpc_other" }, null, []])("rejects client selections: %j", async (request) => {
    await expect(createPortalSession(request, { uid: "owner" }, { db, env, gateway, ownershipGateway })).rejects.toMatchObject({ reason: "invalid_portal_request" });
    expect(gateway.createPortalSession).not.toHaveBeenCalled();
  });

  it.each(["profile", "billing", "both"])("uses only the caller's %s record and preserves account data", async (where) => {
    const profile = { displayName: "Owner", ...(where !== "billing" ? { stripeCustomerId: "cus_owner" } : {}) };
    await db.doc(userDocPath("owner")).set(profile);
    if (where !== "profile") await db.doc(billingSourcePath("owner", "stripe")).set({ stripeCustomerId: "cus_owner", status: "expired" });
    await db.doc(stripeCustomerPath("cus_owner")).set({ uid: "owner" });
    await db.doc(userDocPath("other")).set({ stripeCustomerId: "cus_other" });
    await expect(createPortalSession({}, { uid: "owner" }, { db, env, gateway, ownershipGateway })).resolves.toEqual({ url: "https://billing.stripe.test/owned" });
    expect(gateway.createPortalSession).toHaveBeenCalledWith({ customerId: "cus_owner", configurationId: "bpc_test", returnUrl: "https://staging.example.test/account" });
    expect((await db.doc(userDocPath("owner")).get()).data()).toEqual(profile);
    expect((await db.doc(accountDeletionPath("owner")).get()).exists).toBe(false);
  });

  it("does not use another account's customer when the caller has none (including comp)", async () => {
    await db.doc(userDocPath("other")).set({ stripeCustomerId: "cus_other" });
    await db.doc(billingSourcePath("owner", "comp")).set({ active: true });
    await expect(createPortalSession({}, { uid: "owner" }, { db, env, gateway, ownershipGateway })).rejects.toMatchObject({ reason: "no_stripe_customer" });
    expect(gateway.createPortalSession).not.toHaveBeenCalled();
  });

  it.each(["mapping", "records"])("refuses inconsistent %s ownership", async (kind) => {
    await db.doc(userDocPath("owner")).set({ stripeCustomerId: "cus_owner" });
    if (kind === "mapping") await db.doc(stripeCustomerPath("cus_owner")).set({ uid: "other" });
    else await db.doc(billingSourcePath("owner", "stripe")).set({ stripeCustomerId: "cus_different" });
    await expect(createPortalSession({}, { uid: "owner" }, { db, env, gateway, ownershipGateway })).rejects.toMatchObject({ code: "permission-denied" });
    expect(gateway.createPortalSession).not.toHaveBeenCalled();
  });

  it("refuses a deleted account even while its records remain", async () => {
    await db.doc(userDocPath("owner")).set({ stripeCustomerId: "cus_owner" });
    await db.doc(accountDeletionPath("owner")).set({ status: "deleting" });
    await expect(createPortalSession({}, { uid: "owner" }, { db, env, gateway, ownershipGateway })).rejects.toMatchObject({ reason: "account_deleted" });
    expect(gateway.createPortalSession).not.toHaveBeenCalled();
  });

  it.each(Object.keys(env))("fails safely with missing %s", async (key) => {
    await db.doc(userDocPath("owner")).set({ stripeCustomerId: "cus_owner" });
    await expect(createPortalSession({}, { uid: "owner" }, { db, env: { ...env, [key]: "" }, gateway, ownershipGateway })).rejects.toMatchObject({ reason: "not_configured" });
    expect(gateway.createPortalSession).not.toHaveBeenCalled();
  });

  it("reports a retryable gateway failure without exposing provider internals", async () => {
    await db.doc(userDocPath("owner")).set({ stripeCustomerId: "cus_owner" });
    const failing = { createPortalSession: vi.fn(async () => { throw new Error("private provider error"); }) };
    await expect(createPortalSession({}, { uid: "owner" }, { db, env, gateway: failing, ownershipGateway })).rejects.toMatchObject({ reason: "stripe_error", message: "Could not open billing management. Please try again." });
  });

  it("refuses a Portal session for a customer that also holds another product on the shared account", async () => {
    await db.doc(userDocPath("owner")).set({ stripeCustomerId: "cus_shared" });
    const mixed = { customerProductIds: vi.fn(async () => [KANNA_PRODUCT_ID, "prod_kanji_kongbu"]) };
    await expect(createPortalSession({}, { uid: "owner" }, { db, env, gateway, ownershipGateway: mixed }))
      .rejects.toMatchObject({ code: "permission-denied", reason: "customer_ownership_mismatch" });
    expect(gateway.createPortalSession).not.toHaveBeenCalled();
  });

  it("allows a customer with no billing at all (comp, or a canceled subscription)", async () => {
    await db.doc(userDocPath("owner")).set({ stripeCustomerId: "cus_owner" });
    const clean = { customerProductIds: vi.fn(async () => []) };
    await expect(createPortalSession({}, { uid: "owner" }, { db, env, gateway, ownershipGateway: clean }))
      .resolves.toEqual({ url: "https://billing.stripe.test/owned" });
  });

  it("reports a retryable failure when ownership cannot be verified", async () => {
    await db.doc(userDocPath("owner")).set({ stripeCustomerId: "cus_owner" });
    const failing = { customerProductIds: vi.fn(async () => { throw new Error("private provider error"); }) };
    await expect(createPortalSession({}, { uid: "owner" }, { db, env, gateway, ownershipGateway: failing }))
      .rejects.toMatchObject({ reason: "stripe_error" });
    expect(gateway.createPortalSession).not.toHaveBeenCalled();
  });
});
