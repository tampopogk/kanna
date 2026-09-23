import {
  ADVANCE_EXIT,
  type WorkflowDefinition,
  type WorkflowPlanContext,
  type WorkflowPost,
  type WorkflowStage,
  type WorkflowStagePolicy,
} from "./workflow-types";
import { parseAgentSelection, parseAgentProviderSelector, type AgentSelection } from "../config/agent-providers";

function formatRawValue(value: unknown): string {
  if (value === undefined) {
    return "undefined";
  }
  if (typeof value === "string") {
    return value;
  }
  return JSON.stringify(value) ?? String(value);
}

function validationError(message: string): Error {
  return new Error(`Workflow validation failed:\n  - ${message}`);
}

function parseAgentProviderSelection(
  value: unknown,
  location: string,
): AgentSelection | undefined {
  if (value === undefined) return undefined;
  const values = Array.isArray(value) ? value : [value];
  const invalidStrings = values.filter(v => typeof v === "string" && parseAgentProviderSelector(v) === null);
  if (invalidStrings.length) throw validationError(`${location} has unsupported agent_provider values: ${invalidStrings.join(", ")}`);
  try {
    const entries = parseAgentSelection(value);
    return Array.isArray(value) ? entries : entries[0];
  } catch (error) {
    throw validationError(`${location} has an invalid agent_provider value: ${String(error)}`);
  }
}

function parseTransition(
  value: unknown,
  describeInvalid: (value: string) => string
): WorkflowStagePolicy["transition"] {
  if (value === "manual" || value === "auto") {
    return value;
  }

  throw validationError(describeInvalid(formatRawValue(value)));
}

/**
 * Legacy `execution` / `mode` markers. `"continue"` folds the stage into the
 * preceding stage's `post` (stages swap sessions; posts continue them);
 * anything else is ignored.
 */
function parseLegacyContinueMarker(value: unknown, stageName: string): boolean {
  if (value === undefined || value === "new_task") {
    return false;
  }
  if (value === "continue") {
    return true;
  }
  if (typeof value !== "string") return false;

  throw validationError(
    `Stage "${stageName}" has invalid execution "${value}"; must be "continue"`
  );
}

interface ParsedStagePolicy {
  policy: WorkflowStagePolicy;
  legacyContinue: boolean;
}

function parseStagePolicy(raw: Record<string, unknown>, stageName: string): ParsedStagePolicy {
  const policy = raw["policy"];
  if (policy !== undefined) {
    if (policy === null || typeof policy !== "object" || Array.isArray(policy)) {
      throw validationError(`Stage "${stageName}" has invalid policy "${formatRawValue(policy)}"; must be an object`);
    }
    const p = policy as Record<string, unknown>;
    const revisionTransition = p["revision_transition"] === undefined
      ? undefined
      : parseTransition(
          p["revision_transition"],
          (transition) =>
            `Stage "${stageName}" has invalid policy.revision_transition "${transition}"; must be "manual" or "auto"`
        );
    const loopTransition = p["loop_transition"] === undefined
      ? undefined
      : parseTransition(
          p["loop_transition"],
          (transition) =>
            `Stage "${stageName}" has invalid policy.loop_transition "${transition}"; must be "manual" or "auto"`
        );
    const handoff = p["handoff"];
    if (handoff !== undefined && handoff !== "merge") {
      throw validationError(
        `Stage "${stageName}" has invalid policy.handoff "${formatRawValue(handoff)}"; must be "merge"`
      );
    }
    return {
      policy: {
        transition: parseTransition(
          p["transition"],
          (transition) =>
            `Stage "${stageName}" has invalid policy.transition "${transition}"; must be "manual" or "auto"`
        ),
        ...(revisionTransition === undefined
          ? {}
          : { revision_transition: revisionTransition }),
        ...(loopTransition === undefined ? {} : { loop_transition: loopTransition }),
        ...(handoff === undefined ? {} : { handoff }),
      },
      legacyContinue: parseLegacyContinueMarker(p["execution"], stageName),
    };
  }

  return {
    policy: {
      transition: parseTransition(
        raw["transition"],
        (transition) =>
          `Stage "${stageName}" has invalid transition "${transition}"; must be "manual" or "auto"`
      ),
    },
    legacyContinue: parseLegacyContinueMarker(raw["mode"], stageName),
  };
}

/**
 * Validate a WorkflowDefinition and return a list of validation error messages.
 * An empty array means the definition is valid.
 */
