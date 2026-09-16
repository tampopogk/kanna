import type { AgentProvider } from "@kanna/agent-protocol";
import type { DesktopSummary } from "../lib/api/types";
import type {
  TrustedDesktopLanEndpoint,
  TrustedDesktopRecord
} from "./sessionPersistence";
import type { SecureChannelStatus } from "../lib/security/secureChannelPeer";

export interface MobileMachine {
  desktopId: string;
  displayName: string;
  origins: {
    account: boolean;
    manual: boolean;
  };
  availability: {
    lan: boolean;
    cloud: boolean;
    lastSeenAt: string | null;
  };
  /** Agent provider CLIs the machine reported. Absent when neither source
   * carried one — an older desktop, or a machine known only from the manual
   * pairing record — which callers read as "unknown", not "none". */
  agentProviders?: AgentProvider[];
  lanEndpoints: TrustedDesktopLanEndpoint[];
  /**
   * How this phone's connection to the machine is secured, for a manually
   * paired machine: an observed sealed/refused outcome when one exists,
   * otherwise `sealed` for a pairing with a pinned desktop key that has not
   * yet been exercised, `legacy` for a pairing without one. Absent for a
   * machine known only through the account (cloud-only: it is reached as
   * legacy relay control until it is paired).
   */
  secureChannel?: SecureChannelStatus;
}

export function buildMachineInventory(input: {
  accountDesktops: readonly DesktopSummary[];
  manualDesktops: readonly TrustedDesktopRecord[];
  liveLanDesktops: readonly DesktopSummary[];
  secureChannelStates?: Readonly<Record<string, SecureChannelStatus>>;
}): MobileMachine[] {
  const accountById = new Map(
    input.accountDesktops.map((desktop) => [desktop.id, desktop] as const)
  );
  const manualById = new Map(
    input.manualDesktops.map((desktop) => [desktop.desktopId, desktop] as const)
  );
  const liveLanById = new Map(
    input.liveLanDesktops.map((desktop) => [desktop.id, desktop] as const)
  );
  const desktopIds = new Set([
    ...accountById.keys(),
    ...manualById.keys(),
    ...liveLanById.keys()
  ]);
  const machines = Array.from(desktopIds, (desktopId): MobileMachine => {
    const account = accountById.get(desktopId);
    const manual = manualById.get(desktopId);
    const liveLan = liveLanById.get(desktopId);
    const lan = liveLan?.online === true;
    // The live LAN read came from the machine itself; the account record is a
    // published snapshot that can lag it.
    const agentProviders = liveLan?.agentProviders ?? account?.agentProviders;
    const cloud = Boolean(
      account?.reachableViaRelay === true ||
      (account?.mode === "remote" && account.online)
    );

    return {
      desktopId,
      displayName: freshestDisplayName({ account, liveLan, manual }) ?? desktopId,
      origins: {
        account: account !== undefined,
        manual: manual !== undefined
      },
      availability: {
        lan,
        cloud,
        lastSeenAt: latestTimestamp(
          account?.lastSeenAt,
          liveLan?.lastSeenAt,
          manual?.lastSeenAt
        )
      },
      ...(agentProviders ? { agentProviders } : {}),
      lanEndpoints: manual?.lanEndpoints ?? [],
      ...(manual
        ? {
            secureChannel:
              input.secureChannelStates?.[desktopId] ??
              (manual.channelPublicKey ? { mode: "sealed" as const } : { mode: "legacy" as const })
          }
        : {})
    };
  });

  return machines.sort((left, right) => {
    const leftAvailable = left.availability.lan || left.availability.cloud;
    const rightAvailable = right.availability.lan || right.availability.cloud;
    if (leftAvailable !== rightAvailable) return leftAvailable ? -1 : 1;
    return left.displayName.localeCompare(right.displayName);
  });
}

export function summarizeMachines(machines: readonly MobileMachine[]): {
  total: number;
  available: number;
} {
  return {
    total: machines.length,
    available: machines.filter(
      (machine) => machine.availability.lan || machine.availability.cloud
    ).length
  };
}

function latestTimestamp(
  ...values: Array<string | null | undefined>
): string | null {
  const timestamps = values.filter(
    (value): value is string => typeof value === "string" && value.length > 0
  );
  timestamps.sort();
  return timestamps[timestamps.length - 1] ?? null;
}

function freshestDisplayName({
  account,
  liveLan,
  manual
}: {
  account?: DesktopSummary;
  liveLan?: DesktopSummary;
  manual?: TrustedDesktopRecord;
}): string | null {
  const candidates = [
    { name: manual?.displayName, timestamp: manual?.lastSeenAt ?? "" },
    { name: account?.name, timestamp: account?.lastSeenAt ?? "" },
    { name: liveLan?.name, timestamp: liveLan?.lastSeenAt ?? "" }
  ].filter(
    (candidate): candidate is { name: string; timestamp: string } =>
      typeof candidate.name === "string" && candidate.name.trim().length > 0
  );
  candidates.sort((left, right) => left.timestamp.localeCompare(right.timestamp));
  return candidates[candidates.length - 1]?.name.trim() ?? null;
}
