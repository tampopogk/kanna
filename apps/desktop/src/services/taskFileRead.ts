/**
 * The two ways a task file read comes back as a verdict about the *file*
 * rather than as an outage.
 *
 * `kanna-server` refuses a file over `MAX_TASK_FILE_BYTES` (1 MiB) and a file
 * that is not UTF-8 before it returns any content — see
 * `crates/kanna-server/src/task_files.rs` — and
 * `http_api/task_files.rs` maps those to HTTP 413 and 415. Every reader that
 * goes through the server (the contained local loader, LAN, relay) therefore
 * gets a refusal that says nothing is wrong with the worktree, the task or the
 * connection: that file simply has no text to show.
 *
 * Callers that only display a file act on this the same way they act on a
 * binary they never asked for — the tree explorer's preview column shows
 * "(no preview)" — while a genuine read failure keeps its error path.
 */
export type TaskFileUnreadableReason = "too-large" | "not-text";

/**
 * A refusal carrying the distinction the server drew, so callers act on it
 * instead of pattern-matching prose. The message is the underlying adapter's
 * own, unchanged, so anything that only displays it (the file view) reads
 * exactly as it did before.
 */
export class TaskFileUnreadableError extends Error {
  constructor(
    readonly reason: TaskFileUnreadableReason,
    message: string,
    options?: { cause?: unknown },
  ) {
    super(message, options);
    this.name = "TaskFileUnreadableError";
  }
}

export function isTaskFileUnreadableError(error: unknown): error is TaskFileUnreadableError {
  return error instanceof TaskFileUnreadableError;
}

export function taskFileUnreadableReasonForStatus(
  status: number,
): TaskFileUnreadableReason | null {
  if (status === 413) return "too-large";
  if (status === 415) return "not-text";
  return null;
}

/**
 * The same verdict when it arrives as prose.
 *
 * The LAN peer protocol carries an error as a message and nothing else
 * (`PeerResponse::Error`), and the message the owning desktop produces is
 * `"Kanna server task file read failed with HTTP {status}: {body}"` — see
 * `get_local_kanna_task_json` in `crates/task-transfer/src/runtime/daemon.rs`.
 * Reading the status back out of that sentence is the only classification
 * available on this side of a protocol that does not carry one; giving the
 * peer protocol a structured error is the real fix and a change of its own.
 */
export function taskFileUnreadableReasonForRelayedMessage(
  message: string,
): TaskFileUnreadableReason | null {
  const status = /\bHTTP (\d{3})\b/.exec(message);
  return status ? taskFileUnreadableReasonForStatus(Number(status[1])) : null;
}
