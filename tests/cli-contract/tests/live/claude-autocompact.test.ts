import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { randomUUID } from "node:crypto";
import { runClaudeRaw, findClaudeBinary } from "../../helpers/claude";

interface AutocompactContract {
  flag: string;
  default: string;
  settingsKey: string;
  measuredCliVersion: string;
  minTokens: number;
  maxTokens: number;
  usageErrorFragment: string;
  accepted: Array<{ value: string; tokens: number | null }>;
  rejected: string[];
}

const contract = JSON.parse(
  readFileSync(
    resolve(new URL("../../fixtures/claude-autocompact.json", import.meta.url).pathname),
    "utf8",
  ),
) as AutocompactContract;

/**
 * Ask the CLI what window it is actually running with.
 *
 * `/context` is a local slash command and prints `**Tokens:** <used> / <window>`,
 * which is the only place the CLI states the effective auto-compact window —
 * it is absent from `--debug`, from the stream-json init message and from the
 * session transcript. The model is pinned to a 1M-context one so a configured
 * window below the model's own is visible rather than clamped away.
 */
async function reportedWindow(
  flags: string[],
): Promise<{ window: string | null; stderr: string; exitCode: number }> {
  const { stdout, stderr, exitCode } = await runClaudeRaw(
    ["-p", "/context", "--max-turns", "1", "--model", "sonnet[1m]", ...flags],
    { timeoutMs: 90_000 },
  );
  const match = stdout.match(/^\*\*Tokens:\*\*\s+\S+\s+\/\s+(\S+)/m);
  return { window: match?.[1] ?? null, stderr, exitCode };
}

function normalize(window: string): number {
  const suffix = window.slice(-1).toLowerCase();
  const digits = suffix === "k" || suffix === "m" ? window.slice(0, -1) : window;
  const multiplier = suffix === "k" ? 1_000 : suffix === "m" ? 1_000_000 : 1;
  return Math.round(Number(digits) * multiplier);
}

// The Claude CLI's auto-compact window is a USER-GLOBAL setting
// (`autoCompactWindow` in ~/.claude/settings.json), so before Kanna passed
// this flag every agent session on a machine silently ran at whatever window
// the owner last set for their own terminal. These cases pin the override that
// replaced that inheritance, and the window Kanna validates a configured value
// against. Kanna never writes to the operator's settings file.
describe("claude auto-compact window", () => {
  it("--autocompact overrides an autoCompactWindow that settings set", async () => {
    // `--settings` is the highest-precedence settings source, so a flag that
    // beats it beats the user-global file too — and this proves the leak
    // itself is real without touching the operator's own settings.
    const shrunk = await reportedWindow([
      "--settings",
      JSON.stringify({ [contract.settingsKey]: 200_000 }),
    ]);
    if (shrunk.window === null) return; // CLI unavailable / not logged in
    expect(normalize(shrunk.window)).toBe(200_000);

    const restored = await reportedWindow([
      "--settings",
      JSON.stringify({ [contract.settingsKey]: 200_000 }),
      contract.flag,
      contract.default,
    ]);
    expect(restored.window).not.toBeNull();
    expect(
      normalize(restored.window as string),
      "--autocompact auto must restore the model's native window",
    ).toBe(1_000_000);

    const pinned = await reportedWindow([
      "--settings",
      JSON.stringify({ [contract.settingsKey]: 200_000 }),
      contract.flag,
      "400k",
    ]);
    expect(normalize(pinned.window as string)).toBe(400_000);
  }, 300_000);

  it("resolves every value in the fixture to the window it records", async () => {
    for (const { value, tokens } of contract.accepted) {
      const result = await reportedWindow([contract.flag, value]);
      if (result.window === null) return;
      // `auto` means the model's own window, which for sonnet[1m] is 1M.
      expect(normalize(result.window), `${contract.flag} ${value}`).toBe(
        tokens ?? 1_000_000,
      );
    }
  }, 600_000);

  it("is accepted alongside --session-id and honoured on --resume", async () => {
    // Kanna names a session on every claude PTY spawn and resumes one for a
    // revision, so a window flag that either form rejected would break the
    // spawn rather than the window.
    const sessionId = randomUUID();
    const assigned = await runClaudeRaw(
      [
        "-p", "Say OK", "--model", "haiku", "--max-turns", "1",
        "--session-id", sessionId,
        contract.flag, contract.default,
      ],
      { timeoutMs: 90_000 },
    );
    if (assigned.exitCode !== 0) return; // CLI unavailable / not logged in
    expect(assigned.stderr).not.toContain("is invalid");

    const resumed = await reportedWindow(["--resume", sessionId, contract.flag, "400k"]);
    expect(resumed.window).not.toBeNull();
    expect(normalize(resumed.window as string)).toBe(400_000);
  }, 200_000);

  it("rejects every out-of-range value in the fixture with a usage error", async () => {
    await findClaudeBinary();
    for (const value of contract.rejected) {
      const { exitCode, stderr } = await runClaudeRaw(
        ["-p", "Say OK", "--model", "haiku", "--max-turns", "1", contract.flag, value],
        { timeoutMs: 30_000 },
      );
      // Rejected at argument-parse time, so this costs no API call — and it is
      // why Kanna validates a configured window at request time instead of
      // letting the spawn die on a usage error.
      expect(exitCode, `${contract.flag} ${value}`).not.toBe(0);
      expect(stderr).toContain(contract.usageErrorFragment);
    }
  }, 300_000);
});
