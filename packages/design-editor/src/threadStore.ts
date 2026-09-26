import type { BlockNoteEditor } from "@blocknote/core";
import {
  ThreadStore,
  ThreadStoreAuth,
  type CommentData,
  type ThreadData,
} from "@blocknote/core/comments";
import { newId, type DesignSession } from "./session";
import type { DesignComment, DesignThread } from "./types";

/** BlockNote's comment user ids for the two authors a design has. */
export const PERSON_USER_ID = "person";
export const AGENT_USER_ID = "agent";

/**
 * What the person may do in the editor's comment UI. Comments are records of
 * what reached the agent, so none is edited, deleted or reacted to; threads
 * are resolved and reopened, never deleted.
 */
class DesignThreadAuth extends ThreadStoreAuth {
  constructor(private readonly canWrite: () => boolean) {
    super();
  }
  canCreateThread() {
    return this.canWrite();
  }
  canAddComment() {
    return this.canWrite();
  }
  canUpdateComment() {
    return false;
  }
  canDeleteComment() {
    return false;
  }
  canDeleteThread() {
    return false;
  }
  canResolveThread() {
    return true;
  }
  canUnresolveThread() {
    return true;
  }
  canAddReaction() {
    return false;
  }
  canDeleteReaction() {
    return false;
  }
}

/** Plain text of a BlockNote comment body (a small BlockNote document). */
export function bodyText(body: unknown): string {
  if (!Array.isArray(body)) return "";
  return body
    .map((block: { content?: unknown }) =>
      typeof block.content === "string"
        ? block.content
        : Array.isArray(block.content)
          ? block.content.map((inline: { text?: string }) => inline.text ?? "").join("")
          : "",
    )
    .filter((line) => line.length > 0)
    .join("\n")
    .trim();
}

function toComment(comment: DesignComment): CommentData {
  const at = new Date(comment.createdAt);
  return {
    type: "comment",
    id: comment.id,
    userId: comment.author === "agent" ? AGENT_USER_ID : PERSON_USER_ID,
    createdAt: at,
    updatedAt: at,
    reactions: [],
    metadata: { delivery: comment.delivery ?? null },
    body: [{ type: "paragraph", content: comment.body }],
  };
}

export function toThreadData(thread: DesignThread): ThreadData {
  const comments = thread.comments.map(toComment);
  const createdAt = new Date(thread.createdAt);
  const updatedAt = comments.length ? comments[comments.length - 1].updatedAt : createdAt;
  return {
    type: "thread",
    id: thread.id,
    createdAt,
    updatedAt,
    comments,
    resolved: thread.status === "resolved",
    resolvedUpdatedAt: thread.resolvedAt ? new Date(thread.resolvedAt) : undefined,
    resolvedBy: thread.resolvedBy ?? undefined,
    metadata: { number: thread.number, kind: thread.kind, deliveryStatus: thread.deliveryStatus },
  };
}

interface PendingThread {
  id: string;
  commentId: string;
  body: string;
}

/**
 * BlockNote's comment UI over threads kanna-server owns. The server is the
 * record of numbering, delivery and resolution; the editor only renders it.
 *
 * Creating a comment is ordered so the anchor is never missing: BlockNote
 * asks for the thread, then for its anchor. The thread is only recorded
 * locally until then; the anchor's comment mark is written, the document is
 * flushed to the server, and only then is the thread created, carrying the
 * state vector that includes the mark, so the server can tell whether its
 * copy of the document already holds the anchor before delivering.
 */
export class KannaThreadStore extends ThreadStore {
  private pending = new Map<string, PendingThread>();
  private listeners = new Set<(threads: Map<string, ThreadData>) => void>();
  private unsubscribe: () => void;

  constructor(
    private readonly session: DesignSession,
    private readonly handlers: { onError?: (error: unknown) => void; canWrite?: () => boolean } = {},
  ) {
    super(new DesignThreadAuth(handlers.canWrite ?? (() => true)));
    this.unsubscribe = session.subscribe(() => this.notify());
  }

  destroy(): void {
    this.unsubscribe();
    this.listeners.clear();
  }

  private notify(): void {
    const threads = this.getThreads();
    for (const listener of this.listeners) listener(threads);
  }

  getThreads(): Map<string, ThreadData> {
    return new Map((this.session.view?.threads ?? []).map((thread) => [thread.id, toThreadData(thread)]));
  }

  getThread(threadId: string): ThreadData {
    const thread = this.session.view?.threads.find((candidate) => candidate.id === threadId);
    if (!thread) throw new Error(`thread ${threadId} is not in this design`);
    return toThreadData(thread);
  }

