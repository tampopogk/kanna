import { ref } from "vue";

/**
 * The design surface's one-line confirmations ("Now in “Interactive
 * mockup”", "Sent to the agent's session"), shown briefly at the bottom as
 * in the design prototype. Shared so the header's position ladder and the
 * design view say things in the same place.
 */
export const designToast = ref<{ text: string; kind: "info" | "error"; at: number } | null>(null);
let timer: ReturnType<typeof setTimeout> | null = null;

export function sayDesign(text: string, kind: "info" | "error" = "info"): void {
  designToast.value = { text, kind, at: Date.now() };
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => (designToast.value = null), kind === "error" ? 4000 : 2200);
}
