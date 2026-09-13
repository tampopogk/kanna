import type { AgentDefinition, WorkflowDefinition } from "../../../../packages/core/src/workflow/workflow-types";
import {
  fetchDesktopRepoAgentDefinition,
  fetchDesktopRepoWorkflowDefinition,
  fetchDesktopTaskDetail,
  type PinnedTaskWorkflow,
} from "../services/desktopServerClient";
import { postDesktopTaskAction } from "../services/desktopTaskActions";
import { requireService, type AdvanceStageOptions, type KannaSnapshot, type StoreContext } from "./state";
import { debugLog } from "../utils/debugLog";

export interface WorkflowApi {
  loadWorkflow: (repoId: string, workflowName: string) => Promise<WorkflowDefinition>;
  loadAgent: (repoId: string, agentName: string) => Promise<AgentDefinition>;
  advanceStage: (taskId: string, options?: AdvanceStageOptions) => Promise<AdvanceStageResult>;
  rerunStage: (taskId: string) => Promise<void>;
}

export type AdvanceStageResult = "advanced" | "ignored" | "failed";

export function createWorkflowApi(context: StoreContext): WorkflowApi {
  interface TaskActionResponse {
    taskId: string;
    followTask?: boolean;
  }

  interface StageAdvanceProjection {
    nextStageName: string | null;
    pendingPostName: string | null;
    closesOnSuccess: boolean;
  }

  // Selection during stage advance is only adjusted when the task being advanced
  // (and therefore closed) is the one currently selected — analogous to deletion,
  // where selection moves to the next visible task. When any other task advances
  // (including auto-advance), the user's selection must be left untouched.
  function computeNextVisibleItemId(currentItemId: string): string | null {
    const sortedItems = requireService(context.services.sortedItemsForCurrentRepo, "sortedItemsForCurrentRepo").value;
    const currentIndex = sortedItems.findIndex((candidate) => candidate.id === currentItemId);
    if (currentIndex === -1) return null;

    const remainingItems = sortedItems.filter((candidate) => candidate.id !== currentItemId);
    const nextIndex = currentIndex >= remainingItems.length ? remainingItems.length - 1 : currentIndex;
    return remainingItems[nextIndex]?.id ?? null;
  }

  async function restoreStageAdvanceSelection(itemId: string | null) {
    if (itemId) {
      const item = context.state.items.value.find((candidate) => candidate.id === itemId);
      const isItemHidden = requireService(context.services.isItemHidden, "isItemHidden");
      if (item && !isItemHidden(item) && item.repo_id === context.state.selectedRepoId.value) {
        debugLog("[workflow:advanceStage] restoring selection", {
          targetItemId: itemId,
          targetStage: item.stage,
          targetBranch: item.branch,
          selectedBefore: context.state.selectedItemId.value,
        });
        await requireService(context.services.selectItem, "selectItem")(itemId);
        return;
      }
    }

    debugLog("[workflow:advanceStage] clearing selection during restore", {
      requestedItemId: itemId,
      selectedBefore: context.state.selectedItemId.value,
    });
    const clearedSlotId = context.state.selectedItemId.value;
    const clearedRepoId = context.state.selectedRepoId.value;
    context.state.selectedItemId.value = null;
    if (
      clearedRepoId
      && context.state.lastSelectedItemByRepo.value[clearedRepoId] === clearedSlotId
    ) {
      const { [clearedRepoId]: _removed, ...remaining } = context.state.lastSelectedItemByRepo.value;
      context.state.lastSelectedItemByRepo.value = remaining;
    }
    await requireService(context.services.persistSelection, "persistSelection")();
  }

  function projectStageAdvance(
    stages: Array<{ name: string; post?: { name: string } | null }>,
    stage: string,
  ): StageAdvanceProjection | null {
    const currentIndex = stages.findIndex((candidate) => candidate.name === stage);
    if (currentIndex === -1) return null;
    const pendingPostName = stages[currentIndex]?.post?.name ?? null;
    return {
      nextStageName: stages[currentIndex + 1]?.name ?? null,
      pendingPostName,
      closesOnSuccess: currentIndex === stages.length - 1 && pendingPostName === null,
    };
  }

  /**
   * What this advance is being taken against: the projected next stage, and the
   * exact pinned workflow that projection was read from.
   *
   * A task's stage sequence is not its workflow name's. A plan can publish this
   * task's remaining stages while an earlier stage runs, and an edit can
   * repoint or remove a later one, so the repo file reads a grown task as one
   * stage long — projecting it as closing on the next advance, which moves
   * selection as if the task were gone, and equally can hide a real close
   * behind a stage the repo file still lists.
   *
   * `observedDefinition` is sent back as the advance's `expectedDefinition`, so
   * a tail that moved between reading and advancing is a refused conflict
   * rather than a silently different next stage. It is absent only when the
   * pinned definition could not be read at all; the advance then proceeds
   * unfenced exactly as it always did, rather than becoming unavailable.
   */
  interface ObservedStageAdvance extends StageAdvanceProjection {
    observedDefinition: PinnedTaskWorkflow | null;
  }

  async function resolveStageAdvanceProjection(item: {
    id: string;
    repo_id: string;
    pipeline: string;
    stage: string;
  }): Promise<ObservedStageAdvance> {
    const unknown: ObservedStageAdvance = {
      nextStageName: null,
      pendingPostName: null,
      closesOnSuccess: false,
      observedDefinition: null,
    };
    try {
      const pinned = (await fetchDesktopTaskDetail(item.id)).workflowDefinition;
      if (pinned?.stages?.length) {
        const projection = projectStageAdvance(
          pinned.stages.map((stage) => ({
            name: stage.name,
            post:
              stage.post && typeof stage.post === "object" && "name" in stage.post
                ? { name: String((stage.post as { name: unknown }).name) }
                : null,
          })),
          item.stage,
        );
        return { ...(projection ?? unknown), observedDefinition: pinned };
      }
    } catch (error) {
      console.debug("[workflow:advanceStage] pinned workflow unavailable for optimistic update:", error);
    }
    // No pinned definition to read: fall back to the repo file for the
    // cosmetic projection only, and send no fence, because nothing was
    // observed to fence on.
    try {
      const workflow = await loadWorkflow(item.repo_id, item.pipeline || "no-review");
      return { ...(projectStageAdvance(workflow.stages, item.stage) ?? unknown), observedDefinition: null };
    } catch (error) {
      console.debug("[workflow:advanceStage] could not resolve stage projection for optimistic update:", error);
      return unknown;
    }
  }

  async function withOptimisticStageAdvance<T>(
    taskId: string,
    sourceStageName: string,
    nextStageName: string | null,
    pendingPostName: string | null,
    run: () => Promise<T>,
  ): Promise<T> {
    if (!nextStageName && !pendingPostName) return run();
    return requireService(context.services.withOptimisticItemOverlay, "withOptimisticItemOverlay")({
      key: `advance-stage:${taskId}`,
      apply: (snapshot: KannaSnapshot): KannaSnapshot => ({
        ...snapshot,
        entries: snapshot.entries.map((entry) => ({
          ...entry,
          items: entry.items.map((candidate) =>
            candidate.id === taskId
              ? {
                  ...candidate,
                  ...(pendingPostName
                    ? {
                        active_post_action: candidate.active_post_action ?? pendingPostName,
                        has_running_post: 1,
                        activity: "working" as const,
                      }
                    : {
                        stage: nextStageName ?? candidate.stage,
                        stage_advance_pending: true,
                        stage_advance_from: sourceStageName,
                        activity: "working" as const,
                      }),
                }
              : candidate,
          ),
        })),
      }),
      run,
      // `run` owns an authoritative transition barrier. Once it resolves the
      // base snapshot is already reconciled, so another fetch here could only
      // turn a proven transition into a false client-side failure.
      reconcile: async () => {},
    });
  }

  function stageAdvanceSnapshotCaughtUp(
    snapshot: KannaSnapshot,
    taskId: string,
    nextStageName: string | null,
    pendingPostName: string | null,
    closesOnSuccess: boolean,
  ): boolean {
    const item = snapshot.entries
      .flatMap((entry) => entry.items)
      .find((candidate) => candidate.id === taskId);
    if (!item || item.closed_at != null) return true;
    if (closesOnSuccess) return false;
    if (pendingPostName) {
      return Boolean(item.has_running_post) || item.active_post_action === pendingPostName;
    }
    if (nextStageName) {
      return item.stage === nextStageName;
    }
    return true;
  }

  async function waitForStageAdvanceSnapshot(
    taskId: string,
    nextStageName: string | null,
    pendingPostName: string | null,
    closesOnSuccess: boolean,
    initialTransitionRevision: string | null,
  ): Promise<void> {
    const reloadSnapshot = requireService(context.services.reloadSnapshot, "reloadSnapshot");
    const waitForAuthoritativeSnapshot = requireService(
      context.services.waitForAuthoritativeSnapshot,
      "waitForAuthoritativeSnapshot",
    );
    const abortController = new AbortController();
    let failureMessage: string | null = null;
    const settledSnapshotPromise = waitForAuthoritativeSnapshot(async (snapshot) => {
      if (
        stageAdvanceSnapshotCaughtUp(
          snapshot,
          taskId,
          nextStageName,
          pendingPostName,
          closesOnSuccess,
        )
      ) return true;
      const item = snapshot.entries
        .flatMap((entry) => entry.items)
        .find((candidate) => candidate.id === taskId);
      if ((item?.transition_revision ?? null) === initialTransitionRevision) return false;

      // The successor run is inserted immediately before its daemon spawn,
      // while the stage itself moves only after SessionCreated. A snapshot in
      // that narrow window is still pending, not a failure. Only the run's
      // durable failed verdict may end the projection without the stage move.
      let detail: Awaited<ReturnType<typeof fetchDesktopTaskDetail>>;
      try {
        detail = await fetchDesktopTaskDetail(taskId);
      } catch (error) {
        console.warn("[workflow:advanceStage] could not inspect successor run; transition remains pending:", error);
        return false;
      }
      const latestRun = detail.latestRun;
      if (
        latestRun
        && latestRun.id === item?.transition_revision
        && latestRun.status === "failed"
      ) {
        failureMessage = latestRun.summary
          ?? `Stage advance failed; task remained at ${detail.stage ?? "its current stage"}.`;
        return true;
      }
      return false;
    }, { signal: abortController.signal });

    try {
      await reloadSnapshot();
    } catch (error) {
      // The action was already accepted. A failed observation cannot prove
      // that the transition failed, so retain the projection and let the next
      // authoritative stream/snapshot refresh settle the registered barrier.
      console.warn("[workflow:advanceStage] first post-acceptance snapshot reload failed; transition remains pending:", error);
    }

    let settledSnapshot: KannaSnapshot;
    try {
      settledSnapshot = await settledSnapshotPromise;
    } finally {
      // Normally the waiter removes itself when its predicate settles. Abort
      // also releases it if this operation is cancelled while still pending.
      abortController.abort();
    }
    if (
      stageAdvanceSnapshotCaughtUp(
        settledSnapshot,
        taskId,
        nextStageName,
        pendingPostName,
        closesOnSuccess,
      )
    ) return;
    throw new Error(failureMessage ?? "Stage advance failed before the target stage became durable.");
  }

  async function loadWorkflow(repoId: string, workflowName: string): Promise<WorkflowDefinition> {
    const cacheKey = `${repoId}::${workflowName}`;
    const response = await fetchDesktopRepoWorkflowDefinition(repoId, workflowName);
    const cached = context.state.workflowCache.get(cacheKey);
    if (cached?.revision === response.revision) return cached.definition;

    context.state.workflowCache.set(cacheKey, response);
    return response.definition;
  }

  async function loadAgent(repoId: string, agentName: string): Promise<AgentDefinition> {
    const cacheKey = `${repoId}::${agentName}`;
    const response = await fetchDesktopRepoAgentDefinition(repoId, agentName);
    const cached = context.state.agentCache.get(cacheKey);
    if (cached?.revision === response.revision) return cached.definition;

    context.state.agentCache.set(cacheKey, response);
    return response.definition;
  }

  async function advanceStage(
    taskId: string,
    options: AdvanceStageOptions = {},
  ): Promise<AdvanceStageResult> {
    const item = context.state.items.value.find((candidate) => candidate.id === taskId);
    if (!item) return "ignored";
    if (item.closed_at != null) return "ignored";
    if (item.stage_advance_pending) return "ignored";
    // Single-flight: while a post (e.g. approve) runs, an ordinary repeated
    // advance would hit the backend's running-post override and transition
    // the stage before the post finishes its work. Only the post's own
    // completion may move the task.
    if (item.has_running_post) {
      context.toast.warning(context.tt("toasts.stagePostRunning"));
      return "ignored";
    }
    const sourceTaskIsSelected = requireService(context.services.selectedTaskId, "selectedTaskId").value === item.id;
    const fallbackSelectionId = computeNextVisibleItemId(item.id);
    const initialTransitionRevision = item.transition_revision ?? null;
    const { nextStageName, pendingPostName, closesOnSuccess, observedDefinition } =
      await resolveStageAdvanceProjection(item);
    debugLog("[workflow:advanceStage] selection policy", {
      taskId,
      currentStage: item.stage,
      optimisticNextStage: nextStageName,
      optimisticPendingPost: pendingPostName,
      closesOnSuccess,
      initiatedBy: options.initiatedBy ?? "manual",
      sourceTaskIsSelected,
      fallbackSelectionId,
      selectedBefore: context.state.selectedItemId.value,
    });

    try {
      return await withOptimisticStageAdvance(taskId, item.stage, nextStageName, pendingPostName, async () => {
        const response = await postDesktopTaskAction(taskId, "advance-stage", {
          source: "operator",
          // The workflow this advance was actually taken against.
          ...(observedDefinition ? { expectedDefinition: observedDefinition } : {}),
          ...(options.nextStageAgentProvider ? {
            nextStageAgentProvider: options.nextStageAgentProvider,
            nextStageModel: options.nextStageModel,
            nextStageEffort: options.nextStageEffort,
            nextStageProviderSource: "operator",
          } : {}),
        });
        if (!response.ok) {
          const message = await response.text();
          if (response.status === 409) {
            // A refused fence is a different fact from a blocked task: the
            // stages moved under the person who pressed the key, and saying
            // "blocked" would send them looking for a blocker that is not there.
            context.toast.warning(
              message.includes("pinned workflow changed")
                ? context.tt("mainPanel.stageSequenceChanged")
                : context.tt("mainPanel.taskBlocked"),
            );
            return "ignored" as const;
          }
          throw new Error(message);
        }
        const result = await response.json() as TaskActionResponse;
        await waitForStageAdvanceSnapshot(
          result.taskId,
          nextStageName,
          pendingPostName,
          closesOnSuccess,
          initialTransitionRevision,
        );

        // Durable tasks: an in-workflow advance transitions the SAME task in
        // place, so the user's selection stays put. Only when the advance
        // closed the task (final stage) does selection move to the next
        // visible item — analogous to closing a task.
        const advancedItem = context.state.items.value.find((candidate) => candidate.id === result.taskId);
        const taskClosed = !advancedItem || advancedItem.closed_at != null;
        if (taskClosed && sourceTaskIsSelected) {
          await restoreStageAdvanceSelection(fallbackSelectionId);
        }
        return "advanced" as const;
      });
    } catch (error) {
      console.error("[store] advanceStage: server action failed:", error);
      context.toast.error(`${context.tt("toasts.agentStartFailed")}: ${error instanceof Error ? error.message : error}`);
      return "failed";
    }
  }

  async function rerunStage(taskId: string): Promise<void> {
    const item = context.state.items.value.find((candidate) => candidate.id === taskId);
    if (!item) return;
    if (item.closed_at != null) return;

    try {
      const response = await postDesktopTaskAction(taskId, "rerun-stage");
      if (!response.ok) {
        throw new Error(await response.text());
      }
      await requireService(context.services.reloadSnapshot, "reloadSnapshot")();
    } catch (error) {
      console.error("[store] rerunStage: server action failed:", error);
      context.toast.error(`${context.tt("toasts.agentStartFailed")}: ${error instanceof Error ? error.message : error}`);
    }
  }

  return {
    loadWorkflow,
    loadAgent,
    advanceStage,
    rerunStage,
  };
}
