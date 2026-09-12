// @vitest-environment happy-dom

import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DesktopAuthSession, DesktopAuthState } from "../../services/desktopAuth";

const authSession = {
  initialize: vi.fn(),
  getState: vi.fn(),
  subscribe: vi.fn(),
  signInWithEmailPassword: vi.fn(),
  signOut: vi.fn(),
  getIdToken: vi.fn(),
};

const portal = vi.hoisted(() => ({
  baseUrl: "http://127.0.0.1:5173",
  openUrl: vi.fn(),
}));

vi.mock("../../services/desktopAuthSdk", () => ({
  getConfiguredDesktopAuthSession: vi.fn(async () => authSession),
  getConfiguredDesktopPortalBaseUrl: vi.fn(async () => portal.baseUrl),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: portal.openUrl,
}));

vi.mock("vue-i18n", () => ({
  useI18n: () => ({ t: (key: string) => key, locale: { value: "en" } }),
}));

vi.mock("../../invoke", () => ({
  invoke: vi.fn(async () => ({
    state: "stopped",
    desktopId: "desktop-current",
    desktopName: "Studio Mac",
  })),
}));

function signedOutSession(): DesktopAuthSession {
  let listener: ((state: DesktopAuthState) => void) | null = null;
  const session = authSession as unknown as DesktopAuthSession;
  authSession.initialize.mockResolvedValue(undefined);
  authSession.getState.mockReturnValue({ status: "signedOut" });
  authSession.subscribe.mockImplementation((next: (state: DesktopAuthState) => void) => {
    listener = next;
    next({ status: "signedOut" });
    return () => undefined;
  });
  authSession.signInWithEmailPassword.mockImplementation(async ({ email }) => {
    listener?.({
      status: "signedIn",
      user: { uid: "user-1", email, displayName: null },
    });
  });
  authSession.signOut.mockImplementation(async () => {
    listener?.({ status: "signedOut" });
    return { desktopCredentialError: null };
  });
  authSession.getIdToken.mockResolvedValue("id-token");
  return session;
}

function mountPreferences() {
  return mount(PreferencesPanel, {
    props: {
      preferences: {
        suspendAfterMinutes: 5,
        killAfterMinutes: 30,
        ideCommand: "code",
        locale: "en",
        devLingerTerminals: false,
        defaultAgentProvider: "claude",
        defaultAgentType: "pty",
        appTheme: "dark",
        codeTheme: "match",
        agentMessageAppearance: "chat",
      },
    },
    global: {
      mocks: {
        $t: (key: string) => key,
      },
    },
  });
}

let PreferencesPanel: typeof import("../PreferencesPanel.vue").default;

