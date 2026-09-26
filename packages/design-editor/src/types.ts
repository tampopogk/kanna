/**
 * The App Design API as kanna-server serves it (crates/kanna-server/src/
 * design/service.rs and http_api/design.rs). Shared by the desktop and the
 * phone, which reach it through different transports.
 */

export interface DesignPosition {
  name: string;
  label: string;
  artifact: string;
}

export interface DesignDelivery {
  id: string;
  /** queued | delivering | delivered | uncertain | cancelled */
  state: string;
  detail?: string | null;
  deliveredAt?: string | null;
}

export interface DesignComment {
  id: string;
  author: "operator" | "agent";
  body: string;
  createdAt: string;
  delivery?: DesignDelivery;
}

export interface DesignAnchor {
  blockId: string | null;
  quotedText: string | null;
  /** attached | pending | detached */
  state: "attached" | "pending" | "detached";
  currentText?: string;
}

/** queued | delivering | delivered | agent_replied | uncertain | held | none */
export type DesignDeliveryStatus =
  | "queued"
  | "delivering"
  | "delivered"
  | "agent_replied"
  | "uncertain"
  | "held"
  | "cancelled"
  | "none";

export interface DesignThread {
  id: string;
  number: number;
  kind: "comment" | "message";
  status: "open" | "resolved";
  anchor: DesignAnchor | null;
  comments: DesignComment[];
  deliveryStatus: DesignDeliveryStatus;
  createdAt: string;
  resolvedAt: string | null;
  resolvedBy: string | null;
}

export interface DesignApproval {
  id: string;
  /** candidate | approved | exported | committing | committed | entered | invalidated | failed */
  phase: string;
  epoch: number;
  docRevision: number;
  artifactId: string | null;
  artifactRepoId: string | null;
  sourceCommit: string | null;
  committedSha: string | null;
  retained: Array<{ path: string; sha256: string }> | null;
  policy: { retain: string; path: string; files: string[] } | null;
  error: string | null;
  approvedAt: string | null;
  confirmationExpiresAt: string | null;
  stale: boolean;
}

export interface DesignView {
  taskId: string;
  stage: string;
  nextStage: string | null;
  currentStage: string | null;
  inDesignStage: boolean;
  stageChain: string[];
  epoch: number;
  /** designing | handing_off | handed_off */
  status: string;
  position: string;
  positions: DesignPosition[];
  schemaVersion: string;
  docRevision: number;
  feedRevision: number;
  threads: DesignThread[];
  approval: DesignApproval | null;
  agentRuntime: string | null;
  scratchRepository: string | null;
}

export interface DesignCandidate {
  approval: DesignApproval;
  confirmationToken: string;
  policy: { retain: string; path: string; files: string[] };
  nextStage: string | null;
  openThreads: number;
  undeliveredFeedback: number;
  skippedSourceFiles: string[];
}

export interface DesignSyncRequest {
  schemaVersion: string;
  /** Base64 of the client's Yjs state vector. */
  stateVector: string;
  /** Base64 of a Yjs update, when the client has changes. */
  update?: string;
}

export interface DesignSyncResponse {
  update: string;
  stateVector: string;
  revision: number;
}

export interface DesignChanges {
  docRevision: number;
  feedRevision: number;
}

export interface CreateThreadRequest {
  threadId: string;
  commentId: string;
  kind: "comment" | "message";
  body: string;
  anchor?: { blockId: string; quotedText: string; stateVector?: string };
}

/**
 * How a client reaches kanna-server. The desktop implements it over its
 * authenticated local HTTP client; the phone's WebView over a bridge to the
 * app's LAN or relay transport. Every body is JSON, so both work.
 */
export interface DesignTransport {
  view(): Promise<DesignView>;
  changes(known: { doc?: number; feed?: number; timeoutMs?: number }): Promise<DesignChanges>;
  sync(request: DesignSyncRequest): Promise<DesignSyncResponse>;
  createThread(request: CreateThreadRequest): Promise<DesignThread>;
  reply(threadId: string, request: { commentId: string; body: string }): Promise<DesignThread>;
  resolve(threadId: string, resolved: boolean): Promise<DesignThread>;
  retryDelivery(deliveryId: string): Promise<void>;
}

/** An error the server answered with (status and its JSON body). */
export class DesignRequestError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly reason: string | null,
  ) {
    super(message);
  }
}
