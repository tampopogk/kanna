/**
 * The billing backend against the real Firestore emulator.
 *
 * Skipped without `FIRESTORE_EMULATOR_HOST`; run with
 * `./kd emulators exec -- pnpm test`.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { Firestore } from "firebase-admin/firestore";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createCheckoutSession,
  type CheckoutCaller,
  type CreateCheckoutSessionDependencies,
} from "../src/billing/checkout.js";
import { recomputeEntitlement } from "../src/billing/entitlement.js";
import { BillingRequestError } from "../src/billing/errors.js";
import {
  appStoreSource,
  billingFixtureAccounts,
  compSource,
  stripeSource,
  FIXTURE_ENVIRONMENT,
} from "../src/billing/fixtures.js";
import { seedBillingFixtures } from "../src/billing/seed.js";
import type { StripeCheckoutGateway, StripeCheckoutSessionInput, StripeCheckoutSessionState, StripeCustomerInput } from "../src/billing/stripeGateway.js";
import { signStripePayload } from "../src/billing/stripeSignature.js";
import { handleStripeWebhook } from "../src/billing/stripeWebhook.js";
import {
  accountCheckoutPath,
  accountDeletionPath,
  billingSourcePath,
  entitlementPath,
  stripeCustomerPath,
  stripeEventPath,
  userDocPath,
  type BilledSourceState,
  type EntitlementRecord,
} from "../src/billing/types.js";
import { deleteAccount, firestoreAccountDeletionStore } from "../src/accountDeletion.js";
import {
  clearFirestoreEmulator,
  emulatorFirestore,
  hasFirestoreEmulator,
  shutdownEmulatorFirestore,
} from "./support/emulator.js";

const describeWithEmulator = hasFirestoreEmulator ? describe : describe.skip;

const WEBHOOK_SECRET = "whsec_slice1_test_secret";
const CHECKOUT_UID = "fixture-checkout-user";
const FIXTURE_DIR = join(import.meta.dirname, "fixtures/stripe");

const webhookEnv: NodeJS.ProcessEnv = {
  STRIPE_WEBHOOK_SECRET: WEBHOOK_SECRET,
  GCLOUD_PROJECT: "kanna-local",
};

const checkoutEnv: NodeJS.ProcessEnv = {
  STRIPE_SECRET_KEY: "sk_test_slice1",
  KANNA_PORTAL_BASE_URL: "https://portal.kanna.build/",
  GCLOUD_PROJECT: "kanna-local",
};

const silentLogger = { info: () => {}, warn: () => {}, error: () => {} };

function fixtureBody(name: string): string {
  return readFileSync(join(FIXTURE_DIR, name), "utf8");
}

/**
 * Deliver a fixture the way Stripe does: the raw bytes plus a real signature
 * over those exact bytes.
 *
 * The signature is stamped at delivery time, as Stripe's is — the event's own
 * `created` field is when the event happened, and a signature carrying it would
 * fall outside the replay-tolerance window the moment a fixture ages.
 */
async function deliver(
  db: Firestore,
  name: string,
  options: { secret?: string; now?: string } = {}
) {
  const body = fixtureBody(name);
  const signature = signStripePayload(body, options.secret ?? WEBHOOK_SECRET);
  return handleStripeWebhook(
    { rawBody: Buffer.from(body, "utf8"), signature },
    {
      db,
      env: webhookEnv,
      logger: silentLogger,
      ...(options.now ? { now: () => options.now as string } : {}),
    }
  );
}

async function readDoc<T>(db: Firestore, path: string): Promise<T | null> {
  const snapshot = await db.doc(path).get();
  return (snapshot.data() as T | undefined) ?? null;
}

interface StubGateway extends StripeCheckoutGateway {
  calls: { customers: number; sessions: StripeCheckoutSessionInput[]; closedSessions: string[] };
  sessions: Map<string, StripeCheckoutSessionState>;
  customerRequests: StripeCustomerInput[];
  sessionRequests: StripeCheckoutSessionInput[];
}

function stubGateway(): StubGateway {
  const calls = { customers: 0, sessions: [] as StripeCheckoutSessionInput[], closedSessions: [] as string[] };
  const customerKeys = new Map<string, string>();
  const sessionKeys = new Map<string, string>();
  const sessions = new Map<string, StripeCheckoutSessionState>();
  const customerRequests: StripeCustomerInput[] = [];
  const sessionRequests: StripeCheckoutSessionInput[] = [];
  return {
    calls, sessions, customerRequests, sessionRequests,
    async createCustomer(input) {
      customerRequests.push(input);
      let id = customerKeys.get(input.idempotencyKey);
      if (!id) {
        calls.customers += 1;
        id = calls.customers === 1 ? "cus_TestSlice1" : `cus_${calls.customers}`;
        customerKeys.set(input.idempotencyKey, id);
      }
      return { id };
    },
    async resolvePriceId(lookupKey) {
      return `price_for_${lookupKey}`;
    },
    async createCheckoutSession(input) {
      sessionRequests.push(input);
      let id = sessionKeys.get(input.idempotencyKey);
      if (!id) {
        calls.sessions.push(input);
        id = calls.sessions.length === 1 ? "cs_test_TestSlice1" : `cs_test_${calls.sessions.length}`;
        sessionKeys.set(input.idempotencyKey, id);
        sessions.set(id, {
          id, url: `https://checkout.stripe.com/c/pay/${id}`, status: "open", mode: "subscription",
          uid: input.uid, customerId: input.customerId, subscriptionStatus: null,
        });
      }
      const session = sessions.get(id);
      if (!session) throw new Error("unknown fixture session");
      return { id, url: session.url };
    },
    async retrieveCheckoutSession(id) {
      const session = sessions.get(id);
      if (!session) throw new Error("unknown fixture session");
      return { ...session };
    },
    async listOpenCheckoutSessions(customerId) {
      return [...sessions.values()].filter((session) => session.customerId === customerId && session.status === "open").map((session) => ({ ...session }));
    },
    async hasBlockingSubscription(customerId) {
      return [...sessions.values()].some((session) => session.customerId === customerId
        && session.subscriptionStatus && !["canceled", "incomplete_expired"].includes(session.subscriptionStatus));
    },
    async closeCheckoutSession(sessionId) {
      calls.closedSessions.push(sessionId);
      const session = sessions.get(sessionId);
      if (session) sessions.set(sessionId, { ...session, status: "expired", subscriptionStatus: "canceled" });
    },
  };
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolvePromise = (): void => {};
  const promise = new Promise<void>((resolve) => {
    resolvePromise = resolve;
  });
  return { promise, resolve: resolvePromise };
}

