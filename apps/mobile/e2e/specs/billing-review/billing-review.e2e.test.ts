import { describe, expect, it, vi } from "vitest";
import {
  classifyBillingSource,
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

/**
 * A fake account sheet with a session model: `retainedIdentity` is the
 * E2E-gated identity marker of a session present at open (`undefined` renders
 * no marker, as a build without the marker would), sign-out clears the
 * session unless `signOutSucceeds` is false, and a sign-in that succeeds
 * renders the marker for `identityAfterSignIn`, defaulting to the typed email.
 */
function createUi(input: {
  signedInAtOpen: boolean;
  retainedIdentity?: string;
  identityAfterSignIn?: string;
  signInSucceeds?: boolean;
  signOutSucceeds?: boolean;
  states: Partial<Record<string, FakeElementState>>;
}) {
  const clicks: string[] = [];
  const typed: Record<string, string> = {};
  const screenshots: string[] = [];
  let signedIn = input.signedInAtOpen;
  let identity: string | undefined = input.signedInAtOpen ? input.retainedIdentity : undefined;
  const element = (name: string, defaults: FakeElementState = {}) =>
    fakeElement({ ...defaults, ...(input.states[name] ?? {}) }, clicks, name);
  const missing = { exists: false };
  const ui: BillingReviewUi = {
    getAppShell: async () => element("shell"),
    getAccountButton: async () => element("account"),
    getAccountSheet: async () => element("sheet"),
    getAccountIdentity: async () =>
      element("identity", { exists: signedIn && identity !== undefined, text: identity }),
    getSignOutButton: async () => ({
      ...element("sign-out", { exists: signedIn }),
      click: async () => {
        clicks.push("sign-out");
        if (input.signOutSucceeds !== false) {
          signedIn = false;
          identity = undefined;
        }
      }
    }),
    getEmailInput: async () => ({
      ...element("email", { exists: !signedIn }),
      setValue: async (value: string) => { typed.email = value; }
    }),
    getPasswordInput: async () => ({
      ...element("password", { exists: !signedIn }),
      setValue: async (value: string) => { typed.password = value; }
    }),
    getSignInButton: async () => ({
      ...element("sign-in"),
      click: async () => {
        clicks.push("sign-in");
        signedIn = input.signInSucceeds !== false;
        if (signedIn) identity = input.identityAfterSignIn ?? typed.email;
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

/** The real card with a localized price and an enabled purchase path. */
const purchasableCard = {
  card: { exists: true },
  price: { exists: true, text: "CA$5.00 per month" },
  subscribe: { exists: true, enabled: true },
  restore: { exists: true, enabled: true },
  eula: { exists: true },
  privacy: { exists: true }
};

/** The real card for a blocked account: source rows shown, purchase path hidden. */
const coveredCard = {
  card: { exists: true },
  restore: { exists: true, enabled: true },
  eula: { exists: true },
  privacy: { exists: true }
};

function reportWithoutSecrets(report: unknown): string {
  return JSON.stringify(report);
}

describe("billing review capture", () => {
  it("signs the reviewer in, reads the real card, captures once, and never purchases or restores", async () => {
    const { ui, clicks, typed, screenshots } = createUi({
      signedInAtOpen: false,
      states: purchasableCard
    });

    const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

    expect(typed).toEqual(credentials);
    expect(clicks).toEqual(["account", "sign-in"]);
    expect(screenshots).toEqual([screenshotPath]);
    expect(report).toEqual({
      signedIn: true,
      accountBinding: "fresh-sign-in",
      identityCheck: "matched",
      emailVerified: true,
      accountState: "apple-billing",
      cardRendered: true,
      billingConfirmed: true,
      billingSources: [],
      activeCoverage: false,
      purchasePathRendered: true,
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
    expect(reportWithoutSecrets(report)).not.toContain(credentials.password);
    expect(reportWithoutSecrets(report)).not.toContain(credentials.email);
  });

  describe("binding the session to the selected reviewer account", () => {
    it("keeps a retained session only when its identity marker names the selected account", async () => {
      const { ui, clicks, typed } = createUi({
        signedInAtOpen: true,
        retainedIdentity: "Review@Example.com ",
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(typed).toEqual({});
      expect(clicks).toEqual(["account"]);
      expect(report.accountBinding).toBe("existing-session");
      expect(report.identityCheck).toBe("matched");
      expect(report.ready).toBe(true);
    });

    it("signs a retained session for another account out and signs the selected account in", async () => {
      // The reproduced regression: a ready card under someone else's session
      // used to be credited with zero credentials entered.
      const { ui, clicks, typed } = createUi({
        signedInAtOpen: true,
        retainedIdentity: "someone-else@example.com",
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(clicks).toEqual(["account", "sign-out", "sign-in"]);
      expect(typed).toEqual(credentials);
      expect(report.accountBinding).toBe("reauthenticated");
      expect(report.identityCheck).toBe("matched");
      expect(report.ready).toBe(true);
      expect(reportWithoutSecrets(report)).not.toContain("someone-else");
      expect(reportWithoutSecrets(report)).not.toContain(credentials.email);
    });

    it("re-authenticates a retained session whose identity cannot be read instead of trusting Sign Out", async () => {
      const { ui, clicks, typed } = createUi({
        signedInAtOpen: true,
        retainedIdentity: undefined,
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(clicks).toEqual(["account", "sign-out", "sign-in"]);
      expect(typed).toEqual(credentials);
      expect(report.accountBinding).toBe("reauthenticated");
      expect(report.ready).toBe(true);
    });

    it("refuses readiness when the retained session cannot be signed out", async () => {
      const { ui, clicks, typed, screenshots } = createUi({
        signedInAtOpen: true,
        retainedIdentity: "someone-else@example.com",
        signOutSucceeds: false,
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(clicks).toEqual(["account", "sign-out"]);
      expect(typed).toEqual({});
      expect(report.signedIn).toBe(true);
      expect(report.accountBinding).toBe("unbound");
      expect(report.identityCheck).toBe("mismatched");
      expect(report.cardRendered).toBe(false);
      expect(report.ready).toBe(false);
      expect(report.blockers).toEqual(["reviewer-session-not-replaced"]);
      expect(screenshots).toEqual([screenshotPath]);
    });

    it("refuses readiness when the signed-in identity still differs from the selected account", async () => {
      const { ui } = createUi({
        signedInAtOpen: false,
        identityAfterSignIn: "someone-else@example.com",
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(report.signedIn).toBe(true);
      expect(report.accountBinding).toBe("unbound");
      expect(report.identityCheck).toBe("mismatched");
      expect(report.ready).toBe(false);
      expect(report.blockers).toEqual(["reviewer-account-mismatch"]);
    });

    it("accepts a fresh sign-in on a build without the identity marker, bound by the entered credentials", async () => {
      const { ui, typed } = createUi({
        signedInAtOpen: false,
        identityAfterSignIn: "",
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(typed).toEqual(credentials);
      expect(report.accountBinding).toBe("fresh-sign-in");
      expect(report.identityCheck).toBe("unavailable");
      expect(report.ready).toBe(true);
    });
  });

  it("reports a denied production billing read and a missing storefront price as blockers, with the screenshot still captured", async () => {
    const { ui, screenshots } = createUi({
      signedInAtOpen: false,
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
      signedInAtOpen: false,
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

  describe("coverage follows the card's billing semantics, not source-row presence", () => {
    it("classifies rows exactly as AppleBillingCard renders them", () => {
      expect(classifyBillingSource("comp", "Complimentary access. No purchase needed."))
        .toEqual({ source: "comp", status: null, covering: true });
      expect(classifyBillingSource("app_store", "Apple App Store · active"))
        .toEqual({ source: "app_store", status: "active", covering: true });
      expect(classifyBillingSource("app_store", "Apple App Store · grace (sandbox) · renewal off"))
        .toEqual({ source: "app_store", status: "grace", covering: true });
      expect(classifyBillingSource("app_store", "Apple App Store · expired"))
        .toEqual({ source: "app_store", status: "expired", covering: false });
      expect(classifyBillingSource("stripe", "Managed outside the App Store · revoked"))
        .toEqual({ source: "stripe", status: "revoked", covering: false });
      expect(classifyBillingSource("stripe", "Managed outside the App Store · "))
        .toEqual({ source: "stripe", status: null, covering: false });
    });

    it("does not credit an expired Apple source with no localized price and a disabled Subscribe", async () => {
      // The reproduced regression: this exact card used to report ready with
      // no blockers because a source row existed.
      const { ui } = createUi({
        signedInAtOpen: false,
        states: {
          card: { exists: true },
          "source:app_store": { exists: true, text: "Apple App Store · expired" },
          price: { exists: true, text: "Monthly price unavailable" },
          subscribe: { exists: true, enabled: false },
          restore: { exists: true, enabled: true },
          eula: { exists: true },
          privacy: { exists: true }
        }
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(report.billingSources).toEqual([{ source: "app_store", status: "expired", covering: false }]);
      expect(report.activeCoverage).toBe(false);
      expect(report.purchasePathRendered).toBe(true);
      expect(report.priceAvailable).toBe(false);
      expect(report.subscribeEnabled).toBe(false);
      expect(report.ready).toBe(false);
      expect(report.blockers).toEqual(["storefront-price-unavailable"]);
    });

    it("holds a revoked Stripe source to the purchase path's own requirements", async () => {
      const { ui } = createUi({
        signedInAtOpen: false,
        states: {
          ...purchasableCard,
          "source:stripe": { exists: true, text: "Managed outside the App Store · revoked" },
          subscribe: { exists: true, enabled: false }
        }
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(report.billingSources).toEqual([{ source: "stripe", status: "revoked", covering: false }]);
      expect(report.activeCoverage).toBe(false);
      expect(report.blockers).toEqual(["subscribe-disabled"]);
    });

    it.each([
      { name: "an active Apple subscription", row: "source:app_store", text: "Apple App Store · active", status: "active" },
      { name: "an Apple subscription in its grace period", row: "source:app_store", text: "Apple App Store · grace · renewal off", status: "grace" },
      { name: "an active Stripe subscription", row: "source:stripe", text: "Managed outside the App Store · active", status: "active" },
      { name: "a complimentary grant", row: "source:comp", text: "Complimentary access. No purchase needed.", status: null }
    ])("treats $name as review-ready coverage with the purchase path hidden", async ({ row, text, status }) => {
      const { ui } = createUi({
        signedInAtOpen: false,
        states: { ...coveredCard, [row]: { exists: true, text } }
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(report.billingSources).toEqual([
        { source: row.slice("source:".length), status, covering: true }
      ]);
      expect(report.activeCoverage).toBe(true);
      expect(report.purchasePathRendered).toBe(false);
      expect(report.subscribeEnabled).toBeNull();
      expect(report.price).toBeNull();
      expect(report.ready).toBe(true);
      expect(report.blockers).toEqual([]);
    });

    it("does not credit a covering row that contradicts a rendered purchase path", async () => {
      const { ui } = createUi({
        signedInAtOpen: false,
        states: {
          card: { exists: true },
          "source:app_store": { exists: true, text: "Apple App Store · active" },
          price: { exists: true, text: "Monthly price unavailable" },
          subscribe: { exists: true, enabled: false },
          restore: { exists: true, enabled: true },
          eula: { exists: true },
          privacy: { exists: true }
        }
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(report.activeCoverage).toBe(false);
      expect(report.blockers).toEqual(["storefront-price-unavailable"]);
    });

    it("names a hidden purchase path with no covering source, as an outstanding Apple payment renders", async () => {
      const { ui } = createUi({
        signedInAtOpen: false,
        states: {
          ...coveredCard,
          "source:app_store": { exists: true, text: "Apple App Store · expired" }
        }
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(report.activeCoverage).toBe(false);
      expect(report.purchasePathRendered).toBe(false);
      expect(report.ready).toBe(false);
      expect(report.blockers).toEqual(["purchase-path-missing"]);
    });
  });

  it("names an unverified reviewer account and a failed sign-in instead of a fake card state", async () => {
    const unverified = createUi({
      signedInAtOpen: false,
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
    expect(failedReport.accountBinding).toBe("unbound");
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
