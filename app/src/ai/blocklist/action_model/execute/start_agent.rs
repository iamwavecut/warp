use std::collections::HashMap;

use futures::FutureExt;
use futures::future::BoxFuture;
use warp_cli::agent::Harness;
use warp_core::execution_mode::AppExecutionMode;
use warpui::{Entity, ModelContext, SingletonEntity};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};
use crate::ai::agent::conversation::{AIConversationId, ConversationStatus};
use crate::ai::agent::{
    AIAgentAction, AIAgentActionId, AIAgentActionResultType, AIAgentActionType, LifecycleEventType,
    StartAgentExecutionMode, StartAgentResult,
};
use crate::ai::blocklist::{
    BlocklistAIHistoryEvent, BlocklistAIHistoryModel, BlocklistAIPermissions,
};
use crate::ai::local_agent_registry::{
    LocalAgentCancellationHandle, LocalAgentPreflight, LocalAgentRegistry,
};
use crate::ai::local_child_harnesses::local_child_harness_disabled_message;

/// Per-request outcome of a StartAgent dispatch.
#[derive(Debug, Clone)]
pub enum StartAgentOutcome {
    Started {
        agent_id: String,
    },
    /// An error occurred while starting the agent.
    Error(String),
    Cancelled,
}

fn invalid_local_child_harness_error(harness_type: &str) -> String {
    let harness_name = harness_type.trim();
    if harness_name.is_empty() {
        "Local child harness type is missing.".to_string()
    } else {
        format!("Unsupported local child harness '{harness_name}'.")
    }
}

/// Opaque, monotonically increasing request identifier.
/// Disambiguates parallel in-flight StartAgent requests.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Default)]
pub struct StartAgentRequestId(u64);

impl StartAgentRequestId {
    #[cfg(test)]
    pub const fn from_raw_for_test(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone)]
pub struct StartAgentRequest {
    pub id: StartAgentRequestId,
    pub name: String,
    pub prompt: String,
    pub execution_mode: StartAgentExecutionMode,
    pub lifecycle_subscription: Option<Vec<LifecycleEventType>>,
    pub parent_conversation_id: AIConversationId,
    pub parent_run_id: Option<String>,
    pub cancellation: LocalAgentCancellationHandle,
}

pub struct StartAgentDispatch {
    pub request_id: Option<StartAgentRequestId>,
    pub cancellation: LocalAgentCancellationHandle,
    pub receiver: async_channel::Receiver<StartAgentOutcome>,
}

struct PendingStartAgent {
    /// Set once the child conversation is synchronously created.
    child_conversation_id: Option<AIConversationId>,
    action_id: Option<AIAgentActionId>,
    parent_action_id: Option<AIAgentActionId>,
    cancellation: LocalAgentCancellationHandle,
    senders: Vec<async_channel::Sender<StartAgentOutcome>>,
}

pub struct StartAgentExecutor {
    pending: HashMap<StartAgentRequestId, PendingStartAgent>,
    pending_by_action: HashMap<AIAgentActionId, StartAgentRequestId>,
    completed_by_action: HashMap<AIAgentActionId, StartAgentOutcome>,
    next_request_id: u64,
}

impl StartAgentExecutor {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        let history_model = BlocklistAIHistoryModel::handle(ctx);
        ctx.subscribe_to_model(&history_model, |me, _, event, ctx| {
            me.handle_history_event(event, ctx)
        });

