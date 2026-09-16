import { mkdir } from "node:fs/promises";
import { dirname } from "node:path";
import type { Browser } from "webdriverio";
import { selectors } from "../../helpers/selectors";

/**
 * Production-identity Apple billing review capture.
 *
 * Signs the App Review account into the real production build, opens the
 * account sheet directly, reads the actual `AppleBillingCard` state — the
 * localized StoreKit monthly price, restore control, and legal links — and
 * writes one screenshot for the App Store review submission.
 *
 * It never taps Subscribe or Restore Purchases: a purchase sheet would charge
 * a real Apple account and restore would present Apple sign-in, and neither is
 * evidence this lane needs. Every readiness fact is reported as observed, so a
 * denied billing read or a missing storefront price shows up as a named
 * blocker in the report rather than as a fabricated "ready".
 */

const SCREEN_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 250;
const PRICE_UNAVAILABLE_TEXT = "Monthly price unavailable";

export interface BillingReviewCredentials {
  email?: string;
  password?: string;
}

export interface BillingReviewElement {
  click(): Promise<unknown>;
  getAttribute(name: string): Promise<string | null>;
  getText(): Promise<string>;
  isExisting(): Promise<boolean>;
  setValue(value: string): Promise<unknown>;
  waitForDisplayed(options: { timeout: number }): Promise<unknown>;
}

export interface BillingReviewUi {
  getAppShell(): Promise<BillingReviewElement>;
  getAccountButton(): Promise<BillingReviewElement>;
  getAccountSheet(): Promise<BillingReviewElement>;
  getSignOutButton(): Promise<BillingReviewElement>;
  getEmailInput(): Promise<BillingReviewElement>;
  getPasswordInput(): Promise<BillingReviewElement>;
  getSignInButton(): Promise<BillingReviewElement>;
  getVerificationState(): Promise<BillingReviewElement>;
  getSubscriptionState(): Promise<BillingReviewElement>;
  getEntitledState(): Promise<BillingReviewElement>;
  getBillingCard(): Promise<BillingReviewElement>;
  getBillingUnconfirmed(): Promise<BillingReviewElement>;
  getBillingSource(source: "comp" | "stripe" | "app_store"): Promise<BillingReviewElement>;
  getBillingPrice(): Promise<BillingReviewElement>;
  getBillingSubscribeButton(): Promise<BillingReviewElement>;
  getBillingRestoreButton(): Promise<BillingReviewElement>;
  getBillingEulaLink(): Promise<BillingReviewElement>;
  getBillingPrivacyLink(): Promise<BillingReviewElement>;
  getBillingMessage(): Promise<BillingReviewElement>;
  captureScreenshot(path: string): Promise<void>;
  waitUntil(
    condition: () => Promise<boolean>,
    options: { interval: number; timeout: number; timeoutMsg: string }
  ): Promise<unknown>;
}

export type BillingReviewBlocker =
  | "reviewer-sign-in-failed"
  | "reviewer-email-unverified"
  | "billing-card-not-rendered"
  | "billing-read-unconfirmed"
  | "storefront-price-unavailable"
  | "subscribe-disabled"
  | "restore-disabled"
  | "legal-links-missing";

export interface BillingReviewReport {
  signedIn: boolean;
  emailVerified: boolean;
  /** Which account-sheet state rendered: the Apple card, or a non-Apple state that replaced it. */
  accountState: "apple-billing" | "verification" | "subscription-inactive" | "entitled" | "unknown";
  cardRendered: boolean;
  billingConfirmed: boolean;
  billingSources: Array<"comp" | "stripe" | "app_store">;
  price: string | null;
  priceAvailable: boolean;
  subscribeEnabled: boolean | null;
  restoreEnabled: boolean | null;
  eulaPresent: boolean;
  privacyPresent: boolean;
  message: string | null;
  screenshotPath: string;
  ready: boolean;
  blockers: BillingReviewBlocker[];
}

