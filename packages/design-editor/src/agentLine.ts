/**
 * The `/agent` line (docs/specs/app-design.md §4): picking "Agent" from the
 * slash menu leaves `/agent ` in the line with focus there, and the next
 * Enter sends what follows as a message thread with no anchor.
 */
export type AgentLine =
  | { kind: "send"; message: string }
  /** `/agent` with nothing after it: Enter stays on the line. */
  | { kind: "empty" }
  | { kind: "none" };

const AGENT_LINE = /^\/(?:agent|claude)(?:\s+([\s\S]*))?$/i;

export function parseAgentLine(text: string): AgentLine {
  const match = text.trim().match(AGENT_LINE);
  if (!match) return { kind: "none" };
  const message = (match[1] ?? "").trim();
  return message ? { kind: "send", message } : { kind: "empty" };
}

/** What the slash menu's "Agent" item leaves in the line. */
export const AGENT_PREFIX = "/agent ";