        Self {
            pending: HashMap::new(),
            pending_by_action: HashMap::new(),
            completed_by_action: HashMap::new(),
            next_request_id: 0,
        }
    }

    fn next_request_id(&mut self) -> StartAgentRequestId {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        StartAgentRequestId(id)
    }

    /// Links a pending request to its freshly-created child
    /// conversation so subsequent history events can find it.
    fn record_child_conversation(
        &mut self,
        request_id: StartAgentRequestId,
        child_conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(pending) = self.pending.get_mut(&request_id) else {
            return;
        };
        pending.child_conversation_id = Some(child_conversation_id);
        self.maybe_complete_pending_for_child_state(request_id, child_conversation_id, ctx);
    }

    fn find_pending_by_child(
        &self,
        child_conversation_id: &AIConversationId,
    ) -> Option<StartAgentRequestId> {
        self.pending.iter().find_map(|(id, pending)| {
            (pending.child_conversation_id.as_ref() == Some(child_conversation_id)).then_some(*id)
        })
    }

    fn complete_pending_as_started(
        &mut self,
        request_id: StartAgentRequestId,
        child_conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        // Local children are identified by the run ID assigned while their
        // conversation/pane is created.  A server conversation token is not a
        // readiness signal and direct providers do not emit one.
        let agent_id = BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&child_conversation_id)
            .and_then(|conversation| conversation.run_id());
        match agent_id {
            Some(id) => {
                let controller_registered =
                    LocalAgentRegistry::as_ref(ctx).get(&id).is_some_and(|run| {
                        run.conversation_id == child_conversation_id
                            && run.controller_owner.is_some()
                            && run.status.is_live()
                    });
                if !controller_registered {
                    self.complete_pending(
                        request_id,
                        StartAgentOutcome::Error(
                            "Local child controller was not registered for its local run ID"
                                .to_string(),
                        ),
                    );
                    return;
                }
                self.complete_pending(
                    request_id,
                    StartAgentOutcome::Started {
                        agent_id: id.clone(),
                    },
                );
            }
            None => {
                report_error!(
                    "No agent identifier found for child conversation",
                    extra: { "child_conversation_id" => ?child_conversation_id }
                );
                self.complete_pending(
                    request_id,
                    StartAgentOutcome::Error(
                        "Local child did not receive a local run ID".to_string(),
                    ),
                );
            }
        }
    }

    fn complete_pending_as_error(
        &mut self,
        request_id: StartAgentRequestId,
        _child_conversation_id: AIConversationId,
        error_msg: String,
        _ctx: &mut ModelContext<Self>,
    ) {
        self.complete_pending(request_id, StartAgentOutcome::Error(error_msg));
    }

    fn complete_pending(&mut self, request_id: StartAgentRequestId, outcome: StartAgentOutcome) {
        let Some(pending) = self.pending.remove(&request_id) else {
            return;
        };
        if let Some(action_id) = pending.action_id {
            self.pending_by_action.remove(&action_id);
            self.completed_by_action.insert(action_id, outcome.clone());
        }
        for sender in pending.senders {
            let _ = sender.try_send(outcome.clone());
        }
    }

    fn maybe_complete_pending_for_child_state(
        &mut self,
        request_id: StartAgentRequestId,
        child_conversation_id: AIConversationId,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(conversation) =
            BlocklistAIHistoryModel::as_ref(ctx).conversation(&child_conversation_id)
        else {
            return;
        };
        if let Some(error_msg) = start_agent_error_message_for_status(
            conversation.status(),
            conversation.status_error_message().as_deref(),
        ) {
            self.complete_pending_as_error(request_id, child_conversation_id, error_msg, ctx);
            return;
        }
        if conversation.run_id().is_some() {
            self.complete_pending_as_started(request_id, child_conversation_id, ctx);
        }
    }

    fn handle_history_event(
        &mut self,
        event: &BlocklistAIHistoryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        match event {
            BlocklistAIHistoryEvent::UpdatedConversationStatus {
                conversation_id, ..
            } => {
                let Some(request_id) = self.find_pending_by_child(conversation_id) else {
                    return;
                };
                let history = BlocklistAIHistoryModel::as_ref(ctx);
                let Some(conversation) = history.conversation(conversation_id) else {
                    return;
                };
                let error_msg = start_agent_error_message_for_status(
                    conversation.status(),
                    conversation.status_error_message().as_deref(),
                );
                if let Some(error_msg) = error_msg {
                    self.complete_pending_as_error(request_id, *conversation_id, error_msg, ctx);
                }
            }
            BlocklistAIHistoryEvent::NewConversationRequestComplete {
                request_id,
                conversation_id,
            } => {
                self.record_child_conversation(*request_id, *conversation_id, ctx);
            }
            BlocklistAIHistoryEvent::StartedNewConversation { .. }
            | BlocklistAIHistoryEvent::CreatedSubtask { .. }
            | BlocklistAIHistoryEvent::UpgradedTask { .. }
            | BlocklistAIHistoryEvent::AppendedExchange { .. }
            | BlocklistAIHistoryEvent::ReassignedExchange { .. }
            | BlocklistAIHistoryEvent::UpdatedStreamingExchange { .. }
            | BlocklistAIHistoryEvent::SetActiveConversation { .. }
            | BlocklistAIHistoryEvent::ClearedActiveConversation { .. }
            | BlocklistAIHistoryEvent::ClearedConversationsForTerminalSurface { .. }
            | BlocklistAIHistoryEvent::UpdatedTodoList { .. }
            | BlocklistAIHistoryEvent::UpdatedAutoexecuteOverride { .. }
            | BlocklistAIHistoryEvent::SplitConversation { .. }
            | BlocklistAIHistoryEvent::RemoveConversation { .. }
            | BlocklistAIHistoryEvent::DeletedConversation { .. }
            | BlocklistAIHistoryEvent::RestoredConversations { .. }
            | BlocklistAIHistoryEvent::UpdatedConversationMetadata { .. }
            | BlocklistAIHistoryEvent::UpdatedConversationArtifacts { .. }
            | BlocklistAIHistoryEvent::ConversationTransferredBetweenTerminalSurfaces { .. }
            | BlocklistAIHistoryEvent::ConversationServerTokenAssigned { .. } => {}
            BlocklistAIHistoryEvent::OrchestrationConfigUpdated { .. } => {}
        }
    }

    pub(super) fn should_autoexecute(
        &self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> bool {
        if AppExecutionMode::as_ref(ctx).is_autonomous() {
            return true;
        }
        let terminal_surface_id = BlocklistAIHistoryModel::as_ref(ctx)
            .terminal_surface_id_for_conversation(&input.conversation_id);
        BlocklistAIPermissions::as_ref(ctx)
            .get_run_agents_setting(ctx, terminal_surface_id)
            .is_always_allow()
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> AnyActionExecution {
        let AIAgentAction {
            id: action_id,
            action:
                AIAgentActionType::StartAgent {
                    version,
                    name,
                    prompt,
                    execution_mode,
                    lifecycle_subscription,
                },
            ..
        } = input.action
        else {
            return ActionExecution::<()>::InvalidAction.into();
        };

        let prompt = prompt.clone();
        let version = *version;
        if let Some(outcome) = self.completed_by_action.get(action_id).cloned() {
            return ActionExecution::<()>::Sync(start_agent_result(outcome, version)).into();
        }
        if let Some(request_id) = self.pending_by_action.get(action_id).copied() {
            let (sender, receiver) = async_channel::bounded(1);
            if let Some(pending) = self.pending.get_mut(&request_id) {
                pending.senders.push(sender);
                return start_agent_async_execution(receiver, version);
            }
            self.pending_by_action.remove(action_id);
        }
        let parent_conversation_id = input.conversation_id;
        let requested_execution_mode = execution_mode.clone().local_first();
        let (execution_mode, parent_run_id) = match requested_execution_mode {
            StartAgentExecutionMode::Local {
                harness_type: None,
                model_id,
            } => {
                let parent_run_id = BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&parent_conversation_id)
                    .and_then(|conversation| conversation.run_id());
                let Some(parent_run_id) = parent_run_id else {
                    return ActionExecution::<()>::Sync(AIAgentActionResultType::StartAgent(
                        StartAgentResult::Error {
                            error: "Local child agents require the parent local run ID to be available."
                                .to_string(),
                            version,
                        },
                    ))
                    .into();
                };
                (
                    StartAgentExecutionMode::Local {
                        harness_type: None,
                        model_id,
                    },
                    Some(parent_run_id),
                )
            }
            StartAgentExecutionMode::Local {
                harness_type: Some(harness_type),
                model_id,
            } => {
                let Some(harness) = Harness::parse_local_child_harness(&harness_type) else {
                    return ActionExecution::<()>::Sync(AIAgentActionResultType::StartAgent(
                        StartAgentResult::Error {
                            error: invalid_local_child_harness_error(&harness_type),
                            version,
                        },
                    ))
                    .into();
                };
                if let Some(message) = local_child_harness_disabled_message(harness) {
                    return ActionExecution::<()>::Sync(AIAgentActionResultType::StartAgent(
                        StartAgentResult::Error {
                            error: message.to_string(),
                            version,
                        },
                    ))
                    .into();
                }

                let parent_run_id = BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&parent_conversation_id)
                    .and_then(|conversation| conversation.run_id());
                let Some(parent_run_id) = parent_run_id else {
                    return ActionExecution::<()>::Sync(AIAgentActionResultType::StartAgent(
                        StartAgentResult::Error {
                            error:
                                "Local harness child agents require the parent run_id to be available."
                                    .to_string(),
                            version,
                        },
                    ))
                    .into();
                };

                (
                    StartAgentExecutionMode::Local {
                        harness_type: Some(harness.to_string()),
                        model_id,
                    },
                    Some(parent_run_id),
                )
            }
            StartAgentExecutionMode::Remote { .. } => unreachable!(
                "StartAgentExecutionMode::local_first must convert hosted child agents to local"
            ),
        };

        let preflight_harness = match &execution_mode {
            StartAgentExecutionMode::Local {
                harness_type: None, ..
            } => Harness::Oz,
            StartAgentExecutionMode::Local {
                harness_type: Some(harness_type),
                ..
            } => Harness::parse_local_child_harness(harness_type).unwrap_or(Harness::Unknown),
            StartAgentExecutionMode::Remote { .. } => unreachable!(),
        };
        let preflight = LocalAgentPreflight {
            parent_run_id: parent_run_id.clone(),
            name: name.clone(),
            prompt: prompt.clone(),
            harness: preflight_harness,
            model_available: true,
            tools_available: true,
            working_directory: None,
            requested_fanout: 1,
        };
        if let Err(error) = LocalAgentRegistry::as_ref(ctx).preflight(&preflight) {
            return ActionExecution::<()>::Sync(AIAgentActionResultType::StartAgent(
                StartAgentResult::Error {
                    error: error.to_string(),
                    version,
                },
            ))
            .into();
        }

        let (sender, receiver) = async_channel::bounded(1);
        let request_id = self.next_request_id();
        let cancellation = LocalAgentCancellationHandle::default();
        self.pending.insert(
            request_id,
            PendingStartAgent {
                child_conversation_id: None,
                action_id: Some(action_id.clone()),
                parent_action_id: None,
                cancellation: cancellation.clone(),
                senders: vec![sender],
            },
        );
        self.pending_by_action.insert(action_id.clone(), request_id);

        ctx.emit(StartAgentExecutorEvent::CreateAgent(StartAgentRequest {
            id: request_id,
            name: name.clone(),
            prompt,
            execution_mode,
            lifecycle_subscription: lifecycle_subscription.clone(),
            parent_conversation_id,
            parent_run_id,
            cancellation,
        }));

        start_agent_async_execution(receiver, version)
    }

    /// Dispatch a pre-validated StartAgent request. Returns a receiver
    /// for the resulting [`StartAgentOutcome`].
    #[allow(clippy::too_many_arguments)]
    pub fn dispatch(
        &mut self,
        name: String,
        prompt: String,
        execution_mode: StartAgentExecutionMode,
        lifecycle_subscription: Option<Vec<LifecycleEventType>>,
        parent_conversation_id: AIConversationId,
        parent_run_id: Option<String>,
        parent_action_id: Option<AIAgentActionId>,
        ctx: &mut ModelContext<Self>,
    ) -> StartAgentDispatch {
        let (sender, receiver) = async_channel::bounded(1);
        let cancellation = LocalAgentCancellationHandle::default();
        let harness = match &execution_mode {
            StartAgentExecutionMode::Local {
                harness_type: None, ..
            } => Harness::Oz,
            StartAgentExecutionMode::Local {
                harness_type: Some(harness_type),
                ..
            } => Harness::parse_local_child_harness(harness_type).unwrap_or(Harness::Unknown),
            StartAgentExecutionMode::Remote { .. } => Harness::Unknown,
        };
        let preflight = LocalAgentPreflight {
            parent_run_id: parent_run_id.clone(),
            name: name.clone(),
            prompt: prompt.clone(),
            harness,
            model_available: true,
            tools_available: true,
            working_directory: None,
            requested_fanout: 1,
        };
        if let Err(error) = LocalAgentRegistry::as_ref(ctx).preflight(&preflight) {
            let _ = sender.try_send(StartAgentOutcome::Error(error.to_string()));
            return StartAgentDispatch {
                request_id: None,
                cancellation,
                receiver,
            };
        }
        let request_id = self.next_request_id();
        self.pending.insert(
            request_id,
            PendingStartAgent {
                child_conversation_id: None,
                action_id: None,
                parent_action_id,
                cancellation: cancellation.clone(),
                senders: vec![sender],
            },
        );
        ctx.emit(StartAgentExecutorEvent::CreateAgent(StartAgentRequest {
            id: request_id,
            name,
            prompt,
            execution_mode,
            lifecycle_subscription,
            parent_conversation_id,
            parent_run_id,
            cancellation: cancellation.clone(),
        }));
        StartAgentDispatch {
            request_id: Some(request_id),
            cancellation,
            receiver,
        }
    }

    fn cancel_request(&mut self, request_id: StartAgentRequestId, ctx: &mut ModelContext<Self>) {
        let child_conversation_id = self.pending.get(&request_id).and_then(|pending| {
            pending.cancellation.cancel();
            pending.child_conversation_id
        });
        if let Some(conversation_id) = child_conversation_id {
            ctx.emit(StartAgentExecutorEvent::StopAgent(conversation_id));
        }
        self.complete_pending(request_id, StartAgentOutcome::Cancelled);
    }

    pub(super) fn cancel_requests(
        &mut self,
        request_ids: impl IntoIterator<Item = StartAgentRequestId>,
        ctx: &mut ModelContext<Self>,
    ) {
        for request_id in request_ids {
            self.cancel_request(request_id, ctx);
        }
    }

    pub(super) fn cancel_execution(
        &mut self,
        action_id: &AIAgentActionId,
        ctx: &mut ModelContext<Self>,
    ) {
        let request_ids = self
            .pending
            .iter()
            .filter_map(|(request_id, pending)| {
                (pending.action_id.as_ref() == Some(action_id)
                    || pending.parent_action_id.as_ref() == Some(action_id))
                .then_some(*request_id)
            })
            .collect::<Vec<_>>();
        self.cancel_requests(request_ids, ctx);
    }

    pub(super) fn preprocess_action(
        &mut self,
        _action: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }
}

