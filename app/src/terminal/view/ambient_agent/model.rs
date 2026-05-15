use instant::Instant;
use session_sharing_protocol::common::SessionId;
use warp_cli::agent::Harness;
use warp_core::features::FeatureFlag;
use warpui::{AppContext, Entity, EntityId, ModelContext, SingletonEntity};

use crate::ai::agent::{conversation::AIConversationId, extract_user_query_mode};
use crate::ai::ambient_agents::task::HarnessConfig;
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::execution_profiles::{AgentComputerUseState, ComputerUsePermission};
use crate::ai::harness_availability::HarnessAvailabilityModel;
use crate::ai::llms::{LLMId, LLMPreferences};
use crate::server::ids::{ServerId, SyncId};
use crate::server::server_api::ai::{AgentConfigSnapshot, AttachmentInput, SpawnAgentRequest};
use crate::terminal::view::ambient_agent::{SetupCommandGroupId, SetupCommandState};
use crate::terminal::CLIAgent;

use super::AmbientAgentProgressUIState;

/// Tracks progress timestamps for each step during ambient agent spawning.
#[derive(Debug, Clone)]
pub struct AgentProgress {
    /// When the agent run was requested.
    pub spawned_at: Instant,
    /// When the run was claimed by a worker.
    pub claimed_at: Option<Instant>,
    /// When the agent harness began executing.
    pub harness_started_at: Option<Instant>,
    /// When the agent stopped.
    pub stopped_at: Option<Instant>,
}

impl AgentProgress {
    fn new() -> Self {
        Self {
            spawned_at: Instant::now(),
            claimed_at: None,
            harness_started_at: None,
            stopped_at: None,
        }
    }
}

/// Identifies what kind of session startup the model is currently waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStartupKind {
    InitialRun,
    Followup,
}

/// Status of the ambient agent run.
#[derive(Debug, Clone)]
pub enum Status {
    /// First-time environment setup for agents.
    Setup,
    /// The user is composing their ambient agent prompt.
    Composing,
    /// Waiting for the ambient agent run to be ready.
    WaitingForSession {
        progress: AgentProgress,
        kind: SessionStartupKind,
    },
    /// The agent is running and the session is ready.
    AgentRunning,
    /// The agent failed.
    Failed {
        progress: AgentProgress,
        error_message: String,
    },
    /// The user needs to authenticate with GitHub.
    NeedsGithubAuth {
        progress: AgentProgress,
        error_message: String,
        auth_url: String,
    },
    /// The agent was cancelled.
    Cancelled { progress: AgentProgress },
}

/// Model to track the state of an ambient agent run.
pub struct AmbientAgentViewModel {
    status: Status,

    /// The request with which the agent was spawned, if it was spawned.
    request: Option<SpawnAgentRequest>,

    /// The terminal view this model is part of.
    terminal_view_id: EntityId,

    /// Selected cloud environment to launch the ambient agent with.
    environment_id: Option<SyncId>,

    /// UI state for rendering the ambient agent progress screen.
    pub ui_state: AmbientAgentProgressUIState,

    setup_commands_state: SetupCommandState,

    /// The task ID for the current agent task, if one has been spawned.
    task_id: Option<AmbientAgentTaskId>,

    /// The local conversation associated with this agent run, if any.
    /// Set for remote child agents spawned via `start_agent` so the `run_id`
    /// from the server response can be wired back to the conversation.
    conversation_id: Option<AIConversationId>,

    /// Selected execution harness for the agent run.
    /// Defaults to `Harness::Oz`. Used to populate `AgentConfigSnapshot.harness` on spawn.
    harness: Harness,
    /// Optional worker host value preserved for legacy task snapshots.
    worker_host: Option<String>,
    /// Whether the harness CLI (e.g. `claude`, `gemini`) has started running for a non-oz run.
    /// Used to transition the cloud-mode setup UI out of the pre-first-exchange phase when
    /// there is no oz `AppendedExchange` to key off of.
    harness_command_started: bool,

    /// Session ID for the currently running ambient execution, if the run has attached to a live
    /// shared session.
    active_execution_session_id: Option<SessionId>,
    /// Session ID for the most recently finished ambient execution.
    /// Used as the previous session ID when submitting a follow-up so polling can wait for a
    /// different fresh session after the prior execution has ended.
    last_ended_execution_session_id: Option<SessionId>,

    /// Prompt text for a follow-up that has been submitted but not yet attached to a new session.
    pending_followup_prompt: Option<String>,
}

