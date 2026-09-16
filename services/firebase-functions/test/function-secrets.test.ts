/**
 * The deployed functions' Secret Manager bindings.
 *
 * A deployed 2nd-gen function's environment is populated from declared secrets
 * and committed `.env` parameters. Dropping a secret binding or parameter
 * declaration does not fail a normal build or emulator request, so these
 * assertions pin both deployment channels explicitly.
 *
 * No emulator is needed: `src/index.ts` builds its deployment manifest and
 * initializes Firebase Admin without making a credentialed request at import
 * time.
 */
import { copyFileSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { declaredParams } from "firebase-functions/params";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  CHECKOUT_SECRET_ENVS,
  PORTAL_SECRET_ENVS,
  STRIPE_PORTAL_CONFIGURATION_PARAM,
  STRIPE_PRODUCT_ID_PARAM,
  resolvePortalConfig,
  DELETE_ACCOUNT_SECRET_ENVS,
  PORTAL_BASE_URL_PARAM,
  STRIPE_WEBHOOK_SECRET_ENVS,
  resolveCheckoutConfig,
  resolveWebhookConfig,
} from "../src/billing/config.js";
import { appStoreConfig } from "../src/billing/appStoreConfig.js";
import * as functions from "../src/index.js";

/** The deployment manifest firebase-functions attaches to every v2 handler. */
interface DeployedFunction {
  __endpoint: { secretEnvironmentVariables?: { key: string }[] };
}

function boundSecrets(name: keyof typeof functions): string[] {
  const endpoint = (functions[name] as unknown as DeployedFunction).__endpoint;
  return (endpoint.secretEnvironmentVariables ?? []).map((entry) => entry.key);
}

function envFor(names: readonly string[]): NodeJS.ProcessEnv {
  return Object.fromEntries(names.map((name) => [name, `value-for-${name}`]));
}

