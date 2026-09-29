/**
 * Regenerate the cross-language fixtures the Rust adapter is tested against
 * (crates/kanna-server/src/design/document.rs). Run after changing the design
 * schema or upgrading BlockNote:
 *
 *   pnpm --filter @kanna/design-editor fixtures
 *
 * Every Yjs document is written with fixed client ids, so the output is
 * byte-for-byte reproducible and a diff shows a real schema change.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as Y from "yjs";
import { installDom } from "../src/dom";

await installDom();
const { EditorState } = await import("@tiptap/pm/state");
const { prosemirrorToYXmlFragment } = await import("y-prosemirror");
const { createHeadlessEditor, blocksToNode, yDocToNode } = await import("../src/headless");
const { describeSchema, DOCUMENT_FRAGMENT } = await import("../src/schema");
const { projectDocument } = await import("../src/projection");

const out = join(dirname(fileURLToPath(import.meta.url)), "..", "fixtures");
mkdirSync(out, { recursive: true });
const write = (name: string, data: string | Uint8Array) => {
  writeFileSync(join(out, name), data);
  console.log(`wrote fixtures/${name}`);
};
const json = (value: unknown) => `${JSON.stringify(value, null, 2)}\n`;

const editor = createHeadlessEditor();

// Every block type and style the schema allows, a nested child, a link and a
// comment anchor across a style boundary: the shapes §9.1's schema mismatch
// destroyed.
const baseBlocks = [
  { id: "title", type: "heading", props: { level: 1 }, content: "App Design notes" },
  {
    id: "intro",
    type: "paragraph",
    content: [
      { type: "text", text: "Feedback is ", styles: {} },
      { type: "text", text: "queued", styles: { bold: true } },
      { type: "text", text: " until the agent is free.", styles: {} },
    ],
    children: [
      {
        id: "nested",
        type: "bulletListItem",
        content: [
          { type: "text", text: "nested ", styles: {} },
          { type: "text", text: "italic", styles: { italic: true, textColor: "red" } },
        ],
      },
    ],
  },
  {
    id: "links",
    type: "paragraph",
    content: [
      { type: "text", text: "See ", styles: {} },
      { type: "link", href: "https://kanna.build/spec", content: [{ type: "text", text: "the spec", styles: { underline: true } }] },
      { type: "text", text: " and ", styles: {} },
      { type: "text", text: "code", styles: { code: true } },
      { type: "text", text: " and ", styles: {} },
      { type: "text", text: "struck", styles: { strike: true, backgroundColor: "yellow" } },
    ],
  },
  { id: "numbered", type: "numberedListItem", content: "first step" },
  { id: "check", type: "checkListItem", props: { checked: true }, content: "done item" },
  { id: "toggle", type: "toggleListItem", content: "toggle me" },
  { id: "quote", type: "quote", content: "a quotation" },
  { id: "code", type: "codeBlock", props: { language: "rust" }, content: "fn main() {}" },
  { id: "divider", type: "divider" },
  { id: "agent-line", type: "paragraph", content: "/agent please tighten the intro" },
  { id: "empty", type: "paragraph" },
];

const baseNode = blocksToNode(editor, baseBlocks);
// Anchor a comment on "is queued" — across the plain/bold boundary — the way
// the comments extension does: one `comment` mark carrying the thread id.
let anchorFrom = -1;
baseNode.descendants((node, pos) => {
  if (anchorFrom < 0 && node.isText && node.text === "Feedback is ") anchorFrom = pos + "Feedback ".length;
});
const commentMark = editor.pmSchema.marks.comment.create({ threadId: "thread-1", orphan: false });
const anchored = EditorState.create({ doc: baseNode })
  .tr.addMark(anchorFrom, anchorFrom + "is queued".length, commentMark).doc;

const base = new Y.Doc();
base.clientID = 1;
prosemirrorToYXmlFragment(anchored as never, base.getXmlFragment(DOCUMENT_FRAGMENT));
const baseState = Y.encodeStateAsUpdate(base);
write("schema.json", json(describeSchema()));
// The server's compiled-in copy; `fixtures.test.ts` keeps the two equal.
writeFileSync(join(out, "..", "..", "..", "crates", "kanna-server", "resources", "design-schema.json"), json(describeSchema()));
console.log("wrote crates/kanna-server/resources/design-schema.json");
write("base.ydoc", baseState);
write("base.json", json(projectDocument(yDocToNode(editor, base))));

// The person types in "intro" while the agent works elsewhere: a Yjs update
// relative to the base, from another client.
const person = new Y.Doc();
person.clientID = 2;
Y.applyUpdate(person, baseState);
const introText = (() => {
  const group = person.getXmlFragment(DOCUMENT_FRAGMENT).get(0) as Y.XmlElement;
  const container = group.get(1) as Y.XmlElement;
  const paragraph = container.get(0) as Y.XmlElement;
  return paragraph.get(0) as Y.XmlText;
})();
introText.insert(introText.length, " Typed by the person.");
write("person-edit.update", Y.encodeStateAsUpdate(person, Y.encodeStateVector(base)));
write("person-edit.json", json(projectDocument(yDocToNode(editor, person))));

// A document from a newer schema: a block type this schema does not know.
const future = new Y.Doc();
future.clientID = 3;
Y.applyUpdate(future, baseState);
{
  const group = future.getXmlFragment(DOCUMENT_FRAGMENT).get(0) as Y.XmlElement;
  const container = new Y.XmlElement("blockContainer");
  container.setAttribute("id", "mystery");
  const mystery = new Y.XmlElement("mysteryBlock");
  mystery.insert(0, [new Y.XmlText("content a narrower client would delete")]);
  container.insert(0, [mystery]);
  group.insert(group.length, [container]);
}
write("unknown-block.ydoc", Y.encodeStateAsUpdate(future));
