import { describe, expect, it } from "vitest";
import { elementLabel, readMockupMessage } from "../mockupPins";

const pin = (overrides: Record<string, unknown> = {}) => ({
  kind: "kanna-mockup",
  type: "pin",
  pin: {
    page: "screens/list.html",
    selector: "main > ul > li:nth-of-type(2)",
    tag: "li",
    elementId: "",
    classes: "row selected",
    container: "main Tasks",
    text: "Review design",
    html: "<li class=\"row selected\">Review design</li>",
    rect: { x: 10, y: 20, width: 300, height: 24 },
    ...overrides,
  },
});

describe("messages from a mockup page", () => {
  it("reads a pin as a bounded description of the element", () => {
    const message = readMockupMessage(pin({ text: "x".repeat(2_000), extra: "dropped" }));
    expect(message?.type).toBe("pin");
    if (message?.type !== "pin") return;
    expect(message.pin.text).toHaveLength(500);
    expect(message.pin).not.toHaveProperty("extra");
    expect(message.pin.rect).toEqual({ x: 10, y: 20, width: 300, height: 24 });
    expect(elementLabel(message.pin)).toBe('<li class="row selected">');
  });

  it("refuses anything that is not one of its messages", () => {
    for (const data of [
      null,
      "pin",
      { kind: "other", type: "pin", pin: pin().pin },
      pin({ selector: "  " }),
      pin({ tag: "<script>" }),
      pin({ text: 5 }),
      { kind: "kanna-mockup", type: "select", number: 0 },
      { kind: "kanna-mockup", type: "select", number: 1.5 },
      { kind: "kanna-mockup", type: "approve" },
    ]) {
      expect(readMockupMessage(data), JSON.stringify(data)).toBeNull();
    }
  });

  it("reads ready and select", () => {
    expect(readMockupMessage({ kind: "kanna-mockup", type: "ready", page: "index.html" })).toEqual({
      type: "ready",
      page: "index.html",
    });
    expect(readMockupMessage({ kind: "kanna-mockup", type: "select", number: 3 })).toEqual({ type: "select", number: 3 });
  });
});
