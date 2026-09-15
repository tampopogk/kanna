import { describe, expect, it, vi } from "vitest";
import type { Purchase, PurchaseError } from "expo-iap";
import { APPLE_MONTHLY_PRODUCT, createAppleController, type ApplePurchaseState, type AppleStore } from "./appleController";

const transaction = { id: "1", productId: APPLE_MONTHLY_PRODUCT, purchaseState: "purchased", purchaseToken: "signed.jws.token" } as Purchase;
function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
// Drain the controller's promise continuations after an explicitly ordered
// event/settlement, without timing assertions or polling for unchanged state.
const flush = () => new Promise<void>(resolve => setImmediate(resolve));
function harness() {
  let update!: (p: Purchase) => void;
  let error!: (e: PurchaseError) => void;
  const state: ApplePurchaseState[] = [];
  const remove = vi.fn();
  const store = {
    initConnection: vi.fn(async () => true), endConnection: vi.fn(async () => true),
    fetchProducts: vi.fn(async () => [{ id: APPLE_MONTHLY_PRODUCT, displayPrice: "¥500", subscriptionPeriodUnitIOS: "month", subscriptionPeriodNumberIOS: "1" }]),
    getAvailablePurchases: vi.fn(async () => [] as Purchase[]), getPendingTransactionsIOS: vi.fn(async () => [] as Purchase[]),
    restorePurchases: vi.fn(async () => undefined), requestPurchase: vi.fn(async () => undefined),
    finishTransaction: vi.fn(async () => undefined), showManageSubscriptionsIOS: vi.fn(async () => []),
    purchaseUpdatedListener: vi.fn((fn) => { update = fn; return { remove }; }),
    purchaseErrorListener: vi.fn((fn) => { error = fn; return { remove }; }),
  };
  const client = { begin: vi.fn(async () => ({ appAccountToken: "token", productId: APPLE_MONTHLY_PRODUCT })), register: vi.fn(async () => undefined) };
  const refresh = vi.fn(async () => undefined);
  const controller = createAppleController(store as unknown as AppleStore, client, value => state.push(value), refresh);
  controller.setAccount({ uid: "owner", verified: true });
  return { store, client, controller, state, refresh, remove, update: (p = transaction) => update(p), error: (code: string) => error({ code, message: code } as PurchaseError) };
}

