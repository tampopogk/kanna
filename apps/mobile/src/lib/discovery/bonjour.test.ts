import { describe, expect, it } from "vitest";
import {
  applyBonjourServiceEvent,
  createNativeBonjourBrowser,
  createUnavailableBonjourBrowser,
  type BonjourService
} from "./bonjour";
import { fakeNativeBonjourModule } from "./fakeNativeBonjourModule";

// Full native Bonjour/Appium coverage requires a signed iOS app with local
// network permission and a desktop LAN server; these reducer tests cover the
// JS/native event contract that would otherwise retain stale trusted endpoints.
describe("applyBonjourServiceEvent", () => {
  it("removes a service when native Bonjour reports it as removed", async () => {
    const services = new Map<string, BonjourService>();

    applyBonjourServiceEvent(services, {
      name: "Studio Mac",
      type: "_kanna-mobile._tcp.",
      host: "studio.local",
      port: 48120,
      txt: { desktopId: "desktop-1" }
    });
    expect(Array.from(services.values())).toHaveLength(1);

    applyBonjourServiceEvent(services, {
      name: "Studio Mac",
      type: "_kanna-mobile._tcp.",
      host: "studio.local",
      port: 48120,
      txt: { desktopId: "desktop-1" },
      removed: true
    });

    expect(Array.from(services.values())).toEqual([]);
  });

  it("removes a cached service when the native removal event has no resolved endpoint", async () => {
    const services = new Map<string, BonjourService>();

    applyBonjourServiceEvent(services, {
      name: "Studio Mac",
      type: "_kanna-mobile._tcp.",
      host: "studio.local",
      port: 48120,
      txt: { desktopId: "desktop-1" }
    });

    expect(
      applyBonjourServiceEvent(services, {
        name: "Studio Mac",
        type: "_kanna-mobile._tcp.",
        txt: { desktopId: "desktop-1" },
        removed: true
      })
    ).toBe(true);
    expect(Array.from(services.values())).toEqual([]);
  });

  it("removes a cached service when the native removal endpoint differs from the resolved add", async () => {
    const services = new Map<string, BonjourService>();

    applyBonjourServiceEvent(services, {
      name: "Studio Mac",
      type: "_kanna-mobile._tcp.",
      host: "studio.local",
      port: 48120,
      txt: { desktopId: "desktop-1" }
    });

    expect(
      applyBonjourServiceEvent(services, {
        name: "Studio Mac",
        type: "_kanna-mobile._tcp.",
        host: "stale.local",
        port: 9,
        txt: { desktopId: "desktop-1" },
        removed: true
      })
    ).toBe(true);
    expect(Array.from(services.values())).toEqual([]);
  });

  it("replaces a service endpoint when the same service name resolves to a new host and port", async () => {
    const services = new Map<string, BonjourService>();

    applyBonjourServiceEvent(services, {
      name: "Studio Mac",
      type: "_kanna-mobile._tcp.",
      host: "studio-old.local",
      port: 48120,
      txt: { desktopId: "desktop-1" }
    });
    applyBonjourServiceEvent(services, {
      name: "Studio Mac",
      type: "_kanna-mobile._tcp.",
      host: "studio-new.local",
      port: 48121,
      txt: { desktopId: "desktop-1" }
    });

    expect(Array.from(services.values())).toEqual([
      {
        name: "Studio Mac",
        type: "_kanna-mobile._tcp.",
        host: "studio-new.local",
        port: 48121,
        txt: { desktopId: "desktop-1" },
        removed: false
      }
    ]);
  });
});

describe("native Bonjour browser", () => {
  const studio = {
    name: "Jeremy's Mac Studio",
    type: "_kanna-mobile._tcp.",
    host: "Jeremys-Mac-Studio.local",
    port: 48121,
    txt: { desktopId: "DESKTOP-STUDIO" }
  };

  it("waits for the requested desktop to resolve before answering", async () => {
    const native = fakeNativeBonjourModule();
    const browser = createNativeBonjourBrowser(native.module, native.Emitter);

    const refreshed = browser.refresh?.({ desktopId: "desktop-studio", timeoutMs: 1_000 });
    expect(browser.getServices()).toEqual([]);
    native.emit(studio);
    await refreshed;

    // Case-insensitive: the QR payload and the TXT record disagree on case.
    expect(browser.getServices()).toHaveLength(1);
    expect(native.module.ensureBrowsingCalls).toBe(1);
  });

  it("returns without a match when nothing advertises the requested desktop", async () => {
    const native = fakeNativeBonjourModule();
    const browser = createNativeBonjourBrowser(native.module, native.Emitter);
    native.emit({ ...studio, txt: { desktopId: "desktop-other" } });

    await browser.refresh?.({ desktopId: "DESKTOP-STUDIO", timeoutMs: 10 });

    // A timeout is not an error; the caller reports on the candidates it has.
    expect(browser.getServices()).toHaveLength(1);
  });

  it("accepts any advertised desktop for a typed pairing code", async () => {
    const native = fakeNativeBonjourModule();
    const browser = createNativeBonjourBrowser(native.module, native.Emitter);

    const refreshed = browser.refresh?.({ timeoutMs: 1_000 });
    native.emit(studio);
    await refreshed;

    expect(browser.getServices()).toHaveLength(1);
  });

  it("reports a device that cannot browse at all", async () => {
    const native = fakeNativeBonjourModule({
      ensureBrowsing: async () => {
        throw new Error("Network service discovery could not start (code 3).");
      }
    });
    const browser = createNativeBonjourBrowser(native.module, native.Emitter);

    await expect(browser.refresh?.({ desktopId: "DESKTOP-STUDIO", timeoutMs: 10 }))
      .rejects.toThrow("could not start");
  });

  it("waits out a native module that never confirms its browse", async () => {
    const native = fakeNativeBonjourModule({
      ensureBrowsing: () => new Promise<void>(() => {})
    });
    const browser = createNativeBonjourBrowser(native.module, native.Emitter);

    await expect(browser.refresh?.({ desktopId: "DESKTOP-STUDIO", timeoutMs: 10 }))
      .resolves.toBeUndefined();
  });

  it("keeps one native listener across stop and restart", () => {
    const native = fakeNativeBonjourModule();
    const browser = createNativeBonjourBrowser(native.module, native.Emitter);

    browser.start();
    browser.start();
    expect(native.module.startBrowsingCalls).toBe(1);

    browser.stop();
    expect(native.listenerCount()).toBe(0);

    browser.start();
    expect(native.listenerCount()).toBe(1);
    expect(native.module.startBrowsingCalls).toBe(2);
    expect(native.module.stopBrowsingCalls).toBe(1);
  });

  it("drops a desktop the native module reports as removed", () => {
    const native = fakeNativeBonjourModule();
    const browser = createNativeBonjourBrowser(native.module, native.Emitter);
    const seen: number[] = [];
    browser.subscribe(() => seen.push(browser.getServices().length));

    native.emit(studio);
    native.emit({ name: studio.name, type: studio.type, removed: true });

    expect(seen).toEqual([1, 0]);
    expect(browser.getServices()).toEqual([]);
  });
});

describe("unavailable Bonjour browser", () => {
  it("fails its refresh so pairing reports an unreachable machine", async () => {
    const browser = createUnavailableBonjourBrowser("no discovery here");

    expect(browser.getServices()).toEqual([]);
    await expect(browser.refresh?.()).rejects.toThrow("no discovery here");
  });
});
