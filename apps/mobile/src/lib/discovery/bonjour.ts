export interface BonjourService {
  name: string;
  type: string;
  host: string;
  port: number;
  txt: Record<string, string>;
}

interface BonjourServiceEvent extends BonjourService {
  removed?: boolean;
}

interface BonjourRemovalEvent {
  name: string;
  type?: string;
  removed: true;
}

export interface BonjourRefreshOptions {
  /**
   * Wait for this desktop's advertisement. A QR payload names one desktop; a
   * typed pairing code accepts any desktop that advertises an id, so it leaves
   * this unset.
   */
  desktopId?: string | null;
  timeoutMs?: number;
}

export interface BonjourBrowser {
  getServices(): readonly BonjourService[];
  refresh?(options?: BonjourRefreshOptions): Promise<void>;
  start(): void;
  stop(): void;
  subscribe(listener: () => void): () => void;
}

interface NativeBonjourModule {
  startBrowsing(): void;
  stopBrowsing(): void;
  /**
   * Android only: starts discovery and settles once the platform confirms the
   * browse is running, so a device that cannot browse at all is reported as
   * unreachable instead of as "no machine advertised that id". iOS owns its
   * browse in `start()` and does not expose this.
   */
  ensureBrowsing?(): Promise<void>;
}

interface ReactNativeModule {
  NativeEventEmitter: new (nativeModule: object) => {
    addListener(
      eventName: string,
      listener: (event: unknown) => void
    ): { remove(): void };
  };
  NativeModules: { KannaBonjourModule?: NativeBonjourModule };
}

declare const require: ((id: string) => ReactNativeModule) | undefined;

/**
 * How long a pairing attempt waits for the desktop it is looking for to finish
 * resolving. Discovery is asynchronous on both platforms: the browse, the
 * resolve, and the TXT record each arrive separately, and a QR scan routinely
 * runs before any of them have landed.
 */
const DISCOVERY_TIMEOUT_MS = 6_000;

export function createBonjourBrowser(): BonjourBrowser {
  const reactNative = loadReactNative();
  if (!reactNative) {
    return createStaticBonjourBrowser([]);
  }
  const { NativeEventEmitter, NativeModules } = reactNative;
  const nativeModule = NativeModules.KannaBonjourModule as NativeBonjourModule | undefined;
  if (!nativeModule) {
    // A React Native runtime without the module cannot discover anything. An
    // empty result here would be reported as "no machine matched that code",
    // which sends the user to check their network for a build problem.
    return createUnavailableBonjourBrowser(
      "This build cannot search the local network for Kanna desktops."
    );
  }

  return createNativeBonjourBrowser(nativeModule, NativeEventEmitter);
}

export function createNativeBonjourBrowser(
  nativeModule: NativeBonjourModule,
  NativeEventEmitter: ReactNativeModule["NativeEventEmitter"]
): BonjourBrowser {
  const services = new Map<string, BonjourService>();
  const listeners = new Set<() => void>();
  const emitter = new NativeEventEmitter(nativeModule as object);
  let subscription: { remove(): void } | null = null;
  let browsing = false;
  const notify = () => {
    for (const listener of Array.from(listeners)) {
      listener();
    }
  };
  const listen = () => {
    subscription ??= emitter.addListener("kannaBonjourServiceChanged", (event) => {
      if (!applyBonjourServiceEvent(services, event)) {
        return;
      }
      notify();
    });
  };
  listen();

  const hasMatch = (desktopId: string | null | undefined) =>
    Array.from(services.values()).some((service) => serviceMatches(service, desktopId));

  const waitForMatch = (desktopId: string | null | undefined, timeoutMs: number) =>
    new Promise<void>((resolve) => {
      let timer: ReturnType<typeof setTimeout> | null = null;
      const settle = () => {
        listeners.delete(check);
        if (timer !== null) clearTimeout(timer);
        resolve();
      };
      const check = () => {
        if (hasMatch(desktopId)) settle();
      };
      listeners.add(check);
      timer = setTimeout(settle, timeoutMs);
    });

  return {
    getServices: () => Array.from(services.values()),
    async refresh(options) {
      const timeoutMs = options?.timeoutMs ?? DISCOVERY_TIMEOUT_MS;
      listen();
      if (nativeModule.ensureBrowsing) {
        await ensureBrowsing(nativeModule, timeoutMs);
        browsing = true;
      }
      if (hasMatch(options?.desktopId)) return;
      // A timeout is not an error: nothing advertised the requested desktop,
      // which the caller reports against the candidates it actually has.
      await waitForMatch(options?.desktopId, timeoutMs);
    },
    start() {
      listen();
      if (browsing) return;
      browsing = true;
      nativeModule.startBrowsing();
    },
    stop() {
      browsing = false;
      nativeModule.stopBrowsing();
      subscription?.remove();
      subscription = null;
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    }
  };
}

