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
  provenance: "verified",
  identityChanged: false,
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
  it("offers a pairing string, renders it as text and QR, and counts down to expiry", async () => {
    const w = panel();
    expect(w.find(selector("offer")).exists()).toBe(false);
    await w.get(selector("create-offer")).trigger("click");
    expect(w.emitted("create-offer")).toHaveLength(1);
    await w.setProps({
      offer: {
        desktopId: "desktop-a",
        desktopName: "Studio",
        code: "ABC123",
        pairingString: "KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET",
        expiresAtUnixMs: Date.now() + 125_000,
      },
    });
    await flushPromises();
    expect(w.get(selector("offer-string")).text()).toBe("KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET");
    expect(qrMocks.renderQrCode).toHaveBeenCalledWith("KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET");
    expect(w.get(selector("offer-qr")).attributes("src")).toContain("offer-qr");
    // A wall-clock time was unreadable as "how long have I got"; a m:ss
    // countdown is the whole label.
    expect(w.get(selector("offer-meta")).text()).toMatch(/^Expires in 2:0[45]$/);
    expect(w.text()).not.toContain("Valid until");
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
    // Expired, so the full-width button is back, offering a fresh one.
    expect(w.get(selector("create-offer")).text()).toBe("Show a new string");
  });

  // Replacing a live string is a refresh, not a second full-width button
  // competing with the one that made it.
  it("replaces a live string from a refresh control beside the countdown", async () => {
    const w = panel({
      offer: {
        desktopId: "desktop-a",
        desktopName: "Studio",
        code: "ABC123",
        pairingString: "KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET",
        expiresAtUnixMs: Date.now() + 300_000,
      },
    });
    await flushPromises();
    expect(w.find(selector("create-offer")).exists()).toBe(false);
    const refresh = w.get(selector("new-offer"));
    expect(refresh.attributes("aria-label")).toBe("Show a new string");
    await refresh.trigger("click");
    expect(w.emitted("create-offer")).toHaveLength(1);
  });

  // There is no separate copy affordance: the string is the button.
  it("copies the pairing string when the string itself is clicked", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const w = panel({
      offer: {
        desktopId: "desktop-a",
        desktopName: "Studio",
        code: "ABC123",
        pairingString: "KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET",
        expiresAtUnixMs: Date.now() + 300_000,
      },
    });
    await flushPromises();
    expect(w.find('[data-testid="machines-copy-offer"]').exists()).toBe(false);
    await w.get(selector("offer-string")).trigger("click");
    await flushPromises();
    expect(writeText).toHaveBeenCalledWith("KANNA-PEER:desktop-a:ABC123:KEYKEY:SECRET");
    expect(w.get(selector("offer-meta")).text()).toBe("Copied");
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

  it("lists paired machines with their reachability, and unpairs", async () => {
    const w = panel({
      peers: [
        peer(),
        peer({ desktopId: "desktop-c", displayName: "Studio", reachable: { lan: false, relay: true }, transferIdentityPinned: false }),
        peer({ desktopId: "desktop-d", displayName: "Mini", reachable: { lan: false, relay: false } }),
      ],
    });
    expect(w.find(selector("no-peers")).exists()).toBe(false);
    const laptop = w.get(selector("peer-desktop-b"));
    expect(laptop.text()).toContain("Laptop");
    expect(laptop.text()).toContain("Local network");
    const studio = w.get(selector("peer-desktop-c"));
    expect(studio.text()).toContain("Relay");
    expect(studio.text()).toContain("Transfer identity pending");
    const mini = w.get(selector("peer-desktop-d"));
    expect(mini.text()).toContain("Not reachable");
    expect(mini.text()).not.toContain("Transfer identity pending");
    await w.get(selector("remove-desktop-b")).trigger("click");
    expect(w.emitted("remove-peer")).toEqual([["desktop-b"]]);
    // The list polls every 3s, so there is no manual Refresh to beat it.
    expect(w.find(selector("refresh")).exists()).toBe(false);
    // The raw desktop id is a tooltip, not 36 characters under every row.
    expect(laptop.attributes("title")).toBe("desktop-b");
    expect(laptop.find(".peer-meta").text()).not.toContain("desktop-b");
    await w.setProps({ peers: [] });
    expect(w.find(selector("no-peers")).exists()).toBe(true);
  });

  it("distinguishes an account-trusted pin from a verified one, and shouts about a changed key", async () => {
    const w = panel({
      peers: [
        peer(),
        peer({ desktopId: "desktop-c", displayName: "Studio", provenance: "account" }),
        peer({ desktopId: "desktop-d", displayName: "Mini", provenance: "account", identityChanged: true }),
      ],
    });
    // A verified pin says so, and carries no notice.
    const laptop = w.get(selector("peer-desktop-b"));
    expect(laptop.get(selector("peer-provenance-desktop-b")).text()).toBe("Verified");
    expect(w.find(selector("peer-identity-changed-desktop-b")).exists()).toBe(false);

    // An automatically pinned sibling never claims to be verified.
    const studio = w.get(selector("peer-desktop-c"));
    expect(studio.get(selector("peer-provenance-desktop-c")).text()).toBe("Same account");
    expect(studio.text()).not.toContain("Verified");
    expect(w.find(selector("peer-identity-changed-desktop-c")).exists()).toBe(false);

    // A changed key is an alert with both resolutions, never a silent retry.
    const changed = w.get(selector("peer-identity-changed-desktop-d"));
    expect(changed.attributes("role")).toBe("alert");
    expect(changed.text()).toContain("Key changed");
    expect(changed.text()).toContain("Pair again");
    expect(changed.text()).toContain("unpair");
  });

  it("reports the channel and relay availability notes, and never offers a legacy escape hatch", () => {
    const healthy = panel();
    expect(healthy.find(selector("channel-unavailable")).exists()).toBe(false);
    expect(healthy.find(selector("relay-note")).exists()).toBe(false);
    expect(healthy.get(selector("create-offer")).attributes("disabled")).toBeUndefined();

    const degraded = panel({ peerChannelAvailable: false, relayPeerTunnelsAvailable: false });
    expect(degraded.find(selector("channel-unavailable")).exists()).toBe(true);
    expect(degraded.get(selector("create-offer")).attributes("disabled")).toBeDefined();
    expect(degraded.find(selector("relay-note")).exists()).toBe(true);
  });

  // The unencrypted desktop-to-desktop escape hatch was removed on
  // 2026-09-20: the server refuses those paths outright, so the pane must
  // not imply that anything here can turn them back on.
  it("has no legacy access control at all", () => {
    const w = panel({ peers: [peer()] });
    expect(w.find(selector("legacy-toggle")).exists()).toBe(false);
    expect(w.find("input[type=checkbox]").exists()).toBe(false);
    expect(w.text().toLowerCase()).not.toContain("legacy");
    expect(w.text().toLowerCase()).not.toContain("unencrypted");
  });

  it("reports loading and error states for the paired list", async () => {
    const w = panel({ peersLoading: true });
    expect(w.text()).toContain("Loading…");
    expect(w.find(selector("no-peers")).exists()).toBe(false);
    await w.setProps({ peersLoading: false, peersError: "peer trust store is unusable" });
    expect(w.text()).toContain("peer trust store is unusable");
  });

  // Kanna runs on Linux too, so nothing here may call a machine a Mac.
  it("never calls a machine a Mac", () => {
    const w = panel({
      peers: [peer(), peer({ desktopId: "desktop-c", provenance: "account" })],
      peerChannelAvailable: false,
      relayPeerTunnelsAvailable: false,
    });
    expect(w.text()).not.toMatch(/\bMacs?\b/);
    expect(JSON.stringify(en.machines)).not.toMatch(/\bMacs?\b/);
  });
});
