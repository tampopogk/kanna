import type { Firestore } from "firebase-admin/firestore";
import { applyEntitlement, readBillingState } from "./entitlement.js";
import { accountDeletionPath, appAccountTokenPath, billingSourcePath, userDocPath, type BilledSourceState } from "./types.js";
import { appleRetry, type AppleEvidence, type AppleNotification } from "./appStoreVerification.js";
import { BillingRequestError } from "./errors.js";

export const appleConflict = () => new BillingRequestError("permission-denied", "apple_account_conflict",
  "This Apple purchase belongs to another or deleted Kanna account. Sign in to the original account or contact support@tampopomyoko.com.");
export interface AppleSubscription extends BilledSourceState {
  uid: string;
  token: string;
  transactionId: string;
  purchaseDate: number;
  transactionSignedDate: number;
  renewalSignedDate: number;
  evidenceSignedDate: number;
}
export const subscriptionKey = (evidence: AppleEvidence) => `${evidence.transaction.environment}_${evidence.transaction.originalId}`;
const honored = (state: BilledSourceState) => state.status === "active" || state.status === "grace";

export function appleSource(evidence: AppleEvidence, uid: string, now: string, eventId: string | null): AppleSubscription {
  const { transaction: tx, renewal, status } = evidence;
  const nowMs = Date.parse(now);
  const state = tx.revoked || status === 5 ? "revoked"
    : status === 4 && renewal.graceDate !== null && renewal.graceDate > nowMs ? "grace"
    : status === 1 && tx.expiresDate > nowMs ? "active" : "expired";
  return {
    uid, token: tx.token, source: "app_store", status: state, environment: tx.environment,
    transactionId: tx.transactionId, purchaseDate: tx.purchaseDate,
    transactionSignedDate: tx.signedDate, renewalSignedDate: renewal.signedDate, evidenceSignedDate: evidence.signedDate,
    currentPeriodEndsAt: new Date(tx.expiresDate).toISOString(),
    graceEndsAt: renewal.graceDate === null ? null : new Date(renewal.graceDate).toISOString(),
    cancelAtPeriodEnd: !renewal.autoRenew,
    // Explicit expired status retires the subscription when retry has ended,
    // even if the old renewal preference remains on. Conversely, status 3
    // itself means billing retry; an omitted retry flag must not allow a charge.
    paymentOutstanding: [1, 3, 4].includes(status) || renewal.billingRetry || (status === 5 && renewal.autoRenew),
    appStoreOriginalTransactionId: tx.originalId, stripeCustomerId: null, stripeSubscriptionId: null,
    lastEventAt: new Date(evidence.signedDate).toISOString(), lastEventId: eventId, updatedAt: now,
  };
}

/** Never let an older transaction or either older signed component undo a revoke.
 * Equal evidence can only remove access, never grant it. */
export function shouldApplyApple(previous: AppleSubscription | undefined, next: AppleSubscription): boolean {
  if (!previous) return true;
  if (next.purchaseDate < previous.purchaseDate || next.transactionSignedDate < previous.transactionSignedDate
    || next.renewalSignedDate < previous.renewalSignedDate || next.evidenceSignedDate < previous.evidenceSignedDate) return false;
  if (next.transactionId !== previous.transactionId && next.purchaseDate === previous.purchaseDate) return false;
  const newer = next.transactionSignedDate > previous.transactionSignedDate
    || next.renewalSignedDate > previous.renewalSignedDate || next.evidenceSignedDate > previous.evidenceSignedDate;
  if (previous.status === "revoked" && next.status !== "revoked" && next.transactionSignedDate <= previous.transactionSignedDate) return false;
  if (!newer) {
    const rank = { revoked: 0, expired: 1, grace: 2, active: 3 };
    return rank[next.status] < rank[previous.status];
  }
  return true;
}

export function projectApple(records: AppleSubscription[], revision: number): BilledSourceState {
  const sorted = [...records].sort((a, b) => Number(honored(b)) - Number(honored(a))
    || Number(b.environment === "production") - Number(a.environment === "production")
    || Date.parse(b.currentPeriodEndsAt!) - Date.parse(a.currentPeriodEndsAt!)
    || a.appStoreOriginalTransactionId!.localeCompare(b.appStoreOriginalTransactionId!));
  const first = sorted[0];
  if (!first) throw appleRetry();
  // Ownership/token/order evidence stays private. The source is presentation,
  // not the complete history; independent originals remain in the ledger.
  const { uid: _uid, token: _token, transactionId: _tx, purchaseDate: _purchase,
    transactionSignedDate: _signed, renewalSignedDate: _renewal, evidenceSignedDate: _evidence, ...source } = first;
  return { ...source, paymentOutstanding: records.some(item => item.paymentOutstanding), revision };
}

export interface AppleApplyResult { outcome: "accepted" | "duplicate" | "inactive"; billing: Awaited<ReturnType<typeof readBillingState>> }
export class AppleRevisionConflict extends Error {}

export async function applyAppleEvidence(db: Firestore, uid: string, evidence: AppleEvidence[], options: {
  now: string; notification?: AppleNotification; expectedRevision?: number;
}): Promise<AppleApplyResult> {
  return db.runTransaction(async transaction => {
    const state = await readBillingState(db, uid, transaction);
    const [deletion, user, records] = await Promise.all([
      transaction.get(db.doc(accountDeletionPath(uid))), transaction.get(db.doc(userDocPath(uid))),
      transaction.get(db.collection("appStoreSubscriptions").where("uid", "==", uid)),
    ]);
    if (deletion.exists || !user.exists) throw appleConflict();
    const notificationRef = options.notification
      ? db.doc(`appleNotifications/${options.notification.environment}_${options.notification.id}`) : null;
    const event = notificationRef ? await transaction.get(notificationRef) : null;
    if (event?.exists) return { outcome: "duplicate", billing: state };
    if (options.expectedRevision !== undefined && (state.sources.app_store?.revision ?? 0) !== options.expectedRevision) throw new AppleRevisionConflict();
    const updates = new Map<string, AppleSubscription>();
    for (const item of evidence) {
      const [token, original] = await transaction.getAll(db.doc(appAccountTokenPath(item.transaction.token)),
        db.doc(`appStoreSubscriptions/${subscriptionKey(item)}`));
      if (token.data()?.uid !== uid || (original.exists && original.data()?.uid !== uid)) throw appleConflict();
      const next = appleSource(item, uid, options.now, options.notification?.id ?? null);
      if (shouldApplyApple(original.data() as AppleSubscription | undefined, next)) updates.set(subscriptionKey(item), next);
    }
    const all = new Map(records.docs.map(doc => [doc.id, doc.data() as AppleSubscription]));
    for (const [key, next] of updates) all.set(key, next);
    if (notificationRef) transaction.set(notificationRef, { uid, type: options.notification!.type,
      environment: options.notification!.environment, signedDate: evidence[0]?.signedDate ?? null });
    if (updates.size === 0) return { outcome: "duplicate", billing: state };
    for (const [key, next] of updates) transaction.set(db.doc(`appStoreSubscriptions/${key}`), next);
    const source = projectApple([...all.values()], (state.sources.app_store?.revision ?? 0) + 1);
    transaction.set(db.doc(billingSourcePath(uid, "app_store")), source);
    const sources = { ...state.sources, app_store: source };
    const derived = applyEntitlement({ db, uid, transaction, sources, previous: state.previous,
      defaultEnvironment: source.environment, now: options.now });
    return { outcome: honored(source) ? "accepted" : "inactive", billing: { sources, previous: derived.entitlement } };
  });
}
