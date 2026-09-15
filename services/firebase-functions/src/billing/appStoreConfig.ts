import { defineString } from "firebase-functions/params";
import { BillingConfigError, requireEnv } from "./config.js";

export const APP_STORE_PRODUCT_ID = "build.kanna.cloud.monthly";
export const APP_STORE_BUNDLE_ID = "build.kanna.app";
export const APP_STORE_SECRET = "APP_STORE_PRIVATE_KEY";
// Optional at deployment so an unconfigured Apple channel cannot disable Stripe.
for (const name of ["APP_STORE_APP_ID", "APP_STORE_GROUP_ID", "APP_STORE_KEY_ID", "APP_STORE_ISSUER_ID"]) {
  defineString(name, { default: "", description: `Apple subscription configuration: ${name}` });
}

export function appStoreConfig(env: NodeJS.ProcessEnv) {
  const appId = Number(requireEnv(env, "APP_STORE_APP_ID"));
  if (!Number.isSafeInteger(appId) || appId <= 0) throw new BillingConfigError("APP_STORE_APP_ID");
  return { appId, groupId: requireEnv(env, "APP_STORE_GROUP_ID"),
    bundleId: APP_STORE_BUNDLE_ID, productId: APP_STORE_PRODUCT_ID };
}
