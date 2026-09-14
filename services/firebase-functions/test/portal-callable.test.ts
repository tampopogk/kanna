import type { CallableRequest } from "firebase-functions/v2/https";
import { afterEach, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ core: vi.fn() }));
vi.mock("../src/billing/portal.js", () => ({ createPortalSession: mocks.core }));
import { createPortalSession } from "../src/index.js";
import { BillingRequestError } from "../src/billing/errors.js";
afterEach(() => vi.resetAllMocks());
it("uses only verified callable authentication as the caller, regardless of payload", async () => {
  mocks.core.mockResolvedValue({ url: "https://billing.stripe.test/session" });
  await createPortalSession.run({ data: { uid: "other" }, auth: { uid: "owner" } } as unknown as CallableRequest);
  expect(mocks.core).toHaveBeenCalledWith({ uid: "other" }, { uid: "owner" }, expect.objectContaining({ env: process.env }));
});
it("passes unauthenticated requests as null and preserves core refusal details", async () => {
  mocks.core.mockRejectedValue(new BillingRequestError("unauthenticated", "sign_in_required", "Sign in"));
  await expect(createPortalSession.run({ data: {} } as CallableRequest)).rejects.toMatchObject({ code: "unauthenticated", details: { reason: "sign_in_required" } });
  expect(mocks.core).toHaveBeenCalledWith({}, null, expect.any(Object));
});
