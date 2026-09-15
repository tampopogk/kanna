import { getAuth } from "firebase/auth";
import { collection, onSnapshot } from "firebase/firestore";
import { httpsCallable } from "firebase/functions";
import { getConfiguredFunctions } from "../firebase/configuredFunctions";
import { getConfiguredFirestore } from "../firebase/configuredFirestore";

export interface BillingSource {
  source: "stripe" | "app_store" | "comp";
  status?: "active" | "grace" | "expired" | "revoked";
  active?: boolean;
  environment?: string;
  currentPeriodEndsAt?: string | null;
  cancelAtPeriodEnd?: boolean;
  paymentOutstanding?: boolean;
}
export interface BillingSnapshot { ready: boolean; sources: BillingSource[] }
export const billingUnavailable: BillingSnapshot = { ready: false, sources: [] };

export function observeBilling(uid: string, next: (value: BillingSnapshot) => void): () => void {
  return onSnapshot(collection(getConfiguredFirestore(), "users", uid, "billing"), { includeMetadataChanges: true },
    snapshot => next({ ready: !snapshot.metadata.fromCache, sources: snapshot.docs.map(doc => ({ ...doc.data(), source: doc.id } as BillingSource)) }),
    () => next(billingUnavailable));
}
export function assertBillingAccount(uid: string): void {
  if (getAuth().currentUser?.uid !== uid) throw new Error("Your account changed. Restore purchases after signing in again.");
}
export const appleBillingClient = {
  async begin(uid: string): Promise<{ appAccountToken: string; productId: string }> {
    assertBillingAccount(uid);
    const result = await httpsCallable<Record<string, never>, { appAccountToken: string; productId: string }>(getConfiguredFunctions(), "beginAppStorePurchase")({});
    assertBillingAccount(uid);
    return result.data;
  },
  async register(uid: string, signedTransaction: string): Promise<void> {
    assertBillingAccount(uid);
    const result = await httpsCallable<{ signedTransaction: string }, { outcome: string }>(getConfiguredFunctions(), "registerAppStoreTransaction")({ signedTransaction });
    assertBillingAccount(uid);
    if (!["accepted", "duplicate", "inactive"].includes(result.data.outcome)) throw new Error("Verification is incomplete. Restore purchases to retry.");
  },
};