export function validateWorkflow(def: WorkflowDefinition): string[] {
  const errors: string[] = [];

  if (!def.name || typeof def.name !== "string" || def.name.trim() === "") {
    errors.push("Workflow name is required and must be a non-empty string");
  }

  if (!Array.isArray(def.stages) || def.stages.length === 0) {
    errors.push("Workflow stages is required and must be a non-empty array");
    // Return early — further stage checks are meaningless without stages
    return errors;
  }

  const seenNames = new Set<string>();
  for (const stage of def.stages) {
    if (!stage.name || typeof stage.name !== "string" || stage.name.trim() === "") {
      errors.push("Each stage must have a non-empty string name");
    } else if (seenNames.has(stage.name)) {
      errors.push(`Duplicate stage name: "${stage.name}"`);
    } else {
      seenNames.add(stage.name);
    }

    if (stage.policy?.transition !== "manual" && stage.policy?.transition !== "auto") {
      errors.push(
        `Stage "${stage.name ?? "(unnamed)"}" has invalid policy.transition "${stage.policy?.transition as string}"; must be "manual" or "auto"`
      );
    }

    if (
      stage.policy?.revision_transition !== undefined &&
      stage.policy.revision_transition !== "manual" &&
      stage.policy.revision_transition !== "auto"
    ) {
      errors.push(
        `Stage "${stage.name ?? "(unnamed)"}" has invalid policy.revision_transition "${stage.policy.revision_transition as string}"; must be "manual" or "auto"`
      );
    }

    if (stage.post !== undefined) {
      if (!stage.post.name || typeof stage.post.name !== "string" || stage.post.name.trim() === "") {
        errors.push(`Stage "${stage.name ?? "(unnamed)"}" has a post without a non-empty string name`);
      } else if (seenNames.has(stage.post.name)) {
        errors.push(`Duplicate stage name: "${stage.post.name}"`);
      } else {
        seenNames.add(stage.post.name);
      }
    }

    if (stage.environment !== undefined) {
      const envMap = def.environments ?? {};
      if (!Object.prototype.hasOwnProperty.call(envMap, stage.environment)) {
        errors.push(
          `Stage "${stage.name}" references environment "${stage.environment}" which does not exist in the environments map`
        );
      }
    }
  }

  errors.push(...validateWorkflowRouting(def));

  return errors;
}

const EXIT_NAME = /^[a-z][a-z0-9_-]*$/;

/**
 * The routing contract's own rules, mirroring `validate_workflow_routing` in
 * the server's definitions.rs: the two contracts do not mix, budgets are
 * non-negative, every named-exit stage names its agent, and every loop exit
 * leads to its own stage or an earlier one.
 */
