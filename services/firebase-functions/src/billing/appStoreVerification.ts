import { readFileSync } from "node:fs";
import {
  Environment, SignedDataVerifier, VerificationException, VerificationStatus,
  type JWSTransactionDecodedPayload, type JWSRenewalInfoDecodedPayload,
  type ResponseBodyV2DecodedPayload,
} from "@apple/app-store-server-library";
import { appStoreConfig } from "./appStoreConfig.js";
import { BillingRequestError } from "./errors.js";

export type AppleEnvironment = "production" | "sandbox";
export interface AppleTransaction {
  environment: AppleEnvironment;
  originalId: string;
  transactionId: string;
  token: string;
  purchaseDate: number;
  expiresDate: number;
  signedDate: number;
  revoked: boolean;
}
export interface AppleRenewal {
  signedDate: number;
  autoRenew: boolean;
  billingRetry: boolean;
  graceDate: number | null;
}
export interface AppleEvidence {
  transaction: AppleTransaction;
  renewal: AppleRenewal;
  status: number;
  signedDate: number;
}
export interface AppleNotification {
  environment: AppleEnvironment;
  id: string;
  type: string;
  evidence: AppleEvidence | null;
}
export interface AppleVerifier {
  transaction(jws: string): Promise<AppleTransaction>;
  pair(transaction: string, renewal: string, environment: AppleEnvironment): Promise<Omit<AppleEvidence, "status" | "signedDate">>;
  notification(jws: string): Promise<AppleNotification>;
}
export const invalidApple = () => new BillingRequestError("invalid-argument", "invalid_apple_transaction", "Apple could not verify this subscription.");
export const appleRetry = () => new BillingRequestError("internal", "apple_retry_required", "Apple subscription verification is unavailable. Please retry or restore purchases.");
const idPattern = /^[0-9]{1,80}$/;
const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
export function timestamp(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value <= 0 || value > 8.64e15) throw invalidApple();
  return value;
}
export function appleEnvironment(value: unknown): AppleEnvironment {
  if (value === Environment.PRODUCTION) return "production";
  if (value === Environment.SANDBOX) return "sandbox";
  throw invalidApple();
}
export function appleStatus(value: unknown): number {
  if (typeof value !== "number" || ![1, 2, 3, 4, 5].includes(value)) throw invalidApple();
  return value;
}
export function signedInput(value: unknown): string {
  if (typeof value !== "string" || value.length > 100_000 || value.split(".").length !== 3) throw invalidApple();
  return value;
}

// Build copies these trust anchors into dist/resources for Functions. No remote key
// URLs, decoded-claim routing, local StoreKit signatures, or verification flag.
export function appleRoots(): Buffer[] {
  return ["AppleIncRootCertificate", "AppleRootCA-G2", "AppleRootCA-G3"].map(name =>
    readFileSync(new URL(`../../resources/apple/${name}.cer`, import.meta.url)));
}

export function createAppleVerifier(env: NodeJS.ProcessEnv): AppleVerifier {
  const config = appStoreConfig(env);
  const roots = appleRoots();
  const verifiers = {
    production: new SignedDataVerifier(roots, true, Environment.PRODUCTION, config.bundleId, config.appId),
    sandbox: new SignedDataVerifier(roots, true, Environment.SANDBOX, config.bundleId, config.appId),
  };
  return verifierWithTrust(config, verifiers);
}

