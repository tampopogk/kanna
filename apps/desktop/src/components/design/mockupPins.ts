/**
 * Messages between the design pane and a mockup page's pin script
 * (crates/kanna-server/resources/design-mockup-pins.js). The page is the
 * agent's HTML, running sandboxed, and its own scripts share the pin
 * script's window: what arrives is only ever a description of an element to
 * show and pass on, so it is shape-checked and bounded here, and nothing in
 * it is rendered as markup.
 */

export const MOCKUP_MESSAGE_KIND = "kanna-mockup";

export interface MockupPinDescriptor {
  page: string;
  selector: string;
  tag: string;
  elementId: string;
  classes: string;
  container: string;
  text: string;
  html: string;
  /** Where the element is in the frame's viewport, for placing the composer. */
  rect: { x: number; y: number; width: number; height: number };
}

export type MockupMessage =
  | { type: "ready"; page: string }
  | { type: "pin"; pin: MockupPinDescriptor }
  | { type: "select"; number: number };

/** A pin the page should draw. */
export interface MockupPinMarker {
  number: number;
  page: string;
  selector: string;
}

const LIMITS = { page: 300, selector: 1_000, tag: 40, elementId: 200, classes: 300, container: 200, text: 500, html: 1_000 };

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

function text(value: unknown, max: number): string | null {
  if (typeof value !== "string") return null;
  return value.length > max ? value.slice(0, max) : value;
}

function finite(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

export function readMockupMessage(data: unknown): MockupMessage | null {
  if (!isObject(data) || data.kind !== MOCKUP_MESSAGE_KIND) return null;
  switch (data.type) {
    case "ready": {
      const page = text(data.page, LIMITS.page);
      return page === null ? null : { type: "ready", page };
    }
    case "select":
      return typeof data.number === "number" && Number.isInteger(data.number) && data.number > 0
        ? { type: "select", number: data.number }
        : null;
    case "pin": {
      const pin = data.pin;
      if (!isObject(pin)) return null;
      const fields = {} as Record<keyof typeof LIMITS, string>;
      for (const [name, max] of Object.entries(LIMITS) as Array<[keyof typeof LIMITS, number]>) {
        const value = text(pin[name] ?? "", max);
        if (value === null) return null;
        fields[name] = value;
      }
      if (!fields.selector.trim() || !/^[a-z][a-z0-9-]*$/i.test(fields.tag)) return null;
      const rect = isObject(pin.rect) ? pin.rect : {};
      return {
        type: "pin",
        pin: {
          ...fields,
          rect: { x: finite(rect.x), y: finite(rect.y), width: finite(rect.width), height: finite(rect.height) },
        },
      };
    }
    default:
      return null;
  }
}

/** One line naming the element: `<button id="save" class="primary">`. */
export function elementLabel(element: { tag: string; elementId: string; classes: string }): string {
  let label = `<${element.tag}`;
  if (element.elementId) label += ` id="${element.elementId}"`;
  if (element.classes) label += ` class="${element.classes}"`;
  return `${label}>`;
}
