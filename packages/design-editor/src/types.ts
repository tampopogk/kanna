/**
 * The App Design API as kanna-server serves it (crates/kanna-server/src/
 * design/service.rs and http_api/design.rs). Shared by the desktop and the
 * phone, which reach it through different transports.
 */

export interface DesignMockup {
  repoId: string;
  /** The artifact store's tree id of the published page. */
  artifactId: string;
  entrypoint: string;
  publishedAt: string;
}

export interface DesignPosition {
  name: string;
  label: string;
  artifact: string;
  /** The HTML mockup the position shows, once the agent has published one. */
  mockup?: DesignMockup | null;
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

/**
 * An element of a position's HTML mockup, as the person's click described it
 * (docs/specs/app-design.md §5). It comes from the mockup page, which the
 * agent wrote: text to show, never markup to render.
 */
export interface DesignElementAnchor {
  position: string;
  artifactId: string;
  /** The page inside the mockup, relative to its root. */
  page: string;
  selector: string;
  /** The element's visible text, parts joined with " · ". */
  excerpt: string;
  tag: string;
  /** `tag#id.class.class` */
  label: string;
  /** The nearest containing landmark, as a label. */
  context: string;
  html: string;
}

export interface DesignAnchor {
  blockId: string | null;
  quotedText: string | null;
  /** Document text: attached | pending | detached. A mockup pin: attached. */
  state: "attached" | "pending" | "detached";
  currentText?: string;
  /** A pin: the mockup element the comment is on. */
  element?: DesignElementAnchor;
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
  /** Selected document text, or a pinned mockup element. */
  anchor?: { blockId: string; quotedText: string; stateVector?: string } | { element: DesignElementAnchor };
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
