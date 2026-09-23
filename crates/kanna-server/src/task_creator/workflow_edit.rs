//! Authoring boundary for replacing an existing task's pinned linear workflow.
use super::{definitions::*, TaskWorkflowSnapshot};
use crate::db::{Repo, StageRun};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub(super) static SCHEMA: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str(include_str!("../../../../.kanna/workflows/schema.json"))
        .expect("bundled workflow schema is JSON");
    jsonschema::validator_for(&schema).expect("bundled workflow schema compiles")
});

/// Keys the bundled schema defines at one position in a workflow document.
fn schema_keys(path: &[&str]) -> std::collections::BTreeSet<String> {
    static SCHEMA_JSON: LazyLock<Value> = LazyLock::new(|| {
        serde_json::from_str(include_str!("../../../../.kanna/workflows/schema.json"))
            .expect("bundled workflow schema is JSON")
    });
    let mut node = &*SCHEMA_JSON;
    for segment in path {
        let Some(next) = node.get(segment) else {
            return Default::default();
        };
        node = next;
    }
    node.get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default()
}

/// Input spellings the loader accepts and deliberately rewrites, so a document
/// written for an older Kanna is not mistaken for one written for a newer one.
const LEGACY_STAGE_KEYS: &[&str] = &["transition", "mode", "post_action"];

/// Fields in `definition` that this build's workflow document does not define.
///
/// This is the question "was this written for a Kanna newer than mine?", asked
/// by name rather than by round-tripping the document: normalization
/// deliberately rewrites legacy spellings, so a shape comparison would flag
/// every old definition as a loss. The known names come from the bundled
/// schema itself, so they cannot drift from what the loader actually reads.
///
/// Returns the unknown paths, empty when this build carries the document whole.
pub(crate) fn unknown_workflow_fields(definition: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(definition) else {
        // Not a workflow document at all. Whatever is wrong with it, it is not
        // this question, and the existing paths already answer it.
        return Vec::new();
    };
    let root_keys = schema_keys(&[]);
    let stage_keys = schema_keys(&["properties", "stages", "items"]);
    let post_keys = schema_keys(&["properties", "stages", "items", "properties", "post"]);
    let mut unknown = Vec::new();
    let check = |object: Option<&serde_json::Map<String, Value>>,
                 known: &std::collections::BTreeSet<String>,
                 legacy: &[&str],
                 where_: &str,
                 into: &mut Vec<String>| {
        let Some(object) = object else { return };
        // An empty known set means the schema moved and this lookup found
        // nothing; refusing everything on that basis would be worse than
        // carrying the document.
        if known.is_empty() {
            return;
        }
        for key in object.keys() {
            if !known.contains(key) && !legacy.contains(&key.as_str()) {
                into.push(format!("{where_}{key}"));
            }
        }
    };
    check(value.as_object(), &root_keys, &[], "", &mut unknown);
    for (index, stage) in value
        .get("stages")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        check(
            stage.as_object(),
            &stage_keys,
            LEGACY_STAGE_KEYS,
            &format!("stages[{index}]."),
            &mut unknown,
        );
        check(
            stage.get("post").and_then(Value::as_object),
            &post_keys,
            &[],
            &format!("stages[{index}].post."),
            &mut unknown,
        );
    }
    unknown
}

pub(crate) struct ValidatedWorkflowReplacement {
    pub snapshot: TaskWorkflowSnapshot,
    pub superseded_run_ids: Vec<String>,
    pub changed_execution_stages: Vec<String>,
}

/// What an edit may do to the stamped `plan_context`.
///
/// The plan is Kanna's own provenance, not authored content: an ordinary
/// replacement may neither invent nor change it, and only the combined plan
/// completion stamps one.
pub(crate) enum PlanContextPolicy<'a> {
    /// Carry the prior definition's stamp forward; refuse a different one.
    Preserve,
    /// Overwrite with this stamp, whatever the caller submitted.
    Stamp(&'a WorkflowPlanContext),
}

pub(crate) fn validate_task_workflow_replacement(
    repo: &Repo,
    value: &Value,
    previous: &str,
    current_stage: &str,
    runs: &[StageRun],
) -> Result<ValidatedWorkflowReplacement, String> {
    validate_task_workflow_replacement_with_plan_context(
        repo,
        value,
        previous,
        current_stage,
        runs,
        PlanContextPolicy::Preserve,
    )
}