describeWithEmulator("billing backend against the Firestore emulator", () => {
  let db: Firestore;

  beforeAll(() => {
    db = emulatorFirestore();
  });

  beforeEach(async () => {
    // A Stripe billing relationship is only valid for an existing account.
    // Webhook application reads this root transactionally so delayed events
    // cannot recreate cloud data after account deletion.
    await db.doc(userDocPath(CHECKOUT_UID)).set({ createdAt: "2026-08-19T00:00:00.000Z" });
  });

  afterEach(async () => {
    await clearFirestoreEmulator();
  });

  afterAll(async () => {
    await shutdownEmulatorFirestore();
  });

  describe("stripeWebhook", () => {
    it("rejects a payload whose signature does not verify", async () => {
      const outcome = await deliver(db, "checkout.session.completed.json", {
        secret: "whsec_a_different_secret",
      });
      expect(outcome).toMatchObject({ httpStatus: 400, code: "invalid_signature" });
      expect(await readDoc(db, billingSourcePath(CHECKOUT_UID, "stripe"))).toBeNull();
    });

    it("rejects a payload with no signature header at all", async () => {
      const body = fixtureBody("checkout.session.completed.json");
      const outcome = await handleStripeWebhook(
        { rawBody: body, signature: undefined },
        { db, env: webhookEnv, logger: silentLogger }
      );
      expect(outcome).toMatchObject({ httpStatus: 400, code: "invalid_signature" });
    });

    it("refuses to run at all when the webhook secret is not configured", async () => {
      const body = fixtureBody("checkout.session.completed.json");
      const outcome = await handleStripeWebhook(
        { rawBody: body, signature: signStripePayload(body, WEBHOOK_SECRET) },
        { db, env: { GCLOUD_PROJECT: "kanna-local" }, logger: silentLogger }
      );
      expect(outcome).toMatchObject({ httpStatus: 500, code: "not_configured" });
      expect(outcome.message).toContain("STRIPE_WEBHOOK_SECRET");
    });

    it("acknowledges an event type it does not handle without recording it", async () => {
      const outcome = await deliver(db, "customer.updated.json");
      expect(outcome).toMatchObject({ httpStatus: 200, code: "ignored" });
      expect(await readDoc(db, stripeEventPath("evt_customer_updated"))).toBeNull();
    });

    it("acknowledges and drops an event that resolves to no account", async () => {
      const body = JSON.parse(fixtureBody("customer.subscription.created.json")) as {
        data: { object: { metadata: Record<string, string> } };
      };
      body.data.object.metadata = {};
      const raw = JSON.stringify(body);
      const outcome = await handleStripeWebhook(
        { rawBody: raw, signature: signStripePayload(raw, WEBHOOK_SECRET) },
        { db, env: webhookEnv, logger: silentLogger }
      );
      expect(outcome).toMatchObject({ httpStatus: 200, code: "unresolved_account" });
    });

    it("resolves the account through the customer reverse map when metadata is absent", async () => {
      await db
        .doc(stripeCustomerPath("cus_TestSlice1"))
        .set({ uid: CHECKOUT_UID, updatedAt: "2026-08-19T00:00:00.000Z" });

      const body = JSON.parse(fixtureBody("customer.subscription.created.json")) as {
        data: { object: { metadata: Record<string, string> } };
      };
      body.data.object.metadata = {};
      const raw = JSON.stringify(body);
      const outcome = await handleStripeWebhook(
        { rawBody: raw, signature: signStripePayload(raw, WEBHOOK_SECRET) },
        { db, env: webhookEnv, logger: silentLogger }
      );
      expect(outcome).toMatchObject({ code: "applied", uid: CHECKOUT_UID });
    });

    it("processes a duplicate event id exactly once", async () => {
      const first = await deliver(db, "customer.subscription.created.json", {
        now: "2026-08-19T00:00:05.000Z",
      });
      expect(first).toMatchObject({ code: "applied", entitlementWritten: true });

      const sourceBefore = await readDoc<BilledSourceState>(
        db,
        billingSourcePath(CHECKOUT_UID, "stripe")
      );
      const entitlementBefore = await readDoc<EntitlementRecord>(db, entitlementPath(CHECKOUT_UID));

      const replay = await deliver(db, "customer.subscription.created.json", {
        now: "2026-08-19T00:09:09.000Z",
      });
      expect(replay).toMatchObject({ code: "duplicate", entitlementWritten: false });

      expect(await readDoc(db, billingSourcePath(CHECKOUT_UID, "stripe"))).toEqual(sourceBefore);
      expect(await readDoc(db, entitlementPath(CHECKOUT_UID))).toEqual(entitlementBefore);
    });

    it("does not let a delayed cancellation webhook write after deletion starts", async () => {
      await db.doc(accountDeletionPath(CHECKOUT_UID)).set({ uid: CHECKOUT_UID, started: true });

      const result = await deliver(db, "customer.subscription.deleted.json");

      expect(result).toMatchObject({ code: "deleted_account", uid: CHECKOUT_UID });
      expect(await readDoc(db, stripeEventPath("evt_subscription_deleted"))).toBeNull();
      expect(await readDoc(db, billingSourcePath(CHECKOUT_UID, "stripe"))).toBeNull();
      expect(await readDoc(db, entitlementPath(CHECKOUT_UID))).toBeNull();
    });

    it("never lets a late older event walk back newer state", async () => {
      await deliver(db, "customer.subscription.deleted.json");
      expect(
        (await readDoc<BilledSourceState>(db, billingSourcePath(CHECKOUT_UID, "stripe")))?.status
      ).toBe("expired");

      const stale = await deliver(db, "customer.subscription.created.json");
      expect(stale.code).toBe("stale");
      expect(
        (await readDoc<BilledSourceState>(db, billingSourcePath(CHECKOUT_UID, "stripe")))?.status
      ).toBe("expired");
    });

    it("still records a stale event so it is never reprocessed", async () => {
      await deliver(db, "customer.subscription.deleted.json");
      await deliver(db, "customer.subscription.created.json");
      expect(await readDoc(db, stripeEventPath("evt_subscription_created"))).toMatchObject({
        uid: CHECKOUT_UID,
        type: "customer.subscription.created",
      });
    });

    it("carries a checkout session through to a granted entitlement", async () => {
      const checkout = await deliver(db, "checkout.session.completed.json");
      expect(checkout).toMatchObject({ code: "applied", uid: CHECKOUT_UID });

      expect(await readDoc(db, stripeCustomerPath("cus_TestSlice1"))).toMatchObject({
        uid: CHECKOUT_UID,
      });

      const subscription = await deliver(db, "customer.subscription.created.json");
      expect(subscription.code).toBe("applied");

      expect(
        await readDoc<BilledSourceState>(db, billingSourcePath(CHECKOUT_UID, "stripe"))
      ).toMatchObject({
        source: "stripe",
        status: "active",
        currentPeriodEndsAt: "2026-09-19T00:00:00.000Z",
        stripeCustomerId: "cus_TestSlice1",
        stripeSubscriptionId: "sub_TestSlice1",
      });

      expect(await readDoc<EntitlementRecord>(db, entitlementPath(CHECKOUT_UID))).toMatchObject({
        status: "active",
        source: "stripe",
        duplicateSources: false,
        currentPeriodEndsAt: "2026-09-19T00:00:00.000Z",
        capabilities: ["cloud_relay", "cloud_task_index", "remote_task_control"],
      });
    });

    it("writes only the stripe source doc, never the entitlement doc directly", async () => {
      await deliver(db, "checkout.session.completed.json");
      await deliver(db, "customer.subscription.created.json");

      const billing = await db.collection(`users/${CHECKOUT_UID}/billing`).get();
      expect(billing.docs.map((doc) => doc.id)).toEqual(["stripe"]);
    });

    it("walks the full dunning lifecycle into and back out of grace", async () => {
      await deliver(db, "customer.subscription.created.json");
      await deliver(db, "invoice.payment_failed.json");

      expect(await readDoc<EntitlementRecord>(db, entitlementPath(CHECKOUT_UID))).toMatchObject({
        status: "grace",
        source: "stripe",
        graceEndsAt: "2026-09-26T00:00:00.000Z",
      });

      await deliver(db, "customer.subscription.updated.unpaid.json");
      expect(await readDoc<EntitlementRecord>(db, entitlementPath(CHECKOUT_UID))).toMatchObject({
        status: "expired",
        capabilities: [],
      });
    });

    it("renews the period from a paid invoice", async () => {
      await deliver(db, "customer.subscription.created.json");
      await deliver(db, "invoice.paid.json");
      expect(await readDoc<EntitlementRecord>(db, entitlementPath(CHECKOUT_UID))).toMatchObject({
        status: "active",
        currentPeriodEndsAt: "2026-10-19T00:00:00.000Z",
      });
    });

    it("leaves a comped account entitled no matter what Stripe says", async () => {
      await db.doc(billingSourcePath(CHECKOUT_UID, "comp")).set(compSource());
      await deliver(db, "customer.subscription.deleted.json");

      expect(await readDoc<EntitlementRecord>(db, entitlementPath(CHECKOUT_UID))).toMatchObject({
        status: "active",
        source: "comp",
        duplicateSources: false,
      });
    });

    it("does not let a canceled Stripe subscription erase a live Apple one", async () => {
      await db.doc(billingSourcePath(CHECKOUT_UID, "app_store")).set(appStoreSource());
      await deliver(db, "customer.subscription.deleted.json");

      expect(await readDoc<EntitlementRecord>(db, entitlementPath(CHECKOUT_UID))).toMatchObject({
        status: "active",
        source: "app_store",
      });
    });
  });

  describe("createCheckoutSession", () => {
    const verified: CheckoutCaller = {
      uid: CHECKOUT_UID,
      email: "checkout@example.com",
      emailVerified: true,
    };

    function deps(gateway: StripeCheckoutGateway): CreateCheckoutSessionDependencies {
      return { db, env: checkoutEnv, gateway, logger: silentLogger };
    }

    async function expectRefusal(
      caller: CheckoutCaller | null,
      reason: string
    ): Promise<BillingRequestError> {
      const gateway = stubGateway();
      const error = await createCheckoutSession({ plan: "monthly" }, caller, deps(gateway)).then(
        () => null,
        (thrown: unknown) => thrown
      );
      expect(error).toBeInstanceOf(BillingRequestError);
      const billingError = error as BillingRequestError;
      expect(billingError.reason).toBe(reason);
      expect(gateway.calls.sessions).toHaveLength(0);
      return billingError;
    }

    it("refuses an anonymous caller", async () => {
      const error = await expectRefusal(null, "sign_in_required");
      expect(error.code).toBe("unauthenticated");
    });

    it("refuses an unverified email address before Stripe is ever called", async () => {
      await expectRefusal({ ...verified, emailVerified: false }, "email_verification_required");
    });

    it("refuses a tombstoned account before creating Stripe or Firestore state", async () => {
      await db.doc(accountDeletionPath(CHECKOUT_UID)).set({
        uid: CHECKOUT_UID,
        started: true,
      });
      const before = await readDoc<Record<string, unknown>>(db, userDocPath(CHECKOUT_UID));
      const gateway = stubGateway();

      await expect(
        createCheckoutSession({ plan: "monthly" }, verified, deps(gateway)),
      ).rejects.toMatchObject({
        code: "failed-precondition",
        reason: "account_deleted",
      });

      expect(gateway.calls.customers).toBe(0);
      expect(gateway.calls.sessions).toHaveLength(0);
      expect(await readDoc(db, userDocPath(CHECKOUT_UID))).toEqual(before);
      expect((await db.collection("stripeCustomers").where("uid", "==", CHECKOUT_UID).get()).empty)
        .toBe(true);
    });

    it("refuses an unknown plan", async () => {
      const gateway = stubGateway();
      await expect(
        createCheckoutSession({ plan: "lifetime" }, verified, deps(gateway))
      ).rejects.toMatchObject({ reason: "unknown_plan" });
    });

    it("never lets a comped account pay", async () => {
      await db.doc(billingSourcePath(CHECKOUT_UID, "comp")).set(compSource());
      await expectRefusal(verified, "comp_active");
    });

    it("sends an App Store subscriber to Apple with a distinct reason", async () => {
      await db.doc(billingSourcePath(CHECKOUT_UID, "app_store")).set(appStoreSource());
      const error = await expectRefusal(verified, "app_store_active");
      expect(error.message).toContain("App Store");
    });

    it("refuses a second subscription while the first is active", async () => {
      await db.doc(billingSourcePath(CHECKOUT_UID, "stripe")).set(stripeSource());
      await expectRefusal(verified, "already_subscribed");
    });

    it("refuses while a Stripe subscription is in grace", async () => {
      await db
        .doc(billingSourcePath(CHECKOUT_UID, "stripe"))
        .set(stripeSource({ status: "grace" }));
      await expectRefusal(verified, "already_subscribed");
    });

    it("allows a new checkout once the previous subscription expired", async () => {
      await db
        .doc(billingSourcePath(CHECKOUT_UID, "stripe"))
        .set(stripeSource({ status: "expired" }));
      const gateway = stubGateway();
      await expect(
        createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))
      ).resolves.toMatchObject({ sessionId: "cs_test_TestSlice1" });
    });

    it("reports a clear configuration error rather than calling Stripe with no key", async () => {
      const gateway = stubGateway();
      await expect(
        createCheckoutSession({ plan: "monthly" }, verified, {
          db,
          env: { GCLOUD_PROJECT: "kanna-local" },
          gateway,
          logger: silentLogger,
        })
      ).rejects.toMatchObject({ reason: "not_configured" });
      expect(gateway.calls.customers).toBe(0);
    });

    it("creates a customer stamped with the uid and records the reverse map", async () => {
      const gateway = stubGateway();
      const result = await createCheckoutSession({ plan: "monthly", uid: "victim", customerId: "cus_victim" }, verified, deps(gateway));

      expect(result).toMatchObject({
        sessionId: "cs_test_TestSlice1",
        customerId: "cus_TestSlice1",
      });
      expect(gateway.calls.sessions[0]).toMatchObject({
        uid: CHECKOUT_UID,
        priceId: "price_for_cloud_monthly",
        successUrl: "https://portal.kanna.build/billing/success?session_id={CHECKOUT_SESSION_ID}",
        cancelUrl: "https://portal.kanna.build/billing/canceled",
      });
      expect(await readDoc(db, userDocPath(CHECKOUT_UID))).toMatchObject({
        stripeCustomerId: "cus_TestSlice1",
      });
      expect(await readDoc(db, stripeCustomerPath("cus_TestSlice1"))).toMatchObject({
        uid: CHECKOUT_UID,
      });
    });

    it("rejects the unpriced annual plan", async () => {
      const gateway = stubGateway();
      await expect(createCheckoutSession({ plan: "annual" }, verified, deps(gateway)))
        .rejects.toMatchObject({ reason: "unknown_plan" });
    });

    it("resolves the multi-currency price by its stable lookup key", async () => {
      const gateway = stubGateway();
      await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      expect(gateway.calls.sessions[0]).toMatchObject({ priceId: "price_for_cloud_monthly" });
    });

    it("reuses the account's existing Stripe customer", async () => {
      const gateway = stubGateway();
      await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      expect(gateway.calls.customers).toBe(1);
    });

    it("gives repeated tabs and concurrent callers only one payable session", async () => {
      const gateway = stubGateway();
      const results = await Promise.all(Array.from({ length: 4 }, () =>
        createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))));
      const again = await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      expect(new Set([...results, again].map((result) => result.sessionId)).size).toBe(1);
      expect(gateway.calls.customers).toBe(1);
      expect(gateway.calls.sessions).toHaveLength(1);
      expect([...gateway.sessions.values()].filter((session) => session.status === "open")).toHaveLength(1);
      expect(await readDoc(db, accountCheckoutPath(CHECKOUT_UID))).toMatchObject({
        creating: false, sessionIds: [again.sessionId], attempt: { sessionId: again.sessionId },
      });
    });

    it("blocks a completed session before its delayed webhook, even when payment is still pending", async () => {
      const gateway = stubGateway();
      const first = await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      const session = await gateway.retrieveCheckoutSession(first.sessionId);
      gateway.sessions.set(first.sessionId, { ...session, status: "complete", subscriptionStatus: "incomplete", url: null });
      expect(await readDoc(db, billingSourcePath(CHECKOUT_UID, "stripe"))).toBeNull();
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway)))
        .rejects.toMatchObject({ reason: "already_subscribed" });
      await deliver(db, "checkout.session.completed.json");
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway)))
        .rejects.toMatchObject({ reason: "already_subscribed" });
      expect(gateway.calls.sessions).toHaveLength(1);
      expect(gateway.calls.closedSessions).toEqual([]);
    });

    it.each(["expired", "canceled"])("replaces only a remotely retired %s session under concurrent retry", async (retired) => {
      const gateway = stubGateway();
      const first = await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      const session = await gateway.retrieveCheckoutSession(first.sessionId);
      gateway.sessions.set(first.sessionId, { ...session,
        status: retired === "expired" ? "expired" : "complete",
        subscriptionStatus: retired === "canceled" ? "canceled" : null,
      });
      const results = await Promise.all(Array.from({ length: 3 }, () =>
        createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))));
      expect(new Set(results.map((result) => result.sessionId)).size).toBe(1);
      expect(results[0]?.sessionId).not.toBe(first.sessionId);
      expect(gateway.calls.sessions).toHaveLength(2);
      expect(gateway.calls.customers).toBe(1);
      expect(gateway.calls.closedSessions).toEqual([]);
    });

    it("recovers an uncertain customer response with the original key and frozen email", async () => {
      const gateway = stubGateway();
      const create = gateway.createCustomer.bind(gateway);
      gateway.createCustomer = vi.fn(async (input) => {
        await create(input);
        throw new Error("connection lost after customer creation");
      });
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "stripe_error" });
      await expect(firestoreAccountDeletionStore(db).markAccountDeletionStarted(CHECKOUT_UID))
        .rejects.toMatchObject({ reason: "checkout_in_progress" });
      gateway.createCustomer = create;
      await createCheckoutSession({ plan: "monthly" }, { ...verified, email: "changed@example.test" }, deps(gateway));
      expect(gateway.calls.customers).toBe(1);
      expect(gateway.customerRequests).toHaveLength(2);
      expect(gateway.customerRequests[0]).toEqual(gateway.customerRequests[1]);
      expect(gateway.calls.sessions).toHaveLength(1);
    });

    it("recovers an uncertain session response after a restart without changing price, URLs or key", async () => {
      const gateway = stubGateway();
      const create = gateway.createCheckoutSession.bind(gateway);
      gateway.createCheckoutSession = vi.fn(async (input) => {
        await create(input);
        throw new Error("process interrupted before recording response");
      });
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "stripe_error" });
      expect(await readDoc(db, accountCheckoutPath(CHECKOUT_UID))).toMatchObject({ creating: true });
      gateway.createCheckoutSession = create;
      gateway.resolvePriceId = vi.fn(async () => "price_changed");
      await createCheckoutSession({ plan: "monthly" }, verified, {
        ...deps(gateway), env: { ...checkoutEnv, KANNA_PORTAL_BASE_URL: "https://new.example.test" },
      });
      expect(gateway.resolvePriceId).not.toHaveBeenCalled();
      expect(gateway.sessionRequests).toHaveLength(2);
      expect(gateway.sessionRequests[0]).toEqual(gateway.sessionRequests[1]);
      expect(gateway.calls.customers).toBe(1);
      expect(gateway.calls.sessions).toHaveLength(1);
    });

    it("replays the same session after a failed Firestore response commit", async () => {
      const gateway = stubGateway();
      const create = gateway.createCheckoutSession.bind(gateway);
      gateway.createCheckoutSession = async (input) => {
        const session = await create(input);
        vi.spyOn(db, "runTransaction").mockRejectedValueOnce(new Error("commit unavailable"));
        return session;
      };
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "stripe_error" });
      vi.restoreAllMocks();
      gateway.createCheckoutSession = create;
      await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      expect(gateway.calls.sessions).toHaveLength(1);
      expect(gateway.sessionRequests[0]).toEqual(gateway.sessionRequests[1]);
      expect(gateway.calls.closedSessions).toEqual([]);
    });

    it("resumes an invocation interrupted before the external session call", async () => {
      const gateway = stubGateway();
      const create = gateway.createCheckoutSession.bind(gateway);
      gateway.createCheckoutSession = async () => { throw new Error("invocation interrupted"); };
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "stripe_error" });
      expect(gateway.calls.sessions).toHaveLength(0);
      gateway.createCheckoutSession = create;
      await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      expect(gateway.calls.sessions).toHaveLength(1);
      expect(gateway.calls.customers).toBe(1);
    });

    it("recovers the durable session even if its webhook activated access before the lost response", async () => {
      const gateway = stubGateway();
      const create = gateway.createCheckoutSession.bind(gateway);
      gateway.createCheckoutSession = async (input) => {
        await create(input);
        throw new Error("lost response");
      };
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "stripe_error" });
      await deliver(db, "checkout.session.completed.json");
      gateway.createCheckoutSession = create;
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "already_subscribed" });
      expect(await readDoc(db, accountCheckoutPath(CHECKOUT_UID))).toMatchObject({ creating: false, sessionIds: ["cs_test_TestSlice1"] });
      await expect(firestoreAccountDeletionStore(db).markAccountDeletionStarted(CHECKOUT_UID)).resolves.toEqual(["cs_test_TestSlice1"]);
      expect(gateway.calls.sessions).toHaveLength(1);
    });

    it.each(["customer", "session"])("requires reconciliation for an old uncertain %s, never rotates its key", async (operation) => {
      const gateway = stubGateway();
      const startedAt = "2026-09-14T00:00:00.000Z";
      const broken = { ...gateway,
        ...(operation === "customer" ? { createCustomer: async () => { throw new Error("unknown"); } }
          : { createCheckoutSession: async () => { throw new Error("unknown"); } }),
      };
      await expect(createCheckoutSession({ plan: "monthly" }, verified, { ...deps(broken), now: () => startedAt })).rejects.toMatchObject({ reason: "stripe_error" });
      const before = await readDoc(db, accountCheckoutPath(CHECKOUT_UID));
      await expect(createCheckoutSession({ plan: "monthly" }, verified, { ...deps(gateway), now: () => "2026-09-15T00:00:00.000Z" }))
        .rejects.toMatchObject({ reason: "checkout_reconciliation_required" });
      expect(await readDoc(db, accountCheckoutPath(CHECKOUT_UID))).toEqual(before);
      expect(gateway.calls.sessions).toHaveLength(0);
      expect(gateway.calls.customers).toBe(operation === "customer" ? 0 : 1);
    });

    it("adopts an outstanding legacy session and refuses ambiguous or foreign legacy records", async () => {
      const gateway = stubGateway();
      const first = await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      const legacy = { creating: false, sessionIds: [first.sessionId] };
      await db.doc(accountCheckoutPath(CHECKOUT_UID)).set(legacy);
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).resolves.toEqual(first);
      const session = await gateway.retrieveCheckoutSession(first.sessionId);
      gateway.sessions.set("cs_other", { ...session, id: "cs_other", uid: "other-user" });
      await db.doc(accountCheckoutPath(CHECKOUT_UID)).set({ ...legacy, sessionIds: [first.sessionId, "cs_other"] });
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "checkout_reconciliation_required" });
      gateway.sessions.set("cs_other", { ...session, id: "cs_other" });
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "checkout_reconciliation_required" });
      expect(gateway.calls.sessions).toHaveLength(1);
      expect(gateway.calls.closedSessions).toEqual([]);
      await db.doc(accountCheckoutPath(CHECKOUT_UID)).set({ creating: true, sessionIds: [] });
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "checkout_reconciliation_required" });
    });

    it("finds a legacy open session missing from the ledger without creating or expiring another", async () => {
      const gateway = stubGateway();
      const first = await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      await db.doc(accountCheckoutPath(CHECKOUT_UID)).delete();
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).resolves.toEqual(first);
      expect(gateway.calls.sessions).toHaveLength(1);
      expect(gateway.calls.closedSessions).toEqual([]);
    });

    it("refuses an open legacy session beside an existing subscription even before its webhook", async () => {
      const gateway = stubGateway();
      await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      await db.doc(accountCheckoutPath(CHECKOUT_UID)).delete();
      gateway.hasBlockingSubscription = async () => true;
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "already_subscribed" });
      expect(gateway.calls.sessions).toHaveLength(1);
      expect(gateway.calls.closedSessions).toEqual([]);
    });

    it("never reuses or expires an unrelated checkout on the mapped customer", async () => {
      const gateway = stubGateway();
      await db.doc(userDocPath(CHECKOUT_UID)).set({ stripeCustomerId: "cus_TestSlice1" });
      gateway.sessions.set("cs_unrelated", {
        id: "cs_unrelated", customerId: "cus_TestSlice1", uid: "different-user",
        status: "open", mode: "payment", url: "https://checkout.example.test/unrelated", subscriptionStatus: null,
      });
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "checkout_reconciliation_required" });
      expect(gateway.calls.sessions).toHaveLength(0);
      expect(gateway.calls.closedSessions).toEqual([]);
    });

    it("does not confuse an expired entitlement with a subscription that can still collect payment", async () => {
      const gateway = stubGateway();
      await db.doc(billingSourcePath(CHECKOUT_UID, "stripe")).set(stripeSource({ status: "expired" }));
      gateway.hasBlockingSubscription = async () => true;
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "already_subscribed" });
      expect(gateway.calls.sessions).toHaveLength(0);
      await expect(firestoreAccountDeletionStore(db).markAccountDeletionStarted(CHECKOUT_UID)).resolves.toEqual([]);
    });

    it("rechecks comp and deletion transactionally after read-only price resolution", async () => {
      const gateway = stubGateway();
      gateway.resolvePriceId = async () => {
        await db.doc(billingSourcePath(CHECKOUT_UID, "comp")).set(compSource());
        return "price_for_cloud_monthly";
      };
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "comp_active" });
      expect(gateway.calls.sessions).toHaveLength(0);
      await db.doc(billingSourcePath(CHECKOUT_UID, "comp")).delete();
      gateway.resolvePriceId = async () => {
        await firestoreAccountDeletionStore(db).markAccountDeletionStarted(CHECKOUT_UID);
        return "price_for_cloud_monthly";
      };
      await expect(createCheckoutSession({ plan: "monthly" }, verified, deps(gateway))).rejects.toMatchObject({ reason: "account_deleted" });
      expect(gateway.calls.sessions).toHaveLength(0);
    });

    it("writes no billing source doc merely because checkout was opened", async () => {
      const gateway = stubGateway();
      await createCheckoutSession({ plan: "monthly" }, verified, deps(gateway));
      const billing = await db.collection(`users/${CHECKOUT_UID}/billing`).get();
      expect(billing.empty).toBe(true);
      expect(await readDoc(db, entitlementPath(CHECKOUT_UID))).toBeNull();
    });
  });

  describe("seeded fixtures", () => {
    it("derives the documented entitlement for every fixture account", async () => {
      await seedBillingFixtures(db, "2026-08-20T00:00:00.000Z");

      for (const account of billingFixtureAccounts) {
        const entitlement = await readDoc<EntitlementRecord>(db, entitlementPath(account.uid));
        if (!account.expected) {
          expect(entitlement, account.uid).toBeNull();
          continue;
        }
        expect(entitlement, account.uid).toMatchObject({
          status: account.expected.status,
          source: account.expected.source,
          duplicateSources: account.expected.duplicateSources,
          currentPeriodEndsAt: account.expected.currentPeriodEndsAt,
        });
      }
    });

    it("leaves the entitlement doc untouched when a recompute changes nothing", async () => {
      await db.doc(billingSourcePath("fixture-idempotent", "stripe")).set(stripeSource());
      const first = await recomputeEntitlement({
        db,
        uid: "fixture-idempotent",
        defaultEnvironment: FIXTURE_ENVIRONMENT,
        now: "2026-08-20T00:00:00.000Z",
      });
      expect(first.written).toBe(true);

      const second = await recomputeEntitlement({
        db,
        uid: "fixture-idempotent",
        defaultEnvironment: FIXTURE_ENVIRONMENT,
        now: "2026-08-21T00:00:00.000Z",
      });
      expect(second.written).toBe(false);
      expect(
        (await readDoc<EntitlementRecord>(db, entitlementPath("fixture-idempotent")))?.updatedAt
      ).toBe("2026-08-20T00:00:00.000Z");
    });
  });

  describe("account deletion", () => {
    it("serializes deletion against an admitted checkout and closes its Stripe session", async () => {
      const uid = CHECKOUT_UID;
      const enteredStripe = deferred();
      const releaseStripe = deferred();
      const usableSessions = new Set<string>();
      const gateway: StripeCheckoutGateway = {
        async retrieveCheckoutSession(id) {
          return { id, url: "https://checkout.stripe.test/cs_racing_delete", status: "open", mode: "subscription",
            uid, customerId: "cus_racing_delete", subscriptionStatus: null };
        },
        async hasBlockingSubscription() { return false; },
        async listOpenCheckoutSessions() { return []; },
        async createCustomer() {
          return { id: "cus_racing_delete" };
        },
        async resolvePriceId() {
          return "price_for_cloud_monthly";
        },
        async createCheckoutSession() {
          enteredStripe.resolve();
          await releaseStripe.promise;
          usableSessions.add("cs_racing_delete");
          return {
            id: "cs_racing_delete",
            url: "https://checkout.stripe.test/cs_racing_delete",
          };
        },
        async closeCheckoutSession(sessionId) {
          usableSessions.delete(sessionId);
        },
      };
      const cancelSubscription = vi.fn(async () => undefined);
      const closeCheckoutSession = vi.fn(async (sessionId: string) => {
        await gateway.closeCheckoutSession(sessionId);
      });
      const closeCustomerBilling = vi.fn(async () => undefined);
      const auth = {
        revokeRefreshTokens: vi.fn(async () => undefined),
        deleteUser: vi.fn(async () => undefined),
      };
      const deletionDependencies = {
        store: firestoreAccountDeletionStore(db),
        stripe: { cancelSubscription, closeCheckoutSession, closeCustomerBilling },
        auth,
      };

      const checkout = createCheckoutSession(
        { plan: "monthly" },
        { uid, email: "checkout@example.com", emailVerified: true },
        { db, env: checkoutEnv, gateway, logger: silentLogger },
      );
      await enteredStripe.promise;

      await expect(deleteAccount({ uid }, deletionDependencies)).rejects.toMatchObject({
        code: "failed-precondition",
        reason: "checkout_in_progress",
      });
      expect(auth.deleteUser).not.toHaveBeenCalled();

      releaseStripe.resolve();
      await expect(checkout).resolves.toMatchObject({ sessionId: "cs_racing_delete" });
      expect(usableSessions).toEqual(new Set(["cs_racing_delete"]));

      await expect(deleteAccount({ uid }, deletionDependencies)).resolves.toEqual({ deleted: true });

      expect(closeCheckoutSession).toHaveBeenCalledWith("cs_racing_delete");
      expect(usableSessions.size).toBe(0);
      expect((await db.doc(userDocPath(uid)).get()).exists).toBe(false);
      expect((await db.collection("stripeCustomers").where("uid", "==", uid).get()).empty)
        .toBe(true);
      expect((await db.doc(accountCheckoutPath(uid)).get()).exists).toBe(false);
      expect((await db.doc(accountDeletionPath(uid)).get()).exists).toBe(true);
      expect(auth.deleteUser).toHaveBeenCalledWith(uid);
    });

    it("a paused duplicate invocation cannot recreate billing after deletion settles the shared session", async () => {
      const gateway = stubGateway();
      const create = gateway.createCheckoutSession.bind(gateway);
      const enteredFirst = deferred();
      const enteredReplay = deferred();
      const releaseFirst = deferred();
      const releaseReplay = deferred();
      let calls = 0;
      gateway.createCheckoutSession = async (input) => {
        calls += 1;
        if (calls === 1) { enteredFirst.resolve(); await releaseFirst.promise; }
        else { enteredReplay.resolve(); await releaseReplay.promise; }
        return create(input);
      };
      const dependencies = { db, env: checkoutEnv, gateway, logger: silentLogger };
      const caller = { uid: CHECKOUT_UID, email: "checkout@example.test", emailVerified: true };
      const first = createCheckoutSession({ plan: "monthly" }, caller, dependencies);
      await enteredFirst.promise;
      const replay = createCheckoutSession({ plan: "monthly" }, caller, dependencies);
      const rejectedReplay = expect(replay).rejects.toMatchObject({ reason: "account_deleted" });
      await enteredReplay.promise;
      releaseFirst.resolve();
      await first;
      await deleteAccount({ uid: CHECKOUT_UID }, {
        store: firestoreAccountDeletionStore(db),
        auth: { revokeRefreshTokens: async () => {}, deleteUser: async () => {} },
        stripe: { cancelSubscription: async () => {}, closeCustomerBilling: async () => {},
          closeCheckoutSession: (id) => gateway.closeCheckoutSession(id) },
      });
      releaseReplay.resolve();
      await rejectedReplay;
      expect(gateway.calls.sessions).toHaveLength(1);
      expect([...gateway.sessions.values()].every((session) => session.status === "expired")).toBe(true);
      expect((await db.doc(userDocPath(CHECKOUT_UID)).get()).exists).toBe(false);
      expect((await db.doc(accountCheckoutPath(CHECKOUT_UID)).get()).exists).toBe(false);
      expect((await db.doc(accountDeletionPath(CHECKOUT_UID)).get()).exists).toBe(true);
    });

    it("removes every uid-owned Firestore record, including nested mirrors and relay pairings", async () => {
      const uid = "fixture-delete-user";
      await Promise.all([
        db.doc(`users/${uid}`).set({ stripeCustomerId: "cus_delete" }),
        db.doc(`users/${uid}/billing/stripe`).set(stripeSource({ stripeSubscriptionId: "sub_delete" })),
        db.doc(`users/${uid}/billing/comp`).set(compSource()),
        db.doc(`users/${uid}/entitlements/cloud_access`).set({ status: "active" }),
        db.doc(`users/${uid}/desktops/desktop-1`).set({ desktopId: "desktop-1" }),
        db.doc(`users/${uid}/desktops/desktop-1/tasks/task-1`).set({ title: "private" }),
        db.doc(`users/${uid}/pushDevices/push-1`).set({ token: "push" }),
        db.doc("stripeCustomers/cus_delete").set({ uid }),
        db.doc("stripeEvents/evt_delete").set({ uid }),
        db.doc("appAccountTokens/token-delete").set({ uid }),
        db.doc("desktopCredentials/desktop-1").set({ uid }),
        db.doc("devices/legacy-delete").set({ userId: uid }),
        db.doc("users/other-user/desktops/desktop-other").set({ desktopId: "desktop-other" }),
      ]);
      const cancelSubscription = vi.fn(async () => undefined);
      const closeCheckoutSession = vi.fn(async () => undefined);
      const closeCustomerBilling = vi.fn(async () => undefined);
      const auth = {
        revokeRefreshTokens: vi.fn(async () => undefined),
        deleteUser: vi.fn(async () => undefined),
      };

      await deleteAccount(
        { uid },
        {
          store: firestoreAccountDeletionStore(db),
          stripe: { cancelSubscription, closeCheckoutSession, closeCustomerBilling },
          auth,
        },
      );

      expect(cancelSubscription).toHaveBeenCalledWith("sub_delete");
      for (const path of [
        `users/${uid}`,
        `users/${uid}/billing/stripe`,
        `users/${uid}/billing/comp`,
        `users/${uid}/entitlements/cloud_access`,
        `users/${uid}/desktops/desktop-1`,
        `users/${uid}/desktops/desktop-1/tasks/task-1`,
        `users/${uid}/pushDevices/push-1`,
        "stripeCustomers/cus_delete",
        "stripeEvents/evt_delete",
        "appAccountTokens/token-delete",
        "desktopCredentials/desktop-1",
        "devices/legacy-delete",
      ]) {
        expect((await db.doc(path).get()).exists, path).toBe(false);
      }
      expect((await db.doc(accountDeletionPath(uid)).get()).data()).toMatchObject({
        uid,
        started: true,
      });
      expect((await db.doc("users/other-user/desktops/desktop-other").get()).exists).toBe(true);
      expect(auth.revokeRefreshTokens).toHaveBeenCalledWith(uid);
      expect(auth.deleteUser).toHaveBeenCalledWith(uid);
    });

    it("closes legacy unrecorded customer billing before a retryable deletion completes", async () => {
      const uid = "fixture-delete-legacy-checkout";
      await Promise.all([
        db.doc(userDocPath(uid)).set({ stripeCustomerId: "cus_legacy_user" }),
        db.doc(billingSourcePath(uid, "stripe")).set(stripeSource({
          stripeCustomerId: "cus_legacy_billing",
          stripeSubscriptionId: "sub_known",
        })),
        db.doc(stripeCustomerPath("cus_legacy_user")).set({ uid }),
        db.doc(stripeCustomerPath("cus_legacy_extra")).set({ uid }),
      ]);
      expect((await db.doc(accountCheckoutPath(uid)).get()).exists).toBe(false);

      const sessionStatus = new Map([
        ["cs_preledger_open", "open"],
        ["cs_preledger_completed", "complete"],
      ]);
      const customerSubscriptions = new Map([
        ["cus_legacy_user", new Set(["sub_from_completed", "sub_other_live"])],
        ["cus_legacy_billing", new Set(["sub_billing_customer"])],
        ["cus_legacy_extra", new Set(["sub_extra_customer"])],
      ]);
      const cancelSubscription = vi.fn(async (subscriptionId: string) => {
        for (const subscriptions of customerSubscriptions.values()) {
          subscriptions.delete(subscriptionId);
        }
      });
      const closeCheckoutSession = vi.fn(async () => undefined);
      const closeCustomerBilling = vi.fn(async (customerId: string) => {
        if (customerId === "cus_legacy_user") {
          sessionStatus.set("cs_preledger_open", "expired");
          sessionStatus.set("cs_preledger_completed", "closed");
        }
        customerSubscriptions.get(customerId)?.clear();
      });
      const baseStore = firestoreAccountDeletionStore(db);
      let failPairingRevocation = true;
      const store = {
        ...baseStore,
        async revokeDesktopPairings(accountUid: string) {
          if (failPairingRevocation) {
            failPairingRevocation = false;
            throw new Error("injected pairing failure");
          }
          await baseStore.revokeDesktopPairings(accountUid);
        },
      };
      const auth = {
        revokeRefreshTokens: vi.fn(async () => undefined),
        deleteUser: vi.fn(async () => undefined),
      };
      const dependencies = {
        store,
        stripe: { cancelSubscription, closeCheckoutSession, closeCustomerBilling },
        auth,
      };

      await expect(deleteAccount({ uid }, dependencies)).rejects.toThrow(
        "injected pairing failure",
      );
      expect(sessionStatus.get("cs_preledger_open")).toBe("expired");
      expect(sessionStatus.get("cs_preledger_completed")).toBe("closed");
      expect([...customerSubscriptions.values()].every((subscriptions) => subscriptions.size === 0))
        .toBe(true);
      expect(closeCustomerBilling.mock.calls.map(([customerId]) => customerId).sort()).toEqual([
        "cus_legacy_billing",
        "cus_legacy_extra",
        "cus_legacy_user",
      ]);
      expect(auth.deleteUser).not.toHaveBeenCalled();

      await expect(deleteAccount({ uid }, dependencies)).resolves.toEqual({ deleted: true });
      expect(closeCustomerBilling).toHaveBeenCalledTimes(6);
      expect((await db.doc(userDocPath(uid)).get()).exists).toBe(false);
      expect((await db.collection("stripeCustomers").where("uid", "==", uid).get()).empty)
        .toBe(true);
      expect(auth.deleteUser).toHaveBeenCalledWith(uid);
    });
  });
});
