/** TEST ONLY trust anchors and signing key, never imported by src. */
import { readFileSync } from "node:fs";
import { sign, X509Certificate } from "node:crypto";
import { Environment, SignedDataVerifier } from "@apple/app-store-server-library";
import { verifierWithTrust } from "../../src/billing/appStoreVerification.js";

export const appleEnv = { APP_STORE_APP_ID: "123456789", APP_STORE_GROUP_ID: "12345" };
const read = (name: string) => readFileSync(new URL(`../fixtures/apple/${name}`, import.meta.url));
const root = read("root.pem");
export const fixtureVerifier = () => verifierWithTrust({ appId: 123456789, groupId: "12345",
  bundleId: "build.kanna.app", productId: "build.kanna.cloud.monthly" }, {
  production: new SignedDataVerifier([root], false, Environment.PRODUCTION, "build.kanna.app", 123456789),
  sandbox: new SignedDataVerifier([root], false, Environment.SANDBOX, "build.kanna.app", 123456789),
});
export function appleJws(payload: unknown): string {
  const header = { alg: "ES256", x5c: ["leaf.pem", "intermediate.pem", "root.pem"].map(name => new X509Certificate(read(name)).raw.toString("base64")) };
  const message = [header, payload].map(value => Buffer.from(JSON.stringify(value)).toString("base64url")).join(".");
  return `${message}.${sign("sha256", Buffer.from(message), { key: read("leaf.key"), dsaEncoding: "ieee-p1363" }).toString("base64url")}`;
}
export function transactionClaims(token = "11111111-1111-4111-8111-111111111111", overrides: Record<string, unknown> = {}) {
  return { bundleId: "build.kanna.app", productId: "build.kanna.cloud.monthly", subscriptionGroupIdentifier: "12345",
    originalTransactionId: "100000001", transactionId: "100000002", appAccountToken: token,
    type: "Auto-Renewable Subscription", inAppOwnershipType: "PURCHASED", environment: "Sandbox",
    purchaseDate: Date.now() - 60_000, expiresDate: Date.now() + 3_600_000, signedDate: Date.now(), ...overrides };
}
export function renewalClaims(overrides: Record<string, unknown> = {}) {
  return { environment: "Sandbox", originalTransactionId: "100000001", productId: "build.kanna.cloud.monthly",
    autoRenewProductId: "build.kanna.cloud.monthly", autoRenewStatus: 1, signedDate: Date.now(), ...overrides };
}
export function notificationJws(tx: Record<string, unknown>, renewal: Record<string, unknown>, overrides: Record<string, unknown> = {}) {
  return appleJws({ notificationType: "DID_RENEW", notificationUUID: "22222222-2222-4222-8222-222222222222", signedDate: Date.now(),
    data: { bundleId: "build.kanna.app", appAppleId: 123456789, environment: tx.environment,
      status: 1, signedTransactionInfo: appleJws(tx), signedRenewalInfo: appleJws(renewal) }, ...overrides });
}