export function createBillingReviewUi(driver: Browser): BillingReviewUi {
  const element = (selector: string) => driver.$(selector);
  return {
    getAppShell: async () => element(selectors.appShell),
    getAccountButton: async () => element(selectors.accountButton),
    getAccountSheet: async () => element(selectors.accountSheet),
    getSignOutButton: async () => element(selectors.accountSignOutButton),
    getEmailInput: async () => element(selectors.accountEmailInput),
    getPasswordInput: async () => element(selectors.accountPasswordInput),
    getSignInButton: async () => element(selectors.accountSignInButton),
    getVerificationState: async () => element(selectors.accountVerificationState),
    getSubscriptionState: async () => element(selectors.accountSubscriptionState),
    getEntitledState: async () => element(selectors.accountEntitledState),
    getBillingCard: async () => element(selectors.appleBillingCard),
    getBillingUnconfirmed: async () => element(selectors.appleBillingUnconfirmed),
    getBillingSource: async (source) => element(selectors.appleBillingSource(source)),
    getBillingPrice: async () => element(selectors.appleBillingPrice),
    getBillingSubscribeButton: async () => element(selectors.appleBillingSubscribeButton),
    getBillingRestoreButton: async () => element(selectors.appleBillingRestoreButton),
    getBillingEulaLink: async () => element(selectors.appleBillingEulaLink),
    getBillingPrivacyLink: async () => element(selectors.appleBillingPrivacyLink),
    getBillingMessage: async () => element(selectors.appleBillingMessage),
    async captureScreenshot(path) {
      await mkdir(dirname(path), { recursive: true });
      await driver.saveScreenshot(path);
    },
    waitUntil: async (condition, options) => driver.waitUntil(condition, options)
  };
}

export function requireBillingReviewCredentials(
  credentials: BillingReviewCredentials
): Required<BillingReviewCredentials> {
  if (!credentials.email || !credentials.password) {
    throw new Error(
      "KANNA_E2E_CLOUD_EMAIL and KANNA_E2E_CLOUD_PASSWORD must select the App Review account for the billing review capture."
    );
  }
  return { email: credentials.email, password: credentials.password };
}

async function isEnabled(element: BillingReviewElement): Promise<boolean | null> {
  if (!(await element.isExisting())) return null;
  const enabled = await element.getAttribute("enabled");
  if (enabled === "true") return true;
  if (enabled === "false") return false;
  return null;
}

async function signInReviewer(
  ui: BillingReviewUi,
  credentials: Required<BillingReviewCredentials>
): Promise<boolean> {
  await (await ui.getEmailInput()).setValue(credentials.email);
  await (await ui.getPasswordInput()).setValue(credentials.password);
  await (await ui.getSignInButton()).click();
  return ui.waitUntil(
    async () => (await ui.getSignOutButton()).isExisting(),
    {
      interval: POLL_INTERVAL_MS,
      timeout: SCREEN_TIMEOUT_MS,
      timeoutMsg: "Expected the App Review account sign-in to complete"
    }
  ).then(() => true, () => false);
}

async function resolveAccountState(
  ui: BillingReviewUi
): Promise<BillingReviewReport["accountState"]> {
  if (await (await ui.getVerificationState()).isExisting()) return "verification";
  if (await (await ui.getBillingCard()).isExisting()) return "apple-billing";
  if (await (await ui.getSubscriptionState()).isExisting()) return "subscription-inactive";
  if (await (await ui.getEntitledState()).isExisting()) return "entitled";
  return "unknown";
}

