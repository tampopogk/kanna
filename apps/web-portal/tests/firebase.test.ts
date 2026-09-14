import { afterEach, expect, it, vi } from "vitest";
import type { User } from "firebase/auth";
const mocks = vi.hoisted(() => ({
  auth: { currentUser: null as User | null },
  reset: vi.fn(), resend: vi.fn(), snapshot: vi.fn(), callable: vi.fn(),
}));
vi.mock("firebase/auth", async (original) => ({
  ...await original<typeof import("firebase/auth")>(),
  getAuth: () => mocks.auth, connectAuthEmulator: vi.fn(),
  sendPasswordResetEmail: mocks.reset, sendEmailVerification: mocks.resend,
}));
vi.mock("firebase/firestore", async (original) => ({
  ...await original<typeof import("firebase/firestore")>(), onSnapshot: mocks.snapshot,
}));
vi.mock("firebase/functions", async (original) => ({
  ...await original<typeof import("firebase/functions")>(), httpsCallable: mocks.callable,
}));
import { portalFirebase } from "../src/firebase";
afterEach(() => { vi.clearAllMocks(); mocks.auth.currentUser = null; });

it("uses Firebase's reset and verification SDK calls on the existing identity", async () => {
  const user = { uid: "owner" } as User;
  await portalFirebase.resetPassword("owner@example.test");
  await portalFirebase.resendVerification(user);
  expect(mocks.reset).toHaveBeenCalledWith(mocks.auth, "owner@example.test");
  expect(mocks.resend).toHaveBeenCalledWith(user);
});
it("reloads verification before forcing a token refresh for authenticated callables", async () => {
  const calls: string[] = [];
  const user = { uid: "owner", reload: vi.fn(async () => { calls.push("reload"); }), getIdToken: vi.fn(async () => { calls.push("token"); return "fresh"; }) } as unknown as User;
  mocks.auth.currentUser = user;
  await expect(portalFirebase.reloadUser(user)).resolves.toBe(user);
  expect(calls).toEqual(["reload", "token"]);
  expect(user.getIdToken).toHaveBeenCalledWith(true);
});
it("refuses stale verification after an auth switch", async () => {
  const user = { uid: "owner", reload: vi.fn(async () => { mocks.auth.currentUser = { uid: "second" } as User; }), getIdToken: vi.fn() } as unknown as User;
  mocks.auth.currentUser = user;
  await expect(portalFirebase.reloadUser(user)).rejects.toThrow("account changed");
  expect(user.getIdToken).not.toHaveBeenCalled();
});
it("does not turn a cached document into authoritative access and observes metadata changes", () => {
  const stop = vi.fn(); mocks.snapshot.mockReturnValue(stop);
  const next = vi.fn(); const error = vi.fn();
  expect(portalFirebase.observeEntitlement("owner", next, error)).toBe(stop);
  const [reference, options, callback] = mocks.snapshot.mock.calls[0];
  expect(reference.path).toBe("users/owner/entitlements/cloud_access");
  expect(options).toEqual({ includeMetadataChanges: true });
  callback({ metadata: { fromCache: true }, exists: () => true, data: () => ({ status: "active" }) });
  expect(next).toHaveBeenCalledWith(null, true); expect(error).not.toHaveBeenCalled();
  callback({ metadata: { fromCache: false }, exists: () => true, data: () => ({ status: "active" }) });
  expect(next).toHaveBeenCalledWith({ status: "active" });
});
it("calls billing management with no client-selected parameters", async () => {
  const call = vi.fn(async () => ({ data: { url: "https://billing.stripe.test/session" } }));
  mocks.callable.mockReturnValue(call);
  await expect(portalFirebase.createPortalSession()).resolves.toEqual({ url: "https://billing.stripe.test/session" });
  expect(mocks.callable).toHaveBeenCalledWith(expect.anything(), "createPortalSession");
  expect(call).toHaveBeenCalledWith({});
});