/**
 * A browser for a runtime that has no discovery at all. Every refresh fails, so
 * pairing reports an unreachable machine rather than an empty network.
 */
export function createUnavailableBonjourBrowser(message: string): BonjourBrowser {
  const listeners = new Set<() => void>();
  return {
    getServices: () => [],
    refresh() {
      return Promise.reject(new Error(message));
    },
    start() {},
    stop() {},
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    }
  };
}

async function ensureBrowsing(
  nativeModule: NativeBonjourModule,
  timeoutMs: number
): Promise<void> {
  if (!nativeModule.ensureBrowsing) {
    // iOS starts its browse once, in start().
    return;
  }
  let timer: ReturnType<typeof setTimeout> | null = null;
  try {
    // A rejection is a real "this device cannot browse" answer and propagates.
    // A silent platform is not: keep waiting on the event stream instead.
    await Promise.race([
      nativeModule.ensureBrowsing(),
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, timeoutMs);
      })
    ]);
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
}

function serviceMatches(
  service: BonjourService,
  desktopId: string | null | undefined
): boolean {
  const advertised = typeof service.txt.desktopId === "string"
    ? service.txt.desktopId.trim()
    : "";
  if (!advertised) return false;
  if (!desktopId) return true;
  return advertised.toUpperCase() === desktopId.trim().toUpperCase();
}

function loadReactNative(): ReactNativeModule | null {
  try {
    return typeof require === "function" ? require("react-native") : null;
  } catch {
    return null;
  }
}

export function createStaticBonjourBrowser(
  initialServices: readonly BonjourService[]
): BonjourBrowser {
  const listeners = new Set<() => void>();
  return {
    getServices: () => initialServices,
    start() {},
    stop() {},
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    }
  };
}

export function applyBonjourServiceEvent(
  services: Map<string, BonjourService>,
  event: unknown
): boolean {
  const service = normalizeBonjourServiceEvent(event);
  if (!service) {
    return false;
  }

  const key = getBonjourServiceKey(service);
  if (service.removed === true) {
    const existed = services.delete(key);
    return existed;
  }

  services.set(key, service);
  return true;
}

function getBonjourServiceKey(service: Pick<BonjourService, "name">): string {
  return service.name;
}

function normalizeBonjourServiceEvent(
  event: unknown
): BonjourServiceEvent | BonjourRemovalEvent | null {
  if (!event || typeof event !== "object") {
    return null;
  }

  const record = event as Partial<BonjourServiceEvent>;
  if (record.removed === true) {
    return typeof record.name === "string"
      ? {
          name: record.name,
          type: typeof record.type === "string" ? record.type : undefined,
          removed: true
        }
      : null;
  }

  if (
    typeof record.name !== "string" ||
    typeof record.type !== "string" ||
    typeof record.host !== "string" ||
    typeof record.port !== "number"
  ) {
    return null;
  }

  return {
    name: record.name,
    type: record.type,
    host: record.host,
    port: record.port,
    txt: record.txt && typeof record.txt === "object" ? record.txt : {},
    // The removed === true case already returned above, so this event is an add/update.
    removed: false
  };
}
