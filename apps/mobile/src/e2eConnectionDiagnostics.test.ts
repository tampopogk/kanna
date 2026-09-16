import { describe, expect, it } from "vitest";
import {
  buildE2eConnectionDiagnostics,
  serializeE2eConnectionDiagnostics
} from "./e2eConnectionDiagnostics";

const DEVICE_SECRET = "device-secret-must-never-leak";
const PUSH_SIGNATURE = "push-signature-must-never-leak";

function state(): Parameters<typeof buildE2eConnectionDiagnostics>[0] {
  return {
    connectionMode: "lan",
    connectionState: "connected",
    refreshStatus: "updated",
    taskCollectionStatus: "ready",
    serverStatus: "running",
    errorMessage: null,
    desktopId: "desktop-paired",
    selectedDesktopId: "desktop-paired",
    selectedRepoId: null,
    auth: {
      status: "signedIn",
      user: {
        uid: "uid-1",
        email: "reviewer@example.com",
        displayName: null,
        emailVerified: true
      }
    },
    mobileDeviceId: "phone-1",
    trustedDesktops: [
      {
        desktopId: "desktop-paired",
        displayName: "Paired Mac",
        lanEndpoints: [
          { baseUrl: "http://192.168.1.10:48120", lastSeenAt: "2026-09-16T00:00:00.000Z" },
          { baseUrl: "not a url", lastSeenAt: "2026-09-16T00:00:00.000Z" }
        ],
        lastSeenAt: "2026-09-16T00:00:00.000Z",
        deviceSecret: DEVICE_SECRET,
        pushPairingCert: {
          deviceId: "phone-1",
          issuedAt: 1,
          expiresAt: 2,
          signature: PUSH_SIGNATURE
        }
      },
      {
        desktopId: "desktop-identity-only",
        displayName: "Trust-only Mac",
        lanEndpoints: [],
        lastSeenAt: "2026-09-16T00:00:00.000Z"
      }
    ],
    liveLanDesktops: [{ id: "desktop-paired", name: "Paired Mac", online: true, mode: "lan" }],
    accountDesktops: [],
    repoTasks: [{ id: "task-1", repoId: "repo-1", title: "Secret task title", stage: "in progress" }],
    recentTasks: []
  };
}

describe("E2E connection diagnostics", () => {
  it("reports route, status, and credential presence without the credentials", () => {
    const diagnostics = buildE2eConnectionDiagnostics(state());

    expect(diagnostics).toEqual({
      connectionMode: "lan",
      connectionState: "connected",
      refreshStatus: "updated",
      taskCollectionStatus: "ready",
      serverStatus: "running",
      errorMessage: null,
      desktopId: "desktop-paired",
      selectedDesktopId: "desktop-paired",
      selectedRepoId: null,
      authStatus: "signedIn",
      mobileDeviceIdPresent: true,
      trustedDesktops: [
        {
          desktopId: "desktop-paired",
          lanEndpoints: ["http://192.168.1.10:48120", "<invalid>"],
          deviceSecretPresent: true,
          pushPairingCertPresent: true
        },
        {
          desktopId: "desktop-identity-only",
          lanEndpoints: [],
          deviceSecretPresent: false,
          pushPairingCertPresent: false
        }
      ],
      liveLanDesktopIds: ["desktop-paired"],
      accountDesktopIds: [],
      taskCounts: { repo: 1, recent: 0 }
    });

    const serialized = serializeE2eConnectionDiagnostics(diagnostics);
    expect(serialized).not.toContain(DEVICE_SECRET);
    expect(serialized).not.toContain(PUSH_SIGNATURE);
    expect(serialized).not.toContain("reviewer@example.com");
    expect(serialized).not.toContain("Secret task title");
  });

  it("describes the identity-only trust seed the failed smoke retained", () => {
    const diagnostics = buildE2eConnectionDiagnostics({
      ...state(),
      auth: { status: "signedOut" },
      mobileDeviceId: null,
      connectionState: "idle",
      taskCollectionStatus: "loading",
      trustedDesktops: [
        {
          desktopId: "desktop-identity-only",
          displayName: "Trust-only Mac",
          lanEndpoints: [],
          lastSeenAt: "2026-09-16T00:00:00.000Z"
        }
      ],
      liveLanDesktops: [],
      repoTasks: []
    });

    expect(diagnostics.mobileDeviceIdPresent).toBe(false);
    expect(diagnostics.authStatus).toBe("signedOut");
    expect(diagnostics.trustedDesktops).toEqual([
      {
        desktopId: "desktop-identity-only",
        lanEndpoints: [],
        deviceSecretPresent: false,
        pushPairingCertPresent: false
      }
    ]);
    expect(diagnostics.taskCounts).toEqual({ repo: 0, recent: 0 });
  });
});
