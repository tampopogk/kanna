import type { Auth } from "firebase-admin/auth";
import type {
  DocumentReference,
  Firestore,
  Query,
} from "firebase-admin/firestore";
import { BillingRequestError } from "./billing/errors.js";
import { requireEnv, STRIPE_PRODUCT_ID_ENV, STRIPE_SECRET_KEY_ENV } from "./billing/config.js";
import { stripeSubscriptionGateway, type StripeSubscriptionGateway } from "./billing/stripeGateway.js";
import {
  accountCheckoutPath,
  accountDeletionPath,
  billingSourcePath,
  userDocPath,
  type BilledSourceState,
} from "./billing/types.js";

export interface DeleteAccountCaller {
  uid: string;
}

export interface AccountDeletionStore {
  markAccountDeletionStarted(uid: string): Promise<string[]>;
  stripeBillingReferences(uid: string): Promise<{
    customerIds: string[];
    /** Each subscription paired with the customer it was recorded against, when known. */
    subscriptions: { subscriptionId: string; customerId: string | null }[];
  }>;
  deleteUserTree(uid: string): Promise<void>;
  deleteBillingIndexes(uid: string): Promise<void>;
  revokeDesktopPairings(uid: string): Promise<void>;
  deleteLegacyDevicePairings(uid: string): Promise<void>;
}

export interface AccountDeletionAuth {
  revokeRefreshTokens(uid: string): Promise<void>;
  deleteUser(uid: string): Promise<void>;
}

export interface DeleteAccountDependencies {
  store: AccountDeletionStore;
  auth: AccountDeletionAuth;
  stripe: StripeSubscriptionGateway;
}

export interface DeleteAccountResult {
  deleted: true;
}

/**
 * Permanently delete one account. Each completed step can safely be repeated;
 * the Auth user remains until every cloud record has been removed, making any
 * partial failure retryable by the still-authenticated caller.
 */
export async function deleteAccount(
  caller: DeleteAccountCaller | null,
  dependencies: DeleteAccountDependencies,
): Promise<DeleteAccountResult> {
  if (!caller) {
    throw new BillingRequestError(
      "unauthenticated",
      "sign_in_required",
      "Sign in before deleting your account.",
    );
  }

  const checkoutSessionIds = await dependencies.store.markAccountDeletionStarted(caller.uid);
  const billing = await dependencies.store.stripeBillingReferences(caller.uid);
  // A single unambiguous customer id is required before it disambiguates a
  // direct reference; with more than one on record (a legacy/migration
  // artifact), only the strong uid/product check below applies.
  const soleCustomerId = billing.customerIds.length === 1 ? billing.customerIds[0] : null;
  for (const { subscriptionId, customerId } of billing.subscriptions) {
    await dependencies.stripe.cancelSubscription(subscriptionId, {
      uid: caller.uid,
      customerId: customerId ?? soleCustomerId,
    });
  }
  for (const sessionId of checkoutSessionIds) {
    await dependencies.stripe.closeCheckoutSession(sessionId, { uid: caller.uid, customerId: soleCustomerId });
  }
  for (const customerId of billing.customerIds) {
    await dependencies.stripe.closeCustomerBilling(customerId, { uid: caller.uid });
  }
  await dependencies.store.revokeDesktopPairings(caller.uid);
  await dependencies.store.deleteLegacyDevicePairings(caller.uid);
  await dependencies.store.deleteUserTree(caller.uid);
  await dependencies.store.deleteBillingIndexes(caller.uid);
  await revokeRefreshTokensIdempotently(dependencies.auth, caller.uid);
  await deleteAuthUserIdempotently(dependencies.auth, caller.uid);
  return { deleted: true };
}

