import { computed, ref } from "vue";
import { describe, expect, it } from "vitest";
import { useMainTabs, parsePersistedMainTabs } from "./useMainTabs";
import { getTerminalRecoveryMode, shouldReattachOnDaemonReady, shouldRespawnAfterAttachFailure } from "./terminalSessionRecovery";

describe("editor workspace continuity", () => {
  it("preserves the exact session across task switches, hiding and persisted restoration", () => {
    const scope = ref("item:a");
    const tabs = useMainTabs({ scopeKey: computed(() => scope.value) });
    const session = { sessionId: "shell-editor-1-a-123", worktreePath: "/actual/old-workspace", filePath: "a.txt", command: "vim" };
    const id = tabs.openTab({ kind: "editor", editorSession: session });
    if (!id) throw new Error("Editor tab did not open");
    expect(tabs.activeTabContext.value).toBe("shell");
    scope.value = "item:b";
    expect(tabs.tabs.value.some(t => t.kind === "editor")).toBe(false);
    scope.value = "item:a";
    expect(tabs.activeTab.value?.editorSession).toEqual(session);
    tabs.activateTab("agent");
    const persisted = parsePersistedMainTabs(JSON.stringify(tabs.snapshotScopes()));
    const restored = useMainTabs({ scopeKey: computed(() => scope.value) });
    restored.restoreScopes(persisted);
    restored.activateTab(id);
    expect(restored.activeTab.value?.editorSession).toEqual(session);
    restored.closeTab(id);
    expect(restored.activeTab.value?.kind).toBe("agent");
  });
  it("reattaches after daemon handoff but never respawns a missing editor", () => {
    expect(getTerminalRecoveryMode(undefined, { attachOnly: true })).toBe("attach-only");
    expect(shouldReattachOnDaemonReady(undefined, { attachOnly: true })).toBe(true);
    expect(shouldRespawnAfterAttachFailure({ code: "session_not_found", message: "missing" }, true, false, undefined, { attachOnly: true })).toBe(false);
  });
});