/** Trust injection is code-only for certificate fixtures; deployment uses the factory above. */
export function verifierWithTrust(config: ReturnType<typeof appStoreConfig>, verifiers: Record<AppleEnvironment, SignedDataVerifier>): AppleVerifier {
  async function verify<T>(operation: (verifier: SignedDataVerifier) => Promise<T>): Promise<T> {
    for (const environment of ["production", "sandbox"] as const) {
      try { return await operation(verifiers[environment]); }
      catch (error) {
        if (!(error instanceof VerificationException)) throw appleRetry();
        if (error.status === VerificationStatus.RETRYABLE_VERIFICATION_FAILURE) throw appleRetry();
        if (![VerificationStatus.INVALID_ENVIRONMENT, VerificationStatus.INVALID_APP_IDENTIFIER].includes(error.status) || environment === "sandbox") throw invalidApple();
      }
    }
    throw invalidApple();
  }
  function transaction(data: JWSTransactionDecodedPayload): AppleTransaction {
    if (data.bundleId !== config.bundleId || data.productId !== config.productId
      || data.subscriptionGroupIdentifier !== config.groupId || data.type !== "Auto-Renewable Subscription"
      || data.inAppOwnershipType !== "PURCHASED" || !idPattern.test(data.originalTransactionId ?? "")
      || !idPattern.test(data.transactionId ?? "") || !uuidPattern.test(data.appAccountToken ?? "")) throw invalidApple();
    return { environment: appleEnvironment(data.environment), originalId: data.originalTransactionId!,
      transactionId: data.transactionId!, token: data.appAccountToken!.toLowerCase(),
      purchaseDate: timestamp(data.purchaseDate), expiresDate: timestamp(data.expiresDate),
      signedDate: timestamp(data.signedDate), revoked: data.revocationDate !== undefined };
  }
  function renewal(data: JWSRenewalInfoDecodedPayload, tx: AppleTransaction): AppleRenewal {
    if (appleEnvironment(data.environment) !== tx.environment || data.originalTransactionId !== tx.originalId
      || data.productId !== config.productId || (data.autoRenewProductId && data.autoRenewProductId !== config.productId)
      || (data.appAccountToken && data.appAccountToken.toLowerCase() !== tx.token)
      || ![0, 1].includes(data.autoRenewStatus ?? -1)) throw invalidApple();
    return { signedDate: timestamp(data.signedDate), autoRenew: data.autoRenewStatus === 1,
      billingRetry: data.isInBillingRetryPeriod === true,
      graceDate: data.gracePeriodExpiresDate === undefined ? null : timestamp(data.gracePeriodExpiresDate) };
  }
  const api: AppleVerifier = {
    async transaction(jws) { return transaction(await verify(v => v.verifyAndDecodeTransaction(signedInput(jws)))); },
    async pair(txJws, renewalJws, environment) {
      const tx = await api.transaction(txJws);
      const decoded = await verify(v => v.verifyAndDecodeRenewalInfo(signedInput(renewalJws)));
      if (tx.environment !== environment) throw invalidApple();
      return { transaction: tx, renewal: renewal(decoded, tx) };
    },
    async notification(jws) {
      const data: ResponseBodyV2DecodedPayload = await verify(v => v.verifyAndDecodeNotification(signedInput(jws)));
      if (!uuidPattern.test(data.notificationUUID ?? "") || !data.notificationType) throw invalidApple();
      if (!data.data) {
        // Summary-only extension notifications carry no per-subscription grant.
        if (!data.summary || data.summary.bundleId !== config.bundleId) throw invalidApple();
        return { id: data.notificationUUID!.toLowerCase(), type: data.notificationType,
          environment: appleEnvironment(data.summary.environment), evidence: null };
      }
      const environment = appleEnvironment(data.data.environment);
      if (data.data.bundleId !== config.bundleId || (environment === "production" && data.data.appAppleId !== config.appId)) throw invalidApple();
      // Verify every nested signed value, including on notifications we ignore.
      const tx = data.data.signedTransactionInfo ? await api.transaction(data.data.signedTransactionInfo) : null;
      if (tx && tx.environment !== environment) throw invalidApple();
      let pair: Awaited<ReturnType<AppleVerifier["pair"]>> | null = null;
      if (data.data.signedRenewalInfo) {
        const decoded = await verify(v => v.verifyAndDecodeRenewalInfo(signedInput(data.data!.signedRenewalInfo)));
        if (!tx) throw invalidApple();
        pair = { transaction: tx, renewal: renewal(decoded, tx) };
      }
      const handled = new Set(["SUBSCRIBED", "DID_RENEW", "DID_FAIL_TO_RENEW", "EXPIRED", "GRACE_PERIOD_EXPIRED",
        "REFUND", "REVOKE", "REFUND_REVERSED", "DID_CHANGE_RENEWAL_STATUS", "RENEWAL_EXTENDED", "DID_CHANGE_RENEWAL_PREF"]);
      let evidence: AppleEvidence | null = null;
      if (handled.has(data.notificationType)) {
        if (!pair) throw invalidApple();
        evidence = { ...pair, status: appleStatus(data.data.status), signedDate: timestamp(data.signedDate) };
      }
      return { id: data.notificationUUID!.toLowerCase(), environment, type: data.notificationType, evidence };
    },
  };
  return api;
}
