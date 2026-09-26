import { useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { BlockNoteView } from "@blocknote/mantine";
import { ENGLISH_LABELS, useDesignBlockNote } from "../DesignEditor";
import { newId, type DesignSession } from "../session";
import { createAnchoredComment } from "../threadStore";
import type { DesignThread } from "../types";

export interface PhoneLabels {
  comment: string;
  commentOn: string;
  messageAgent: string;
  send: string;
  sending: string;
  notSent: string;
  retry: string;
  cancel: string;
  reply: string;
  resolve: string;
  reopen: string;
  resend: string;
  resolved: string;
  feedback: string;
  empty: string;
  agent: string;
  message: string;
  saved: string;
  offline: string;
  incompatible: string;
  handedOff: string;
  delivery: Record<string, string>;
}

export const ENGLISH_PHONE_LABELS: PhoneLabels = {
  comment: "💬 Comment",
  commentOn: "Comment on",
  messageAgent: "Message the agent…",
  send: "Send",
  sending: "Sending…",
  notSent: "Not sent",
  retry: "Try again",
  cancel: "Cancel",
  reply: "Reply",
  resolve: "Resolve",
  reopen: "Reopen",
  resend: "Send again",
  resolved: "resolved",
  feedback: "Feedback → agent",
  empty: "Select text to comment, or message the agent below. Feedback is queued and delivered when the agent is free.",
  agent: "Agent",
  message: "✦ message to the agent",
  saved: "Up to date",
  offline: "Offline: will catch up when reconnected",
  incompatible: "This document needs a newer Kanna app.",
  handedOff: "Handed to the software factory: read-only.",
  delivery: {
    queued: "queued for the agent",
    delivering: "delivering…",
    delivered: "delivered ✓",
    agent_replied: "agent replied",
    uncertain: "not delivered: check the terminal, then send again",
    held: "held: design handed off",
    cancelled: "not delivered",
    none: "",
  },
};

/** A send the person made that the server has not accepted yet. */
interface PendingSend {
  key: string;
  label: string;
  state: "sending" | "failed";
  error?: string;
  run: () => Promise<void>;
}

/**
 * The design on the phone (docs/specs/app-design.md §11, the owner's choice
 * for slice 1): view the current document and comment on it, message the
 * agent, and follow and answer the feedback threads. The document is not
 * edited here; commenting writes only its anchor mark.
 */
export function PhoneDesign(props: { session: DesignSession; theme: "light" | "dark"; labels?: PhoneLabels }): ReactElement {
  const { session } = props;
  const labels = props.labels ?? ENGLISH_PHONE_LABELS;
  const [, rerender] = useState(0);
  useEffect(() => session.subscribe(() => rerender((n) => n + 1)), [session]);
  const view = session.view;
  const canWrite = !!view?.inDesignStage && view.status === "designing" && session.status !== "incompatible";
  const { editor } = useDesignBlockNote(session, { ...ENGLISH_LABELS, person: "You", agent: labels.agent }, {
    canWrite: () => canWrite,
    onError: () => undefined,
  });
  const root = useRef<HTMLDivElement>(null);
  const [selection, setSelection] = useState<{ from: number; to: number; text: string } | null>(null);
  const [composer, setComposer] = useState<{ mode: "message" } | { mode: "comment"; from: number; to: number; text: string } | { mode: "reply"; threadId: string; number: number }>({ mode: "message" });
  const [draft, setDraft] = useState("");
  const [pending, setPending] = useState<PendingSend[]>([]);
  const [showResolved, setShowResolved] = useState(false);

  // A selection in the document offers a comment on it.
  useEffect(() => {
    const onSelection = () => {
      const tiptap = (editor as unknown as { _tiptapEditor: { state: { selection: { from: number; to: number }; doc: { textBetween(a: number, b: number, s?: string): string } } } })._tiptapEditor;
      const { from, to } = tiptap.state.selection;
      const text = from < to ? tiptap.state.doc.textBetween(from, to, " ").trim() : "";
      const inside = root.current?.contains(document.getSelection()?.anchorNode ?? null);
      setSelection(text && inside ? { from, to, text } : null);
    };
    document.addEventListener("selectionchange", onSelection);
    return () => document.removeEventListener("selectionchange", onSelection);
  }, [editor]);

  /** Run a send; keep it visible until the server accepts it, with a retry on failure. */
  const send = (key: string, label: string, run: () => Promise<void>) => {
    const attempt = async () => {
      setPending((items) => items.map((item) => (item.key === key ? { ...item, state: "sending", error: undefined } : item)));
      try {
        await run();
        setPending((items) => items.filter((item) => item.key !== key));
      } catch (error) {
        setPending((items) =>
          items.map((item) =>
            item.key === key ? { ...item, state: "failed", error: error instanceof Error ? error.message : String(error) } : item,
          ),
        );
      }
    };
    setPending((items) => [...items, { key, label, state: "sending", run: attempt }]);
    void attempt();
  };

  const submit = () => {
    const body = draft.trim();
    if (!body || !canWrite) return;
    setDraft("");
    // Ids are chosen once, so a retried send is the same thread or comment.
    const threadId = newId("th");
    const commentId = newId("cm");
    if (composer.mode === "message") {
      send(commentId, body, () =>
        session.createThread({ threadId, commentId, kind: "message", body }).then(() => undefined),
      );
    } else if (composer.mode === "comment") {
      const { from, to, text } = composer;
      send(commentId, `${labels.commentOn} “${text}”: ${body}`, () =>
        createAnchoredComment(session, editor, { from, to }, body, { threadId, commentId }),
      );
    } else {
      send(commentId, body, () => session.reply(composer.threadId, { commentId, body }).then(() => undefined));
    }
    setComposer({ mode: "message" });
  };

  const threads = useMemo(() => [...(view?.threads ?? [])].sort((a, b) => a.number - b.number), [view]);
  const open = threads.filter((thread) => thread.status === "open");
  const resolved = threads.filter((thread) => thread.status === "resolved");
  const statusLine =
    session.status === "incompatible"
      ? labels.incompatible
      : session.status === "offline"
        ? labels.offline
        : view && !view.inDesignStage
          ? labels.handedOff
          : labels.saved;

  return (
    <div className="kanna-phone" data-theme={props.theme}>
      <div className={`kanna-phone-status status-${session.status}`} role="status">
        {statusLine}
      </div>
      <div ref={root} className="kanna-phone-document kanna-design-editor" data-theme={props.theme}>
        <BlockNoteView editor={editor} theme={props.theme} editable={false} slashMenu={false} formattingToolbar={false} />
      </div>
      {selection && canWrite && composer.mode !== "comment" ? (
        <button
          type="button"
          className="kanna-phone-comment-button"
          onClick={() => setComposer({ mode: "comment", ...selection })}
        >
          {labels.comment}
        </button>
      ) : null}
      <section className="kanna-phone-feed" aria-label={labels.feedback}>
        <h2>{labels.feedback}</h2>
        {resolved.length ? (
          <button type="button" className="kanna-phone-link" onClick={() => setShowResolved((value) => !value)}>
            {showResolved ? "▾" : "▸"} {resolved.length} {labels.resolved}
          </button>
        ) : null}
        {[...(showResolved ? resolved : []), ...open].map((thread) => (
          <ThreadCard
            key={thread.id}
            thread={thread}
            labels={labels}
            canWrite={canWrite}
            onReply={() => setComposer({ mode: "reply", threadId: thread.id, number: thread.number })}
            onResolve={() => void session.resolve(thread.id, thread.status !== "resolved")}
            onResend={(deliveryId) => void session.retryDelivery(deliveryId)}
          />
        ))}
        {pending.map((item) => (
          <div key={item.key} className={`kanna-phone-pending ${item.state}`}>
            <span>{item.label}</span>
            <small>{item.state === "sending" ? labels.sending : `${labels.notSent}${item.error ? `: ${item.error}` : ""}`}</small>
            {item.state === "failed" ? (
              <button type="button" className="kanna-phone-link" onClick={() => void item.run()}>
                {labels.retry}
              </button>
            ) : null}
          </div>
        ))}
        {!threads.length && !pending.length ? <p className="kanna-phone-empty">{labels.empty}</p> : null}
      </section>
      {canWrite ? (
        <form
          className="kanna-phone-composer"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          {composer.mode !== "message" ? (
            <div className="kanna-phone-composer-context">
              <span>
                {composer.mode === "comment" ? `${labels.commentOn} “${composer.text}”` : `${labels.reply} #${composer.number}`}
              </span>
              <button type="button" className="kanna-phone-link" onClick={() => setComposer({ mode: "message" })}>
                {labels.cancel}
              </button>
            </div>
          ) : null}
          <div className="kanna-phone-composer-row">
            <textarea
              value={draft}
              rows={1}
              placeholder={composer.mode === "message" ? labels.messageAgent : ""}
              aria-label={composer.mode === "message" ? labels.messageAgent : labels.reply}
              onChange={(event) => setDraft(event.target.value)}
            />
            <button type="submit" disabled={!draft.trim()}>
              {labels.send}
            </button>
          </div>
        </form>
      ) : null}
    </div>
  );
}

function ThreadCard(props: {
  thread: DesignThread;
  labels: PhoneLabels;
  canWrite: boolean;
  onReply: () => void;
  onResolve: () => void;
  onResend: (deliveryId: string) => void;
}): ReactElement {
  const { thread, labels } = props;
  const person = thread.comments.filter((comment) => comment.author === "operator");
  const last = person[person.length - 1] ?? thread.comments[0];
  const reply = thread.comments.slice(thread.comments.indexOf(last) + 1).filter((comment) => comment.author === "agent").pop();
  const uncertain = last?.delivery?.state === "uncertain" ? last.delivery.id : null;
  return (
    <article className={`kanna-phone-thread ${thread.status}`}>
      <header>
        <span className="kanna-phone-number">{thread.number}</span>
        <span className="kanna-phone-anchor">
          {thread.kind === "message" ? labels.message : `“${thread.anchor?.quotedText ?? ""}”`}
        </span>
      </header>
      <p>{last?.body}</p>
      <small className={`delivery-${thread.deliveryStatus}`}>{labels.delivery[thread.deliveryStatus] ?? thread.deliveryStatus}</small>
      {reply ? (
        <p className="kanna-phone-reply">
          <strong>{labels.agent}:</strong> {reply.body}
        </p>
      ) : null}
      <footer>
        {uncertain && props.canWrite ? (
          <button type="button" className="kanna-phone-link" onClick={() => props.onResend(uncertain)}>
            {labels.resend}
          </button>
        ) : null}
        {props.canWrite && thread.status === "open" ? (
          <button type="button" className="kanna-phone-link" onClick={props.onReply}>
            {labels.reply}
          </button>
        ) : null}
        <button type="button" className="kanna-phone-link" onClick={props.onResolve}>
          {thread.status === "resolved" ? labels.reopen : labels.resolve}
        </button>
      </footer>
    </article>
  );
}
