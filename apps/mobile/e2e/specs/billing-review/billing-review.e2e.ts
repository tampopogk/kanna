import { mkdir } from "node:fs/promises";
import { dirname } from "node:path";
import type { Browser } from "webdriverio";
import { normalizeAccountIdentity } from "../../../src/accountIdentity";
import { selectors } from "../../helpers/selectors";

/**
 * Production-identity Apple billing review capture.
 *
 * Signs the App Review account into the real production build, opens the
 * account sheet directly, reads the actual `AppleBillingCard` state — the
 * localized StoreKit monthly price, restore control, and legal links — and
 * writes one screenshot for the App Store review submission.
 *
 * Two facts are bound before anything is credited as ready:
 *
 * - The session belongs to the selected reviewer account. A retained session
 *   is only kept when the E2E-gated account identity marker names the selected
 *   account; otherwise it is signed out and the selected account is signed in.
 *   "Sign Out is visible" was never evidence of *which* account is signed in.
 * - Coverage follows the card's own semantics, not the presence of a source
 *   row. `AppleBillingCard` lists Apple and Stripe rows for expired and revoked
 *   records too, and only an active complimentary grant or an active/grace paid
 *   subscription blocks the purchase path. A historical source with no
 *   localized price and a disabled Subscribe is not review-ready.
 *
 * It never taps Subscribe or Restore Purchases: a purchase sheet would charge
 * a real Apple account and restore would present Apple sign-in, and neither is
 * evidence this lane needs. Every readiness fact is reported as observed, so a
 * denied billing read or a missing storefront price shows up as a named
 * blocker in the report rather than as a fabricated "ready". The report never
 * carries the reviewer email or password.
 *
 * The screenshot is the deliverable, and the report reads the accessibility
 * tree, which sees straight through anything iOS paints over the app. The
 * lane's own sign-in raises the system "Save Password?" sheet, which once
 * covered the card heading, price, terms and Subscribe in a capture that the
 * tree-derived report still credited as ready. So before capturing, the lane
 * declines every system alert it can (`Not Now`, never `Save`), and a capture
 * taken under an alert it could not clear is reported as `screenshot-obscured`
 * rather than as ready.
 */

const SCREEN_TIMEOUT_MS = 30_000;
const POLL_INTERVAL_MS = 250;
const PRICE_UNAVAILABLE_TEXT = "Monthly price unavailable";
/** How long a tapped system alert gets to leave the screen. */
const SYSTEM_ALERT_SETTLE_TIMEOUT_MS = 5_000;
/** Upper bound on stacked system alerts cleared before one capture. */
const MAX_SYSTEM_ALERTS_PER_CAPTURE = 3;
/**
 * Alert buttons that decline whatever the alert offers, in preference order.
 * `Not Now` is the Save Password sheet's own decline. An alert offering none
 * of these is never tapped blindly: it is reported as obscuring the capture.
 */
export const SYSTEM_ALERT_DECLINE_LABELS: readonly string[] = ["Not Now", "Don't Allow", "Cancel"];

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
  getAccountIdentity(): Promise<BillingReviewElement>;
  getSignOutButton(): Promise<BillingReviewElement>;
  getEmailInput(): Promise<BillingReviewElement>;
  getPasswordInput(): Promise<BillingReviewElement>;
  getSignInButton(): Promise<BillingReviewElement>;
  getVerificationState(): Promise<BillingReviewElement>;
  getSubscriptionState(): Promise<BillingReviewElement>;
  getEntitledState(): Promise<BillingReviewElement>;
  getBillingCard(): Promise<BillingReviewElement>;
  getBillingUnconfirmed(): Promise<BillingReviewElement>;
  getBillingSource(source: BillingReviewSourceKind): Promise<BillingReviewElement>;
  getBillingPrice(): Promise<BillingReviewElement>;
  getBillingSubscribeButton(): Promise<BillingReviewElement>;
  getBillingRestoreButton(): Promise<BillingReviewElement>;
  getBillingEulaLink(): Promise<BillingReviewElement>;
  getBillingPrivacyLink(): Promise<BillingReviewElement>;
  getBillingMessage(): Promise<BillingReviewElement>;
  /** The iOS system alert or sheet currently painted over the app, or `null` when none is. */
  getSystemAlert(): Promise<BillingReviewSystemAlert | null>;
  /** Taps the button of the current system alert whose label is `label`. */
  tapSystemAlertButton(label: string): Promise<void>;
  captureScreenshot(path: string): Promise<void>;
  waitUntil(
    condition: () => Promise<boolean>,
    options: { interval: number; timeout: number; timeoutMsg: string }
  ): Promise<unknown>;
}

