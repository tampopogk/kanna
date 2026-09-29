// @vitest-environment jsdom
/**
 * The browser half of the schema gate (docs/specs/app-design.md §9.1): a
 * document the server's Rust adapter edited is read back by y-prosemirror, the
 * reader that deleted content in the prototype, and must come out exactly as
 * the server described it — nothing dropped, nothing normalised.
 */
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { prosemirrorToYXmlFragment } from "y-prosemirror";
import * as Y from "yjs";
import { commentAnchors, createHeadlessEditor, yDocToNode } from "./headless";
import { projectDocument } from "./projection";
import { DESIGN_SCHEMA_VERSION, DOCUMENT_FRAGMENT, describeSchema } from "./schema";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const fixture = (name: string) => new Uint8Array(readFileSync(join(root, "fixtures", name)));
const fixtureJson = (name: string) => JSON.parse(readFileSync(join(root, "fixtures", name), "utf8"));
const load = (...updates: Uint8Array[]) => {
  const doc = new Y.Doc();
  for (const update of updates) Y.applyUpdate(doc, update);
  return doc;
};

describe("design document fixtures", () => {
  const editor = createHeadlessEditor();

  it("describes the schema the server compiled in", () => {
    const compiled = JSON.parse(
      readFileSync(join(root, "..", "..", "crates", "kanna-server", "resources", "design-schema.json"), "utf8"),
    );
    expect(describeSchema()).toEqual(JSON.parse(JSON.stringify(compiled)));
    expect(compiled.version).toBe(DESIGN_SCHEMA_VERSION);
    expect(fixtureJson("schema.json")).toEqual(compiled);
  });

  it("projects the base document as committed", () => {
    expect(projectDocument(yDocToNode(editor, load(fixture("base.ydoc"))))).toEqual(fixtureJson("base.json"));
  });

  it("reads the server's edits exactly as the server described them", () => {
    const doc = load(fixture("rust/agent-edits.ydoc"));
    const node = yDocToNode(editor, doc);
    node.check();
    expect(projectDocument(node)).toEqual(fixtureJson("rust/agent-edits.json"));
    expect(commentAnchors(node)).toEqual({ "thread-1": "is queued" });
  });

  it("round-trips the server's edits through the editor without losing anything", () => {
    const node = yDocToNode(editor, load(fixture("rust/agent-edits.ydoc")));
    const rewritten = new Y.Doc();
    prosemirrorToYXmlFragment(node as never, rewritten.getXmlFragment(DOCUMENT_FRAGMENT));
    expect(projectDocument(yDocToNode(editor, rewritten))).toEqual(fixtureJson("rust/agent-edits.json"));
  });

  it("merges the server's edits with the person's concurrent typing", () => {
    const serverEdits = Y.encodeStateAsUpdate(load(fixture("rust/agent-edits.ydoc")), Y.encodeStateVector(load(fixture("base.ydoc"))));
    const merged = load(fixture("base.ydoc"), fixture("person-edit.update"), serverEdits);
    const blocks = projectDocument(yDocToNode(editor, merged));
    const intro = blocks.find((block) => block.id === "intro")!;
    expect(intro.text).toBe("Feedback is queued while the agent is busy. Typed by the person.");
    expect(blocks.find((block) => block.id === "title")!.text).toBe("App Design core notes");
  });

  it("a narrower reader would drop the unknown block — which is why the server refuses it", () => {
    const doc = load(fixture("unknown-block.ydoc"));
    const blocks = projectDocument(yDocToNode(editor, doc));
    expect(blocks.some((block) => block.id === "mystery")).toBe(false);
  });
});