function validateWorkflowRouting(def: WorkflowDefinition): string[] {
  const errors: string[] = [];
  const usesExitFields =
    def.budget !== undefined ||
    def.stages.some(
      (stage) =>
        stage.exits !== undefined ||
        stage.budget !== undefined ||
        stage.policy?.loop_transition !== undefined
    );
  const usesTransitionFields = def.stages.some(
    (stage) => stage.exit_commit === true || stage.setup !== undefined || stage.teardown !== undefined
  );
  const usesHandoff = def.stages.some((stage) => stage.policy?.handoff !== undefined);
  if (def.routing !== "exits") {
    if (usesHandoff) {
      errors.push(
        'policy.handoff belongs to named-exit routing; declare "routing": "exits" to use it (a legacy workflow hands off through its approve post)'
      );
    }
    if (usesExitFields) {
      errors.push(
        'exits, budget and loop_transition belong to named-exit routing; declare "routing": "exits" to use them'
      );
    }
    if (usesTransitionFields) {
      errors.push(
        'exit_commit and stage setup/teardown belong to named-exit routing; declare "routing": "exits" to use them (a legacy workflow commits through a post and runs scripts through its environments)'
      );
    }
    return errors;
  }
  if (def.revision_limit !== undefined) {
    errors.push(
      'routing "exits" budgets each destination stage (budget); revision_limit is the legacy task-wide cap and cannot be combined with it'
    );
  }
  if (def.plan_context !== undefined) {
    errors.push(
      'routing "exits" keeps the plan in the task ledger; plan_context belongs to legacy plan publication'
    );
  }
  def.stages.forEach((stage, index) => {
    if (stage.policy?.revision_transition !== undefined) {
      errors.push(
        `Stage "${stage.name}": routing "exits" uses policy.loop_transition; revision_transition is the legacy revision policy`
      );
    }
    if (stage.agent !== undefined && stage.agent.trim() === "") {
      errors.push(
        `Stage "${stage.name}": agent must name a role; omit it for a stage without a role`
      );
    }
    if (stage.policy?.handoff !== undefined && index !== def.stages.length - 1) {
      errors.push(
        `Stage "${stage.name}": policy.handoff runs as the task leaves its final stage; declare it on the final stage`
      );
    }
    if (stage.exit_commit && stage.post !== undefined) {
      errors.push(
        `Stage "${stage.name}": exit_commit is the commit step of this stage's transition and cannot be combined with a post`
      );
    }
    for (const field of ["setup", "teardown"] as const) {
      if ((stage[field] ?? []).some((command) => command.trim() === "")) {
        errors.push(`Stage "${stage.name}": ${field} commands must not be empty`);
      }
    }
    // A stage with no role enters, runs setup and parks until a person or
    // manager advances it (spec §5); the shapes this build does not run are
    // refused, mirroring the server.
    if (stage.agent === undefined) {
      const refuse = (reason: string) =>
        errors.push(`stage '${stage.name}' has no role, so ${reason}`);
      if (index === 0) {
        refuse("it cannot be the first stage yet: task creation starts the first stage's agent");
      } else if (
        stage.policy?.transition !== "manual" ||
        (stage.policy?.loop_transition !== undefined && stage.policy.loop_transition !== "manual")
      ) {
        refuse("it parks until a person or manager advances it; its transition must be manual");
      } else if (stage.exits !== undefined) {
        refuse("no session can name an exit; it declares none");
      } else if (stage.exit_commit || stage.post !== undefined) {
        refuse("no session can run a commit step or post on its way out");
      } else if (stage.prompt !== undefined || stage.agent_provider !== undefined) {
        refuse("it runs no agent; prompt and agent_provider do not apply");
      }
    }
    for (const [exit, destination] of Object.entries(stage.exits ?? {})) {
      if (!EXIT_NAME.test(exit)) {
        errors.push(
          `Stage "${stage.name}": exit name "${exit}" must be lowercase letters, digits, '_' or '-', starting with a letter`
        );
      } else if (exit === ADVANCE_EXIT) {
        errors.push(
          `Stage "${stage.name}": "${ADVANCE_EXIT}" is every stage's implicit exit to the next stage and cannot be declared`
        );
      } else if (!def.stages.slice(0, index + 1).some((candidate) => candidate.name === destination)) {
        errors.push(
          `Stage "${stage.name}": exit "${exit}" leads to "${destination}", which is not this stage or an earlier stage of the workflow`
        );
      } else if (def.stages.slice(0, index + 1).find((candidate) => candidate.name === destination)?.agent === undefined) {
        errors.push(
          `Stage "${stage.name}": exit "${exit}" leads to "${destination}", a stage without a role; loops into such a stage are not supported yet`
        );
      }
    }
  });
  return errors;
}

/**
 * Keys a routing "exits" document may use at each level: exactly what
 * `.kanna/workflows/schema.json` defines (a parity test holds the two
 * together), plus the legacy stage spellings the loader rewrites. A named-exit
 * workflow opts into a contract whose fields this build either runs or
 * refuses, so an unknown key there is refused rather than silently dropped.
 * Legacy definitions keep their historical tolerance.
 */
export const WORKFLOW_ROOT_KEYS = [
  "$schema", "name", "description", "visibility", "routing", "budget", "plan_context",
  "revision_limit", "environments", "stages",
] as const;
export const WORKFLOW_STAGE_KEYS = [
  "name", "description", "agent", "prompt", "agent_provider", "environment", "exits",
  "budget", "policy", "post", "exit_commit", "setup", "teardown",
] as const;
export const WORKFLOW_POST_KEYS = [
  "name", "description", "agent", "prompt", "agent_provider",
] as const;
const LEGACY_STAGE_KEYS = ["transition", "mode", "post_action"];

