import { describe, expect, it, vi } from "vitest";
import {
  requireBillingReviewCredentials,
  runBillingReviewJourney,
  type BillingReviewElement,
  type BillingReviewUi
} from "./billing-review.e2e";

interface FakeElementState {
  exists?: boolean;
  text?: string;
  enabled?: boolean;
}

function fakeElement(
  state: FakeElementState,
  clicks: string[],
  name: string
): BillingReviewElement {
  return {
    click: async () => { clicks.push(name); },
    getAttribute: async (attribute) =>
      attribute === "enabled" && state.enabled !== undefined ? String(state.enabled) : null,
    getText: async () => state.text ?? "",
    isExisting: async () => state.exists !== false,
    setValue: async () => undefined,
    waitForDisplayed: async () => undefined
  };
}

function createUi(input: {
  signedInAtOpen: boolean;
  signInSucceeds?: boolean;
  states: Partial<Record<string, FakeElementState>>;
}) {
  const clicks: string[] = [];
  const typed: Record<string, string> = {};
  const screenshots: string[] = [];
  let signedIn = input.signedInAtOpen;
  const element = (name: string, defaults: FakeElementState = {}) =>
    fakeElement({ ...defaults, ...(input.states[name] ?? {}) }, clicks, name);
  const missing = { exists: false };
  const ui: BillingReviewUi = {
    getAppShell: async () => element("shell"),
    getAccountButton: async () => element("account"),
    getAccountSheet: async () => element("sheet"),
    getSignOutButton: async () => element("sign-out", { exists: signedIn }),
    getEmailInput: async () => ({
      ...element("email"),
      setValue: async (value: string) => { typed.email = value; }
    }),
    getPasswordInput: async () => ({
      ...element("password"),
      setValue: async (value: string) => { typed.password = value; }
    }),
    getSignInButton: async () => ({
      ...element("sign-in"),
      click: async () => {
        clicks.push("sign-in");
        signedIn = input.signInSucceeds !== false;
      }
    }),
    getVerificationState: async () => element("verification", missing),
    getSubscriptionState: async () => element("subscription", missing),
    getEntitledState: async () => element("entitled", missing),
    getBillingCard: async () => element("card", missing),
    getBillingUnconfirmed: async () => element("unconfirmed", missing),
    getBillingSource: async (source) => element(`source:${source}`, missing),
    getBillingPrice: async () => element("price", missing),
    getBillingSubscribeButton: async () => element("subscribe", missing),
    getBillingRestoreButton: async () => element("restore", missing),
    getBillingEulaLink: async () => element("eula", missing),
    getBillingPrivacyLink: async () => element("privacy", missing),
    getBillingMessage: async () => element("message", missing),
    captureScreenshot: async (path) => { screenshots.push(path); },
    waitUntil: async (condition, options) => {
      for (let attempt = 0; attempt < 3; attempt += 1) {
        if (await condition()) return true;
      }
      throw new Error(options.timeoutMsg);
    }
  };
  return { ui, clicks, typed, screenshots };
}

const credentials = { email: "review@example.com", password: "review-password" };
const screenshotPath = "/repo/.tmp/app-review/billing.png";

describe("billing review capture", () => {
  it("signs the reviewer in, reads the real card, captures once, and never purchases or restores", async () => {
    const { ui, clicks, typed, screenshots } = createUi({
      signedInAtOpen: false,
      states: {
        card: { exists: true },
        price: { exists: true, text: "CA$5.00 per month" },
        subscribe: { exists: true, enabled: true },
        restore: { exists: true, enabled: true },
        eula: { exists: true },
        privacy: { exists: true }
      }
    });

    const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

    expect(typed).toEqual(credentials);
    expect(clicks).toEqual(["account", "sign-in"]);
    expect(screenshots).toEqual([screenshotPath]);
    expect(report).toEqual({
      signedIn: true,
      emailVerified: true,
      accountState: "apple-billing",
      cardRendered: true,
      billingConfirmed: true,
      billingSources: [],
      price: "CA$5.00 per month",
      priceAvailable: true,
      subscribeEnabled: true,
      restoreEnabled: true,
      eulaPresent: true,
      privacyPresent: true,
      message: null,
      screenshotPath,
      ready: true,
      blockers: []
    });
  });

  it("reports a denied production billing read and a missing storefront price as blockers, with the screenshot still captured", async () => {
    const { ui, screenshots } = createUi({
      signedInAtOpen: true,
      states: {
        card: { exists: true },
        unconfirmed: { exists: true },
        restore: { exists: true, enabled: false },
        eula: { exists: true },
        privacy: { exists: true }
      }
    });

    const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

    expect(screenshots).toEqual([screenshotPath]);
    expect(report.ready).toBe(false);
    expect(report.billingConfirmed).toBe(false);
    expect(report.price).toBeNull();
    expect(report.blockers).toEqual(["billing-read-unconfirmed", "restore-disabled"]);

    const priceless = createUi({
      signedInAtOpen: true,
      states: {
        card: { exists: true },
        price: { exists: true, text: "Monthly price unavailable" },
        subscribe: { exists: true, enabled: false },
        restore: { exists: true, enabled: true },
        eula: { exists: true },
        privacy: { exists: true },
        message: { exists: true, text: "Subscriptions are unavailable in this storefront. You can still restore purchases." }
      }
    });
    const pricelessReport = await runBillingReviewJourney(priceless.ui, { credentials, screenshotPath });
    expect(pricelessReport.priceAvailable).toBe(false);
    expect(pricelessReport.message).toContain("unavailable in this storefront");
    expect(pricelessReport.blockers).toEqual(["storefront-price-unavailable"]);
  });

  it("treats an already-covered account as review-ready without a purchase path", async () => {
    const { ui } = createUi({
      signedInAtOpen: true,
      states: {
        card: { exists: true },
        "source:app_store": { exists: true, text: "Apple App Store · active" },
        restore: { exists: true, enabled: true },
        eula: { exists: true },
        privacy: { exists: true }
      }
    });

    const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });
    expect(report.billingSources).toEqual(["app_store"]);
    expect(report.subscribeEnabled).toBeNull();
    expect(report.ready).toBe(true);
  });

  it("names an unverified reviewer account and a failed sign-in instead of a fake card state", async () => {
    const unverified = createUi({
      signedInAtOpen: true,
      states: { verification: { exists: true } }
    });
    const unverifiedReport = await runBillingReviewJourney(unverified.ui, { credentials, screenshotPath });
    expect(unverifiedReport.accountState).toBe("verification");
    expect(unverifiedReport.emailVerified).toBe(false);
    expect(unverifiedReport.cardRendered).toBe(false);
    expect(unverifiedReport.blockers).toEqual(["reviewer-email-unverified", "billing-card-not-rendered"]);

    const failed = createUi({ signedInAtOpen: false, signInSucceeds: false, states: {} });
    const failedReport = await runBillingReviewJourney(failed.ui, { credentials, screenshotPath });
    expect(failedReport.signedIn).toBe(false);
    expect(failedReport.blockers).toEqual(["reviewer-sign-in-failed"]);
    expect(failed.screenshots).toEqual([screenshotPath]);
  });

  it("refuses to run without the reviewer credential selectors", () => {
    expect(() => requireBillingReviewCredentials({ email: "x@example.com" })).toThrow(
      "KANNA_E2E_CLOUD_EMAIL and KANNA_E2E_CLOUD_PASSWORD"
    );
    expect(requireBillingReviewCredentials(credentials)).toEqual(credentials);
    expect(vi.isMockFunction(requireBillingReviewCredentials)).toBe(false);
  });
});