/** An iOS system alert as observed over the app: its text and the buttons it offers. */
export interface BillingReviewSystemAlert {
  text: string;
  buttons: string[];
}

export type BillingReviewSourceKind = "comp" | "stripe" | "app_store";
export type BillingReviewSourceStatus = "active" | "grace" | "expired" | "revoked";

/** One billing source row as the real card rendered it. */
export interface BillingReviewSource {
  source: BillingReviewSourceKind;
  /** Status the row names; `null` when the row carries none (a comp row never does). */
  status: BillingReviewSourceStatus | null;
  /** Whether this row is coverage under the card's own purchase-blocking rule. */
  covering: boolean;
}

/** How the session was bound to the selected reviewer account. */
export type BillingReviewAccountBinding =
  /** The retained session's identity marker named the selected account. */
  | "existing-session"
  /** No session was retained; the selected credentials were entered. */
  | "fresh-sign-in"
  /** A retained session was not provably the selected account, so it was signed out and the selected account signed in. */
  | "reauthenticated"
  /** No session provably belonging to the selected account was established. */
  | "unbound";

/** What the E2E-gated account identity marker said after binding. */
export type BillingReviewIdentityCheck = "matched" | "mismatched" | "unavailable";

export type BillingReviewBlocker =
  | "reviewer-sign-in-failed"
  | "reviewer-session-not-replaced"
  | "reviewer-account-mismatch"
  | "reviewer-email-unverified"
  | "billing-card-not-rendered"
  | "billing-read-unconfirmed"
  | "purchase-path-missing"
  | "storefront-price-unavailable"
  | "subscribe-disabled"
  | "restore-disabled"
  | "legal-links-missing"
  /** A system alert still covered the app when the screenshot was taken. */
  | "screenshot-obscured";

export interface BillingReviewReport {
  signedIn: boolean;
  accountBinding: BillingReviewAccountBinding;
  identityCheck: BillingReviewIdentityCheck;
  emailVerified: boolean;
  /** Which account-sheet state rendered: the Apple card, or a non-Apple state that replaced it. */
  accountState: "apple-billing" | "verification" | "subscription-inactive" | "entitled" | "unknown";
  cardRendered: boolean;
  billingConfirmed: boolean;
  billingSources: BillingReviewSource[];
  /** True only when a covering source is shown and the card hides its purchase path, as it does for a blocked account. */
  activeCoverage: boolean;
  /** Whether the card rendered its price/Subscribe purchase path at all. */
  purchasePathRendered: boolean;
  price: string | null;
  priceAvailable: boolean;
  subscribeEnabled: boolean | null;
  restoreEnabled: boolean | null;
  eulaPresent: boolean;
  privacyPresent: boolean;
  message: string | null;
  /** Text of each system alert declined before the capture, in order. */
  systemAlertsDismissed: string[];
  /** Text of the system alert still covering the app at capture time, or `null` for a clean capture. */
  screenshotObscuredBy: string | null;
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
    getAccountIdentity: async () => element(selectors.accountIdentity),
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
    getSystemAlert: async () => readSystemAlert(driver),
    async tapSystemAlertButton(label) {
      const alertText = await driver.getAlertText().catch(() => null);
      if (alertText !== null) {
        await driver.execute("mobile: alert", { action: "accept", buttonLabel: label });
        return;
      }
      const prompt = await findInAppPrompt(driver);
      if (!prompt) throw new Error(`No system prompt is painted over the app to offer a ${label} button`);
      const [button] = await childrenOf(prompt, promptButtonSelector(label));
      if (!button) throw new Error(`The system prompt over the app offers no ${label} button`);
      await button.click();
    },
    async captureScreenshot(path) {
      await mkdir(dirname(path), { recursive: true });
      await driver.saveScreenshot(path);
    },
    waitUntil: async (condition, options) => driver.waitUntil(condition, options)
  };
}

function promptButtonSelector(label: string): string {
  return `-ios predicate string:type == "XCUIElementTypeButton" AND visible == 1 AND label == ${JSON.stringify(label)}`;
}

