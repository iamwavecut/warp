use futures::{FutureExt, future::BoxFuture};
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::ai::agent::{
    AIAgentAction, AIAgentActionResultType, AIAgentActionType, SendMessageToAgentResult,
};
use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::ai::local_agent_registry::LocalAgentRegistry;

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
                let delivery = LocalAgentRegistry::handle(ctx).update(ctx, |registry, _ctx| {
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
