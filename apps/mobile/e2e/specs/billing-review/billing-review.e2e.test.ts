import { describe, expect, it, vi } from "vitest";
import type { Browser } from "webdriverio";
import {
  classifyBillingSource,
  createBillingReviewUi,
  requireBillingReviewCredentials,
  runBillingReviewJourney,
  type BillingReviewElement,
  type BillingReviewSystemAlert,
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

/** A system alert iOS paints over the fake app; `stuck` ones survive their button tap. */
interface FakeSystemAlert extends BillingReviewSystemAlert {
  stuck?: boolean;
}

/**
 * A fake account sheet with a session model: `retainedIdentity` is the
 * E2E-gated identity marker of a session present at open (`undefined` renders
 * no marker, as a build without the marker would), sign-out clears the
 * session unless `signOutSucceeds` is false, and a sign-in that succeeds
 * renders the marker for `identityAfterSignIn`, defaulting to the typed email.
 * `systemAlerts` are raised, in order, by a successful sign-in — as the real
 * Save Password sheet is — and each tapped button is recorded as
 * `alert:<label>`.
 */
function createUi(input: {
  signedInAtOpen: boolean;
  retainedIdentity?: string;
  identityAfterSignIn?: string;
  signInSucceeds?: boolean;
  signOutSucceeds?: boolean;
  systemAlerts?: FakeSystemAlert[];
  states: Partial<Record<string, FakeElementState>>;
}) {
  const clicks: string[] = [];
  const typed: Record<string, string> = {};
  const screenshots: string[] = [];
  const systemAlerts: FakeSystemAlert[] = [];
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
        if (signedIn) {
          identity = input.identityAfterSignIn ?? typed.email;
          systemAlerts.push(...(input.systemAlerts ?? []));
        }
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
    getSystemAlert: async () => {
      const alert = systemAlerts[0];
      return alert ? { text: alert.text, buttons: [...alert.buttons] } : null;
    },
    tapSystemAlertButton: async (label) => {
      const alert = systemAlerts[0];
      if (!alert || !alert.buttons.includes(label)) {
        throw new Error(`No system alert button labelled ${label}`);
      }
      clicks.push(`alert:${label}`);
      if (!alert.stuck) systemAlerts.shift();
    },
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
      systemAlertsDismissed: [],
      screenshotObscuredBy: null,
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

  describe("system alerts over the capture", () => {
    // The reproduced defect: the sign-in raised iOS's Save Password sheet,
    // the tree-derived report read through it and credited a ready card, and
    // the delivered screenshot showed the sheet over the price and Subscribe.
    const savePasswordSheet: FakeSystemAlert = {
      text: "Save Password?\nSecurely store your password so it's filled automatically the next time you need it.",
      buttons: ["Not Now", "Save"]
    };

    it("declines the Save Password sheet the sign-in raised before capturing, and stays ready", async () => {
      const { ui, clicks, screenshots } = createUi({
        signedInAtOpen: false,
        systemAlerts: [savePasswordSheet],
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(clicks).toEqual(["account", "sign-in", "alert:Not Now"]);
      expect(clicks).not.toContain("alert:Save");
      expect(screenshots).toEqual([screenshotPath]);
      expect(report.systemAlertsDismissed).toEqual([savePasswordSheet.text]);
      expect(report.screenshotObscuredBy).toBeNull();
      expect(report.ready).toBe(true);
      expect(report.blockers).toEqual([]);
    });

    it("clears stacked alerts in order, declining each with its own non-saving button", async () => {
      const notifications: FakeSystemAlert = {
        text: "“Kanna” Would Like to Send You Notifications",
        buttons: ["Don't Allow", "Allow"]
      };
      const { ui, clicks } = createUi({
        signedInAtOpen: false,
        systemAlerts: [savePasswordSheet, notifications],
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(clicks).toEqual(["account", "sign-in", "alert:Not Now", "alert:Don't Allow"]);
      expect(report.systemAlertsDismissed).toEqual([savePasswordSheet.text, notifications.text]);
      expect(report.ready).toBe(true);
    });

    it("fails the capture as obscured when the alert survives its decline", async () => {
      const { ui, clicks, screenshots } = createUi({
        signedInAtOpen: false,
        systemAlerts: [{ ...savePasswordSheet, stuck: true }],
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(clicks).toEqual(["account", "sign-in", "alert:Not Now"]);
      // The capture is still written as evidence, but never credited.
      expect(screenshots).toEqual([screenshotPath]);
      expect(report.systemAlertsDismissed).toEqual([]);
      expect(report.screenshotObscuredBy).toBe(savePasswordSheet.text);
      expect(report.ready).toBe(false);
      expect(report.blockers).toEqual(["screenshot-obscured"]);
    });

    it("never taps a button it does not recognise as a decline, and reports the alert instead", async () => {
      const { ui, clicks } = createUi({
        signedInAtOpen: false,
        systemAlerts: [{ text: "Save Password?", buttons: ["Save"] }],
        states: purchasableCard
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(clicks).toEqual(["account", "sign-in"]);
      expect(report.screenshotObscuredBy).toBe("Save Password?");
      expect(report.ready).toBe(false);
      expect(report.blockers).toEqual(["screenshot-obscured"]);
    });

    it("reports an obscured capture alongside the card's own blockers", async () => {
      const { ui } = createUi({
        signedInAtOpen: false,
        systemAlerts: [{ ...savePasswordSheet, stuck: true }],
        states: { ...purchasableCard, restore: { exists: true, enabled: false } }
      });

      const report = await runBillingReviewJourney(ui, { credentials, screenshotPath });

      expect(report.blockers).toEqual(["restore-disabled", "screenshot-obscured"]);
    });

    describe("reading in-app system prompts through the driver", () => {
      // The Save Password sheet as WebDriver reported it on an iPhone 15
      // simulator (iOS 26.5): an XCUIElementTypeSheet inside the app's own
      // window, not SpringBoard, with its title, body and buttons as
      // descendants. WebDriverAgent's alert search stops at the account
      // sheet's scroll view above it, so getAlertText reports no alert.
      interface FakePromptChild {
        kind: "button" | "text";
        label: string;
      }
      interface FakePrompt {
        type: "XCUIElementTypeSheet" | "XCUIElementTypeAlert";
        label: string;
        children: FakePromptChild[];
      }
      const savePasswordSheet: FakePrompt = {
        type: "XCUIElementTypeSheet",
        label: "Save Password?",
        children: [
          { kind: "text", label: "Save Password?" },
          { kind: "text", label: "Securely store your password so it's filled automatically the next time you need it." },
          { kind: "button", label: "Not Now" },
          { kind: "button", label: "Save" }
        ]
      };

      function fakeDriver(input: {
        alertText?: string;
        prompts: FakePrompt[];
        readFails?: boolean;
      }) {
        const clicks: string[] = [];
        const executed: unknown[] = [];
        const settingsUpdates: unknown[] = [];
        const queries: string[] = [];
        const kindOf = (selector: string) => (selector.includes("XCUIElementTypeButton") ? "button" : "text");
        const child = (fake: FakePromptChild) => ({
          click: async () => { clicks.push(fake.label); },
          getAttribute: async (name: string) => (name === "label" ? fake.label : null),
          getText: async () => fake.label
        });
        const prompt = (fake: FakePrompt) => ({
          ...child({ kind: "text", label: fake.label }),
          $$: async (selector: string) => {
            queries.push(selector);
            const labelMatch = /label == "([^"]+)"/.exec(selector);
            return fake.children
              .filter((c) => c.kind === kindOf(selector) && (!labelMatch || c.label === labelMatch[1]))
              .map(child);
          }
        });
        const driver = {
          getAlertText: async () => {
            if (input.alertText === undefined) throw new Error("An attempt was made to operate on a modal dialog when one was not open");
            return input.alertText;
          },
          execute: async (script: string, args: unknown) => {
            executed.push([script, args]);
            return script === "mobile: alert" && (args as { action: string }).action === "getButtons"
              ? ["Don't Allow", "Allow"]
              : undefined;
          },
          updateSettings: async (next: unknown) => { settingsUpdates.push(next); },
          $$: async (selector: string) => {
            queries.push(selector);
            if (input.readFails) throw new Error("WebDriverAgent session lost");
            // Only a sheet or alert is ever asked for at the top level: the
            // app's own buttons are never candidates.
            if (!/XCUIElementTypeSheet|XCUIElementTypeAlert/.test(selector)) {
              throw new Error(`unexpected top-level query: ${selector}`);
            }
            return input.prompts.filter((fake) => selector.includes(fake.type)).map(prompt);
          },
          $: () => { throw new Error("single-element lookups are not part of the prompt read"); }
        } as unknown as Browser;
        return { driver, clicks, executed, settingsUpdates, queries };
      }

      it("reads no alert when the app shows no sheet or alert of its own, without switching WebDriverAgent's application", async () => {
        // The app's own screen is full of buttons (Sign Out, links), and the
        // status bar above it carries a "Return to <app>" breadcrumb; none of
        // that is queried, so none of it can read as a prompt.
        const { driver, settingsUpdates, queries } = fakeDriver({ prompts: [] });

        await expect(createBillingReviewUi(driver).getSystemAlert()).resolves.toBeNull();
        expect(settingsUpdates).toEqual([]);
        expect(queries).toEqual([
          '-ios predicate string:(type == "XCUIElementTypeSheet" OR type == "XCUIElementTypeAlert") AND visible == 1'
        ]);
      });

      it("reads the Save Password sheet's text and buttons from the app's own tree", async () => {
        const { driver, settingsUpdates } = fakeDriver({ prompts: [savePasswordSheet] });

        await expect(createBillingReviewUi(driver).getSystemAlert()).resolves.toEqual({
          text: "Save Password?\nSecurely store your password so it's filled automatically the next time you need it.",
          buttons: ["Not Now", "Save"]
        });
        expect(settingsUpdates).toEqual([]);
      });

      it("falls back to the sheet's own label when it carries no static text", async () => {
        const { driver } = fakeDriver({
          prompts: [{ type: "XCUIElementTypeAlert", label: "Allow tracking?", children: [{ kind: "button", label: "Ask App Not to Track" }] }]
        });

        await expect(createBillingReviewUi(driver).getSystemAlert()).resolves.toEqual({
          text: "Allow tracking?",
          buttons: ["Ask App Not to Track"]
        });
      });

      it("surfaces a failed read instead of reporting a clean screen", async () => {
        const { driver, settingsUpdates } = fakeDriver({ prompts: [savePasswordSheet], readFails: true });

        await expect(createBillingReviewUi(driver).getSystemAlert()).rejects.toThrow("WebDriverAgent session lost");
        expect(settingsUpdates).toEqual([]);
      });

      it("answers a WebDriver alert first, without asking the app's tree", async () => {
        const { driver, queries } = fakeDriver({
          alertText: "“Kanna” Would Like to Send You Notifications",
          prompts: []
        });

        await expect(createBillingReviewUi(driver).getSystemAlert()).resolves.toEqual({
          text: "“Kanna” Would Like to Send You Notifications",
          buttons: ["Don't Allow", "Allow"]
        });
        expect(queries).toEqual([]);
      });

      it("taps the sheet's decline button inside the sheet and refuses a button it does not offer", async () => {
        const { driver, clicks } = fakeDriver({ prompts: [savePasswordSheet] });
        const ui = createBillingReviewUi(driver);

        await ui.tapSystemAlertButton("Not Now");
        expect(clicks).toEqual(["Not Now"]);

        await expect(ui.tapSystemAlertButton("Don't Allow")).rejects.toThrow("offers no Don't Allow button");
        await expect(createBillingReviewUi(fakeDriver({ prompts: [] }).driver).tapSystemAlertButton("Not Now"))
          .rejects.toThrow("No system prompt is painted over the app");
      });
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
