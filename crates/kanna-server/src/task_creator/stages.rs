use crate::config::Config;
use crate::db::{Db, StageProviderOverride, StageTrigger, TaskStageSource};

use super::definitions::{
    describe_stage_exits, parse_stored_workflow_definition, parse_workflow_definition,
    post_as_stage, resolve_stage_position, RepoDefinitions, StagePosition, WorkflowDefinition,
    WorkflowStage, WorkflowStageTransition, ADVANCE_EXIT,
};
use super::prepare_stage_run_spawn;
use super::prompt::{
    build_completed_stage_recovery_prompt, build_revision_resume_message,
    build_revision_task_prompt, build_target_stage_prompt_parts,
    build_target_stage_prompt_with_instructions, RevisionRound, StagePromptParts,
};
use super::resume::{prepare_resume_session, prepare_resume_workspace, same_cwd};
use super::session::{self, RevisitPlan};
use super::types::{
    PreparedPostDispatch, PreparedRunWorkspace, PreparedStageRunSpawn, PreparedStageTransition,
    RevisitResume, RunWorkspaceSpec,
};
use super::worktree::allocate_task_branch;
use super::AgentInstructions;
use super::SpawnAgentOverrides;
use super::FALLBACK_WORKFLOW_NAME;
use crate::db::Repo;
use crate::db::TransitionExit;

pub(super) const REREVIEW_VERDICT_COMPLETION_INSTRUCTION: &str = "Your run is not complete until you have called `kanna_complete_stage` or `kanna_request_revision`; a summary without one of these is an unfinished review.";

/// Everything stage routing needs about the task being transitioned.
struct StageTransitionContext<'a> {
    source_task: &'a TaskStageSource,
    source_task_id: &'a str,
    repo: &'a Repo,
    definitions: &'a RepoDefinitions,
    workflow_name: &'a str,
    workflow: &'a WorkflowDefinition,
}

struct LoadedStageTransitionSource {
    source_task: TaskStageSource,
    repo: Repo,
    definitions: RepoDefinitions,
    workflow_name: String,
    workflow: WorkflowDefinition,
    current_stage_name: String,
}

struct LoadedStageIdentity {
    source_task: TaskStageSource,
    repo: Repo,
}

fn load_stage_identity(db: &Db, source_task_id: &str) -> Result<LoadedStageIdentity, String> {
    let source_task = db
        .get_task_stage_source(source_task_id)
        .map_err(|e| format!("db error: {}", e))?
        .ok_or_else(|| format!("task not found: {}", source_task_id))?;
    let repo = db
        .get_repo(&source_task.repo_id)
        .map_err(|e| format!("db error: {}", e))?
        .ok_or_else(|| format!("repo not found for task: {}", source_task_id))?;
    Ok(LoadedStageIdentity { source_task, repo })
}

/// Load everything a stage preparation needs, with the task's workspace
/// identity first reconciled onto the branch that actually holds its committed
/// work — unless the triggering result recorded the input commit, which is
/// then the base (see `session::stage_input`).
///
/// A fork without a recorded input cuts from `source_task.branch`, so that
/// field has to be the task's real committed tip before anything else reads
/// it. A revision round whose commit landed on a workspace the field no
/// longer named used to be dropped by the next fork, and the next reviewer
/// re-raised the same finding — see `work_tip` and its regression tests.
fn load_stage_transition_source(
    db: &Db,
    config: &Config,
    identity: LoadedStageIdentity,
    source_task_id: &str,
) -> Result<LoadedStageTransitionSource, String> {
    let LoadedStageIdentity {
        mut source_task,
        repo,
    } = identity;
    // When the triggering result recorded the commit the next session takes
    // (spec §6), that commit is the base and newest-branch discovery must not
    // move the task. Discovery remains the safety net only for a transition
    // with no recorded input, which is every transition from before the
    // ledger recorded one.
    let recorded_input = source_task.stage.as_deref().is_some_and(|stage| {
        session::stage_input(config, db, &repo.path, source_task_id, stage).is_some()
    });
    if source_task.closed_at.is_none() && !recorded_input {
        super::work_tip::reconcile_task_work_branch(
            db,
            &repo.path,
            source_task_id,
            &mut source_task,
        )?;
    }
    let definitions = RepoDefinitions::resolve(&repo)?;
    let workflow_name = source_task
        .pipeline
        .clone()
        .unwrap_or_else(|| FALLBACK_WORKFLOW_NAME.to_string());
    let current_stage_name = source_task
        .stage
        .clone()
        .ok_or_else(|| format!("task has no stage: {}", source_task_id))?;
    let workflow =
        definitions.task_workflow(&workflow_name, source_task.pipeline_def.as_deref())?;
    Ok(LoadedStageTransitionSource {
        source_task,
        repo,
        definitions,
        workflow_name,
        workflow,
        current_stage_name,
    })
}

/// The caller-declared context of one explicit stage advance.
///
/// `trigger` says who asked for the advance; `provider_override` is the
/// optional provider/model/effort the next stage must spawn with, carrying its
/// own declared source because the agent that recommends a builder tier is
/// often not the operator who accepts it.
#[derive(Debug, Clone)]
pub(crate) struct StageAdvanceIntent {
    pub(crate) trigger: StageTrigger,
    pub(crate) provider_override: Option<StageProviderOverride>,
}

impl Default for StageAdvanceIntent {
    fn default() -> Self {
        Self {
            trigger: StageTrigger::Unspecified,
            provider_override: None,
        }
    }
}

#[cfg(test)]
pub(crate) fn prepare_advance_stage_for_api(
    db: &Db,
    config: &Config,
    source_task_id: &str,
) -> Result<PreparedStageTransition, String> {
    prepare_advance_stage_for_api_with_intent(
        db,
        config,
        source_task_id,
        StageAdvanceIntent::default(),
    )
}

pub(crate) fn prepare_advance_stage_for_api_with_intent(
    db: &Db,
    config: &Config,
    source_task_id: &str,
    intent: StageAdvanceIntent,
) -> Result<PreparedStageTransition, String> {
    let StageAdvanceIntent {
        trigger,
        provider_override,
    } = intent;
    let identity = load_stage_identity(db, source_task_id)?;
    if identity.source_task.closed_at.is_some() {
        return Err(format!("task is closed: {}", source_task_id));
    }
    let open_blockers = db
        .count_open_task_blockers(source_task_id)
        .map_err(|e| format!("db error: {}", e))?;
    if open_blockers > 0 {
        return Err(format!("task is blocked: {}", source_task_id));
    }
    let loaded = load_stage_transition_source(db, config, identity, source_task_id)?;
    let context = StageTransitionContext {
        source_task: &loaded.source_task,
        source_task_id,
        repo: &loaded.repo,
        definitions: &loaded.definitions,
        workflow_name: &loaded.workflow_name,
        workflow: &loaded.workflow,
    };

    let position = resolve_stage_position(&loaded.workflow, &loaded.current_stage_name)
        .ok_or_else(|| format!("stage not found in workflow: {}", loaded.current_stage_name))?;
    match position {
        // Legacy in-flight task parked at a folded post name (e.g. `commit`):
        // the post is the current context, so advancing swaps past its owner.
        StagePosition::Post { owner } => {
            prepare_swap_to_index(db, config, &context, owner + 1, trigger, provider_override)
                .map(|transition| with_operator_advance_exit(&loaded.workflow, transition))
        }
        StagePosition::Stage(index) => {
            let stage = &loaded.workflow.stages[index];
            if let Some(post) = &stage.post {
                let latest = db
                    .latest_stage_run(source_task_id)
                    .map_err(|e| format!("db error: {}", e))?;
                // Dispatch the post unless it already ran for this stage
                // visit. A repeated advance while it is running is never an
                // override; only the post's verdict may complete the deferred
                // transition. Failed or cancelled posts are re-dispatched.
                let post_pending = match &latest {
                    Some(run) if run.kind == "post" && run.stage == post.name => {
                        if run.status == "running" {
                            return Err(format!(
                                "post is still running for task {source_task_id}: {}",
                                post.name
                            ));
                        }
                        matches!(run.status.as_str(), "failed" | "cancelled")
                    }
                    _ => true,
                };
                if post_pending {
                    // The stage's post owns the transition from here: this
                    // advance only dispatches it, and the swap happens when
                    // the post reports success. An override handed to a
                    // dispatch would be silently dropped at that boundary, so
                    // say so instead of losing it.
                    if let Some(provider_override) = provider_override {
                        return Err(format!(
                            "cannot apply a provider override for the next stage of \
                             {source_task_id}: advancing dispatches this stage's post \
                             ({}), and the transition to {} runs when that post completes. \
                             Requested provider: {}.",
                            post.name,
                            loaded
                                .workflow
                                .stages
                                .get(index + 1)
                                .map(|stage| stage.name.as_str())
                                .unwrap_or("task close"),
                            provider_override.provider,
                        ));
                    }
                    return prepare_post_dispatch(db, config, &context, index, trigger);
                }
            }
            prepare_swap_to_index(db, config, &context, index + 1, trigger, provider_override)
                .map(|transition| with_operator_advance_exit(&loaded.workflow, transition))
        }
    }
}

/// An explicit advance of a named-exit task is a person or manager operating
/// the stage's gate: the transition takes `advance`, and no session chose it.
fn with_operator_advance_exit(
    workflow: &WorkflowDefinition,
    mut transition: PreparedStageTransition,
) -> PreparedStageTransition {
    if workflow.routes_by_exits() {
        transition.set_entry_exit(Some(TransitionExit {
            exit: Some(ADVANCE_EXIT.to_string()),
            source: TransitionExit::OPERATOR.to_string(),
            budget: None,
        }));
    }
    transition
}