  subscribe(callback: (threads: Map<string, ThreadData>) => void): () => void {
    this.listeners.add(callback);
    return () => this.listeners.delete(callback);
  }

  async createThread(options: { initialComment: { body: unknown } }): Promise<ThreadData> {
    const pending: PendingThread = {
      id: newId("th"),
      commentId: newId("cm"),
      body: bodyText(options.initialComment.body),
    };
    if (!pending.body) throw new Error("A comment needs text.");
    this.pending.set(pending.id, pending);
    const now = new Date();
    return {
      type: "thread",
      id: pending.id,
      createdAt: now,
      updatedAt: now,
      comments: [],
      resolved: false,
      metadata: { pending: true },
    };
  }

  addThreadToDocument = async (options: {
    threadId: string;
    selection: { from?: number; to?: number; anchor?: number; head?: number };
    editor: BlockNoteEditor<any, any, any>;
  }): Promise<void> => {
    const pending = this.pending.get(options.threadId);
    if (!pending) return;
    this.pending.delete(options.threadId);
    const tiptap = (options.editor as unknown as { _tiptapEditor: TiptapLike })._tiptapEditor;
    const from = Math.min(options.selection.from ?? options.selection.anchor ?? 0, options.selection.to ?? options.selection.head ?? 0);
    const to = Math.max(options.selection.from ?? options.selection.anchor ?? 0, options.selection.to ?? options.selection.head ?? 0);
    const quotedText = tiptap.state.doc.textBetween(from, to, " ").trim();
    const blockId = blockIdAt(options.editor, from);
    tiptap.chain().setTextSelection({ from, to }).setMark("comment", { orphan: false, threadId: pending.id }).run();
    try {
      await this.session.whenSaved();
      await this.session.createThread({
        threadId: pending.id,
        commentId: pending.commentId,
        kind: "comment",
        body: pending.body,
        anchor: { blockId: blockId ?? "", quotedText: quotedText || pending.body.slice(0, 80), stateVector: this.session.stateVector() },
      });
    } catch (error) {
      // Not accepted: take the anchor back out so nothing claims feedback
      // that the agent will never get.
      tiptap.chain().setTextSelection({ from, to }).unsetMark("comment").run();
      this.handlers.onError?.(error);
      throw error;
    }
  };

  async addComment(options: { comment: { body: unknown }; threadId: string }): Promise<CommentData> {
    const body = bodyText(options.comment.body);
    const commentId = newId("cm");
    try {
      const thread = await this.session.reply(options.threadId, { commentId, body });
      const comment = thread.comments.find((candidate) => candidate.id === commentId) ?? thread.comments[thread.comments.length - 1];
      return toComment(comment);
    } catch (error) {
      this.handlers.onError?.(error);
      throw error;
    }
  }

  async resolveThread(options: { threadId: string }): Promise<void> {
    await this.session.resolve(options.threadId, true);
  }

  async unresolveThread(options: { threadId: string }): Promise<void> {
    await this.session.resolve(options.threadId, false);
  }

  async updateComment(): Promise<void> {
    throw new Error("Comments are a record of what reached the agent and cannot be edited.");
  }

  async deleteComment(): Promise<void> {
    throw new Error("Comments are a record of what reached the agent and cannot be deleted.");
  }

  async deleteThread(): Promise<void> {
    throw new Error("Threads are resolved, not deleted.");
  }

  async addReaction(): Promise<void> {}

  async deleteReaction(): Promise<void> {}
}

interface TiptapLike {
  state: { doc: { textBetween(from: number, to: number, separator?: string): string } };
  chain(): {
    setTextSelection(range: { from: number; to: number }): ReturnType<TiptapLike["chain"]>;
    setMark(name: string, attrs: Record<string, unknown>): ReturnType<TiptapLike["chain"]>;
    unsetMark(name: string): ReturnType<TiptapLike["chain"]>;
    run(): boolean;
  };
}

/** The id of the block holding a document position. */
function blockIdAt(editor: BlockNoteEditor<any, any, any>, position: number): string | null {
  const tiptap = (editor as unknown as { _tiptapEditor: { state: { doc: { resolve(pos: number): ResolvedLike } } } })._tiptapEditor;
  try {
    const resolved = tiptap.state.doc.resolve(position);
    for (let depth = resolved.depth; depth >= 0; depth -= 1) {
      const node = resolved.node(depth);
      if (node.type.name === "blockContainer") return node.attrs.id as string;
    }
  } catch {
    // Fall through to the cursor's block.
  }
  return editor.getTextCursorPosition().block.id ?? null;
}

interface ResolvedLike {
  depth: number;
  node(depth: number): { type: { name: string }; attrs: Record<string, unknown> };
}
