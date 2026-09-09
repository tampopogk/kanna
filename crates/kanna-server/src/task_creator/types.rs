use super::definitions::WorkflowStageTransition;
use super::provider::AgentProvider;
use kanna_daemon::protocol::AgentProvider as DaemonAgentProvider;
use std::collections::HashMap;

pub(super) struct TaskCreationRequest {
    pub(super) requested_task_id: Option<String>,
    /// Canonical API create request retained until the first running stage
    /// run is durable, so an interrupted prepared spawn can be reconstructed.
    pub(super) create_intent_json: Option<String>,
    pub(super) task_prompt: String,
    pub(super) display_name: Option<String>,
    pub(super) workflow_name: Option<String>,
    pub(super) workflow_def: Option<String>,
    pub(super) base_ref: Option<String>,
    pub(super) stored_base_ref: Option<String>,
    pub(super) stage_override: Option<String>,
    pub(super) agent: Option<String>,
    pub(super) explicit_provider: Option<String>,
    pub(super) default_provider: Option<String>,
    pub(super) agent_type: Option<String>,
    pub(super) initial_terminal_geometry: Option<(u16, u16)>,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) permission_mode: Option<String>,
    pub(super) allowed_tools: Vec<String>,
    pub(super) disallowed_tools: Vec<String>,
    pub(super) max_turns: Option<u32>,
    pub(super) max_budget_usd: Option<f64>,
    pub(super) setup_cmds: Vec<String>,
    pub(super) task_template: Option<crate::mobile_api::TaskTemplateLaunch>,
    pub(super) resume_session_id: Option<String>,
    pub(super) recovery_snapshot: Option<crate::mobile_api::CreateTaskRecoverySnapshot>,
    /// Display-only import notice for a task arriving by cross-machine
    /// transfer; printed once into the destination PTY before the agent runs.
    pub(super) transfer_import: Option<crate::mobile_api::TransferImportSummary>,
    /// Retired request compatibility field. Resolution reads and discards it;
    /// the public HTTP boundary rejects non-null values.
    pub(super) notify_task_id: Option<String>,
    pub(super) parent_task_id: Option<String>,
}

/// Caller-supplied overrides for a repo-scoped singleton agent task. They are
/// only consulted when the signal creates the task: an already-running
/// singleton keeps the provider and effort its session was spawned with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SingletonAgentOverrides {
    pub(crate) agent_provider: Option<String>,
    pub(crate) effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrepareTaskError {
    RequestedTaskIdAlreadyExists,
    InvalidRequest(String),
    Other(String),
}

impl From<String> for PrepareTaskError {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

impl From<rusqlite::Error> for PrepareTaskError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Other(format!("db error: {error}"))
    }
}

impl std::fmt::Display for PrepareTaskError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RequestedTaskIdAlreadyExists => formatter.write_str("task id already exists"),
            Self::InvalidRequest(error) | Self::Other(error) => formatter.write_str(error),
        }
    }
}

#[derive(Clone)]
pub(super) struct CreatedTask {
    pub(super) task_id: String,
    pub(super) repo_id: String,
    pub(super) title: String,
    pub(super) prompt: String,
    pub(super) stage: String,
    pub(super) agent_type: String,
    pub(super) worktree_path: String,
}

#[derive(Clone)]
pub(crate) struct PreparedTaskSpawn {
    pub(super) created_task: CreatedTask,
    pub(super) branch: String,
    pub(super) session_id: String,
    pub(super) cwd: String,
    pub(super) env: HashMap<String, String>,
    pub(super) stage_agent: Option<String>,
    pub(super) agent_provider: String,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) completion_transition: WorkflowStageTransition,
    /// The agent CLI's own session id assigned at spawn (Claude or Copilot
    /// PTY); recorded on the stage run so a later recovery can resume it.
    pub(super) provider_session_id: Option<String>,
    pub(super) recovery_snapshot: Option<crate::mobile_api::CreateTaskRecoverySnapshot>,
    pub(super) session: PreparedSessionSpawn,
    /// The startup terminal this launch runs its setup in, before the agent
    /// starts. Present only for a PTY launch with setup commands; the session
    /// above is provisional while it is, and is rebuilt from the startup
    /// shell's own environment once that terminal exits cleanly.
    pub(super) setup_terminal: Option<super::setup_session::SetupTerminalPlan>,
    pub(super) deferred_launch: Option<DeferredNewTaskLaunch>,
}