/// Routes a stage-run completion verdict (`complete-stage` with
/// status=success). `finished_run_kind` identifies the run that just
/// finished: a `post` completion performs the deferred swap regardless of
/// the stage's transition policy (the gate was passed when the post was
/// dispatched); a `main` completion follows the stage's policy — `auto`
/// dispatches the stage's post (or swaps when there is none), `manual`
/// parks the task.
#[cfg(test)]
pub(crate) fn prepare_stage_completion_for_api(
    db: &Db,
    config: &Config,
    source_task_id: &str,
    finished_run_kind: Option<&str>,
    completion_transition: Option<&str>,
) -> Result<Option<PreparedStageTransition>, String> {
    prepare_stage_completion_for_api_with_trigger(
        db,
        config,
        source_task_id,
        finished_run_kind,
        completion_transition,
        None,
        None,
    )
}

/// `exit` is the exit the completion took; it is only kept so a completion
/// parked on stage dependency edges replays with it.
pub(crate) fn prepare_stage_completion_for_api_with_trigger(
    db: &Db,
    config: &Config,
    source_task_id: &str,
    finished_run_kind: Option<&str>,
    completion_transition: Option<&str>,
    finished_run_trigger: Option<&str>,
    exit: Option<&crate::db::TransitionExit>,
) -> Result<Option<PreparedStageTransition>, String> {
    let identity = load_stage_identity(db, source_task_id)?;
    if identity.source_task.closed_at.is_some() {
        return Ok(None);
    }
    let loaded = load_stage_transition_source(db, config, identity, source_task_id)?;
    let context = StageTransitionContext {
        source_task: &loaded.source_task,
        source_task_id,
        repo: &loaded.repo,
        definitions: &loaded.definitions,
        workflow_name: &loaded.workflow_name,
        workflow: &loaded.workflow,
    };

    let position = resolve_stage_position(&loaded.workflow, &loaded.current_stage_name)
        .ok_or_else(|| format!("stage not found in workflow: {}", loaded.current_stage_name))?;
    match position {
        // Legacy in-flight task parked at a folded post name: success means
        // the post finished, which always advances past its owner.
        StagePosition::Post { owner } => swap_or_wait_on_dependencies(
            db,
            config,
            &context,
            owner + 1,
            stage_trigger_from_stored(finished_run_trigger),
            finished_run_kind,
            completion_transition,
            finished_run_trigger,
            exit,
        ),
        StagePosition::Stage(index) => {
            let stage = &loaded.workflow.stages[index];
            if finished_run_kind == Some("post") {
                return swap_or_wait_on_dependencies(
                    db,
                    config,
                    &context,
                    index + 1,
                    stage_trigger_from_stored(finished_run_trigger),
                    finished_run_kind,
                    completion_transition,
                    finished_run_trigger,
                    exit,
                );
            }
            let transition = match completion_transition {
                Some("manual") => WorkflowStageTransition::Manual,
                Some("auto") => WorkflowStageTransition::Auto,
                Some(value) => {
                    return Err(format!("invalid stage run completion transition: {value}"))
                }
                None => stage.policy.transition,
            };
            if transition != WorkflowStageTransition::Auto {
                return Ok(None);
            }
            if stage.post.is_some() {
                return prepare_post_dispatch(db, config, &context, index, StageTrigger::Auto)
                    .map(Some);
            }
            if !main_completion_has_continuation(&loaded.workflow, index) {
                // An auto main-run completion never closes the task; only an
                // explicit advance (or a post completion) moves past the
                // final stage.
                return Ok(None);
            }
            swap_or_wait_on_dependencies(
                db,
                config,
                &context,
                index + 1,
                StageTrigger::Auto,
                finished_run_kind,
                completion_transition,
                finished_run_trigger,
                exit,
            )
        }
    }
}

/// Swap to `next_index`, gated on the stage dependency edges into that stage
/// (T4): with any unsatisfied the move is refused as blocked, and otherwise
/// the new session is told which upstream results held it. Gating never
/// changes the new stage's base.
fn prepare_swap_to_index(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    next_index: usize,
    trigger: StageTrigger,
    provider_override: Option<StageProviderOverride>,
) -> Result<PreparedStageTransition, String> {
    let Some(next_stage) = context.workflow.stages.get(next_index) else {
        return prepare_swap_to_index_ungated(
            db,
            config,
            context,
            next_index,
            trigger,
            provider_override,
        );
    };
    let pending = db
        .unsatisfied_stage_edges_into(context.source_task_id, &next_stage.name)
        .map_err(|e| format!("db error: {}", e))?;
    if !pending.is_empty() {
        return Err(stage_dependencies_pending_error(
            context.source_task_id,
            &next_stage.name,
            &pending,
        ));
    }
    let inputs = db
        .stage_edge_inputs(context.source_task_id, &next_stage.name, false)
        .map_err(|e| format!("db error: {}", e))?
        .unwrap_or_default();
    crate::task_store::with_dependency_inputs(
        inputs
            .iter()
            .map(crate::db::ConsumedDependency::to_session_input)
            .collect(),
        || {
            prepare_swap_to_index_ungated(
                db,
                config,
                context,
                next_index,
                trigger,
                provider_override,
            )
        },
    )
}

fn stage_dependencies_pending_error(
    task_id: &str,
    stage: &str,
    pending: &[crate::db::StageEdge],
) -> String {
    let edges = pending
        .iter()
        .map(|edge| format!("{} ({})", edge.upstream_task_id, edge.upstream_stage))
        .collect::<Vec<_>>()
        .join(", ");
    format!("task is blocked: {task_id} cannot enter stage '{stage}' until its dependencies leave their stages: {edges}")
}

/// A completion's automatic swap to `next_index`, or — when edges into that
/// stage are not satisfied yet — a recorded wait that replays this
/// completion once they are. The task parks in its stage meanwhile.
#[allow(clippy::too_many_arguments)]
fn swap_or_wait_on_dependencies(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    next_index: usize,
    trigger: StageTrigger,
    finished_run_kind: Option<&str>,
    completion_transition: Option<&str>,
    finished_run_trigger: Option<&str>,
    exit: Option<&crate::db::TransitionExit>,
) -> Result<Option<PreparedStageTransition>, String> {
    if let Some(next_stage) = context.workflow.stages.get(next_index) {
        let pending = db
            .unsatisfied_stage_edges_into(context.source_task_id, &next_stage.name)
            .map_err(|e| format!("db error: {}", e))?;
        if !pending.is_empty() {
            let from_stage = context.source_task.stage.clone().unwrap_or_default();
            db.record_dependency_wait(
                context.source_task_id,
                &from_stage,
                &next_stage.name,
                &serde_json::json!({
                    "kind": finished_run_kind,
                    "completionTransition": completion_transition,
                    "trigger": finished_run_trigger,
                    "exit": exit,
                }),
            )
            .map_err(|e| format!("db error: {}", e))?;
            log::info!(
                "{}",
                stage_dependencies_pending_error(
                    context.source_task_id,
                    &next_stage.name,
                    &pending
                )
            );
            return Ok(None);
        }
    }
    prepare_swap_to_index(db, config, context, next_index, trigger, None).map(Some)
}

fn prepare_swap_to_index_ungated(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    next_index: usize,
    trigger: StageTrigger,
    provider_override: Option<StageProviderOverride>,
) -> Result<PreparedStageTransition, String> {
    let Some(next_stage) = context.workflow.stages.get(next_index) else {
        // Past the final stage there is no stage to give a provider to, and
        // advancing here closes the task. Refuse rather than accept a value
        // that would decide nothing.
        if let Some(provider_override) = provider_override {
            return Err(format!(
                "cannot apply a provider override for the next stage of {}: this advance                  closes the task, so there is no stage to spawn. Requested provider: {}.",
                context.source_task_id, provider_override.provider,
            ));
        }
        let workspace_teardown = context
            .source_task
            .branch
            .as_deref()
            .zip(context.source_task.stage.as_deref())
            .and_then(|(branch, stage_name)| {
                super::prepare_workspace_teardown_for_transition_close(
                    db,
                    config,
                    context.repo,
                    context.definitions,
                    context.source_task_id,
                    context.workflow,
                    stage_name,
                    branch,
                )
            })
            .map(Box::new);
        return Ok(PreparedStageTransition::Close {
            task_id: context.source_task_id.to_string(),
            workspace_teardown,
        });
    };
    let from_stage = context
        .source_task
        .stage
        .as_deref()
        .ok_or_else(|| format!("task has no stage: {}", context.source_task_id))?;
    let prompt_suffix = if next_stage.agent.as_deref() == Some("review")
        && db
            .latest_stage_run_for_stage(context.source_task_id, &next_stage.name, "main")
            .map_err(|error| format!("db error: {error}"))?
            .is_some()
    {
        Some(REREVIEW_VERDICT_COMPLETION_INSTRUCTION)
    } else {
        None
    };
    let mut run = prepare_stage_run_for_target(
        db,
        config,
        context,
        next_stage,
        &next_stage.name,
        "main",
        None,
        None,
        prompt_suffix,
        trigger,
        provider_override,
    )?;
    run.terminal_prelude = Some(super::terminal_marker::format_stage_transition_marker(
        from_stage,
        &next_stage.name,
    ));
    Ok(PreparedStageTransition::Run(Box::new(run)))
}

fn prepare_post_dispatch(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    owner_index: usize,
    trigger: StageTrigger,
) -> Result<PreparedStageTransition, String> {
    let owner = &context.workflow.stages[owner_index];
    let post_stage =
        post_as_stage(owner).ok_or_else(|| format!("stage has no post: {}", owner.name))?;
    let run_stage = post_stage.name.clone();

    // The fallback spawn resolves the post session and keeps its normal
    // auto-stage prompt. The returned live-session message is recomposed with
    // an explicit completion instruction before the post's task section.
    // `item_stage` stays the owner: a post never moves the task's stage.
    let task_id = context.source_task_id;
    let completion_instruction = format!(
        "When this work is complete, record stage completion: call MCP `kanna_complete_stage {{\"task_id\": \"{task_id}\", \"status\": \"success\", \"summary\": \"...\"}}`; only if MCP tools are unavailable, fall back to `kanna-cli stage-complete --task-id \"{task_id}\" --status success --summary \"...\"`. Kanna will then advance this task's workflow."
    );
    let (fallback, message) = prepare_stage_run_for_target_returning_prompt(
        db,
        config,
        context,
        &post_stage,
        &owner.name,
        "post",
        post_stage.policy.transition,
        None,
        None,
        SpawnAgentOverrides::default(),
        Some(&completion_instruction),
        None,
        trigger,
        None,
        None,
    )?;

    Ok(PreparedStageTransition::Post(Box::new(
        PreparedPostDispatch {
            task_id: context.source_task_id.to_string(),
            session_id: fallback.session_id.clone(),
            message,
            run_stage,
            fallback,
        },
    )))
}