describe("deployed function secret bindings", () => {
  it("deploys exactly the intended account and billing functions and no stray endpoint", () => {
    const deployed = Object.entries(functions)
      .filter(([, value]) => typeof value === "function")
      .map(([name]) => name)
      .sort();
    expect(deployed).toEqual(["appStoreNotifications", "beginAppStorePurchase", "createCheckoutSession", "createPortalSession", "deleteAccount", "registerAppStoreTransaction", "stripeWebhook"]);
  });

  it("scopes Apple and Stripe keys to their provider I/O", () => {
    expect(boundSecrets("appStoreNotifications")).toEqual([]);
    expect(boundSecrets("registerAppStoreTransaction")).toEqual(["APP_STORE_PRIVATE_KEY"]);
    expect(boundSecrets("beginAppStorePurchase")).toEqual(["STRIPE_SECRET_KEY"]);
  });

  it("binds createCheckoutSession to its declared Secret Manager entries", () => {
    expect(boundSecrets("createCheckoutSession")).toEqual([...CHECKOUT_SECRET_ENVS]);
    expect(boundSecrets("createCheckoutSession")).toEqual(["STRIPE_SECRET_KEY"]);
  });

  it("binds Customer Portal to only the Stripe API key and declares its configuration", () => {
    expect(boundSecrets("createPortalSession")).toEqual([...PORTAL_SECRET_ENVS]);
    expect(boundSecrets("createPortalSession")).toEqual(["STRIPE_SECRET_KEY"]);
    expect(declaredParams.map((param) => param.name)).toContain("STRIPE_PORTAL_CONFIGURATION_ID");
    expect(STRIPE_PORTAL_CONFIGURATION_PARAM.options.default).toBe("");
    expect(readFileSync(join(import.meta.dirname, "..", ".env"), "utf8")).toContain("STRIPE_PORTAL_CONFIGURATION_ID=\n");
    expect(() => resolvePortalConfig({
      STRIPE_SECRET_KEY: "sk_test_mocked", KANNA_PORTAL_BASE_URL: "https://account.example.test",
      STRIPE_PRODUCT_ID: "prod_test",
    })).toThrow("STRIPE_PORTAL_CONFIGURATION_ID");
  });

  it("declares the Kanna Cloud product id as an environment-specific parameter, unconfigured by default", () => {
    expect(declaredParams.map((param) => param.name)).toContain("STRIPE_PRODUCT_ID");
    expect(STRIPE_PRODUCT_ID_PARAM.options.default).toBe("");
    expect(readFileSync(join(import.meta.dirname, "..", ".env"), "utf8")).toContain("STRIPE_PRODUCT_ID=\n");
    expect(() => resolvePortalConfig({
      STRIPE_SECRET_KEY: "sk_test_mocked", KANNA_PORTAL_BASE_URL: "https://account.example.test",
      STRIPE_PORTAL_CONFIGURATION_ID: "bpc_test",
    })).toThrow("STRIPE_PRODUCT_ID");
  });

  it("declares the portal URL as a required Firebase string parameter", () => {
    expect(declaredParams.map((param) => param.name)).toContain("KANNA_PORTAL_BASE_URL");
    expect(PORTAL_BASE_URL_PARAM.options.default).toBeUndefined();
    expect(PORTAL_BASE_URL_PARAM.options.input).toEqual(
      expect.objectContaining({ text: expect.objectContaining({ nonEmpty: true }) })
    );
  });

  it.each([
    [".env", "https://kanna-build-account.web.app"],
    [".env.kanna-staging", "https://kanna-staging-account.web.app"],
  ])("commits %s with the portal parameter", (filename, expectedUrl) => {
    const contents = readFileSync(join(import.meta.dirname, "..", filename), "utf8");
    expect(contents).toContain(`KANNA_PORTAL_BASE_URL=${expectedUrl}`);
  });

  it("binds stripeWebhook to its signing secret and the read-only ownership-lookup key", () => {
    expect(boundSecrets("stripeWebhook")).toEqual([...STRIPE_WEBHOOK_SECRET_ENVS]);
    expect(boundSecrets("stripeWebhook")).toEqual(["STRIPE_WEBHOOK_SECRET", "STRIPE_SECRET_KEY"]);
  });

  describe("effective per-project parameters under firebase-tools dotenv layering", () => {
    /**
     * firebase-tools resolves a deploy's or emulator's parameters by layering
     * `.env`, then `.env.<projectId>`, then `.env.<alias>`, then (emulators
     * only) `.env.local`, later files winning. Raw file contents cannot show
     * what a project actually receives, so these cases run the real loader
     * from the pinned firebase-tools over the committed files.
     */
    interface UserEnvsOptions {
      functionsSource: string;
      projectId: string;
      projectAlias?: string;
      isEmulator?: boolean;
    }
    const { loadUserEnvs } = createRequire(import.meta.url)("firebase-tools/lib/functions/env.js") as {
      loadUserEnvs: (opts: UserEnvsOptions) => Record<string, string>;
    };

    const COMMITTED_ENV_FILES = [".env", ".env.kanna-build", ".env.kanna-staging"] as const;
    const APPLE_SELECTORS = {
      APP_STORE_APP_ID: "6802176590",
      APP_STORE_GROUP_ID: "22390376",
      APP_STORE_KEY_ID: "L258DQNKB6",
      APP_STORE_ISSUER_ID: "210ba685-e121-42fa-ad73-35e401031777",
    } as const;
    const UNCONFIGURED_APPLE_SELECTORS = {
      APP_STORE_APP_ID: "",
      APP_STORE_GROUP_ID: "",
      APP_STORE_KEY_ID: "",
      APP_STORE_ISSUER_ID: "",
    } as const;
    const SHARED_STRIPE_PARAMS = { STRIPE_PORTAL_CONFIGURATION_ID: "", STRIPE_PRODUCT_ID: "" } as const;

    // Only the committed dotenv files: a developer's untracked `.env.local`
    // must not leak into the clean-emulator case.
    let functionsSource = "";
    beforeAll(() => {
      functionsSource = mkdtempSync(join(tmpdir(), "kanna-functions-env-"));
      for (const filename of COMMITTED_ENV_FILES) {
        copyFileSync(join(import.meta.dirname, "..", filename), join(functionsSource, filename));
      }
    });
    afterAll(() => rmSync(functionsSource, { recursive: true, force: true }));

    const effectiveEnv = (opts: Omit<UserEnvsOptions, "functionsSource">) =>
      loadUserEnvs({ functionsSource, ...opts });

    it.each([
      ["by project id, as kd cloud deploy passes it", { projectId: "kanna-build" }],
      ["by the .firebaserc production alias", { projectId: "kanna-build", projectAlias: "production" }],
    ])("resolves the production Apple selectors for kanna-build %s", (_label, opts) => {
      const env = effectiveEnv(opts);
      expect(env).toEqual({
        KANNA_PORTAL_BASE_URL: "https://kanna-build-account.web.app",
        ...SHARED_STRIPE_PARAMS,
        ...APPLE_SELECTORS,
      });
      expect(appStoreConfig(env)).toEqual(expect.objectContaining({ appId: 6802176590, groupId: "22390376" }));
    });

    it("does not let kanna-staging inherit the production Apple selectors", () => {
      const env = effectiveEnv({ projectId: "kanna-staging" });
      expect(env).toEqual({
        KANNA_PORTAL_BASE_URL: "https://kanna-staging-account.web.app",
        ...SHARED_STRIPE_PARAMS,
        ...UNCONFIGURED_APPLE_SELECTORS,
      });
      expect(() => appStoreConfig(env)).toThrow("APP_STORE_APP_ID");
    });

    it("does not let a clean kanna-local emulator inherit the production Apple selectors", () => {
      const env = effectiveEnv({ projectId: "kanna-local", isEmulator: true });
      expect(env).toEqual({
        KANNA_PORTAL_BASE_URL: "https://kanna-build-account.web.app",
        ...SHARED_STRIPE_PARAMS,
        ...UNCONFIGURED_APPLE_SELECTORS,
      });
      expect(() => appStoreConfig(env)).toThrow("APP_STORE_APP_ID");
    });
  });

  it("binds deleteAccount only to the Stripe API key", () => {
    expect(boundSecrets("deleteAccount")).toEqual([...DELETE_ACCOUNT_SECRET_ENVS]);
    expect(boundSecrets("deleteAccount")).toEqual(["STRIPE_SECRET_KEY"]);
  });

  it("never hands createCheckoutSession the webhook signing secret", () => {
    expect(boundSecrets("createCheckoutSession")).not.toContain("STRIPE_WEBHOOK_SECRET");
  });

  describe("each secret binding list plus parameter is exactly what its resolver requires", () => {
    it("resolves the checkout config from its secret and parameters", () => {
      expect(() =>
        resolveCheckoutConfig({
          ...envFor(CHECKOUT_SECRET_ENVS),
          KANNA_PORTAL_BASE_URL: "https://portal.example.test",
          STRIPE_PRODUCT_ID: "prod_test",
        })
      ).not.toThrow();
    });

    it("resolves the webhook config from its bound entries plus the product parameter", () => {
      expect(() =>
        resolveWebhookConfig({ ...envFor(STRIPE_WEBHOOK_SECRET_ENVS), STRIPE_PRODUCT_ID: "prod_test" })
      ).not.toThrow();
    });

    it.each([...CHECKOUT_SECRET_ENVS])(
      "fails checkout when %s is the one entry missing",
      (missing) => {
        const env = {
          ...envFor(CHECKOUT_SECRET_ENVS.filter((name) => name !== missing)),
          KANNA_PORTAL_BASE_URL: "https://portal.example.test",
          STRIPE_PRODUCT_ID: "prod_test",
        };
        expect(() => resolveCheckoutConfig(env)).toThrow(missing);
      }
    );

    it("fails checkout when the portal parameter is missing", () => {
      expect(() => resolveCheckoutConfig({ ...envFor(CHECKOUT_SECRET_ENVS), STRIPE_PRODUCT_ID: "prod_test" })).toThrow(
        "KANNA_PORTAL_BASE_URL"
      );
    });

    it("fails checkout when the product parameter is missing", () => {
      expect(() => resolveCheckoutConfig({
        ...envFor(CHECKOUT_SECRET_ENVS), KANNA_PORTAL_BASE_URL: "https://portal.example.test",
      })).toThrow("STRIPE_PRODUCT_ID");
    });

    it.each([...STRIPE_WEBHOOK_SECRET_ENVS])(
      "fails the webhook when %s is the one entry missing",
      (missing) => {
        const env = {
          ...envFor(STRIPE_WEBHOOK_SECRET_ENVS.filter((name) => name !== missing)),
          STRIPE_PRODUCT_ID: "prod_test",
        };
        expect(() => resolveWebhookConfig(env)).toThrow(missing);
      }
    );

    it("fails the webhook when the product parameter is missing", () => {
      expect(() => resolveWebhookConfig(envFor(STRIPE_WEBHOOK_SECRET_ENVS))).toThrow("STRIPE_PRODUCT_ID");
    });
  });
});