export function firestoreAccountDeletionStore(db: Firestore): AccountDeletionStore {
  return {
    async markAccountDeletionStarted(uid) {
      const deletionRef = db.doc(accountDeletionPath(uid));
      const checkoutRef = db.doc(accountCheckoutPath(uid));
      return db.runTransaction(async (transaction) => {
        const checkout = await transaction.get(checkoutRef);
        const data = checkout.data() as {
          creating?: unknown;
          sessionIds?: unknown;
        } | undefined;
        if (data?.creating === true) {
          throw new BillingRequestError(
            "failed-precondition",
            "checkout_in_progress",
            "An earlier checkout must be recovered before account deletion. Retry checkout or contact support.",
          );
        }
        transaction.set(deletionRef, { uid, started: true });
        return Array.isArray(data?.sessionIds)
          ? data.sessionIds.filter((value): value is string => typeof value === "string")
          : [];
      });
    },
    async stripeBillingReferences(uid) {
      const [user, sourceSnapshot, mappings] = await Promise.all([
        db.doc(userDocPath(uid)).get(),
        db.doc(billingSourcePath(uid, "stripe")).get(),
        db.collection("stripeCustomers").where("uid", "==", uid).get(),
      ]);
      const userData = user.data() as { stripeCustomerId?: unknown } | undefined;
      const source = sourceSnapshot.data() as Partial<BilledSourceState> | undefined;
      const sourceCustomerId = typeof source?.stripeCustomerId === "string" ? source.stripeCustomerId : null;
      return {
        customerIds: uniqueNonEmptyStrings([
          userData?.stripeCustomerId,
          source?.stripeCustomerId,
          ...mappings.docs.flatMap((document) => {
            const data = document.data() as { stripeCustomerId?: unknown };
            return [document.id, data.stripeCustomerId];
          }),
        ]),
        subscriptions: uniqueNonEmptyStrings([source?.stripeSubscriptionId]).map((subscriptionId) => ({
          subscriptionId,
          customerId: sourceCustomerId,
        })),
      };
    },
    async deleteUserTree(uid) {
      await db.recursiveDelete(db.doc(userDocPath(uid)));
    },
    async deleteBillingIndexes(uid) {
      await deleteQueries(db, [
        db.collection("stripeCustomers").where("uid", "==", uid),
        db.collection("stripeEvents").where("uid", "==", uid),
        db.collection("appAccountTokens").where("uid", "==", uid),
        db.collection("appStoreSubscriptions").where("uid", "==", uid),
        db.collection("appleNotifications").where("uid", "==", uid),
      ]);
      await db.doc(accountCheckoutPath(uid)).delete();
    },
    async revokeDesktopPairings(uid) {
      await deleteQuery(db, db.collection("desktopCredentials").where("uid", "==", uid));
    },
    async deleteLegacyDevicePairings(uid) {
      await deleteQuery(db, db.collection("devices").where("userId", "==", uid));
    },
  };
}

function uniqueNonEmptyStrings(values: readonly unknown[]): string[] {
  return [...new Set(values.filter(
    (value): value is string => typeof value === "string" && value.trim().length > 0,
  ))];
}

export function accountDeletionDependencies(
  db: Firestore,
  auth: Auth,
  env: NodeJS.ProcessEnv,
): DeleteAccountDependencies {
  return {
    store: firestoreAccountDeletionStore(db),
    auth,
    stripe: stripeSubscriptionGateway(
      requireEnv(env, STRIPE_SECRET_KEY_ENV),
      requireEnv(env, STRIPE_PRODUCT_ID_ENV),
    ),
  };
}

async function deleteQueries(db: Firestore, queries: readonly Query[]): Promise<void> {
  for (const query of queries) {
    await deleteQuery(db, query);
  }
}

async function deleteQuery(db: Firestore, query: Query): Promise<void> {
  const snapshot = await query.get();
  if (snapshot.empty) return;
  const writer = db.bulkWriter();
  for (const document of snapshot.docs) {
    writer.delete(document.ref as DocumentReference);
  }
  await writer.close();
}

async function deleteAuthUserIdempotently(auth: AccountDeletionAuth, uid: string): Promise<void> {
  try {
    await auth.deleteUser(uid);
  } catch (error) {
    if (isAuthUserNotFound(error)) return;
    throw error;
  }
}

async function revokeRefreshTokensIdempotently(
  auth: AccountDeletionAuth,
  uid: string,
): Promise<void> {
  try {
    await auth.revokeRefreshTokens(uid);
  } catch (error) {
    if (isAuthUserNotFound(error)) return;
    throw error;
  }
}

function isAuthUserNotFound(error: unknown): boolean {
  return typeof error === "object"
    && error !== null
    && "code" in error
    && (error as { code?: unknown }).code === "auth/user-not-found";
}
