/**
 * The `removeAccountDesktop` callable adapter.
 *
 * The emulator suite proves the same wrapper for real, but only where three
 * emulators are running. This one runs everywhere `pnpm test` does, and pins
 * the two things the adapter alone decides: which caller identity reaches the
 * core, and how a refusal is shaped for the client.
 */
import type { CallableRequest } from "firebase-functions/v2/https";
import { HttpsError } from "firebase-functions/v2/https";
import { beforeEach, describe, expect, it, vi } from "vitest";

const desktopMocks = vi.hoisted(() => ({
  removeAccountDesktopCore: vi.fn(),
  accountDesktopDependencies: vi.fn(() => ({ store: "store" })),
}));

vi.mock("../src/accountDesktops.js", () => ({
  removeAccountDesktop: desktopMocks.removeAccountDesktopCore,
  accountDesktopDependencies: desktopMocks.accountDesktopDependencies,
}));

import { removeAccountDesktop } from "../src/index.js";
import { BillingRequestError } from "../src/billing/errors.js";

function request(data: unknown, uid: string | null): CallableRequest<unknown> {
  return {
    data,
    auth: uid ? { uid, token: {} } : undefined,
  } as unknown as CallableRequest<unknown>;
}

describe("removeAccountDesktop callable adapter", () => {
  beforeEach(() => {
    desktopMocks.removeAccountDesktopCore.mockReset();
    desktopMocks.removeAccountDesktopCore.mockResolvedValue({ removed: 1 });
  });

  it("passes the authenticated uid even when the request body names another", async () => {
    await removeAccountDesktop.run(request({ desktopId: "desktop-1", uid: "victim" }, "user-1"));

    expect(desktopMocks.removeAccountDesktopCore).toHaveBeenCalledWith(
      { desktopId: "desktop-1", uid: "victim" },
      { uid: "user-1" },
      expect.objectContaining({ store: expect.anything() }),
    );
  });

  it("hands the core no caller at all when the request is unauthenticated", async () => {
    await removeAccountDesktop
      .run(request({ desktopId: "desktop-1" }, null))
      .catch(() => undefined);

    expect(desktopMocks.removeAccountDesktopCore).toHaveBeenCalledWith(
      { desktopId: "desktop-1" },
      null,
      expect.anything(),
    );
  });

  it("translates a refusal into an HttpsError carrying its reason", async () => {
    desktopMocks.removeAccountDesktopCore.mockRejectedValue(
      new BillingRequestError("failed-precondition", "account_deleted", "This account is being deleted."),
    );

    const error = await removeAccountDesktop
      .run(request({ desktopId: "desktop-1" }, "user-1"))
      .then(() => null)
      .catch((thrown: unknown) => thrown);

    expect(error).toBeInstanceOf(HttpsError);
    expect(error).toMatchObject({
      code: "failed-precondition",
      message: "This account is being deleted.",
      details: { reason: "account_deleted" },
    });
  });

  it("lets an unexpected failure through instead of dressing it as a refusal", async () => {
    // A bug in the store must not reach the client wearing a reason code the
    // app would render as an explanation of what the person did wrong.
    desktopMocks.removeAccountDesktopCore.mockRejectedValue(new Error("firestore exploded"));

    const error = await removeAccountDesktop
      .run(request({ desktopId: "desktop-1" }, "user-1"))
      .then(() => null)
      .catch((thrown: unknown) => thrown);

    expect(error).not.toBeInstanceOf(HttpsError);
    expect(error).toMatchObject({ message: "firestore exploded" });
  });
});
