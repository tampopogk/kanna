import { describe, expect, it } from "vitest";
import { chordToKeyArgs } from "./realKeys";
import { matchesChord, physicalCode, type ObservedKey } from "./realKeyProbe";

/**
 * The harness, not the app. A chord translated to the wrong keycode presses
 * the wrong key and the lane reports a keymap verdict about a keystroke nobody
 * made — the exact class of silent wrongness the lane exists to remove.
 */
function observe(overrides: Partial<ObservedKey>): ObservedKey {
  return {
    key: "",
    code: "",
    ctrl: false,
    shift: false,
    alt: false,
    meta: false,
    defaultPrevented: false,
    target: "body",
    ...overrides,
  };
}

describe("chordToKeyArgs", () => {
  it("presses and releases a bare key", () => {
    expect(chordToKeyArgs("PageUp")).toEqual(["104:1", "104:0"]);
  });

  it("holds the modifiers down around the key", () => {
    expect(chordToKeyArgs("Ctrl+PageDown")).toEqual(["29:1", "109:1", "109:0", "29:0"]);
  });

  /**
   * Reverse release order is what a hand does, and what a compositor's own
   * grab matching expects: releasing Ctrl before the letter turns one chord
   * into two different ones on the way up.
   */
  it("releases the modifiers in reverse", () => {
    expect(chordToKeyArgs("Ctrl+Shift+ArrowLeft")).toEqual([
      "29:1", "42:1", "105:1", "105:0", "42:0", "29:0",
    ]);
  });

  it("takes a letter in either case", () => {
    expect(chordToKeyArgs("Ctrl+Shift+U")).toEqual(chordToKeyArgs("Ctrl+Shift+u"));
  });

  it("refuses a key or a modifier it has no keycode for", () => {
    expect(() => chordToKeyArgs("Ctrl+F13")).toThrow(/no Linux keycode for "F13"/);
    expect(() => chordToKeyArgs("Hyper+a")).toThrow(/no Linux keycode for modifier "Hyper"/);
    expect(() => chordToKeyArgs("")).toThrow(/empty chord/);
  });
});

describe("matchesChord", () => {
  /**
   * Matched on `code`, never on `key`: Shift rewrites `key` ("u" becomes "U",
   * "-" becomes "_"), so a `key` comparison would miss the very chords this
   * lane was written to check.
   */
  it("matches the physical key regardless of what Shift made of it", () => {
    expect(matchesChord(observe({ code: "KeyU", key: "U", ctrl: true, shift: true }), "Ctrl+Shift+u")).toBe(true);
  });

  it("rejects a different modifier set on the same key", () => {
    const event = observe({ code: "KeyU", key: "u", ctrl: true, alt: true });
    expect(matchesChord(event, "Ctrl+Alt+u")).toBe(true);
    expect(matchesChord(event, "Ctrl+Shift+u")).toBe(false);
    expect(matchesChord(event, "Ctrl+u")).toBe(false);
  });

  it("matches named keys by their own code", () => {
    expect(matchesChord(observe({ code: "PageUp", key: "PageUp", ctrl: true }), "Ctrl+PageUp")).toBe(true);
    expect(matchesChord(observe({ code: "PageDown", key: "PageDown", ctrl: true }), "Ctrl+PageUp")).toBe(false);
  });
});

describe("physicalCode", () => {
  it("names letters and digits the way KeyboardEvent.code does", () => {
    expect(physicalCode("u")).toBe("KeyU");
    expect(physicalCode("U")).toBe("KeyU");
    expect(physicalCode("4")).toBe("Digit4");
    expect(physicalCode("ArrowLeft")).toBe("ArrowLeft");
  });
});
