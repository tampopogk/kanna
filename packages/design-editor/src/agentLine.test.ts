import { describe, expect, it } from "vitest";
import { parseAgentLine } from "./agentLine";

describe("the /agent line", () => {
  it("sends what follows /agent", () => {
    expect(parseAgentLine("/agent tighten the intro")).toEqual({ kind: "send", message: "tighten the intro" });
    expect(parseAgentLine("  /Agent   two  words ")).toEqual({ kind: "send", message: "two  words" });
  });

  it("stays on the line when nothing follows", () => {
    expect(parseAgentLine("/agent")).toEqual({ kind: "empty" });
    expect(parseAgentLine("/agent   ")).toEqual({ kind: "empty" });
  });

  it("ignores ordinary text and other commands", () => {
    expect(parseAgentLine("the /agent word mid-line")).toEqual({ kind: "none" });
    expect(parseAgentLine("/agents plural")).toEqual({ kind: "none" });
    expect(parseAgentLine("/heading")).toEqual({ kind: "none" });
  });
});