/// What it takes to build a new task's agent session *after* its startup
/// terminal has run. The provider is already bound and stamped on the task —
/// setup may install it, but it does not get to change which one this task is
/// — so only the executable, the search path, and the shell command are
/// resolved late, against the environment setup actually left behind.
#[derive(Clone)]
pub(super) struct DeferredNewTaskLaunch {
    pub(super) provider: AgentProvider,
    pub(super) agent_type: super::provider::AgentSessionType,
    pub(super) stage_name: String,
    pub(super) workflow_name: String,
    pub(super) stage_transition: String,
    pub(super) final_prompt: String,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) permission_mode: Option<String>,
    pub(super) allowed_tools: Vec<String>,
    pub(super) disallowed_tools: Vec<String>,
    pub(super) max_turns: Option<u32>,
    pub(super) max_budget_usd: Option<f64>,
    pub(super) mcp_config_path: Option<String>,
    pub(super) resume_session_id: Option<String>,
    pub(super) transfer_import: Option<crate::mobile_api::TransferImportSummary>,
    pub(super) local_config_override: Option<super::local_config::LocalConfigOverride>,
    pub(super) geometry: Option<(u16, u16)>,
}

impl PreparedTaskSpawn {
    pub(crate) fn task_id(&self) -> &str {
        &self.created_task.task_id
    }

    /// Whether this launch runs its setup in a startup terminal first.
    ///
    /// A caller on a request path uses this to decide whether it can wait for
    /// the agent: startup is repo work of unbounded length — a dependency
    /// install, a container build — and a create request that blocks on it
    /// would time out long before the terminal it is waiting for finished
    /// printing.
    pub(crate) fn has_setup_terminal(&self) -> bool {
        self.setup_terminal.is_some()
    }

    /// The startup terminal's shell command, for tests that assert setup runs
    /// there rather than inside the agent's own shell.
    #[cfg(test)]
    pub(crate) fn setup_terminal_command(&self) -> Option<&str> {
        self.setup_terminal
            .as_ref()
            .map(|plan| plan.command.as_str())
    }

    /// The response describing the task this launch created. Everything in it
    /// is settled before the agent starts, which is what lets a launch with a
    /// startup terminal answer its caller and finish in the background.
    pub(crate) fn create_response(&self) -> crate::mobile_api::CreateTaskResponse {
        crate::mobile_api::CreateTaskResponse {
            task_id: self.created_task.task_id.clone(),
            repo_id: self.created_task.repo_id.clone(),
            title: self.created_task.title.clone(),
            prompt: self.created_task.prompt.clone(),
            stage: self.created_task.stage.clone(),
            agent_type: self.created_task.agent_type.clone(),
            worktree_path: Some(self.created_task.worktree_path.clone()),
        }
    }
}

#[derive(Clone)]
pub(crate) enum PreparedSessionSpawn {
    Pty {
        executable: String,
        args: Vec<String>,
        cols: u16,
        rows: u16,
        /// The provider whose detection rules this session's output is read
        /// with. `None` is a plain terminal — a startup or teardown shell —
        /// and is what keeps its output out of prompt and composer detection.
        agent_provider: Option<DaemonAgentProvider>,
        /// The provider CLI this shell command line runs, resolved here to an
        /// absolute path. `executable` is the login shell that runs repo setup
        /// first, so the daemon cannot find the CLI by inspecting its own
        /// child; it probes this path for the version its detection rules are
        /// selected by.
        agent_executable: Option<String>,
    },
    Agent {
        agent_provider: DaemonAgentProvider,
        prompt: String,
        model: Option<String>,
        effort: Option<String>,
        permission_mode: Option<String>,
        allowed_tools: Vec<String>,
        disallowed_tools: Vec<String>,
        max_turns: Option<u32>,
        max_budget_usd: Option<f64>,
        system_prompt: String,
        mcp_config_path: Option<String>,
        executable: Option<String>,
    },
}

/// Stage transitions are durable: an in-workflow hop starts a new stage run on
/// the SAME task (`Run`), a stage with pending tail work dispatches its post
/// into the running session (`Post`), and advancing past the final stage
/// closes the task (`Close`). No transition ever creates a new task or
/// worktree.
pub(crate) enum PreparedStageTransition {
    Run(Box<PreparedStageRunSpawn>),
    Post(Box<PreparedPostDispatch>),
    Close {
        task_id: String,
        /// Teardown for the final workspace the close leaves behind.
        workspace_teardown: Option<Box<PreparedWorkspaceTeardown>>,
    },
}

