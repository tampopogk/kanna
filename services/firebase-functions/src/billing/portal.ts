import type { Firestore } from "firebase-admin/firestore";
import type { PortalSessionResponse } from "./contract.js";
import { resolvePortalConfig } from "./config.js";
import { BillingRequestError } from "./errors.js";
import type { StripePortalGateway } from "./stripeGateway.js";
import { accountDeletionPath, billingSourcePath, stripeCustomerPath, userDocPath } from "./types.js";

/** Hosted account management only: no purchase, cancellation policy or account writes. */
export async function createPortalSession(
  request: unknown,
  caller: { uid: string } | null,
  deps: { db: Firestore; env: NodeJS.ProcessEnv; gateway?: StripePortalGateway },
): Promise<PortalSessionResponse> {
  if (!caller) {
    throw new BillingRequestError("unauthenticated", "sign_in_required", "Sign in to manage billing.");
  }
  // No client-selected customer, uid, configuration, flow or return URL.
  if (!request || typeof request !== "object" || Array.isArray(request) || Object.keys(request).length) {
    throw new BillingRequestError("invalid-argument", "invalid_portal_request", "Billing management accepts no parameters.");
  }
  const customerId = await deps.db.runTransaction(async (transaction) => {
    const [deletion, user, stripe] = await transaction.getAll(
      deps.db.doc(accountDeletionPath(caller.uid)),
      deps.db.doc(userDocPath(caller.uid)),
      deps.db.doc(billingSourcePath(caller.uid, "stripe")),
    );
    if (deletion.exists) {
      throw new BillingRequestError("failed-precondition", "account_deleted", "This account has been permanently deleted.");
    }
    // Both fields are admin-written; Firestore rules exclude stripeCustomerId
    // from client profile writes. The source also supports pre-profile records.
    const ids = [stripe.data()?.stripeCustomerId, user.data()?.stripeCustomerId]
      .filter((id): id is string => typeof id === "string" && id.length > 0);
    const [id] = ids;
    if (!id) {
      throw new BillingRequestError("failed-precondition", "no_stripe_customer", "This account has no Stripe billing to manage.");
    }
    const mapping = await transaction.get(deps.db.doc(stripeCustomerPath(id)));
    if (ids.some((other) => other !== id) || (mapping.exists && mapping.data()?.uid !== caller.uid)) {
      throw new BillingRequestError("permission-denied", "customer_ownership_mismatch", "Could not verify billing ownership. Please contact support.");
    }
    return id;
  });

  let config: ReturnType<typeof resolvePortalConfig>;
  try {
    config = resolvePortalConfig(deps.env);
  } catch {
    throw new BillingRequestError("failed-precondition", "not_configured", "Billing management is not configured. Please contact support.");
  }
  const gateway = deps.gateway ?? (await import("./stripeGateway.js")).stripePortalGateway(config.secretKey);
  try {
    return await gateway.createPortalSession({
      customerId,
      configurationId: config.configurationId,
      returnUrl: `${config.portalBaseUrl.replace(/\/+$/, "")}/account`,
    });
  } catch {
    throw new BillingRequestError("internal", "stripe_error", "Could not open billing management. Please try again.");
  }
}
