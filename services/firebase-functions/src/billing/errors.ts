/**
 * Callable-shaped errors raised by the billing core.
 *
 * The core does not import `firebase-functions/https` so the emulator tests can
 * drive it directly; `src/index.ts` translates these into `HttpsError`. The
 * `reason` is the machine-readable half — the portal branches on it to render
 * "you're subscribed via the App Store" rather than a generic failure.
 */
export type BillingErrorCode =
  | "unauthenticated"
  | "permission-denied"
  | "failed-precondition"
  | "invalid-argument"
  | "internal";

export type BillingErrorReason =
  | "sign_in_required"
  | "account_deleted"
  | "checkout_in_progress"
  | "checkout_reconciliation_required"
  | "email_verification_required"
  | "comp_active"
  | "app_store_active"
  | "already_subscribed"
  | "unknown_plan"
  | "invalid_portal_request"
  | "invalid_desktop_request"
  | "no_stripe_customer"
  | "customer_ownership_mismatch"
  | "not_configured"
  | "stripe_error"
  | "invalid_apple_transaction"
  | "apple_account_conflict"
  | "apple_retry_required";

export class BillingRequestError extends Error {
  constructor(
    readonly code: BillingErrorCode,
    readonly reason: BillingErrorReason,
    message: string
  ) {
    super(message);
    this.name = "BillingRequestError";
  }
}
