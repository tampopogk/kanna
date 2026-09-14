import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { User } from "firebase/auth";
import { createMemoryHistory, createRouter } from "vue-router";
import { defineComponent } from "vue";
import { providePortalSession, type PortalSession } from "../src/session";
import type { PortalFirebase } from "../src/firebase";
import type { CloudEntitlement } from "../src/types";
import AccountPage from "../src/pages/AccountPage.vue";
import CheckoutReturnPage from "../src/pages/CheckoutReturnPage.vue";
import SignInPage from "../src/pages/SignInPage.vue";
import VerifyEmailPage from "../src/pages/VerifyEmailPage.vue";

const owner = { uid: "owner", email: "owner@example.test", emailVerified: true } as User;
const active: CloudEntitlement = { status: "active", source: "stripe", capabilities: ["cloud_relay"], currentPeriodEndsAt: null, graceEndsAt: null, environment: "staging" };
const wrappers: VueWrapper[] = [];
afterEach(() => { wrappers.splice(0).forEach((w) => w.unmount()); vi.useRealTimers(); });

function harness(template = '<CheckoutReturnPage result="success" session-id="untrusted" />') {
  let authNext!: (user: User | null) => void;
  const listeners: { uid: string; next: (value: CloudEntitlement | null) => void; error: (error: Error) => void; stop: ReturnType<typeof vi.fn> }[] = [];
  const stopAuth = vi.fn();
  const api = {
    observeUser: vi.fn((next) => { authNext = next; next(owner); return stopAuth; }),
    observeEntitlement: vi.fn((uid, next, error) => { const listener = { uid, next, error, stop: vi.fn() }; listeners.push(listener); return listener.stop; }),
    resetPassword: vi.fn(async () => undefined), resendVerification: vi.fn(async () => undefined),
    reloadUser: vi.fn(async (user) => user), createPortalSession: vi.fn(async () => ({ url: "https://billing.stripe.test/owner" })),
    signIn: vi.fn(async () => owner), signOut: vi.fn(async () => undefined), deleteAccount: vi.fn(), register: vi.fn(),
    entitlement: vi.fn(), createCheckoutSession: vi.fn(),
  } satisfies PortalFirebase;
  const redirect = vi.fn();
  let session!: PortalSession;
  const host = defineComponent({
    template, components: { AccountPage, CheckoutReturnPage, SignInPage, VerifyEmailPage },
    setup() { session = providePortalSession(api); return { redirect }; },
  });
  const router = createRouter({ history: createMemoryHistory(), routes: ["/", "/subscribe", "/account", "/verify-email", "/register"].map((path) => ({ path, component: { template: "<div />" } })) });
  const wrapper = mount(host, { global: { plugins: [router] } });
  wrappers.push(wrapper);
  return { wrapper, listeners, authNext, api, session, stopAuth, redirect, router };
}

describe("authoritative access lifecycle", () => {
  it("waits through webhook delay, then renders observed activation and the setup handoff", async () => {
    const h = harness();
    expect(h.wrapper.text()).not.toContain("access is active");
    h.listeners[0].next(null);
    await flushPromises();
    expect(h.wrapper.text()).toContain("has not been confirmed yet");
    expect(h.wrapper.text()).not.toContain("untrusted");
    h.listeners[0].next(active);
    await flushPromises();
    expect(h.wrapper.text()).toContain("access is active");
    expect(h.wrapper.text()).toContain("same verified account");
    expect(h.wrapper.text()).toContain("Keep Kanna running");
    expect(h.wrapper.text()).toContain("LAN");
    expect(h.wrapper.text()).toContain("WAN");
    expect(h.wrapper.find('a[href="https://kanna.build/"]').exists()).toBe(true);
  });

  it("clears stale access on read failure, retries explicitly and rejects old listener callbacks", async () => {
    const h = harness();
    h.listeners[0].next(active);
    h.listeners[0].error(new Error("offline"));
    await flushPromises();
    expect(h.wrapper.text()).toContain("could not confirm");
    expect(h.session.subscribed.value).toBe(false);
    await h.wrapper.get("button").trigger("click");
    expect(h.listeners[0].stop).toHaveBeenCalledOnce();
    h.listeners[0].next(active);
    expect(h.session.subscribed.value).toBe(false);
    h.listeners[1].next(active);
    await flushPromises();
    expect(h.session.subscribed.value).toBe(true);
  });

  it("expires grace at its deadline without a new snapshot or network poll", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-14T00:00:00Z"));
    const h = harness();
    h.listeners[0].next({ ...active, status: "grace", graceEndsAt: "2026-09-14T00:00:01Z" });
    await flushPromises();
    expect(h.wrapper.text()).toContain("temporarily available");
    await vi.advanceTimersByTimeAsync(1000);
    expect(h.wrapper.text()).toContain("expired");
    expect(h.session.subscribed.value).toBe(false);
    expect(h.api.observeEntitlement).toHaveBeenCalledOnce();
  });

  it.each([null, "invalid"])("matches relay grace semantics with no valid deadline (%s)", (graceEndsAt) => {
    const h = harness();
    h.listeners[0].next({ ...active, status: "grace", graceEndsAt });
    expect(h.session.accessState.value).toBe("grace");
  });

  it("does not expire active access during renewal webhook latency", () => {
    const h = harness();
    h.listeners[0].next({ ...active, currentPeriodEndsAt: "2000-01-01T00:00:00Z" });
    expect(h.session.accessState.value).toBe("active");
  });

  it("clears account data synchronously on switch/sign-out and disposes every listener/timer", () => {
    vi.useFakeTimers();
    const h = harness();
    h.listeners[0].next({ ...active, status: "grace", graceEndsAt: new Date(Date.now() + 5000).toISOString() });
    h.authNext({ ...owner, uid: "second", email: "second@example.test" } as User);
    expect(h.session.entitlement.value).toBeNull();
    expect(h.listeners[0].stop).toHaveBeenCalledOnce();
    h.listeners[0].next(active);
    expect(h.session.entitlement.value).toBeNull();
    h.listeners[1].next({ ...active, source: "comp" });
    h.listeners[0].error(new Error("old failure"));
    expect(h.session.subscribed.value).toBe(true);
    h.authNext(null);
    expect(h.session.entitlement.value).toBeNull();
    expect(h.listeners[1].stop).toHaveBeenCalledOnce();
    h.wrapper.unmount(); wrappers.pop();
    expect(h.stopAuth).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
    h.authNext(owner); h.listeners[1].next(active);
    expect(h.session.entitlement.value).toBeNull();
  });
});