impl AmbientAgentViewModel {
    pub fn new(terminal_view_id: EntityId, ctx: &mut ModelContext<Self>) -> Self {
        ctx.subscribe_to_model(&HarnessAvailabilityModel::handle(ctx), |me, _event, ctx| {
            me.validate_selected_harness(ctx);
        });

        let ui_state = AmbientAgentProgressUIState::new(ctx);

        let harness = Harness::default();
        let availability = HarnessAvailabilityModel::as_ref(ctx);
        // If the default harness is not available, find the first available one.
        let harness = if !availability.is_harness_enabled(harness) {
            availability
                .available_harnesses()
                .iter()
                .find(|h| h.enabled)
                .map(|h| h.harness)
                .unwrap_or(harness)
        } else {
            harness
        };

        Self {
            status: Status::Composing,
            request: None,
            terminal_view_id,
            environment_id: None,
            ui_state,
            setup_commands_state: Default::default(),
            task_id: None,
            conversation_id: None,
            harness,
            worker_host: None,
            harness_command_started: false,
            active_execution_session_id: None,
            last_ended_execution_session_id: None,
            pending_followup_prompt: None,
        }
    }

    pub fn request(&self) -> Option<&SpawnAgentRequest> {
        self.request.as_ref()
    }

    pub fn setup_command_state(&self) -> &SetupCommandState {
        &self.setup_commands_state
    }

    pub fn setup_command_state_mut(&mut self) -> &mut SetupCommandState {
        &mut self.setup_commands_state
    }

    pub(super) fn start_new_setup_command_group(&mut self, ctx: &mut ModelContext<Self>) {
        self.setup_commands_state.start_new_group();
        self.harness_command_started = false;
        ctx.emit(AmbientAgentViewModelEvent::UpdatedSetupCommandVisibility);
    }

    pub(super) fn finish_setup_command_group(
        &mut self,
        group_id: SetupCommandGroupId,
        ctx: &mut ModelContext<Self>,
    ) {
        if self.setup_commands_state.is_running(group_id) {
            self.setup_commands_state.finish_group(group_id);
            ctx.emit(AmbientAgentViewModelEvent::UpdatedSetupCommandVisibility);
        }
    }

    pub(super) fn set_setup_command_group_visibility(
        &mut self,
        group_id: SetupCommandGroupId,
        is_visible: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        if is_visible != self.setup_commands_state.should_expand(group_id) {
            self.setup_commands_state
                .set_should_expand(group_id, is_visible);
            ctx.emit(AmbientAgentViewModelEvent::UpdatedSetupCommandVisibility);
        }
    }

    pub(super) fn set_setup_command_visibility(
        &mut self,
        is_visible: bool,
        ctx: &mut ModelContext<Self>,
    ) {
        let group_id = self.setup_commands_state.current_group_id();
        self.set_setup_command_group_visibility(group_id, is_visible, ctx);
    }

    /// Returns the agent progress for tracking spawn steps.
    /// Returns `None` if not in the `WaitingForSession`, `Failed`, `NeedsGithubAuth`, or `Cancelled` state.
    pub fn agent_progress(&self) -> Option<&AgentProgress> {
        match &self.status {
            Status::WaitingForSession { progress, .. }
            | Status::Failed { progress, .. }
            | Status::NeedsGithubAuth { progress, .. }
            | Status::Cancelled { progress } => Some(progress),
            _ => None,
        }
    }

    /// Returns the currently selected environment ID.
    pub fn selected_environment_id(&self) -> Option<&SyncId> {
        self.environment_id.as_ref()
    }

    pub fn selected_harness(&self) -> Harness {
        self.harness
    }

    pub fn set_harness(&mut self, harness: Harness, ctx: &mut ModelContext<Self>) {
        if self.harness == harness {
            return;
        }
        self.harness = harness;
        ctx.emit(AmbientAgentViewModelEvent::HarnessSelected);
    }

    /// True when the run is configured to use a non-Oz execution harness and the
    /// required feature flags are enabled.
    pub(super) fn is_third_party_harness(&self) -> bool {
        FeatureFlag::AgentHarness.is_enabled() && self.selected_harness() != Harness::Oz
    }

    /// Returns the [`CLIAgent`] corresponding to the currently selected harness when it is a
    /// third-party harness (e.g. Claude, Gemini). Returns `None` for [`Harness::Oz`].
    /// Used to drive the correct tab icon for a cloud run as soon as a non-oz harness is
    /// selected, even before the CLI session is registered with [`CLIAgentSessionsModel`].
    pub fn selected_third_party_cli_agent(&self) -> Option<CLIAgent> {
        CLIAgent::from_harness(self.selected_harness())
    }