/**
 * The system prompts iOS presents *inside* the app's own accessibility tree.
 * The Save Password sheet is one: on iOS 26.5 it is an XCUIElementTypeSheet
 * named "Save Password?" inside the app's window, with "Not Now" and "Save"
 * as descendant buttons; it is not hosted by SpringBoard. WebDriverAgent's
 * alert detection walks the app's descendants and stops at the first Alert,
 * Sheet or ScrollView it meets, so with the account sheet's scroll view above
 * it `getAlertText` answers "no alert" while the sheet covers the card. This
 * read asks for the sheet itself, and only for elements of an alert or sheet,
 * so nothing the app itself renders can be mistaken for a prompt.
 */
const IN_APP_PROMPT_SELECTOR =
  '-ios predicate string:(type == "XCUIElementTypeSheet" OR type == "XCUIElementTypeAlert") AND visible == 1';
const VISIBLE_BUTTONS_SELECTOR = '-ios predicate string:type == "XCUIElementTypeButton" AND visible == 1';
const VISIBLE_TEXTS_SELECTOR = '-ios predicate string:type == "XCUIElementTypeStaticText" AND visible == 1';

/** The subset of a WebdriverIO element the prompt read needs; the fake driver in the tests implements it. */
interface PromptChild {
  click(): Promise<unknown>;
  getAttribute(name: string): Promise<string | null>;
  getText(): Promise<string>;
}

interface PromptElement extends PromptChild {
  $$(selector: string): PromiseLike<Iterable<PromptChild>>;
}

async function findInAppPrompt(driver: Browser): Promise<PromptElement | null> {
  const found = await driver.$$(IN_APP_PROMPT_SELECTOR);
  const [prompt] = Array.from(found as unknown as Iterable<PromptElement>);
  return prompt ?? null;
}

async function childrenOf(prompt: PromptElement, selector: string): Promise<PromptChild[]> {
  return Array.from(await prompt.$$(selector));
}

/**
 * Observes the system alert over the app, if any. A WebDriver alert is asked
 * first, which is how SpringBoard-hosted permission alerts are seen; then the
 * app's tree is asked for a sheet or alert of its own, which is where the Save
 * Password prompt lives and where WebDriverAgent's alert search never reaches
 * it.
 */
async function readSystemAlert(driver: Browser): Promise<BillingReviewSystemAlert | null> {
  const alertText = await driver.getAlertText().catch(() => null);
  if (alertText !== null) {
    const buttons = await driver
      .execute("mobile: alert", { action: "getButtons" })
      .catch(() => []);
    return { text: alertText, buttons: labelsOf(buttons) };
  }
  const prompt = await findInAppPrompt(driver);
  if (!prompt) return null;
  const buttons = labelsOf(
    await Promise.all((await childrenOf(prompt, VISIBLE_BUTTONS_SELECTOR)).map((button) => button.getAttribute("label")))
  );
  const texts = (
    await Promise.all((await childrenOf(prompt, VISIBLE_TEXTS_SELECTOR)).map((text) => text.getText()))
  ).filter((text) => text.trim().length > 0);
  const text = texts.length > 0 ? texts.join("\n") : (await prompt.getAttribute("label")) ?? "";
  return { text, buttons };
}

