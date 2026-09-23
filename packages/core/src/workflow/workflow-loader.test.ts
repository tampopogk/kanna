import { readFileSync } from "node:fs";
import { describe, it, expect } from "vitest";
import {
  parseWorkflowJson,
  validateWorkflow,
  WORKFLOW_POST_KEYS,
  WORKFLOW_ROOT_KEYS,
  WORKFLOW_STAGE_KEYS,
} from "./workflow-loader";

describe("parseWorkflowJson", () => {
  it("parses valid workflow JSON", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        { name: "Stage 1", transition: "manual" },
        { name: "Stage 2", transition: "auto" },
      ],
    });
    const result = parseWorkflowJson(json);
    expect(result.name).toBe("My Workflow");
    expect(result.stages).toHaveLength(2);
    expect(result.stages[0].name).toBe("Stage 1");
    expect(result.stages[1].policy.transition).toBe("auto");
  });

  it("rejects missing name", () => {
    const json = JSON.stringify({
      stages: [{ name: "Stage 1", transition: "manual" }],
    });
    expect(() => parseWorkflowJson(json)).toThrow();
  });

  it("rejects empty stages array", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [],
    });
    expect(() => parseWorkflowJson(json)).toThrow();
  });

  it("rejects duplicate stage names", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        { name: "Stage 1", transition: "manual" },
        { name: "Stage 1", transition: "auto" },
      ],
    });
    expect(() => parseWorkflowJson(json)).toThrow();
  });

  it("rejects invalid transition value", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1", transition: "invalid" }],
    });
    expect(() => parseWorkflowJson(json)).toThrow();
  });

  it("parses an optional revision transition", () => {
    const result = parseWorkflowJson(JSON.stringify({
      name: "Revision loop",
      stages: [{
        name: "in progress",
        policy: { transition: "manual", revision_transition: "auto" },
      }],
    }));

    expect(result.stages[0].policy).toEqual({
      transition: "manual",
      revision_transition: "auto",
    });
  });

  it("leaves revision transition absent for existing policies", () => {
    const result = parseWorkflowJson(JSON.stringify({
      name: "Existing",
      stages: [{ name: "in progress", policy: { transition: "manual" } }],
    }));

    expect(result.stages[0].policy).toEqual({ transition: "manual" });
  });

  it("rejects an invalid revision transition", () => {
    const json = JSON.stringify({
      name: "Invalid",
      stages: [{
        name: "in progress",
        policy: { transition: "manual", revision_transition: "sometimes" },
      }],
    });

    expect(() => parseWorkflowJson(json)).toThrow(
      /invalid policy\.revision_transition "sometimes"; must be "manual" or "auto"/
    );
  });

  it("reports missing transition as undefined instead of an empty string", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1" }],
    });

    expect(() => parseWorkflowJson(json)).toThrow(/invalid transition "undefined"/);
  });

  it("validates environment references exist", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1", transition: "manual", environment: "nonexistent" }],
    });
    expect(() => parseWorkflowJson(json)).toThrow();
  });

  it("accepts workflow with environments", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      environments: {
        production: { setup: ["echo setup"], teardown: ["echo teardown"] },
      },
      stages: [{ name: "Stage 1", transition: "manual", environment: "production" }],
    });
    const result = parseWorkflowJson(json);
    expect(result.environments?.["production"]).toBeDefined();
    expect(result.stages[0].environment).toBe("production");
  });

  it("accepts stage with optional fields omitted", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1", transition: "auto" }],
    });
    const result = parseWorkflowJson(json);
    const stage = result.stages[0];
    expect(stage.description).toBeUndefined();
    expect(stage.agent).toBeUndefined();
    expect(stage.prompt).toBeUndefined();
    expect(stage.agent_provider).toBeUndefined();
    expect(stage.environment).toBeUndefined();
  });

  it("parses a single-string stage agent_provider", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1", transition: "auto", agent_provider: "codex" }],
    });
    const result = parseWorkflowJson(json);
    expect(result.stages[0].agent_provider).toBe("codex");
  });

  it("parses a stage agent_provider array", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1", transition: "auto", agent_provider: ["codex", "claude"] }],
    });
    const result = parseWorkflowJson(json);
    expect(result.stages[0].agent_provider).toEqual(["codex", "claude"]);
  });

  it("parses compact provider selectors on stages and posts", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        {
          name: "Stage 1",
          transition: "auto",
          agent_provider: ["claude-fable-hi", "codex-gpt-6-astra-lo"],
          post: { name: "commit", agent_provider: "claude-haiku" },
        },
      ],
    });
    const result = parseWorkflowJson(json);
    // Selectors keep their written form; the server derives each candidate's
    // model/effort at spawn time.
    expect(result.stages[0].agent_provider).toEqual([
      "claude-fable-hi",
      "codex-gpt-6-astra-lo",
    ]);
    expect(result.stages[0].post?.agent_provider).toBe("claude-haiku");
  });

  it("rejects a selector whose provider segment is unknown", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        { name: "Stage 1", transition: "auto", agent_provider: "clod-fable" },
      ],
    });
    expect(() => parseWorkflowJson(json)).toThrow(
      /Stage "Stage 1" has unsupported agent_provider values: clod-fable/,
    );
  });

  it("rejects a stage agent_provider array containing non-strings", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1", transition: "auto", agent_provider: ["codex", 3] }],
    });
    expect(() => parseWorkflowJson(json)).toThrow(
      /Stage "Stage 1" has an invalid agent_provider value/,
    );
  });

  it("rejects an unknown stage agent_provider", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1", transition: "auto", agent_provider: "future-agent" }],
    });
    expect(() => parseWorkflowJson(json)).toThrow(
      /Stage "Stage 1" has unsupported agent_provider values: future-agent/,
    );
  });

  it("rejects an unknown post agent_provider", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{
        name: "in progress",
        transition: "manual",
        post: { name: "commit", agent_provider: "future-agent" },
      }],
    });
    expect(() => parseWorkflowJson(json)).toThrow(
      /Post "commit" on stage "in progress" has unsupported agent_provider values: future-agent/,
    );
  });

  it("rejects a post agent_provider array containing non-strings", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{
        name: "in progress",
        transition: "manual",
        post: { name: "commit", agent_provider: ["codex", 3] },
      }],
    });
    expect(() => parseWorkflowJson(json)).toThrow(
      /Post "commit" on stage "in progress" has an invalid agent_provider value/,
    );
  });

  it("rejects an unknown agent_provider on a folded legacy post", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        { name: "in progress", transition: "manual" },
        {
          name: "commit",
          transition: "auto",
          mode: "continue",
          agent_provider: "future-agent",
        },
      ],
    });
    expect(() => parseWorkflowJson(json)).toThrow(
      /Stage "commit" has unsupported agent_provider values: future-agent/,
    );
  });

  it("rejects a mixed agent_provider array on a folded legacy post", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        { name: "in progress", transition: "manual" },
        {
          name: "commit",
          transition: "auto",
          mode: "continue",
          agent_provider: ["codex", 3],
        },
      ],
    });
    expect(() => parseWorkflowJson(json)).toThrow(
      /Stage "commit" has an invalid agent_provider value/,
    );
  });

  it("drops legacy follow_task values", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Stage 1", transition: "manual", follow_task: "nope" }],
    });

    const result = parseWorkflowJson(json);

    expect("follow_task" in result.stages[0]).toBe(false);
  });

  it("folds a legacy continue-mode stage into the preceding stage's post", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        { name: "in progress", transition: "manual", agent: "implement" },
        { name: "Commit", transition: "auto", mode: "continue", agent: "commit", prompt: "Commit it" },
        { name: "pr", transition: "manual" },
      ],
    });

    const result = parseWorkflowJson(json);

    expect(result.stages.map((stage) => stage.name)).toEqual(["in progress", "pr"]);
    expect(result.stages[0].post).toEqual({
      name: "Commit",
      agent: "commit",
      prompt: "Commit it",
    });
  });

  it("keeps a first-stage legacy continue marker as a normal stage", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Commit", transition: "auto", mode: "continue" }],
    });

    const result = parseWorkflowJson(json);

    expect(result.stages.map((stage) => stage.name)).toEqual(["Commit"]);
    expect(result.stages[0].post).toBeUndefined();
  });

  it("ignores non-string mode values", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Commit", transition: "auto", mode: true }],
    });

    const result = parseWorkflowJson(json);

    expect(result.stages[0].post).toBeUndefined();
  });

  it("rejects invalid legacy stage mode values", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "Commit", transition: "auto", mode: "sideways" }],
    });

    expect(() => parseWorkflowJson(json)).toThrow(/invalid execution "sideways"/);
  });

  it("compiles a legacy stage post_action into the stage's post", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        {
          name: "in progress",
          transition: "manual",
          post_action: {
            name: "commit",
            description: "Commit the relevant work",
            agent: "commit",
            prompt: "Commit $TASK_PROMPT",
            agent_provider: ["codex", "claude"],
            transition: "auto",
          },
        },
        { name: "pr", transition: "manual" },
      ],
    });

    const result = parseWorkflowJson(json);

    expect(result.stages.map((stage) => stage.name)).toEqual(["in progress", "pr"]);
    expect(result.stages[0].post).toEqual({
      name: "commit",
      description: "Commit the relevant work",
      agent: "commit",
      prompt: "Commit $TASK_PROMPT",
      agent_provider: ["codex", "claude"],
    });
    expect("post_action" in result.stages[0]).toBe(false);
  });

  it("accepts a legacy post_action without a transition", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        {
          name: "in progress",
          transition: "manual",
          post_action: {
            name: "commit",
          },
        },
      ],
    });

    const result = parseWorkflowJson(json);

    expect(result.stages[0].post).toEqual({ name: "commit" });
  });

  it("drops non-object legacy post_action values", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [{ name: "in progress", transition: "manual", post_action: "commit" }],
    });

    const result = parseWorkflowJson(json);

    expect("post_action" in result.stages[0]).toBe(false);
  });

  it("folds pinned snapshot continue stages into posts (legacy pipeline_def compatibility)", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        {
          name: "in progress",
          agent: "implement",
          prompt: "$TASK_PROMPT",
          policy: { transition: "manual" },
        },
        {
          name: "commit",
          agent: "commit",
          prompt: "Commit $TASK_PROMPT",
          policy: { transition: "auto", execution: "continue" },
        },
        {
          name: "pr",
          agent: "pr",
          prompt: "Create PR",
          policy: { transition: "manual" },
        },
      ],
    });

    const result = parseWorkflowJson(json);

    expect(result.stages.map((stage) => ({
      name: stage.name,
      policy: stage.policy,
    }))).toEqual([
      { name: "in progress", policy: { transition: "manual" } },
      { name: "pr", policy: { transition: "manual" } },
    ]);
    expect(result.stages[0].post).toEqual({
      name: "commit",
      agent: "commit",
      prompt: "Commit $TASK_PROMPT",
    });
  });

  it("parses a stage post declaration", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        {
          name: "in progress",
          agent: "implement",
          prompt: "$TASK_PROMPT",
          policy: { transition: "manual" },
          post: {
            name: "commit",
            agent: "commit",
            prompt: "Commit $TASK_PROMPT",
          },
        },
        { name: "pr", policy: { transition: "manual" } },
      ],
    });

    const result = parseWorkflowJson(json);

    expect(result.stages.map((stage) => stage.name)).toEqual(["in progress", "pr"]);
    expect(result.stages[0].post).toEqual({
      name: "commit",
      agent: "commit",
      prompt: "Commit $TASK_PROMPT",
    });
  });

  it("prefers an explicit post over a legacy post_action on the same stage", () => {
    const json = JSON.stringify({
      name: "My Workflow",
      stages: [
        {
          name: "in progress",
          agent: "implement",
          prompt: "$TASK_PROMPT",
          transition: "manual",
          post: { name: "commit", agent: "commit", prompt: "Commit new" },
          post_action: {
            name: "legacy-commit",
            agent: "commit",
            prompt: "Commit legacy",
            transition: "auto",
          },
        },
        { name: "pr", agent: "pr", transition: "manual" },
      ],
    });

    const result = parseWorkflowJson(json);

    expect(result.stages[0].post).toEqual({
      name: "commit",
      agent: "commit",
      prompt: "Commit new",
    });
  });

  it("preserves a revision_limit and leaves it unset when omitted", () => {
    const stages = [{ name: "in progress", policy: { transition: "manual" } }];

    expect(parseWorkflowJson(JSON.stringify({ name: "p", stages })).revision_limit).toBeUndefined();
    expect(
      parseWorkflowJson(JSON.stringify({ name: "p", revision_limit: 2, stages })).revision_limit
    ).toBe(2);
    // 0 is a meaningful value (no cap), not an absent one.
    expect(
      parseWorkflowJson(JSON.stringify({ name: "p", revision_limit: 0, stages })).revision_limit
    ).toBe(0);
  });

  it("rejects a revision_limit that is not a non-negative integer", () => {
    const stages = [{ name: "in progress", policy: { transition: "manual" } }];

    for (const limit of [-1, 1.5, "3", true]) {
      expect(() =>
        parseWorkflowJson(JSON.stringify({ name: "p", revision_limit: limit, stages }))
      ).toThrow(/revision_limit/);
    }
  });
});