export async function runBillingReviewJourney(
  ui: BillingReviewUi,
  options: { credentials: BillingReviewCredentials; screenshotPath: string }
): Promise<BillingReviewReport> {
  const credentials = requireBillingReviewCredentials(options.credentials);
  await (await ui.getAppShell()).waitForDisplayed({ timeout: SCREEN_TIMEOUT_MS });

  const accountButton = await ui.getAccountButton();
  await accountButton.waitForDisplayed({ timeout: SCREEN_TIMEOUT_MS });
  await accountButton.click();
  await (await ui.getAccountSheet()).waitForDisplayed({ timeout: SCREEN_TIMEOUT_MS });

  let signedIn = await (await ui.getSignOutButton()).isExisting();
  if (!signedIn) {
    signedIn = await signInReviewer(ui, credentials);
  }

  // Held in an object: TypeScript does not track assignments made inside the
  // poll callback, and the later comparisons must see the settled value.
  const observed: { accountState: BillingReviewReport["accountState"] } = {
    accountState: "unknown"
  };
  if (signedIn) {
    await ui.waitUntil(
      async () => {
        observed.accountState = await resolveAccountState(ui);
        return observed.accountState !== "unknown";
      },
      {
        interval: POLL_INTERVAL_MS,
        timeout: SCREEN_TIMEOUT_MS,
        timeoutMsg: "Expected the account sheet to settle into a verification, billing, or entitlement state"
      }
    ).catch(() => undefined);
  }

  const accountState = observed.accountState;
  const cardRendered = accountState === "apple-billing";
  if (cardRendered) {
    // One bounded settle for the asynchronous billing read and StoreKit
    // product fetch. A denied read or a missing storefront price is reported,
    // never retried.
    await ui.waitUntil(
      async () => {
        if (await (await ui.getBillingUnconfirmed()).isExisting()) return false;
        const price = await ui.getBillingPrice();
        if (await price.isExisting()) {
          return (await price.getText()).trim() !== PRICE_UNAVAILABLE_TEXT;
        }
        for (const source of ["comp", "stripe", "app_store"] as const) {
          if (await (await ui.getBillingSource(source)).isExisting()) return true;
        }
        return false;
      },
      {
        interval: POLL_INTERVAL_MS,
        timeout: SCREEN_TIMEOUT_MS,
        timeoutMsg: "Apple billing card did not settle"
      }
    ).catch(() => undefined);
  }

  const billingConfirmed = cardRendered && !(await (await ui.getBillingUnconfirmed()).isExisting());
  const billingSources: BillingReviewReport["billingSources"] = [];
  if (cardRendered) {
    for (const source of ["comp", "stripe", "app_store"] as const) {
      if (await (await ui.getBillingSource(source)).isExisting()) billingSources.push(source);
    }
  }
  const priceElement = cardRendered ? await ui.getBillingPrice() : null;
  const priceText = priceElement && (await priceElement.isExisting())
    ? (await priceElement.getText()).trim()
    : null;
  const priceAvailable = priceText !== null && priceText !== PRICE_UNAVAILABLE_TEXT && priceText.length > 0;
  const subscribeEnabled = cardRendered ? await isEnabled(await ui.getBillingSubscribeButton()) : null;
  const restoreEnabled = cardRendered ? await isEnabled(await ui.getBillingRestoreButton()) : null;
  const eulaPresent = cardRendered && (await (await ui.getBillingEulaLink()).isExisting());
  const privacyPresent = cardRendered && (await (await ui.getBillingPrivacyLink()).isExisting());
  const messageElement = cardRendered ? await ui.getBillingMessage() : null;
  const message = messageElement && (await messageElement.isExisting())
    ? (await messageElement.getText()).trim() || null
    : null;

  await ui.captureScreenshot(options.screenshotPath);

  const blockers: BillingReviewBlocker[] = [];
  if (!signedIn) blockers.push("reviewer-sign-in-failed");
  if (accountState === "verification") blockers.push("reviewer-email-unverified");
  if (signedIn && !cardRendered) blockers.push("billing-card-not-rendered");
  if (cardRendered && !billingConfirmed) blockers.push("billing-read-unconfirmed");
  const alreadyCovered = billingSources.length > 0;
  if (billingConfirmed && !alreadyCovered && !priceAvailable) blockers.push("storefront-price-unavailable");
  if (billingConfirmed && !alreadyCovered && priceAvailable && subscribeEnabled !== true) blockers.push("subscribe-disabled");
  if (cardRendered && restoreEnabled !== true) blockers.push("restore-disabled");
  if (cardRendered && (!eulaPresent || !privacyPresent)) blockers.push("legal-links-missing");

  return {
    signedIn,
    emailVerified: signedIn && accountState !== "verification",
    accountState,
    cardRendered,
    billingConfirmed,
    billingSources,
    price: priceText,
    priceAvailable,
    subscribeEnabled,
    restoreEnabled,
    eulaPresent,
    privacyPresent,
    message,
    screenshotPath: options.screenshotPath,
    ready: blockers.length === 0,
    blockers
  };
}

export async function runBillingReviewCapture(
  driver: Browser,
  options: { credentials: BillingReviewCredentials; screenshotPath: string }
): Promise<BillingReviewReport> {
  return runBillingReviewJourney(createBillingReviewUi(driver), options);
}