function unknownExitWorkflowFields(obj: Record<string, unknown>): string[] {
  const unknown: string[] = [];
  const check = (value: unknown, known: readonly string[], where: string) => {
    if (value === null || typeof value !== "object" || Array.isArray(value)) return;
    for (const key of Object.keys(value)) {
      if (!known.includes(key)) unknown.push(`${where}${key}`);
    }
  };
  check(obj, WORKFLOW_ROOT_KEYS, "");
  if (Array.isArray(obj["stages"])) {
    (obj["stages"] as unknown[]).forEach((stage, index) => {
      check(stage, [...WORKFLOW_STAGE_KEYS, ...LEGACY_STAGE_KEYS], `stages[${index}].`);
      if (stage !== null && typeof stage === "object") {
        check((stage as Record<string, unknown>)["post"], WORKFLOW_POST_KEYS, `stages[${index}].post.`);
      }
    });
  }
  return unknown;
}

function parseBudget(value: unknown, location: string): number | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
    throw validationError(
      `${location} has an invalid budget ${formatRawValue(value)}; must be a non-negative integer`
    );
  }
  return value;
}

/**
 * Parse a raw JSON string into a validated WorkflowDefinition.
 * Throws an Error if the JSON is malformed or validation fails.
 */
export function parseWorkflowJson(raw: string): WorkflowDefinition {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (err) {
    throw new Error(`Invalid JSON: ${err instanceof Error ? err.message : String(err)}`);
  }

  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("Workflow definition must be a JSON object");
  }

  const obj = parsed as Record<string, unknown>;

  // Build a WorkflowDefinition from the raw object, preserving optional fields
  const stages = extractStages(obj);
  const def: WorkflowDefinition = {
    name: typeof obj["name"] === "string" ? obj["name"] : "",
    stages,
  };

  if (typeof obj["description"] === "string") {
    def.description = obj["description"];
  }

  if (obj["revision_limit"] !== undefined && obj["revision_limit"] !== null) {
    const limit = obj["revision_limit"];
    if (typeof limit !== "number" || !Number.isInteger(limit) || limit < 0) {
      throw validationError(
        `Workflow "${def.name}" has an invalid revision_limit ${formatRawValue(limit)}; must be a non-negative integer`
      );
    }
    def.revision_limit = limit;
  }

  // Preserved rather than parsed from a file: only the server stamps it, but
  // a pinned definition round-tripped through this loader must not silently
  // drop the plan the task's later stages were published under.
  const planContext = obj["plan_context"];
  if (planContext !== undefined && planContext !== null) {
    if (
      typeof planContext !== "object" ||
      Array.isArray(planContext) ||
      typeof (planContext as Record<string, unknown>)["source_run_id"] !== "string" ||
      typeof (planContext as Record<string, unknown>)["stage"] !== "string" ||
      typeof (planContext as Record<string, unknown>)["result"] !== "string"
    ) {
      throw validationError(
        `Workflow "${def.name}" has an invalid plan_context; Kanna stamps it when a plan stage publishes its remaining stages and it is not authored by hand`
      );
    }
    def.plan_context = planContext as WorkflowPlanContext;
  }

  const routing = obj["routing"];
  if (routing !== undefined && routing !== null) {
    if (routing !== "legacy" && routing !== "exits") {
      throw validationError(
        `Workflow "${def.name}" has an invalid routing ${formatRawValue(routing)}; must be "legacy" or "exits"`
      );
    }
    def.routing = routing;
  }
  const budget = parseBudget(obj["budget"], `Workflow "${def.name}"`);
  if (budget !== undefined) {
    def.budget = budget;
  }
  if (def.routing === "exits") {
    const unknown = unknownExitWorkflowFields(obj);
    if (unknown.length > 0) {
      throw validationError(
        `Workflow "${def.name}" uses routing "exits" with fields this version does not support: ${unknown.join(", ")}; remove them rather than rely on them being ignored`
      );
    }
  }

  if (obj["environments"] !== undefined && obj["environments"] !== null) {
    if (typeof obj["environments"] === "object" && !Array.isArray(obj["environments"])) {
      def.environments = obj["environments"] as Record<string, { setup?: string[]; teardown?: string[] }>;
    }
  }

  const errors = validateWorkflow(def);
  if (errors.length > 0) {
    throw new Error(`Workflow validation failed:\n${errors.map((e) => `  - ${e}`).join("\n")}`);
  }

  return def;
}

