import { useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { filterSuggestionItems, FormattingToolbarExtension } from "@blocknote/core/extensions";
import { withCollaboration } from "@blocknote/core/yjs";
import { CommentsExtension } from "@blocknote/core/comments";
import {
  FormattingToolbar,
  FormattingToolbarController,
  getDefaultReactSlashMenuItems,
  getFormattingToolbarItems,
  SuggestionMenuController,
  useCreateBlockNote,
  useExtension,
  type DefaultReactSuggestionItem,
} from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import { AGENT_PREFIX, parseAgentLine } from "./agentLine";
import { DOCUMENT_FRAGMENT, designSchema } from "./schema";
import { newId, type DesignSession } from "./session";
import { AGENT_USER_ID, KannaThreadStore, PERSON_USER_ID } from "./threadStore";

export interface DesignEditorLabels {
  comment: string;
  commentShortcut: string;
  agentItemTitle: string;
  agentItemSubtext: string;
  agentGroup: string;
  person: string;
  agent: string;
  messageSent: string;
  messageFailed: string;
  commentFailed: string;
}

export const ENGLISH_LABELS: DesignEditorLabels = {
  comment: "Comment",
  commentShortcut: "⌘↵",
  agentItemTitle: "Agent",
  agentItemSubtext: "Message the agent: /agent your message, then Enter",
  agentGroup: "Agent",
  person: "You",
  agent: "Agent",
  messageSent: "Sent to the agent's queue",
  messageFailed: "The message was not sent",
  commentFailed: "The comment was not sent",
};

export interface DesignEditorProps {
  session: DesignSession;
  theme: "light" | "dark";
  /** False when the design is not being edited (handed off, incompatible). */
  editable: boolean;
  labels?: DesignEditorLabels;
  /** Short notices for the host to show (sent, failed). */
  onNotice?: (message: string, kind: "info" | "error") => void;
  /** The thread the host's feedback panel selected, to scroll to its anchor. */
  selectedThreadId?: string | null;
}

const DUPLICATE_COMMENT_BUTTONS = new Set(["addCommentButton", "addTiptapCommentButton"]);

function CommentToolbarButton({ onComment, labels }: { onComment: () => void; labels: DesignEditorLabels }) {
  const { store } = useExtension(FormattingToolbarExtension);
  return (
    <button
      type="button"
      className="kanna-toolbar-comment"
      aria-label={`${labels.comment} (${labels.commentShortcut})`}
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => {
        onComment();
        store.setState(false);
      }}
    >
      <span aria-hidden="true">💬</span>
      {labels.comment}
      <kbd>{labels.commentShortcut}</kbd>
    </button>
  );
}

/**
 * The live design document (docs/specs/app-design.md §4): BlockNote on the
 * session's Yjs document, in the one shared schema, with the person's comment
 * threads served by kanna-server. Mount it only after the session's first
 * sync, so BlockNote never writes an initial block of its own into a
 * document the server already has.
 */