    /// Whether the harness CLI has started running. Only meaningful for non-oz runs.
    pub(super) fn harness_command_started(&self) -> bool {
        self.harness_command_started
    }

    /// Marks the harness CLI as started and emits `HarnessCommandStarted`.
    /// Idempotent: subsequent calls after the first are no-ops and do not re-emit.
    pub(super) fn mark_harness_command_started(&mut self, ctx: &mut ModelContext<Self>) {
        debug_assert!(
            self.harness != Harness::Oz,
            "harness_command_started is only meaningful for non-oz runs"
        );
        if self.harness_command_started {
            return;
        }
        self.harness_command_started = true;
        ctx.emit(AmbientAgentViewModelEvent::HarnessCommandStarted);
    }

    /// Sets the selected environment ID.
    pub fn set_environment_id(
        &mut self,
        environment_id: Option<SyncId>,
        ctx: &mut ModelContext<Self>,
    ) {
        self.environment_id = environment_id;
        ctx.emit(AmbientAgentViewModelEvent::EnvironmentSelected);
    }

    /// Resets to the first enabled harness if the current selection is no longer enabled.
    fn validate_selected_harness(&mut self, ctx: &mut ModelContext<Self>) {
        let model = HarnessAvailabilityModel::as_ref(ctx);
        if !model.is_harness_enabled(self.harness) {
            if let Some(first_enabled) = model.available_harnesses().iter().find(|h| h.enabled) {
                self.set_harness(first_enabled.harness, ctx);
            }
        }
    }

    /// Whether or not this terminal session is for an ambient agent.
    pub fn is_ambient_agent(&self) -> bool {
        true
    }

    /// Returns the task ID for the current agent task, if one has been spawned.
    pub fn task_id(&self) -> Option<AmbientAgentTaskId> {
        self.task_id
    }

    /// Whether or not this terminal session is in the setup state (first-time environment creation).
    pub fn is_in_setup(&self) -> bool {
        matches!(self.status, Status::Setup)
    }

    /// Whether or not this terminal session is currently setting up an ambient agent run.
    pub fn is_configuring_ambient_agent(&self) -> bool {
        matches!(self.status, Status::Composing)
    }

    /// Whether or not this terminal session is waiting for an ambient agent session to be ready.
    pub fn is_waiting_for_session(&self) -> bool {
        matches!(self.status, Status::WaitingForSession { .. })
    }

    /// Whether or not the ambient agent failed to spawn.
    pub fn is_failed(&self) -> bool {
        matches!(self.status, Status::Failed { .. })
    }

    /// Whether or not the ambient agent was cancelled.
    pub fn is_cancelled(&self) -> bool {
        matches!(self.status, Status::Cancelled { .. })
    }

    /// Whether or not the ambient agent needs GitHub authentication.
    pub fn is_needs_github_auth(&self) -> bool {
        matches!(self.status, Status::NeedsGithubAuth { .. })
    }

    /// Whether or not the ambient agent is currently running.
    pub fn is_agent_running(&self) -> bool {
        matches!(self.status, Status::AgentRunning)
    }

    /// Whether or not we should show a status footer (loading, error, auth, or cancelled).
    pub fn should_show_status_footer(&self) -> bool {
        if FeatureFlag::CloudModeSetupV2.is_enabled() {
            return false;
        }

        self.is_waiting_for_session()
            || self.is_failed()
            || self.is_needs_github_auth()
            || self.is_cancelled()
    }

    /// Returns the error message if the agent is in a failed state.
    pub fn error_message(&self) -> Option<&str> {
        match &self.status {
            Status::Failed { error_message, .. } => Some(error_message),
            _ => None,
        }
    }

    /// Returns the GitHub auth URL if the agent needs GitHub authentication.
    pub fn github_auth_url(&self) -> Option<&str> {
        match &self.status {
            Status::NeedsGithubAuth { auth_url, .. } => Some(auth_url),
            _ => None,
        }
    }

    /// Returns the error message for GitHub authentication failures.
    pub fn github_auth_error_message(&self) -> Option<&str> {
        match &self.status {
            Status::NeedsGithubAuth { error_message, .. } => Some(error_message),
            _ => None,
        }
    }

    /// Enter the setup state for first-time environment creation.
    pub fn enter_setup(&mut self, ctx: &mut ModelContext<Self>) {
        self.status = Status::Setup;
        ctx.emit(AmbientAgentViewModelEvent::EnteredSetupState);
    }

