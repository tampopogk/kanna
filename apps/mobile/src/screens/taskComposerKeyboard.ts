const RESTING_COMPOSER_BOTTOM = 14;
const KEYBOARD_COMPOSER_GAP = 8;

export function taskKeyboardEventNames(platform: string): {
  show: "keyboardWillShow" | "keyboardDidShow";
  hide: "keyboardWillHide" | "keyboardDidHide";
} {
  return platform === "android"
    ? { show: "keyboardDidShow", hide: "keyboardDidHide" }
    : { show: "keyboardWillShow", hide: "keyboardWillHide" };
}

export function getComposerBottomOffset(keyboardHeight: number): number {
  if (keyboardHeight <= 0) {
    return RESTING_COMPOSER_BOTTOM;
  }

  return keyboardHeight + KEYBOARD_COMPOSER_GAP;
}

export interface TaskKeyboardEndCoordinates {
  height: number;
  screenY: number;
}

/**
 * Android's IME height omits system bars and can omit Samsung accessory
 * panels, while screenY is the actual top edge occluding the edge-to-edge
 * React root. Prefer that full occlusion, retaining the reported height as a
 * fallback for keyboards/platforms whose coordinate space differs.
 */
export function getTaskKeyboardOccludedHeight(
  endCoordinates: TaskKeyboardEndCoordinates,
  platform: string,
  viewportHeight: number
): number {
  const reportedHeight = Math.max(0, endCoordinates.height);
  if (platform !== "android" || !Number.isFinite(endCoordinates.screenY)) {
    return reportedHeight;
  }

  return Math.max(
    reportedHeight,
    Math.max(0, viewportHeight - endCoordinates.screenY)
  );
}

/**
 * Positions the task composer above the measured IME occlusion. The closed
 * state alone consumes the Android navigation-bar inset.
 */
export function getTaskComposerBottomOffset(
  keyboardHeight: number,
  platform: string,
  bottomInset: number
): number {
  if (keyboardHeight > 0) {
    return getComposerBottomOffset(keyboardHeight);
  }

  return getComposerBottomOffset(0) +
    (platform === "android" ? Math.max(0, bottomInset) : 0);
}
