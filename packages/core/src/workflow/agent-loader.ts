import type { AgentDefinition, AgentExtension } from "./workflow-types";
import { parseFrontmatter } from "../config/custom-tasks";
import {
  VALID_AGENT_PROVIDERS,
  isAgentProvider,
  splitAgentProviderValue,
  parseAgentSelection,
  resolveAgentSelectionEntry,
  validateSelectionSiblings,
  type AgentSelectionEntry,
} from "../config/agent-providers";

const VALID_PERMISSION_MODES = ["default", "acceptEdits", "dontAsk"] as const;
type PermissionMode = AgentDefinition["permission_mode"];

function parsePermissionMode(value: unknown): PermissionMode | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  if ((VALID_PERMISSION_MODES as readonly string[]).includes(value)) {
    return value as PermissionMode;
  }

  throw new Error(
    `permission_mode must be one of: ${VALID_PERMISSION_MODES.join(", ")} (got "${value}")`
  );
}

function parseAgentProviders(value: unknown): AgentSelectionEntry[] {
  if (typeof value === "object" && value !== null && (!Array.isArray(value) || value.some(v => typeof v === "object"))) return parseAgentSelection(value, false);
  if (
    typeof value !== "string" &&
    !(Array.isArray(value) && value.every((provider) => typeof provider === "string"))
  ) {
    throw new Error("agent_provider must be a string or an array of strings");
  }

  const providers = splitAgentProviderValue(value);
  if (providers.length === 0) {
    throw new Error("agent_provider must include at least one non-empty provider");
  }

  const invalid = providers.filter((provider) => !isAgentProvider(provider));
  if (invalid.length > 0) {
    throw new Error(
      `agent_provider must be one of: ${VALID_AGENT_PROVIDERS.join(", ")} (got "${invalid.join(", ")}")`,
    );
  }

  return providers.filter(isAgentProvider);
}

// An agent definition (`.kanna/agents/*/AGENT.md`) describes a reusable *role* and
// intentionally supports a focused field set: name, description, prompt (body),
// model, effort, permission_mode, allowed_tools, and agent_provider. Per-task execution
// limits (execution_mode, max_turns, max_budget_usd, disallowed_tools) and
// worktree setup/teardown live on task templates (`.kanna/tasks/*/agent.md`, parsed
// by parseAgentMd), not on agent definitions. Keep this boundary in mind before
// widening the agent schema.
export function parseAgentDefinition(content: string): AgentDefinition {
  const { frontmatter, body } = parseFrontmatter(content);

  const fm: Record<string, unknown> = frontmatter ?? {};
  const prompt = body.trim();

  // `role`/`providers` are the definition-formula aliases (spec §12) for
  // `description`/`agent_provider`; declaring either opts the definition into
  // the formula's line-count and four-section shape (checkDefinitionFormula).
  const usesFormula = fm.role !== undefined || fm.providers !== undefined;
  const description = fm.description ?? fm.role;
  const providerValue = fm.agent_provider ?? fm.providers;

  const def: AgentDefinition = {
    name: typeof fm.name === "string" ? fm.name : "",
    description: typeof description === "string" ? description : "",
    prompt,
  };

  if (typeof fm.model === "string") {
    def.model = fm.model;
  }
  if (typeof fm.effort === "string") {
    def.effort = fm.effort;
  }

  const permissionMode = parsePermissionMode(fm.permission_mode);
  if (permissionMode !== undefined) {
    def.permission_mode = permissionMode;
  }

  if (Array.isArray(fm.allowed_tools) && fm.allowed_tools.every((t: unknown) => typeof t === "string")) {
    def.allowed_tools = fm.allowed_tools as string[];
  }

  // agent_provider/providers: YAML array, single string, or comma-separated string.
  if (providerValue !== undefined) {
    const agentProviders = parseAgentProviders(providerValue);
    def.agent_provider = agentProviders;
  }

  const errors = validateAgentDefinition(def);
  if (errors.length > 0) {
    throw new Error(`Invalid AGENT.md: ${errors.join("; ")}`);
  }

  if (usesFormula) {
    const formulaErrors = checkDefinitionFormula(content);
    if (formulaErrors.length > 0) {
      throw new Error(`Invalid AGENT.md: ${formulaErrors.join("; ")}`);
    }
  }

  return def;
}

/**
 * Spec §12's definition formula: a definition that opts in (by declaring
 * `role` or `providers` in its frontmatter) must resolve to 15-40 lines
 * total and carry these four section headers, in order, in its body. Mirrors
 * `check_definition_formula` in the server's `definitions.rs`.
 */
const DEFINITION_FORMULA_SECTIONS = ["## Produces", "## Reads", "## Must not", "## Stop when"] as const;
const DEFINITION_FORMULA_RESULT_VARS = ["$PREV_RESULT", "$PREV_MAIN_RESULT", "$PLAN_RESULT"] as const;