fn start_agent_result(
    outcome: StartAgentOutcome,
    version: ai::agent::action_result::StartAgentVersion,
) -> AIAgentActionResultType {
    match outcome {
        StartAgentOutcome::Started { agent_id } => {
            AIAgentActionResultType::StartAgent(StartAgentResult::Success { agent_id, version })
        }
        StartAgentOutcome::Error(error) => {
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
        }
        StartAgentOutcome::Cancelled => {
            AIAgentActionResultType::StartAgent(StartAgentResult::Cancelled { version })
        }
    }
}

fn start_agent_async_execution(
    receiver: async_channel::Receiver<StartAgentOutcome>,
    version: ai::agent::action_result::StartAgentVersion,
) -> AnyActionExecution {
    ActionExecution::new_async(async move { receiver.recv().await }, move |result, _ctx| {
        result.map_or_else(
            |_| AIAgentActionResultType::StartAgent(StartAgentResult::Cancelled { version }),
            |outcome| start_agent_result(outcome, version),
        )
    })
    .into()
}

fn start_agent_error_message_for_status(
    status: &ConversationStatus,
    error_message: Option<&str>,
) -> Option<String> {
    match status {
        ConversationStatus::Error => Some(
            error_message
                .filter(|message| !message.trim().is_empty())
                .unwrap_or("Child agent failed to initialize")
                .to_string(),
        ),
        ConversationStatus::Cancelled => {
            Some("Child agent was cancelled before initialization".to_string())
        }
        ConversationStatus::Blocked { blocked_action } => {
            let blocked_action = blocked_action.trim();
            Some(if blocked_action.is_empty() {
                "Child agent startup was blocked before initialization".to_string()
            } else {
                blocked_action.to_string()
            })
        }
        ConversationStatus::InProgress | ConversationStatus::Success => None,
    }
}

impl Entity for StartAgentExecutor {
    type Event = StartAgentExecutorEvent;
}

pub enum StartAgentExecutorEvent {
    CreateAgent(StartAgentRequest),
    StopAgent(AIConversationId),
}

#[cfg(test)]
#[path = "start_agent_tests.rs"]
mod tests;
