//! Local executor for `AIAgentActionType::WaitForEvents`.
//!
//! Waits on the process-local agent registry and uses a watchdog so an
//! unavailable child cannot leave its parent suspended forever.

use std::collections::HashMap;
use std::time::Duration;

use futures::FutureExt;
use futures::future::BoxFuture;
use warpui::r#async::SpawnedFutureHandle;
use warpui::{Entity, EntityId, ModelContext, SingletonEntity};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};
use crate::ai::agent::conversation::{AIConversationId, ConversationStatus};
use crate::ai::agent::{AIAgentActionResultType, AIAgentActionType, WaitForEventsResult};
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::ai::local_agent_registry::{LocalAgentRegistry, LocalAgentRegistryEvent};

pub(crate) const DEFAULT_IDLE_TIMEOUT_SECONDS: i32 = 30 * 60;
pub(crate) const WATCHDOG_SAFETY_MARGIN: Duration = Duration::from_secs(30);
pub(crate) const WATCHDOG_FLOOR: Duration = Duration::from_secs(5);

pub(crate) fn watchdog_timeout(stamped_seconds: i32) -> Duration {
    let seconds = if stamped_seconds <= 0 {
        DEFAULT_IDLE_TIMEOUT_SECONDS
    } else {
        stamped_seconds
    };
    Duration::from_secs(seconds as u64)
        .checked_sub(WATCHDOG_SAFETY_MARGIN)
        .filter(|duration| *duration >= WATCHDOG_FLOOR)
        .unwrap_or(WATCHDOG_FLOOR)
}

struct PendingWait {
    tool_call_id: String,
    sender: async_channel::Sender<WaitForEventsResult>,
    watchdog_handle: SpawnedFutureHandle,
}

pub struct WaitForEventsExecutor {
    terminal_view_id: EntityId,
    conversation_generation: HashMap<AIConversationId, usize>,
    pending: HashMap<AIConversationId, PendingWait>,
}

impl WaitForEventsExecutor {
    pub fn new(terminal_view_id: EntityId, ctx: &mut ModelContext<Self>) -> Self {
        let registry = LocalAgentRegistry::handle(ctx);
        ctx.subscribe_to_model(&registry, |executor, _, event, ctx| {
            executor.handle_registry_event(event, ctx);
        });
        Self {
            terminal_view_id,
            conversation_generation: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    pub(super) fn should_autoexecute(
        &self,
        _input: ExecuteActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> bool {
        true
    }

    pub(super) fn preprocess_action(
        &mut self,
        _action: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> impl Into<AnyActionExecution> + use<> {
        let AIAgentActionType::WaitForEvents {
            tool_call_id,
            idle_timeout_seconds,
        } = &input.action.action
        else {
            return ActionExecution::InvalidAction;
        };

        let tool_call_id = tool_call_id.clone();
        let conversation_id = input.conversation_id;
        let timeout = watchdog_timeout(*idle_timeout_seconds);
        let generation = self
            .conversation_generation
            .entry(conversation_id)
            .or_insert(0);
        *generation += 1;
        let expected_generation = *generation;

        let watchdog_tool_call_id = tool_call_id.clone();
        let watchdog_handle = ctx.spawn(
            async move {
                warpui::r#async::Timer::after(timeout).await;
            },
            move |executor, (), ctx| {
                executor.fire_watchdog_if_current(
                    conversation_id,
                    &watchdog_tool_call_id,
                    expected_generation,
                    ctx,
                );
            },
        );

        let (sender, receiver) = async_channel::bounded(1);
        if let Some(previous) = self.pending.insert(
            conversation_id,
            PendingWait {
                tool_call_id: tool_call_id.clone(),
                sender,
                watchdog_handle,
            },
        ) {
            previous.watchdog_handle.abort();
            drop(previous.sender);
        }

        let terminal_view_id = self.terminal_view_id;
        BlocklistAIHistoryModel::handle(ctx).update(ctx, move |history, ctx| {
            history.update_conversation_status(
                terminal_view_id,
                conversation_id,
                ConversationStatus::WaitingForEvents,
                ctx,
            );
        });

        ActionExecution::new_async(async move { receiver.recv().await }, move |result, _ctx| {
            AIAgentActionResultType::WaitForEvents(result.unwrap_or(WaitForEventsResult::Completed))
        })
    }

    pub(crate) fn cancel_execution(&mut self, tool_call_id: &str) {
        let Some(conversation_id) = self.pending.iter().find_map(|(conversation_id, pending)| {
            (pending.tool_call_id == tool_call_id).then_some(*conversation_id)
        }) else {
            return;
        };
        if let Some(generation) = self.conversation_generation.get_mut(&conversation_id) {
            *generation += 1;
        }
        if let Some(pending) = self.pending.remove(&conversation_id) {
            pending.watchdog_handle.abort();
            drop(pending.sender);
        }
    }

    fn handle_registry_event(
        &mut self,
        event: &LocalAgentRegistryEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let pending_conversations = self.pending.keys().copied().collect::<Vec<_>>();
        let conversations_to_wake = {
            let registry = LocalAgentRegistry::as_ref(ctx);
            pending_conversations
                .into_iter()
                .filter(|conversation_id| {
                    let Some(parent_run_id) = registry.run_id_for_conversation(*conversation_id)
                    else {
                        return false;
                    };
                    match event {
                        LocalAgentRegistryEvent::MessageAccepted { recipient_run_id } => {
                            recipient_run_id == parent_run_id
                        }
                        LocalAgentRegistryEvent::StatusChanged { run_id, .. } => {
                            registry
                                .get(run_id)
                                .and_then(|run| run.parent_run_id.as_deref())
                                == Some(parent_run_id)
                        }
                    }
                })
                .collect::<Vec<_>>()
        };
        for conversation_id in conversations_to_wake {
            self.complete_wait(conversation_id, "local agent registry event");
        }
    }

    fn fire_watchdog_if_current(
        &mut self,
        conversation_id: AIConversationId,
        tool_call_id: &str,
        expected_generation: usize,
        ctx: &mut ModelContext<Self>,
    ) {
        if self
            .conversation_generation
            .get(&conversation_id)
            .copied()
            .unwrap_or_default()
            != expected_generation
        {
            return;
        }
        let still_waiting = BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .is_some_and(|conversation| {
                matches!(conversation.status(), ConversationStatus::WaitingForEvents)
            });
        if !still_waiting {
            self.pending.remove(&conversation_id);
            return;
        }
        if self
            .pending
            .get(&conversation_id)
            .is_some_and(|pending| pending.tool_call_id == tool_call_id)
        {
            self.complete_wait(conversation_id, "local idle watchdog");
        }
    }

    fn complete_wait(&mut self, conversation_id: AIConversationId, reason: &str) {
        let Some(pending) = self.pending.remove(&conversation_id) else {
            return;
        };
        pending.watchdog_handle.abort();
        log::info!(
            "Completing local wait_for_events conversation_id={conversation_id:?}: {reason}"
        );
        let _ = pending.sender.try_send(WaitForEventsResult::Completed);
    }
}

impl Entity for WaitForEventsExecutor {
    type Event = ();
}

#[cfg(test)]
#[path = "wait_for_events_tests.rs"]
mod tests;