#[allow(clippy::too_many_arguments)]
fn prepare_stage_run_for_target(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    target_stage: &WorkflowStage,
    item_stage: &str,
    run_kind: &'static str,
    prompt_override: Option<&str>,
    feedback: Option<String>,
    prompt_suffix: Option<&str>,
    trigger: StageTrigger,
    provider_override: Option<StageProviderOverride>,
) -> Result<PreparedStageRunSpawn, String> {
    // A stage transition otherwise lets the target stage and its agent
    // definition own the provider; an advance-carried override is the one
    // explicit layer above them.
    let agent_overrides = provider_override
        .as_ref()
        .map(SpawnAgentOverrides::from_provider_override)
        .unwrap_or_default();
    prepare_stage_run_for_target_with_provider(
        db,
        config,
        context,
        target_stage,
        item_stage,
        run_kind,
        target_stage.policy.transition,
        prompt_override,
        feedback,
        agent_overrides,
        prompt_suffix,
        trigger,
        provider_override,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_stage_run_for_target_with_provider(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    target_stage: &WorkflowStage,
    item_stage: &str,
    run_kind: &'static str,
    completion_transition: WorkflowStageTransition,
    prompt_override: Option<&str>,
    feedback: Option<String>,
    agent_overrides: SpawnAgentOverrides,
    prompt_suffix: Option<&str>,
    trigger: StageTrigger,
    provider_override: Option<StageProviderOverride>,
    workspace: Option<RunWorkspaceSpec>,
) -> Result<PreparedStageRunSpawn, String> {
    prepare_stage_run_for_target_returning_prompt(
        db,
        config,
        context,
        target_stage,
        item_stage,
        run_kind,
        completion_transition,
        prompt_override,
        feedback,
        agent_overrides,
        None,
        prompt_suffix,
        trigger,
        provider_override,
        workspace,
    )
    .map(|(run, _)| run)
}

#[allow(clippy::too_many_arguments)]
fn prepare_stage_run_for_target_returning_prompt(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    target_stage: &WorkflowStage,
    item_stage: &str,
    run_kind: &'static str,
    completion_transition: WorkflowStageTransition,
    prompt_override: Option<&str>,
    feedback: Option<String>,
    agent_overrides: SpawnAgentOverrides,
    additional_agent_instructions: Option<&str>,
    prompt_suffix: Option<&str>,
    trigger: StageTrigger,
    provider_override: Option<StageProviderOverride>,
    workspace: Option<RunWorkspaceSpec>,
) -> Result<(PreparedStageRunSpawn, String), String> {
    let source_task = context.source_task;
    let current_worktree = source_task.branch.as_deref().map(|branch| {
        session::current_workspace_path(db, &context.repo.path, context.source_task_id, branch)
    });
    let source_branch = current_worktree
        .as_deref()
        .and_then(super::resume::current_branch)
        .or_else(|| source_task.branch.clone());
    let prev_result = previous_stage_result(db, context.source_task_id, source_task)?;
    let prev_main_result = previous_main_stage_result(db, context.source_task_id)?;
    let plan_result = stamped_plan_result(db, context.source_task_id);
    let task_prompt = prompt_override
        .or(source_task.prompt.as_deref())
        .unwrap_or("");
    // Entering a stage forks a fresh workspace, named `task-<taskid>-<n>`
    // — the durable task id plus the task's persisted branch counter (N
    // worktrees, N branches, one PR — the PR agent renames the final branch
    // into something meaningful). It starts at the commit the triggering
    // result recorded; a task with no recorded input forks from its current
    // workspace as before. Posts run inside the stage, so their fallback
    // spawn keeps the stage's workspace. A loop back hands in its own
    // revisit of the stage's retained directory.
    let workspace_spec = match workspace {
        Some(workspace) => workspace,
        None if run_kind == "main" => fork_spec(
            db,
            config,
            context,
            current_worktree.as_deref(),
            &target_stage.name,
            None,
        )?,
        None => RunWorkspaceSpec::Current,
    };
    let prompt_branch = match &workspace_spec {
        RunWorkspaceSpec::Fork { branch, .. } => Some(branch.clone()),
        RunWorkspaceSpec::Revisit(revisit) => Some(revisit.branch.clone()),
        _ => source_branch.clone(),
    };
    let StagePromptParts {
        prompt: mut final_prompt,
        agent_instructions,
    } = build_target_stage_prompt_parts(
        context.definitions,
        &context.repo.path,
        target_stage,
        task_prompt,
        prev_result.as_deref(),
        prev_main_result.as_deref(),
        plan_result.as_deref(),
        prompt_branch.as_deref(),
        source_task.base_ref.as_deref(),
        source_task.branch.as_deref(),
        trigger.as_str(),
        None,
    )?;
    if let Some(suffix) = prompt_suffix {
        final_prompt.push_str("\n\n");
        final_prompt.push_str(suffix);
    }
    let returned_prompt = match additional_agent_instructions {
        Some(instructions) => build_target_stage_prompt_with_instructions(
            context.definitions,
            &context.repo.path,
            target_stage,
            task_prompt,
            prev_result.as_deref(),
            prev_main_result.as_deref(),
            plan_result.as_deref(),
            prompt_branch.as_deref(),
            source_task.base_ref.as_deref(),
            source_task.branch.as_deref(),
            trigger.as_str(),
            Some(instructions),
        )?,
        None => final_prompt.clone(),
    };
    // Stage transitions let the target stage and agent definition own the
    // provider, model, and effort. Only a real override (for example a
    // revision or recovery pin) is explicit; the task's stored provider
    // remains the final fallback.
    let branch = source_task
        .branch
        .as_deref()
        .ok_or_else(|| format!("task has no branch: {}", context.source_task_id))?;

    let mut run = prepare_stage_run_spawn(
        db,
        config,
        context.repo,
        context.definitions,
        context.source_task_id,
        context.workflow_name,
        context.workflow,
        target_stage,
        item_stage,
        run_kind,
        completion_transition,
        workspace_spec,
        final_prompt.clone(),
        agent_instructions.map(AgentInstructions::at_prompt_head),
        branch,
        feedback,
        source_task.agent_type.as_deref(),
        agent_overrides,
        source_task.agent_provider.as_deref(),
        trigger,
        provider_override,
    )?;
    if matches!(run.workspace, PreparedRunWorkspace::Forked(_)) {
        let departed_stage = source_task
            .stage
            .as_deref()
            .ok_or_else(|| format!("task has no stage: {}", context.source_task_id))?;
        run.workspace_teardown = super::prepare_workspace_teardown(
            db,
            config,
            context.repo,
            context.definitions,
            context.source_task_id,
            context.workflow,
            departed_stage,
            branch,
        );
    }
    Ok((run, returned_prompt))
}

fn stage_trigger_from_stored(trigger: Option<&str>) -> StageTrigger {
    match trigger {
        Some("auto") => StageTrigger::Auto,
        Some("operator") => StageTrigger::Operator,
        Some("manager") => StageTrigger::Manager,
        _ => StageTrigger::Unspecified,
    }
}

pub(crate) fn previous_stage_result(
    db: &Db,
    source_task_id: &str,
    _source_task: &TaskStageSource,
) -> Result<Option<String>, String> {
    if let Some(result) = db
        .latest_finished_stage_run_result(source_task_id)
        .map_err(|e| format!("db error: {}", e))?
    {
        return Ok(Some(result));
    }
    Ok(db
        .transferred_task_context(source_task_id)
        .map_err(|e| format!("db error: {}", e))?
        .and_then(|context| context.2))
}

/// Result of the previous stage agent's own run, skipping posts. A stage
/// whose predecessor declares a post (e.g. `in progress` → `commit` →
/// `review`) sees the post's result in `$PREV_RESULT`; this is what binds
/// `$PREV_MAIN_RESULT` so such a stage can still read what the stage agent
/// itself reported.
/// The plan stamped onto this task's pinned workflow, if its plan stage
/// published one. Read from the pinned definition rather than from run
/// history, so it survives every later stage, revision, resume, and recovery
/// without a second durable record.
pub(crate) fn stamped_plan_result(db: &Db, source_task_id: &str) -> Option<String> {
    let item = db.get_pipeline_item(source_task_id).ok().flatten()?;
    let definition = item.pipeline_def.as_deref()?;
    parse_stored_workflow_definition(definition)
        .ok()?
        .plan_context
        .map(|context| context.result)
}

pub(crate) fn previous_main_stage_result(
    db: &Db,
    source_task_id: &str,
) -> Result<Option<String>, String> {
    if let Some(result) = db
        .latest_finished_main_stage_run_result(source_task_id)
        .map_err(|e| format!("db error: {}", e))?
    {
        return Ok(Some(result));
    }
    Ok(db
        .transferred_task_context(source_task_id)
        .map_err(|e| format!("db error: {}", e))?
        .and_then(|context| context.3))
}

/// Reconstruct the predecessor values an active stage received when it was
/// first spawned.
///
/// Recovery closes the interrupted run before preparing its replacement, so
/// the ordinary "latest finished" lookups would select that bookkeeping
/// result (or an earlier replacement from the same stage). Looking only at
/// history before the first run in the replacement lineage reproduces the
/// values that run originally saw while preserving a deliberate earlier
/// rerun's inputs.
fn recovery_predecessor_results(
    db: &Db,
    source_task_id: &str,
    interrupted_run: &crate::db::StageRun,
) -> Result<(Option<String>, Option<String>), String> {
    let runs = db
        .list_stage_runs_for_task(source_task_id)
        .map_err(|error| format!("db error: {error}"))?;
    let mut lineage_start = runs
        .iter()
        .position(|run| run.id == interrupted_run.id)
        .ok_or_else(|| format!("interrupted stage run is missing: {}", interrupted_run.id))?;
    let mut lineage_cursor = lineage_start;
    let mut seen = std::collections::HashSet::new();
    seen.insert(interrupted_run.id.clone());
    for _ in 0..COMPLETED_STAGE_WALK_LIMIT {
        let Some(previous_id) = runs[lineage_cursor].replaces_run_id.as_deref() else {
            break;
        };
        if !seen.insert(previous_id.to_string()) {
            break;
        }
        let Some(previous_index) = runs.iter().position(|run| run.id == previous_id) else {
            break;
        };
        lineage_cursor = previous_index;
        lineage_start = lineage_start.min(previous_index);
    }

    let predecessors = &runs[..lineage_start];
    let finished_result = |kind: Option<&str>| {
        predecessors.iter().rev().find_map(|run| {
            (kind.is_none_or(|expected| run.kind == expected)
                && matches!(run.status.as_str(), "succeeded" | "failed"))
            .then(|| run.result.clone())
            .flatten()
        })
    };
    let previous = finished_result(None);
    let previous_main = finished_result(Some("main"));
    if previous.is_some() && previous_main.is_some() {
        return Ok((previous, previous_main));
    }

    let transferred = db
        .transferred_task_context(source_task_id)
        .map_err(|error| format!("db error: {error}"))?;
    Ok((
        previous.or_else(|| transferred.as_ref().and_then(|context| context.2.clone())),
        previous_main.or_else(|| transferred.and_then(|context| context.3)),
    ))
}

/// The feedback a revision actually runs on.
///
/// A revision whose reviewer-feedback section is empty is worse than no
/// revision: the agent has nothing to act on, the round is spent anyway, and
/// the verdict that triggered it is lost — so a request that carries no
/// feedback falls back to the verdict recorded on the task's terminating run
/// (its `feedback`, then its result `summary`), which is where a review's
/// findings are already durable. A revision with nothing to act on anywhere is
/// refused rather than started; the caller hands its claimed round back.
///
/// The agent-origin path is refused earlier, at the API boundary, so it can be
/// told to resend its findings. This is the backstop for every other caller.
fn resolve_revision_feedback(
    db: &Db,
    source_task_id: &str,
    requested: &str,
) -> Result<String, String> {
    if !requested.trim().is_empty() {
        return Ok(requested.to_string());
    }
    // The terminating run is the task's latest: a revision request closes the
    // review run before preparing the revision.
    let run = db
        .latest_stage_run(source_task_id)
        .map_err(|error| format!("db error: {error}"))?;
    let recorded = run.as_ref().and_then(|run| {
        run.feedback
            .as_deref()
            .filter(|feedback| !feedback.trim().is_empty())
            .map(str::to_string)
            .or_else(|| stage_run_result_summary(run.result.as_deref()))
    });
    let recorded = match recorded {
        Some(value) => Some(value),
        None => db
            .transferred_task_context(source_task_id)
            .map_err(|error| format!("db error: {error}"))?
            .and_then(|context| context.4),
    };
    match recorded {
        Some(feedback) => {
            log::warn!(
                "revision for task {source_task_id} carried no feedback; \
                 falling back to the terminating run's recorded verdict"
            );
            Ok(feedback)
        }
        None => Err(format!(
            "revision requires reviewer feedback: the request carried none and task \
             {source_task_id}'s terminating run recorded no verdict to fall back on"
        )),
    }
}

/// The `summary` of a stage run's `{status, summary, metadata}` result JSON,
/// when it has one worth reading.
fn stage_run_result_summary(result: Option<&str>) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(result?)
        .ok()?
        .get("summary")?
        .as_str()
        .filter(|summary| !summary.trim().is_empty())
        .map(str::to_string)
}

pub(crate) fn prepare_revision_task_for_api(
    db: &Db,
    config: &Config,
    source_task_id: &str,
    target_stage_name: &str,
    revision_prompt: &str,
    round: Option<RevisionRound>,
) -> Result<PreparedStageRunSpawn, String> {
    let identity = load_stage_identity(db, source_task_id)?;
    if identity.source_task.closed_at.is_some() {
        return Err(format!("task is closed: {}", source_task_id));
    }
    // Whatever the request carried, the agent must be started on real
    // feedback — an empty "Reviewer feedback:" section spends a budgeted
    // round on nothing and silently loses the verdict that triggered it.
    let revision_feedback = resolve_revision_feedback(db, source_task_id, revision_prompt)?;
    let revision_prompt = revision_feedback.as_str();
    let loaded = load_stage_transition_source(db, config, identity, source_task_id)?;
    let context = StageTransitionContext {
        source_task: &loaded.source_task,
        source_task_id,
        repo: &loaded.repo,
        definitions: &loaded.definitions,
        workflow_name: &loaded.workflow_name,
        workflow: &loaded.workflow,
    };

    let position = resolve_stage_position(&loaded.workflow, target_stage_name)
        .ok_or_else(|| format!("stage not found in workflow: {}", target_stage_name))?;
    let (target_stage, item_stage, run_kind): (WorkflowStage, String, &'static str) = match position
    {
        StagePosition::Stage(index) => {
            let stage = loaded.workflow.stages[index].clone();
            let item_stage = stage.name.clone();
            (stage, item_stage, "main")
        }
        // Revision targeting a post name (legacy `commit` targets): rerun
        // the post as a fresh session with feedback; the task's stage is
        // the post's owner.
        StagePosition::Post { owner } => {
            let owner_stage = &loaded.workflow.stages[owner];
            let post_stage = post_as_stage(owner_stage)
                .ok_or_else(|| format!("stage has no post: {}", owner_stage.name))?;
            (post_stage, owner_stage.name.clone(), "post")
        }
    };

    // A loop back re-enters the stage's retained directory on a newly
    // allocated branch (spec §6). There it prefers resuming the stage's
    // previous agent session, which already holds the exploration and
    // decision context the feedback refers to; without a transcript the
    // session starts fresh in the same directory from the ledger. A
    // directory that cannot be reused without moving what it holds is
    // preserved, reported, and the stage forks fresh instead.
    let (workspace, resume_fallback_reason) = if run_kind == "main" {
        let current_worktree = loaded.source_task.branch.as_deref().map(|branch| {
            session::current_workspace_path(db, &loaded.repo.path, source_task_id, branch)
        });
        let plan = session::plan_stage_revisit(
            config,
            db,
            &loaded.repo.path,
            source_task_id,
            &target_stage.name,
            current_worktree.as_deref().unwrap_or(&loaded.repo.path),
        )?;
        match plan {
            RevisitPlan::Reuse(revisit) => {
                match prepare_revision_resume(
                    db,
                    config,
                    &context,
                    &target_stage,
                    revision_prompt,
                    round,
                    revisit,
                )? {
                    ResumePreparation::Resumed(prepared) => return Ok(*prepared),
                    ResumePreparation::Fallback(reason, revisit) => {
                        (Some(RunWorkspaceSpec::Revisit(*revisit)), Some(reason))
                    }
                }
            }
            RevisitPlan::Fresh { reason, report } => (
                Some(fork_spec(
                    db,
                    config,
                    &context,
                    current_worktree.as_deref(),
                    &target_stage.name,
                    report,
                )?),
                Some(reason),
            ),
        }
    } else {
        (
            None,
            Some("post runs do not have an independently resumable provider session".to_string()),
        )
    };

    // Fresh fallback: compose the original task prompt with the reviewer's
    // feedback so the new agent still sees what the task was — a bare
    // prompt_override would clobber $TASK_PROMPT entirely. The run keeps the
    // task's provider: a revision continues the same stage's work, so the
    // agent def's provider priority list must not switch providers on it.
    // The model and effort come from the stage's own last run for the same
    // reason — a revision that quietly downgrades the model is not the same
    // work.
    let composed_prompt = build_revision_task_prompt(
        loaded.source_task.prompt.as_deref().unwrap_or(""),
        revision_prompt,
        round,
    );
    let last_run = db
        .latest_stage_run_for_stage(source_task_id, &target_stage.name, run_kind)
        .map_err(|error| format!("db error: {error}"))?;
    let superseded = last_run
        .as_ref()
        .map(|run| db.stage_run_workflow_superseded(source_task_id, &run.id))
        .transpose()
        .map_err(|error| format!("db error: {error}"))?
        .unwrap_or(false);
    let execution_edited = db
        .workflow_stage_execution_edited(source_task_id, &target_stage.name)
        .map_err(|error| format!("db error: {error}"))?;
    let agent_overrides = if superseded {
        SpawnAgentOverrides::default()
    } else if execution_edited {
        // Once a replacement has spawned, its new run owns the conversation's
        // tuning. A fresh revision must not resurrect the creation-time task
        // provider just because that provider has no resumable transcript.
        last_run
            .as_ref()
            .map(SpawnAgentOverrides::from_stage_run)
            .unwrap_or_default()
    } else {
        SpawnAgentOverrides {
            provider: loaded.source_task.agent_provider.clone(),
            model: last_run.as_ref().and_then(|run| run.model.clone()),
            effort: last_run.as_ref().and_then(|run| run.effort.clone()),
        }
    };
    let trigger = last_run
        .as_ref()
        .map(|run| stage_trigger_from_stored(Some(&run.trigger)))
        .unwrap_or(StageTrigger::Unspecified);
    let mut prepared = prepare_stage_run_for_target_with_provider(
        db,
        config,
        &context,
        &target_stage,
        &item_stage,
        run_kind,
        target_stage.policy.revision_transition(),
        Some(&composed_prompt),
        Some(revision_prompt.to_string()),
        agent_overrides,
        None,
        trigger,
        // Edited stages reproduce their new run's coherent stamp and origin.
        // The legacy fresh-revision path uses the task's provider and therefore
        // cannot attribute it to a previous run's per-advance override.
        if execution_edited && !superseded {
            last_run
                .as_ref()
                .and_then(|run| run.provider_override.clone())
        } else {
            None
        },
        workspace,
    )?;
    prepared.resume_fallback_reason = resume_fallback_reason;
    Ok(prepared)
}

/// A fresh workspace for entering `stage`: the task's next counter branch,
/// started at the recorded input commit when there is one. `report` carries
/// what the caller already preserved; otherwise it is what the fork leaves
/// behind in the task's current workspace.
fn fork_spec(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    current_worktree: Option<&str>,
    stage: &str,
    report: Option<String>,
) -> Result<RunWorkspaceSpec, String> {
    let input = session::stage_input(
        config,
        db,
        &context.repo.path,
        context.source_task_id,
        stage,
    );
    let report = report.or_else(|| {
        input
            .as_ref()
            .zip(current_worktree)
            .and_then(|(input, current)| {
                session::fork_input_report(&context.repo.path, current, input)
            })
    });
    Ok(RunWorkspaceSpec::Fork {
        branch: allocate_task_branch(db, &context.repo.path, context.source_task_id)?,
        start_point: input.map(|input| input.commit),
        report,
    })
}

/// Why a stage is being restarted in place, and whether the previous run's
/// provider transcript may be used at all.
enum StageRestartIntent {
    /// Operator or API recovery of an interrupted run: prefer the previous
    /// provider session, fall back to a fresh conversation when any resume
    /// precondition fails.
    ResumeProviderSession,
    /// The provider CLI accepted the launch and then rejected the transcript
    /// itself. Asking for it again would fail the same way, so the
    /// replacement must not carry `--resume`.
    FreshAfterRejectedResume {
        /// The still-running resume attempt this replaces. Checked against the
        /// task's latest run so a stale classification cannot restart a stage
        /// that has already moved on.
        rejected_run_id: String,
        reason: String,
    },
    /// The provider positively refused the turn for spent quota before the
    /// attempt changed anything, and the stage's own ordered candidate list
    /// names another authorized provider.
    ///
    /// The next candidate runs in the *same* workspace with the *same* prompt
    /// — nothing about the task changes except which CLI is asked. It carries
    /// the model and effort written beside that candidate in the workflow's
    /// compact selector, never the rejected candidate's: a selector list gives
    /// every fallback its own coherent pair, and composing one candidate's
    /// model onto another provider is exactly the cross-layer mistake
    /// provider resolution exists to prevent.
    NextProviderAfterQuotaRejection {
        rejected_run_id: String,
        candidate: super::ProviderCandidate,
        reason: String,
    },
}

/// Prepare recovery of the latest interrupted run in the task's existing
/// stage and worktree. The provider transcript is preferred; when any shared
/// resume precondition fails, the same preparation produces a fresh session
/// and carries the exact reason into the replacement run record.
pub(crate) fn prepare_resume_task_for_api(
    db: &Db,
    config: &Config,
    task_id: &str,
) -> Result<PreparedStageRunSpawn, String> {
    prepare_stage_restart(
        db,
        config,
        task_id,
        StageRestartIntent::ResumeProviderSession,
    )
}

/// Prepare the one fresh relaunch a rejected Claude resume is allowed: same
/// task, same stage, same worktree, no `--resume`. The replacement is a plain
/// fresh run, so if the provider rejects it too there is nothing left to
/// classify as a resume failure and the retry cannot repeat.
pub(crate) fn prepare_fresh_restart_after_rejected_resume(
    db: &Db,
    config: &Config,
    task_id: &str,
    rejected_run_id: &str,
    reason: &str,
) -> Result<PreparedStageRunSpawn, String> {
    prepare_stage_restart(
        db,
        config,
        task_id,
        StageRestartIntent::FreshAfterRejectedResume {
            rejected_run_id: rejected_run_id.to_string(),
            reason: reason.to_string(),
        },
    )
}

/// Prepare the one automatic attempt a quota rejection is allowed: the stage's
/// next ordered candidate, in the same task, stage and worktree, with that
/// candidate's own model and effort.
///
/// Nothing about the workspace is touched — no reset, no fork, no new task —
/// so the attempt is a replacement, not a replay of anything that ran.
pub(crate) fn prepare_provider_fallback_for_api(
    db: &Db,
    config: &Config,
    task_id: &str,
    rejected_run_id: &str,
    candidate: super::ProviderCandidate,
    reason: &str,
) -> Result<PreparedStageRunSpawn, String> {
    prepare_stage_restart(
        db,
        config,
        task_id,
        StageRestartIntent::NextProviderAfterQuotaRejection {
            rejected_run_id: rejected_run_id.to_string(),
            candidate,
            reason: reason.to_string(),
        },
    )
}

/// How far back a completed-stage lookup will walk before giving up.
///
/// Lineage is data, and data can be wrong: a cycle or a pathological chain
/// must end the walk rather than the process.
const COMPLETED_STAGE_WALK_LIMIT: usize = 32;

/// Does this restart follow a stage that already recorded its verdict, and if
/// so what did it record?
///
/// Recovery is not one hop. A succeeded run can be followed by a recovery run
/// that itself dies, and by a fresh fallback that dies after that; every
/// replacement is a new row, and only walking back through them reaches the
/// verdict. A second reboot used to stop at the first hop, find a `failed`
/// bookkeeping row, and conclude the stage had never succeeded — then hand a
/// finished agent the stage instructions again.
///
/// Runs that recorded no verdict of their own are transparent: still running,
/// or terminated with the session-interruption bookkeeping the resume route
/// writes. The walk stops at the first *genuine* verdict. A real success is
/// the answer; a real failure or cancellation means this stage is being redone
/// deliberately, and no-redo must not apply to it.
///
/// Only `replaces_run_id` carries completion lineage. `resumed_from_run_id`
/// records conversation reuse: a new revision can resume a successful run's
/// conversation while owing a new verdict. Following that pointer would
/// suppress the reviewer's new work. A row without explicit replacement
/// provenance therefore starts its own completion obligation, including a
/// legacy row whose conversation-only link cannot establish recovery intent.
fn resolve_completed_stage(
    db: &Db,
    run: &crate::db::StageRun,
) -> Result<(bool, Option<String>), String> {
    let mut status = run.status.clone();
    let mut result = run.result.clone();
    let mut feedback = run.feedback.clone();
    let mut no_work_termination = run.no_work_termination.clone();
    let mut previous = run.replaces_run_id.clone();
    let mut seen = std::collections::HashSet::new();
    seen.insert(run.id.clone());

    for _ in 0..COMPLETED_STAGE_WALK_LIMIT {
        if status == "succeeded" {
            return Ok((true, result));
        }
        // Producer-declared, never inferred. The legacy feedback marker is
        // still honoured so rows written before the column existed keep
        // working; new rows are classified at the write by all six
        // bookkeeping producers.
        let recorded_no_verdict = matches!(status.as_str(), "running" | "pending")
            || no_work_termination.is_some()
            || feedback.as_deref() == Some(crate::http_api::SESSION_INTERRUPTION_FEEDBACK);
        if !recorded_no_verdict {
            return Ok((false, None));
        }
        let Some(previous_id) = previous else {
            return Ok((false, None));
        };
        if !seen.insert(previous_id.clone()) {
            log::warn!(
                "completed-stage lineage for run {} cycles at {previous_id}",
                run.id
            );
            return Ok((false, None));
        }
        let Some(row) = db
            .stage_run(&previous_id)
            .map_err(|error| format!("db error: {error}"))?
        else {
            return Ok((false, None));
        };
        status = row.status;
        result = row.result;
        feedback = row.feedback;
        no_work_termination = row.no_work_termination;
        previous = row.replaces_run_id;
    }
    log::warn!(
        "completed-stage lineage for run {} exceeded {COMPLETED_STAGE_WALK_LIMIT} hops",
        run.id
    );
    Ok((false, None))
}

fn prepare_stage_restart(
    db: &Db,
    config: &Config,
    task_id: &str,
    intent: StageRestartIntent,
) -> Result<PreparedStageRunSpawn, String> {
    let identity = load_stage_identity(db, task_id)?;
    if identity.source_task.closed_at.is_some() {
        return Err(format!("task is closed: {task_id}"));
    }
    let loaded = load_stage_transition_source(db, config, identity, task_id)?;
    let source_task = &loaded.source_task;
    let run = db
        .latest_stage_run(task_id)
        .map_err(|error| format!("db error: {error}"))?
        .ok_or_else(|| format!("task has no stage run to resume: {task_id}"))?;
    match &intent {
        StageRestartIntent::ResumeProviderSession => {
            // A `succeeded` run is resumable for the same reason a failed one
            // is: the verdict describes the turn that ended, not the session
            // that carried it. A manual stage parks its agent at the composer
            // after recording success, so a daemon death there leaves a task
            // whose conversation is still worth reopening. The caller has
            // already proven the session absent, and the succeeded run keeps
            // its own verdict — the resume records a new run beside it rather
            // than rewriting history as an interruption.
            if !matches!(run.status.as_str(), "cancelled" | "failed" | "succeeded") {
                return Err(format!(
                    "latest run is {}, not cancelled, failed or succeeded: {}",
                    run.status, task_id
                ));
            }
            // A quota refusal recorded against this run is deliberately *not*
            // a reason to refuse the resume. A resume reopens that run's own
            // conversation, which is exactly what an operator wants once the
            // allowance has reset, and it is what the parked action for an
            // attempt that had already changed its workspace tells them to do.
            // Gating it on the refusal — worse, on every provider ever refused
            // at this stage name — disabled resume for the rest of the task's
            // life at that stage, on workflows that name no candidates at all.
            // The refusal is on task detail as `providerRejection`; the
            // decision to reopen it belongs to whoever is reading that.
        }
        // The rejected attempt is deliberately still `running` here: the
        // replacement is prepared before anything is written, so a failed
        // preparation leaves the exit to the caller's normal reporting.
        StageRestartIntent::FreshAfterRejectedResume {
            rejected_run_id, ..
        }
        | StageRestartIntent::NextProviderAfterQuotaRejection {
            rejected_run_id, ..
        } => {
            if &run.id != rejected_run_id {
                return Err(format!(
                    "rejected attempt {rejected_run_id} is no longer the latest run: {task_id}"
                ));
            }
        }
    }
    let item_stage = source_task
        .stage
        .as_deref()
        .ok_or_else(|| format!("task has no stage: {task_id}"))?;
    let current_position = resolve_stage_position(&loaded.workflow, item_stage)
        .ok_or_else(|| format!("stage not found in workflow: {item_stage}"))?;
    let current_owner = match current_position {
        StagePosition::Stage(index) => index,
        StagePosition::Post { owner } => owner,
    };
    let (target_stage, run_kind, run_owner): (WorkflowStage, &'static str, usize) =
        match resolve_stage_position(&loaded.workflow, &run.stage)
            .ok_or_else(|| format!("stage not found in workflow: {}", run.stage))?
        {
            StagePosition::Stage(index) => (loaded.workflow.stages[index].clone(), "main", index),
            StagePosition::Post { owner } => (
                post_as_stage(&loaded.workflow.stages[owner])
                    .ok_or_else(|| format!("stage has no post: {}", run.stage))?,
                "post",
                owner,
            ),
        };
    if run.kind != run_kind || run_owner != current_owner {
        return Err(format!(
            "latest interrupted run is not the task's current stage: {}",
            run.stage
        ));
    }
    let branch = source_task
        .branch
        .as_deref()
        .ok_or_else(|| format!("task has no branch: {task_id}"))?;
    let current_worktree = session::current_workspace_path(db, &loaded.repo.path, task_id, branch);
    let setup_pending = db
        .task_worktree_setup_pending(task_id)
        .map_err(|error| format!("db error: {error}"))?;
    let fallback_workspace = || {
        if std::path::Path::new(&current_worktree).is_dir() {
            if setup_pending {
                RunWorkspaceSpec::FinishRecreate {
                    branch: branch.to_string(),
                    worktree_path: current_worktree.clone(),
                }
            } else {
                RunWorkspaceSpec::Current
            }
        } else {
            RunWorkspaceSpec::Recreate {
                branch: branch.to_string(),
                worktree_path: current_worktree.clone(),
            }
        }
    };
    // A revision directive is authored when the revision run is created. The
    // session-interruption producer preserves it and records the interruption
    // separately in `no_work_termination`; an ordinary interrupted run keeps
    // only the legacy synthetic marker. Carry a genuine directive onto every
    // replacement run so a later fresh fallback or transfer cannot lose it,
    // while never treating a completed run's verdict summary as new work.
    let requested_changes = match &intent {
        StageRestartIntent::FreshAfterRejectedResume { .. }
        | StageRestartIntent::NextProviderAfterQuotaRejection { .. } => run
            .feedback
            .clone()
            .filter(|feedback| feedback != crate::http_api::SESSION_INTERRUPTION_FEEDBACK),
        StageRestartIntent::ResumeProviderSession
            if run.no_work_termination.as_deref()
                == Some(crate::db::no_work_termination::SESSION_INTERRUPTED) =>
        {
            run.feedback
                .clone()
                .filter(|feedback| feedback != crate::http_api::SESSION_INTERRUPTION_FEEDBACK)
        }
        StageRestartIntent::ResumeProviderSession => None,
    };
    // Does this restart follow a stage whose verdict is already recorded? One
    // walk answers it for all three intents, because the answer is a property
    // of the stage's history, not of which intent is asking.
    let (stage_already_succeeded, completed_stage_result) = resolve_completed_stage(db, &run)?;
    let superseded = db
        .stage_run_workflow_superseded(task_id, &run.id)
        .map_err(|error| format!("db error: {error}"))?;
    let resume = if superseded {
        Err("pinned workflow execution binding changed".into())
    } else {
        match &intent {
            StageRestartIntent::FreshAfterRejectedResume { reason, .. } => Err(reason.clone()),
            // A different provider cannot continue the refused one's
            // conversation, and the refused one produced none: this is always
            // a fresh session.
            StageRestartIntent::NextProviderAfterQuotaRejection { reason, .. } => {
                Err(reason.clone())
            }
            StageRestartIntent::ResumeProviderSession => match run.cwd.as_deref() {
                Some(run_cwd)
                    if std::path::Path::new(run_cwd).is_dir()
                        && std::path::Path::new(&current_worktree).is_dir()
                        && !same_cwd(run_cwd, &current_worktree) =>
                {
                    Err(
                        "previous run was recorded in a different worktree than the current stage"
                            .into(),
                    )
                }
                _ => prepare_resume_workspace(
                    run.agent_provider.as_deref(),
                    source_task.agent_type.as_deref(),
                    run.cwd.as_deref(),
                    run.provider_session_id.as_deref(),
                    &run.id,
                    &current_worktree,
                ),
            },
        }
    };
    // Every arm below composes against the same predecessor context, and each
    // one needs the target stage's own resolved agent body: a singleton must
    // carry its manual on *every* spawn of its session, not only on the one
    // whose prompt happened to be composed from the stage definition. Resolve
    // both once.
    //
    // The failed resume/recovery row is bookkeeping, not the active stage's
    // predecessor, so the results are reconstructed from before this
    // replacement lineage; for a just-imported transfer, that falls back to the
    // source-pinned snapshots persisted at import.
    let (prev_result, prev_main_result) = recovery_predecessor_results(db, task_id, &run)?;
    let plan_result = stamped_plan_result(db, task_id);
    let stage_prompt_parts = |task_prompt: &str| -> Result<StagePromptParts, String> {
        build_target_stage_prompt_parts(
            &loaded.definitions,
            &loaded.repo.path,
            &target_stage,
            task_prompt,
            prev_result.as_deref(),
            prev_main_result.as_deref(),
            plan_result.as_deref(),
            Some(branch),
            source_task.base_ref.as_deref(),
            source_task.branch.as_deref(),
            &run.trigger,
            None,
        )
    };
    let (workspace_spec, final_prompt, agent_instructions, resume_fallback_reason) =
        match resume {
            Ok((_provider, mut workspace)) => {
                workspace.repository_setup_pending = setup_pending;
                let workspace = RunWorkspaceSpec::Resume(workspace);
                // The reminder belongs to the run being recovered, not to the
                // durable task that originally created the workflow. Compose the
                // active stage just like a fresh recovery: this preserves its own
                // instructions and only includes `$TASK_PROMPT` when that stage
                // deliberately asks for it. Injecting the task prompt directly can
                // turn a read-only review back into the build assignment.
                let parts = stage_prompt_parts(source_task.prompt.as_deref().unwrap_or(""))?;
                // What the agent is told must match what actually happened to it.
                // A run that recorded success and then lost its PTY has no
                // interrupted work to finish, and telling it otherwise is how a
                // recovered manual stage redoes a stage it already completed.
                if stage_already_succeeded {
                    let message = format!(
                        "Kanna recovered this task after its previous terminal session ended. \
                     The last run already recorded its stage verdict, so there is no \
                     interrupted work to finish and nothing to redo. Continue the existing \
                     task from the preserved conversation and worktree context, and pick up \
                     from wherever that conversation left off. Do not restart the task from \
                     scratch and do not re-record a verdict you have already \
                     recorded.\n\nTask reminder:\n{}",
                        source_task.prompt.as_deref().unwrap_or("")
                    );
                    // This message never carried the stage's instructions, and
                    // neither does the recovery below it — the resumed
                    // conversation was where they lived. That is precisely why a
                    // relocating spawn has to deliver them here: its body is no
                    // longer a message in that conversation to be replayed.
                    let instructions =
                        parts
                            .agent_instructions
                            .map(|body| AgentInstructions::BesideProse {
                                body,
                                inline_prompt: None,
                            });
                    (workspace, message, instructions, None)
                } else {
                    let prose = "Kanna recovered this task after its previous terminal session \
                             ended before a stage verdict was recorded. Continue the existing \
                             task from the preserved conversation and worktree context. Review \
                             the current state, finish the interrupted work, and follow the \
                             stage completion instructions. Do not restart the task from \
                             scratch.";
                    let inline_prompt =
                        format!("{prose}\n\nActive stage instructions:\n{}", parts.prompt);
                    // Embedding the whole composed stage prompt puts the agent
                    // body back into the conversation, where the next compaction
                    // summarizes it away again — the defect this change exists to
                    // fix, on the one agent class it targets. A relocating spawn
                    // is therefore given the same message with the section left
                    // out, and the body beside it; every other spawn keeps the
                    // message exactly as Kanna has always composed it.
                    match parts.agent_instructions.as_deref().and_then(|body| {
                        super::split_agent_instructions_prefix(&parts.prompt, body)
                    }) {
                        Some(remainder) => {
                            let relocated_prompt = if remainder.is_empty() {
                                // The stage composes nothing but its instructions
                                // — the merge master, whose task prompt is empty
                                // by construction. Keeping the heading with
                                // nothing under it would be the only thing the
                                // block said.
                                prose.to_string()
                            } else {
                                format!("{prose}\n\nActive stage instructions:\n{remainder}")
                            };
                            let body = parts.agent_instructions.unwrap_or_default();
                            (
                                workspace,
                                relocated_prompt,
                                Some(AgentInstructions::BesideProse {
                                    body,
                                    inline_prompt: Some(inline_prompt),
                                }),
                                None,
                            )
                        }
                        // The stage resolves to no agent body at all, so there is
                        // nothing to deliver either way.
                        None => (workspace, inline_prompt, None, None),
                    }
                }
            }
            Err(reason) if stage_already_succeeded => {
                // Both fallbacks land here: a transcript that failed preflight, and
                // a resume the provider rejected at runtime. Neither may replay a
                // stage whose verdict is already recorded.
                log::info!(
                    "task resume unavailable for {task_id}: {reason}; \
                 spawning fresh after a recorded success"
                );
                let prompt = build_completed_stage_recovery_prompt(
                    &target_stage.name,
                    &reason,
                    completed_stage_result.as_deref(),
                    source_task.prompt.as_deref().unwrap_or(""),
                );
                // Kanna's own prose again, with the stage's instructions nowhere in
                // it — and this is a fresh conversation, so nothing replays them
                // either. A relocating spawn delivers them as configuration.
                let instructions = stage_prompt_parts(source_task.prompt.as_deref().unwrap_or(""))?
                    .agent_instructions
                    .map(|body| AgentInstructions::BesideProse {
                        body,
                        inline_prompt: None,
                    });
                (fallback_workspace(), prompt, instructions, Some(reason))
            }
            Err(reason) => {
                log::info!("task resume unavailable for {task_id}: {reason}; spawning fresh");
                // A fresh conversation knows only what the prompt tells it. When
                // the interrupted run was a revision, its reviewer feedback is
                // part of what the task is, so it is composed back into the task
                // prompt rather than lost with the transcript.
                let task_prompt = match requested_changes.as_deref() {
                    Some(feedback) => build_revision_task_prompt(
                        source_task.prompt.as_deref().unwrap_or(""),
                        feedback,
                        None,
                    ),
                    None => source_task.prompt.as_deref().unwrap_or("").to_string(),
                };
                // Unlike the two arms above, this one composes the stage prompt
                // bare — no wrapping prose — so it does open with the
                // agent-instructions section. It is also the path a long-lived
                // singleton takes when its session cannot be resumed: the fresh
                // conversation that will compact again. Relocate here too.
                let StagePromptParts {
                    prompt,
                    agent_instructions,
                } = stage_prompt_parts(&task_prompt)?;
                (
                    fallback_workspace(),
                    prompt,
                    agent_instructions.map(AgentInstructions::at_prompt_head),
                    Some(reason),
                )
            }
        };
    let mut prepared = prepare_stage_run_spawn(
        db,
        config,
        &loaded.repo,
        &loaded.definitions,
        task_id,
        &loaded.workflow_name,
        &loaded.workflow,
        &target_stage,
        item_stage,
        run_kind,
        target_stage.policy.transition,
        workspace_spec,
        final_prompt,
        agent_instructions,
        branch,
        // A restarted revision keeps the requested changes on its record, so
        // the run history does not read as an unexplained re-run of the stage.
        requested_changes.clone(),
        source_task.agent_type.as_deref(),
        // Recovery continues the interrupted run: it must respawn with what
        // that run was actually using, not with what the stage would resolve
        // to today. Two exceptions, and only two: an explicit workflow edit
        // has superseded that run's execution binding, or this is a quota
        // fallback — which exists precisely because reproducing that run would
        // ask the same exhausted provider again.
        match &intent {
            StageRestartIntent::NextProviderAfterQuotaRejection { candidate, .. } => {
                SpawnAgentOverrides {
                    provider: Some(candidate.provider.clone()),
                    model: candidate.model.clone(),
                    effort: candidate.effort.clone(),
                }
            }
            _ if superseded => SpawnAgentOverrides::default(),
            _ => SpawnAgentOverrides::from_stage_run(&run),
        },
        source_task.agent_provider.as_deref(),
        stage_trigger_from_stored(Some(&run.trigger)),
        // Reproducing a run reproduces where its provider came from, so the
        // record keeps naming whoever picked this stage's model. A superseded
        // binding names nobody, and neither does a quota fallback: the engine
        // walked to that provider, and stamping it as an explicit override
        // would make the next rerun treat an outage detour as a decision.
        match &intent {
            StageRestartIntent::NextProviderAfterQuotaRejection { .. } => None,
            _ if superseded => None,
            _ => run.provider_override.clone(),
        },
    )?;
    prepared.resume_fallback_reason = resume_fallback_reason;
    // Every restart records what it replaced, whatever workspace it landed in.
    // A fresh fallback resumes nothing, so `resumed_from_run_id` stays null on
    // it and cannot carry this; without a separate pointer the chain back to a
    // recorded verdict breaks at the first fallback.
    prepared.replaces_run_id = Some(run.id.clone());
    Ok(prepared)
}

enum ResumePreparation {
    Resumed(Box<PreparedStageRunSpawn>),
    /// The conversation cannot be resumed; the session starts fresh in the
    /// same revisited directory, whose branch is already reserved.
    Fallback(String, Box<super::types::RevisitWorkspaceSpec>),
}

/// Try to prepare a revision as a resumed run of the target stage's previous
/// provider session, in the stage's revisited directory on its new branch.
/// Every unavailable precondition becomes a durable fresh-spawn reason on the
/// replacement run, which still starts in that directory.
fn prepare_revision_resume(
    db: &Db,
    config: &Config,
    context: &StageTransitionContext<'_>,
    target_stage: &WorkflowStage,
    revision_prompt: &str,
    round: Option<RevisionRound>,
    mut revisit: super::types::RevisitWorkspaceSpec,
) -> Result<ResumePreparation, String> {
    let task_id = context.source_task_id;
    let fall_back = |reason: &str, revisit: super::types::RevisitWorkspaceSpec| {
        log::info!(
            "revision resume unavailable for task {task_id}: {reason}; starting fresh in {}",
            revisit.worktree_path
        );
        Ok(ResumePreparation::Fallback(
            reason.to_string(),
            Box::new(revisit),
        ))
    };

    let run = match db
        .latest_resumable_stage_run(task_id, &target_stage.name)
        .map_err(|e| format!("db error: {}", e))?
    {
        Some(run) => run,
        None => return fall_back("no stage run recorded a provider session", revisit),
    };
    if db
        .stage_run_workflow_superseded(task_id, &run.id)
        .map_err(|error| format!("db error: {error}"))?
    {
        return fall_back("pinned workflow execution binding changed", revisit);
    }
    // Provider transcripts are keyed by working directory, so only a
    // conversation held in this very directory can continue here.
    if !run
        .cwd
        .as_deref()
        .is_some_and(|cwd| same_cwd(cwd, &revisit.worktree_path))
    {
        return fall_back(
            "the stage's latest conversation ran in a different directory",
            revisit,
        );
    }
    let source_task = context.source_task;
    let (provider, provider_session_id) = match prepare_resume_session(
        run.agent_provider.as_deref(),
        source_task.agent_type.as_deref(),
        &revisit.worktree_path,
        run.provider_session_id.as_deref(),
    ) {
        Ok(resume) => resume,
        Err(reason) => return fall_back(&reason, revisit),
    };
    let current_branch_name = revisit.branch.clone();
    let start_point = revisit.start_point.clone();
    revisit.resume = Some(RevisitResume {
        provider_session_id: provider_session_id.clone(),
        resumed_from_run_id: run.id.clone(),
    });

    let message = build_revision_resume_message(
        source_task.prompt.as_deref().unwrap_or(""),
        revision_prompt,
        task_id,
        target_stage.policy.revision_transition(),
        round,
    );
    // A resumed run continues the recorded run's conversation, so it must
    // resolve to that run's provider — never the agent def's priority list —
    // and keep the model and effort that conversation was held with.
    let agent_overrides = SpawnAgentOverrides {
        provider: Some(provider.as_str().to_string()),
        ..SpawnAgentOverrides::from_stage_run(&run)
    };
    let prepared = prepare_stage_run_spawn(
        db,
        config,
        context.repo,
        context.definitions,
        task_id,
        context.workflow_name,
        context.workflow,
        target_stage,
        &target_stage.name,
        "main",
        target_stage.policy.revision_transition(),
        RunWorkspaceSpec::Revisit(revisit),
        message,
        // A revision resume message is a continuation turn, not a composed
        // stage prompt: the session already carries its agent instructions.
        None,
        &current_branch_name,
        Some(revision_prompt.to_string()),
        source_task.agent_type.as_deref(),
        agent_overrides,
        source_task.agent_provider.as_deref(),
        stage_trigger_from_stored(Some(&run.trigger)),
        run.provider_override.clone(),
    )?;
    // A definition that changed provider or session type since the source run
    // cannot continue that conversation. The branch checkout is undone so the
    // fresh start below can make it again.
    if prepared.agent_provider != provider.as_str()
        || prepared.provider_session_id.as_deref() != Some(provider_session_id.as_str())
    {
        let PreparedRunWorkspace::Revisited(revisited) = &prepared.workspace else {
            return Err("revision resume prepared a workspace it did not revisit".to_string());
        };
        if let Some(preserved) =
            super::lifecycle::roll_back_prepared_workspace(&prepared.workspace)?
        {
            return Err(format!(
                "stage no longer resolves to the recorded resumable provider session; {preserved}"
            ));
        }
        let revisit = super::types::RevisitWorkspaceSpec {
            worktree_path: revisited.workspace.worktree_path.clone(),
            branch: revisited.workspace.branch.clone(),
            start_point,
            previous_branch: revisited.previous_branch.clone(),
            previous_head: revisited.previous_head.clone(),
            observed_dirty: revisited.observed_dirty,
            report: prepared.session_identity.workspace_report.clone(),
            resume: None,
        };
        return fall_back(
            "stage no longer resolves to the recorded resumable provider session",
            revisit,
        );
    }
    log::info!(
        "revision resumes task {task_id} stage '{}' from run {} in {} on {}",
        target_stage.name,
        run.id,
        prepared.cwd,
        current_branch_name,
    );
    Ok(ResumePreparation::Resumed(Box::new(prepared)))
}

/// How many agent-requested revision rounds a task has spent, and how many
/// its workflow allows. A `limit` of `0` means the workflow opted out of the
/// cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RevisionBudget {
    pub(crate) rounds: i64,
    pub(crate) limit: i64,
}

/// Effective revision-round cap for a task's pinned workflow.
pub(crate) fn resolve_revision_limit(
    repo: &Repo,
    workflow_name: &str,
    workflow_def: Option<&str>,
) -> Result<i64, String> {
    let workflow = match workflow_def.filter(|value| !value.trim().is_empty()) {
        Some(stored) => parse_stored_workflow_definition(stored)?,
        None => RepoDefinitions::resolve(repo)?.workflow(workflow_name)?,
    };
    Ok(workflow.revision_limit())
}

/// Rounds spent plus the workflow's cap for a task, as the revision endpoint
/// needs them before deciding whether to fork another revision run.
pub(crate) fn resolve_revision_budget(
    db: &Db,
    source_task_id: &str,
) -> Result<RevisionBudget, String> {
    let identity = load_stage_identity(db, source_task_id)?;
    let workflow_name = identity
        .source_task
        .pipeline
        .clone()
        .unwrap_or_else(|| FALLBACK_WORKFLOW_NAME.to_string());
    let limit = resolve_revision_limit(
        &identity.repo,
        &workflow_name,
        identity.source_task.pipeline_def.as_deref(),
    )?;
    let rounds = db
        .task_revision_rounds(source_task_id)
        .map_err(|e| format!("db error: {}", e))?;
    Ok(RevisionBudget { rounds, limit })
}

/// The built-in post whose whole job is handing the finished PR to the repo's
/// merge master. A workflow that declares it on a stage is promising the
/// handoff, which is what lets the engine notice when the post finished
/// without delivering one.
const MERGE_APPROVE_POST: &str = "approve";

/// True when the task's pinned stage declares the merge-signaling `approve`
/// post. Pre-change snapshots and custom workflows without that post promise
/// no merge side effect, so nothing may be enforced on their behalf.
pub(crate) fn stage_declares_merge_approve_post(
    repo: &Repo,
    workflow_name: &str,
    workflow_def: Option<&str>,
    stage_name: &str,
) -> Result<bool, String> {
    let workflow = match workflow_def.filter(|value| !value.trim().is_empty()) {
        Some(stored) => parse_stored_workflow_definition(stored)?,
        None => RepoDefinitions::resolve(repo)?.workflow(workflow_name)?,
    };
    let owner = match resolve_stage_position(&workflow, stage_name) {
        Some(StagePosition::Stage(index)) => index,
        Some(StagePosition::Post { owner }) => owner,
        None => return Ok(false),
    };
    Ok(workflow.stages[owner].post.as_ref().is_some_and(|post| {
        post.name == MERGE_APPROVE_POST || post.agent.as_deref() == Some(MERGE_APPROVE_POST)
    }))
}

pub(crate) fn resolve_stage_transition(
    repo: &Repo,
    workflow_name: &str,
    workflow_def: Option<&str>,
    stage_name: &str,
) -> Result<Option<String>, String> {
    let workflow = match workflow_def.filter(|value| !value.trim().is_empty()) {
        Some(stored) => parse_stored_workflow_definition(stored)?,
        None => RepoDefinitions::resolve(repo)?.workflow(workflow_name)?,
    };
    Ok(match resolve_stage_position(&workflow, stage_name) {
        Some(StagePosition::Stage(index)) => Some(
            workflow.stages[index]
                .policy
                .transition
                .as_str()
                .to_string(),
        ),
        // A post always advances on success.
        Some(StagePosition::Post { .. }) => {
            Some(WorkflowStageTransition::Auto.as_str().to_string())
        }
        None => None,
    })
}

// Shared with notification enrichment: an auto main completion dispatches a
// post or enters a successor, but never closes a final stage without a post.
fn main_completion_has_continuation(workflow: &WorkflowDefinition, index: usize) -> bool {
    workflow.stages[index].post.is_some() || workflow.stages.get(index + 1).is_some()
}

pub(crate) fn main_completion_continuation(
    db: &Db,
    task_id: &str,
    stage: &str,
) -> Result<Option<bool>, String> {
    let identity = load_stage_identity(db, task_id)?;
    let source = &identity.source_task;
    let workflow = match source
        .pipeline_def
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(stored) => parse_stored_workflow_definition(stored)?,
        None => RepoDefinitions::resolve(&identity.repo)?
            .workflow(source.pipeline.as_deref().unwrap_or(FALLBACK_WORKFLOW_NAME))?,
    };
    Ok(match resolve_stage_position(&workflow, stage) {
        Some(StagePosition::Stage(index)) => {
            Some(main_completion_has_continuation(&workflow, index))
        }
        Some(StagePosition::Post { .. }) => Some(true),
        None => None,
    })
}

/// Where a result goes under named-exit routing (spec §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedResultExit {
    /// The exit taken or requested: `advance` or a declared loop exit.
    pub(crate) exit: String,
    /// [`TransitionExit::EXPLICIT`] when the result named it,
    /// [`TransitionExit::DEFAULT`] when it named none.
    pub(crate) source: &'static str,
    /// The loop exit's destination stage; `None` for `advance`.
    pub(crate) destination: Option<String>,
    /// The destination's budget; `None` for `advance`.
    pub(crate) budget_limit: Option<i64>,
}