describe("validateWorkflow", () => {
  it("returns empty array for valid workflow", () => {
    const workflow = {
      name: "Valid Workflow",
      stages: [{ name: "Stage 1", policy: { transition: "manual" as const } }],
    };
    expect(validateWorkflow(workflow)).toEqual([]);
  });

  it("returns error for missing name", () => {
    const workflow = {
      name: "",
      stages: [{ name: "Stage 1", policy: { transition: "manual" as const } }],
    };
    const errors = validateWorkflow(workflow);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.includes("name"))).toBe(true);
  });

  it("returns error for empty stages", () => {
    const workflow = {
      name: "Workflow",
      stages: [],
    };
    const errors = validateWorkflow(workflow);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.includes("stage"))).toBe(true);
  });

  it("returns error for duplicate stage names", () => {
    const workflow = {
      name: "Workflow",
      stages: [
        { name: "Dup", policy: { transition: "manual" as const } },
        { name: "Dup", policy: { transition: "auto" as const } },
      ],
    };
    const errors = validateWorkflow(workflow);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.includes("duplicate") || e.includes("Dup"))).toBe(true);
  });

  it("returns error for invalid transition", () => {
    const workflow = {
      name: "Workflow",
      stages: [{ name: "Stage 1", policy: { transition: "bad" as "manual" | "auto" } }],
    };
    const errors = validateWorkflow(workflow);
    expect(errors.length).toBeGreaterThan(0);
  });

  it("returns error for invalid revision transition", () => {
    const workflow = {
      name: "Workflow",
      stages: [{
        name: "Stage 1",
        policy: {
          transition: "manual" as const,
          revision_transition: "bad" as "manual" | "auto",
        },
      }],
    };
    const errors = validateWorkflow(workflow);
    expect(errors.some((error) => error.includes("invalid policy.revision_transition"))).toBe(true);
  });

  it("returns error for a post whose name collides with a stage name", () => {
    const workflow = {
      name: "Workflow",
      stages: [
        {
          name: "in progress",
          policy: { transition: "manual" as const },
          post: { name: "pr" },
        },
        { name: "pr", policy: { transition: "manual" as const } },
      ],
    };

    const errors = validateWorkflow(workflow);

    expect(errors.some((error) => error.includes("Duplicate stage name"))).toBe(true);
  });

  it("returns error for undefined environment reference", () => {
    const workflow = {
      name: "Workflow",
      stages: [{ name: "Stage 1", policy: { transition: "manual" as const }, environment: "missing" }],
    };
    const errors = validateWorkflow(workflow);
    expect(errors.length).toBeGreaterThan(0);
    expect(errors.some((e) => e.includes("missing") || e.includes("environment"))).toBe(true);
  });

  it("returns multiple errors when multiple issues exist", () => {
    const workflow = {
      name: "",
      stages: [],
    };
    const errors = validateWorkflow(workflow);
    expect(errors.length).toBeGreaterThanOrEqual(2);
  });
});

