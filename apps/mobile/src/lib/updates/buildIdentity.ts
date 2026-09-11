import {
  readKannaExpoExtra,
  resolveMobileAppEnvironment,
  type KannaAppEnvironmentName,
  type OtaChannel
} from "../../mobileEnvironment";
import { readExpoConfig } from "../expoConfig";

export type BuildSource =
  | { kind: "ota"; label: string; updateId: string }
  | { kind: "embedded"; label: "Embedded bundle" }
  | { kind: "development"; label: "Development bundle (Metro)" }
  | { kind: "unknown"; label: "Unknown" };

export interface BuildIdentity {
  releaseVersion: string;
  releaseSummary: string;
  nativeVersion: string;
  nativeBuild: string;
  nativeSummary: string;
  runtimeVersion: string;
  environment: KannaAppEnvironmentName;
  channel: string;
  source: BuildSource;
}

export interface BuildIdentityInput {
  nativeApplicationVersion: string | null;
  nativeBuildVersion: string | null;
  isDevelopment: boolean;
  updatesEnabled: boolean;
  isEmbeddedLaunch: boolean;
  updateId: string | null;
  otaReleaseVersion?: string | null;
  runtimeVersion: string | null;
  channel: string | null;
  appEnvironment: KannaAppEnvironmentName;
  configuredRuntimeVersion: string;
  configuredReleaseVersion?: string | null;
  configuredChannel: OtaChannel | null;
}

interface ExpoApplicationApi {
  nativeApplicationVersion: string | null;
  nativeBuildVersion: string | null;
}

interface ExpoUpdatesIdentityApi {
  isEnabled: boolean;
  isEmbeddedLaunch: boolean;
  updateId: string | null;
  runtimeVersion: string | null;
  channel: string | null;
  manifest?: {
    metadata?: {
      kanna?: {
        releaseVersion?: unknown;
      };
    };
  };
}

export function buildIdentity(input: BuildIdentityInput): BuildIdentity {
  const nativeVersion = normalizeValue(input.nativeApplicationVersion);
  const nativeBuild = normalizeValue(input.nativeBuildVersion);
  const nativeSummary = buildNativeSummary(nativeVersion, nativeBuild);
  const runtimeVersion = normalizeValue(
    input.runtimeVersion ?? input.configuredRuntimeVersion
  );
  const source = buildSource(input);
  const releaseVersion = normalizeValue(
    input.otaReleaseVersion ??
      input.configuredReleaseVersion ??
      input.nativeApplicationVersion
  );

  return {
    releaseVersion,
    releaseSummary: buildReleaseSummary(releaseVersion, source.kind),
    nativeVersion,
    nativeBuild,
    nativeSummary,
    runtimeVersion,
    environment: input.appEnvironment,
    channel: normalizeValue(input.channel ?? input.configuredChannel, "None"),
    source
  };
}

export function getCurrentBuildIdentity(): BuildIdentity {
  const application = require("expo-application") as ExpoApplicationApi;
  const updates = require("expo-updates") as ExpoUpdatesIdentityApi;
  const extra = readKannaExpoExtra(readExpoConfig());
  const environment = resolveMobileAppEnvironment(extra?.appEnv);

  return buildIdentity({
    nativeApplicationVersion: application.nativeApplicationVersion,
    nativeBuildVersion: application.nativeBuildVersion,
    isDevelopment: typeof __DEV__ !== "undefined" && __DEV__,
    updatesEnabled: updates.isEnabled,
    isEmbeddedLaunch: updates.isEmbeddedLaunch,
    updateId: updates.updateId,
    otaReleaseVersion: readOtaReleaseVersion(updates.manifest),
    runtimeVersion: updates.runtimeVersion,
    channel: updates.channel,
    appEnvironment: extra?.appEnv ?? environment.name,
    configuredRuntimeVersion:
      extra?.runtimeVersion ?? environment.runtimeVersion,
    configuredReleaseVersion: extra?.releaseVersion,
    configuredChannel: extra?.ota?.channel ?? environment.otaChannel
  });
}

function readOtaReleaseVersion(
  manifest: ExpoUpdatesIdentityApi["manifest"]
): string | null {
  const value = manifest?.metadata?.kanna?.releaseVersion;
  return typeof value === "string" ? normalizeOptionalValue(value) : null;
}

function buildReleaseSummary(
  releaseVersion: string,
  source: BuildSource["kind"]
): string {
  if (source === "ota") return `${releaseVersion} (OTA)`;
  if (source === "development") return `${releaseVersion} (development)`;
  return releaseVersion;
}

function buildSource(input: BuildIdentityInput): BuildSource {
  if (input.isDevelopment) {
    return {
      kind: "development",
      label: "Development bundle (Metro)"
    };
  }

  if (!input.updatesEnabled || input.isEmbeddedLaunch) {
    return { kind: "embedded", label: "Embedded bundle" };
  }

  const updateId = normalizeOptionalValue(input.updateId);
  return updateId
    ? { kind: "ota", label: updateId, updateId }
    : { kind: "unknown", label: "Unknown" };
}

function buildNativeSummary(version: string, build: string): string {
  if (version !== "Unknown" && build !== "Unknown") {
    return `${version} (${build})`;
  }

  return version !== "Unknown" ? version : build;
}

function normalizeValue(
  value: string | null | undefined,
  fallback = "Unknown"
): string {
  return normalizeOptionalValue(value) ?? fallback;
}

function normalizeOptionalValue(
  value: string | null | undefined
): string | null {
  const normalized = value?.trim();
  return normalized ? normalized : null;
}
