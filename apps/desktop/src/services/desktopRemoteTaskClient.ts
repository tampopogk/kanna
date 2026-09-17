import type { CompanionEvent } from "@kanna/agent-protocol";
import type {
  CompanionEventResult,
  CompanionSnapshot,
} from "@kanna/stream-client";

export type DesktopRemoteTerminalEvent =
  | { type: "snapshot"; taskId: string; cols: number; rows: number; data: Uint8Array }
  | { type: "output"; taskId: string; data: Uint8Array }
  | { type: "exit"; taskId: string; code: number }
  | { type: "error"; taskId: string; message: string };

export interface DesktopRemoteTerminalSubscription {
  close(): void;
  registerViewer?(cols: number, rows: number): void;
  setViewerVisible?(visible: boolean): void;
  activate?(): void;
}

export interface ObserveDesktopRemoteTerminalOptions {
  desktopId: string;
  taskId: string;
  listener(event: DesktopRemoteTerminalEvent): void;
}

export type DesktopRemoteCompanionEvent =
  | { type: "snapshot"; taskId: string; snapshot: CompanionSnapshot }
  | { type: "unavailable"; taskId: string }
  | { type: "event_result"; taskId: string; result: CompanionEventResult }
  | { type: "connection"; taskId: string; connected: boolean }
  | { type: "error"; taskId: string; code: string; message: string };

export interface ObserveDesktopRemoteCompanionOptions {
  desktopId: string;
  taskId: string;
  listener(event: DesktopRemoteCompanionEvent): void;
}

export interface DesktopRemoteCompanionSubscription {
  close(): void;
  sendEvent(
    sessionId: string,
    revision: string,
    event: CompanionEvent,
  ): boolean;
}

export interface RemoteTaskActionOptions {
  desktopId: string;
  taskId: string;
}

export interface MarkRemoteTaskReadOptions extends RemoteTaskActionOptions {
  expectedActivityRevision: number;
}

export interface AdvanceRemoteTaskStageOptions extends RemoteTaskActionOptions {
  expectedTransitionRevision?: string;
}

export interface SendRemoteTerminalInputOptions extends RemoteTaskActionOptions {
  data: string;
  submissionBoundary?: boolean;
  controlInput?: boolean;
}

export interface ResizeRemoteTerminalOptions extends RemoteTaskActionOptions {
  cols: number;
  rows: number;
}

export interface ReadRemoteTaskFileOptions extends RemoteTaskActionOptions {
  path: string;
}

export interface RemoteTaskFileContent {
  path: string;
  content: string;
}

export interface RemoteTaskDirectoryEntry {
  name: string;
  path: string;
  isDir: boolean;
  size?: number | null;
}

export interface RemoteTaskDirectoryListing {
  path: string;
  entries: RemoteTaskDirectoryEntry[];
  offset: number;
  nextOffset: number | null;
  totalEntries: number;
}

export type RemoteTaskDiffRequest =
  | { scope: "branch"; mode: "none" | "staged" | "all" }
  | { scope: "working"; mode: "all" | "unstaged" | "staged" };

export interface RemoteTaskDiffContent {
  taskId: string;
  baseRef: string | null;
  mergeBase: string | null;
  patch: string;
  truncated: boolean;
}

export interface RemoteTaskGraphContent {
  taskId: string;
  commits: import("../utils/commitGraph").GraphCommit[];
  headCommit: string | null;
}

/** Omit `fromRef` to walk every owner ref; HEAD limits the graph to task HEAD. */
export interface RemoteTaskGraphRequest {
  fromRef?: "HEAD";
}

export interface AgentTerminalAttempt {
  id: string;
  stage: string;
  /**
   * Which stream this attempt is: "main" is the stage's agent session,
   * "teardown" is the workspace cleanup that ran when the task left that
   * workspace. A teardown is labelled by the stage whose workspace it tore
   * down, never by the stage the task entered.
   */
  kind: "main" | "teardown";
  startedAt: string;
  cwd: string | null;
  /**
   * The owner's daemon still runs the terminal this attempt was launched into,
   * so it is the live session rather than history. Only the session registry
   * can say this: the launching run finishes at a manual gate while its PTY
   * keeps running and a post continues in that same terminal, and a finished
   * attempt whose final frame never arrived is history with nothing to show,
   * which is all `archived` reports.
   */
  live: boolean;
  archived: boolean;
  recordedLaunch: boolean;
  observedExitCode: number | null;
}

export interface AgentTerminalArchive {
  binding: { task_id: string; spawned_run_id: string };
  session_id: string;
  cwd: string;
  snapshot: { vt: string; cols: number; rows: number } | null;
  unavailable_reason: string | null;
  observed_exit_code: number | null;
}

export interface ReadRemoteAgentTerminalArchiveOptions extends RemoteTaskActionOptions {
  runId: string;
}

export interface DesktopRemoteTerminalClient {
  close(): void;
  observeTerminal(
    options: ObserveDesktopRemoteTerminalOptions,
  ): DesktopRemoteTerminalSubscription;
  sendInput(options: SendRemoteTerminalInputOptions): Promise<void>;
  resize(options: ResizeRemoteTerminalOptions): Promise<void>;
  closeTask(options: RemoteTaskActionOptions): Promise<void>;
  advanceStage(options: AdvanceRemoteTaskStageOptions): Promise<void>;
  readTaskFile(options: ReadRemoteTaskFileOptions): Promise<RemoteTaskFileContent>;
  markTaskRead(options: MarkRemoteTaskReadOptions): Promise<void>;
}

export interface DesktopRemoteTaskClient extends DesktopRemoteTerminalClient {
  observeCompanion(
    options: ObserveDesktopRemoteCompanionOptions,
  ): DesktopRemoteCompanionSubscription;
}

export interface DesktopRemoteTaskViewClient extends DesktopRemoteTaskClient {
  listTaskDirectory(
    options: ReadRemoteTaskFileOptions & { showAllFiles?: boolean },
  ): Promise<RemoteTaskDirectoryListing>;
  readTaskDiff(
    options: RemoteTaskActionOptions & { request: RemoteTaskDiffRequest },
  ): Promise<RemoteTaskDiffContent>;
  readTaskGraph(
    options: RemoteTaskActionOptions & { request: RemoteTaskGraphRequest },
  ): Promise<RemoteTaskGraphContent>;
  listAgentTerminalAttempts(
    options: RemoteTaskActionOptions,
  ): Promise<AgentTerminalAttempt[]>;
  readAgentTerminalArchive(
    options: ReadRemoteAgentTerminalArchiveOptions,
  ): Promise<AgentTerminalArchive | null>;
}