export function DesignEditor(props: DesignEditorProps): ReactElement {
  const { session, theme, editable } = props;
  const labels = props.labels ?? ENGLISH_LABELS;
  const container = useRef<HTMLDivElement>(null);
  const sending = useRef(false);
  const notice = useRef(props.onNotice);
  notice.current = props.onNotice;

  const editableRef = useRef(editable);
  editableRef.current = editable;
  const threadStore = useMemo(
    () =>
      new KannaThreadStore(session, {
        canWrite: () => editableRef.current,
        onError: () => notice.current?.(labels.commentFailed, "error"),
      }),
    [session],
  );
  useEffect(() => () => threadStore.destroy(), [threadStore]);

  const editor = useCreateBlockNote(
    withCollaboration({
      schema: designSchema,
      collaboration: {
        fragment: session.doc.getXmlFragment(DOCUMENT_FRAGMENT),
        user: { name: labels.person, color: "#7b4ff0" },
        showCursorLabels: "activity",
      },
      extensions: [
        CommentsExtension({
          threadStore,
          resolveUsers: async (ids: string[]) =>
            ids.map((id) => ({
              id,
              username: id === AGENT_USER_ID ? labels.agent : id === PERSON_USER_ID ? labels.person : id,
              avatarUrl: "",
            })),
        }),
      ],
    }),
    [session, threadStore],
  );

  const comments = () => editor.getExtension(CommentsExtension);
  const startComment = () => comments()?.startPendingComment();

  const sendToAgent = async (message: string) => {
    if (sending.current) return false;
    sending.current = true;
    try {
      await session.createThread({ threadId: newId("th"), commentId: newId("cm"), kind: "message", body: message });
      notice.current?.(labels.messageSent, "info");
      return true;
    } catch {
      notice.current?.(labels.messageFailed, "error");
      return false;
    } finally {
      sending.current = false;
    }
  };

  // "/agent" completes in place: picking it leaves "/agent " in the line and
  // focus there; the next Enter on "/agent <message>" sends it, once.
  const completeAgent = () => {
    const menu = editor.getExtension("suggestionMenu") as { clearQuery(): void; closeMenu(): void } | undefined;
    menu?.clearQuery();
    menu?.closeMenu();
    editor.insertInlineContent(AGENT_PREFIX);
  };

  useEffect(() => {
    const root = container.current;
    if (!root) return;
    const menuShown = () =>
      (editor.getExtension("suggestionMenu") as { shown(): boolean } | undefined)?.shown() ?? false;
    const onKeyDown = (event: KeyboardEvent) => {
      if (!root.contains(document.activeElement)) return;
      // Tab while the / menu is open picks the highlighted item, like Enter.
      if (event.key === "Tab" && !event.shiftKey && menuShown()) {
        event.preventDefault();
        event.stopPropagation();
        document.activeElement?.dispatchEvent(
          new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
        );
        return;
      }
      // ⌘↵ with text selected starts a comment.
      if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
        const selection = window.getSelection();
        if (selection && !selection.isCollapsed && root.contains(selection.anchorNode)) {
          event.preventDefault();
          event.stopPropagation();
          startComment();
        }
        return;
      }
      if (event.key !== "Enter" || event.shiftKey || event.isComposing || menuShown()) return;
      const block = editor.getTextCursorPosition()?.block;
      const text = Array.isArray(block?.content)
        ? (block.content as Array<{ text?: string }>).map((inline) => inline.text ?? "").join("")
        : "";
      const line = parseAgentLine(text);
      if (line.kind === "none") return;
      event.preventDefault();
      event.stopPropagation();
      if (line.kind === "empty") return;
      editor.updateBlock(block, { content: [] });
      void sendToAgent(line.message).then((sent) => {
        // Not sent: give the words back rather than lose them.
        if (!sent) editor.updateBlock(block, { content: `${AGENT_PREFIX}${line.message}` });
      });
    };
    root.addEventListener("keydown", onKeyDown, true);
    return () => root.removeEventListener("keydown", onKeyDown, true);
  }, [editor]);

  // Scroll to a thread's anchor when the feedback panel selects it.
  useEffect(() => {
    const id = props.selectedThreadId;
    if (!id || !container.current) return;
    const mark = container.current.querySelector(`[data-bn-thread-id="${CSS.escape(id)}"]`);
    mark?.scrollIntoView({ block: "center", behavior: "smooth" });
    comments()?.selectThread(id);
  }, [props.selectedThreadId]);

  const slashItems = async (query: string) =>
    filterSuggestionItems<DefaultReactSuggestionItem>(
      [
        {
          title: labels.agentItemTitle,
          subtext: labels.agentItemSubtext,
          aliases: ["agent", "claude", "ask", "ai"],
          group: labels.agentGroup,
          icon: <span className="kanna-agent-icon">✦</span>,
          onItemClick: completeAgent,
        },
        ...getDefaultReactSlashMenuItems(editor),
      ],
      query,
    );

  return (
    <div ref={container} className="kanna-design-editor" data-theme={theme}>
      <BlockNoteView
        editor={editor}
        theme={theme}
        editable={editable}
        slashMenu={false}
        formattingToolbar={false}
      >
        <FormattingToolbarController
          formattingToolbar={() => (
            <FormattingToolbar>
              <CommentToolbarButton key="kannaComment" onComment={startComment} labels={labels} />
              {getFormattingToolbarItems().filter((item) => !DUPLICATE_COMMENT_BUTTONS.has(String(item.key)))}
            </FormattingToolbar>
          )}
        />
        <SuggestionMenuController triggerCharacter="/" getItems={slashItems} />
      </BlockNoteView>
    </div>
  );
}

/** Render until the session's first sync, then the editor. */
export function DesignEditorWhenReady(props: DesignEditorProps & { loading?: ReactElement }): ReactElement | null {
  const [ready, setReady] = useState(props.session.status !== "connecting");
  useEffect(
    () =>
      props.session.subscribe(() => {
        if (props.session.status !== "connecting") setReady(true);
      }),
    [props.session],
  );
  return ready ? <DesignEditor {...props} /> : (props.loading ?? null);
}
