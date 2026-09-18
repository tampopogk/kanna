import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

interface AcceptedWindow {
  value: string;
  tokens: number | null;
}

interface AutocompactContract {
  flag: string;
  default: string;
  settingsKey: string;
  measuredCliVersion: string;
  minTokens: number;
  maxTokens: number;
  usageErrorFragment: string;
  accepted: AcceptedWindow[];
  rejected: string[];
}

const fixturePath = resolve(
  new URL("../../fixtures/claude-autocompact.json", import.meta.url).pathname,
);
const contract = JSON.parse(
  readFileSync(fixturePath, "utf8"),
) as AutocompactContract;

/**
 * Kanna's own parser, mirroring `resolve_autocompact_window` in
 * crates/kanna-agent-protocol/src/providers.rs. The Rust side is what
 * validates a configured window; this reimplementation exists so the measured
 * CLI behaviour and Kanna's reading of it are checked against each other
 * rather than each drifting on its own.
 */
function resolveWindow(value: string): number | null | "invalid" {
  const trimmed = value.trim();
  if (trimmed.toLowerCase() === contract.default) return null;
  const suffix = trimmed.slice(-1);
  const multiplier =
    suffix === "k" || suffix === "K"
      ? 1_000
      : suffix === "m" || suffix === "M"
        ? 1_000_000
        : undefined;
  const digits = multiplier === undefined ? trimmed : trimmed.slice(0, -1);
  if (digits.length === 0 || !/^[0-9.]+$/.test(digits)) return "invalid";
  const parsed = Number(digits);
  if (!Number.isFinite(parsed) || parsed < 0) return "invalid";
  const tokens = Math.round(
    multiplier !== undefined
      ? parsed * multiplier
      : parsed <= 1_000
        ? parsed * 1_000
        : parsed,
  );
  if (tokens < contract.minTokens || tokens > contract.maxTokens) return "invalid";
  return tokens;
}

// The window is a USER-GLOBAL Claude setting, so a spawn that names no window
// runs at whatever the machine's owner last chose. `--autocompact` is the only
// per-invocation override, and Kanna passes it on every claude spawn.
// tests/cli-contract/tests/live/claude-autocompact.test.ts measures the same
// values against the installed CLI.
describe("claude auto-compact window contract", () => {
  it("pins the flag, the default and the window Kanna validates against", () => {
    expect(contract.flag).toBe("--autocompact");
    expect(contract.default).toBe("auto");
    expect(contract.settingsKey).toBe("autoCompactWindow");
    expect(contract.minTokens).toBe(100_000);
    expect(contract.maxTokens).toBe(1_000_000);
  });

  it("resolves every measured accepted value to the window the CLI reported", () => {
    for (const { value, tokens } of contract.accepted) {
      expect(resolveWindow(value), `accepted: ${value}`).toBe(tokens);
    }
  });

  it("rejects every measured usage error", () => {
    for (const value of contract.rejected) {
      expect(resolveWindow(value), `rejected: ${value}`).toBe("invalid");
    }
  });

  it("reads a bare number as thousands only up to 1000", () => {
    // The trap this pins: `200` is 200k, but `5000` is 5000 tokens and out of
    // range, so the two spellings of "five thousand-ish" mean opposite things.
    expect(resolveWindow("200")).toBe(200_000);
    expect(resolveWindow("200000")).toBe(200_000);
    expect(resolveWindow("5000")).toBe("invalid");
  });
});