function labelsOf(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((label) => label !== null && label !== undefined).map((label) => String(label))
    : [];
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

/**
 * Classifies a rendered source row exactly as `AppleBillingCard` does when it
 * decides whether to block the purchase path: a comp row is only rendered when
 * the grant is active, and a paid row counts only while `active` or `grace`.
 * Expired and revoked rows are shown as history and never count as coverage.
 */
export function classifyBillingSource(
  source: BillingReviewSourceKind,
  rowText: string
): BillingReviewSource {
  if (source === "comp") return { source, status: null, covering: true };
  const status = parseSourceStatus(rowText);
  return { source, status, covering: status === "active" || status === "grace" };
}

function parseSourceStatus(rowText: string): BillingReviewSourceStatus | null {
  const afterSeparator = rowText.split("·")[1]?.trim() ?? "";
  const token = afterSeparator.split(/[\s(]/)[0] ?? "";
  return token === "active" || token === "grace" || token === "expired" || token === "revoked"
    ? token
    : null;
}

async function isEnabled(element: BillingReviewElement): Promise<boolean | null> {
  if (!(await element.isExisting())) return null;
  const enabled = await element.getAttribute("enabled");
  if (enabled === "true") return true;
  if (enabled === "false") return false;
  return null;
}

/**
 * Compares the E2E-gated identity marker with the selected reviewer account,
 * in-process only. The marker text is never returned, logged, or stored.
 */
async function checkAccountIdentity(
  ui: BillingReviewUi,
  selectedEmail: string
): Promise<BillingReviewIdentityCheck> {
  const marker = await ui.getAccountIdentity();
  if (!(await marker.isExisting())) return "unavailable";
  const rendered = (await marker.getText()).trim();
  if (!rendered) return "unavailable";
  return normalizeAccountIdentity(rendered) === normalizeAccountIdentity(selectedEmail)
    ? "matched"
    : "mismatched";
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

/** Signs the retained session out and waits for the sign-in form to return. */
async function signOutRetainedSession(ui: BillingReviewUi): Promise<boolean> {
  await (await ui.getSignOutButton()).click();
  return ui.waitUntil(
    async () =>
      !(await (await ui.getSignOutButton()).isExisting()) &&
      (await (await ui.getEmailInput()).isExisting()),
    {
      interval: POLL_INTERVAL_MS,
      timeout: SCREEN_TIMEOUT_MS,
      timeoutMsg: "Expected the retained session to sign out before the App Review account signs in"
    }
  ).then(() => true, () => false);
}

interface BoundSession {
  signedIn: boolean;
  accountBinding: BillingReviewAccountBinding;
  identityCheck: BillingReviewIdentityCheck;
  /** Why no bound session exists; set exactly when `accountBinding` is `unbound`. */
  unboundBlocker?: "reviewer-sign-in-failed" | "reviewer-session-not-replaced" | "reviewer-account-mismatch";
}

/**
 * Establishes a session that provably belongs to the selected reviewer
 * account. A retained session is kept only when its identity marker matches;
 * a mismatched or unverifiable one is signed out and the selected account is
 * signed in. Nothing here is credited from the presence of Sign Out alone.
 */
async function bindReviewerSession(
  ui: BillingReviewUi,
  credentials: Required<BillingReviewCredentials>
): Promise<BoundSession> {
  const retained = await (await ui.getSignOutButton()).isExisting();
  if (retained) {
    const retainedIdentity = await checkAccountIdentity(ui, credentials.email);
    if (retainedIdentity === "matched") {
      return { signedIn: true, accountBinding: "existing-session", identityCheck: "matched" };
    }
    if (!(await signOutRetainedSession(ui))) {
      return {
        signedIn: true,
        accountBinding: "unbound",
        identityCheck: retainedIdentity,
        unboundBlocker: "reviewer-session-not-replaced"
      };
    }
  }

  const signedIn = await signInReviewer(ui, credentials);
  if (!signedIn) {
    return {
      signedIn: false,
      accountBinding: "unbound",
      identityCheck: "unavailable",
      unboundBlocker: "reviewer-sign-in-failed"
    };
  }
  const identityCheck = await checkAccountIdentity(ui, credentials.email);
  if (identityCheck === "mismatched") {
    return {
      signedIn: true,
      accountBinding: "unbound",
      identityCheck,
      unboundBlocker: "reviewer-account-mismatch"
    };
  }
  return {
    signedIn: true,
    accountBinding: retained ? "reauthenticated" : "fresh-sign-in",
    identityCheck
  };
}

interface ClearedSystemAlerts {
  /** Text of each alert declined, in order. */
  dismissed: string[];
  /** The alert still present after clearing, which will obscure the capture. */
  remaining: BillingReviewSystemAlert | null;
}

/**
 * Declines the system alerts painted over the app so the capture shows the
 * card. Only a decline button is ever tapped — the Save Password sheet's
 * `Save` would store the reviewer password in the simulator keychain — and
 * an alert offering none, or one that survives its tap, is left in place and
 * reported so the capture is not credited.
 */
async function clearSystemAlerts(ui: BillingReviewUi): Promise<ClearedSystemAlerts> {
  const dismissed: string[] = [];
  for (let cleared = 0; cleared < MAX_SYSTEM_ALERTS_PER_CAPTURE; cleared += 1) {
    const alert = await ui.getSystemAlert();
    if (alert === null) return { dismissed, remaining: null };
    const decline = SYSTEM_ALERT_DECLINE_LABELS.find((label) => alert.buttons.includes(label));
    if (decline === undefined) return { dismissed, remaining: alert };
    await ui.tapSystemAlertButton(decline).catch(() => undefined);
    const gone = await ui.waitUntil(
      async () => {
        const current = await ui.getSystemAlert();
        return current === null || current.text !== alert.text;
      },
      {
        interval: POLL_INTERVAL_MS,
        timeout: SYSTEM_ALERT_SETTLE_TIMEOUT_MS,
        timeoutMsg: `Expected the system alert ${JSON.stringify(alert.text)} to leave after ${decline}`
      }
    ).then(() => true, () => false);
    if (!gone) return { dismissed, remaining: alert };
    dismissed.push(alert.text);
  }
  return { dismissed, remaining: await ui.getSystemAlert() };
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

async function readBillingSources(ui: BillingReviewUi): Promise<BillingReviewSource[]> {
  const sources: BillingReviewSource[] = [];
  for (const source of ["comp", "stripe", "app_store"] as const) {
    const row = await ui.getBillingSource(source);
    if (await row.isExisting()) sources.push(classifyBillingSource(source, await row.getText()));
  }
  return sources;
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

  const session = await bindReviewerSession(ui, credentials);
  const { signedIn, accountBinding, identityCheck } = session;
  const accountBound = signedIn && accountBinding !== "unbound";

  // Held in an object: TypeScript does not track assignments made inside the
  // poll callback, and the later comparisons must see the settled value.
  const observed: { accountState: BillingReviewReport["accountState"] } = {
    accountState: "unknown"
  };
  if (accountBound) {
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
    // product fetch: a confirmed read that either priced the product or shows
    // covering source. A denied read, a missing storefront price, or a
    // history-only source is reported, never retried.
    await ui.waitUntil(
      async () => {
        if (await (await ui.getBillingUnconfirmed()).isExisting()) return false;
        const price = await ui.getBillingPrice();
        if (await price.isExisting() && (await price.getText()).trim() !== PRICE_UNAVAILABLE_TEXT) {
          return true;
        }
        return (await readBillingSources(ui)).some((source) => source.covering);
      },
      {
        interval: POLL_INTERVAL_MS,
        timeout: SCREEN_TIMEOUT_MS,
        timeoutMsg: "Apple billing card did not settle"
      }
    ).catch(() => undefined);
  }

  const billingConfirmed = cardRendered && !(await (await ui.getBillingUnconfirmed()).isExisting());
  const billingSources = cardRendered ? await readBillingSources(ui) : [];
  const priceElement = cardRendered ? await ui.getBillingPrice() : null;
  const purchasePathRendered = priceElement !== null && (await priceElement.isExisting());
  const priceText = priceElement && purchasePathRendered
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
  // The card hides its purchase path exactly when the account is blocked, so a
  // covering row that coexists with a rendered price contradicts the card and
  // is not credited; the purchase path is then held to its own requirements.
  const activeCoverage =
    billingConfirmed && !purchasePathRendered && billingSources.some((source) => source.covering);

  // The tree reads above see through anything painted over the app; the
  // screenshot does not. Clear the system alerts the sign-in raised, then
  // re-observe at capture time so an obscured capture is named, not credited.
  const cleared = await clearSystemAlerts(ui);
  const obscuringAlert = cleared.remaining ?? (await ui.getSystemAlert());
  const screenshotObscuredBy = obscuringAlert === null ? null : obscuringAlert.text;
  await ui.captureScreenshot(options.screenshotPath);

  const blockers: BillingReviewBlocker[] = [];
  if (session.unboundBlocker) blockers.push(session.unboundBlocker);
  if (accountState === "verification") blockers.push("reviewer-email-unverified");
  if (accountBound && !cardRendered) blockers.push("billing-card-not-rendered");
  if (cardRendered && !billingConfirmed) blockers.push("billing-read-unconfirmed");
  if (billingConfirmed && !activeCoverage) {
    if (!purchasePathRendered) blockers.push("purchase-path-missing");
    else if (!priceAvailable) blockers.push("storefront-price-unavailable");
    else if (subscribeEnabled !== true) blockers.push("subscribe-disabled");
  }
  if (cardRendered && restoreEnabled !== true) blockers.push("restore-disabled");
  if (cardRendered && (!eulaPresent || !privacyPresent)) blockers.push("legal-links-missing");
  if (screenshotObscuredBy !== null) blockers.push("screenshot-obscured");

  return {
    signedIn,
    accountBinding,
    identityCheck,
    emailVerified: accountBound && accountState !== "verification",
    accountState,
    cardRendered,
    billingConfirmed,
    billingSources,
    activeCoverage,
    purchasePathRendered,
    price: priceText,
    priceAvailable,
    subscribeEnabled,
    restoreEnabled,
    eulaPresent,
    privacyPresent,
    message,
    systemAlertsDismissed: cleared.dismissed,
    screenshotObscuredBy,
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
