import React from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { describe, expect, it, vi } from "vitest";
import { Linking } from "react-native";
import type { AppleBillingView } from "../lib/billing/useAppleBilling";
import type { BillingSource } from "../lib/billing/client";
import { AppleBillingCard } from "./AppleBillingCard";
import { MOBILE_E2E_IDS } from "../e2eTestIds";

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
    expect(text).toContain("Apple Standard EULA");
    expect(text).not.toContain("View subscription");
    expect(text).not.toContain("stripe.com");
    const eulaLink = tree.root.findAllByType("Pressable").find(node =>
      node.findAllByType("Text").some(textNode => textNode.props.children === "Apple Standard EULA"));
    expect(eulaLink).toBeDefined();
    await act(async () => { eulaLink?.props.onPress(); });
    expect(Linking.openURL).toHaveBeenCalledWith("https://www.apple.com/legal/internet-services/itunes/dev/stdeula/");
    await act(async () => tree.unmount());
  });
  it("exposes stable review-capture targets for the card, price, restore, and legal links", async () => {
    let tree!: ReactTestRenderer;
    await act(async () => { tree = create(<AppleBillingCard value={value([])} verified />); });
    const byTestId = (testID: string) => tree.root.findAll(node => node.props.testID === testID);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingCard)).toHaveLength(1);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingPrice)[0]?.props.children).toBe("CA$5.00 per month");
    expect(byTestId(MOBILE_E2E_IDS.appleBillingSubscribeButton)[0]?.props.disabled).toBe(false);
    // The purchase call to action renders as a filled primary button, not a text link.
    expect(byTestId(MOBILE_E2E_IDS.appleBillingSubscribeButton)[0]?.props.style).toEqual([
      expect.objectContaining({ backgroundColor: "#E8F1FF", borderRadius: 16 }),
      null
    ]);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingRestoreButton)[0]?.props.disabled).toBe(false);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingEulaLink)).toHaveLength(1);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingPrivacyLink)).toHaveLength(1);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingUnconfirmed)).toHaveLength(0);
    await act(async () => tree.unmount());
  });
  it("hides the purchase path while billing is unconfirmed and disables restore for an unverified account", async () => {
    const input = value([]);
    input.billing = { ready: false, sources: [] };
    let tree!: ReactTestRenderer;
    await act(async () => { tree = create(<AppleBillingCard value={input} verified={false} />); });
    const byTestId = (testID: string) => tree.root.findAll(node => node.props.testID === testID);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingUnconfirmed)).toHaveLength(1);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingPrice)).toHaveLength(0);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingSubscribeButton)).toHaveLength(0);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingRestoreButton)[0]?.props.disabled).toBe(true);
    await act(async () => tree.unmount());
  });
  it("reports a missing localized price as not purchasable", async () => {
    const input = value([]);
    input.purchase = { ...input.purchase, price: null };
    let tree!: ReactTestRenderer;
    await act(async () => { tree = create(<AppleBillingCard value={input} verified />); });
    const byTestId = (testID: string) => tree.root.findAll(node => node.props.testID === testID);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingPrice)[0]?.props.children).toBe("Monthly price unavailable");
    expect(byTestId(MOBILE_E2E_IDS.appleBillingSubscribeButton)[0]?.props.disabled).toBe(true);
    expect(byTestId(MOBILE_E2E_IDS.appleBillingRestoreButton)[0]?.props.disabled).toBe(false);
    await act(async () => tree.unmount());
  });
});
