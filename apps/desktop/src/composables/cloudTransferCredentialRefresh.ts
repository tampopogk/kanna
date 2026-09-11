import { CloudTransferSignInRequiredError } from "../services/desktopTransferMachines";

export const CLOUD_TRANSFER_CREDENTIAL_REFRESH_EVENT = "cloud-transfer-credential-refresh";

export interface CloudTransferCredentialRefreshCommand {
  requestId: string;
  peerId: string;
}

export type CloudTransferCredentialRefreshOutcome =
  | "refreshed"
  | "sign_in_required"
  | "refresh_failed";

export function parseCloudTransferCredentialRefreshCommand(
  payload: unknown,
): CloudTransferCredentialRefreshCommand {
  if (!payload || typeof payload !== "object") {
    throw new Error("cloud transfer credential refresh command must be an object");
  }
  const record = payload as Record<string, unknown>;
  const requestId = typeof record.requestId === "string" ? record.requestId.trim() : "";
  const peerId = typeof record.peerId === "string" ? record.peerId.trim() : "";
  if (!requestId || !peerId) {
    throw new Error("cloud transfer credential refresh command is missing requestId or peerId");
  }
  return { requestId, peerId };
}

export async function performCloudTransferCredentialRefresh(
  command: CloudTransferCredentialRefreshCommand,
  refreshRoute: (peerId: string) => Promise<void>,
): Promise<CloudTransferCredentialRefreshOutcome> {
  try {
    await refreshRoute(command.peerId);
    return "refreshed";
  } catch (error) {
    if (error instanceof CloudTransferSignInRequiredError) {
      return "sign_in_required";
    }
    return "refresh_failed";
  }
}
