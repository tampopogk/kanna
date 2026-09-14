import { computed, inject, onScopeDispose, provide, readonly, ref, shallowRef, triggerRef, watch, type ComputedRef, type InjectionKey, type Ref, type ShallowRef } from "vue";
import type { User } from "firebase/auth";
import { portalFirebase, type PortalFirebase } from "./firebase";
import { cloudAccessState, graceDeadline, type CloudAccessState, type CloudEntitlement } from "./types";

export interface PortalSession {
  user: ShallowRef<User | null>;
  entitlement: ShallowRef<CloudEntitlement | null>;
  ready: Readonly<Ref<boolean>>;
  entitlementReady: Readonly<Ref<boolean>>;
  accessState: ComputedRef<CloudAccessState>;
  subscribed: Readonly<ComputedRef<boolean>>;
  canSubscribe: Readonly<ComputedRef<boolean>>;
  refreshEntitlement(): Promise<void>;
}

const sessionKey: InjectionKey<PortalSession> = Symbol("portal-session");
const firebaseKey: InjectionKey<PortalFirebase> = Symbol("portal-firebase");

export function providePortalSession(api: PortalFirebase = portalFirebase): PortalSession {
  const user = shallowRef<User | null>(null);
  const entitlement = shallowRef<CloudEntitlement | null>(null);
  const ready = ref(false);
  const entitlementReady = ref(false);
  const readFailed = ref(false);
  const now = ref(Date.now());
  let generation = 0;
  let disposed = false;
  let stopEntitlement: (() => void) | undefined;
  let deadlineTimer: ReturnType<typeof setTimeout> | undefined;

  function clearDeadline(): void {
    clearTimeout(deadlineTimer);
    deadlineTimer = undefined;
  }

  function updateDeadline(): void {
    clearDeadline();
    now.value = Date.now();
    const deadline = graceDeadline(entitlement.value);
    if (entitlement.value?.status === "grace" && deadline !== null && deadline > now.value) {
      // A deadline wakeup, never a network poll. Cap long delays to avoid JS timer overflow.
      deadlineTimer = setTimeout(updateDeadline, Math.min(deadline - now.value, 2_147_483_647));
    }
  }

  async function refreshEntitlement(): Promise<void> {
    const version = ++generation;
    stopEntitlement?.();
    stopEntitlement = undefined;
    clearDeadline();
    entitlement.value = null;
    entitlementReady.value = false;
    readFailed.value = false;
    const uid = user.value?.uid;
    if (!uid || disposed) return;
    const current = () => !disposed && version === generation && user.value?.uid === uid;
    const failed = () => {
      if (!current()) return;
      entitlement.value = null;
      entitlementReady.value = false;
      readFailed.value = true;
      clearDeadline();
    };
    try {
      stopEntitlement = api.observeEntitlement(uid, (next, fromCache = false) => {
        if (!current()) return;
        entitlement.value = next;
        entitlementReady.value = !fromCache;
        readFailed.value = false;
        updateDeadline();
      }, failed);
    } catch {
      failed();
    }
  }

  watch(() => user.value?.uid, () => { void refreshEntitlement(); }, { flush: "sync" });
  const stopUser = api.observeUser((nextUser) => {
    if (disposed) return;
    user.value = nextUser;
    triggerRef(user);
    ready.value = true;
  });
  window.addEventListener("focus", updateDeadline);
  onScopeDispose(() => {
    disposed = true;
    ++generation;
    stopUser();
    stopEntitlement?.();
    clearDeadline();
    window.removeEventListener("focus", updateDeadline);
  });
  const accessState = computed(() => readFailed.value ? "read-failure" : cloudAccessState(entitlement.value, now.value));
  const subscribed = computed(() => accessState.value === "active" || accessState.value === "grace");
  const canSubscribe = computed(() => {
    if (!entitlementReady.value) return false;
    const record = entitlement.value;
    return !record || (record.source === "stripe" && (record.status === "expired" || record.status === "revoked"));
  });
  const session: PortalSession = {
    user, entitlement, ready: readonly(ready), entitlementReady: readonly(entitlementReady),
    accessState, subscribed, canSubscribe, refreshEntitlement,
  };
  provide(sessionKey, session);
  provide(firebaseKey, api);
  return session;
}

export function usePortalSession(): PortalSession {
  const session = inject(sessionKey);
  if (!session) throw new Error("Portal session has not been provided");
  return session;
}

export function usePortalFirebase(): PortalFirebase {
  const api = inject(firebaseKey);
  if (!api) throw new Error("Portal Firebase API has not been provided");
  return api;
}