pub(crate) fn validate_task_workflow_replacement_with_plan_context(
    repo: &Repo,
    value: &Value,
    previous: &str,
    current_stage: &str,
    runs: &[StageRun],
    plan_context: PlanContextPolicy<'_>,
) -> Result<ValidatedWorkflowReplacement, String> {
    if value.to_string().len() > 256 * 1024 {
        return Err("workflowDefinition exceeds 256 KiB".into());
    }
    let errors: Vec<_> = SCHEMA
        .iter_errors(value)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect();
    if !errors.is_empty() {
        return Err(format!("workflowDefinition: {}", errors.join("; ")));
    }
    let mut workflow = parse_workflow_definition(&value.to_string())?;
    let prior = parse_stored_workflow_definition(previous)?;
    // A running session was instructed under its workflow's routing contract
    // (a legacy reviewer names a stage through the revision API; a named-exit
    // one names an exit). Switching the contract under it would silently
    // change what its instructions mean, so the switch waits for a moment
    // when no run is live.
    if workflow.routing != prior.routing && runs.iter().any(|run| run.status == "running") {
        return Err(format!(
            "cannot change this task's result routing from {:?} to {:?} while a stage run is \
             running: its session was instructed under the current routing. Replace the \
             workflow once the run has finished.",
            prior.routing, workflow.routing
        ));
    }
    if workflow.routes_by_exits() {
        // The remaining plan is replaced wholesale; the one thing it may not
        // do is change what the task is doing now (spec §10).
        let role = |definition: &WorkflowDefinition| {
            definition
                .stages
                .iter()
                .find(|stage| stage.name == current_stage)
                .map(|stage| stage.agent.clone())
        };
        match (role(&prior), role(&workflow)) {
            (Some(before), Some(after)) if before != after => {
                return Err(format!(
                    "the current stage '{current_stage}' must keep its role: it runs {} and the \
                     replacement names {}",
                    before.as_deref().unwrap_or("no agent"),
                    after.as_deref().unwrap_or("no agent"),
                ))
            }
            (_, None) => {
                return Err(format!(
                    "the current stage '{current_stage}' must survive the replacement as a stage"
                ))
            }
            _ => {}
        }
    }
    match plan_context {
        PlanContextPolicy::Stamp(_) if workflow.routes_by_exits() => {
            return Err(
                "a named-exit workflow keeps its plan in the task ledger; plan_context is not \
                 stamped onto it"
                    .into(),
            )
        }
        PlanContextPolicy::Stamp(stamp) => workflow.plan_context = Some(stamp.clone()),
        // Named-exit routing reads the plan from the ledger, so a legacy stamp
        // is not carried across a routing switch (the parse above already
        // refused one authored by the caller).
        PlanContextPolicy::Preserve if workflow.routes_by_exits() => {}
        PlanContextPolicy::Preserve => {
            if workflow.plan_context.is_some() && workflow.plan_context != prior.plan_context {
                return Err(
                    "plan_context is stamped by Kanna when a plan stage publishes its remaining \
                     stages; an edit cannot author or change it"
                        .into(),
                );
            }
            workflow.plan_context = prior.plan_context.clone();
        }
    }
    let definitions = RepoDefinitions::resolve(repo)?;
    let bindings =
        |workflow: &WorkflowDefinition| -> Result<BTreeMap<String, (String, Value)>, String> {
            let mut bindings = BTreeMap::new();
            for stage in &workflow.stages {
                if bindings
                    .insert(
                        stage.name.clone(),
                        (
                            "main".into(),
                            serde_json::to_value(stage)
                                .map_err(|error| format!("stage '{}': {error}", stage.name))?,
                        ),
                    )
                    .is_some()
                {
                    return Err(format!("duplicate stage/post name '{}'", stage.name));
                }
                if let Some(post) = &stage.post {
                    if bindings
                        .insert(
                            post.name.clone(),
                            (
                                format!("post:{}", stage.name),
                                serde_json::to_value(post)
                                    .map_err(|error| format!("post '{}': {error}", post.name))?,
                            ),
                        )
                        .is_some()
                    {
                        return Err(format!("duplicate stage/post name '{}'", post.name));
                    }
                }
            }
            Ok(bindings)
        };
    let before = bindings(&prior)?;
    let after = bindings(&workflow)?;
    if after.len() > 32 {
        return Err("workflowDefinition exceeds 32 stages including posts".into());
    }
    for name in std::iter::once(current_stage).chain(runs.iter().map(|run| run.stage.as_str())) {
        let old = before
            .get(name)
            .ok_or_else(|| format!("recorded stage '{name}' is absent from the pinned workflow"))?;
        let new = after
            .get(name)
            .ok_or_else(|| format!("cannot remove or rename current/recorded stage '{name}'"))?;
        if old.0 != new.0 {
            return Err(format!(
                "cannot change the main/post ownership of recorded stage '{name}'"
            ));
        }
    }
    let protected: std::collections::BTreeSet<&str> = std::iter::once(current_stage)
        .chain(runs.iter().map(|run| run.stage.as_str()))
        .collect();
    let protected_order = |definition: &WorkflowDefinition| -> Vec<String> {
        definition
            .stages
            .iter()
            .flat_map(|stage| {
                std::iter::once(&stage.name).chain(stage.post.iter().map(|post| &post.name))
            })
            .filter(|name| protected.contains(name.as_str()))
            .cloned()
            .collect()
    };
    if protected_order(&prior) != protected_order(&workflow) {
        return Err("cannot reorder current/recorded stages or posts".into());
    }
    for stage in &workflow.stages {
        if let Some(environment) = &stage.environment {
            if !workflow
                .environments
                .as_ref()
                .is_some_and(|all| all.contains_key(environment))
            {
                return Err(format!(
                    "stage '{}': unknown environment '{environment}'",
                    stage.name
                ));
            }
        }
        for (name, agent) in std::iter::once((&stage.name, &stage.agent))
            .chain(stage.post.iter().map(|post| (&post.name, &post.agent)))
        {
            if let Some(agent) = agent {
                definitions
                    .agent(agent)
                    .map_err(|error| format!("stage '{name}', agent '{agent}': {error}"))?;
            }
        }
    }
    // A changed execution binding supersedes old conversations for this stage
    // only. Description/policy edits need no provider re-resolution.
    let execution = |definition: &WorkflowDefinition, name: &str, value: &Value| {
        let owner = definition.stages.iter().find(|stage| {
            stage.name == name || stage.post.as_ref().is_some_and(|post| post.name == name)
        });
        let environment = owner.and_then(|stage| stage.environment.as_deref());
        let selection = owner.and_then(|stage| {
            if stage.name == name { stage.agent_provider.as_ref() }
            else { stage.post.as_ref()?.agent_provider.as_ref() }
        }).map(|entries| entries.iter().map(|entry| {
            let candidate = entry.resolve(true).expect("validated workflow selection");
            serde_json::json!({"harness": candidate.provider, "model": candidate.model, "effort": candidate.effort})
        }).collect::<Vec<_>>());
        serde_json::json!({
            "agent": value.get("agent"), "agent_provider": selection,
            "prompt": value.get("prompt"), "environment": environment,
            "environmentDefinition": environment.and_then(|name| definition.environments.as_ref()?.get(name))
        })
    };
    let changed_execution_stages: Vec<String> = after
        .iter()
        .filter(|(name, new)| {
            before.get(*name).is_none_or(|old| {
                execution(&prior, name, &old.1) != execution(&workflow, name, &new.1)
            })
        })
        .map(|(name, _)| name.clone())
        .collect();
    let superseded_run_ids = runs
        .iter()
        .filter(|run| changed_execution_stages.contains(&run.stage))
        .map(|run| run.id.clone())
        .collect();
    Ok(ValidatedWorkflowReplacement {
        snapshot: TaskWorkflowSnapshot {
            definition_json: serde_json::to_string(&workflow).map_err(|error| error.to_string())?,
            stage_names: workflow
                .stages
                .iter()
                .map(|stage| stage.name.clone())
                .collect(),
            revision_limit: workflow.revision_limit(),
        },
        superseded_run_ids,
        changed_execution_stages,
    })
}