describe("plan context", () => {
  const base = {
    name: "grown",
    stages: [{ name: "plan", policy: { transition: "manual" } }],
  };

  it("preserves a stamped plan context through the loader", () => {
    const def = parseWorkflowJson(
      JSON.stringify({
        ...base,
        plan_context: { source_run_id: "run-1", stage: "plan", result: '{"status":"success"}' },
      })
    );

    expect(def.plan_context).toEqual({
      source_run_id: "run-1",
      stage: "plan",
      result: '{"status":"success"}',
    });
  });

  it("leaves a workflow without one alone", () => {
    expect(parseWorkflowJson(JSON.stringify(base)).plan_context).toBeUndefined();
  });

  it("rejects a malformed plan context rather than dropping it", () => {
    expect(() =>
      parseWorkflowJson(JSON.stringify({ ...base, plan_context: { source_run_id: "run-1" } }))
    ).toThrow(/invalid plan_context/);
  });
});

describe("named-exit routing (parity with the server loader)", () => {
  interface RoutingFixture {
    name: string;
    valid: boolean;
    rejects?: string;
    definition: unknown;
  }
  const fixtures = (
    JSON.parse(readFileSync(new URL("./routing-fixtures.json", import.meta.url), "utf8")) as {
      cases: RoutingFixture[];
    }
  ).cases;

  for (const fixture of fixtures) {
    it(`${fixture.valid ? "accepts" : "refuses"} ${fixture.name}`, () => {
      const parse = () => parseWorkflowJson(JSON.stringify(fixture.definition));
      if (fixture.valid) {
        expect(parse).not.toThrow();
      } else {
        expect(parse).toThrow(fixture.rejects);
      }
    });
  }

  it("keeps the named-exit fields a routed workflow declares", () => {
    const parsed = parseWorkflowJson(JSON.stringify(fixtures[0].definition));
    expect(parsed.routing).toBe("exits");
    expect(parsed.budget).toBe(4);
    const review = parsed.stages.find((stage) => stage.name === "review");
    expect(review?.exits).toEqual({ revise: "in progress", replan: "plan" });
    expect(review?.budget).toBe(3);
    expect(parsed.stages.find((stage) => stage.name === "in progress")?.policy).toEqual({
      transition: "auto",
      loop_transition: "auto",
    });
  });

  it("names exactly the keys the bundled schema defines", () => {
    const schema = JSON.parse(
      readFileSync(new URL("../../../../.kanna/workflows/schema.json", import.meta.url), "utf8"),
    );
    const keys = (node: { properties: Record<string, unknown> }) => Object.keys(node.properties).sort();
    expect([...WORKFLOW_ROOT_KEYS].sort()).toEqual(keys(schema));
    expect([...WORKFLOW_STAGE_KEYS].sort()).toEqual(keys(schema.properties.stages.items));
    expect([...WORKFLOW_POST_KEYS].sort()).toEqual(keys(schema.properties.stages.items.properties.post));
  });

  it("loads the release workflow the merge master runs, as the server does", () => {
    // task_creator/tests/stage.rs asserts the same shape through the server
    // loader.
    const release = parseWorkflowJson(
      readFileSync(new URL("../../../../.kanna/workflows/release.json", import.meta.url), "utf8"),
    );
    expect(release.name).toBe("release");
    expect(release.routing).toBe("exits");
    expect(
      release.stages.map((stage) => [stage.name, stage.agent ?? null, stage.policy?.transition]),
    ).toEqual([
      ["in progress", "merge", "manual"],
      ["qa gauntlet", null, "manual"],
      ["ship staging", "ship", "manual"],
      ["soak", null, "manual"],
      ["ship production", "ship", "manual"],
    ]);
    expect(release.stages[0].prompt).toBe("$TASK_PROMPT");
    expect(release.stages.some((stage) => stage.policy?.handoff !== undefined)).toBe(false);
  });

  it("loads the T10d intake lineup (shaped, planned, designed) as the server does", () => {
    // Loader parity with task_creator/tests/stage.rs's
    // builtin_{shaped,planned,designed}_workflow_* assertions.
    const shaped = parseWorkflowJson(
      readFileSync(new URL("../../../../.kanna/workflows/shaped.json", import.meta.url), "utf8"),
    );
    expect(shaped.routing).toBe("exits");
    expect(shaped.stages.map((stage) => stage.name)).toEqual(["in progress", "review", "pr"]);
    expect(shaped.stages.find((stage) => stage.name === "review")?.exits).toEqual({
      revise: "in progress",
    });
    expect(shaped.stages.find((stage) => stage.name === "pr")?.policy.handoff).toBe("merge");

    const planned = parseWorkflowJson(
      readFileSync(new URL("../../../../.kanna/workflows/planned.json", import.meta.url), "utf8"),
    );
    expect(planned.stages.map((stage) => stage.name)).toEqual(["plan", "in progress", "review", "pr"]);
    expect(planned.stages.find((stage) => stage.name === "review")?.exits).toEqual({
      revise: "in progress",
      replan: "plan",
    });

    const designed = parseWorkflowJson(
      readFileSync(new URL("../../../../.kanna/workflows/designed.json", import.meta.url), "utf8"),
    );
    expect(designed.stages.map((stage) => stage.name)).toEqual([
      "mockup",
      "stakeholder",
      "plan",
      "in progress",
      "review",
      "pr",
      "pr-review",
    ]);
    const stakeholder = designed.stages.find((stage) => stage.name === "stakeholder");
    expect(stakeholder?.agent).toBeUndefined();
    expect(stakeholder?.exits).toBeUndefined();
    expect(designed.stages.find((stage) => stage.name === "pr-review")?.policy.handoff).toBe("merge");
  });

  it("ships a schema example that loads", () => {
    const schema = JSON.parse(
      readFileSync(new URL("../../../../.kanna/workflows/schema.json", import.meta.url), "utf8"),
    ) as { examples: unknown[] };
    for (const example of schema.examples) {
      expect(() => parseWorkflowJson(JSON.stringify(example))).not.toThrow();
    }
  });
});
