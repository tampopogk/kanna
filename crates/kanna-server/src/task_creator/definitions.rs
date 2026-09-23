#[path = "doctor.rs"]
pub(crate) mod doctor;

use super::definition_source::{OriginFreshness, RepoDefinitionSnapshot};
use super::local_config::{apply_local_config_override, LocalConfigOverride};
use crate::db::Repo;
use kanna_agent_protocol::{validate_agent_selection, AgentSelectionEntry};
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct RepoConfig {
    #[serde(alias = "workflow", skip_serializing_if = "Option::is_none")]
    pub(super) workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) setup: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) teardown: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) test: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ports: Option<HashMap<String, u16>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) flavors: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) vars: Option<HashMap<String, String>>,
    #[serde(
        rename = "agentProviders",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_agent_provider_preferences"
    )]
    pub(super) agent_providers: Option<BTreeMap<String, AgentProviderPreference>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) reserved_ports: Vec<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) reserved_port_offsets: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stage_order: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) workspace: Option<RepoWorkspaceConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) artifacts: Option<RepoArtifactsConfig>,
    /// Provenance of the machine-local `.kanna/config.local.json` layer merged
    /// over the committed config, or `None` when no local file applies. It is
    /// recorded during resolution rather than read from either file, so it
    /// always describes the configuration actually in force.
    #[serde(
        rename = "localOverride",
        default,
        skip_deserializing,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) local_override: Option<LocalConfigOverride>,
}

/// Where this repository's artifact repository lives and which retention
/// policy new artifact versions record (spec §8). Both fields are optional;
/// the artifact module supplies the defaults.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct RepoArtifactsConfig {
    #[serde(
        rename = "repositoryPath",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(super) repository_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) retention: Option<crate::artifacts::ArtifactRetention>,
    /// The artifact remote: a Git URL or path both sharing homes can reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) remote: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct AgentProviderPreference {
    #[serde(rename = "provider")]
    pub(super) providers: Vec<AgentSelectionEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) effort: Option<String>,
    /// Claude's auto-compact window, for the provider this entry selects.
    /// Like `model` and `effort` it belongs to the *first* name in
    /// `provider`; a fallback candidate behind it is a different harness and
    /// takes nothing from here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) autocompact: Option<String>,
}

impl<'de> Deserialize<'de> for AgentProviderPreference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        parse_agent_provider_preference(&value)
            .ok_or_else(|| serde::de::Error::custom("invalid agent provider preference"))
    }
}

impl RepoConfig {
    /// Resolve a repo-level provider/model preference for an agent selector.
    ///
    /// Exact names win. Otherwise, the glob with the most non-wildcard
    /// characters wins; equally specific globs use lexical order so JSON map
    /// ordering never affects resolution.
    pub(super) fn agent_provider_preference(
        &self,
        agent_selector: Option<&str>,
    ) -> Option<&AgentProviderPreference> {
        let selector = agent_selector?.trim();
        if selector.is_empty() {
            return None;
        }
        let preferences = self.agent_providers.as_ref()?;
        if let Some(preference) = preferences.get(selector) {
            return Some(preference);
        }
        let compatible_selectors = agent_repo_dirs(selector);
        for alias in compatible_selectors.iter().skip(1) {
            if let Some(preference) = preferences.get(alias) {
                return Some(preference);
            }
        }

        preferences
            .iter()
            .filter(|(pattern, _)| {
                pattern.contains('*')
                    && compatible_selectors
                        .iter()
                        .any(|name| wildcard_matches(pattern, name))
            })
            .min_by(|(left, _), (right, _)| compare_agent_provider_globs(left, right))
            .map(|(_, preference)| preference)
    }
}

fn compare_agent_provider_globs(left: &str, right: &str) -> Ordering {
    let specificity = |pattern: &str| pattern.bytes().filter(|byte| *byte != b'*').count();
    specificity(right)
        .cmp(&specificity(left))
        .then_with(|| left.cmp(right))
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut pattern_index, mut value_index) = (0, 0);
    let mut wildcard_index = None;
    let mut wildcard_value_index = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == value[value_index] {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            wildcard_index = Some(pattern_index);
            pattern_index += 1;
            wildcard_value_index = value_index;
        } else if let Some(wildcard) = wildcard_index {
            pattern_index = wildcard + 1;
            wildcard_value_index += 1;
            value_index = wildcard_value_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct RepoWorkspaceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) env: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) path: Option<RepoWorkspacePathConfig>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct RepoWorkspacePathConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prepend: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) append: Option<Vec<String>>,
}

/// Whether a definition is offered as a *choice*. Declared by the definition
/// itself — a top-level `visibility` field in a workflow JSON, a `visibility`
/// key in AGENT.md frontmatter — and defaulting to public when absent.
///
/// `internal` keeps the name out of every listing (`workflow_names()`,
/// `agents()`, and everything built on them: the repo manifest, the desktop's
/// new-task picker, `kanna_list_agents`) because Kanna binds the definition
/// itself and offering it only invites picking it by mistake. Visibility is
/// not access control: resolution by explicit name never consults it, so the
/// dispatcher naming `specialty-review`, the task manager naming
/// `architect-research`, and a stage post binding `commit` keep working
/// unchanged.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum DefinitionVisibility {
    #[default]
    Public,
    Internal,
}

impl DefinitionVisibility {
    fn is_public(&self) -> bool {
        matches!(self, Self::Public)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct WorkflowDefinition {
    #[allow(dead_code)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    pub(super) stages: Vec<WorkflowStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) environments: Option<HashMap<String, WorkflowEnvironment>>,
    /// Cap on agent-requested revision rounds per task before the task parks
    /// for its human instead of looping. Omitted means
    /// `DEFAULT_REVISION_LIMIT`; `0` means unlimited. Pinned `workflow_def`
    /// snapshots written before this field existed omit it and therefore
    /// inherit the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) revision_limit: Option<i64>,
    /// Listing-only: `internal` keeps this workflow out of `workflow_names()`.
    /// Resolution never consults it. See `DefinitionVisibility`.
    #[serde(default, skip_serializing_if = "DefinitionVisibility::is_public")]
    pub(super) visibility: DefinitionVisibility,
    /// Plan published by an earlier stage of *this task*, stamped by the
    /// server when a plan stage completes and appends the remaining stages in
    /// one operation. Never authored by a caller. It rides inside the pinned
    /// workflow so the approved plan survives later stages, revisions,
    /// resume, and recovery without a second durable record, and it binds
    /// `$PLAN_RESULT` for every stage and post of the extended workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) plan_context: Option<WorkflowPlanContext>,
    /// How results route (spec §5). Absent means legacy, so every snapshot
    /// pinned before named exits existed reads, serializes and routes exactly
    /// as it did.
    #[serde(default, skip_serializing_if = "WorkflowRouting::is_legacy")]
    pub(super) routing: WorkflowRouting,
    /// Routing `exits` only: the budget of a stage that declares none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) budget: Option<i64>,
}

/// How a stage's result chooses where the task goes.
///
/// `Legacy` is today's contract: success follows the stage's transition
/// policy, and a reviewer names a target *stage* through the revision API
/// under one task-wide round budget. `Exits` is the target contract: a result
/// names one of its stage's declared exits (or none, taking the default), and
/// each loop spends its destination stage's own budget. It is opt-in per
/// workflow, so a pinned legacy task keeps the adapter it was started under.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowRouting {
    #[default]
    Legacy,
    Exits,
}

impl WorkflowRouting {
    fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy)
    }
}

/// The exit every stage has: the next stage, under the stage's transition
/// policy. Never declared, so a workflow cannot remap it.
pub(crate) const ADVANCE_EXIT: &str = "advance";

/// Agent-chosen loops into a stage before a further one parks the task, for
/// a routing `exits` workflow that sets no budget (spec §5).
pub(crate) const DEFAULT_STAGE_BUDGET: i64 = 5;

/// The stamped plan carried by an extended workflow. `result` is the full
/// recorded stage result of the publishing run, in the same shape
/// `$PREV_MAIN_RESULT` carries, so a stage prompt can read either without
/// learning a second format.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WorkflowPlanContext {
    pub(crate) source_run_id: String,
    pub(crate) stage: String,
    pub(crate) result: String,
    /// Fingerprint of the exact combined completion that published these
    /// stages. The recorded result alone cannot identify that operation: two
    /// completions can carry the same summary and publish different stages, so
    /// without this a retry with a different suffix reads as a replay of the
    /// first. Optional because a snapshot stamped before it existed has none;
    /// such a stamp cannot confirm a replay and is treated as "not this
    /// request".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request_digest: Option<String>,
}

/// Rounds of agent-requested revision a task gets before the engine stops
/// forking new work and parks the task for its human. A review agent that
/// keeps finding new work each round is the mechanism by which a scoped task
/// turns into an open-ended project, so the loop is bounded by default.
pub(crate) const DEFAULT_REVISION_LIMIT: i64 = 5;

impl WorkflowDefinition {
    /// Effective revision-round cap: `0` means unlimited. Negative values are
    /// rejected when the definition is parsed, so there is nothing to clamp
    /// here — silently clamping would turn a typo into an unbounded loop,
    /// which is the failure this cap exists to prevent.
    pub(super) fn revision_limit(&self) -> i64 {
        self.revision_limit.unwrap_or(DEFAULT_REVISION_LIMIT)
    }

    /// True when `stage` has no role under named-exit routing (spec §5). A
    /// legacy stage without `agent` runs the default agent, as it always has.
    pub(crate) fn is_roleless_stage(&self, stage: &WorkflowStage) -> bool {
        self.routes_by_exits() && stage.is_roleless()
    }

    /// True when results route by named exits rather than the legacy
    /// revision adapter.
    pub(crate) fn routes_by_exits(&self) -> bool {
        self.routing == WorkflowRouting::Exits
    }

    /// Loops back into `stage_name` the task may take before a further one
    /// parks it: the stage's own budget, else the workflow default, else 5.
    pub(crate) fn stage_budget(&self, stage_name: &str) -> i64 {
        self.stages
            .iter()
            .find(|stage| stage.name == stage_name)
            .and_then(|stage| stage.budget)
            .or(self.budget)
            .unwrap_or(DEFAULT_STAGE_BUDGET)
    }

    /// Where `exit` leads from `stage_name`: `Ok(None)` for `advance` (the
    /// next stage, under the transition policy), `Ok(Some(destination))` for a
    /// declared loop exit, and an error naming the stage's exits otherwise.
    pub(crate) fn resolve_exit(
        &self,
        stage_name: &str,
        exit: &str,
    ) -> Result<Option<String>, String> {
        if exit == ADVANCE_EXIT {
            return Ok(None);
        }
        let stage = self
            .stages
            .iter()
            .find(|stage| stage.name == stage_name)
            .ok_or_else(|| format!("stage '{stage_name}' is not a stage of this workflow"))?;
        stage
            .exits
            .as_ref()
            .and_then(|exits| exits.get(exit))
            .cloned()
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "stage '{stage_name}' declares no exit '{exit}'; its exits are {}",
                    describe_stage_exits(stage)
                )
            })
    }
}

