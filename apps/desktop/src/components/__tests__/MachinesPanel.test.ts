// @vitest-environment happy-dom
import { flushPromises, mount } from "@vue/test-utils";
import { createI18n } from "vue-i18n";
import { afterEach, describe, expect, it, vi } from "vitest";
import MachinesPanel from "../MachinesPanel.vue";
import en from "../../i18n/locales/en.json";
import type { DesktopPeer } from "../../services/desktopServerClient";

const qrMocks = vi.hoisted(() => ({
  renderPairingQr: vi.fn(async () => "data:image/png;base64,pairing-qr"),
  renderQrCode: vi.fn(async () => "data:image/png;base64,offer-qr"),
}));
vi.mock("../../utils/pairingQr", () => qrMocks);

const wrappers: ReturnType<typeof mount>[] = [];
function panel(props = {}) {
  const wrapper = mount(MachinesPanel, {
    props: { desktopName: "Studio Mac", desktopId: "desktop-a", peers: [], ...props },
    global: { plugins: [createI18n({ legacy: false, locale: "en", messages: { en } })] },
  });
  wrappers.push(wrapper);
  return wrapper;
}
const selector = (id: string) => `[data-testid="machines-${id}"]`;
const peer = (overrides: Partial<DesktopPeer> = {}): DesktopPeer => ({
  desktopId: "desktop-b",
  displayName: "Laptop",
  encryption: "e2ee",
  pairedAtUnixMs: 1,
  lastSeenUnixMs: null,
  transferIdentityPinned: true,
  reachable: { lan: true, relay: false },
  ...overrides,
});
afterEach(() => {
  wrappers.splice(0).forEach((w) => w.unmount());
  vi.clearAllMocks();
});

describe("MachinesPanel", () => {
  it("offers a pairing string, renders it as text and QR, and expires it", async () => {
    const w = panel();
    expect(w.find(selector("offer")).exists()).toBe(false);
    await w.get(selector("create-offer")).trigger("click");
    expect(w.emitted("create-offer")).toHaveLength(1);
    await w.setProps({
      offer: {
        desktopId: "desktop-a",
        desktopName: "Studio Mac",
        code: "ABC123",
        pairingString: "KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET",
        expiresAtUnixMs: Date.now() + 300_000,
      },
    });
    await flushPromises();
    expect(w.get(selector("offer-string")).text()).toBe("KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET");
    expect(qrMocks.renderQrCode).toHaveBeenCalledWith("KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET");
    expect(w.get(selector("offer-qr")).attributes("src")).toContain("offer-qr");
    expect(w.text()).toContain("Valid until");
    await w.setProps({
      offer: {
        desktopId: "desktop-a",
        desktopName: "Studio Mac",
        code: "ABC123",
        pairingString: "KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET",
        expiresAtUnixMs: Date.now() - 1,
      },
    });
    await flushPromises();
    expect(w.find(selector("offer")).exists()).toBe(false);
    expect(w.find(selector("offer-expired")).exists()).toBe(true);
  });

  it("pastes a string to pair and reports the outcome", async () => {
    const w = panel();
    expect(w.get(selector("pair")).attributes("disabled")).toBeDefined();
    await w.get(selector("pairing-input")).setValue("  KANNA-PEER:desktop-b:ABC123:KEY:SECRET ");
    await w.get(selector("pair")).trigger("submit");
    expect(w.emitted("pair")).toEqual([["KANNA-PEER:desktop-b:ABC123:KEY:SECRET"]]);
    await w.setProps({ pairError: "peer_identity_mismatch: the paired machine's identity changed" });
    expect(w.get(selector("pair-error")).text()).toContain("peer_identity_mismatch");
    await w.setProps({ pairError: null, pairSuccess: "Paired with Laptop (peer-lan)." });
    expect(w.get(selector("pair-success")).text()).toContain("Laptop");
    expect((w.get(selector("pairing-input")).element as HTMLInputElement).value).toBe("");
  });

  it("lists paired machines as end-to-end encrypted with reachability, and unpairs", async () => {
    const w = panel({
      peers: [
        peer(),
        peer({ desktopId: "desktop-c", displayName: "Studio", reachable: { lan: false, relay: true }, transferIdentityPinned: false }),
      ],
    });
    expect(w.find(selector("no-peers")).exists()).toBe(false);
    const laptop = w.get(selector("peer-desktop-b"));
    expect(laptop.text()).toContain("Laptop");
    expect(laptop.text()).toContain("End-to-end encrypted");
    expect(laptop.text()).toContain("Reachable on the local network");
    const studio = w.get(selector("peer-desktop-c"));
    expect(studio.text()).toContain("Reachable through the relay");
    expect(studio.text()).toContain("Transfer identity not exchanged yet");
    await w.get(selector("remove-desktop-b")).trigger("click");
    expect(w.emitted("remove-peer")).toEqual([["desktop-b"]]);
    await w.setProps({ peers: [] });
    expect(w.find(selector("no-peers")).exists()).toBe(true);
  });

  it("exposes the legacy routing switch and the channel/relay availability notes", async () => {
    const w = panel({ legacyAccessAllowed: true, peerChannelAvailable: false, relayPeerTunnelsAvailable: false });
    expect(w.find(selector("channel-unavailable")).exists()).toBe(true);
    expect(w.get(selector("create-offer")).attributes("disabled")).toBeDefined();
    expect(w.find(selector("relay-note")).exists()).toBe(true);
    const toggle = w.get(selector("legacy-toggle"));
    expect((toggle.element as HTMLInputElement).checked).toBe(true);
    await toggle.setValue(false);
    expect(w.emitted("set-legacy-access")).toEqual([[false]]);
    expect(w.text()).toContain("not end-to-end protected");
  });
});