describe("PreferencesPanel account sign-in", () => {
  beforeEach(async () => {
    vi.resetModules();
    vi.clearAllMocks();
    portal.baseUrl = "http://127.0.0.1:5173";
    portal.openUrl.mockResolvedValue(undefined);
    signedOutSession();
    PreferencesPanel = (await import("../PreferencesPanel.vue")).default;
  });

  it("signs in with email and password from the Account tab", async () => {
    const wrapper = mountPreferences();
    await flushPromises();

    await wrapper.get('[data-testid="preferences-account-tab"]').trigger("click");
    await wrapper.get('[data-testid="account-email"]').setValue("upvote.sieve.7t@icloud.com");
    await wrapper.get('[data-testid="account-password"]').setValue("password123");
    await wrapper.get('[data-testid="account-sign-in"]').trigger("submit");
    await flushPromises();

    expect(authSession.signInWithEmailPassword).toHaveBeenCalledWith({
      email: "upvote.sieve.7t@icloud.com",
      password: "password123",
    });
    expect(wrapper.text()).toContain("upvote.sieve.7t@icloud.com");
  });

  it("toggles password visibility on the Account tab", async () => {
    const wrapper = mountPreferences();
    await flushPromises();

    await wrapper.get('[data-testid="preferences-account-tab"]').trigger("click");
    const passwordInput = wrapper.get<HTMLInputElement>('[data-testid="account-password"]');

    expect(passwordInput.element.type).toBe("password");

    await wrapper.get('[data-testid="account-toggle-password"]').trigger("click");
    expect(passwordInput.element.type).toBe("text");

    await wrapper.get('[data-testid="account-toggle-password"]').trigger("click");
    expect(passwordInput.element.type).toBe("password");
  });

  it("warns when sign-out left this desktop claimed by the previous account", async () => {
    authSession.signOut.mockResolvedValue({
      desktopCredentialError: "cannot release this desktop: its local credential is unavailable",
    });

    const wrapper = mountPreferences();
    await flushPromises();

    await wrapper.get('[data-testid="preferences-account-tab"]').trigger("click");
    await wrapper.get('[data-testid="account-email"]').setValue("upvote.sieve.7t@icloud.com");
    await wrapper.get('[data-testid="account-password"]').setValue("password123");
    await wrapper.get('[data-testid="account-sign-in"]').trigger("submit");
    await flushPromises();

    await wrapper.get('[data-testid="account-sign-out"]').trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("this desktop was not released from the previous account");
    expect(wrapper.text()).toContain("its local credential is unavailable");
  });

  it("shows the current desktop id on the Account tab", async () => {
    const wrapper = mountPreferences();
    await flushPromises();

    await wrapper.get('[data-testid="preferences-account-tab"]').trigger("click");

    expect(wrapper.text()).toContain("Desktop ID");
    expect(wrapper.text()).toContain("desktop-current");
  });

  it.each([
    ["development", "http://127.0.0.1:5173"],
    ["staging", "https://kanna-staging-account.web.app"],
    ["production", "https://kanna-build-account.web.app"],
  ])("opens the %s portal registration route for signed-out users", async (_environment, baseUrl) => {
    portal.baseUrl = baseUrl;
    const wrapper = mountPreferences();
    await flushPromises();

    await wrapper.get('[data-testid="preferences-account-tab"]').trigger("click");
    await wrapper.get('[data-testid="account-create"]').trigger("click");
    await flushPromises();

    expect(portal.openUrl).toHaveBeenCalledExactlyOnceWith(`${baseUrl}/register`);
  });

  it.each([
    ["development", "http://127.0.0.1:5173"],
    ["staging", "https://kanna-staging-account.web.app"],
    ["production", "https://kanna-build-account.web.app"],
  ])("opens the %s portal account route for signed-in users", async (_environment, baseUrl) => {
    portal.baseUrl = baseUrl;
    const wrapper = mountPreferences();
    await flushPromises();

    await wrapper.get('[data-testid="preferences-account-tab"]').trigger("click");
    await wrapper.get('[data-testid="account-email"]').setValue("upvote.sieve.7t@icloud.com");
    await wrapper.get('[data-testid="account-password"]').setValue("password123");
    await wrapper.get('[data-testid="account-sign-in"]').trigger("submit");
    await flushPromises();

    expect(wrapper.text()).toContain("Sign in with your Kanna account.");
    await wrapper.get('[data-testid="account-manage-subscription"]').trigger("click");
    await flushPromises();

    expect(portal.openUrl).toHaveBeenCalledExactlyOnceWith(`${baseUrl}/account`);
  });
  it("preserves an active session on Mobile re-entry and prevents duplicate pairing submissions", async () => {
    const { invoke } = await import("../../invoke");
    let resolvePairing!: (value: unknown) => void;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "create_mobile_pairing_session") return await new Promise(resolve => { resolvePairing = resolve; });
      return { state: "running", desktopName: "Studio Mac", pairingCode: null };
    });
    const wrapper = mountPreferences();
    await flushPromises();
    await wrapper.get('[data-testid="preferences-mobile-tab"]').trigger("click");
    await flushPromises();
    const button = wrapper.get('[data-testid="mobile-access-start-pairing"]');
    await button.trigger("click");
    await button.trigger("click");
    expect(vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === "create_mobile_pairing_session")).toHaveLength(1);
    expect(button.attributes("disabled")).toBeDefined();
    resolvePairing({ code: "ABC123", pairingPayload: "payload", expiresAtUnixMs: Date.now() + 300000 });
    await flushPromises();
    await wrapper.get('[data-testid="preferences-account-tab"]').trigger("click");
    await wrapper.get('[data-testid="preferences-mobile-tab"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="mobile-access-pairing-code"]').text()).toBe("ABC123");
    wrapper.unmount();
  });

  it("shows a pairing failure without changing confirmed server status and allows retry", async () => {
    const { invoke } = await import("../../invoke");
    let failed = false;
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "create_mobile_pairing_session") {
        if (!failed) { failed = true; throw new Error("pairing rejected"); }
        return { code: "ABC123", pairingPayload: "payload", expiresAtUnixMs: Date.now() + 300000 };
      }
      return { state: "running" };
    });
    const wrapper = mountPreferences();
    await flushPromises();
    await wrapper.get('[data-testid="preferences-mobile-tab"]').trigger("click");
    await flushPromises();
    await wrapper.get('[data-testid="mobile-access-start-pairing"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="mobile-access-status"]').text()).toBe("mobileAccess.ready");
    expect(wrapper.text()).toContain("mobileAccess.pairingFailed");
    await wrapper.get('[data-testid="mobile-access-start-pairing"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="mobile-access-pairing-code"]').text()).toBe("ABC123");
    wrapper.unmount();
  });

  it("discards account probes after account changes and sign-out", async () => {
    const { invoke } = await import("../../invoke");
    const pending: Array<(value: unknown) => void> = [];
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "mobile_push_registration_status") return await new Promise(resolve => pending.push(resolve));
      return { state: "running" };
    });
    const wrapper = mountPreferences();
    await flushPromises();
    const setAuth = authSession.subscribe.mock.calls[0][0];
    setAuth({ status: "signedIn", user: { uid: "first", email: "first@example.com" } });
    await wrapper.get('[data-testid="preferences-mobile-tab"]').trigger("click");
    await flushPromises();
    setAuth({ status: "signedIn", user: { uid: "second", email: "second@example.com" } });
    await flushPromises();
    expect(pending).toHaveLength(2);
    pending[0]({ status: "registered", registeredDeviceCount: 9 });
    await flushPromises();
    expect(wrapper.get('[data-testid="mobile-access-push-registration"]').attributes("data-status")).toBe("loading");
    setAuth({ status: "signedOut" });
    pending[1]({ status: "registered", registeredDeviceCount: 2 });
    await flushPromises();
    expect(wrapper.get('[data-testid="mobile-access-push-registration"]').attributes("data-status")).toBe("signedOut");
    wrapper.unmount();
  });

  it("shows status loading and failure, then recovers through explicit retry", async () => {
    const { invoke } = await import("../../invoke");
    let rejectStatus!: (error: Error) => void;
    vi.mocked(invoke).mockImplementation(async () => await new Promise((_resolve, reject) => { rejectStatus = reject; }));
    const wrapper = mountPreferences();
    await flushPromises();
    await wrapper.get('[data-testid="preferences-mobile-tab"]').trigger("click");
    expect(wrapper.get('[data-testid="mobile-access-status"]').text()).toBe("mobileAccess.checking");
    rejectStatus(new Error("status offline"));
    await flushPromises();
    expect(wrapper.text()).toContain("mobileAccess.statusFailed");
    expect(wrapper.get('[data-testid="mobile-access-status"]').text()).toBe("mobileAccess.unavailable");
    await wrapper.get('[data-testid="mobile-access-troubleshooting-toggle"]').trigger("click");
    expect(wrapper.text()).toContain("status offline");
    vi.mocked(invoke).mockResolvedValue({ state: "running" });
    await wrapper.get('[data-testid="mobile-access-status-refresh"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="mobile-access-status"]').text()).toBe("mobileAccess.ready");
    expect(wrapper.text()).not.toContain("status offline");
    wrapper.unmount();
  });

});
