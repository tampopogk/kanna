// Provider identity comes from the generated @kanna/agent-protocol contract.
// This module keeps the shared frontmatter helper alongside compatibility
// exports used by the agent-definition and task-template loaders.
import {
  AGENT_PROVIDERS,
  isAgentProvider,
  type AgentProvider,
  type AgentCandidate,
  type AgentSelectionEntry,
} from "@kanna/agent-protocol";

export { AGENT_PROVIDERS, isAgentProvider };
export type { AgentProvider };
export const VALID_AGENT_PROVIDERS = AGENT_PROVIDERS;
export type KnownAgentProvider = AgentProvider;

/**
 * Split a frontmatter `agent_provider` value into an ordered list of trimmed,
 * non-empty tokens. Accepts a YAML array, a single string, or a comma-separated
 * string (e.g. `codex, claude, copilot, opencode, antigravity`). Does not validate the tokens — callers
 * decide whether unknown providers should throw or be filtered out.
 */
export function splitAgentProviderValue(value: unknown): string[] {
  if (Array.isArray(value)) {
    return value
      .filter((v): v is string => typeof v === "string")
      .map((v) => v.trim())
      .filter((v) => v.length > 0);
  }
  if (typeof value === "string") {
    return value
      .split(",")
      .map((v) => v.trim())
      .filter((v) => v.length > 0);
  }
  return [];
}

/**
 * A parsed compact provider selector: `provider[-model[-effort]]`.
 *
 * Workflow stage/post `agent_provider` entries name provider candidates with
 * an optional model and reasoning effort folded into one token — `claude`,
 * `codex-gpt-5.6-sol`, `claude-fable-hi`, `codex-gpt-6-astra-lo` — so each candidate in an
 * ordered fallback list carries its own coherent pair. Anything not specified
 * inherits the provider CLI's own defaults.
 */
export interface AgentProviderSelector {
  provider: AgentProvider;
  model?: string;
  effort?: string;
  /**
   * Claude's per-session auto-compact window. Compact selectors have no slot
   * for it, so it is set only by a structured candidate.
   */
  autocompact?: string;
}

/**
 * Effort tokens a selector's trailing segment may use, mapped to the
 * canonical spelling. Only these tokens read as an effort suffix; any other
 * trailing segment is part of the model string. Mirrors the Rust parser in
 * crates/kanna-agent-protocol/src/providers.rs (`parse_provider_selector`) —
 * keep the two in step.
 */
const EFFORT_ALIASES: Record<string, string> = {
  lo: "low",
  low: "low",
  med: "medium",
  medium: "medium",
  hi: "high",
  high: "high",
  xhi: "xhigh",
  xhigh: "xhigh",
  max: "max",
};

/**
 * Parse a compact provider selector, returning `null` when the value is not
 * one. The first `-`-separated segment must be a known provider id; a
 * recognized trailing effort token is the effort (normalized), and everything
 * in between is the model, kept verbatim (multi-segment ids like
 * `gpt-5.6-sol` survive). Provider-specific validity of the model and effort
 * (e.g. a provider with no model flag) is enforced server-side; this parser
 * covers syntax and provider identity.
 */
export function parseAgentProviderSelector(
  value: string,
): AgentProviderSelector | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (trimmed.length === 0) return null;
  const dash = trimmed.indexOf("-");
  const head = dash === -1 ? trimmed : trimmed.slice(0, dash);
  if (!isAgentProvider(head)) return null;
  if (dash === -1) return { provider: head };
  const rest = trimmed.slice(dash + 1);
  const segments = rest.split("-");
  if (segments.some((segment) => segment.length === 0)) return null;
  const last = segments[segments.length - 1];
  const effort = last !== undefined ? EFFORT_ALIASES[last] : undefined;
  const modelSegments = effort === undefined ? segments : segments.slice(0, -1);
  const selector: AgentProviderSelector = { provider: head };
  if (modelSegments.length > 0) selector.model = modelSegments.join("-");
  if (effort !== undefined) selector.effort = effort;
  return selector;
}

export type { AgentCandidate, AgentSelectionEntry };
export type AgentSelection = AgentSelectionEntry | AgentSelectionEntry[];

export function parseAgentCandidate(value: unknown): AgentCandidate | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const raw = value as Record<string, unknown>;
  if (!isAgentProvider(raw.harness) || Object.keys(raw).some(k => !["harness", "model", "effort", "autocompact"].includes(k))) return null;
  for (const key of ["model", "effort", "autocompact"]) {
    if (key in raw && (typeof raw[key] !== "string" || !raw[key] || raw[key].trim() !== raw[key] || /[\x00-\x1f\x7f-\x9f]/.test(raw[key]))) return null;
  }
  return value as AgentCandidate;
}

export function resolveAgentSelectionEntry(value: AgentSelectionEntry, compact = true): AgentProviderSelector | null {
  if (typeof value === "string") return compact ? parseAgentProviderSelector(value) : isAgentProvider(value) ? { provider: value } : null;
  const candidate = parseAgentCandidate(value);
  return candidate ? { provider: candidate.harness, model: candidate.model, effort: candidate.effort, autocompact: candidate.autocompact } : null;
}

export function parseAgentSelection(value: unknown, compact = true): AgentSelectionEntry[] {
  const entries = (Array.isArray(value) ? value : [value]).map(entry =>
    !compact && typeof entry === "string" ? entry.trim() : entry);
  if (!entries.length) throw new Error("agent_provider must include at least one non-empty provider");
  const structured = entries.some(entry => typeof entry === "object" && entry !== null);
  const seen = new Set<string>();
  for (const entry of entries) {
    const candidate = resolveAgentSelectionEntry(entry, compact);
    if (!candidate) throw new Error(`agent_provider must be a string or an array of strings or valid structured harness candidates (got ${JSON.stringify(entry)})`);
    if (structured && seen.has(candidate.provider)) throw new Error(`repeated harness '${candidate.provider}' in structured candidate list`);
    seen.add(candidate.provider);
  }
  return entries as AgentSelectionEntry[];
}

export function validateSelectionSiblings(entries: AgentSelectionEntry[], model?: string, effort?: string, autocompact?: string): void {
  for (const entry of entries) {
    if (typeof entry === "string") continue;
    if ((entry.model !== undefined && model !== undefined && entry.model !== model) ||
        (entry.effort !== undefined && effort !== undefined && entry.effort !== effort) ||
        (entry.autocompact !== undefined && autocompact !== undefined && entry.autocompact !== autocompact)) {
      throw new Error("conflicting nested and sibling model/effort in agent_provider");
    }
  }
}
