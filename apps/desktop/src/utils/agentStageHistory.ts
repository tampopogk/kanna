/**
 * The agent tab's history list: what a task's stage selector offers besides
 * the live session.
 *
 * A task leaves three kinds of stored stream behind, all addressed by the
 * stage run that produced them. The agent's own PTY scrollback is the one the
 * selector always had; beside it the server records the workspace teardown
 * that ran when the task left a workspace (`stage_run.kind = 'teardown'`, an
 * ordinary terminal archive) and the workspace setup that prepared each spawn
 * (`workspace_setup_run`, captured text rather than terminal frames).
 *
 * Two rules the selector cannot get from the list order alone:
 * - "attempt N" counts the agent's own sessions. A teardown is not an attempt
 *   and must not consume an ordinal, or every attempt after the first
 *   workspace fork is numbered for a session the user never ran.
 * - A stream is labelled by the stage whose workspace it acted on, which the
 *   run row already carries — never by the stage the task went on to enter.
 */
import type { AgentTerminalAttempt, WorkspaceSetupRun } from "../services/desktopServerClient";

/**
 * A setup stream shares its run id with the agent attempt it prepared, so the
 * two need distinct option values. Run ids are uuids and never carry this
 * prefix themselves.
 */
const SETUP_SELECTION_PREFIX = "setup:";

export type StageHistoryKind = "attempt" | "teardown" | "setup";

export interface StageHistoryItem {
  /** The `<option>` value, and what the agent tab holds as its selection. */
  value: string;
  kind: StageHistoryKind;
  /** The stage whose workspace this stream belongs to. */
  stage: string;
  /** Compact name for the tab chrome while this item is selected. */
  title: string;
  label: string;
}

export type StageHistorySelection =
  | { kind: "latest" }
  | { kind: "attempt"; runId: string }
  | { kind: "setup"; runId: string };

export function setupSelectionValue(runId: string): string {
  return `${SETUP_SELECTION_PREFIX}${runId}`;
}

/**
 * Read a selection token. A teardown is an ordinary terminal archive, so it
 * resolves to `attempt` — the archive reader is addressed by run id and does
 * not care which kind of session produced the frames.
 */
export function parseStageHistorySelection(value: string): StageHistorySelection {
  if (!value) return { kind: "latest" };
  if (value.startsWith(SETUP_SELECTION_PREFIX)) {
    return { kind: "setup", runId: value.slice(SETUP_SELECTION_PREFIX.length) };
  }
  return { kind: "attempt", runId: value };
}

function archiveSuffix(attempt: AgentTerminalAttempt): string {
  return attempt.archived ? "" : " · history unavailable";
}

/**
 * Every history item a task offers, newest first.
 *
 * The live attempt is left out: "Latest" already shows that session, so
 * listing it again would repeat it as a permanently unavailable row. Its setup
 * stream is still history — it finished before the session started — so it
 * stays. Each stage run's setup sits just after its own attempt in the
 * newest-first order, which is where it ran in time.
 */
export function stageHistoryItems(
  attempts: AgentTerminalAttempt[],
  setupRuns: WorkspaceSetupRun[],
): StageHistoryItem[] {
  const setupByRun = new Map(setupRuns.map(run => [run.runId, run]));
  const items: StageHistoryItem[] = [];
  let ordinal = 0;
  for (const attempt of attempts) {
    const teardown = attempt.kind === "teardown";
    if (!teardown) ordinal += 1;
    const setup = setupByRun.get(attempt.id);
    if (setup) {
      setupByRun.delete(attempt.id);
      items.push(setupItem(setup, attempt.stage));
    }
    if (attempt.live) continue;
    items.push(teardown
      ? {
        value: attempt.id,
        kind: "teardown",
        stage: attempt.stage,
        title: `Teardown · ${attempt.stage}`,
        label: `Teardown · ${attempt.stage} · ${attempt.startedAt}${archiveSuffix(attempt)}`,
      }
      : {
        value: attempt.id,
        kind: "attempt",
        stage: attempt.stage,
        title: attempt.stage,
        label: `${attempt.stage} · attempt ${ordinal} · ${attempt.startedAt}${archiveSuffix(attempt)}`,
      });
  }
  // A setup record whose run left no launch row still has output worth
  // reading; it has no stage to borrow, so it is labelled by its own time.
  for (const run of setupRuns) {
    if (setupByRun.has(run.runId)) items.push(setupItem(run, null));
  }
  return items.reverse();
}

function setupItem(run: WorkspaceSetupRun, stage: string | null): StageHistoryItem {
  const failed = run.status !== "succeeded" ? " · failed" : "";
  const scope = stage ? `${stage} · ` : "";
  return {
    value: setupSelectionValue(run.runId),
    kind: "setup",
    stage: stage ?? "Setup",
    title: stage ? `Setup · ${stage}` : "Setup",
    label: `Setup · ${scope}${run.finishedAt}${failed}`,
  };
}
