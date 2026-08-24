use futures::{FutureExt, future::BoxFuture};
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::ai::agent::{
    AIAgentAction, AIAgentActionResultType, AIAgentActionType, SendMessageToAgentResult,
};
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::ai::local_agent_registry::{LocalAgentRegistry, LocalAgentRegistryEvent};

use super::{ActionExecution, AnyActionExecution, ExecuteActionInput, PreprocessActionInput};

pub struct SendMessageToAgentExecutor {
    ambient_agent_task_id: Option<AmbientAgentTaskId>,
}

impl SendMessageToAgentExecutor {
    pub fn new() -> Self {
        Self {
            ambient_agent_task_id: None,
        }
    }

    pub fn set_ambient_agent_task_id(&mut self, id: Option<AmbientAgentTaskId>) {
        self.ambient_agent_task_id = id;
    }

    pub(super) fn should_autoexecute(
        &self,
        _input: ExecuteActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> bool {
        true
    }

    pub(super) fn execute(
        &mut self,
        input: ExecuteActionInput,
        ctx: &mut ModelContext<Self>,
    ) -> AnyActionExecution {
        let AIAgentAction {
            action:
                AIAgentActionType::SendMessageToAgent {
                    addresses,
                    subject,
                    message,
                },
            ..
        } = input.action
        else {
            return ActionExecution::<()>::InvalidAction.into();
        };

        let conversation_id = input.conversation_id;
        let addresses = addresses.clone();
        let subject = subject.clone();
        let message_body = message.clone();

        let sender_run_id = BlocklistAIHistoryModel::as_ref(ctx)
            .conversation(&conversation_id)
            .and_then(|conversation| conversation.run_id())
            .or_else(|| {
                LocalAgentRegistry::as_ref(ctx)
                    .run_id_for_conversation(conversation_id)
                    .map(ToString::to_string)
            });
        let result = match sender_run_id {
            None => SendMessageToAgentResult::Error(
                "Local sender has no local run ID; message was not sent".to_string(),
            ),
            Some(_sender_run_id) if addresses.is_empty() => SendMessageToAgentResult::Error(
                "No local recipient run IDs were provided".to_string(),
            ),
            Some(sender_run_id) => {
                let mut unique_addresses = std::collections::HashSet::new();
                if addresses
                    .iter()
                    .any(|address| address.trim().is_empty() || !unique_addresses.insert(address))
                {
                    return ActionExecution::<()>::Sync(
                        AIAgentActionResultType::SendMessageToAgent(
                            SendMessageToAgentResult::Error(
                                "Local recipient run IDs must be non-empty and unique".to_string(),
                            ),
                        ),
                    )
                    .into();
                }
                let delivery = LocalAgentRegistry::handle(ctx).update(ctx, |registry, ctx| {
                    let mut first_message_id = None;
                    for address in &addresses {
                        match registry.send_message(
                            &sender_run_id,
                            address,
                            subject.clone(),
                            message_body.clone(),
                        ) {
                            Ok(ack) => {
                                first_message_id.get_or_insert(ack.message_id);
                                ctx.emit(LocalAgentRegistryEvent::MessageAccepted {
                                    recipient_run_id: address.clone(),
                                });
                            }
                            Err(error) => {
                                return Err((first_message_id, error.to_string()));
                            }
                        }
                    }
                    Ok(first_message_id)
                });
                match delivery {
                    Ok(Some(message_id)) => SendMessageToAgentResult::Success { message_id },
                    Ok(None) => SendMessageToAgentResult::Error(
                        "No local recipient run IDs were provided".to_string(),
                    ),
                    Err((Some(message_id), error)) => SendMessageToAgentResult::Error(format!(
                        "Local message {message_id} was partially delivered: {error}"
                    )),
                    Err((None, error)) => SendMessageToAgentResult::Error(error),
                }
            }
        };

        ActionExecution::<()>::Sync(AIAgentActionResultType::SendMessageToAgent(result)).into()
    }

    pub(super) fn preprocess_action(
        &mut self,
        _action: PreprocessActionInput,
        _ctx: &mut ModelContext<Self>,
    ) -> BoxFuture<'static, ()> {
        futures::future::ready(()).boxed()
    }
}

impl Default for SendMessageToAgentExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Entity for SendMessageToAgentExecutor {
    type Event = ();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::agent::conversation::AIConversationId;
    use crate::ai::agent::task::TaskId;
    use crate::ai::agent::{AIAgentActionId, AIAgentActionType};
    use crate::ai::local_agent_registry::{LocalAgentStatus, local_controller_owner_id};
    use crate::test_util::settings::initialize_history_persistence_for_tests;
    use warp_cli::agent::Harness;
    use warpui::{App, EntityId};

    fn register_run(
        registry: &mut LocalAgentRegistry,
        conversation_id: AIConversationId,
        terminal_surface_id: EntityId,
        run_id: String,
        name: &str,
    ) {
        registry
            .register_existing(
                run_id,
                conversation_id,
                Some(terminal_surface_id),
                None,
                None,
                name.to_string(),
                Harness::Oz,
                Some(local_controller_owner_id(terminal_surface_id)),
                LocalAgentStatus::Idle,
            )
            .expect("register local run");
    }

    #[test]
    fn local_send_message_to_agent_queues_by_local_run_id_without_server_token() {
        App::test((), |mut app| async move {
            initialize_history_persistence_for_tests(&mut app);
            let history = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
            let registry = app.add_singleton_model(|_| LocalAgentRegistry::new());
            let sender_terminal = EntityId::new();
            let recipient_terminal = EntityId::new();
            let sender_conversation = history.update(&mut app, |history, ctx| {
                history.start_new_conversation(sender_terminal, false, false, false, ctx)
            });
            let recipient_conversation = history.update(&mut app, |history, ctx| {
                history.start_new_conversation(recipient_terminal, false, false, false, ctx)
            });
            let sender_run_id = history
                .update(&mut app, |history, ctx| {
                    history.ensure_local_run_id_for_conversation(sender_conversation, ctx)
                })
                .expect("sender run id");
            let recipient_run_id = history
                .update(&mut app, |history, ctx| {
                    history.ensure_local_run_id_for_conversation(recipient_conversation, ctx)
                })
                .expect("recipient run id");
            registry.update(&mut app, |registry, _ctx| {
                register_run(
                    registry,
                    sender_conversation,
                    sender_terminal,
                    sender_run_id,
                    "sender",
                );
                register_run(
                    registry,
                    recipient_conversation,
                    recipient_terminal,
                    recipient_run_id.clone(),
                    "recipient",
                );
            });

            let action = AIAgentAction {
                id: AIAgentActionId::from("local-send-message".to_string()),
                action: AIAgentActionType::SendMessageToAgent {
                    addresses: vec![recipient_run_id.clone()],
                    subject: "status".to_string(),
                    message: "report when ready".to_string(),
                },
                task_id: TaskId::new("local-send-task".to_string()),
                requires_result: true,
            };
            let executor = app.add_model(|_| SendMessageToAgentExecutor::new());
            let result = executor.update(&mut app, |executor, ctx| {
                executor.execute(
                    ExecuteActionInput {
                        action: &action,
                        conversation_id: sender_conversation,
                    },
                    ctx,
                )
            });

            assert!(matches!(
                result,
                AnyActionExecution::Sync(AIAgentActionResultType::SendMessageToAgent(
                    SendMessageToAgentResult::Success { .. }
                ))
            ));
            registry.read(&app, |registry, _| {
                assert_eq!(registry.pending_message_count(&recipient_run_id), 1);
            });
        });
    }
}
