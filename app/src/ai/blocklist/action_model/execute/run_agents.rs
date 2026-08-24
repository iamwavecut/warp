//! Async executor for `AIAgentActionType::RunAgents`.
//!
//! Fans out per-child via [`super::start_agent::StartAgentExecutor::dispatch`]
//! and aggregates the outcomes into a single `RunAgentsResult`.
use std::collections::HashMap;
use std::time::Duration;

use ai::agent::action::{RunAgentsAgentRunConfig, RunAgentsExecutionMode, RunAgentsRequest};
use ai::agent::action_result::{
    RunAgentsAgentOutcome, RunAgentsAgentOutcomeKind, RunAgentsLaunchedExecutionMode,
    RunAgentsResult,
};
use ai::agent::orchestration_config::OrchestrationConfig;
use ai::skills::SkillReference;

use crate::ai::blocklist::inline_action::orchestration_controls::OrchestrationEditState;
use futures::{FutureExt, future::BoxFuture};
use warp_core::execution_mode::AppExecutionMode;
use warpui::{Entity, EntityId, ModelContext, ModelHandle};

use super::start_agent::{StartAgentExecutor, StartAgentOutcome};
use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::{
    AIAgentAction, AIAgentActionId, AIAgentActionResultType, AIAgentActionType,
    StartAgentExecutionMode,
};
use crate::ai::blocklist::{BlocklistAIHistoryModel, BlocklistAIPermissions};
use crate::ai::local_agent_registry::MAX_LOCAL_CHILD_FANOUT;
use warpui::SingletonEntity;

/// Per-child spawn timeout. If a child agent doesn't report back within
/// this window (e.g. binary not found, server error), the slot is failed
/// rather than hanging the "Spawning agents" UI indefinitely.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Snapshot of an in-flight dispatch, carried through
/// [`RunAgentsExecutorEvent::SpawningStarted`].
#[derive(Debug, Clone, Copy)]
pub struct RunAgentsSpawningSnapshot {
    pub agent_count: usize,
}

/// In-flight tracking per `RunAgents` action (idempotency guard).
struct PendingRunAgents;

pub struct RunAgentsExecutor {
    pending: HashMap<AIAgentActionId, PendingRunAgents>,
    start_agent_executor: ModelHandle<StartAgentExecutor>,
    terminal_view_id: EntityId,
}

/// Lifecycle events for in-flight dispatches.
pub enum RunAgentsExecutorEvent {
    SpawningStarted {
        action_id: AIAgentActionId,
        snapshot: RunAgentsSpawningSnapshot,
    },
    SpawningFinished {
        action_id: AIAgentActionId,
    },
}

impl Entity for RunAgentsExecutor {
    type Event = RunAgentsExecutorEvent;
}

impl RunAgentsExecutor {
    pub fn new(
        start_agent_executor: ModelHandle<StartAgentExecutor>,
        terminal_view_id: EntityId,
    ) -> Self {
        Self {
            pending: HashMap::new(),
            start_agent_executor,
            terminal_view_id,
        }
    }

    pub fn is_pending(&self, action_id: &AIAgentActionId) -> bool {
        self.pending.contains_key(action_id)
    }

    /// Fans out a prepared request into per-child dispatches and returns a
    /// receiver for the aggregate `RunAgentsResult`. Validation failures
    /// short-circuit synchronously.
    fn dispatch_prepared_run_agents(
        &mut self,
        action_id: AIAgentActionId,
        request: RunAgentsRequest,
        parent_conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) -> async_channel::Receiver<RunAgentsResult> {
        let (sender, receiver) = async_channel::bounded(1);

        if self.pending.contains_key(&action_id) {
            log::warn!("RunAgentsExecutor: dispatch reentered for {action_id:?}; rejecting");
            let _ = sender.try_send(RunAgentsResult::Cancelled);
            return receiver;
        }

        if let Err(error) = validate_request(&request) {
            log::warn!("RunAgentsExecutor: validation failure: {error}");
            let _ = sender.try_send(RunAgentsResult::Failure { error });
            return receiver;
        }

        let snapshot = RunAgentsSpawningSnapshot {
            agent_count: request.agent_run_configs.len(),
        };
        self.pending.insert(action_id.clone(), PendingRunAgents);
        ctx.emit(RunAgentsExecutorEvent::SpawningStarted {
            action_id: action_id.clone(),
            snapshot,
        });

        let parent_run_id = BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&parent_conversation_id)
            .and_then(|c| c.run_id());

