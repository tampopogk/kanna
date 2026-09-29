import { BlockNoteEditor } from "@blocknote/core";
import { _blocksToProsemirrorNode } from "@blocknote/core/yjs";
import {
  CommentsExtension,
  DefaultThreadStoreAuth,
  ThreadStore,
} from "@blocknote/core/comments";
import type { Node as PMNode } from "@tiptap/pm/model";
import { prosemirrorToYXmlFragment, yXmlFragmentToProseMirrorRootNode } from "y-prosemirror";
import * as Y from "yjs";
import { DOCUMENT_FRAGMENT, designSchema } from "./schema";

/**
 * A thread store that holds nothing. The headless editor only needs the
 * comment mark in its schema; threads themselves live on kanna-server.
 */
class EmptyThreadStore extends ThreadStore {
  constructor() {
    super(new DefaultThreadStoreAuth("headless", "comment"));
  }
  addThreadToDocument = undefined;
  async createThread(): Promise<never> { throw new Error("read-only"); }
  async addComment(): Promise<never> { throw new Error("read-only"); }
  async updateComment(): Promise<void> {}
  async deleteComment(): Promise<void> {}
  async deleteThread(): Promise<void> {}
  async resolveThread(): Promise<void> {}
  async unresolveThread(): Promise<void> {}
  async addReaction(): Promise<void> {}
  async deleteReaction(): Promise<void> {}
  getThread(): never { throw new Error("no threads"); }
  getThreads() { return new Map(); }
  subscribe() { return () => {}; }
}

/**
 * An editor with exactly the document schema, never mounted: used to convert
 * between Yjs, ProseMirror and blocks in fixtures and tests. Needs a DOM
 * (jsdom or happy-dom) the way BlockNote does.
 */
export function createHeadlessEditor() {
  return BlockNoteEditor.create({
    schema: designSchema,
    extensions: [
      CommentsExtension({
        threadStore: new EmptyThreadStore() as unknown as ThreadStore,
        resolveUsers: async () => [],
      }),
    ],
  });
}

export type HeadlessEditor = ReturnType<typeof createHeadlessEditor>;

/** Blocks → a ProseMirror document in the editor's schema. */
export function blocksToNode(editor: HeadlessEditor, blocks: unknown[]): PMNode {
  return _blocksToProsemirrorNode(editor as never, blocks as never) as unknown as PMNode;
}

/** Write a ProseMirror document into a fresh Yjs document's fragment. */
export function nodeToYDoc(node: PMNode): Y.Doc {
  const doc = new Y.Doc();
  prosemirrorToYXmlFragment(node as never, doc.getXmlFragment(DOCUMENT_FRAGMENT));
  return doc;
}

/**
 * Read a Yjs document the way the browser's editor does. y-prosemirror
 * silently drops content it cannot place in the schema, which is exactly the
 * failure docs/specs/app-design.md §9.1 records; callers compare the result
 * with what they expected rather than trusting it.
 */
export function yDocToNode(editor: HeadlessEditor, doc: Y.Doc): PMNode {
  return yXmlFragmentToProseMirrorRootNode(
    doc.getXmlFragment(DOCUMENT_FRAGMENT),
    editor.pmSchema as never,
  ) as unknown as PMNode;
}

/** Comment anchors in a ProseMirror document: thread id → anchored text. */
export function commentAnchors(node: PMNode): Record<string, string> {
  const anchors: Record<string, string> = {};
  node.descendants((child) => {
    if (!child.isText) return;
    for (const mark of child.marks) {
      const threadId = mark.type.name === "comment" ? (mark.attrs.threadId as string) : "";
      if (threadId) anchors[threadId] = (anchors[threadId] ?? "") + (child.text ?? "");
    }
  });
  return anchors;
}
