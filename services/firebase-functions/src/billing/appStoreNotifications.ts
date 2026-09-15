import { BillingConfigError } from "./config.js";
import { BillingRequestError } from "./errors.js";
import { applyAppleEvidence } from "./appStoreEvents.js";
import { createAppleVerifier, signedInput } from "./appStoreVerification.js";
import type { AppleDependencies } from "./appStorePurchase.js";
import { appAccountTokenPath } from "./types.js";
import { consoleBillingLogger, type BillingLogger } from "./logger.js";

export async function handleAppStoreNotification(body: unknown, deps: AppleDependencies & { logger?: BillingLogger }) {
  const logger = deps.logger ?? consoleBillingLogger;
  try {
    const verifier = deps.verifier ?? createAppleVerifier(deps.env);
    const notification = await verifier.notification(signedInput((body as { signedPayload?: unknown } | null)?.signedPayload));
    if (!notification.evidence) return { httpStatus: 200, code: "ignored" };
    const token = await deps.db.doc(appAccountTokenPath(notification.evidence.transaction.token)).get();
    const uid: unknown = token.data()?.uid;
    if (typeof uid !== "string") {
      logger.warn("Apple notification has no account binding", { eventId: notification.id, environment: notification.environment });
      return { httpStatus: 200, code: "unresolved_account" };
    }
    const result = await applyAppleEvidence(deps.db, uid, [notification.evidence], {
      now: deps.now?.() ?? new Date().toISOString(), notification,
    });
    logger.info("Apple notification processed", { eventId: notification.id, environment: notification.environment, outcome: result.outcome });
    return { httpStatus: 200, code: result.outcome };
  } catch (error) {
    if (error instanceof BillingRequestError) {
      if (error.reason === "apple_account_conflict") return { httpStatus: 200, code: "unresolved_account" };
      if (error.reason === "invalid_apple_transaction") return { httpStatus: 400, code: error.reason };
    }
    // Never log JWS, tokens, private key, or provider error bodies. Transient
    // failures have no dedupe write and must remain retryable by Apple.
    logger.error("Apple notification requires retry", { reason: error instanceof BillingConfigError ? "not_configured" : "verification_or_storage" });
    return { httpStatus: 503, code: "retry_required" };
  }
}