        let RunAgentsRequest {
            execution_mode: run_execution_mode,
            harness_type,
            model_id,
            skills,
            agent_run_configs,
            base_prompt,
            ..
        } = request;

        let mut slots: Vec<ChildSlot> = Vec::with_capacity(agent_run_configs.len());
        for cfg in &agent_run_configs {
            let prompt = compose_run_agents_child_prompt(&base_prompt, &cfg.prompt);
            let mode = match run_agents_to_start_agent_mode(
                &run_execution_mode,
                &harness_type,
                &model_id,
                &skills,
                cfg,
            ) {
                Ok(mode) => mode,
                Err(err) => {
                    slots.push(ChildSlot::Failed(err));
                    continue;
                }
            };
            let recv = self.start_agent_executor.update(ctx, |executor, exec_ctx| {
                executor.dispatch(
                    cfg.name.clone(),
                    prompt,
                    mode,
                    None, /* lifecycle_subscription */
                    parent_conversation_id,
                    parent_run_id.clone(),
                    exec_ctx,
                )
            });
            slots.push(ChildSlot::Pending(recv));
        }

        let agent_run_configs_for_result = agent_run_configs.clone();
        let action_id_for_aggr = action_id.clone();
        let run_model_id = model_id.clone();
        let run_harness_type = harness_type.clone();
        let run_execution_mode_for_aggr = run_execution_mode.clone();

        ctx.spawn(
            async move {
                let mut outcomes: Vec<RunAgentsAgentOutcomeKind> = Vec::with_capacity(slots.len());
                for slot in slots {
                    let kind = match slot {
                        ChildSlot::Failed(error) => RunAgentsAgentOutcomeKind::Failed { error },
                        ChildSlot::Pending(recv) => {
                            let timeout = warpui::r#async::Timer::after(SPAWN_TIMEOUT);
                            match futures::future::select(Box::pin(recv.recv()), Box::pin(timeout))
                                .await
                            {
                                futures::future::Either::Left((
                                    Ok(StartAgentOutcome::Started { agent_id }),
                                    _,
                                )) => RunAgentsAgentOutcomeKind::Launched { agent_id },
                                futures::future::Either::Left((
                                    Ok(StartAgentOutcome::Error(error)),
                                    _,
                                )) => RunAgentsAgentOutcomeKind::Failed { error },
                                futures::future::Either::Left((Err(_), _)) => {
                                    RunAgentsAgentOutcomeKind::Failed {
                                        error: "Cancelled before launch".to_string(),
                                    }
                                }
                                futures::future::Either::Right((_, _)) => {
                                    log::warn!(
                                        "Agent spawn timed out after {} seconds",
                                        SPAWN_TIMEOUT.as_secs()
                                    );
                                    RunAgentsAgentOutcomeKind::Failed {
                                        error: format!(
                                            "Agent failed to start within {} seconds. \
                                             The harness binary may not be installed.",
                                            SPAWN_TIMEOUT.as_secs()
                                        ),
                                    }
                                }
                            }
                        }
                    };
                    outcomes.push(kind);
                }
                outcomes
            },
            move |me, outcomes, ctx| {
                let agents: Vec<RunAgentsAgentOutcome> = agent_run_configs_for_result
                    .iter()
                    .zip(outcomes)
                    .map(|(cfg, kind)| RunAgentsAgentOutcome {
                        name: cfg.name.clone(),
                        kind,
                    })
                    .collect();
                let launched_mode = match &run_execution_mode_for_aggr {
                    RunAgentsExecutionMode::Local => RunAgentsLaunchedExecutionMode::Local,
                    RunAgentsExecutionMode::Remote { .. } => RunAgentsLaunchedExecutionMode::Local,
                };
                let result = RunAgentsResult::Launched {
                    model_id: run_model_id,
                    harness_type: run_harness_type,
                    execution_mode: launched_mode,
                    agents,
                };
                me.pending.remove(&action_id_for_aggr);
                ctx.emit(RunAgentsExecutorEvent::SpawningFinished {
                    action_id: action_id_for_aggr,
                });
                let _ = sender.try_send(result);
            },
        );

