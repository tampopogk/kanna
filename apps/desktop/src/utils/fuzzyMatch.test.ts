import { describe, expect, it } from "vitest";
import { fuzzyMatch } from "./fuzzyMatch";

function rank(query: string, paths: string[]): string[] {
  return paths
    .map((path) => ({ path, result: fuzzyMatch(query, path) }))
    .filter((entry) => entry.result !== null)
    .sort((a, b) => (b.result?.score ?? 0) - (a.result?.score ?? 0))
    .map((entry) => entry.path);
}

function score(query: string, path: string): number {
  const result = fuzzyMatch(query, path);
  expect(result).not.toBeNull();
  return result?.score ?? 0;
}

describe("fuzzyMatch", () => {
  it("returns null for an empty or whitespace-only query", () => {
    expect(fuzzyMatch("", "AGENTS.md")).toBeNull();
    expect(fuzzyMatch("   ", "AGENTS.md")).toBeNull();
  });

  it("returns null when the query does not match", () => {
    expect(fuzzyMatch("zzz", "AGENTS.md")).toBeNull();
  });

  it("reports the matched indices in the full path", () => {
    expect(fuzzyMatch("main.ts", "apps/desktop/src/main.ts")?.indices).toEqual([
      17, 18, 19, 20, 21, 22, 23,
    ]);
  });

  it("ranks a typed filename above the partial matches it appears inside", () => {
    // The per-character bonuses alone let a longer filename with more word
    // boundaries out-earn a short exact match, which buried AGENTS.md.
    expect(
      rank("agents.md", [
        "docs/specs/agent-status-detection-rules.md",
        "docs/superpowers/plans/2026-03-28-agent-cli-setup.md",
        "AGENTS.md",
      ])[0],
    ).toBe("AGENTS.md");
  });

  it("orders exact above prefix above substring above scattered filename matches", () => {
    expect(
      rank("agent", [
        "src/agentStageHistory.ts",
        "src/useAgentStream.ts",
        "src/agent.ts",
        "src/assignment.ts",
      ]),
    ).toEqual([
      "src/agent.ts",
      "src/agentStageHistory.ts",
      "src/useAgentStream.ts",
      "src/assignment.ts",
    ]);
  });

  it("prefers a filename match over a directory-only match", () => {
    expect(rank("config", ["config/settings.ts", "src/config.ts"])[0]).toBe("src/config.ts");
  });

  it("prefers the shorter filename when the match quality is the same", () => {
    expect(rank("agent", ["src/agentStageHistory.ts", "src/agent.ts"])).toEqual([
      "src/agent.ts",
      "src/agentStageHistory.ts",
    ]);
  });

  it("never lets the length tiebreak outweigh a real scoring difference", () => {
    // The long name matches at a word boundary; the short one does not.
    expect(rank("stage", ["src/agent-stage.ts", "src/stgxe.ts"])[0]).toBe("src/agent-stage.ts");
  });

  it("penalizes characters skipped inside the matched span", () => {
    expect(score("abc", "abc.ts")).toBeGreaterThan(score("abc", "axbxc.ts"));
  });

  it("scores identically regardless of the directory a filename sits in", () => {
    // baseBranchPicker relies on this tie to keep its canonical ordering.
    expect(score("main", "main")).toBe(score("main", "origin/main"));
  });

  it("requires every part of a multi-part query to match", () => {
    expect(fuzzyMatch("comp btn", "src/components/button.vue")).not.toBeNull();
    expect(fuzzyMatch("comp btn", "src/components/input.vue")).toBeNull();
  });

  it("merges and sorts indices across multi-part queries", () => {
    expect(fuzzyMatch("bu vue", "button.vue")?.indices).toEqual([0, 1, 7, 8, 9]);
  });
});
