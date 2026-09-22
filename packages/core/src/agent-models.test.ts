import { describe, expect, it } from "vitest";
import { VALID_AGENT_PROVIDERS } from "./config/agent-providers.js";
import { AGENT_MODELS, agentModelsFor } from "./agent-models.js";

describe("agentModelsFor", () => {
  it("offers the verified releases only under their native harness", () => {
    const claude = agentModelsFor("claude");
    const codex = agentModelsFor("codex");
    expect(claude).toContainEqual({ id: "claude-opus-5-5", label: "Opus 5.5" });
    for (const family of ["Astra", "Sol", "Luna"]) {
      expect(codex).toContainEqual({ id: `gpt-6-${family.toLowerCase()}`, label: `GPT-6 ${family}` });
    }
    expect(claude.every(({ id }) => id.startsWith("claude-"))).toBe(true);
    expect(codex.every(({ id }) => id.startsWith("gpt-"))).toBe(true);
    for (const models of [claude, codex]) {
      expect(new Set(models.map(({ id }) => id)).size).toBe(models.length);
    }
  });

  it("uses Claude models only when no provider is selected", () => {
    expect(agentModelsFor(undefined)).toEqual(AGENT_MODELS.claude);
  });

  it("returns no models for known providers without a verified catalog", () => {
    const providersWithoutCatalog = VALID_AGENT_PROVIDERS.filter(
      (provider) => !(provider in AGENT_MODELS),
    );

    for (const provider of providersWithoutCatalog) {
      expect(agentModelsFor(provider), provider).toEqual([]);
    }
  });
});