        receiver
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> + use<> {
        let AIAgentAction { action, id, .. } = input.action;
        let AIAgentActionType::RunAgents(request) = action else {
            return ActionExecution::InvalidAction;
        };
        let mut request = request.clone();
        let action_id = id.clone();
        let parent_conversation_id = input.conversation_id;
        if let Some(reason) = prepare_request_for_execution(
            &mut request,
            parent_conversation_id,
            self.terminal_view_id,
            ctx,
        ) {
            return ActionExecution::Sync(AIAgentActionResultType::RunAgents(
                RunAgentsResult::Denied {
                    reason: reason.to_string(),
                },
            ));
        }

        let receiver =
            self.dispatch_prepared_run_agents(action_id, request, parent_conversation_id, ctx);

        ActionExecution::new_async(
            async move { receiver.recv().await },
            |result, _| match result {
                Ok(r) => AIAgentActionResultType::RunAgents(r),
                Err(_) => AIAgentActionResultType::RunAgents(RunAgentsResult::Cancelled),
            },
        )
    }

    pub(super) fn should_autoexecute(
        &self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        let AIAgentActionType::RunAgents(request) = &input.action.action else {
            return false;
        };
        if AppExecutionMode::as_ref(ctx).is_autonomous() {
            return true;
        }
        approved_orchestration_config_can_autoexecute(request, input.conversation_id, ctx)
            || BlocklistAIPermissions::as_ref(ctx)
                .get_run_agents_setting(ctx, Some(self.terminal_view_id))
                .is_always_allow()
    }

    pub(super) fn preprocess_action(
        &mut self,
        _action: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }
}

enum ChildSlot {
    Failed(String),
    Pending(async_channel::Receiver<StartAgentOutcome>),
}

fn approved_orchestration_config_can_autoexecute(
    request: &RunAgentsRequest,
    parent_conversation_id: AIConversationId,
    ctx: &ModelContext<RunAgentsExecutor>,
) -> bool {
    let mut resolved_request = request.clone();
    resolve_request_from_approved_config(&mut resolved_request, parent_conversation_id, ctx)
        .is_some_and(|status| status.is_approved())
}

fn resolve_request_from_approved_config(
    request: &mut RunAgentsRequest,
    parent_conversation_id: AIConversationId,
    ctx: &ModelContext<RunAgentsExecutor>,
) -> Option<ai::agent::orchestration_config::OrchestrationConfigStatus> {
    let conversation =
        BlocklistAIHistoryModel::as_ref(ctx).conversation(&parent_conversation_id)?;
    let (config, status) = conversation.orchestration_config_for_plan(&request.plan_id)?;
    if status.is_approved() {
        resolve_request_from_config(request, config);
    }
    Some(status)
}

/// Normalizes the request and returns a denial reason when launch is blocked.
///
/// Autonomous agents always run: their calls may still inherit approved plan
/// config fields, but they bypass interactive policy denials because they cannot
/// present a confirmation card.
fn prepare_request_for_execution(
    request: &mut RunAgentsRequest,
    parent_conversation_id: AIConversationId,
    terminal_view_id: EntityId,
    ctx: &ModelContext<RunAgentsExecutor>,
) -> Option<&'static str> {
    let status = resolve_request_from_approved_config(request, parent_conversation_id, ctx);

    if AppExecutionMode::as_ref(ctx).is_autonomous() {
        return None;
    }

    if status.is_some_and(|status| status.is_disapproved()) {
        return Some("Orchestration config was disapproved");
    }

    if BlocklistAIPermissions::as_ref(ctx)
        .get_run_agents_setting(ctx, Some(terminal_view_id))
        .is_never_allow()
    {
        return Some("Running child agents is disabled by the active execution profile.");
    }

    None
}

