import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

interface CapacityRefusalCapture {
  provider: string;
  cliVersion: string;
  capturedAt: string;
  capturedFrom: string;
  ruleId: string | null;
  kind?: string;
  scope: string | null;
  scopeClaim?: string;
  frame?: string[];
  wrappedFrame?: string[] | null;
  wrapNote?: string;
  parksAs?: string;
  parksAsEvidence?: string;
  mustNotMatch?: string[];
}

const fixturePath = resolve(
  new URL("../../fixtures/provider-capacity-refusal.json", import.meta.url).pathname,
);
const captures = JSON.parse(readFileSync(fixturePath, "utf8")) as CapacityRefusalCapture[];
const measured = captures.filter((capture) => capture.frame);

// The daemon's detection tests read this same file, so a pattern change and the
// chrome it was measured against cannot drift apart. What this suite owns is
// the *provenance*, and one claim beyond it: a capacity refusal is not a spent
// allowance, and a fixture that blurred the two would send a transient
// condition into quota recovery, where it would burn a stage's candidates.
describe("provider capacity-refusal chrome contract", () => {
  it("carries at least one measured refusal, and only kinds that are not quota", () => {
    expect(measured.length).toBeGreaterThan(0);
    for (const capture of measured) {
      expect(capture.kind).toBe("capacity-refusal");
      expect(capture.ruleId).toMatch(/\/notice\/capacity-refusal$/);
    }
  });

  it("tags every capture with the CLI version and origin it was measured from", () => {
    for (const capture of captures) {
      expect(capture.cliVersion).toMatch(/^\d+\.\d+\.\d+$/);
      expect(capture.capturedAt).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      expect(capture.capturedFrom.trim()).not.toBe("");
    }
  });

  it("states how wide the provider's own claim is", () => {
    // Codex refuses "the selected model" and names no identifier for it, so
    // the stated scope is null. Recording anything else there would widen a
    // claim about one model into one about an account or a provider.
    for (const capture of measured) {
      expect(capture.scopeClaim?.trim()).toBeTruthy();
      if (capture.scope === null) {
        expect(capture.scopeClaim).toMatch(/names? no|without naming|did not say/i);
      }
    }
  });

  it("declares an unmeasured narrow-terminal wrap rather than inventing one", () => {
    // A provider re-wraps its own sentence in its own way, so a wrapped frame
    // is chrome and has to be measured like any other. Where none has been
    // observed the fixture says so and why, instead of carrying a plausible
    // guess that a rule could then be tuned against.
    for (const capture of measured) {
      if (capture.wrappedFrame && capture.wrappedFrame.length > 0) {
        expect(capture.wrappedFrame).not.toEqual(capture.frame);
      } else {
        expect(capture.wrapNote?.trim()).toBeTruthy();
      }
    }
  });

  it("records that a refused session parks rather than dying, and how that is known", () => {
    // The whole reason this needs its own signal: the session is still a live,
    // healthy agent afterwards — during the incident the owner typed a retry
    // into that same composer and the turn continued — so no runtime status
    // can carry the fact.
    for (const capture of measured) {
      expect(capture.parksAs).toBe("idle");
      expect(capture.parksAsEvidence?.trim()).toBeTruthy();
    }
  });

  it("keeps the spent-allowance refusal among the negatives", () => {
    const negatives = captures.flatMap((capture) => capture.mustNotMatch ?? []);
    expect(negatives.length).toBeGreaterThan(0);
    // The one that matters most: Codex's own quota refusal. It is about the
    // same provider, it is drawn by the same CLI, and it means something else
    // entirely — an allowance that has to reset, with a candidate to fall back
    // to. A capacity rule that claimed it would silently reroute a task.
    expect(negatives.some((line) => line.includes("You've hit your usage limit"))).toBe(true);
    // And a capacity sentence from another provider, which no Codex rule owns.
    expect(negatives.some((line) => line.includes("The API is at capacity"))).toBe(true);
  });
});