/// A stage's post, ready to be injected into the task's live agent session.
/// `fallback` spawns the post as a fresh session (with the post's agent) when
/// the live session turns out to be dead.
pub(crate) struct PreparedPostDispatch {
    pub(super) task_id: String,
    pub(super) session_id: String,
    /// Post prompt (with $VAR substitution) plus the completion reminder,
    /// submitted through the task-input path.
    pub(super) message: String,
    /// Run-history label: the post's name.
    pub(super) run_stage: String,
    pub(super) fallback: PreparedStageRunSpawn,
}

pub(crate) struct PreparedStageRerun {
    pub(super) task_id: String,
    pub(super) session_id: String,
    /// Run-history label: the stage name, or the post name when rerunning a
    /// legacy task parked at a folded post stage.
    pub(super) stage: String,
    pub(super) run_kind: &'static str,
    pub(super) stage_agent: Option<String>,
    pub(super) agent_provider: String,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    /// Carried forward from the run this rerun reproduces, so the durable
    /// record keeps naming whoever picked the model it respawns with.
    pub(super) provider_override: Option<crate::db::StageProviderOverride>,
    pub(super) completion_transition: WorkflowStageTransition,
    pub(super) provider_session_id: Option<String>,
    pub(super) cwd: String,
    pub(super) env: HashMap<String, String>,
    /// Headless reruns execute setup only after the prior session is killed,
    /// then resolve their executable from the initialized workspace.
    pub(super) deferred_setup: Vec<String>,
    /// A PTY rerun runs its setup in a startup terminal of its own, like every
    /// other launch, and rebuilds the agent session from what that shell
    /// leaves behind.
    pub(super) setup_terminal: Option<super::setup_session::SetupTerminalPlan>,
    pub(super) deferred_launch: Option<DeferredNewTaskLaunch>,
    pub(super) recovery_snapshot: Option<crate::mobile_api::CreateTaskRecoverySnapshot>,
    pub(super) session: PreparedSessionSpawn,
}

impl PreparedStageRerun {
    /// The startup terminal's shell command, for tests that assert a rerun's
    /// setup runs there rather than inside the agent's own shell.
    #[cfg(test)]
    pub(crate) fn setup_terminal_command(&self) -> Option<&str> {
        self.setup_terminal
            .as_ref()
            .map(|plan| plan.command.as_str())
    }
}

/// A stage-run workspace forked from the task's committed tip: swaps get a
/// fresh branch + worktree (N worktrees, N branches, one PR — the PR agent
/// renames the final random branch into something meaningful). The previous
/// worktree stays on disk until cleanup; only committed work crosses the
/// boundary.
pub(crate) struct ForkedWorkspace {
    pub(super) branch: String,
    pub(super) worktree_path: String,
}

/// Input to `prepare_stage_run_spawn`: which workspace the run should use.
pub(super) enum RunWorkspaceSpec {
    /// Keep the task's current workspace (post fallbacks, reruns).
    Current,
    /// Fork a fresh branch + worktree from the task's committed tip.
    Fork { branch: String },
    /// Adopt a previous run's workspace and resume its agent-CLI session.
    Resume(ResumeWorkspaceSpec),
}

pub(super) struct ResumeWorkspaceSpec {
    /// The previous run's worktree (still on disk).
    pub(super) cwd: String,
    /// Branch checked out in that worktree; the task's branch moves back to
    /// it.
    pub(super) branch: String,
    /// The agent-CLI session id to resume.
    pub(super) provider_session_id: String,
    /// The stage run whose session is being resumed.
    pub(super) resumed_from_run_id: String,
}

/// Where a prepared stage run executes, and what the spawn must do about it.
pub(crate) enum PreparedRunWorkspace {
    /// The task's current workspace (post fallbacks, reruns).
    Current,
    /// A freshly created branch + worktree; the spawn moves
    /// `pipeline_item.branch` and the worktree record with it, and rolls the
    /// fork back if the daemon spawn fails.
    Forked(ForkedWorkspace),
    /// A previous run's still-on-disk workspace adopted for a resumed
    /// revision: moves the task's branch and worktree record like a fork,
    /// but was never created here and is never rolled back.
    Resumed(ForkedWorkspace),
}

