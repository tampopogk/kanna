import type { AgentProvider } from "@kanna/agent-protocol";
import { parseAgentProviderInventory } from "../api/agentProviders";
import type { FetchLike } from "../transports/lanTransport";
import type { BonjourService } from "./bonjour";

export interface TrustedBonjourEndpoint {
  baseUrl: string;
  desktopId: string;
  displayName: string;
  /** Agent provider CLIs the desktop reported on its status probe. Absent from
   * desktops that predate provider inventory. */
  agentProviders?: AgentProvider[];
}

export interface TrustedLanEndpointHint {
  baseUrl: string;
  desktopId: string;
}

export async function resolveTrustedBonjourEndpoint(input: {
  fetchImpl: FetchLike;
  services: readonly BonjourService[];
  persistedEndpoints?: readonly TrustedLanEndpointHint[];
  trustedDesktopIds: readonly string[];
  preferredDesktopId: string | null;
  probeTimeoutMs?: number;
}): Promise<TrustedBonjourEndpoint | null> {
  const trustedDesktopIds = new Set(input.trustedDesktopIds);
  for (const candidate of endpointCandidates(input)) {
    const endpoint = await validateTrustedEndpoint(
      candidate,
      trustedDesktopIds,
      input.fetchImpl,
      input.probeTimeoutMs
    );
    if (endpoint) return endpoint;
  }

  return null;
}

export async function resolveTrustedBonjourEndpoints(input: {
  fetchImpl: FetchLike;
  services: readonly BonjourService[];
  persistedEndpoints?: readonly TrustedLanEndpointHint[];
  trustedDesktopIds: readonly string[];
  preferredDesktopId: string | null;
  probeTimeoutMs?: number;
}): Promise<TrustedBonjourEndpoint[]> {
  const trustedDesktopIds = new Set(input.trustedDesktopIds);
  const candidates = await Promise.all(
    endpointCandidates(input).map((candidate) =>
      validateTrustedEndpoint(
        candidate,
        trustedDesktopIds,
        input.fetchImpl,
        input.probeTimeoutMs
      )
    )
  );
  const seenDesktopIds = new Set<string>();
  return candidates.filter((endpoint): endpoint is TrustedBonjourEndpoint => {
    if (!endpoint || seenDesktopIds.has(endpoint.desktopId)) return false;
    seenDesktopIds.add(endpoint.desktopId);
    return true;
  });
}

interface TrustedEndpointCandidate {
  baseUrl: string;
  desktopId: string;
}

function endpointCandidates(input: {
  services: readonly BonjourService[];
  persistedEndpoints?: readonly TrustedLanEndpointHint[];
  preferredDesktopId: string | null;
}): TrustedEndpointCandidate[] {
  const candidates: TrustedEndpointCandidate[] = [];
  const seen = new Set<string>();
  const add = (candidate: TrustedEndpointCandidate) => {
    const key = `${candidate.desktopId}\0${candidate.baseUrl}`;
    if (seen.has(key)) return;
    seen.add(key);
    candidates.push(candidate);
  };

  // A live advertisement is fresher than a saved address. The persisted
  // address is the cold-start fallback: pairing already authenticated the
  // desktop that supplied it, but status must still prove the same identity.
  for (const service of orderServices(input.services, input.preferredDesktopId)) {
    add({
      baseUrl: `http://${service.host}:${service.port}`,
      desktopId: service.txt.desktopId
    });
  }
  for (const endpoint of orderPersistedEndpoints(
    input.persistedEndpoints ?? [],
    input.preferredDesktopId
  )) {
    try {
      const url = new URL(endpoint.baseUrl);
      if (
        url.protocol !== "http:" ||
        url.pathname !== "/" ||
        url.username ||
        url.password ||
        url.search ||
        url.hash
      ) continue;
      add({
        baseUrl: url.origin,
        desktopId: endpoint.desktopId
      });
    } catch {
      // Persistence parsing is intentionally tolerant. A malformed old hint
      // is unusable, not a reason to suppress current Bonjour discovery.
    }
  }
  return candidates;
}

async function validateTrustedEndpoint(
  candidate: TrustedEndpointCandidate,
  trustedDesktopIds: ReadonlySet<string>,
  fetchImpl: FetchLike,
  probeTimeoutMs?: number
): Promise<TrustedBonjourEndpoint | null> {
  const desktopId = candidate.desktopId;
  if (!trustedDesktopIds.has(desktopId)) return null;

  const baseUrl = candidate.baseUrl;
  const status = await fetchDesktopStatus(baseUrl, fetchImpl, probeTimeoutMs);
  const displayName =
    typeof status?.desktopName === "string"
      ? status.desktopName.trim()
      : "";
  if (status?.desktopId !== desktopId || !displayName) return null;
  const agentProviders = parseAgentProviderInventory(status.agentProviders);
  return {
    baseUrl,
    desktopId,
    displayName,
    ...(agentProviders ? { agentProviders } : {})
  };
}

function orderPersistedEndpoints(
  endpoints: readonly TrustedLanEndpointHint[],
  selectedDesktopId: string | null
): TrustedLanEndpointHint[] {
  return [...endpoints].sort((left, right) => {
    const leftSelected = left.desktopId === selectedDesktopId ? 0 : 1;
    const rightSelected = right.desktopId === selectedDesktopId ? 0 : 1;
    return leftSelected - rightSelected;
  });
}

function orderServices(
  services: readonly BonjourService[],
  selectedDesktopId: string | null
): BonjourService[] {
  return [...services].sort((left, right) => {
    const leftSelected = left.txt.desktopId === selectedDesktopId ? 0 : 1;
    const rightSelected = right.txt.desktopId === selectedDesktopId ? 0 : 1;
    return leftSelected - rightSelected;
  });
}

export async function fetchDesktopStatus(
  baseUrl: string,
  fetchImpl: FetchLike,
  timeoutMs = 5_000
): Promise<{
  desktopId?: unknown;
  desktopName?: unknown;
  agentProviders?: unknown;
} | null> {
  const controller = new AbortController();
  let timeout: ReturnType<typeof setTimeout> | null = null;
  try {
    const response = await Promise.race([
      fetchImpl(`${baseUrl}/v1/status`, { signal: controller.signal }),
      new Promise<never>((_resolve, reject) => {
        timeout = setTimeout(() => {
          controller.abort();
          reject(new Error("LAN status probe timed out"));
        }, timeoutMs);
      })
    ]);
    if (!response.ok) {
      return null;
    }
    const body = await response.json();
    return body && typeof body === "object"
      ? (body as {
          desktopId?: unknown;
          desktopName?: unknown;
          agentProviders?: unknown;
        })
      : null;
  } catch {
    return null;
  } finally {
    if (timeout) clearTimeout(timeout);
  }
}
