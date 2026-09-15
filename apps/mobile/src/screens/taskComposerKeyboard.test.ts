import { describe, expect, it } from "vitest";
import {
  getComposerBottomOffset,
  getTaskComposerBottomOffset,
  getTaskKeyboardOccludedHeight,
  taskKeyboardEventNames
} from "./taskComposerKeyboard";

describe("getComposerBottomOffset", () => {
  it("keeps the composer at its resting bottom when the keyboard is hidden", () => {
    expect(getComposerBottomOffset(0)).toBe(14);
  });

  it("docks the composer just above the keyboard when the keyboard is visible", () => {
    expect(getComposerBottomOffset(320)).toBe(328);
  });

  it("uses the keyboard events React Native exposes on Android", () => {
    expect(taskKeyboardEventNames("android")).toEqual({
      show: "keyboardDidShow",
      hide: "keyboardDidHide"
    });
    expect(taskKeyboardEventNames("ios")).toEqual({
      show: "keyboardWillShow",
      hide: "keyboardWillHide"
    });
  });
});

describe("getTaskKeyboardOccludedHeight", () => {
  it("uses the Android IME top to include system and accessory rows", () => {
    expect(
      getTaskKeyboardOccludedHeight(
        { height: 300, screenY: 360 },
        "android",
        800
      )
    ).toBe(440);
  });

  it("retains the reported height when it is the larger measurement", () => {
    expect(
      getTaskKeyboardOccludedHeight(
        { height: 460, screenY: 360 },
        "android",
        800
      )
    ).toBe(460);
  });

  it("preserves iOS keyboard-height ownership", () => {
    expect(
      getTaskKeyboardOccludedHeight(
        { height: 300, screenY: 360 },
        "ios",
        800
      )
    ).toBe(300);
  });
});

describe("getTaskComposerBottomOffset", () => {
  it("keeps Android resting chrome above the system navigation inset", () => {
    expect(getTaskComposerBottomOffset(0, "android", 48)).toBe(62);
    expect(getTaskComposerBottomOffset(0, "android", 0)).toBe(14);
  });

  it("docks above Android IME occlusion without adding the safe area", () => {
    expect(getTaskComposerBottomOffset(300, "android", 48)).toBe(308);
    expect(getTaskComposerBottomOffset(420, "android", 48)).toBe(428);
  });

  it("preserves iOS keyboard positioning and safe-area ownership", () => {
    expect(getTaskComposerBottomOffset(0, "ios", 34)).toBe(14);
    expect(getTaskComposerBottomOffset(300, "ios", 34)).toBe(308);
  });
});