/// A new stage run spawned on an existing task: same task id, but a swap runs
/// in a freshly forked workspace, a resumed revision reopens a previous run's
/// workspace, and a post fallback or rerun keeps the task's current one.
pub(crate) struct PreparedStageRunSpawn {
    pub(super) task_id: String,
    pub(super) session_id: String,
    /// Value written to `pipeline_item.stage`. For a post fallback spawn this
    /// is the owning stage (a post never moves the task's stage).
    pub(super) next_stage: String,
    /// Run-history label (`stage_run.stage`): the stage name, or the post's
    /// name for a post fallback spawn.
    pub(super) run_stage: String,
    /// `stage_run.kind`: "main" or "post".
    pub(super) run_kind: &'static str,
    pub(super) workspace: PreparedRunWorkspace,
    /// Teardown for the workspace this run leaves behind (forked swaps only);
    /// spawned after the transition succeeds, never on rollback.
    pub(super) workspace_teardown: Option<PreparedWorkspaceTeardown>,
    pub(super) stage_agent: Option<String>,
    pub(super) agent_provider: String,
    pub(super) model: Option<String>,
    pub(super) effort: Option<String>,
    pub(super) completion_transition: WorkflowStageTransition,
    /// How this run's stage was entered. This is caller-declared for explicit
    /// advances and server-owned for automatic policy transitions.
    pub(super) trigger: crate::db::StageTrigger,
    /// The provider override the advance that started this run carried, with
    /// the source that declared it. Recorded on the run so the durable record
    /// says who picked this stage's model.
    pub(super) provider_override: Option<crate::db::StageProviderOverride>,
    pub(super) feedback: Option<String>,
    /// The agent CLI's own session id this run starts (fresh assign) or
    /// continues (resume); recorded on the stage run.
    pub(super) provider_session_id: Option<String>,
    /// Set on a resumed revision: the stage run whose provider session this
    /// run continues.
    pub(super) resumed_from_run_id: Option<String>,
    /// Why a requested resume became a fresh provider conversation.
    pub(super) resume_fallback_reason: Option<String>,
    pub(super) cwd: String,
    pub(super) env: HashMap<String, String>,
    /// Ordered bytes seeded into a replacement PTY's terminal history before
    /// the new stage's process output. Absent for non-transition spawns.
    pub(super) terminal_prelude: Option<Vec<u8>>,
    pub(super) session: PreparedSessionSpawn,
    /// Repository setup is deliberately finalized by the detached transition
    /// worker. The provisional provider/session above are never spawned while
    /// this is present.
    pub(super) deferred_setup: Option<DeferredStageSetup>,
    /// Test seam: when armed, this launch's startup terminal reports its hard
    /// timeout. See `setup_session::run_setup_terminal_with_armed_timeout`.
    #[cfg(test)]
    pub(super) setup_timeout_signal: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

pub(super) struct DeferredStageSetup {
    pub(super) commands: Vec<String>,
    pub(super) provider_candidates: Vec<AgentProvider>,
    pub(super) source_agent_type: Option<String>,
    pub(super) workflow_name: String,
    pub(super) final_prompt: String,
    pub(super) tuning: super::provider::AgentTuningPlan,
    pub(super) permission_mode: Option<String>,
    pub(super) allowed_tools: Vec<String>,
    pub(super) mcp_config_path: Option<String>,
    pub(super) resume_session_id: Option<String>,
    pub(super) local_config_override: Option<super::local_config::LocalConfigOverride>,
}

/// Detached best-effort cleanup for a workspace that the task is leaving.
/// The teardown runs in the departed worktree and uses a session id tied to
/// that worktree's branch so it does not collide with future task workspaces.
pub(crate) struct PreparedWorkspaceTeardown {
    pub(crate) session_id: String,
    pub(super) daemon_dir: String,
    pub(super) db_path: String,
    pub(super) task_id: String,
    pub(super) cwd: String,
    pub(super) env: HashMap<String, String>,
    pub(super) session: PreparedSessionSpawn,
}

impl PreparedStageRunSpawn {
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    #[cfg(test)]
    pub(crate) fn has_deferred_setup(&self) -> bool {
        self.deferred_setup.is_some()
    }

    #[cfg(test)]
    pub(crate) fn set_setup_timeout_signal(
        &mut self,
        signal: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        self.setup_timeout_signal = Some(signal);
    }

    /// The freshly created workspace, when this run forked one.
    #[cfg(test)]
    pub(crate) fn forked_workspace(&self) -> Option<&ForkedWorkspace> {
        match &self.workspace {
            PreparedRunWorkspace::Forked(workspace) => Some(workspace),
            _ => None,
        }
    }

    /// The previous run's workspace adopted by a resumed revision.
    #[cfg(test)]
    pub(crate) fn resumed_workspace(&self) -> Option<&ForkedWorkspace> {
        match &self.workspace {
            PreparedRunWorkspace::Resumed(workspace) => Some(workspace),
            _ => None,
        }
    }
}
