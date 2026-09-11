import { describe, expect, it, vi } from "vitest";

import { CloudTransferSignInRequiredError } from "../services/desktopTransferMachines";
import {
  parseCloudTransferCredentialRefreshCommand,
  performCloudTransferCredentialRefresh,
} from "./cloudTransferCredentialRefresh";

describe("cloud transfer credential refresh commands", () => {
  it("accepts only a correlated peer request and no credential material", () => {
    expect(parseCloudTransferCredentialRefreshCommand({
      requestId: "refresh-1",
      peerId: "peer-studio",
    })).toEqual({ requestId: "refresh-1", peerId: "peer-studio" });
    expect(() => parseCloudTransferCredentialRefreshCommand({ peerId: "peer-studio" }))
      .toThrow();
    expect(() => parseCloudTransferCredentialRefreshCommand({
      requestId: "refresh-1",
      peerId: " ",
    })).toThrow();
  });

  it("reports refreshed only after the authenticated owner completed renewal", async () => {
    const refresh = vi.fn(async () => {});
    await expect(performCloudTransferCredentialRefresh(
      { requestId: "refresh-1", peerId: "peer-studio" },
      refresh,
    )).resolves.toBe("refreshed");
    expect(refresh).toHaveBeenCalledWith("peer-studio");
  });

  it("reports actual sign-in absence distinctly without forwarding error text", async () => {
    const refresh = vi.fn(async () => {
      throw new CloudTransferSignInRequiredError();
    });
    await expect(performCloudTransferCredentialRefresh(
      { requestId: "refresh-2", peerId: "peer-studio" },
      refresh,
    )).resolves.toBe("sign_in_required");
  });

  it("collapses other provider failures to a fixed non-secret verdict", async () => {
    await expect(performCloudTransferCredentialRefresh(
      { requestId: "refresh-3", peerId: "peer-studio" },
      async () => {
        throw new Error("sensitive provider response");
      },
    )).resolves.toBe("refresh_failed");
  });
});