/// `advance`, then every declared loop exit with its destination.
pub(crate) fn describe_stage_exits(stage: &WorkflowStage) -> String {
    std::iter::once(format!("'{ADVANCE_EXIT}'"))
        .chain(
            stage
                .exits
                .iter()
                .flatten()
                .map(|(name, destination)| format!("'{name}' (to '{destination}')")),
        )
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct WorkflowStage {
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) agent_provider: Option<Vec<AgentSelectionEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) environment: Option<String>,
    /// Routing `exits` only: loop exits by name, each mapped to this stage or
    /// an earlier one. `advance` is implicit and never listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) exits: Option<BTreeMap<String, String>>,
    /// Routing `exits` only: agent-chosen loops into this stage before a
    /// further one parks the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) budget: Option<i64>,
    pub(super) policy: WorkflowStagePolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) post: Option<WorkflowPost>,
    /// Routing `exits` only: this stage's forward transition starts with the
    /// commit step (spec §5) — the live session is told to commit and record
    /// its result, or a short commit session runs in the same workspace when
    /// it is dead — and the transition fires on that result.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) exit_commit: bool,
    /// Routing `exits` only: commands run in the stage's workspace when the
    /// stage is entered, after the environment's setup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) setup: Option<Vec<String>>,
    /// Routing `exits` only: commands run in the stage's workspace when the
    /// task leaves it, after the environment's teardown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) teardown: Option<Vec<String>>,
}

/// Name, agent and prompt of the commit step a stage's `exit_commit` adds to
/// its forward transition. The name is the run-history label of its run,
/// unique per stage so recovery resolves the run back to its own stage. It runs through the post delivery machinery (live
/// session first, a fresh commit session in the same workspace when the
/// session is dead), but it is a phase of the transition, not a declared post.
pub(super) fn commit_step_name(stage: &str) -> String {
    format!("{stage} commit")
}
pub(super) const COMMIT_STEP_AGENT: &str = "commit";
pub(super) const COMMIT_STEP_PROMPT: &str = "The task is leaving this stage. Commit the work \
     that belongs to this task in this workspace now (leave unrelated local changes alone), then \
     record your result again: its message is what the next stage receives, so carry forward \
     what that stage must know about this stage's work and say what you committed. Record \
     `failure` if task work remains that you cannot safely commit; the task then stays here.";

impl WorkflowStage {
    /// Names no agent. Only a named-exit workflow reads that as a stage with
    /// no role; see [`WorkflowDefinition::is_roleless_stage`].
    fn is_roleless(&self) -> bool {
        self.agent.is_none()
    }

    /// The work a forward transition out of this stage runs in the stage's
    /// session before it fires: the declared post, or the commit step that
    /// `exit_commit` asks for. A workflow cannot declare both.
    pub(super) fn transition_post(&self) -> Option<std::borrow::Cow<'_, WorkflowPost>> {
        if let Some(post) = self.post.as_ref() {
            return Some(std::borrow::Cow::Borrowed(post));
        }
        self.exit_commit.then(|| {
            std::borrow::Cow::Owned(WorkflowPost {
                name: commit_step_name(&self.name),
                description: Some("Commit step of this stage's transition".to_string()),
                agent: Some(COMMIT_STEP_AGENT.to_string()),
                prompt: Some(COMMIT_STEP_PROMPT.to_string()),
                agent_provider: None,
            })
        })
    }

    /// Setup commands entering this stage runs: its environment's, then its
    /// own.
    pub(super) fn setup_commands(&self, workflow: &WorkflowDefinition) -> Vec<String> {
        let mut commands = self
            .environment
            .as_deref()
            .and_then(|name| workflow.environments.as_ref()?.get(name))
            .and_then(|environment| environment.setup.clone())
            .unwrap_or_default();
        commands.extend(self.setup.iter().flatten().cloned());
        commands
    }

    /// Teardown commands leaving this stage runs: its environment's, then its
    /// own.
    pub(super) fn teardown_commands(&self, workflow: &WorkflowDefinition) -> Vec<String> {
        let mut commands = self
            .environment
            .as_deref()
            .and_then(|name| workflow.environments.as_ref()?.get(name))
            .and_then(|environment| environment.teardown.clone())
            .unwrap_or_default();
        commands.extend(self.teardown.iter().flatten().cloned());
        commands
    }
}

/// Tail work of a stage, injected into the stage's running agent session when
/// the stage transitions forward. `agent` is the fallback used to spawn a
/// fresh session (and the prompt-body source) when the task's session is
/// dead; a live session keeps whatever agent is already running.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct WorkflowPost {
    pub(super) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) agent_provider: Option<Vec<AgentSelectionEntry>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct WorkflowStagePolicy {
    pub(super) transition: WorkflowStageTransition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) revision_transition: Option<WorkflowStageTransition>,
    /// Routing `exits` only: how a stage re-entered by a loop leaves through
    /// `advance`. Never set together with `revision_transition`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) loop_transition: Option<WorkflowStageTransition>,
    /// Routing `exits` only, final stage only: leaving the stage hands the
    /// task's pull request to the repository's merge master (spec §10, "the
    /// `pr` stage's `advance` hands to it"). It delivers the request a legacy
    /// `approve` post sends, through the same pre-close backstop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) handoff: Option<WorkflowHandoff>,
}

/// Who a stage's transition hands the task's work to. The merge master is the
/// only receiver this build has.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkflowHandoff {
    Merge,
}

impl WorkflowStagePolicy {
    /// How a run entered by a loop (a legacy revision, or a loop exit) leaves
    /// the stage. The two fields belong to different routing contracts and
    /// are never both set.
    pub(super) fn revision_transition(&self) -> WorkflowStageTransition {
        self.revision_transition
            .or(self.loop_transition)
            .unwrap_or(self.transition)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum WorkflowStageTransition {
    Manual,
    Auto,
}

impl WorkflowStageTransition {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct WorkflowEnvironment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) setup: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) teardown: Option<Vec<String>>,
}

/// Where a stored stage name sits in a workflow. In-flight tasks created
/// before posts replaced interleaved continue stages can be parked *at* a
/// folded post name (e.g. `commit`); those resolve to the owning stage's
/// post rather than erroring.
pub(super) enum StagePosition {
    Stage(usize),
    Post { owner: usize },
}

pub(super) fn resolve_stage_position(
    workflow: &WorkflowDefinition,
    stage_name: &str,
) -> Option<StagePosition> {
    if let Some(index) = workflow
        .stages
        .iter()
        .position(|stage| stage.name == stage_name)
    {
        return Some(StagePosition::Stage(index));
    }
    workflow
        .stages
        .iter()
        .position(|stage| {
            stage
                .transition_post()
                .is_some_and(|post| post.name == stage_name)
        })
        .map(|owner| StagePosition::Post { owner })
}

/// A stage's post viewed as a stage: the shape `prepare_stage_run_spawn` and
/// prompt building consume for dead-session fallbacks and legacy in-flight
/// tasks parked at a folded post name. Post success always advances, so the
/// synthetic policy is `auto`.
pub(super) fn post_as_stage(owner: &WorkflowStage) -> Option<WorkflowStage> {
    owner.transition_post().map(|post| WorkflowStage {
        name: post.name.clone(),
        description: post.description.clone(),
        agent: post.agent.clone(),
        prompt: post.prompt.clone(),
        agent_provider: post.agent_provider.clone(),
        environment: owner.environment.clone(),
        exits: None,
        budget: None,
        policy: WorkflowStagePolicy {
            transition: WorkflowStageTransition::Auto,
            revision_transition: None,
            loop_transition: None,
            handoff: None,
        },
        post: None,
        exit_commit: false,
        setup: None,
        teardown: None,
    })
}

#[derive(Deserialize)]
struct RawWorkflowDefinition {
    name: Option<String>,
    description: Option<String>,
    stages: Vec<RawWorkflowStage>,
    environments: Option<HashMap<String, WorkflowEnvironment>>,
    revision_limit: Option<i64>,
    #[serde(default)]
    visibility: DefinitionVisibility,
    #[serde(default)]
    plan_context: Option<WorkflowPlanContext>,
    #[serde(default)]
    routing: WorkflowRouting,
    budget: Option<i64>,
}