function extractPost(value: unknown, stageName: string): WorkflowPost | undefined {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return undefined;
  }

  const raw = value as Record<string, unknown>;
  const name = typeof raw["name"] === "string" ? raw["name"] : "";
  if (!name) {
    throw validationError(`Stage "${stageName}" has a post without a non-empty string name`);
  }
  const post: WorkflowPost = { name };

  if (typeof raw["description"] === "string") {
    post.description = raw["description"];
  }
  if (typeof raw["agent"] === "string") {
    post.agent = raw["agent"];
  }
  if (typeof raw["prompt"] === "string") {
    post.prompt = raw["prompt"];
  }
  const agentProvider = parseAgentProviderSelection(
    raw["agent_provider"],
    `Post "${name}" on stage "${stageName}"`,
  );
  if (agentProvider !== undefined) {
    post.agent_provider = agentProvider;
  }

  return post;
}

function extractStages(obj: Record<string, unknown>): WorkflowStage[] {
  if (!Array.isArray(obj["stages"])) {
    return [];
  }

  const stages: WorkflowStage[] = [];
  for (const [index, item] of (obj["stages"] as unknown[]).entries()) {
    if (item === null || typeof item !== "object" || Array.isArray(item)) {
      throw new Error(`Stage at index ${index} must be an object`);
    }
    const s = item as Record<string, unknown>;
    const name = typeof s["name"] === "string" ? s["name"] : "";
    const { policy, legacyContinue } = parseStagePolicy(s, name || "(unnamed)");

    // Legacy interleaved continue stage (old post_action compilation or an
    // `execution: "continue"` policy, including pinned pipeline_def
    // snapshots): fold into the preceding stage's post.
    if (legacyContinue) {
      const previous = stages[stages.length - 1];
      if (previous && previous.post === undefined) {
        const folded: WorkflowPost = { name };
        if (typeof s["description"] === "string") folded.description = s["description"];
        if (typeof s["agent"] === "string") folded.agent = s["agent"];
        if (typeof s["prompt"] === "string") folded.prompt = s["prompt"];
        const agentProvider = parseAgentProviderSelection(
          s["agent_provider"],
          `Stage "${name || "(unnamed)"}"`,
        );
        if (agentProvider !== undefined) {
          folded.agent_provider = agentProvider;
        }
        previous.post = folded;
        continue;
      }
    }

    const stage: WorkflowStage = {
      name,
      policy,
    };

    if (typeof s["description"] === "string") {
      stage.description = s["description"];
    }
    if (typeof s["agent"] === "string") {
      stage.agent = s["agent"];
    }
    if (typeof s["prompt"] === "string") {
      stage.prompt = s["prompt"];
    }
    const agentProvider = parseAgentProviderSelection(
      s["agent_provider"],
      `Stage "${name || "(unnamed)"}"`,
    );
    if (agentProvider !== undefined) {
      stage.agent_provider = agentProvider;
    }
    if (typeof s["environment"] === "string") {
      stage.environment = s["environment"];
    }
    const exits = s["exits"];
    if (exits !== undefined && exits !== null) {
      if (
        typeof exits !== "object" ||
        Array.isArray(exits) ||
        Object.values(exits).some((destination) => typeof destination !== "string" || destination === "")
      ) {
        throw validationError(
          `Stage "${name || "(unnamed)"}" has invalid exits ${formatRawValue(exits)}; must map exit names to stage names`
        );
      }
      stage.exits = { ...(exits as Record<string, string>) };
    }
    const stageBudget = parseBudget(s["budget"], `Stage "${name || "(unnamed)"}"`);
    if (stageBudget !== undefined) {
      stage.budget = stageBudget;
    }
    if (s["exit_commit"] !== undefined && s["exit_commit"] !== null) {
      if (typeof s["exit_commit"] !== "boolean") {
        throw validationError(
          `Stage "${name || "(unnamed)"}" has an invalid exit_commit ${formatRawValue(s["exit_commit"])}; must be true or false`
        );
      }
      stage.exit_commit = s["exit_commit"];
    }
    for (const field of ["setup", "teardown"] as const) {
      const commands = s[field];
      if (commands === undefined || commands === null) continue;
      if (!Array.isArray(commands) || commands.some((command) => typeof command !== "string")) {
        throw validationError(
          `Stage "${name || "(unnamed)"}" has an invalid ${field} ${formatRawValue(commands)}; must be an array of commands`
        );
      }
      stage[field] = [...(commands as string[])];
    }

    const post = extractPost(s["post"], stage.name || "(unnamed)");
    if (post) {
      stage.post = post;
    } else {
      // Legacy `post_action` declarations become the stage's post directly.
      const legacyPost = extractPost(s["post_action"], stage.name || "(unnamed)");
      if (legacyPost) {
        stage.post = legacyPost;
      }
    }

    stages.push(stage);
  }

  return stages;
}
