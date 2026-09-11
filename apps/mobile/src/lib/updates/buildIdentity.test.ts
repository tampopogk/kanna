import { describe, expect, it } from "vitest";
import { buildIdentity } from "./buildIdentity";

describe("buildIdentity", () => {
  it("identifies the exact downloaded OTA update", () => {
    expect(
      buildIdentity({
        nativeApplicationVersion: "2.4.0",
        nativeBuildVersion: "108",
        isDevelopment: false,
        updatesEnabled: true,
        isEmbeddedLaunch: false,
        updateId: "84667f93-5c7b-45fb-9f78-7045160cb842",
        otaReleaseVersion: "2.5.1",
        runtimeVersion: "2.1.2",
        channel: "staging",
        appEnvironment: "staging",
        configuredRuntimeVersion: "2.1.2",
        configuredReleaseVersion: "2.5.0",
        configuredChannel: "staging"
      })
    ).toEqual({
      releaseVersion: "2.5.1",
      releaseSummary: "2.5.1 (OTA)",
      nativeVersion: "2.4.0",
      nativeBuild: "108",
      nativeSummary: "2.4.0 (108)",
      runtimeVersion: "2.1.2",
      environment: "staging",
      channel: "staging",
      source: {
        kind: "ota",
        label: "84667f93-5c7b-45fb-9f78-7045160cb842",
        updateId: "84667f93-5c7b-45fb-9f78-7045160cb842"
      }
    });
  });

  it("clearly identifies an embedded bundle", () => {
    const identity = buildIdentity({
      nativeApplicationVersion: "2.4.0",
      nativeBuildVersion: "108",
      isDevelopment: false,
      updatesEnabled: true,
      isEmbeddedLaunch: true,
      updateId: "embedded-update-id",
      configuredReleaseVersion: "2.4.0",
      runtimeVersion: "2.1.2",
      channel: "production",
      appEnvironment: "prod",
      configuredRuntimeVersion: "2.1.2",
      configuredChannel: "production"
    });

    expect(identity.source).toEqual({
      kind: "embedded",
      label: "Embedded bundle"
    });
    expect(identity.releaseSummary).toBe("2.4.0");
  });

  it("distinguishes a Metro development bundle and uses configured metadata", () => {
    const identity = buildIdentity({
      nativeApplicationVersion: "2.4.0",
      nativeBuildVersion: "108",
      isDevelopment: true,
      updatesEnabled: false,
      isEmbeddedLaunch: false,
      updateId: null,
      configuredReleaseVersion: "2.5.0",
      runtimeVersion: null,
      channel: null,
      appEnvironment: "dev",
      configuredRuntimeVersion: "2.1.2",
      configuredChannel: null
    });

    expect(identity.runtimeVersion).toBe("2.1.2");
    expect(identity.releaseSummary).toBe("2.5.0 (development)");
    expect(identity.channel).toBe("None");
    expect(identity.source).toEqual({
      kind: "development",
      label: "Development bundle (Metro)"
    });
  });

  it("renders stable fallbacks when build identity is unavailable", () => {
    expect(
      buildIdentity({
        nativeApplicationVersion: null,
        nativeBuildVersion: null,
        isDevelopment: false,
        updatesEnabled: true,
        isEmbeddedLaunch: false,
        updateId: null,
        runtimeVersion: null,
        channel: null,
        appEnvironment: "prod",
        configuredRuntimeVersion: "",
        configuredChannel: null
      })
    ).toEqual({
      releaseVersion: "Unknown",
      releaseSummary: "Unknown",
      nativeVersion: "Unknown",
      nativeBuild: "Unknown",
      nativeSummary: "Unknown",
      runtimeVersion: "Unknown",
      environment: "prod",
      channel: "None",
      source: { kind: "unknown", label: "Unknown" }
    });
  });

  it("falls back to the native version for a legacy OTA manifest", () => {
    const identity = buildIdentity({
      nativeApplicationVersion: "1.0.0",
      nativeBuildVersion: "3",
      isDevelopment: false,
      updatesEnabled: true,
      isEmbeddedLaunch: false,
      updateId: "43d4e1d7-b5e0-4f2e-6d47-96b71048692b",
      otaReleaseVersion: null,
      runtimeVersion: "2.2.3",
      channel: "staging",
      appEnvironment: "staging",
      configuredRuntimeVersion: "2.2.3",
      configuredChannel: "staging"
    });

    expect(identity.releaseVersion).toBe("1.0.0");
    expect(identity.releaseSummary).toBe("1.0.0 (OTA)");
    expect(identity.nativeSummary).toBe("1.0.0 (3)");
    expect(identity.source).toMatchObject({
      kind: "ota",
      updateId: "43d4e1d7-b5e0-4f2e-6d47-96b71048692b"
    });
  });

  it("uses the available native value in the collapsed summary", () => {
    const base = {
      isDevelopment: true,
      updatesEnabled: false,
      isEmbeddedLaunch: false,
      updateId: null,
      runtimeVersion: null,
      channel: null,
      appEnvironment: "dev" as const,
      configuredRuntimeVersion: "2.1.2",
      configuredChannel: null
    };

    expect(
      buildIdentity({
        ...base,
        nativeApplicationVersion: "2.4.0",
        nativeBuildVersion: null
      }).nativeSummary
    ).toBe("2.4.0");
    expect(
      buildIdentity({
        ...base,
        nativeApplicationVersion: null,
        nativeBuildVersion: "108"
      }).nativeSummary
    ).toBe("108");
  });

  it("identifies a disabled packaged release as its embedded bundle", () => {
    const identity = buildIdentity({
      nativeApplicationVersion: "2.4.0",
      nativeBuildVersion: "108",
      isDevelopment: false,
      updatesEnabled: false,
      isEmbeddedLaunch: false,
      updateId: null,
      runtimeVersion: "2.1.2",
      channel: "production",
      appEnvironment: "prod",
      configuredRuntimeVersion: "2.1.2",
      configuredChannel: "production"
    });

    expect(identity.source).toEqual({
      kind: "embedded",
      label: "Embedded bundle"
    });
  });
});
