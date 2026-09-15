import React from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { describe, expect, it, vi } from "vitest";
import type { AppleBillingView } from "../lib/billing/useAppleBilling";
import type { BillingSource } from "../lib/billing/client";
import { AppleBillingCard } from "./AppleBillingCard";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
vi.mock("react-native", () => ({ Linking: { openURL: vi.fn() }, Pressable: "Pressable", Text: "Text", View: "View", StyleSheet: { create: (value: unknown) => value } }));
function value(sources: BillingSource[]): AppleBillingView {
  return { enabled: true, billing: { ready: true, sources },
    purchase: { ready: true, price: "CA$5.00", busy: false, pending: false, message: "" },
    refreshBilling: vi.fn(), buy: vi.fn(), restore: vi.fn(), manage: vi.fn() };
}
describe("native source-aware billing", () => {
  it.each([
    [{ source: "stripe", status: "active" }],
    [{ source: "app_store", status: "active" }],
    [{ source: "stripe", status: "active" }, { source: "app_store", status: "active" }],
    [{ source: "comp", active: true }, { source: "stripe", status: "active" }, { source: "app_store", status: "active" }],
  ] as BillingSource[][])("retains provider management independently of honored access: %j", async (...sources) => {
    const input = value(sources);
    let tree!: ReactTestRenderer;
    await act(async () => { tree = create(<AppleBillingCard value={input} verified />); });
    const text = JSON.stringify(tree.toJSON());
    expect(text).toContain("Restore Purchases");
    expect(text).not.toContain("Subscribe to Kanna Cloud");
    expect(text).not.toContain("web.app");
    if (sources.some(s => s.source === "stripe")) expect(text).toContain("Managed outside the App Store");
    if (sources.some(s => s.source === "app_store")) expect(text).toContain("Manage Apple subscription");
    if (sources.filter(s => s.status === "active").length === 2) expect(text).toContain("two paid subscriptions");
    await act(async () => tree.unmount());
  });
  it("shows only localized StoreKit metadata and never a web purchase link", async () => {
    let tree!: ReactTestRenderer;
    await act(async () => { tree = create(<AppleBillingCard value={value([])} verified />); });
    const text = JSON.stringify(tree.toJSON());
    expect(text).toContain("CA$5.00 per month");
    expect(text).toContain("Automatically renews monthly");
    expect(text).toContain("Terms of Service");
    expect(text).not.toContain("View subscription");
    expect(text).not.toContain("stripe.com");
    await act(async () => tree.unmount());
  });
});
