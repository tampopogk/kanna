// @vitest-environment happy-dom
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { afterEach, describe, expect, it, vi } from "vitest";
import MobileAccessPanel from "../MobileAccessPanel.vue";
import en from "../../i18n/locales/en.json";

const qrMocks = vi.hoisted(() => ({
  renderPairingQr: vi.fn(async () => "data:image/png;base64,pairing-qr"),
  renderQrCode: vi.fn(async () => "data:image/png;base64,install-qr"),
}));
vi.mock("../../utils/pairingQr", () => qrMocks);
const wrappers: ReturnType<typeof mount>[] = [];
function panel(props = {}) {
  const wrapper = mount(MobileAccessPanel, {
    props: { desktopName: "Studio Mac", serverStatus: "running", pairingCode: null, pairingPayload: null, ...props },
    global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en } })] },
  });
  wrappers.push(wrapper);
  return wrapper;
}
const selector = (id: string) => `[data-testid="mobile-access-${id}"]`;
afterEach(() => {
  wrappers.splice(0).forEach(w => w.unmount());
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("MobileAccessPanel", () => {
  it("offers signed-out local pairing immediately and only shows installation on request", async () => {
    const w = panel();
    await flushPromises();
    expect(w.text()).toContain("Ready for pairing");
    expect(w.text()).toContain("Local pairing does not require account sign-in");
    expect(w.get(selector("start-pairing")).text()).toBe("Pair a device");
    expect(w.find("img").exists()).toBe(false);
    await w.get(selector("install-toggle")).trigger("click");
    expect(w.get(selector("install-toggle")).attributes("aria-expanded")).toBe("true");
    expect(w.get(selector("install-qr")).attributes("src")).toContain("install-qr");
    expect(w.text()).toContain("phone camera to open the App Store");
    expect(w.text()).toContain("iPhone and iPad");
    expect(w.find("a").exists()).toBe(false);
    expect(w.text()).not.toContain("https://");
    expect(w.find(selector("install-copy")).exists()).toBe(false);
    await w.get(selector("start-pairing")).trigger("click");
    expect(w.emitted("start-pairing")).toHaveLength(1);
  });

  it("switches between QR codes without generating or discarding a session", async () => {
    const w = panel();
    await w.get(selector("install-toggle")).trigger("click");
    await w.setProps({ pairingCode: "ABC123", pairingPayload: "KANNA1:DESKTOP:ABC123", expiresAtUnixMs: Date.now() + 300000 });
    await flushPromises();
    expect(qrMocks.renderPairingQr).toHaveBeenCalledWith("KANNA1:DESKTOP:ABC123");
    expect(w.findAll("img")).toHaveLength(1);
    expect(w.get(selector("pairing-code")).text()).toBe("ABC123");
    expect(w.text()).toContain("Valid until");
    expect(w.text()).toContain("Scan in Kanna from Machines → Add");
    await w.get(selector("install-toggle")).trigger("click");
    expect(w.find(selector("pairing-qr")).exists()).toBe(false);
    expect(w.findAll("img")).toHaveLength(1);
    await w.get(selector("pairing-toggle")).trigger("click");
    expect(w.get(selector("pairing-code")).text()).toBe("ABC123");
    expect(w.findAll("img")).toHaveLength(1);
    expect(w.emitted("start-pairing")).toBeUndefined();
  });

  it("recovers installation rendering without a store link and preserves the unconfigured state", async () => {
    qrMocks.renderQrCode.mockRejectedValueOnce(new Error("render failed"));
    const w = panel();
    await w.get(selector("install-toggle")).trigger("click");
    await flushPromises();
    expect(w.find(selector("install-error")).exists()).toBe(true);
    await w.get(selector("install-retry")).trigger("click");
    await flushPromises();
    expect(w.find(selector("install-qr")).exists()).toBe(true);
    expect(w.find("a").exists()).toBe(false);
    await w.setProps({ environment: "unknown" });
    expect(w.find(selector("install-unconfigured")).exists()).toBe(true);
    expect(w.find("img").exists()).toBe(false);
  });

  it("keeps the manual code if pairing QR rendering fails and hides expired credentials", async () => {
    vi.useFakeTimers();
    qrMocks.renderPairingQr.mockRejectedValueOnce(new Error("bad QR"));
    const w = panel({ pairingCode: "ABC123", pairingPayload: "payload", expiresAtUnixMs: Date.now() + 1000 });
    await flushPromises();
    expect(w.text()).toContain("Enter the pairing code instead");
    expect(w.get(selector("pairing-code")).text()).toBe("ABC123");
    await vi.advanceTimersByTimeAsync(1000);
    expect(w.find(selector("pairing-code")).exists()).toBe(false);
    expect(w.text()).toContain("Code expired");
    expect(w.get(selector("start-pairing")).text()).toBe("Generate new code");
  });

  it("disables pending submissions and separates request failures from server state", async () => {
    const w = panel({ pairingPending: true });
    expect(w.get(selector("start-pairing")).attributes("disabled")).toBeDefined();
    await w.get(selector("start-pairing")).trigger("click");
    expect(w.emitted("start-pairing")).toBeUndefined();
    await w.setProps({ pairingPending: false, pairingError: "request rejected" });
    expect(w.get(selector("status")).text()).toBe("Ready for pairing");
    expect(w.text()).toContain("Could not create a pairing code");
    expect(w.text()).not.toContain("request rejected");
    await w.get(selector("troubleshooting-toggle")).trigger("click");
    expect(w.text()).toContain("request rejected");
    await w.get(selector("status-refresh")).trigger("click");
    expect(w.emitted("refresh-status")).toHaveLength(1);
  });

  it("describes account registration honestly and discloses full diagnostics", async () => {
    const w = panel({ accountSignedIn: true, pushRegistration: { status: "registered", registeredDeviceCount: 2 } });
    expect(w.text()).toContain("2 devices registered for account notifications");
    expect(w.text()).not.toContain("notifications reach");
    await w.setProps({ pushRegistrationLoading: true });
    expect(w.get(selector("push-registration")).text()).toContain("Checking notification");
    await w.setProps({ pushRegistrationLoading: false, pushRegistration: { status: "noRegisteredDevices", registeredDeviceCount: 0, noDevicesReason: { code: "tokenRejected", message: "Retired token", providerCode: "DeviceNotRegistered", retiredAt: "2026-09-01" } } });
    expect(w.text()).toContain("allow notifications");
    expect(w.text()).not.toContain("Retired token");
    await w.get(selector("troubleshooting-toggle")).trigger("click");
    expect(w.text()).toContain("Retired token");
    expect(w.text()).toContain("DeviceNotRegistered");
    await w.get(selector("push-refresh")).trigger("click");
    expect(w.emitted("refresh-push-registration")).toHaveLength(1);
    await w.setProps({ pushRegistration: { status: "unavailable", registeredDeviceCount: 0, error: "relay offline" } });
    expect(w.get(selector("push-registration")).text()).toContain("unavailable");
    expect(w.get(selector("push-registration")).text()).not.toContain("No devices");
    await w.setProps({ accountSignedIn: false });
    expect(w.get(selector("push-registration")).text()).toContain("Sign in");
    expect(w.text()).not.toContain("relay offline");
  });
});
