import type { AgentSelection } from "../config/agent-providers";

export interface WorkflowEnvironment {
  setup?: string[];
  teardown?: string[];
}

export interface WorkflowStagePolicy {
  transition: "manual" | "auto";
  /** Legacy routing: how a stage entered by a revision request leaves. */
  revision_transition?: "manual" | "auto";
  /**
   * Routing "exits" only: how a stage re-entered by a loop leaves through its
   * `advance` exit. Defaults to `transition`.
   */
  loop_transition?: "manual" | "auto";
  /**
   * Routing "exits" only, final stage only: leaving the stage hands the
   * task's pull request to the repository's merge master, delivering the same
   * request a legacy approve post sends.
   */
  handoff?: "merge";
}

/**
 * How a stage's result chooses where the task goes. Absent means "legacy":
 * success follows the transition policy and a reviewer names a stage through
 * the revision API under the task-wide `revision_limit`. "exits" opts into
 * named exits: a result names one of its stage's exits (never a stage), and
 * each loop spends its destination stage's own `budget`.
 */
export type WorkflowRouting = "legacy" | "exits";

/** Every stage's implicit exit to the next stage; never declared. */
export const ADVANCE_EXIT = "advance";

/** Loops into a stage before a further one parks, when no budget is set. */
export const DEFAULT_STAGE_BUDGET = 5;

/**
 * Tail work of a stage, injected into the stage's running agent session when
 * the stage transitions forward. `agent` is the fallback used to spawn a
 * fresh session when the task's session is dead.
 */
export interface WorkflowPost {
  name: string;
  description?: string;
  agent?: string;
  prompt?: string;
  /**
   * Compact provider selectors (`provider[-model[-effort]]`, e.g. `claude`,
   * `codex-gpt-5.6-sol`, `claude-fable-hi`) — validated by the loader via
   * `parseAgentProviderSelector`; entries keep their written form.
   */
  agent_provider?: AgentSelection;
}

export interface WorkflowStage {
  name: string;
  description?: string;
  agent?: string;
  prompt?: string;
  /**
   * Compact provider selectors (`provider[-model[-effort]]`, e.g. `claude`,
   * `codex-gpt-5.6-sol`, `claude-fable-hi`) — validated by the loader via
   * `parseAgentProviderSelector`; entries keep their written form.
   */
  agent_provider?: AgentSelection;
  environment?: string;
  /**
   * Routing "exits" only: loop exits by name, each mapped to this stage or an
   * earlier one (e.g. `{ revise: "in progress", replan: "plan" }`).
   */
  exits?: Record<string, string>;
  /** Routing "exits" only: agent loops into this stage before one parks. */
  budget?: number;
  policy: WorkflowStagePolicy;
  post?: WorkflowPost;
  /**
   * Routing "exits" only: the stage's forward transition starts with a commit
   * step — the live session commits and records its result (or a short commit
   * session runs in the same workspace), and the transition fires on it.
   */
  exit_commit?: boolean;
  /** Routing "exits" only: commands run in the workspace on entering the stage. */
  setup?: string[];
  /** Routing "exits" only: commands run in the workspace on leaving the stage. */
  teardown?: string[];
}

export interface WorkflowDefinition {
  name: string;
  description?: string;
  environments?: Record<string, WorkflowEnvironment>;
  stages: WorkflowStage[];
  /**
   * Agent-requested revision rounds a task may spend before Kanna stops
   * forking revisions and parks the task for its human. Omitted means the
   * engine default (3); 0 disables the cap. Enforced server-side by
   * `request_revision`.
   */
  revision_limit?: number;
  /**
   * The plan Kanna stamps onto a task's pinned workflow when its planning
   * stage publishes the remaining stages in the same call that records the
   * plan. Server-written provenance, never authored in a `.kanna/workflows`
   * file: an edit that changes it is refused, and its `result` is bound to the
   * reserved `$PLAN_RESULT` prompt variable for the whole extended workflow.
   */
  plan_context?: WorkflowPlanContext;
  /** Result routing contract; absent means "legacy". */
  routing?: WorkflowRouting;
  /** Routing "exits" only: the budget of a stage that declares none. */
  budget?: number;
}

export interface WorkflowPlanContext {
  source_run_id: string;
  stage: string;
  /** The full recorded stage result, in the shape `$PREV_MAIN_RESULT` carries. */
  result: string;
}

export interface AgentDefinition {
  name: string;
  description: string;
  agent_provider?: AgentSelection;
  model?: string;
  effort?: string;
  permission_mode?: "default" | "acceptEdits" | "dontAsk";
  allowed_tools?: string[];
  prompt: string; // markdown body
}

/**
 * A repo-local extension (`.kanna/agents/{name}/EXTEND.md`) layered onto the
 * resolved agent definition — the repo's own AGENT.md override or the bundled
 * built-in. The body is appended to the base prompt; frontmatter fields
 * replace the base's when present. The agent's identity (name) comes from the
 * directory, so an extension cannot rename the agent.
 */
export interface AgentExtension {
  description?: string;
  agent_provider?: AgentSelection;
  model?: string;
  effort?: string;
  permission_mode?: "default" | "acceptEdits" | "dontAsk";
  allowed_tools?: string[];
  prompt: string; // markdown body appended to the base prompt
}

/**
 * The verdict vocabulary `kanna_complete_stage` accepts, mirroring
 * `kanna_runtime_defaults::stage_verdict`. Only `success` completes a stage;
 * every other word records what happened and stops advancement. `closed` is
 * deliberately absent — closing a task is a lifecycle action, not a verdict.
 */
export type StageVerdict =
  | "success"
  | "unverified"
  | "partial"
  | "needs-input"
  | "declined"
  | "failure";

export interface StageCompleteResult {
  status: StageVerdict;
  summary: string;
  metadata?: Record<string, unknown>;
}