/// Validate the remaining plan a named-exit task's result publishes for itself
/// (spec §10): the replacement is wholesale, fenced by the caller on the
/// definition it read, and the only rules are the ordinary replacement ones —
/// recorded stages keep their names and order, the current stage keeps its
/// role, and every stage, role and exit resolves. There is no recipe and no
/// stamped plan: the result that published it is in the ledger.
pub(crate) fn validate_remaining_plan_replacement(
    repo: &Repo,
    value: &Value,
    previous: &str,
    current_stage: &str,
    runs: &[StageRun],
) -> Result<ValidatedWorkflowReplacement, String> {
    let prior = parse_stored_workflow_definition(previous)?;
    let workflow = parse_workflow_definition(&value.to_string())?;
    if !prior.routes_by_exits() || !workflow.routes_by_exits() {
        return Err(
            "a result may replace its task's remaining plan only when both the pinned workflow \
             and the replacement route by named exits (\"routing\": \"exits\")"
                .into(),
        );
    }
    validate_task_workflow_replacement(repo, value, previous, current_stage, runs)
}

/// The stage suffixes a plan stage may publish for its own task.
///
/// These are the existing product-work recipes — `no-review` and the
/// `single-reviewer`/`specialized-reviewers` shape — expressed as the stage
/// and post names they must use. Only the *shape* is fixed: the planner
/// chooses each stage's agent binding and provider selectors, which is how one
/// entry covers both the ordinary reviewer and the QA dispatcher. Restricting
/// the shape is deliberate: this is a linear engine, and an arbitrary suffix
/// would be a workflow language nobody has committed to executing.
const PLAN_SUFFIX_RECIPES: &[&[(&str, Option<&str>)]] = &[
    &[("in progress", Some("commit")), ("pr", Some("approve"))],
    &[
        ("in progress", Some("commit")),
        ("review", None),
        ("pr", Some("approve")),
    ],
];