impl ResolvedResultExit {
    /// The transition record for this exit, with the budget it spent.
    pub(crate) fn transition_exit(
        &self,
        budget: Option<crate::db::StageBudgetSpend>,
    ) -> TransitionExit {
        TransitionExit {
            exit: Some(self.exit.clone()),
            source: self.source.to_string(),
            budget,
        }
    }
}

/// The workflow a task routes by: its pinned snapshot, else the named one.
fn task_workflow_for_routing(db: &Db, task_id: &str) -> Result<WorkflowDefinition, String> {
    let identity = load_stage_identity(db, task_id)?;
    let source = &identity.source_task;
    match source
        .pipeline_def
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(stored) => parse_stored_workflow_definition(stored),
        None => RepoDefinitions::resolve(&identity.repo)?
            .workflow(source.pipeline.as_deref().unwrap_or(FALLBACK_WORKFLOW_NAME)),
    }
}

/// True when the task's workflow routes results by named exits.
pub(crate) fn task_routes_by_exits(db: &Db, task_id: &str) -> Result<bool, String> {
    Ok(task_workflow_for_routing(db, task_id)?.routes_by_exits())
}

/// Resolve the exit a result on `run` names (or the default it takes).
///
/// `Ok(None)` for a legacy-routed task that named no exit: the legacy
/// adapter routes it exactly as before. A legacy task naming an exit is
/// refused, since its workflow declares none. `publishing` is the definition
/// the same call publishes, which the result then routes by. Only a main run
/// of the task's current stage may name a loop exit; a post always advances.
pub(crate) fn resolve_result_exit(
    db: &Db,
    task_id: &str,
    run: &crate::db::StageRun,
    requested: Option<&str>,
    publishing: Option<&serde_json::Value>,
) -> Result<Option<ResolvedResultExit>, String> {
    let workflow = match publishing {
        Some(definition) => parse_workflow_definition(&definition.to_string())?,
        None => task_workflow_for_routing(db, task_id)?,
    };
    let requested = requested.map(str::trim).filter(|exit| !exit.is_empty());
    if !workflow.routes_by_exits() {
        return match requested {
            None => Ok(None),
            Some(exit) => Err(format!(
                "this task's workflow does not route by named exits, so a result cannot name \
                 exit '{exit}'; record the result without an exit (a review asks for changes \
                 through kanna_request_revision naming the stage)"
            )),
        };
    }
    let Some(exit) = requested else {
        return Ok(Some(ResolvedResultExit {
            exit: ADVANCE_EXIT.to_string(),
            source: TransitionExit::DEFAULT,
            destination: None,
            budget_limit: None,
        }));
    };
    let stage = db
        .get_pipeline_item(task_id)
        .map_err(|error| format!("db error: {error}"))?
        .and_then(|item| item.stage)
        .ok_or_else(|| format!("task has no stage: {task_id}"))?;
    if exit != ADVANCE_EXIT && (run.kind != "main" || run.stage != stage) {
        return Err(format!(
            "only the main run of stage '{stage}' may name a loop exit; this is the {} run of \
             '{}', which can only advance",
            run.kind, run.stage
        ));
    }
    let destination = workflow.resolve_exit(&stage, exit)?;
    let budget_limit = destination
        .as_deref()
        .map(|destination| workflow.stage_budget(destination));
    Ok(Some(ResolvedResultExit {
        exit: exit.to_string(),
        source: TransitionExit::EXPLICIT,
        destination,
        budget_limit,
    }))
}

