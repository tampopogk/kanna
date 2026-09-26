import { describe, expect, it } from "vitest";
import { resolveTaskItemForDaemonSession, type TaskSessionIdentity } from "./taskSessionIdentity";
import type { PipelineItem } from "../types/kanna";

function item(overrides: Partial<TaskSessionIdentity> & { id: string }): TaskSessionIdentity {
  return { branch: null, workspace_id: null, ...overrides };
}

/**
 * A full `PipelineItem` as `/v1/snapshot` actually shapes one — this is the
 * type `context.state.items.value` holds in the real store, not the trimmed
 * `TaskSessionIdentity` fixture the tests above use. `workspace_id` mirrors
 * `crates/kanna-server/src/db/snapshot.rs`'s `list_snapshot_pipeline_items`,
 * which reads it from the latest agent `stage_run`'s T2 session identity.
 */
function pipelineItem(overrides: Partial<PipelineItem> & { id: string }): PipelineItem {
  return {
    repo_id: "repo-1",
    issue_number: null,
    issue_title: null,
    prompt: "Fix port ordering",
    pipeline: "default",
    pipeline_def: null,
    stage: "in progress",
    pr_number: null,
    pr_url: null,
    branch: null,
    closed_at: null,
    agent_type: null,
    agent_provider: "claude",
    activity: "idle",
    activity_revision: 0,
    activity_changed_at: null,
    unread_at: null,
    port_offset: null,
    attention_requested: false,
    display_name: null,
    last_output_preview: null,
    port_env: null,
    agent_spawn_options: null,
    pinned: 0,
    pin_order: null,
    base_ref: null,
    agent_session_id: null,
    workspace_id: null,
    teardown_started_at: null,
    parent_task_id: null,
    notify_task_id: null,
    notified_at: null,
    created_at: "2026-09-23T00:00:00.000Z",
    updated_at: "2026-09-23T00:00:00.000Z",
    ...overrides,
  };
}

describe("resolveTaskItemForDaemonSession", () => {
  it("matches on task id first", () => {
    const items = [item({ id: "task-a", workspace_id: "ws-task-a" }), item({ id: "task-b" })];
    expect(resolveTaskItemForDaemonSession(items, "task-a")?.id).toBe("task-a");
  });

  it("matches on the T2 workspace id when the daemon session id is not a task id (T11b)", () => {
    const items = [
      item({ id: "task-a", branch: "task-a-build", workspace_id: "ws-task-a-build-2" }),
      item({ id: "task-b", branch: "task-b-plan", workspace_id: "ws-task-b-plan-1" }),
    ];
    expect(resolveTaskItemForDaemonSession(items, "ws-task-b-plan-1")?.id).toBe("task-b");
  });

  it("prefers the workspace id over a stale branch match once the task has moved stages", () => {
    // The task forked a new workspace/branch on its most recent stage
    // transition; an older branch-keyed session id must not resolve here.
    const items = [
      item({ id: "task-a", branch: "task-a-review", workspace_id: "ws-task-a-review-3" }),
    ];
    expect(resolveTaskItemForDaemonSession(items, "ws-task-a-review-3")?.id).toBe("task-a");
    expect(resolveTaskItemForDaemonSession(items, "task-a-plan")).toBeNull();
  });

  it("falls back to branch when no workspace id is recorded — an older server payload", () => {
    const items = [item({ id: "task-a", branch: "task-a-plan", workspace_id: null })];
    expect(resolveTaskItemForDaemonSession(items, "task-a-plan")?.id).toBe("task-a");
  });

  it("returns null when nothing matches", () => {
    const items = [item({ id: "task-a", branch: "task-a-plan", workspace_id: "ws-task-a-plan-1" })];
    expect(resolveTaskItemForDaemonSession(items, "unrelated")).toBeNull();
  });

  it("resolves by workspace id against a real server-shaped snapshot payload (T11b)", () => {
    const items: PipelineItem[] = [
      pipelineItem({ id: "task-a", branch: "task-a-build", workspace_id: "ws-task-a-build-2" }),
      pipelineItem({ id: "task-b", branch: "task-b-plan", workspace_id: "ws-task-b-plan-1" }),
    ];
    expect(resolveTaskItemForDaemonSession(items, "ws-task-a-build-2")?.id).toBe("task-a");
  });

  it("falls back to branch against a snapshot from a server that omits workspace_id", () => {
    // `workspace_id` is `#[serde(default, skip_serializing_if = "Option::is_none")]`
    // on the wire, so an older server's snapshot simply omits the key rather
    // than sending it as null.
    const olderServerItem = pipelineItem({ id: "task-a", branch: "task-a-plan" });
    delete (olderServerItem as { workspace_id?: string | null }).workspace_id;
    const items: PipelineItem[] = [olderServerItem];

    expect(resolveTaskItemForDaemonSession(items, "task-a-plan")?.id).toBe("task-a");
  });
});