/// Validate the stages a plan stage publishes onto its own task.
///
/// The prior definition must survive byte-for-byte as a prefix: a plan may
/// only decide what has not happened yet, never rewrite the research or
/// planning that produced it.
pub(crate) fn validate_plan_workflow_extension(
    previous: &str,
    value: &Value,
    plan_stage: &str,
) -> Result<(), String> {
    let prior = parse_stored_workflow_definition(previous)?;
    let workflow = parse_workflow_definition(&value.to_string())?;
    let Some(last) = prior.stages.last() else {
        return Err("pinned workflow has no stages".into());
    };
    if last.name != plan_stage {
        return Err(format!(
            "stage '{plan_stage}' is not the final stage of the pinned workflow; \
             its remaining stages have already been published"
        ));
    }
    if workflow.stages.len() <= prior.stages.len() {
        return Err(
            "workflowDefinition must append the remaining stages after the planning stage".into(),
        );
    }
    let serialize = |stage: &WorkflowStage| {
        serde_json::to_value(stage).map_err(|error| format!("stage '{}': {error}", stage.name))
    };
    for (index, before) in prior.stages.iter().enumerate() {
        let after = &workflow.stages[index];
        if serialize(before)? != serialize(after)? {
            return Err(format!(
                "workflowDefinition may only append stages: stage '{}' differs from the \
                 pinned definition",
                before.name
            ));
        }
    }
    let suffix: Vec<(&str, Option<&str>)> = workflow.stages[prior.stages.len()..]
        .iter()
        .map(|stage| {
            (
                stage.name.as_str(),
                stage.post.as_ref().map(|post| post.name.as_str()),
            )
        })
        .collect();
    if !PLAN_SUFFIX_RECIPES
        .iter()
        .any(|recipe| recipe.iter().copied().eq(suffix.iter().copied()))
    {
        return Err(format!(
            "the published stages must follow a supported recipe — {} — choosing each stage's \
             agent and provider freely; got {}",
            PLAN_SUFFIX_RECIPES
                .iter()
                .map(|recipe| format!("[{}]", describe_recipe(recipe)))
                .collect::<Vec<_>>()
                .join(" or "),
            format_args!("[{}]", describe_recipe(&suffix)),
        ));
    }
    match workflow.revision_limit {
        Some(limit) if limit > 0 => {}
        _ => {
            return Err(
                "workflowDefinition must declare a finite positive revision_limit so the \
                 published review loop is bounded"
                    .into(),
            )
        }
    }
    Ok(())
}

fn describe_recipe(recipe: &[(&str, Option<&str>)]) -> String {
    recipe
        .iter()
        .map(|(stage, post)| match post {
            Some(post) => format!("{stage} (+{post})"),
            None => (*stage).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" -> ")
}
