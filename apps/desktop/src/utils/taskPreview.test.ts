import { describe, expect, it } from "vitest";
import { localTaskPreviewUrl } from "./taskPreview";
import type { DesktopTaskDetail } from "../services/desktopServerClient";
const detail: DesktopTaskDetail = {
  id: "task-a", stage: "in progress", closedAt: null, latestRun: null,
  revisionRounds: 0, revisionLimit: 5, childTaskIds: [],
  worktreePath: "/repo/task-a", ports: [{ name: "DEV_PORT", port: 4321 }],
};
const expected = { taskId: "task-a", workspace: "/repo/task-a", portName: "DEV_PORT" };
describe("local task preview routing", () => {
  it("uses the current server-owned port, never a saved URL", () => {
    expect(localTaskPreviewUrl(detail, expected)).toBe("http://localhost:4321");
    expect(localTaskPreviewUrl({ ...detail, ports: [{ name: "DEV_PORT", port: 4322 }] }, expected)).toBe("http://localhost:4322");
  });
  it.each([
    { ...detail, id: "other" },
    { ...detail, worktreePath: "/repo/task-a-2" },
    { ...detail, closedAt: "2026-09-12" },
    { ...detail, ports: [] },
    { ...detail, ports: [{ name: "DEV_PORT", port: 70000 }] },
  ])("refuses missing, closed or changed task context", stale => {
    expect(() => localTaskPreviewUrl(stale, expected)).toThrow();
  });
  it("does not resolve remote presentation ids onto localhost", () => {
    expect(() => localTaskPreviewUrl({ ...detail, id: "cloud:owner:task-a" }, { ...expected, taskId: "cloud:owner:task-a" })).toThrow();
  });
});
