import { describe, expect, it, vi } from "vitest";
import {
  parseDesktopId,
  removeAccountDesktop,
  type AccountDesktopStore,
} from "../src/accountDesktops.js";
import { BillingRequestError } from "../src/billing/errors.js";

function harness(options: { deleting?: boolean; removed?: number } = {}) {
  const removals: { uid: string; desktopId: string }[] = [];
  const store: AccountDesktopStore = {
    isAccountDeleting: vi.fn(async () => options.deleting ?? false),
    removeDesktop: vi.fn(async (uid: string, desktopId: string) => {
      removals.push({ uid, desktopId });
      return options.removed ?? 1;
    }),
  };
  return { removals, store, dependencies: { store } };
}

describe("removeAccountDesktop", () => {
  it("removes the named entry from the caller's own directory", async () => {
    const { removals, dependencies } = harness();

    await expect(
      removeAccountDesktop({ desktopId: "desktop-dev-42" }, { uid: "user-1" }, dependencies),
    ).resolves.toEqual({ removed: 1 });

    expect(removals).toEqual([{ uid: "user-1", desktopId: "desktop-dev-42" }]);
  });

  it("scopes the removal to the authenticated uid, not anything the caller sends", async () => {
    const { removals, dependencies } = harness();

    await removeAccountDesktop(
      { desktopId: "desktop-1", uid: "user-2" } as Record<string, unknown>,
      { uid: "user-1" },
      dependencies,
    );

    expect(removals).toEqual([{ uid: "user-1", desktopId: "desktop-1" }]);
  });

  it("reports an entry that was already gone as a completed removal, not a failure", async () => {
    const { dependencies } = harness({ removed: 0 });

    await expect(
      removeAccountDesktop({ desktopId: "desktop-1" }, { uid: "user-1" }, dependencies),
    ).resolves.toEqual({ removed: 0 });
  });

  it("refuses an unauthenticated caller before touching the store", async () => {
    const { store, dependencies } = harness();

    await expect(
      removeAccountDesktop({ desktopId: "desktop-1" }, null, dependencies),
    ).rejects.toMatchObject({ code: "unauthenticated", reason: "sign_in_required" });
    expect(store.removeDesktop).not.toHaveBeenCalled();
  });

  it("refuses while the account is being deleted", async () => {
    const { store, dependencies } = harness({ deleting: true });

    await expect(
      removeAccountDesktop({ desktopId: "desktop-1" }, { uid: "user-1" }, dependencies),
    ).rejects.toMatchObject({ code: "failed-precondition", reason: "account_deleted" });
    expect(store.removeDesktop).not.toHaveBeenCalled();
  });

  it.each([
    ["a missing id", undefined],
    ["a blank id", "   "],
    ["a non-string id", 7],
  ])("refuses %s before touching the store", async (_label, desktopId) => {
    const { store, dependencies } = harness();

    await expect(
      removeAccountDesktop({ desktopId }, { uid: "user-1" }, dependencies),
    ).rejects.toMatchObject({
      code: "invalid-argument",
      reason: "invalid_desktop_request",
    });
    expect(store.removeDesktop).not.toHaveBeenCalled();
  });

  it("accepts any desktop id a machine may legitimately carry", () => {
    // Path-shaped values are not refused here: they are never interpolated
    // into a document path, only compared against the stored `desktopId`.
    expect(parseDesktopId("desktop-2e")).toBe("desktop-2e");
    expect(parseDesktopId(" desktop-1 ")).toBe("desktop-1");
    expect(parseDesktopId("desktop/with/slashes")).toBe("desktop/with/slashes");
  });

  it("raises callable-shaped errors the entry point can translate", async () => {
    const { dependencies } = harness();

    await expect(
      removeAccountDesktop({}, { uid: "user-1" }, dependencies),
    ).rejects.toBeInstanceOf(BillingRequestError);
  });
});