#[derive(Deserialize)]
struct RawWorkflowStage {
    name: String,
    description: Option<String>,
    agent: Option<String>,
    prompt: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_provider_list")]
    agent_provider: Option<Vec<AgentSelectionEntry>>,
    environment: Option<String>,
    exits: Option<BTreeMap<String, String>>,
    budget: Option<i64>,
    policy: Option<RawWorkflowStagePolicy>,
    transition: Option<WorkflowStageTransition>,
    mode: Option<RawWorkflowStageExecution>,
    post: Option<RawWorkflowPost>,
    post_action: Option<RawWorkflowPostAction>,
    #[serde(default)]
    exit_commit: bool,
    setup: Option<Vec<String>>,
    teardown: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawWorkflowStagePolicy {
    transition: WorkflowStageTransition,
    revision_transition: Option<WorkflowStageTransition>,
    loop_transition: Option<WorkflowStageTransition>,
    handoff: Option<WorkflowHandoff>,
    execution: Option<RawWorkflowStageExecution>,
}

#[derive(Deserialize)]
struct RawWorkflowPost {
    name: String,
    description: Option<String>,
    agent: Option<String>,
    prompt: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_provider_list")]
    agent_provider: Option<Vec<AgentSelectionEntry>>,
}

#[derive(Deserialize)]
struct RawWorkflowPostAction {
    name: String,
    description: Option<String>,
    agent: Option<String>,
    prompt: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_provider_list")]
    agent_provider: Option<Vec<AgentSelectionEntry>>,
    #[allow(dead_code)]
    transition: Option<WorkflowStageTransition>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawWorkflowStageExecution {
    NewTask,
    Continue,
}

#[derive(Default, Deserialize)]
struct AgentFrontmatter {
    name: Option<String>,
    description: Option<String>,
    /// Definition-formula alias for `description` (spec §12): "one sentence"
    /// naming the role. `description` wins when both are present; a
    /// definition that declares `role` opts into the formula's line-count and
    /// four-section shape, checked by `check_definition_formula`.
    role: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_yaml_value")]
    agent_provider: Option<YamlValue>,
    /// Definition-formula alias for `agent_provider` ("ordered candidates").
    /// `agent_provider` wins when both are present.
    #[serde(default, deserialize_with = "deserialize_optional_yaml_value")]
    providers: Option<YamlValue>,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
    allowed_tools: Option<Vec<String>>,
    visibility: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct AgentDefinition {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) prompt: String,
    #[serde(rename = "agent_provider", skip_serializing_if = "Vec::is_empty")]
    pub(super) agent_providers: Vec<AgentSelectionEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) allowed_tools: Vec<String>,
    /// Listing-only: `internal` keeps this agent out of `agents()`. Resolution
    /// by explicit name never consults it. See `DefinitionVisibility`.
    #[serde(skip_serializing_if = "DefinitionVisibility::is_public")]
    pub(super) visibility: DefinitionVisibility,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AgentDefinitionSource {
    BuiltIn,
    RepoOverride,
    RepoAuthored,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResolvedAgentDefinition {
    pub(super) name: String,
    pub(super) description: String,
    pub(super) default_provider: Option<String>,
    pub(super) default_model: Option<String>,
    pub(super) default_effort: Option<String>,
    pub(super) source: AgentDefinitionSource,
}

struct AgentExtension {
    prompt: String,
    description: Option<String>,
    agent_providers: Option<Vec<AgentSelectionEntry>>,
    model: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
    allowed_tools: Option<Vec<String>>,
    visibility: Option<DefinitionVisibility>,
}

pub(super) struct RepoDefinitions {
    snapshot: RepoDefinitionSnapshot,
    config: RepoConfig,
}

impl RepoDefinitions {
    /// Resolve against a freshly fetched `origin`. This is the authoritative
    /// read: callers that pin a workflow onto a task or fork a workspace must
    /// see the real remote tip, and may wait for the network to report it.
    pub(super) fn resolve(repo: &Repo) -> Result<Self, String> {
        Self::resolve_path(
            &repo.path,
            repo.default_branch.as_deref(),
            OriginFreshness::Fetch,
        )
        .map_err(|error| repo_definition_resolution_error(repo, error))
    }

    /// Resolve against the remote-tracking refs already on disk. Reads that
    /// only display definitions take this path, so opening a picker or
    /// refreshing the sidebar never blocks on `git fetch`.
    pub(super) fn resolve_local(repo: &Repo) -> Result<Self, String> {
        Self::resolve_path(
            &repo.path,
            repo.default_branch.as_deref(),
            OriginFreshness::Local,
        )
        .map_err(|error| repo_definition_resolution_error(repo, error))
    }

    fn resolve_path(
        repo_path: &str,
        default_branch: Option<&str>,
        freshness: OriginFreshness,
    ) -> Result<Self, String> {
        let snapshot = RepoDefinitionSnapshot::resolve(repo_path, default_branch, freshness)?;
        let config_path = ".kanna/config.json";
        let mut raw_config = match read_snapshot_utf8(&snapshot, config_path)? {
            Some(content) => parse_config_object(&content)
                .map_err(|error| definition_error(&snapshot, config_path, error))?,
            None => serde_json::Map::new(),
        };
        // Agents, workflows, and the committed config come from the origin
        // snapshot; only this one file comes from the working tree, because a
        // per-machine override that needed a commit would not be one.
        let local_override = apply_local_config_override(Path::new(repo_path), &mut raw_config)?;
        if let Some(local) = &local_override {
            log::info!(
                "repo config for `{repo_path}` layers `{}` over `{config_path}` from `{}` at revision `{}` (keys: {})",
                local.path(),
                snapshot.ref_name(),
                snapshot.revision().unwrap_or("<none>"),
                local.keys().join(", "),
            );
        }
        validate_structured_preferences(&raw_config)
            .map_err(|error| definition_error(&snapshot, config_path, error))?;
        let mut config = repo_config_from_object(&raw_config);
        config.local_override = local_override;
        Ok(Self { snapshot, config })
    }

    pub(super) fn revision(&self) -> Option<&str> {
        self.snapshot.revision()
    }

    pub(super) fn ref_name(&self) -> &str {
        self.snapshot.ref_name()
    }

    pub(super) fn config(&self) -> &RepoConfig {
        &self.config
    }

    pub(super) fn workflow(&self, name: &str) -> Result<WorkflowDefinition, String> {
        self.workflow_optional(name)?.ok_or_else(|| {
            if let Some(agent) = name.strip_prefix("singleton-") {
                return format!(
                    "{name} is a synthetic singleton workflow; create or recover it with \
                     kanna_signal_agent (agent: {agent}) or, for merge, kanna_signal_merge_handoff; \
                     these operations atomically claim account-wide ownership"
                );
            }
            format!("compiled resource not found: .kanna/workflows/{name}.json")
        })
    }

    pub(super) fn workflow_optional(
        &self,
        name: &str,
    ) -> Result<Option<WorkflowDefinition>, String> {
        let path = format!(".kanna/workflows/{name}.json");
        match read_snapshot_utf8(&self.snapshot, &path)? {
            Some(content) => parse_workflow_definition(&content)
                .map(Some)
                .map_err(|error| definition_error(&self.snapshot, &path, error)),
            None => {
                // Repositories created before the terminology rename may
                // still carry `.kanna/pipelines`. A canonical workflow file
                // always wins when both exist.
                let legacy_path = format!(".kanna/pipelines/{name}.json");
                match read_snapshot_utf8(&self.snapshot, &legacy_path)? {
                    Some(content) => parse_workflow_definition(&content)
                        .map(Some)
                        .map_err(|error| definition_error(&self.snapshot, &legacy_path, error)),
                    None => compiled_builtin_resource(&path)
                        .map(parse_workflow_definition)
                        .transpose()
                        .map_err(|error| format!("invalid compiled resource `{path}`: {error}")),
                }
            }
        }
    }

    pub(super) fn task_workflow(
        &self,
        name: &str,
        stored: Option<&str>,
    ) -> Result<WorkflowDefinition, String> {
        if let Some(stored) = stored.filter(|value| !value.trim().is_empty()) {
            return parse_stored_workflow_definition(stored);
        }
        self.workflow(name)
    }

    pub(super) fn agent(&self, selector: &str) -> Result<AgentDefinition, String> {
        self.agent_optional(selector)?.ok_or_else(|| {
            let (role, _) = split_agent_selector(selector);
            format!("compiled resource not found: .kanna/agents/{role}/AGENT.md")
        })
    }

    pub(super) fn agent_optional(&self, selector: &str) -> Result<Option<AgentDefinition>, String> {
        let selector = AgentSelector::resolve(selector, self.config.flavors.as_ref());
        // `pr-triage` shipped before the human PR-review flow settled on its
        // product terminology. Probe both names so a new workflow still sees
        // an existing repository override/extension, and an old pinned
        // workflow still sees a repository that has moved to the current
        // name. The requested name wins when both paths exist.
        let repo_agent_dirs = agent_repo_dirs(&selector.role);
        let mut definition = None;
        let mut uses_formula = false;
        for dir in &repo_agent_dirs {
            let agent_path = format!(".kanna/agents/{dir}/AGENT.md");
            if let Some(content) = read_snapshot_utf8(&self.snapshot, &agent_path)? {
                let content = self
                    .expand_partials(&content, &agent_path)
                    .map_err(|error| definition_error(&self.snapshot, &agent_path, error))?;
                uses_formula |= content_uses_formula(&content)
                    .map_err(|error| definition_error(&self.snapshot, &agent_path, error))?;
                definition = Some(
                    parse_agent_definition(&content)
                        .map_err(|error| definition_error(&self.snapshot, &agent_path, error))?,
                );
                break;
            }
        }
        let mut definition = match definition {
            Some(definition) => definition,
            None => {
                let Some(content) = optional_builtin_agent_resource(&selector) else {
                    return Ok(None);
                };
                let builtin_path = format!(".kanna/agents/{}/AGENT.md", selector.role);
                let content = self
                    .expand_partials(&content, &builtin_path)
                    .map_err(|error| {
                        format!(
                            "invalid compiled agent resource for selector `{}`: {error}",
                            selector.display()
                        )
                    })?;
                uses_formula |= content_uses_formula(&content).map_err(|error| {
                    format!(
                        "invalid compiled agent resource for selector `{}`: {error}",
                        selector.display()
                    )
                })?;
                parse_agent_definition(&content).map_err(|error| {
                    format!(
                        "invalid compiled agent resource for selector `{}`: {error}",
                        selector.display()
                    )
                })?
            }
        };

        for dir in repo_agent_dirs {
            let extension_path = format!(".kanna/agents/{dir}/EXTEND.md");
            if let Some(extension) = read_snapshot_utf8(&self.snapshot, &extension_path)? {
                let extension = self
                    .expand_partials(&extension, &extension_path)
                    .map_err(|error| definition_error(&self.snapshot, &extension_path, error))?;
                uses_formula |= content_uses_formula(&extension)
                    .map_err(|error| definition_error(&self.snapshot, &extension_path, error))?;
                apply_agent_extension(&mut definition, &extension)
                    .map_err(|error| definition_error(&self.snapshot, &extension_path, error))?;
                break;
            }
        }
        // A base or extension that opts into the definition formula must still
        // satisfy it once EXTEND.md is merged in: an extension can otherwise
        // push a compliant base past the 40-line cap, or smuggle in a legacy
        // result variable, with nothing checking the *resolved* document (see
        // `check_definition_formula`, which only ever saw the base file).
        if uses_formula {
            let resolved = render_agent_md(&definition)?;
            check_definition_formula(&resolved).map_err(|error| {
                format!(
                    "invalid resolved agent `{}` (AGENT.md merged with EXTEND.md): {error}",
                    selector.display()
                )
            })?;
        }
        Ok(Some(definition))
    }

    /// Resolve one `.kanna/partials/{name}.md` fragment: the repository's own
    /// override first, falling back to a bundled built-in — the same
    /// override-by-name rule `agent_optional` applies to `AGENT.md`/`EXTEND.md`
    /// themselves.
    fn partial(&self, name: &str) -> Result<Option<String>, String> {
        let path = format!(".kanna/partials/{name}.md");
        if let Some(content) = read_snapshot_utf8(&self.snapshot, &path)? {
            return Ok(Some(content));
        }
        Ok(optional_builtin_partial_resource(name).map(str::to_string))
    }

    /// Expand every `{{> name}}` partial include in `content`, which was read
    /// from `origin` (an AGENT.md/EXTEND.md path, used only for error
    /// messages). A missing partial or a recursive include fails the whole
    /// definition rather than silently emitting nothing or looping forever —
    /// see the module doc comment above [`expand_partials_with_stack`].
    fn expand_partials(&self, content: &str, origin: &str) -> Result<String, String> {
        expand_partials_with_stack(self, content, origin, &mut Vec::new())
    }

    /// Every workflow name this repo offers as a *choice* — what the desktop's
    /// new-task picker lists and what a caller may name on task creation.
    ///
    /// A definition opts out of being offered by declaring
    /// `"visibility": "internal"` (see `DefinitionVisibility`). The effective
    /// definition decides: a repo file shadowing an internal built-in speaks
    /// for itself, so omitting `visibility` deliberately promotes the name to
    /// a public choice, and re-declaring `internal` keeps the customization
    /// unlisted. A repo file that cannot be read or parsed stays listed —
    /// listing must not fail, or silently shrink, because one file is
    /// malformed; the parse error stays with that workflow's own endpoint.
    pub(super) fn workflow_names(&self) -> Result<Vec<String>, String> {
        let mut repo_names = BTreeSet::new();
        for path in [".kanna/pipelines", ".kanna/workflows"] {
            let entries = self
                .snapshot
                .list_direct_entries(path)
                .map_err(|error| definition_error(&self.snapshot, path, error))?;
            for entry in entries {
                let Some(name) = entry.strip_suffix(".json") else {
                    continue;
                };
                if name.is_empty() || name == "schema" {
                    continue;
                }
                repo_names.insert(name.to_string());
            }
        }

        let mut names = BTreeSet::new();
        for (name, definition) in BUILTIN_WORKFLOWS {
            // A repo file under this name shadows the bundled definition,
            // visibility included; the repo loop below judges it instead.
            if repo_names.contains(*name) {
                continue;
            }
            if declared_workflow_visibility(definition).is_public() {
                names.insert((*name).to_string());
            }
        }
        for name in repo_names {
            let file_path = format!(".kanna/workflows/{name}.json");
            let legacy_file_path = format!(".kanna/pipelines/{name}.json");
            let effective_path = if self.snapshot.read_optional_utf8(&file_path)?.is_some() {
                &file_path
            } else {
                &legacy_file_path
            };
            let visible = match self.snapshot.read_optional_utf8(effective_path) {
                Ok(Some(content)) => declared_workflow_visibility(&content).is_public(),
                // A listed entry should always read back; one that does not
                // cannot have declared itself internal.
                Ok(None) => true,
                Err(error) => {
                    log::warn!(
                        "listing workflow `{effective_path}` without reading its visibility: {error}"
                    );
                    true
                }
            };
            if visible {
                names.insert(name);
            }
        }
        Ok(names.into_iter().collect())
    }

    /// Every named agent selector that can be passed to task creation, after
    /// applying the same repo override, configured-flavor, and EXTEND.md
    /// resolution as `agent()`. Definitions whose resolved `visibility` is
    /// `internal` (see `DefinitionVisibility`) are omitted: Kanna binds those
    /// itself, but they still resolve when named explicitly.
    pub(super) fn agents(&self) -> Result<Vec<ResolvedAgentDefinition>, String> {
        let mut names = builtin_agent_names();
        let entries = self
            .snapshot
            .list_direct_entries(".kanna/agents")
            .map_err(|error| definition_error(&self.snapshot, ".kanna/agents", error))?;

        for name in entries {
            let agent_path = format!(".kanna/agents/{name}/AGENT.md");
            if read_snapshot_utf8(&self.snapshot, &agent_path)?.is_some() {
                names.insert(canonical_builtin_agent_name(&name).to_string());
            }
        }

        let mut resolved = Vec::new();
        for name in names {
            let repo_dirs = agent_repo_dirs(&name);
            let mut repo_has_agent = false;
            let mut repo_has_extension = false;
            for dir in repo_dirs {
                let repo_agent_path = format!(".kanna/agents/{dir}/AGENT.md");
                repo_has_agent |= read_snapshot_utf8(&self.snapshot, &repo_agent_path)?.is_some();
                let repo_extension_path = format!(".kanna/agents/{dir}/EXTEND.md");
                repo_has_extension |=
                    read_snapshot_utf8(&self.snapshot, &repo_extension_path)?.is_some();
            }
            let builtin = is_builtin_agent_name(&name);
            let source = match (repo_has_agent, repo_has_extension, builtin) {
                (true, _, true) | (false, true, true) => AgentDefinitionSource::RepoOverride,
                (false, false, true) => AgentDefinitionSource::BuiltIn,
                (true, _, false) => AgentDefinitionSource::RepoAuthored,
                (false, _, false) => {
                    return Err(format!(
                        "agent `{name}` disappeared while resolving repository definitions"
                    ));
                }
            };
            let definition = self.agent(&name)?;
            if !definition.visibility.is_public() {
                continue;
            }
            resolved.push(ResolvedAgentDefinition {
                name,
                description: definition.description,
                default_provider: definition
                    .agent_providers
                    .first()
                    .and_then(|v| v.resolve(false).ok())
                    .map(|v| v.provider.to_string()),
                default_model: definition
                    .agent_providers
                    .first()
                    .and_then(|v| v.resolve(false).ok())
                    .and_then(|v| v.model)
                    .or(definition.model),
                default_effort: definition
                    .agent_providers
                    .first()
                    .and_then(|v| v.resolve(false).ok())
                    .and_then(|v| v.effort)
                    .or(definition.effort),
                source,
            });
        }
        Ok(resolved)
    }

    /// The raw, unresolved source behind an agent selector: `AGENT.md` and,
    /// when the repo layers one, `EXTEND.md`, exactly as authored — partial
    /// includes left as literal `{{> name}}` tokens, no EXTEND merge applied.
    /// This is what `kanna-cli agent show --raw` and its MCP counterpart
    /// serve: the file(s) a repo would actually be overriding, since the
    /// resolved prompt alone never showed a customer what to copy.
    pub(super) fn agent_source(&self, selector: &str) -> Result<Option<AgentSourceView>, String> {
        let selector = AgentSelector::resolve(selector, self.config.flavors.as_ref());
        let repo_agent_dirs = agent_repo_dirs(&selector.role);

        let mut agent_md = None;
        for dir in &repo_agent_dirs {
            let agent_path = format!(".kanna/agents/{dir}/AGENT.md");
            if let Some(content) = read_snapshot_utf8(&self.snapshot, &agent_path)? {
                agent_md = Some(content);
                break;
            }
        }
        let repo_has_agent = agent_md.is_some();
        let agent_md = match agent_md {
            Some(content) => content,
            None => match optional_builtin_agent_resource(&selector) {
                Some(content) => content,
                None => return Ok(None),
            },
        };

        let mut extend_md = None;
        for dir in &repo_agent_dirs {
            let extension_path = format!(".kanna/agents/{dir}/EXTEND.md");
            if let Some(content) = read_snapshot_utf8(&self.snapshot, &extension_path)? {
                extend_md = Some(content);
                break;
            }
        }
        let repo_has_extension = extend_md.is_some();

        let builtin = is_builtin_agent_name(&selector.role);
        let source = match (repo_has_agent, repo_has_extension, builtin) {
            (true, _, true) | (false, true, true) => AgentDefinitionSource::RepoOverride,
            (false, false, true) => AgentDefinitionSource::BuiltIn,
            (true, _, false) => AgentDefinitionSource::RepoAuthored,
            (false, _, false) => {
                return Err(format!(
                    "agent `{}` disappeared while resolving repository definitions",
                    selector.display()
                ));
            }
        };

        let canonical_role = canonical_builtin_agent_name(&selector.role);
        let name = match selector.selected_flavor() {
            // The served text is whichever flavor actually resolved
            // (explicit `role@flavor`, or a role's config-selected flavor),
            // so the reported name must say which agent this is, or a caller
            // reading `pr@draft-pr`'s raw source under the name `pr` would
            // copy it into `.kanna/agents/pr/` believing it was the
            // unflavored role.
            Some(flavor) => format!("{canonical_role}@{flavor}"),
            None => canonical_role.to_string(),
        };
        Ok(Some(AgentSourceView {
            name,
            source,
            agent_md,
            extend_md,
        }))
    }
}

/// The raw source behind a resolved agent, for `kanna-cli agent show --raw`
/// and its MCP counterpart. See `RepoDefinitions::agent_source`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentSourceView {
    pub(super) name: String,
    pub(super) source: AgentDefinitionSource,
    /// Raw `AGENT.md` text exactly as authored (repo override, or the
    /// bundled built-in when the repo has none) — frontmatter and body,
    /// partial includes left unexpanded.
    pub(super) agent_md: String,
    /// Raw `EXTEND.md` text, when the repo layers one over the base
    /// definition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) extend_md: Option<String>,
}

/// Serializable mirror of `AgentDefinition`'s frontmatter, for `agent eject`.
/// Round-trips through `parse_agent_definition`: a file this writes reads
/// back to the same resolved definition (partials already expanded, so there
/// is nothing left to include).
#[derive(Serialize)]
struct AgentFrontmatterOut<'a> {
    name: &'a str,
    description: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    agent_provider: &'a Vec<AgentSelectionEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    permission_mode: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    allowed_tools: &'a Vec<String>,
    #[serde(skip_serializing_if = "DefinitionVisibility::is_public")]
    visibility: DefinitionVisibility,
}

/// Render a fully-resolved `AgentDefinition` back into `AGENT.md` text, for
/// `kanna-cli agent eject`. The output is self-contained: partials and any
/// `EXTEND.md` are already merged into `definition.prompt`, so the file this
/// produces has nothing left to resolve against — which is also why ejecting
/// over an existing `EXTEND.md` is refused by the caller rather than handled
/// here: applying that extension again on the next resolution would double it.
pub(super) fn render_agent_md(definition: &AgentDefinition) -> Result<String, String> {
    let frontmatter = AgentFrontmatterOut {
        name: &definition.name,
        description: &definition.description,
        agent_provider: &definition.agent_providers,
        model: definition.model.as_deref(),
        effort: definition.effort.as_deref(),
        permission_mode: definition.permission_mode.as_deref(),
        allowed_tools: &definition.allowed_tools,
        visibility: definition.visibility,
    };
    let yaml = serde_yaml::to_string(&frontmatter)
        .map_err(|error| format!("failed to render agent frontmatter: {error}"))?;
    Ok(format!("---\n{yaml}---\n\n{}\n", definition.prompt.trim()))
}

fn repo_definition_resolution_error(repo: &Repo, error: String) -> String {
    let branch = repo.default_branch.as_deref().unwrap_or("main");
    let source = repo
        .default_branch_source
        .as_deref()
        .unwrap_or("legacy_unknown");
    let message = format!(
        "failed to resolve repository definitions for repo `{}` from recorded default branch `{branch}` (source: {source}): {error}",
        repo.id
    );
    log::error!("{message}");
    message
}

/// The committed config as a raw JSON object, so the machine-local layer can
/// merge into it before either side is interpreted. A document that is not an
/// object yields no keys, matching the tolerance the typed parser has always
/// had for repos Kanna does not control.
fn parse_config_object(
    content: &str,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    let value: serde_json::Value =
        serde_json::from_str(content).map_err(|error| format!("invalid repo config: {error}"))?;
    Ok(value.as_object().cloned().unwrap_or_default())
}

fn validate_structured_preferences(
    raw: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if let Some(entries) = raw
        .get("agentProviders")
        .and_then(serde_json::Value::as_object)
    {
        for (name, value) in entries {
            let structured = value.get("harness").is_some()
                || value.is_array()
                || (value.is_object() && value.get("provider").is_none())
                || value.get("provider").is_some_and(|v| {
                    v.is_object()
                        || v.as_array()
                            .is_some_and(|a| a.iter().any(serde_json::Value::is_object))
                });
            if structured && parse_agent_provider_preference(value).is_none() {
                return Err(format!("invalid structured agentProviders entry '{name}': expected harness, optional model/effort/autocompact, and unique harness candidates"));
            }
        }
    }
    Ok(())
}

fn repo_config_from_object(raw: &serde_json::Map<String, serde_json::Value>) -> RepoConfig {
    let string_array = |name: &str| {
        raw.get(name).and_then(|value| {
            let values = value.as_array()?;
            values
                .iter()
                .map(|value| value.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
    };

    let string_map = |value: Option<&serde_json::Value>| {
        let values = value?.as_object()?;
        let normalized = values
            .iter()
            .filter_map(|(name, value)| {
                value
                    .as_str()
                    .map(|value| (name.clone(), value.to_string()))
            })
            .collect::<HashMap<_, _>>();
        (!normalized.is_empty()).then_some(normalized)
    };

    let ports = raw.get("ports").and_then(|value| {
        let values = value.as_object()?;
        let normalized = values
            .iter()
            .filter_map(|(name, value)| {
                let port = value.as_u64().and_then(|value| u16::try_from(value).ok())?;
                Some((name.clone(), port))
            })
            .collect::<HashMap<_, _>>();
        (!normalized.is_empty()).then_some(normalized)
    });

    let agent_providers = raw
        .get("agentProviders")
        .and_then(serde_json::Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter_map(|(pattern, value)| {
                    (!pattern.trim().is_empty())
                        .then(|| parse_agent_provider_preference(value))
                        .flatten()
                        .map(|preference| (pattern.clone(), preference))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .filter(|values| !values.is_empty());

    let integer_array = |name: &str, valid: fn(i64) -> bool| {
        raw.get(name)
            .and_then(serde_json::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_i64)
                    .filter(|value| valid(*value))
                    .collect::<Vec<_>>()
            })
            .filter(|values| !values.is_empty())
            .unwrap_or_default()
    };

    let workspace = raw
        .get("workspace")
        .and_then(serde_json::Value::as_object)
        .and_then(|workspace_raw| {
            let env = string_map(workspace_raw.get("env"));
            let path = workspace_raw
                .get("path")
                .and_then(serde_json::Value::as_object)
                .and_then(|path_raw| {
                    let filtered_entries = |name: &str| {
                        let entries = path_raw.get(name)?.as_array()?;
                        let entries = entries
                            .iter()
                            .filter_map(|entry| entry.as_str().map(str::to_string))
                            .collect::<Vec<_>>();
                        (!entries.is_empty()).then_some(entries)
                    };
                    let prepend = filtered_entries("prepend");
                    let append = filtered_entries("append");
                    (prepend.is_some() || append.is_some())
                        .then_some(RepoWorkspacePathConfig { prepend, append })
                });
            (env.is_some() || path.is_some()).then_some(RepoWorkspaceConfig { env, path })
        });

    let artifacts = raw
        .get("artifacts")
        .and_then(serde_json::Value::as_object)
        .and_then(|artifacts_raw| {
            let repository_path = artifacts_raw
                .get("repositoryPath")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_string);
            let retention = artifacts_raw
                .get("retention")
                .and_then(serde_json::Value::as_str)
                .and_then(crate::artifacts::ArtifactRetention::parse);
            let remote = artifacts_raw
                .get("remote")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|remote| !remote.is_empty())
                .map(str::to_string);
            (repository_path.is_some() || retention.is_some() || remote.is_some()).then_some(
                RepoArtifactsConfig {
                    repository_path,
                    retention,
                    remote,
                },
            )
        });

    RepoConfig {
        // `pipeline` is the retired spelling of the `workflow` key; repo
        // configs written before the rename must keep loading.
        workflow: raw
            .get("workflow")
            .or_else(|| raw.get("pipeline"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        setup: string_array("setup"),
        teardown: string_array("teardown"),
        test: string_array("test"),
        ports,
        flavors: string_map(raw.get("flavors")),
        vars: string_map(raw.get("vars")),
        agent_providers,
        reserved_port_offsets: integer_array("reserved_port_offsets", |value| value >= 0),
        reserved_ports: integer_array("reserved_ports", |value| (1..=65535).contains(&value)),
        stage_order: string_array("stage_order"),
        workspace,
        artifacts,
        local_override: None,
    }
}

fn deserialize_optional_agent_provider_preferences<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, AgentProviderPreference>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(raw) = value.and_then(|value| value.as_object().cloned()) else {
        return Ok(None);
    };
    validate_structured_preferences(&serde_json::Map::from_iter([(
        String::from("agentProviders"),
        serde_json::Value::Object(raw.clone()),
    )]))
    .map_err(serde::de::Error::custom)?;
    let preferences = raw
        .iter()
        .filter_map(|(pattern, value)| {
            (!pattern.trim().is_empty())
                .then(|| parse_agent_provider_preference(value))
                .flatten()
                .map(|preference| (pattern.clone(), preference))
        })
        .collect::<BTreeMap<_, _>>();
    Ok((!preferences.is_empty()).then_some(preferences))
}

pub(super) fn parse_agent_provider_preference(
    value: &serde_json::Value,
) -> Option<AgentProviderPreference> {
    if value.get("harness").is_some()
        || (value.is_object() && value.get("provider").is_none())
        || value.is_array()
    {
        return Some(AgentProviderPreference {
            providers: parse_selection_value(value.clone(), false).ok()?,
            model: None,
            effort: None,
            autocompact: None,
        });
    }
    let (provider, model, effort, autocompact) = match value {
        serde_json::Value::String(_) => (value, None, None, None),
        serde_json::Value::Object(raw) => (
            raw.get("provider")?,
            raw.get("model")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            raw.get("effort")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            raw.get("autocompact")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        ),
        _ => return None,
    };
    if provider.is_object()
        || provider
            .as_array()
            .is_some_and(|v| v.iter().any(serde_json::Value::is_object))
    {
        let raw = value.as_object()?;
        if raw.keys().any(|key| {
            !matches!(
                key.as_str(),
                "provider" | "model" | "effort" | "autocompact"
            )
        }) || ["model", "effort", "autocompact"]
            .iter()
            .any(|key| raw.get(*key).is_some_and(|value| !value.is_string()))
        {
            return None;
        }
        let providers = parse_selection_value(provider.clone(), false).ok()?;
        validate_selection_siblings(
            &providers,
            model.as_deref(),
            effort.as_deref(),
            autocompact.as_deref(),
        )
        .ok()?;
        return Some(AgentProviderPreference {
            providers,
            model,
            effort,
            autocompact,
        });
    }
    let providers = match provider {
        serde_json::Value::String(provider) => provider
            .split(',')
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>(),
        serde_json::Value::Array(providers) => providers
            .iter()
            .map(serde_json::Value::as_str)
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>(),
        _ => return None,
    };
    (!providers.is_empty()).then_some(AgentProviderPreference {
        providers: providers.into_iter().map(Into::into).collect(),
        model,
        effort,
        autocompact,
    })
}

fn read_snapshot_utf8(
    snapshot: &RepoDefinitionSnapshot,
    relative_path: &str,
) -> Result<Option<String>, String> {
    snapshot
        .read_optional_utf8(relative_path)
        .map_err(|error| definition_error(snapshot, relative_path, error))
}

fn definition_error(
    snapshot: &RepoDefinitionSnapshot,
    relative_path: &str,
    error: impl std::fmt::Display,
) -> String {
    format!(
        "repository definition `{relative_path}` from `{}` at revision `{}`: {error}",
        snapshot.ref_name(),
        snapshot.revision().unwrap_or("<none>"),
    )
}

pub(super) fn parse_workflow_definition(content: &str) -> Result<WorkflowDefinition, String> {
    let value: serde_json::Value = serde_json::from_str(content)
        .map_err(|error| format!("invalid workflow definition: {error}"))?;
    reject_explicit_null_workflow_providers(&value)?;
    let raw: RawWorkflowDefinition = serde_json::from_value(value)
        .map_err(|error| format!("invalid workflow definition: {error}"))?;
    let workflow = normalize_workflow_definition(raw)
        .map_err(|error| format!("invalid workflow definition: {error}"))?;
    // A legacy definition keeps its historical tolerance of fields this build
    // ignores. A named-exit definition opts into a contract whose fields this
    // build either runs or refuses, so an unknown field there is refused, not
    // dropped.
    if workflow.routes_by_exits() {
        let unknown = super::workflow_edit::unknown_workflow_fields(content);
        if !unknown.is_empty() {
            return Err(format!(
                "invalid workflow definition: routing \"exits\" does not support {} in this \
                 version of Kanna; remove them rather than rely on them being ignored",
                unknown.join(", ")
            ));
        }
    }
    Ok(workflow)
}

fn reject_explicit_null_workflow_providers(value: &serde_json::Value) -> Result<(), String> {
    let Some(stages) = value.get("stages").and_then(serde_json::Value::as_array) else {
        return Ok(());
    };
    for (index, stage) in stages.iter().enumerate() {
        if stage
            .get("agent_provider")
            .is_some_and(serde_json::Value::is_null)
        {
            return Err(format!(
                "invalid workflow definition: stages[{index}].agent_provider must be a string or a non-empty array of strings"
            ));
        }
        for post_key in ["post", "post_action"] {
            if stage
                .get(post_key)
                .and_then(serde_json::Value::as_object)
                .and_then(|post| post.get("agent_provider"))
                .is_some_and(serde_json::Value::is_null)
            {
                return Err(format!(
                    "invalid workflow definition: stages[{index}].{post_key}.agent_provider must be a string or a non-empty array of strings"
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn parse_stored_workflow_definition(
    content: &str,
) -> Result<WorkflowDefinition, String> {
    let mut value: serde_json::Value = serde_json::from_str(content)
        .map_err(|error| format!("invalid stored workflow definition: {error}"))?;
    normalize_legacy_workflow_provider_csv(&mut value);
    let raw: RawWorkflowDefinition = serde_json::from_value(value)
        .map_err(|error| format!("invalid stored workflow definition: {error}"))?;
    normalize_workflow_definition(raw)
        .map_err(|error| format!("invalid stored workflow definition: {error}"))
}

fn normalize_legacy_workflow_provider_csv(value: &mut serde_json::Value) {
    let Some(stages) = value
        .get_mut("stages")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };

    for stage in stages {
        if let Some(provider) = stage.get_mut("agent_provider") {
            normalize_legacy_provider_csv(provider);
        }
        for post_key in ["post", "post_action"] {
            if let Some(provider) = stage
                .get_mut(post_key)
                .and_then(|post| post.get_mut("agent_provider"))
            {
                normalize_legacy_provider_csv(provider);
            }
        }
    }
}

fn normalize_legacy_provider_csv(value: &mut serde_json::Value) {
    let serde_json::Value::String(provider) = value else {
        return;
    };
    if !provider.contains(',') {
        return;
    }

    *value = serde_json::Value::Array(
        provider
            .split(',')
            .map(str::trim)
            .filter(|provider| !provider.is_empty())
            .map(|provider| serde_json::Value::String(provider.to_string()))
            .collect(),
    );
}

struct AgentSelector {
    role: String,
    explicit_flavor: Option<String>,
    configured_flavor: Option<String>,
}

impl AgentSelector {
    fn resolve(agent_name: &str, flavors: Option<&HashMap<String, String>>) -> Self {
        let (role, explicit_flavor) = split_agent_selector(agent_name);
        let configured_flavor = explicit_flavor
            .is_none()
            .then(|| {
                flavors.and_then(|map| {
                    agent_repo_dirs(&role)
                        .into_iter()
                        .find_map(|name| map.get(&name).cloned())
                })
            })
            .flatten();
        Self {
            role,
            explicit_flavor,
            configured_flavor,
        }
    }

    fn selected_flavor(&self) -> Option<&str> {
        self.explicit_flavor
            .as_deref()
            .or(self.configured_flavor.as_deref())
    }

    fn display(&self) -> String {
        match self.selected_flavor() {
            Some(flavor) => format!("{}@{flavor}", self.role),
            None => self.role.clone(),
        }
    }
}

fn split_agent_selector(agent_name: &str) -> (String, Option<String>) {
    let Some((role, flavor)) = agent_name.split_once('@') else {
        return (agent_name.to_string(), None);
    };
    if role.is_empty() || flavor.is_empty() || flavor.contains('@') {
        return (agent_name.to_string(), None);
    }
    (role.to_string(), Some(flavor.to_string()))
}

/// Shared prompt fragments an `AGENT.md`/`EXTEND.md` body can include by name,
/// with `{{> name}}`.
///
/// That syntax is Handlebars/Mustache's partial-include marker, chosen
/// deliberately: agent prompts already use `$NAME`/`${NAME}` for the engine's
/// own variable substitution (`prompt::substitute_prompt_vars`), and a
/// double-brace marker cannot collide with that, with Markdown, or with the
/// shell/JSON snippets these prompts routinely quote. No `.kanna/agents/*.md`
/// file uses `{{` today.
///
/// A partial resolves exactly like `AGENT.md`/`EXTEND.md` themselves:
/// `.kanna/partials/{name}.md` in the repo's definition snapshot, falling
/// back to a bundled built-in of the same name. Resolution happens right
/// where AGENT.md/EXTEND.md are read and merged, in `agent_optional`, so the
/// text every later stage (var substitution, provider dispatch) sees is
/// already flat.
///
/// A missing partial or a recursive/self-including chain fails the whole
/// agent definition rather than silently emitting nothing or looping
/// forever — an agent that quietly loses a safety paragraph, or a server that
/// hangs expanding a cycle, is worse than a definition that refuses to
/// resolve with a clear error naming the missing or cyclic partial.
fn expand_partials_with_stack(
    definitions: &RepoDefinitions,
    content: &str,
    origin: &str,
    stack: &mut Vec<String>,
) -> Result<String, String> {
    const MAX_PARTIAL_DEPTH: usize = 16;

    let mut out = String::with_capacity(content.len());
    let mut index = 0;
    while index < content.len() {
        let Some(marker_offset) = content[index..].find("{{>") else {
            out.push_str(&content[index..]);
            break;
        };
        out.push_str(&content[index..index + marker_offset]);
        let after_marker = index + marker_offset + 3;
        let Some(end_offset) = content[after_marker..].find("}}") else {
            return Err(format!(
                "unterminated partial include `{{{{> ...` in {origin}"
            ));
        };
        let name = content[after_marker..after_marker + end_offset].trim();
        if name.is_empty() {
            return Err(format!("empty partial include `{{{{>}}}}` in {origin}"));
        }
        if let Some(cycle_start) = stack.iter().position(|included| included == name) {
            let mut cycle = stack[cycle_start..].to_vec();
            cycle.push(name.to_string());
            return Err(format!(
                "recursive partial include in {origin}: {}",
                cycle.join(" -> ")
            ));
        }
        if stack.len() >= MAX_PARTIAL_DEPTH {
            return Err(format!(
                "partial include nesting exceeds {MAX_PARTIAL_DEPTH} levels while including \
                 `{name}` in {origin}"
            ));
        }
        let partial_content = definitions.partial(name)?.ok_or_else(|| {
            format!(
                "unknown partial `{name}` referenced in {origin} (expected \
                 `.kanna/partials/{name}.md` in the repository or a bundled built-in)"
            )
        })?;
        let partial_origin = format!(".kanna/partials/{name}.md");
        stack.push(name.to_string());
        let expanded =
            expand_partials_with_stack(definitions, &partial_content, &partial_origin, stack)?;
        stack.pop();
        out.push_str(expanded.trim_end_matches('\n'));
        index = after_marker + end_offset + 2;
    }
    Ok(out)
}

fn optional_builtin_partial_resource(name: &str) -> Option<&'static str> {
    let path = format!(".kanna/partials/{name}.md");
    BUILTIN_PARTIAL_RESOURCES
        .iter()
        .find_map(|(resource_path, content)| (*resource_path == path).then_some(*content))
}

const BUILTIN_PARTIAL_RESOURCES: &[(&str, &str)] = &[(
    ".kanna/partials/no-ai-attribution.md",
    include_str!("../../../../.kanna/partials/no-ai-attribution.md"),
)];

fn optional_builtin_agent_resource(selector: &AgentSelector) -> Option<String> {
    let role = canonical_builtin_agent_name(&selector.role);
    if let Some(flavor) = selector.selected_flavor() {
        let flavor_path = format!(".kanna/agents/{}/flavors/{}/AGENT.md", role, flavor);
        if let Some(content) = compiled_builtin_resource(&flavor_path) {
            return Some(content.to_string());
        }
    }

    compiled_builtin_resource(&format!(".kanna/agents/{role}/AGENT.md")).map(str::to_string)
}

const BUILTIN_AGENT_RESOURCES: &[(&str, &str)] = &[
    (
        ".kanna/agents/agent-factory/AGENT.md",
        include_str!("../../../../.kanna/agents/agent-factory/AGENT.md"),
    ),
    (
        ".kanna/agents/approve/AGENT.md",
        include_str!("../../../../.kanna/agents/approve/AGENT.md"),
    ),
    (
        ".kanna/agents/architect/AGENT.md",
        include_str!("../../../../.kanna/agents/architect/AGENT.md"),
    ),
    (
        ".kanna/agents/task-manager/AGENT.md",
        include_str!("../../../../.kanna/agents/task-manager/AGENT.md"),
    ),
    (
        ".kanna/agents/commit/AGENT.md",
        include_str!("../../../../.kanna/agents/commit/AGENT.md"),
    ),
    (
        ".kanna/agents/researcher/AGENT.md",
        include_str!("../../../../.kanna/agents/researcher/AGENT.md"),
    ),
    (
        ".kanna/agents/implement/AGENT.md",
        include_str!("../../../../.kanna/agents/implement/AGENT.md"),
    ),
    (
        ".kanna/agents/plan/AGENT.md",
        include_str!("../../../../.kanna/agents/plan/AGENT.md"),
    ),
    (
        ".kanna/agents/merge/AGENT.md",
        include_str!("../../../../.kanna/agents/merge/AGENT.md"),
    ),
    (
        ".kanna/agents/merge/flavors/git/AGENT.md",
        include_str!("../../../../.kanna/agents/merge/flavors/git/AGENT.md"),
    ),
    (
        ".kanna/agents/merge/flavors/github/AGENT.md",
        include_str!("../../../../.kanna/agents/merge/flavors/github/AGENT.md"),
    ),
    (
        ".kanna/agents/workflow-factory/AGENT.md",
        include_str!("../../../../.kanna/agents/workflow-factory/AGENT.md"),
    ),
    (
        ".kanna/agents/pr/AGENT.md",
        include_str!("../../../../.kanna/agents/pr/AGENT.md"),
    ),
    (
        ".kanna/agents/pr/flavors/draft-pr/AGENT.md",
        include_str!("../../../../.kanna/agents/pr/flavors/draft-pr/AGENT.md"),
    ),
    (
        ".kanna/agents/pr/flavors/push-only/AGENT.md",
        include_str!("../../../../.kanna/agents/pr/flavors/push-only/AGENT.md"),
    ),
    (
        ".kanna/agents/pr-reviewer/AGENT.md",
        include_str!("../../../../.kanna/agents/pr-reviewer/AGENT.md"),
    ),
    (
        ".kanna/agents/pr-review-manager/AGENT.md",
        include_str!("../../../../.kanna/agents/pr-review-manager/AGENT.md"),
    ),
    (
        ".kanna/agents/qa-dispatcher/AGENT.md",
        include_str!("../../../../.kanna/agents/qa-dispatcher/AGENT.md"),
    ),
    (
        ".kanna/agents/review/AGENT.md",
        include_str!("../../../../.kanna/agents/review/AGENT.md"),
    ),
    (
        ".kanna/agents/review-compat/AGENT.md",
        include_str!("../../../../.kanna/agents/review-compat/AGENT.md"),
    ),
    (
        ".kanna/agents/review-concurrency/AGENT.md",
        include_str!("../../../../.kanna/agents/review-concurrency/AGENT.md"),
    ),
    (
        ".kanna/agents/review-migration/AGENT.md",
        include_str!("../../../../.kanna/agents/review-migration/AGENT.md"),
    ),
    (
        ".kanna/agents/review-perf/AGENT.md",
        include_str!("../../../../.kanna/agents/review-perf/AGENT.md"),
    ),
    (
        ".kanna/agents/review-security/AGENT.md",
        include_str!("../../../../.kanna/agents/review-security/AGENT.md"),
    ),
    (
        ".kanna/agents/review-ui/AGENT.md",
        include_str!("../../../../.kanna/agents/review-ui/AGENT.md"),
    ),
    (
        ".kanna/agents/setup/AGENT.md",
        include_str!("../../../../.kanna/agents/setup/AGENT.md"),
    ),
    (
        ".kanna/agents/ship/AGENT.md",
        include_str!("../../../../.kanna/agents/ship/AGENT.md"),
    ),
];

fn builtin_agent_names() -> BTreeSet<String> {
    BUILTIN_AGENT_RESOURCES
        .iter()
        .filter_map(|(path, _)| {
            path.strip_prefix(".kanna/agents/")?
                .strip_suffix("/AGENT.md")
                .filter(|name| !name.contains('/'))
                .map(str::to_string)
        })
        .collect()
}

fn is_builtin_agent_name(name: &str) -> bool {
    let path = format!(
        ".kanna/agents/{}/AGENT.md",
        canonical_builtin_agent_name(name)
    );
    BUILTIN_AGENT_RESOURCES
        .iter()
        .any(|(resource_path, _)| *resource_path == path)
}

/// Built-in agents that shipped under an earlier product term. These are
/// resolution aliases only: listings expose the current name, while both
/// names continue to probe repository definitions and extensions.
const LEGACY_BUILTIN_AGENTS: &[(&str, &str)] = &[
    ("pr-triage", "pr-review-manager"),
    ("config-factory", "setup"),
    ("consultant", "researcher"),
];

fn canonical_builtin_agent_name(name: &str) -> &str {
    LEGACY_BUILTIN_AGENTS
        .iter()
        .find_map(|(legacy, current)| (*legacy == name).then_some(*current))
        .unwrap_or(name)
}

fn agent_repo_dirs(name: &str) -> Vec<String> {
    if let Some((legacy, current)) = LEGACY_BUILTIN_AGENTS
        .iter()
        .find(|(legacy, current)| *legacy == name || *current == name)
    {
        return if *legacy == name {
            vec![(*legacy).to_string(), (*current).to_string()]
        } else {
            vec![(*current).to_string(), (*legacy).to_string()]
        };
    }
    vec![name.to_string()]
}

/// Built-in workflows that shipped under an earlier name, mapped to the
/// definition each now resolves to. Single source of truth: the compiled
/// resource fallback below and manifest canonicalization in
/// `load_repo_kanna_definitions` both read this table, so a retired name never
/// has its mapping written twice.
///
/// These are resolution aliases only. They stay out of `workflow_names()`, so
/// a retired name never returns as a user-facing choice, and they always lose
/// to a repo that ships its own workflow under the same name.
pub(super) const LEGACY_BUILTIN_WORKFLOWS: &[(&str, &str)] = &[
    ("default", "no-review"),
    ("qa", "single-reviewer"),
    ("qa-dispatch", "specialized-reviewers"),
    ("consultation", "research"),
    ("architect-consultation", "architect-research"),
];

/// The current name a possibly-retired built-in workflow resolves to, or
/// `name` unchanged when it was never retired.
pub(super) fn canonical_builtin_workflow_name(name: &str) -> &str {
    LEGACY_BUILTIN_WORKFLOWS
        .iter()
        .find_map(|(legacy, current)| (*legacy == name).then_some(*current))
        .unwrap_or(name)
}

/// Single source of truth for the built-in workflows, mapping each name to its
/// bundled definition: both `workflow_names()` and the compiled-resource
/// fallback read this table, so a built-in can never be offered without
/// shipping a definition. Whether a name is offered as a choice is declared by
/// the definition itself, through its `visibility` field. Purpose-built child
/// workflows such as `specialty-review` and `architect-research` declare
/// `"visibility": "internal"`: their invoking agents bind them explicitly,
/// while public research and complete product-work workflows remain
/// operator choices.
const BUILTIN_WORKFLOWS: &[(&str, &str)] = &[
    (
        "repository-setup",
        include_str!("../../../../.kanna/workflows/repository-setup.json"),
    ),
    (
        "architect-research",
        include_str!("../../../../.kanna/workflows/architect-research.json"),
    ),
    (
        "no-review",
        include_str!("../../../../.kanna/workflows/no-review.json"),
    ),
    (
        "mechanical",
        include_str!("../../../../.kanna/workflows/mechanical.json"),
    ),
    (
        "plan-build-review",
        include_str!("../../../../.kanna/workflows/plan-build-review.json"),
    ),
    (
        "pr-review",
        include_str!("../../../../.kanna/workflows/pr-review.json"),
    ),
    (
        "pr-review-single",
        include_str!("../../../../.kanna/workflows/pr-review-single.json"),
    ),
    (
        "research",
        include_str!("../../../../.kanna/workflows/research.json"),
    ),
    (
        "single-reviewer",
        include_str!("../../../../.kanna/workflows/single-reviewer.json"),
    ),
    (
        "specialized-reviewers",
        include_str!("../../../../.kanna/workflows/specialized-reviewers.json"),
    ),
    (
        "specialty-review",
        include_str!("../../../../.kanna/workflows/specialty-review.json"),
    ),
    (
        RELEASE_WORKFLOW_NAME,
        include_str!("../../../../.kanna/workflows/release.json"),
    ),
];

/// The release workflow a repository's merge master runs (spec §10): the
/// merge singleton is claimed onto it, so its first stage is the merge window
/// and runs the `merge` agent. It is internal, and task creation by name
/// refuses it: a second task running it would be a competing merge master.
pub(crate) const RELEASE_WORKFLOW_NAME: &str = "release";

/// The `visibility` a workflow definition file declares, probed tolerantly for
/// listing: `workflow_names()` must not fail — or silently drop a name —
/// because one repo file is malformed, so anything that is not a well-formed
/// top-level `"visibility"` declaration counts as the public default. The
/// file's real parse error stays with its own endpoint, where
/// `workflow_optional()` reports it strictly.
fn declared_workflow_visibility(content: &str) -> DefinitionVisibility {
    serde_json::from_str::<serde_json::Value>(content)
        .ok()
        .and_then(|value| {
            serde_json::from_value::<DefinitionVisibility>(value.get("visibility")?.clone()).ok()
        })
        .unwrap_or_default()
}

fn compiled_builtin_resource(relative_path: &str) -> Option<&'static str> {
    // A retired built-in workflow name serves its current definition.
    if let Some(name) = relative_path
        .strip_prefix(".kanna/workflows/")
        .and_then(|file| file.strip_suffix(".json"))
    {
        let canonical = canonical_builtin_workflow_name(name);
        if canonical != name {
            return compiled_builtin_resource(&format!(".kanna/workflows/{canonical}.json"));
        }
    }

    // Keep explicit agent references in pinned workflow definitions working
    // after a built-in agent adopts its current product terminology.
    if let Some(name) = relative_path
        .strip_prefix(".kanna/agents/")
        .and_then(|file| file.strip_suffix("/AGENT.md"))
    {
        let canonical = canonical_builtin_agent_name(name);
        if canonical != name {
            return compiled_builtin_resource(&format!(".kanna/agents/{canonical}/AGENT.md"));
        }
    }

    let workflow = relative_path
        .strip_prefix(".kanna/workflows/")
        .and_then(|file| file.strip_suffix(".json"))
        .and_then(|name| {
            BUILTIN_WORKFLOWS
                .iter()
                .find_map(|(builtin, definition)| (*builtin == name).then_some(*definition))
        });
    workflow.or_else(|| {
        BUILTIN_AGENT_RESOURCES
            .iter()
            .find_map(|(path, content)| (*path == relative_path).then_some(*content))
    })
}

/// Merge an `EXTEND.md` document into a resolved agent definition: the body
/// is appended to the base prompt and frontmatter fields replace the base's
/// when present. Frontmatter is optional; a plain markdown file is a pure
/// prompt extension.
fn apply_agent_extension(definition: &mut AgentDefinition, content: &str) -> Result<(), String> {
    let extension = parse_agent_extension(content)?;

    // A selection object replaces the selection field. Inherited sibling
    // tuning must not acquire a different owner through that replacement.
    if let Some(replacement) = &extension.agent_providers {
        let uses_objects = definition
            .agent_providers
            .iter()
            .chain(replacement)
            .any(|entry| matches!(entry, AgentSelectionEntry::Candidate(_)));
        let owner = definition
            .agent_providers
            .first()
            .and_then(|entry| entry.resolve(false).ok())
            .map(|entry| entry.provider);
        let next_owner = replacement
            .first()
            .and_then(|entry| entry.resolve(false).ok())
            .map(|entry| entry.provider);
        let inherits_tuning = (definition.model.is_some() && extension.model.is_none())
            || (definition.effort.is_some() && extension.effort.is_none());
        if uses_objects && inherits_tuning && owner.is_some() && owner != next_owner {
            return Err("conflicting selection representations: EXTEND changes harness while inheriting sibling model/effort written for another harness".into());
        }
    }

    if let Some(description) = extension.description {
        definition.description = description;
    }
    if !extension.prompt.is_empty() {
        if definition.prompt.is_empty() {
            definition.prompt = extension.prompt;
        } else {
            definition.prompt = format!("{}\n\n{}", definition.prompt, extension.prompt);
        }
    }
    if let Some(agent_providers) = extension.agent_providers {
        definition.agent_providers = agent_providers;
    }
    if extension.model.is_some() {
        definition.model = extension.model;
    }
    if extension.effort.is_some() {
        definition.effort = extension.effort;
    }
    if extension.permission_mode.is_some() {
        definition.permission_mode = extension.permission_mode;
    }
    if let Some(allowed_tools) = extension.allowed_tools {
        definition.allowed_tools = allowed_tools;
    }
    // Like every other frontmatter field: declared, it replaces the base's —
    // so an extension can deliberately promote an internal built-in into the
    // listing (or demote a public one) — and absent, the base's visibility
    // survives the extension.
    if let Some(visibility) = extension.visibility {
        definition.visibility = visibility;
    }

    validate_agent_definition(definition)
        .map_err(|error| format!("invalid extended agent: {error}"))
}

fn parse_agent_definition(content: &str) -> Result<AgentDefinition, String> {
    let (frontmatter, body) = split_frontmatter(content);
    let fm: AgentFrontmatter = match frontmatter {
        Some(raw) => {
            serde_yaml::from_str(raw).map_err(|e| format!("invalid AGENT.md frontmatter: {}", e))?
        }
        None => AgentFrontmatter::default(),
    };
    let uses_formula = fm.role.is_some() || fm.providers.is_some();

    let definition = AgentDefinition {
        name: fm.name.unwrap_or_default(),
        description: fm.description.or(fm.role).unwrap_or_default(),
        prompt: body.trim().to_string(),
        agent_providers: parse_agent_providers(fm.agent_provider.or(fm.providers))?,
        model: fm.model,
        effort: fm.effort,
        permission_mode: validate_permission_mode(fm.permission_mode)?,
        allowed_tools: fm.allowed_tools.unwrap_or_default(),
        visibility: validate_visibility(fm.visibility)?.unwrap_or_default(),
    };
    validate_agent_definition(&definition).map_err(|error| format!("invalid AGENT.md: {error}"))?;
    if uses_formula {
        check_definition_formula(content).map_err(|error| format!("invalid AGENT.md: {error}"))?;
    }
    Ok(definition)
}

/// Whether an AGENT.md/EXTEND.md's frontmatter declares `role` or `providers`
/// (see `AgentFrontmatter`), the definition-formula opt-in. Checked
/// separately from `parse_agent_definition`/`parse_agent_extension` so a
/// caller merging a base with an extension can tell whether *either* side
/// opted in, before either one's own formula check has necessarily run.
fn content_uses_formula(content: &str) -> Result<bool, String> {
    let (frontmatter, _) = split_frontmatter(content);
    let fm: AgentFrontmatter = match frontmatter {
        Some(raw) => {
            serde_yaml::from_str(raw).map_err(|e| format!("invalid AGENT.md frontmatter: {}", e))?
        }
        None => AgentFrontmatter::default(),
    };
    Ok(fm.role.is_some() || fm.providers.is_some())
}

/// Spec §12's definition formula: a definition that opts in (by declaring
/// `role` or `providers` in its frontmatter, see `AgentFrontmatter`) must
/// resolve to 15-40 lines total and carry the four required section headers,
/// in order, in its body. `checkDefinitionFormula` in the core package's
/// `agent-loader.ts` mirrors this on the resolved-definition text.
const DEFINITION_FORMULA_SECTIONS: [&str; 4] =
    ["## Produces", "## Reads", "## Must not", "## Stop when"];
const DEFINITION_FORMULA_RESULT_VARS: [&str; 3] =
    ["$PREV_RESULT", "$PREV_MAIN_RESULT", "$PLAN_RESULT"];

fn check_definition_formula(content: &str) -> Result<(), String> {
    let line_count = content.trim_end_matches('\n').lines().count();
    if !(15..=40).contains(&line_count) {
        return Err(format!(
            "definition-formula definitions must be 15-40 lines, got {line_count}"
        ));
    }
    let mut search_from = 0usize;
    for section in DEFINITION_FORMULA_SECTIONS {
        match content[search_from..].find(section) {
            Some(offset) => search_from += offset + section.len(),
            None => {
                return Err(format!(
                    "definition-formula definitions require the section \"{section}\", in order after {:?}",
                    &DEFINITION_FORMULA_SECTIONS
                ));
            }
        }
    }
    for var in DEFINITION_FORMULA_RESULT_VARS {
        if content.contains(var) {
            return Err(format!(
                "definition-formula definitions must not reference the legacy result variable {var}; the engine delivers results through the ledger"
            ));
        }
    }
    Ok(())
}

fn parse_agent_extension(content: &str) -> Result<AgentExtension, String> {
    let (frontmatter, body) = split_frontmatter(content);
    let fm: AgentFrontmatter = match frontmatter {
        Some(raw) => {
            serde_yaml::from_str(raw).map_err(|e| format!("invalid AGENT.md frontmatter: {}", e))?
        }
        None => AgentFrontmatter::default(),
    };

    let agent_providers = fm
        .agent_provider
        .or(fm.providers)
        .map(|value| parse_agent_providers(Some(value)))
        .transpose()?;

    Ok(AgentExtension {
        prompt: body.trim().to_string(),
        description: fm.description.or(fm.role),
        agent_providers,
        model: fm.model,
        effort: fm.effort,
        permission_mode: validate_permission_mode(fm.permission_mode)?,
        allowed_tools: fm.allowed_tools,
        visibility: validate_visibility(fm.visibility)?,
    })
}

fn validate_permission_mode(permission_mode: Option<String>) -> Result<Option<String>, String> {
    let Some(permission_mode) = permission_mode else {
        return Ok(None);
    };
    if matches!(
        permission_mode.as_str(),
        "default" | "acceptEdits" | "dontAsk"
    ) {
        Ok(Some(permission_mode))
    } else {
        Err(format!(
            "permission_mode must be one of: default, acceptEdits, dontAsk (got \"{permission_mode}\")"
        ))
    }
}

fn validate_visibility(visibility: Option<String>) -> Result<Option<DefinitionVisibility>, String> {
    let Some(visibility) = visibility else {
        return Ok(None);
    };
    match visibility.as_str() {
        "public" => Ok(Some(DefinitionVisibility::Public)),
        "internal" => Ok(Some(DefinitionVisibility::Internal)),
        _ => Err(format!(
            "visibility must be one of: public, internal (got \"{visibility}\")"
        )),
    }
}

fn validate_agent_definition(definition: &AgentDefinition) -> Result<(), String> {
    if definition.name.trim().is_empty() {
        return Err("name is required and must be a non-empty string".to_string());
    }
    if definition.description.trim().is_empty() {
        return Err("description is required and must be a non-empty string".to_string());
    }
    validate_selection_siblings(
        &definition.agent_providers,
        definition.model.as_deref(),
        definition.effort.as_deref(),
        None,
    )?;
    Ok(())
}

fn validate_selection_siblings(
    entries: &[AgentSelectionEntry],
    model: Option<&str>,
    effort: Option<&str>,
    autocompact: Option<&str>,
) -> Result<(), String> {
    for entry in entries {
        if let AgentSelectionEntry::Candidate(candidate) = entry {
            for (name, nested, sibling) in [
                ("model", candidate.model.as_deref(), model),
                ("effort", candidate.effort.as_deref(), effort),
                ("autocompact", candidate.autocompact.as_deref(), autocompact),
            ] {
                if nested.zip(sibling).is_some_and(|(a, b)| a != b) {
                    return Err(format!(
                        "conflicting nested and sibling {name} in agent_provider"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let normalized = content.trim_start_matches('\u{feff}');
    let Some(rest) = normalized.strip_prefix("---") else {
        return (None, normalized);
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return (None, normalized);
    };
    if let Some(index) = rest.find("\n---\n") {
        let frontmatter = &rest[..index];
        let body = &rest[index + 5..];
        return (Some(frontmatter), body);
    }
    if let Some(index) = rest.find("\r\n---\r\n") {
        let frontmatter = &rest[..index];
        let body = &rest[index + 7..];
        return (Some(frontmatter), body);
    }
    (None, normalized)
}

fn parse_agent_providers(value: Option<YamlValue>) -> Result<Vec<AgentSelectionEntry>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let mut value = serde_json::to_value(value).map_err(|e| e.to_string())?;
    if let Some(csv) = value.as_str() {
        value = serde_json::json!(csv
            .split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>());
    }
    if let Some(entries) = value
        .as_array_mut()
        .filter(|entries| entries.iter().all(serde_json::Value::is_string))
    {
        entries.retain(|v| v.as_str().is_some_and(|v| !v.trim().is_empty()));
    }
    parse_selection_value(value, false)
}

fn normalize_workflow_definition(raw: RawWorkflowDefinition) -> Result<WorkflowDefinition, String> {
    let mut stages: Vec<WorkflowStage> = Vec::new();
    for stage in raw.stages {
        let RawWorkflowStage {
            name,
            description,
            agent,
            prompt,
            agent_provider,
            environment,
            exits,
            budget,
            policy,
            transition,
            mode,
            post,
            post_action,
            exit_commit,
            setup,
            teardown,
        } = stage;

        let (transition, revision_transition, loop_transition, handoff, continues) = match policy {
            Some(policy) => (
                policy.transition,
                policy.revision_transition,
                policy.loop_transition,
                policy.handoff,
                matches!(policy.execution, Some(RawWorkflowStageExecution::Continue)),
            ),
            None => (
                transition.ok_or_else(|| format!("stage {name:?} is missing policy.transition"))?,
                None,
                None,
                None,
                matches!(mode, Some(RawWorkflowStageExecution::Continue)),
            ),
        };

        // Legacy interleaved continue stage (old `post_action` compilation or
        // an `execution: "continue"` policy, including pinned workflow_def
        // snapshots): fold into the preceding stage's post. Stages swap
        // sessions; posts continue them.
        if continues {
            if let Some(previous) = stages.last_mut() {
                if previous.post.is_none() {
                    previous.post = Some(WorkflowPost {
                        name,
                        description,
                        agent,
                        prompt,
                        agent_provider,
                    });
                    continue;
                }
            }
        }

        let post = match (post, post_action) {
            (Some(post), _) => Some(WorkflowPost {
                name: post.name,
                description: post.description,
                agent: post.agent,
                prompt: post.prompt,
                agent_provider: post.agent_provider,
            }),
            (None, Some(post_action)) => Some(WorkflowPost {
                name: post_action.name,
                description: post_action.description,
                agent: post_action.agent,
                prompt: post_action.prompt,
                agent_provider: post_action.agent_provider,
            }),
            (None, None) => None,
        };

        stages.push(WorkflowStage {
            name,
            description,
            agent,
            prompt,
            agent_provider,
            environment,
            exits,
            budget,
            policy: WorkflowStagePolicy {
                transition,
                revision_transition,
                loop_transition,
                handoff,
            },
            post,
            exit_commit,
            setup,
            teardown,
        });
    }

    // A negative cap is a definition error, not "unlimited": accepting it
    // would silently disable the very bound the field configures.
    if let Some(revision_limit) = raw.revision_limit {
        if revision_limit < 0 {
            return Err(format!(
                "revision_limit must be zero or greater, got {revision_limit} \
                 (0 disables the cap; omit the field for the default of \
                 {DEFAULT_REVISION_LIMIT})"
            ));
        }
    }

    let workflow = WorkflowDefinition {
        name: raw.name,
        description: raw.description,
        stages,
        environments: raw.environments,
        revision_limit: raw.revision_limit,
        visibility: raw.visibility,
        plan_context: raw.plan_context,
        routing: raw.routing,
        budget: raw.budget,
    };
    validate_workflow_routing(&workflow)?;
    Ok(workflow)
}

/// The routing contract's own rules (spec §5), checked wherever a definition
/// is read — a repo file, a replacement, or a pinned snapshot — so a
/// definition cannot mix the two contracts or reach an exit that goes
/// nowhere.
fn validate_workflow_routing(workflow: &WorkflowDefinition) -> Result<(), String> {
    let uses_exit_fields = workflow.budget.is_some()
        || workflow.stages.iter().any(|stage| {
            stage.exits.is_some()
                || stage.budget.is_some()
                || stage.policy.loop_transition.is_some()
        });
    let uses_transition_fields = workflow
        .stages
        .iter()
        .any(|stage| stage.exit_commit || stage.setup.is_some() || stage.teardown.is_some());
    let uses_handoff = workflow
        .stages
        .iter()
        .any(|stage| stage.policy.handoff.is_some());
    if !workflow.routes_by_exits() {
        if uses_handoff {
            return Err(
                "policy.handoff belongs to named-exit routing; declare \"routing\": \"exits\" \
                 to use it (a legacy workflow hands off through its approve post)"
                    .into(),
            );
        }
        if uses_exit_fields {
            return Err(
                "exits, budget and loop_transition belong to named-exit routing; declare \
                 \"routing\": \"exits\" to use them"
                    .into(),
            );
        }
        if uses_transition_fields {
            return Err(
                "exit_commit and stage setup/teardown belong to named-exit routing; declare \
                 \"routing\": \"exits\" to use them (a legacy workflow commits through a \
                 post and runs scripts through its environments)"
                    .into(),
            );
        }
        return Ok(());
    }
    if workflow.revision_limit.is_some() {
        return Err(
            "routing \"exits\" budgets each destination stage (budget); revision_limit is \
             the legacy task-wide cap and cannot be combined with it"
                .into(),
        );
    }
    if workflow.plan_context.is_some() {
        return Err(
            "routing \"exits\" keeps the plan in the task ledger; plan_context belongs to \
             legacy plan publication"
                .into(),
        );
    }
    if let Some(budget) = workflow.budget.filter(|budget| *budget < 0) {
        return Err(format!("budget must be zero or greater, got {budget}"));
    }
    for (index, stage) in workflow.stages.iter().enumerate() {
        if stage.policy.revision_transition.is_some() {
            return Err(format!(
                "stage '{}': routing \"exits\" uses policy.loop_transition; \
                 revision_transition is the legacy revision policy",
                stage.name
            ));
        }
        if let Some(budget) = stage.budget.filter(|budget| *budget < 0) {
            return Err(format!(
                "stage '{}': budget must be zero or greater, got {budget}",
                stage.name
            ));
        }
        if stage
            .agent
            .as_deref()
            .is_some_and(|agent| agent.trim().is_empty())
        {
            return Err(format!(
                "stage '{}': agent must name a role; omit it for a stage without a role",
                stage.name
            ));
        }
        // The handoff runs where the legacy approve post's backstop runs: as
        // the task closes after its final stage.
        if stage.policy.handoff.is_some() && index + 1 != workflow.stages.len() {
            return Err(format!(
                "stage '{}': policy.handoff runs as the task leaves its final stage; declare it \
                 on the final stage",
                stage.name
            ));
        }
        if stage.exit_commit && stage.post.is_some() {
            return Err(format!(
                "stage '{}': exit_commit is the commit step of this stage's transition and \
                 cannot be combined with a post",
                stage.name
            ));
        }
        for (field, commands) in [("setup", &stage.setup), ("teardown", &stage.teardown)] {
            if commands
                .iter()
                .flatten()
                .any(|command| command.trim().is_empty())
            {
                return Err(format!(
                    "stage '{}': {field} commands must not be empty",
                    stage.name
                ));
            }
        }
        // A stage with no role enters, runs setup and parks until a person or
        // manager advances it (spec §5). The shapes that would need a session
        // to decide something, or a way in this build does not run, are
        // refused rather than run as something the definition does not say.
        if stage.is_roleless() {
            let refuse =
                |reason: &str| Err(format!("stage '{}' has no role, so {reason}", stage.name));
            if index == 0 {
                return refuse(
                    "it cannot be the first stage yet: task creation starts the first \
                     stage's agent",
                );
            }
            if stage.policy.transition != WorkflowStageTransition::Manual
                || stage
                    .policy
                    .loop_transition
                    .is_some_and(|transition| transition != WorkflowStageTransition::Manual)
            {
                return refuse(
                    "it parks until a person or manager advances it; its transition must be \
                     manual",
                );
            }
            if stage.exits.is_some() {
                return refuse("no session can name an exit; it declares none");
            }
            if stage.exit_commit || stage.post.is_some() {
                return refuse("no session can run a commit step or post on its way out");
            }
            if stage.prompt.is_some() || stage.agent_provider.is_some() {
                return refuse("it runs no agent; prompt and agent_provider do not apply");
            }
        }
        for (exit, destination) in stage.exits.iter().flatten() {
            let valid_name = exit
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_lowercase())
                && exit.chars().all(|character| {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || character == '_'
                        || character == '-'
                });
            if !valid_name {
                return Err(format!(
                    "stage '{}': exit name '{exit}' must be lowercase letters, digits, '_' or '-', \
                     starting with a letter",
                    stage.name
                ));
            }
            if exit == ADVANCE_EXIT {
                return Err(format!(
                    "stage '{}': '{ADVANCE_EXIT}' is every stage's implicit exit to the next \
                     stage and cannot be declared",
                    stage.name
                ));
            }
            // A loop goes back: to this stage or an earlier one. A forward
            // jump would be a route the linear stage order does not show.
            match workflow.stages[..=index]
                .iter()
                .position(|candidate| &candidate.name == destination)
            {
                // A loop re-enters its destination with a new session; a
                // stage without a role has none to re-enter in this build.
                Some(target) if workflow.stages[target].is_roleless() => {
                    return Err(format!(
                        "stage '{}': exit '{exit}' leads to '{destination}', a stage without a \
                         role; loops into such a stage are not supported yet",
                        stage.name
                    ))
                }
                Some(_) => {}
                None => {
                    return Err(format!(
                        "stage '{}': exit '{exit}' leads to '{destination}', which is not this \
                         stage or an earlier stage of the workflow",
                        stage.name
                    ))
                }
            }
        }
    }
    Ok(())
}

fn deserialize_optional_provider_list<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<AgentSelectionEntry>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    } // Historical stored snapshots.
    parse_selection_value(value, true)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

fn parse_selection_value(
    value: serde_json::Value,
    compact: bool,
) -> Result<Vec<AgentSelectionEntry>, String> {
    let values =
        match value {
            serde_json::Value::Array(values) => values,
            value @ (serde_json::Value::String(_) | serde_json::Value::Object(_)) => vec![value],
            _ => return Err(
                "agent_provider must be a string or an array of strings or structured candidates"
                    .into(),
            ),
        };
    if values.iter().any(|v| !v.is_string() && !v.is_object()) {
        return Err(
            "agent_provider must be a string or an array of strings or structured candidates"
                .into(),
        );
    }
    let entries = values
        .into_iter()
        .map(|value| {
            let value = match value {
                serde_json::Value::String(value) => serde_json::Value::String(value.trim().into()),
                value => value,
            };
            serde_json::from_value(value).map_err(|e| format!("invalid agent_provider: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_agent_selection(&entries, compact)
        .map_err(|e| format!("invalid agent_provider: {e}"))?;
    Ok(entries)
}

fn deserialize_optional_yaml_value<'de, D>(deserializer: D) -> Result<Option<YamlValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    YamlValue::deserialize(deserializer).map(Some)
}