describe("account billing guidance", () => {
  it.each(["comp", "app_store", "grandfathered"] as const)("does not direct %s access to a purchase", async (source) => {
    const h = harness("<AccountPage />");
    h.listeners[0].next({ ...active, source });
    await flushPromises();
    expect(h.wrapper.text()).not.toContain("Choose a plan");
    expect(h.wrapper.text()).not.toContain("Manage billing");
    expect(h.wrapper.text()).not.toContain("subscription is ready");
    if (source === "comp") expect(h.wrapper.text()).toContain("No purchase is needed");
    if (source === "app_store") expect(h.wrapper.find('a[href="https://apps.apple.com/account/subscriptions"]').exists()).toBe(true);
  });

  it("opens the hosted portal for expired Stripe billing without deleting the account", async () => {
    const h = harness('<AccountPage :redirect="redirect" />');
    h.listeners[0].next({ ...active, status: "expired" });
    await flushPromises();
    await h.wrapper.findAll("button").find((button) => button.text() === "Manage billing")!.trigger("click");
    expect(h.api.createPortalSession).toHaveBeenCalledWith();
    expect(h.redirect).toHaveBeenCalledWith("https://billing.stripe.test/owner");
    expect(h.api.deleteAccount).not.toHaveBeenCalled();
  });

  it("allows comp with an existing Stripe relationship to manage that billing", async () => {
    const h = harness("<AccountPage />");
    h.listeners[0].next({ ...active, source: "comp", stripeCustomerId: "cus_owner" });
    await flushPromises();
    expect(h.wrapper.text()).toContain("Manage billing");
    expect(h.wrapper.text()).not.toContain("Choose a plan");
  });

  it("does not redirect to an old account's portal after an account switch", async () => {
    const h = harness('<AccountPage :redirect="redirect" />');
    let finish!: (value: { url: string }) => void;
    h.api.createPortalSession.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    h.listeners[0].next(active);
    await flushPromises();
    await h.wrapper.findAll("button").find((button) => button.text() === "Manage billing")!.trigger("click");
    h.authNext({ ...owner, uid: "second" } as User);
    finish({ url: "https://billing.stripe.test/owner" });
    await flushPromises();
    expect(h.redirect).not.toHaveBeenCalled();
  });
});

describe("SDK recovery actions", () => {
  it("resets the entered email without a password and displays a recoverable failure", async () => {
    const h = harness("<SignInPage />");
    const reset = h.wrapper.findAll("button").find((button) => button.text() === "Forgot password?")!;
    await reset.trigger("click");
    expect(h.api.resetPassword).not.toHaveBeenCalled();
    await h.wrapper.get('input[type="email"]').setValue("owner@example.test");
    await reset.trigger("click");
    await flushPromises();
    expect(h.api.resetPassword).toHaveBeenCalledWith("owner@example.test");
    expect(h.wrapper.text()).toContain("If an account exists");
    h.api.resetPassword.mockRejectedValueOnce(new Error("Please try again later"));
    await reset.trigger("click"); await flushPromises();
    expect(h.wrapper.get('[role="alert"]').text()).toContain("Please try again later");
  });

  it("resends to the signed-in identity and checks verification on focus without polling", async () => {
    const h = harness("<VerifyEmailPage />");
    await h.wrapper.findAll("button").find((button) => button.text() === "Resend verification email")!.trigger("click");
    expect(h.api.resendVerification).toHaveBeenCalledWith(owner);
    await flushPromises();
    expect(h.wrapper.text()).toContain("Verification email sent");
    h.api.reloadUser.mockResolvedValueOnce(owner);
    window.dispatchEvent(new Event("focus"));
    await flushPromises();
    expect(h.api.reloadUser).toHaveBeenCalledWith(owner);
    expect(h.router.currentRoute.value.path).toBe("/subscribe");
    h.wrapper.unmount(); wrappers.pop();
    window.dispatchEvent(new Event("focus"));
    expect(h.api.reloadUser).toHaveBeenCalledOnce();
  });
});