describe("native Apple purchase lifecycle", () => {
  it("uses StoreKit price and account token, blocks repeat taps until an event and finishes only after acceptance", async () => {
    const h = harness(); await h.controller.start();
    expect(h.state.at(-1)?.price).toBe("¥500");
    let accept!: () => void;
    h.client.register.mockImplementationOnce(() => new Promise(resolve => { accept = resolve; }));
    await h.controller.purchase(); await h.controller.purchase();
    expect(h.store.requestPurchase).toHaveBeenCalledOnce();
    expect(h.store.requestPurchase).toHaveBeenCalledWith({ type: "subs", request: { apple: { sku: APPLE_MONTHLY_PRODUCT, appAccountToken: "token", andDangerouslyFinishTransactionAutomatically: false } } });
    h.update();
    await vi.waitFor(() => expect(h.client.register).toHaveBeenCalled());
    expect(h.store.finishTransaction).not.toHaveBeenCalled();
    accept();
    await vi.waitFor(() => expect(h.store.finishTransaction).toHaveBeenCalledWith({ purchase: transaction, isConsumable: false }));
    h.controller.dispose(); expect(h.remove).toHaveBeenCalledTimes(2);
  });
  it("leaves failures unfinished and retries on explicit restore, including comp/web accounts", async () => {
    const h = harness(); await h.controller.start();
    h.client.register.mockRejectedValueOnce(new Error("verification offline")); h.update();
    await vi.waitFor(() => expect(h.state.at(-1)?.message).toContain("offline"));
    expect(h.store.finishTransaction).not.toHaveBeenCalled();
    h.store.getAvailablePurchases.mockResolvedValue([transaction]);
    await h.controller.restore();
    expect(h.store.restorePurchases).toHaveBeenCalledOnce();
    expect(h.client.begin).not.toHaveBeenCalled();
    expect(h.store.finishTransaction).toHaveBeenCalledOnce();
  });
  it("recovers unfinished transactions on restart without forced sync or another charge", async () => {
    const h = harness(); h.store.getPendingTransactionsIOS.mockResolvedValue([transaction]);
    await h.controller.start();
    expect(h.store.finishTransaction).toHaveBeenCalledOnce();
    expect(h.store.restorePurchases).not.toHaveBeenCalled();
    expect(h.store.requestPurchase).not.toHaveBeenCalled();
  });
  it("does not finish or publish a prior account's in-flight verification", async () => {
    const h = harness(); await h.controller.start();
    let finish!: () => void;
    h.client.register.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
    h.update(); await vi.waitFor(() => expect(h.client.register).toHaveBeenCalled());
    h.controller.setAccount(null); finish(); await Promise.resolve(); await Promise.resolve();
    expect(h.store.finishTransaction).not.toHaveBeenCalled();
    expect(h.refresh).not.toHaveBeenCalled();
    expect(h.state.at(-1)?.message).toBe("");
  });
  it.each(["accepted", "rejected"])("recovers a purchase after account change during registration (%s)", async outcome => {
    const h = harness(); await h.controller.start();
    const registration = deferred();
    h.client.register.mockImplementationOnce(() => registration.promise);
    await h.controller.purchase(); h.update();
    expect(h.client.register).toHaveBeenCalledWith("owner", transaction.purchaseToken);
    expect(h.state.at(-1)?.busy).toBe(true);
    h.controller.setAccount(null);
    h.controller.setAccount({ uid: "other", verified: true });
    await flush();
    const beforeSettlement = [...h.state];
    if (outcome === "accepted") registration.resolve();
    else registration.reject(new Error("Old account registration failed"));
    await flush();
    expect(h.store.finishTransaction).not.toHaveBeenCalled();
    expect(h.refresh).not.toHaveBeenCalled();
    expect(h.state).toEqual(beforeSettlement);
    expect(h.state.at(-1)).toMatchObject({ ready: true, busy: false, pending: false, message: "" });

    h.controller.setAccount({ uid: "owner", verified: true });
    await flush();
    h.store.getPendingTransactionsIOS.mockResolvedValue([transaction]);
    await h.controller.restore();
    expect(h.store.restorePurchases).toHaveBeenCalledOnce();
    expect(h.client.register.mock.calls.map(([uid]) => uid)).toEqual(["owner", "owner"]);
    expect(h.store.finishTransaction).toHaveBeenCalledOnce();
    expect(h.refresh).toHaveBeenCalledOnce();
    expect(h.state.at(-1)).toMatchObject({ busy: false, pending: false });
    expect(h.store.requestPurchase).toHaveBeenCalledOnce();
    expect(h.store.purchaseUpdatedListener).toHaveBeenCalledOnce();
    expect(h.store.purchaseErrorListener).toHaveBeenCalledOnce();
    await h.controller.dispose();
  });
  it("does not let stale registration cleanup release a newer purchase lock", async () => {
    const h = harness(); await h.controller.start();
    const registration = deferred();
    h.client.register.mockImplementationOnce(() => registration.promise);
    await h.controller.purchase(); h.update();
    h.controller.setAccount(null);
    h.controller.setAccount({ uid: "other", verified: true });
    await flush();
    await h.controller.purchase();
    expect(h.store.requestPurchase).toHaveBeenCalledTimes(2);
    const beforeSettlement = [...h.state];
    registration.resolve(); await flush();
    expect(h.state).toEqual(beforeSettlement);
    expect(h.state.at(-1)?.busy).toBe(true);
    expect(h.store.finishTransaction).not.toHaveBeenCalled();
    expect(h.refresh).not.toHaveBeenCalled();
    await h.controller.purchase(); await h.controller.restore(); await h.controller.resume(); await h.controller.manage();
    expect(h.store.requestPurchase).toHaveBeenCalledTimes(2);
    expect(h.store.restorePurchases).not.toHaveBeenCalled();
    expect(h.store.showManageSubscriptionsIOS).not.toHaveBeenCalled();
    h.error("user-cancelled");
    await h.controller.restore();
    expect(h.store.restorePurchases).toHaveBeenCalledOnce();
    expect(h.state.at(-1)?.busy).toBe(false);
    await h.controller.dispose();
  });
  it("keeps an outstanding native request locked across account changes until its result", async () => {
    const h = harness(); await h.controller.start(); await h.controller.purchase();
    h.controller.setAccount(null);
    h.controller.setAccount({ uid: "other", verified: true });
    await h.controller.purchase(); await h.controller.restore(); await h.controller.start();
    expect(h.state.at(-1)?.busy).toBe(true);
    expect(h.store.requestPurchase).toHaveBeenCalledOnce();
    expect(h.store.restorePurchases).not.toHaveBeenCalled();
    expect(h.store.initConnection).toHaveBeenCalledOnce();
    h.update({ ...transaction, productId: "unrelated.product" }); await flush();
    expect(h.state.at(-1)?.busy).toBe(true);
    h.error("user-cancelled");
    expect(h.state.at(-1)).toMatchObject({ busy: false, message: "" });
    await h.controller.restore();
    expect(h.store.restorePurchases).toHaveBeenCalledOnce();
    await h.controller.dispose();
  });
  it("keeps the original account's newer restore locked when its old verification settles", async () => {
    const h = harness(); await h.controller.start();
    const oldRegistration = deferred(), restoredRegistration = deferred();
    h.client.register.mockImplementationOnce(() => oldRegistration.promise)
      .mockImplementationOnce(() => restoredRegistration.promise);
    await h.controller.purchase(); h.update();
    h.controller.setAccount(null);
    h.controller.setAccount({ uid: "owner", verified: true });
    await flush();
    h.store.getPendingTransactionsIOS.mockResolvedValue([transaction]);
    const restore = h.controller.restore(); await flush();
    expect(h.client.register).toHaveBeenCalledTimes(2);
    const beforeSettlement = [...h.state];
    oldRegistration.resolve(); await flush();
    expect(h.state).toEqual(beforeSettlement);
    expect(h.state.at(-1)?.busy).toBe(true);
    expect(h.store.finishTransaction).not.toHaveBeenCalled();
    expect(h.refresh).not.toHaveBeenCalled();
    await h.controller.restore(); await h.controller.resume();
    expect(h.store.restorePurchases).toHaveBeenCalledOnce();
    expect(h.client.register).toHaveBeenCalledTimes(2);
    restoredRegistration.resolve(); await restore;
    expect(h.store.finishTransaction).toHaveBeenCalledOnce();
    expect(h.refresh).toHaveBeenCalledOnce();
    expect(h.state.at(-1)?.busy).toBe(false);
    await h.controller.dispose();
  });
  it("keeps verification locked when the native request returns after its purchased event", async () => {
    const h = harness(); await h.controller.start();
    const request = deferred(), registration = deferred();
    h.store.requestPurchase.mockImplementationOnce(() => request.promise);
    h.client.register.mockImplementationOnce(() => registration.promise);
    const purchase = h.controller.purchase(); await flush();
    expect(h.store.requestPurchase).toHaveBeenCalledOnce();
    h.update(); request.resolve(); await purchase;
    await h.controller.purchase(); await h.controller.restore();
    expect(h.state.at(-1)?.busy).toBe(true);
    expect(h.store.requestPurchase).toHaveBeenCalledOnce();
    expect(h.store.restorePurchases).not.toHaveBeenCalled();
    registration.resolve(); await flush();
    expect(h.store.finishTransaction).toHaveBeenCalledOnce();
    expect(h.state.at(-1)?.busy).toBe(false);
    await h.controller.dispose();
  });
  it("does not charge when existing purchases conflict with the Kanna account", async () => {
    const h = harness(); await h.controller.start();
    h.store.getAvailablePurchases.mockResolvedValue([transaction]);
    h.client.register.mockRejectedValue(Object.assign(new Error("conflict"), { details: { reason: "apple_account_conflict" } }));
    await h.controller.purchase();
    expect(h.client.begin).not.toHaveBeenCalled();
    expect(h.store.requestPurchase).not.toHaveBeenCalled();
    expect(h.state.at(-1)?.message).toContain("original account");
  });
  it("handles cancellation and deferred approval without granting access", async () => {
    const h = harness(); await h.controller.start(); await h.controller.purchase(); h.error("user-cancelled");
    expect(h.state.at(-1)?.message).toBe("Purchase canceled.");
    await h.controller.purchase(); h.error("deferred-payment");
    await h.controller.purchase();
    expect(h.store.requestPurchase).toHaveBeenCalledTimes(2);
    expect(h.state.at(-1)?.pending).toBe(true);
    expect(h.store.finishTransaction).not.toHaveBeenCalled();
  });
  it("allows Apple management before deletion even when Kanna email verification is unavailable", async () => {
    const h = harness(); await h.controller.start();
    h.controller.setAccount({ uid: "owner", verified: false });
    await Promise.resolve(); await Promise.resolve();
    await h.controller.manage();
    expect(h.store.showManageSubscriptionsIOS).toHaveBeenCalledOnce();
    expect(h.client.begin).not.toHaveBeenCalled();
    expect(h.client.register).not.toHaveBeenCalled();
  });
});