    /// Transition from Setup to Composing state.
    pub fn enter_composing_from_setup(&mut self, ctx: &mut ModelContext<Self>) {
        self.status = Status::Composing;
        ctx.emit(AmbientAgentViewModelEvent::EnteredComposingState);
    }

    /// This is used when we join an already-running ambient agent shared session (e.g. from the
    /// agent management view). We want the ambient agent UI affordances (like the environment
    /// selector) to be visible even though we did not spawn the agent from this view model.
    pub fn enter_viewing_existing_session(
        &mut self,
        task_id: AmbientAgentTaskId,
        ctx: &mut ModelContext<Self>,
    ) {
        self.task_id = Some(task_id);
        self.status = Status::AgentRunning;
        self.set_environment_id(None, ctx);
    }

    pub fn record_ambient_execution_ended(&mut self, session_id: SessionId) {
        if self.active_execution_session_id.as_ref() == Some(&session_id) {
            self.active_execution_session_id = None;
        }
        self.last_ended_execution_session_id = Some(session_id);
    }

    pub fn submit_cloud_followup(&mut self, prompt: String, ctx: &mut ModelContext<Self>) {
        let _ = (prompt, ctx);
        log::warn!("Cloud follow-up submission is disabled in this local-first build");
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn pending_followup_prompt(&self) -> Option<&str> {
        self.pending_followup_prompt.as_deref()
    }

    pub fn should_show_followup_progress(&self) -> bool {
        self.pending_followup_prompt.is_some()
            && matches!(
                self.status,
                Status::WaitingForSession { .. }
                    | Status::Failed { .. }
                    | Status::NeedsGithubAuth { .. }
                    | Status::Cancelled { .. }
            )
    }

    /// Reset hosted prompt state so a retained agent view can compose a new task locally.
    pub fn reset_for_new_cloud_prompt(&mut self, ctx: &mut ModelContext<Self>) {
        self.status = Status::Composing;
        self.environment_id = None;
        self.task_id = None;
        self.conversation_id = None;
        self.harness_command_started = false;
        self.active_execution_session_id = None;
        self.last_ended_execution_session_id = None;
        self.pending_followup_prompt = None;
        self.setup_commands_state = Default::default();
        ctx.notify();
    }

    /// Sets the local conversation ID associated with this agent run.
    pub fn set_conversation_id(&mut self, id: Option<AIConversationId>) {
        self.conversation_id = id;
    }

    /// Builds the default `AgentConfigSnapshot` for spawning a local agent from this pane.
    ///
    /// Reads the user's preferred model, computer-use autonomy, and the pane's
    /// currently-selected env and harness.
    pub(crate) fn build_default_spawn_config(&self, ctx: &AppContext) -> AgentConfigSnapshot {
        let model_id = LLMPreferences::as_ref(ctx)
            .get_active_base_model(ctx, Some(self.terminal_view_id))
            .id
            .to_string();

        // Determine computer_use_enabled based on workspace AI autonomy settings
        let AgentComputerUseState { enabled, .. } = ComputerUsePermission::resolve_agent_state(ctx);
        let computer_use_enabled = Some(enabled);

        let selected_harness = self.selected_harness();
        let harness_override = (selected_harness != Harness::Oz)
            .then(|| HarnessConfig::from_harness_type(selected_harness));

        AgentConfigSnapshot {
            environment_id: self.environment_id.as_ref().map(|id| id.to_string()),
            model_id: Some(model_id),
            computer_use_enabled,
            worker_host: self.worker_host.clone(),
            harness: harness_override,
            ..Default::default()
        }
    }

    /// Spawn an ambient agent with the given prompt and current session configuration.
    pub fn spawn_agent(
        &mut self,
        prompt: String,
        attachments: Vec<AttachmentInput>,
        ctx: &mut ModelContext<Self>,
    ) {
        let config = Some(self.build_default_spawn_config(ctx));

        let (prompt, mode) = extract_user_query_mode(prompt);
        let request = SpawnAgentRequest {
            prompt,
            mode,
            config,
            title: None,
            team: None,
            skill: None,
            attachments,
            interactive: None,
            parent_run_id: None,
            runtime_skills: vec![],
            referenced_attachments: vec![],
            conversation_id: None,
            agent_identity_uid: None,
        };

        self.spawn_internal(request, ctx);
    }

    /// Spawn an ambient agent with a fully-constructed request.
    pub fn spawn_agent_with_request(
        &mut self,
        request: SpawnAgentRequest,
        ctx: &mut ModelContext<Self>,
    ) {
        // Apply pane settings from the request.
        if let Some(config) = request.config.as_ref() {
            self.environment_id = config
                .environment_id
                .as_deref()
                .and_then(|id| ServerId::try_from(id).ok())
                .map(SyncId::ServerId);

            if let Some(model_id) = config.model_id.as_deref() {
                LLMPreferences::handle(ctx).update(ctx, |prefs, ctx| {
                    prefs.update_preferred_agent_mode_llm(
                        &LLMId::from(model_id),
                        self.terminal_view_id,
                        ctx,
                    )
                });
            }
        }

        self.spawn_internal(request, ctx);
    }

    /// Spawn an ambient agent given `request`.
    fn spawn_internal(&mut self, mut request: SpawnAgentRequest, ctx: &mut ModelContext<Self>) {
        request.interactive = Some(true);
        self.request = Some(request.clone());
        self.status = Status::WaitingForSession {
            progress: AgentProgress::new(),
            kind: SessionStartupKind::InitialRun,
        };
        ctx.emit(AmbientAgentViewModelEvent::DispatchedAgent);
        self.handle_spawn_error(
            "Hosted ambient agents are disabled in this local-first build. Use local Agent Mode with an OpenAI-compatible provider or a local CLI harness instead."
                .to_string(),
            ctx,
        );
    }

    /// Handles a spawn error by transitioning to the Failed state.
    fn handle_spawn_error(&mut self, error_message: String, ctx: &mut ModelContext<Self>) {
        let now = Instant::now();

        // Extract or create progress tracking.
        let progress = if let Status::WaitingForSession { mut progress, .. } =
            std::mem::replace(&mut self.status, Status::Composing)
        {
            progress.stopped_at = Some(now);
            progress
        } else {
            // If not in WaitingForSession, create a new progress with current time.
            AgentProgress {
                spawned_at: now,
                claimed_at: None,
                harness_started_at: None,
                stopped_at: Some(now),
            }
        };

        self.status = Status::Failed {
            progress,
            error_message: error_message.clone(),
        };
        self.pending_followup_prompt = None;
        ctx.emit(AmbientAgentViewModelEvent::Failed { error_message });
    }

    /// Handles cancellation by transitioning to the Cancelled state.
    fn handle_cancellation(&mut self, ctx: &mut ModelContext<Self>) {
        let now = Instant::now();

        // Extract or create progress tracking.
        let progress = if let Status::WaitingForSession { mut progress, .. } =
            std::mem::replace(&mut self.status, Status::Composing)
        {
            progress.stopped_at = Some(now);
            progress
        } else {
            // If not in WaitingForSession, create a new progress with current time.
            AgentProgress {
                spawned_at: now,
                claimed_at: None,
                harness_started_at: None,
                stopped_at: Some(now),
            }
        };

        self.status = Status::Cancelled { progress };
        self.pending_followup_prompt = None;

        ctx.emit(AmbientAgentViewModelEvent::Cancelled);
    }

    /// Cancels the ambient agent task if one is currently running.
    pub fn cancel_task(&mut self, ctx: &mut ModelContext<Self>) {
        if !self.is_waiting_for_session() {
            log::warn!("Attempted to cancel ambient agent task but not in WaitingForSession state");
            return;
        }

        self.handle_cancellation(ctx);
    }
}

/// Events emitted by the ambient agent view model.
#[derive(Debug, Clone)]
pub enum AmbientAgentViewModelEvent {
    /// The user has entered the setup state (first-time environment creation).
    EnteredSetupState,
    /// The user has entered the composing state (typing their prompt).
    EnteredComposingState,
    /// The ambient agent run has been dispatched.
    DispatchedAgent,
    /// A follow-up execution has been submitted and is waiting for a new session.
    FollowupDispatched,
    /// The spawn progress has been updated (e.g., task claimed or in-progress).
    ProgressUpdated,
    /// An environment was selected.
    EnvironmentSelected,
    /// The ambient agent failed.
    Failed {
        error_message: String,
    },
    /// The ambient agent needs GitHub authentication.
    NeedsGithubAuth,
    /// The ambient agent was cancelled.
    Cancelled,
    /// The selected execution harness (Oz / Claude Code) changed.
    HarnessSelected,
    /// The harness CLI (for non-oz runs) has started executing in the shared session.
    /// Fires once per run and signals the transition out of the pre-first-exchange phase
    /// for claude / gemini / other third-party harnesses.
    HarnessCommandStarted,
    UpdatedSetupCommandVisibility,
}

impl Entity for AmbientAgentViewModel {
    type Event = AmbientAgentViewModelEvent;
}
