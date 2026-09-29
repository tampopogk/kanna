import { describe, expect, it } from "vitest";
import { pinLabel, readMockupMessage } from "../mockupPins";

const pick = (overrides: Record<string, unknown> = {}) => ({
  kind: "kanna-mockup",
  type: "pick",
  pin: {
    page: "screens/list.html",
    selector: "body > main:nth-of-type(1) > ul:nth-of-type(1) > li:nth-of-type(2)",
    excerpt: "Review design",
    tag: "li",
    label: "li.row.selected",
    context: "main#list",
    html: "<li class=\"row selected\">Review design</li>",
    rect: { x: 10, y: 20, width: 300, height: 24 },
    ...overrides,
  },
});

describe("messages from a mockup page", () => {
  it("reads a pick as a bounded description of the element", () => {
    const message = readMockupMessage(pick({ excerpt: "x".repeat(2_000), extra: "dropped" }));
    expect(message?.type).toBe("pick");
    if (message?.type !== "pick") return;
    expect(message.pin.excerpt).toHaveLength(300);
    expect(message.pin).not.toHaveProperty("extra");
    expect(message.pin.rect).toEqual({ x: 10, y: 20, width: 300, height: 24 });
    expect(pinLabel({ ...message.pin, excerpt: "Review design" })).toBe("li.row.selected “Review design”");
  });

  it("refuses anything that is not one of its messages", () => {
    for (const data of [
      null,
      "pick",
      { kind: "other", type: "pick", pin: pick().pin },
      pick({ selector: "  " }),
      pick({ tag: "<script>" }),
      pick({ excerpt: 5 }),
      { kind: "kanna-mockup", type: "focus", id: "../th" },
      { kind: "kanna-mockup", type: "detached", ids: ["ok", 5] },
      { kind: "kanna-mockup", type: "approve" },
    ]) {
      expect(readMockupMessage(data), JSON.stringify(data)).toBeNull();
    }
  });

  it("reads ready, focus and detached", () => {
    expect(readMockupMessage({ kind: "kanna-mockup", type: "ready", page: "index.html" })).toEqual({
      type: "ready",
      page: "index.html",
    });
    expect(readMockupMessage({ kind: "kanna-mockup", type: "focus", id: "th-1" })).toEqual({ type: "focus", id: "th-1" });
    expect(readMockupMessage({ kind: "kanna-mockup", type: "detached", ids: ["th-1", "th-2"] })).toEqual({
      type: "detached",
      ids: ["th-1", "th-2"],
    });
  });
});
