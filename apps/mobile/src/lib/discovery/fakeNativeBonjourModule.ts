import type { BonjourService } from "./bonjour";

interface FakeNativeModule {
  startBrowsing(): void;
  stopBrowsing(): void;
  ensureBrowsing(): Promise<void>;
  startBrowsingCalls: number;
  stopBrowsingCalls: number;
  ensureBrowsingCalls: number;
}

type BonjourEvent =
  | (BonjourService & { removed?: boolean })
  | { name: string; type?: string; removed: true };

/**
 * Stands in for the platform module on either side: the same method and event
 * contract KannaBonjourModule.kt and KannaBonjourModule.swift implement. It
 * lets the real JS browser and the real pairing service run against native
 * events in process; it proves nothing about the native code itself.
 */
export function fakeNativeBonjourModule(options: {
  ensureBrowsing?: () => Promise<void>;
} = {}) {
  const listeners = new Set<(event: unknown) => void>();
  const module: FakeNativeModule = {
    startBrowsingCalls: 0,
    stopBrowsingCalls: 0,
    ensureBrowsingCalls: 0,
    startBrowsing() {
      module.startBrowsingCalls += 1;
    },
    stopBrowsing() {
      module.stopBrowsingCalls += 1;
    },
    ensureBrowsing() {
      module.ensureBrowsingCalls += 1;
      return (options.ensureBrowsing ?? (() => Promise.resolve()))();
    }
  };

  class Emitter {
    constructor(_nativeModule: object) {}

    addListener(_eventName: string, listener: (event: unknown) => void) {
      listeners.add(listener);
      return {
        remove() {
          listeners.delete(listener);
        }
      };
    }
  }

  return {
    module,
    Emitter,
    listenerCount: () => listeners.size,
    emit(event: BonjourEvent) {
      for (const listener of Array.from(listeners)) {
        listener(event);
      }
    }
  };
}
