import { readFile } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import {
  AppStoreConnectClient,
  resolveAppStoreConnectCredentials,
  type AppStoreConnectHttpRunner
} from "./app-store-connect";
import { resolveKdEnvironment } from "./environment";
import {
  MOBILE_OTA_PRIVATE_KEY_ENV,
  resolveMobileOtaPrivateKeyPath
} from "./mobile-ota-private-key";
import type { CommandRunner } from "./process";

export interface MobileQaCheck {
  name: string;
  ok: boolean;
  detail: string;
}

export interface MobileQaCommand {
  name: string;
  command: string;
  args: string[];
}

export interface MobileQaCommandResult extends MobileQaCommand {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export interface MobileQaResult {
  configChecks: MobileQaCheck[];
  commands: MobileQaCommandResult[];
}

type JsonRecord = Record<string, unknown>;

export function buildProductionMobileQaCommands(repoRoot: string): MobileQaCommand[] {
  const mobileRoot = join(repoRoot, "apps", "mobile");
  return [
    {
      name: "typecheck",
      command: "pnpm",
      args: ["--dir", mobileRoot, "run", "typecheck"]
    },
    {
      name: "unit",
      command: "pnpm",
      args: ["--dir", mobileRoot, "run", "test"]
    },
    {
      name: "simulator-preflight",
      command: "pnpm",
      args: ["--dir", mobileRoot, "run", "test:e2e:preflight"]
    },
    {
      name: "simulator-smoke",
      command: "pnpm",
      args: ["--dir", mobileRoot, "run", "test:e2e:smoke"]
    }
  ];
}

export async function executeProductionMobileQa(input: {
  repoRoot: string;
  env: NodeJS.ProcessEnv;
  keyPath?: string;
  runner: CommandRunner;
}): Promise<MobileQaResult> {
  const configPath = join(input.repoRoot, "apps", "mobile", "src", "mobileEnvironments.json");
  const configChecks = validateProductionMobileConfig(JSON.parse(await readFile(configPath, "utf8")));
  const commands: MobileQaCommandResult[] = [];
  const commandEnv: NodeJS.ProcessEnv = {
    ...input.env,
    KANNA_APP_ENV: "prod",
    KANNA_E2E_DESKTOP_SERVER_URL: input.env.KANNA_E2E_DESKTOP_SERVER_URL ?? "http://127.0.0.1:48120"
  };
  if (input.keyPath !== undefined) {
    commandEnv[MOBILE_OTA_PRIVATE_KEY_ENV] = input.keyPath;
  }
  resolveMobileOtaPrivateKeyPath(commandEnv, { required: true });

  if (configChecks.some((check) => !check.ok)) {
    return { configChecks, commands };
  }
  for (const command of buildProductionMobileQaCommands(input.repoRoot)) {
    const result = await input.runner.run(command.command, command.args, {
      cwd: input.repoRoot,
      env: commandEnv,
      streamOutput: true
    });
    commands.push({
      ...command,
      exitCode: result.exitCode,
      stdout: result.stdout,
      stderr: result.stderr
    });
    if (result.exitCode !== 0) {
      break;
    }
  }

  return { configChecks, commands };
}

export function validateProductionMobileConfig(environments: unknown): MobileQaCheck[] {
  const identity = resolveKdEnvironment("prod");
  const root = asRecord(environments);
  const prod = asRecord(root.prod);
  const firebase = asRecord(prod.firebase);

  return [
    expectString("displayName", prod.displayName, "Kanna"),
    expectString("scheme", prod.scheme, "kanna"),
    expectString("iosBundleId", prod.iosBundleId, identity.iosBundleId),
    expectString("iosGoogleServicesFile", prod.iosGoogleServicesFile, "./firebase/GoogleService-Info.production.plist"),
    expectNonEmpty("runtimeVersion", prod.runtimeVersion),
    expectString("firebase.projectId", firebase.projectId, identity.firebaseProjectId),
    expectString("firebase.storageBucket", firebase.storageBucket, identity.otaBucket ?? ""),
    expectProductionSecretLike("firebase.apiKey", firebase.apiKey),
    expectNonEmpty("firebase.appId", firebase.appId),
    expectString("relayUrl", prod.relayUrl, identity.relayUrl),
    expectString("otaChannel", prod.otaChannel, identity.otaChannel ?? "")
  ];
}

export function formatProductionMobileQaResult(result: MobileQaResult): string {
  const configLines = result.configChecks.map((check) =>
    `${check.ok ? "PASS" : "FAIL"} config:${check.name} ${check.detail}`
  );
  const commandLines = result.commands.map((command) =>
    `${command.exitCode === 0 ? "PASS" : "FAIL"} ${command.name}: ${command.command} ${command.args.join(" ")}`
  );
  const skippedCommands =
    result.configChecks.some((check) => !check.ok) && result.commands.length === 0
      ? ["SKIP automated commands: production mobile config sanity failed"]
      : [];
  return [
    "Production mobile QA gate",
    ...configLines,
    ...skippedCommands,
    ...commandLines,
    "",
    "Manual-only: install the TestFlight/App Store candidate on a physical iPhone, verify Local Network permission, sign in, connect to production desktop/relay, open a task, stream terminal output, send input, and confirm OTA update behavior when an OTA is expected."
  ].join("\n");
}

export function isProductionMobileQaOk(result: MobileQaResult): boolean {
  return result.configChecks.every((check) => check.ok) &&
    result.commands.length === buildProductionMobileQaCommands("").length &&
    result.commands.every((command) => command.exitCode === 0);
}

function asRecord(value: unknown): JsonRecord {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as JsonRecord
    : {};
}

function expectString(name: string, actual: unknown, expected: string): MobileQaCheck {
  const actualString = typeof actual === "string" ? actual.trim() : "";
  return {
    name,
    ok: actualString === expected,
    detail: actualString === expected ? actualString : `expected ${expected}, got ${actualString || "<missing>"}`
  };
}

function expectNonEmpty(name: string, actual: unknown): MobileQaCheck {
  const actualString = typeof actual === "string" ? actual.trim() : "";
  return {
    name,
    ok: actualString.length > 0,
    detail: actualString.length > 0 ? actualString : "missing"
  };
}

function expectProductionSecretLike(name: string, actual: unknown): MobileQaCheck {
  const actualString = typeof actual === "string" ? actual.trim() : "";
  const ok = actualString.length > 0 && actualString !== "kanna-local";
  return {
    name,
    ok,
    detail: ok ? "set" : `expected production Firebase apiKey, got ${actualString || "<missing>"}`
  };
}

// ---------------------------------------------------------------------------
// Production-identity billing review capture
//
// A focused lane beside the full QA gate: it does not replace the task-list
// smoke, TestFlight purchase/restore acceptance, or the human physical-device
// check. It signs the App Review account into the real production build on a
// simulator, reads the actual Apple billing card, and writes the App Store
// review screenshot where the caller asked.
// ---------------------------------------------------------------------------

export const BILLING_REVIEW_EMAIL_ENV = "KANNA_E2E_CLOUD_EMAIL";
export const BILLING_REVIEW_PASSWORD_ENV = "KANNA_E2E_CLOUD_PASSWORD";
export const BILLING_REVIEW_SCREENSHOT_ENV = "KANNA_E2E_BILLING_REVIEW_SCREENSHOT_PATH";

export type ReviewerCredentialSource = "environment" | "app-store-connect";

export interface ReviewerCredentials {
  email: string;
  password: string;
  source: ReviewerCredentialSource;
}

export interface BillingReviewReportSummary {
  ready: boolean;
  blockers: string[];
  accountState?: string;
  price?: string | null;
  screenshotPath?: string;
  [key: string]: unknown;
}

export interface BillingReviewResult {
  configChecks: MobileQaCheck[];
  credentialSource: ReviewerCredentialSource | null;
  command: MobileQaCommandResult | null;
  report: BillingReviewReportSummary | null;
  screenshotPath: string;
}

export function buildBillingReviewCommand(repoRoot: string): MobileQaCommand {
  return {
    name: "billing-review",
    command: "pnpm",
    args: ["--dir", join(repoRoot, "apps", "mobile"), "run", "test:e2e:billing-review"]
  };
}

/**
 * The reviewer account, from the existing protected selectors only.
 *
 * Explicitly exported `KANNA_E2E_CLOUD_EMAIL` / `KANNA_E2E_CLOUD_PASSWORD`
 * (the mobile cloud E2E's own credential selectors) win. Otherwise the account
 * is read from the App Store Connect review detail of the current marketing
 * version, through the same `APP_STORE_CONNECT_API_KEY_ID` /
 * `APP_STORE_CONNECT_API_ISSUER_ID` key `kd mobile publish` already uses. The
 * value is never printed and reaches only the E2E subprocess environment.
 */
export async function resolveReviewerCredentials(input: {
  env: NodeJS.ProcessEnv;
  fromAppStoreConnect: () => Promise<{ email?: string; password?: string } | null>;
}): Promise<ReviewerCredentials> {
  const email = input.env[BILLING_REVIEW_EMAIL_ENV]?.trim();
  const password = input.env[BILLING_REVIEW_PASSWORD_ENV];
  if (email && password) {
    return { email, password, source: "environment" };
  }
  if (email || password) {
    throw new Error(
      `${BILLING_REVIEW_EMAIL_ENV} and ${BILLING_REVIEW_PASSWORD_ENV} must be exported together to select the App Review account explicitly.`
    );
  }
  const detail = await input.fromAppStoreConnect();
  const ascEmail = detail?.email?.trim();
  const ascPassword = detail?.password;
  if (!ascEmail || !ascPassword) {
    throw new Error(
      "App Store Connect has no review demo account recorded for this version. " +
        `Enter it in the App Review Information for the version, or export ${BILLING_REVIEW_EMAIL_ENV} and ${BILLING_REVIEW_PASSWORD_ENV}.`
    );
  }
  return { email: ascEmail, password: ascPassword, source: "app-store-connect" };
}

export function parseBillingReviewReport(stdout: string): BillingReviewReportSummary | null {
  for (const line of stdout.split(/\r?\n/).reverse()) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{")) continue;
    try {
      const parsed = JSON.parse(trimmed) as { billingReview?: unknown };
      const report = asRecord(parsed.billingReview);
      if (typeof report.ready === "boolean" && Array.isArray(report.blockers)) {
        return {
          ...report,
          ready: report.ready,
          blockers: report.blockers.map((blocker) => String(blocker))
        };
      }
    } catch {
      // Not the report line; keep scanning.
    }
  }
  return null;
}

export async function executeProductionBillingReview(input: {
  repoRoot: string;
  env: NodeJS.ProcessEnv;
  keyPath?: string;
  screenshotPath: string;
  runner: CommandRunner;
  resolveAppStoreReviewerAccount: () => Promise<{ email?: string; password?: string } | null>;
}): Promise<BillingReviewResult> {
  if (!isAbsolute(input.screenshotPath) || !input.screenshotPath.toLowerCase().endsWith(".png")) {
    throw new Error(
      `--screenshot-path must be an absolute .png path under the calling task's .tmp directory, got ${JSON.stringify(input.screenshotPath)}.`
    );
  }
  const configPath = join(input.repoRoot, "apps", "mobile", "src", "mobileEnvironments.json");
  const configChecks = validateProductionMobileConfig(JSON.parse(await readFile(configPath, "utf8")));
  const commandEnv: NodeJS.ProcessEnv = {
    ...input.env,
    KANNA_APP_ENV: "prod",
    [BILLING_REVIEW_SCREENSHOT_ENV]: input.screenshotPath
  };
  if (input.keyPath !== undefined) {
    commandEnv[MOBILE_OTA_PRIVATE_KEY_ENV] = input.keyPath;
  }
  resolveMobileOtaPrivateKeyPath(commandEnv, { required: true });
  if (configChecks.some((check) => !check.ok)) {
    return { configChecks, credentialSource: null, command: null, report: null, screenshotPath: input.screenshotPath };
  }

  const credentials = await resolveReviewerCredentials({
    env: input.env,
    fromAppStoreConnect: input.resolveAppStoreReviewerAccount
  });
  commandEnv[BILLING_REVIEW_EMAIL_ENV] = credentials.email;
  commandEnv[BILLING_REVIEW_PASSWORD_ENV] = credentials.password;

  const command = buildBillingReviewCommand(input.repoRoot);
  const result = await input.runner.run(command.command, command.args, {
    cwd: input.repoRoot,
    env: commandEnv,
    streamOutput: true
  });
  return {
    configChecks,
    credentialSource: credentials.source,
    command: { ...command, exitCode: result.exitCode, stdout: result.stdout, stderr: result.stderr },
    report: parseBillingReviewReport(result.stdout),
    screenshotPath: input.screenshotPath
  };
}

export function isProductionBillingReviewOk(result: BillingReviewResult): boolean {
  return result.configChecks.every((check) => check.ok) &&
    result.command?.exitCode === 0 &&
    result.report?.ready === true;
}

export function formatProductionBillingReviewResult(result: BillingReviewResult): string {
  const lines = [
    "Production mobile billing review capture",
    ...result.configChecks.map((check) =>
      `${check.ok ? "PASS" : "FAIL"} config:${check.name} ${check.detail}`
    )
  ];
  if (!result.command) {
    lines.push("SKIP billing-review: production mobile config sanity failed");
  } else {
    lines.push(
      `reviewer account: selected from ${result.credentialSource ?? "<unresolved>"} (value not shown)`,
      `${result.command.exitCode === 0 ? "PASS" : "FAIL"} ${result.command.name}: ${result.command.command} ${result.command.args.join(" ")}`
    );
    if (result.report) {
      lines.push(
        `screenshot: ${result.report.screenshotPath ?? result.screenshotPath}`,
        `ready: ${result.report.ready ? "yes" : "no"}${result.report.blockers.length > 0 ? ` (blockers: ${result.report.blockers.join(", ")})` : ""}`,
        `report: ${JSON.stringify(result.report)}`
      );
    } else {
      lines.push("report: none (the capture did not print a billing review report)");
    }
  }
  lines.push(
    "",
    "This capture is independent of the task-list smoke and waives nothing: the full production QA gate, TestFlight purchase/restore acceptance, and the human physical-device check remain required."
  );
  return lines.join("\n");
}

/**
 * Read the review demo account App Store Connect records for the current
 * marketing version of the production app, through the existing ASC key
 * selectors. Returns null when Apple has no review detail for that version.
 */
export async function readAppStoreReviewerAccount(input: {
  env: NodeJS.ProcessEnv;
  repoRoot: string;
  home?: string;
  http?: AppStoreConnectHttpRunner;
}): Promise<{ email?: string; password?: string } | null> {
  const credentials = await resolveAppStoreConnectCredentials(input.env, {
    command: "mobile billing-review",
    home: input.home
  });
  const version = (await readFile(join(input.repoRoot, "apps", "mobile", "VERSION"), "utf8")).trim();
  const client = new AppStoreConnectClient({ credentials, http: input.http });
  const appId = await client.findAppId(resolveKdEnvironment("prod").iosBundleId);
  const appStoreVersion = await client.findAppStoreVersion({ appId, version });
  if (!appStoreVersion) {
    throw new Error(
      `App Store Connect has no App Store version ${version} for the production app, so no review demo account can be read from it.`
    );
  }
  const detail = await client.findAppStoreReviewDetail({ appStoreVersionId: appStoreVersion.id });
  if (!detail) return null;
  return { email: detail.demoAccountName, password: detail.demoAccountPassword };
}
