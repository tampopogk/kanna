/**
 * Messages between the design pane and a mockup page's pin script
 * (crates/kanna-server/resources/design-mockup-pins.js), after the design
 * prototype's review room. The page is the agent's HTML, running sandboxed,
 * and its own scripts share the pin script's window: what arrives is only
 * ever a description of an element to show and pass on, so it is
 * shape-checked and bounded here, and nothing in it is rendered as markup.
 */

export const MOCKUP_MESSAGE_KIND = "kanna-mockup";

/** An element the person clicked, as the prototype described it. */
export interface MockupPinDescriptor {
  page: string;
  selector: string;
  /** Its visible text, parts joined with " · ". */
  excerpt: string;
  tag: string;
  /** `tag#id.class.class` */
  label: string;
  /** The nearest containing landmark, as a label. */
  context: string;
  html: string;
  /** Where the element is in the mockup's viewport. */
  rect: { x: number; y: number; width: number; height: number };
}

export type MockupMessage =
  | { type: "ready"; page: string }
  | { type: "pick"; pin: MockupPinDescriptor }
  | { type: "focus"; id: string }
  | { type: "detached"; ids: string[] };

/** A pin the page should draw. */
export interface MockupPinMarker {
  id: string;
  n: number;
  page: string;
  selector: string;
  excerpt: string;
}

const LIMITS = { page: 300, selector: 1_000, excerpt: 300, tag: 40, label: 200, context: 200, html: 400 };
const THREAD_ID = /^[A-Za-z0-9_-]{1,80}$/;

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
    case "focus":
      return typeof data.id === "string" && THREAD_ID.test(data.id) ? { type: "focus", id: data.id } : null;
    case "detached":
      return Array.isArray(data.ids) && data.ids.length <= 500 && data.ids.every((id) => typeof id === "string" && THREAD_ID.test(id))
        ? { type: "detached", ids: data.ids as string[] }
        : null;
    case "pick": {
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
        type: "pick",
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

/** How a pinned element reads, as in the prototype: `button#save “Save”`. */
export function pinLabel(element: { tag: string; label: string; excerpt: string }, max = 60): string {
  const name = element.label || `<${element.tag}>`;
  const excerpt = element.excerpt.length > max ? `${element.excerpt.slice(0, max)}…` : element.excerpt;
  return excerpt ? `${name} “${excerpt}”` : name;
}
