import type { Purchase, ProductSubscriptionIOS } from "expo-iap";
export const APPLE_MONTHLY_PRODUCT = "build.kanna.cloud.monthly";
export interface ApplePurchaseState {
  ready: boolean; busy: boolean; pending: boolean; price: string | null; message: string;
}
export const initialAppleState: ApplePurchaseState = { ready: false, busy: false, pending: false, price: null, message: "" };
export type AppleStore = Pick<typeof import("expo-iap"), "initConnection" | "endConnection" | "fetchProducts" |
  "getAvailablePurchases" | "getPendingTransactionsIOS" | "restorePurchases" | "requestPurchase" |
  "finishTransaction" | "purchaseUpdatedListener" | "purchaseErrorListener" | "showManageSubscriptionsIOS">;
export interface AppleBillingClient {
  begin(uid: string): Promise<{ appAccountToken: string; productId: string }>;
  register(uid: string, signedTransaction: string): Promise<void>;
}

/** One controller owns the StoreKit listener for the app lifetime. Account
 * generations fence every asynchronous result, including finishTransaction. */
export function createAppleController(store: AppleStore, client: AppleBillingClient,
  publish: (state: ApplePurchaseState) => void, refresh: () => Promise<void>) {
  let state = { ...initialAppleState };
  let account: { uid: string; verified: boolean } | null = null;
  let generation = 0;
  let disposed = false;
  let processing = new Map<string, Promise<void>>();
  type Operation = { generation: number };
  let inFlight: Operation | null = null;
  // Only the native request survives account changes, until StoreKit delivers
  // its result. Verification after that event belongs to the account generation.
  let awaitingPurchase: Operation | null = null;
  const set = (patch: Partial<ApplePurchaseState>) => { if (!disposed) { state = { ...state, ...patch }; publish(state); } };
  const release = (owner: Operation) => {
    if (inFlight === owner && awaitingPurchase !== owner) { inFlight = null; set({ busy: false }); }
  };
  const current = (version: number, uid: string) => !disposed && generation === version && account?.uid === uid;
  const errorMessage = (error: unknown) => {
    const reason = (error as { details?: { reason?: string } })?.details?.reason;
    return reason === "apple_account_conflict"
      ? "This purchase belongs to another or deleted Kanna account. Sign in to the original account or contact support@tampopomyoko.com."
      : error instanceof Error ? error.message : "Purchase could not be verified. Use Restore Purchases to retry.";
  };
  async function process(purchase: Purchase): Promise<void> {
    if (purchase.productId !== APPLE_MONTHLY_PRODUCT || !account?.verified) return;
    const uid = account.uid, version = generation;
    if (purchase.purchaseState === "pending") { set({ pending: true, message: "Purchase awaiting Apple approval. Access will update after confirmation." }); return; }
    if (purchase.purchaseState !== "purchased" || !purchase.purchaseToken) throw new Error("Apple transaction is unavailable. Restore purchases to retry.");
    const key = `${version}:${purchase.id}`;
    const existing = processing.get(key);
    if (existing) return existing;
    const work = (async () => {
      await client.register(uid, purchase.purchaseToken!);
      if (!current(version, uid)) return;
      await store.finishTransaction({ purchase, isConsumable: false });
      if (!current(version, uid)) return;
      await refresh();
      if (current(version, uid)) set({ pending: false, message: "Purchase verified. Your account billing is up to date." });
    })();
    processing.set(key, work);
    try { await work; } finally { processing.delete(key); }
  }
  const updates = store.purchaseUpdatedListener(purchase => {
    if (purchase.productId !== APPLE_MONTHLY_PRODUCT) return;
    const version = generation;
    // Hand off the native request to a distinct verification owner so neither
    // the request's return nor stale listener cleanup can release another lock.
    const owner = awaitingPurchase || !inFlight ? { generation: version } : null;
    awaitingPurchase = null;
    if (owner) { inFlight = owner; set({ busy: true }); }
    void process(purchase).catch(error => { if (version === generation) set({ message: errorMessage(error) }); })
      .finally(() => { if (owner) release(owner); });
  });
  const errors = store.purchaseErrorListener(error => {
    const owner = awaitingPurchase;
    awaitingPurchase = null;
    if (owner) release(owner);
    if (inFlight || (owner && owner.generation !== generation)) return;
    if (error.code === "user-cancelled") set({ busy: false, pending: false, message: "Purchase canceled." });
    else if ((error.code === "deferred-payment" || error.code === "pending")) set({ busy: false, pending: true, message: "Purchase awaiting Apple approval." });
    else set({ busy: false, message: errorMessage(error) });
  });
  async function reconcile(sync: boolean) {
    if (!account?.verified) return;
    const version = generation, uid = account.uid;
    if (sync) await store.restorePurchases();
    if (!current(version, uid)) return;
    const purchases = [...await store.getPendingTransactionsIOS(), ...await store.getAvailablePurchases({ onlyIncludeActiveItemsIOS: true })];
    for (const purchase of new Map(purchases.map(p => [p.id, p])).values()) {
      if (!current(version, uid)) return;
      await process(purchase);
    }
  }
  async function operation(run: (owner: Operation) => Promise<void>, requiresVerifiedAccount = true) {
    if (inFlight || !state.ready || (requiresVerifiedAccount && !account?.verified)) return;
    const version = generation;
    const owner = { generation: version };
    inFlight = owner; set({ busy: true, message: "" });
    try { await run(owner); }
    catch (error) {
      if (awaitingPurchase === owner) awaitingPurchase = null;
      if (inFlight === owner && version === generation) set({ message: errorMessage(error) });
    }
    finally { release(owner); }
  }
  return {
    async start() {
      if (inFlight || disposed) return;
      const version = generation;
      const owner = { generation: version };
      inFlight = owner;
      try {
        await store.initConnection();
        if (disposed) { await store.endConnection(); return; }
        set({ ready: true });
        const products = await store.fetchProducts({ skus: [APPLE_MONTHLY_PRODUCT], type: "subs" });
        const product = products?.find(p => p.id === APPLE_MONTHLY_PRODUCT) as ProductSubscriptionIOS | undefined;
        // The only admitted catalog is monthly; a misconfigured ASC product
        // must not be advertised using an invented period or price.
        const valid = product?.subscriptionPeriodUnitIOS?.toLowerCase() === "month" && product.subscriptionPeriodNumberIOS === "1";
        set({ ready: true, price: valid ? product!.displayPrice : null,
          message: valid ? "" : "Subscriptions are unavailable in this storefront. You can still restore purchases." });
        await reconcile(false);
      } catch (error) { if (version === generation) set({ message: errorMessage(error) }); }
      finally { release(owner); }
    },
    setAccount(next: { uid: string; verified: boolean } | null) {
      if (next?.uid === account?.uid && next?.verified === account?.verified) return;
      generation++; account = next; processing = new Map(); inFlight = awaitingPurchase;
      set({ busy: !!awaitingPurchase, pending: false, message: "" });
      if (state.ready) void operation(() => reconcile(false));
    },
    async purchase() {
      if (state.pending || !state.price) return;
      await operation(async owner => {
        const version = generation, uid = account!.uid;
        await reconcile(false); // Foreign/unknown purchases refuse a fresh charge.
        if (!current(version, uid)) return;
        const preflight = await client.begin(uid);
        if (!current(version, uid)) return;
        if (preflight.productId !== APPLE_MONTHLY_PRODUCT) throw new Error("Subscription product is unavailable.");
        awaitingPurchase = owner;
        await store.requestPurchase({ type: "subs", request: { apple: { sku: preflight.productId,
          appAccountToken: preflight.appAccountToken, andDangerouslyFinishTransactionAutomatically: false } } });
      });
    },
    restore: () => operation(() => reconcile(true)),
    resume: () => operation(() => reconcile(false)),
    manage: () => operation(async () => { await store.showManageSubscriptionsIOS(); await reconcile(false); await refresh(); }, false),
    dispose() { disposed = true; generation++; updates.remove(); errors.remove(); return store.endConnection(); },
  };
}