/// Unconditionally overrides run-wide fields on a `RunAgentsRequest`
/// from the approved orchestration config, delegating to
/// `OrchestrationEditState::override_from_approved_config`.
fn resolve_request_from_config(request: &mut RunAgentsRequest, config: &OrchestrationConfig) {
    // The approved plan config is the source of truth for these run-wide fields,
    // so callers pass a mutable request and continue with the normalized value.
    let mut edit_state = OrchestrationEditState::from_run_agents_fields(
        &request.model_id,
        &request.harness_type,
        &request.execution_mode,
    );
    edit_state.override_from_approved_config(config);
    request.model_id = edit_state.model_id;
    request.harness_type = edit_state.harness_type;
    request.execution_mode = edit_state.execution_mode;
}

/// Defence-in-depth validation; mirrors the card view's
/// `accept_disabled_reason` check.
fn validate_request(request: &RunAgentsRequest) -> Result<(), String> {
    if request.agent_run_configs.is_empty() {
        return Err("orchestrate: empty agent_run_configs".to_string());
    }
    if request.agent_run_configs.len() > MAX_LOCAL_CHILD_FANOUT {
        return Err(format!(
            "orchestrate: fan-out exceeds local limit of {MAX_LOCAL_CHILD_FANOUT} agents"
        ));
    }
    if request.model_id.trim().is_empty() {
        return Err("orchestrate: a local model is required".to_string());
    }
    if request.base_prompt.trim().is_empty()
        && request
            .agent_run_configs
            .iter()
            .all(|config| config.prompt.trim().is_empty())
    {
        return Err("orchestrate: at least one non-empty prompt is required".to_string());
    }
    let mut names = std::collections::HashSet::with_capacity(request.agent_run_configs.len());
    for config in &request.agent_run_configs {
        if config.name.trim().is_empty() {
            return Err("orchestrate: child agent names are required".to_string());
        }
        if !names.insert(config.name.trim().to_ascii_lowercase()) {
            return Err(format!(
                "orchestrate: child agent name `{}` is duplicated",
                config.name.trim()
            ));
        }
    }
    if request
        .harness_type
        .trim()
        .eq_ignore_ascii_case("gemini")
    {
        return Err("orchestrate: Gemini is not an available local child harness".to_string());
    }
    Ok(())
}

/// Joins `base_prompt` and a per-agent prompt with `"\n\n"`,
/// falling back to whichever is non-empty.
pub fn compose_run_agents_child_prompt(base_prompt: &str, per_agent_prompt: &str) -> String {
    let base_trimmed = base_prompt.trim();
    let per_agent_trimmed = per_agent_prompt.trim();
    match (base_trimmed.is_empty(), per_agent_trimmed.is_empty()) {
        (false, false) => format!("{base_prompt}\n\n{per_agent_prompt}"),
        (false, true) => base_prompt.to_string(),
        (true, false) => per_agent_prompt.to_string(),
        (true, true) => String::new(),
    }
}

/// Translates run-wide config into a per-child
/// [`StartAgentExecutionMode`].
pub fn run_agents_to_start_agent_mode(
    run_execution_mode: &RunAgentsExecutionMode,
    run_harness_type: &str,
    run_model_id: &str,
    _run_skills: &[SkillReference],
    _cfg: &RunAgentsAgentRunConfig,
) -> Result<StartAgentExecutionMode, String> {
    match run_execution_mode {
        RunAgentsExecutionMode::Local => Ok(StartAgentExecutionMode::local_from_hosted_fields(
            run_harness_type,
            run_model_id,
        )),
        RunAgentsExecutionMode::Remote { .. } => Ok(
            StartAgentExecutionMode::local_from_hosted_fields(run_harness_type, run_model_id),
        ),
    }
}
