import { useEffect, useRef, useState } from "react";
import { AppState } from "react-native";
import type { AuthState } from "../../state/sessionStore";
import { appleBillingClient, billingUnavailable, observeBilling, type BillingSnapshot } from "./client";
import { createAppleController, initialAppleState } from "./appleController";

export function useAppleBilling(enabled: boolean, auth: AuthState, refresh: () => Promise<void>) {
  const [readVersion, setReadVersion] = useState(0);
  const [purchase, setPurchase] = useState(initialAppleState);
  const [observed, setObserved] = useState<{ uid: string | null; value: BillingSnapshot }>({ uid: null, value: billingUnavailable });
  const controller = useRef<ReturnType<typeof createAppleController> | null>(null);
  const refreshRef = useRef(refresh); refreshRef.current = refresh;
  const uid = auth.status === "signedIn" ? auth.user.uid : null;
  const verified = auth.status === "signedIn" && auth.user.emailVerified === true;
  const accountRef = useRef({ uid, verified }); accountRef.current = { uid, verified };
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    void import("expo-iap").then(store => {
      if (!live) return;
      const next = createAppleController(store, appleBillingClient, setPurchase, () => refreshRef.current());
      controller.current = next;
      const account = accountRef.current;
      next.setAccount(account.uid ? { uid: account.uid, verified: account.verified } : null);
      void next.start();
    }).catch(() => { if (live) setPurchase({ ...initialAppleState, message: "This build cannot connect to Apple purchases. Install the current native app." }); });
    const foreground = AppState.addEventListener("change", state => { if (state === "active") void controller.current?.resume(); });
    return () => { live = false; foreground.remove(); controller.current?.dispose(); controller.current = null; };
  }, [enabled]);
  useEffect(() => {
    controller.current?.setAccount(uid ? { uid, verified } : null);
    setObserved({ uid, value: billingUnavailable });
    if (!enabled || !uid) return;
    let live = true;
    let stop: (() => void) | undefined;
    try { stop = observeBilling(uid, value => { if (live) setObserved({ uid, value }); }); }
    catch { setObserved({ uid, value: billingUnavailable }); }
    return () => { live = false; stop?.(); };
  }, [enabled, uid, verified, readVersion]);
  return { enabled, billing: observed.uid === uid ? observed.value : billingUnavailable, purchase,
    refreshBilling: () => { setReadVersion(version => version + 1); void controller.current?.start(); },
    buy: () => { void controller.current?.purchase(); },
    restore: () => { void controller.current?.restore(); },
    manage: () => { void controller.current?.manage(); },
  };
}
export type AppleBillingView = ReturnType<typeof useAppleBilling>;