export function checkDefinitionFormula(content: string): string[] {
  const errors: string[] = [];
  const lineCount = content.replace(/\n+$/, "").split("\n").length;
  if (lineCount < 15 || lineCount > 40) {
    errors.push(`definition-formula definitions must be 15-40 lines, got ${lineCount}`);
  }

  let searchFrom = 0;
  for (const section of DEFINITION_FORMULA_SECTIONS) {
    const offset = content.indexOf(section, searchFrom);
    if (offset === -1) {
      errors.push(
        `definition-formula definitions require the section "${section}", in order after ${JSON.stringify(DEFINITION_FORMULA_SECTIONS)}`
      );
      break;
    }
    searchFrom = offset + section.length;
  }

  for (const variable of DEFINITION_FORMULA_RESULT_VARS) {
    if (content.includes(variable)) {
      errors.push(
        `definition-formula definitions must not reference the legacy result variable ${variable}; the engine delivers results through the ledger`
      );
    }
  }

  return errors;
}

// An extension (`.kanna/agents/{name}/EXTEND.md`) customizes the resolved
// agent — repo override or built-in — without a total rewrite: its body is
// appended to the base prompt and its frontmatter fields replace the base's.
// Frontmatter is optional; a plain markdown file is a pure prompt extension.
export function parseAgentExtension(content: string): AgentExtension {
  const { frontmatter, body } = parseFrontmatter(content);

  const fm: Record<string, unknown> = frontmatter ?? {};
  const ext: AgentExtension = { prompt: body.trim() };

  const description = fm.description ?? fm.role;
  if (typeof description === "string") {
    ext.description = description;
  }

  if (typeof fm.model === "string") {
    ext.model = fm.model;
  }
  if (typeof fm.effort === "string") {
    ext.effort = fm.effort;
  }

  const permissionMode = parsePermissionMode(fm.permission_mode);
  if (permissionMode !== undefined) {
    ext.permission_mode = permissionMode;
  }

  if (Array.isArray(fm.allowed_tools) && fm.allowed_tools.every((t: unknown) => typeof t === "string")) {
    ext.allowed_tools = fm.allowed_tools as string[];
  }

  const providerValue = fm.agent_provider ?? fm.providers;
  if (providerValue !== undefined) {
    const agentProviders = parseAgentProviders(providerValue);
    ext.agent_provider = agentProviders;
  }

  if (ext.agent_provider) validateSelectionSiblings(parseAgentSelection(ext.agent_provider, false), ext.model, ext.effort);
  return ext;
}

export function applyAgentExtension(base: AgentDefinition, extension: AgentExtension): AgentDefinition {
  if (base.agent_provider && extension.agent_provider) {
    const previous = parseAgentSelection(base.agent_provider, false);
    const replacement = parseAgentSelection(extension.agent_provider, false);
    const usesObjects = [...previous, ...replacement].some(entry => typeof entry === "object");
    const owner = resolveAgentSelectionEntry(previous[0]!, false)?.provider;
    const nextOwner = resolveAgentSelectionEntry(replacement[0]!, false)?.provider;
    const inheritsTuning = (base.model !== undefined && extension.model === undefined)
      || (base.effort !== undefined && extension.effort === undefined);
    if (usesObjects && inheritsTuning && owner !== undefined && owner !== nextOwner) {
      throw new Error("conflicting selection representations: EXTEND changes harness while inheriting sibling model/effort written for another harness");
    }
  }
  const merged: AgentDefinition = {
    ...base,
    ...(extension.description !== undefined && { description: extension.description }),
    ...(extension.model !== undefined && { model: extension.model }),
    ...(extension.effort !== undefined && { effort: extension.effort }),
    ...(extension.permission_mode !== undefined && { permission_mode: extension.permission_mode }),
    ...(extension.allowed_tools !== undefined && { allowed_tools: extension.allowed_tools }),
    ...(extension.agent_provider !== undefined && { agent_provider: extension.agent_provider }),
  };

  if (extension.prompt !== "") {
    merged.prompt = base.prompt === "" ? extension.prompt : `${base.prompt}\n\n${extension.prompt}`;
  }

  const errors = validateAgentDefinition(merged);
  if (errors.length > 0) {
    throw new Error(`Invalid extended agent: ${errors.join("; ")}`);
  }

  return merged;
}

export function validateAgentDefinition(def: AgentDefinition): string[] {
  const errors: string[] = [];

  if (typeof def.name !== "string" || def.name.trim() === "") {
    errors.push("name is required and must be a non-empty string");
  }

  if (typeof def.description !== "string" || def.description.trim() === "") {
    errors.push("description is required and must be a non-empty string");
  }

  if (def.prompt !== undefined && typeof def.prompt !== "string") {
    errors.push("prompt (AGENT.md body) must be a string");
  }

  if (
    def.permission_mode !== undefined &&
    !(VALID_PERMISSION_MODES as readonly string[]).includes(def.permission_mode)
  ) {
    errors.push(
      `permission_mode must be one of: ${VALID_PERMISSION_MODES.join(", ")} (got "${def.permission_mode}")`
    );
  }

  if (def.agent_provider !== undefined) {
    try {
      const entries = parseAgentSelection(def.agent_provider, false);
      validateSelectionSiblings(entries, def.model, def.effort);
    } catch (error) { errors.push(String(error)); }
  }

  return errors;
}