/// For a person sending a named-exit task from `from_stage` back to
/// `destination`: the exit of `from_stage` that leads there, if one does.
pub(crate) fn exit_leading_to(
    db: &Db,
    task_id: &str,
    from_stage: &str,
    destination: &str,
) -> Result<Option<String>, String> {
    let workflow = task_workflow_for_routing(db, task_id)?;
    Ok(workflow
        .stages
        .iter()
        .find(|stage| stage.name == from_stage)
        .and_then(|stage| {
            stage
                .exits
                .iter()
                .flatten()
                .find(|(_, target)| target.as_str() == destination)
                .map(|(name, _)| name.clone())
        }))
}

/// The exits a named-exit task's current stage offers, for a refusal that
/// tells an agent what it may name instead.
pub(crate) fn describe_current_stage_exits(db: &Db, task_id: &str) -> Result<String, String> {
    let workflow = task_workflow_for_routing(db, task_id)?;
    let stage = db
        .get_pipeline_item(task_id)
        .map_err(|error| format!("db error: {error}"))?
        .and_then(|item| item.stage)
        .ok_or_else(|| format!("task has no stage: {task_id}"))?;
    Ok(workflow
        .stages
        .iter()
        .find(|candidate| candidate.name == stage)
        .map(describe_stage_exits)
        .unwrap_or_else(|| format!("'{ADVANCE_EXIT}'")))
}

/// The budget of `stage` in a named-exit task's workflow.
pub(crate) fn resolve_stage_budget_limit(
    db: &Db,
    task_id: &str,
    stage: &str,
) -> Result<i64, String> {
    Ok(task_workflow_for_routing(db, task_id)?.stage_budget(stage))
}
