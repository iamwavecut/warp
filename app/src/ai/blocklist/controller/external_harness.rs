// Plumbing for harnesses that run outside Warp's agent loop but render through a native
// conversation: the harness translates its protocol into response events for the local
// controller. This client owns history and task status; the external harness owns the turn.
use crate::ai::agent::AIAgentAttachment;
use uuid::Uuid;
use warp_multi_agent_api::response_event::StreamInit;
use warp_multi_agent_api::{ResponseEvent, response_event};
use warpui::{ModelContext, SingletonEntity};

use super::response_stream::ResponseStreamId;
use super::{BlocklistAIController, RequestInput};
use crate::ai::agent::conversation::{AIConversationId, ConversationDriver, ConversationStatus};
use crate::ai::blocklist::history_model::{BlocklistAIHistoryModel, UpdateHistoryError};

/// Identifiers for one harness-driven turn.
#[derive(Clone, Debug)]
pub(in crate::ai) struct ExternalHarnessTurn {
    pub stream_id: ResponseStreamId,
    pub request_id: String,
}

/// A prompt relayed to the external harness driving a native conversation.
#[derive(Clone, Debug)]
pub(in crate::ai) struct ExternalHarnessPrompt {
    pub text: String,
    /// Context the prompt's author attached, kept in its existing attachment shape so the
    /// harness can decide how to present each kind to its agent.
    pub attachments: Vec<AIAgentAttachment>,
}

impl BlocklistAIController {
    pub(in crate::ai) fn cancel_external_harness_turn(
        &mut self,
        stream_id: &ResponseStreamId,
        reason: crate::ai::agent::CancellationReason,
        ctx: &mut ModelContext<Self>,
    ) {
        let history = BlocklistAIHistoryModel::handle(ctx);
        let Some(id) = history
            .as_ref(ctx)
            .conversation_for_response_stream(stream_id)
        else {
            return;
        };
        let terminal = self.terminal_surface_id;
        history.update(ctx, |history, ctx| {
            history.mark_response_stream_cancelled(stream_id, id, terminal, reason, ctx);
        });
    }

    pub(in crate::ai) fn set_external_harness_cancel_sink(
        &mut self,
        sink: async_channel::Sender<()>,
    ) {
        self.external_harness_cancel_sink = Some(sink);
    }
    pub(in crate::ai) fn mark_external_harness_cancelled(&self, ctx: &mut ModelContext<Self>) {
        if let Some(id) = self.native_prompt_conversation_id {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.update_conversation_status(
                    self.terminal_surface_id,
                    id,
                    ConversationStatus::Cancelled,
                    ctx,
                );
            });
        }
    }
    pub(crate) fn native_prompt_conversation_id(&self) -> Option<AIConversationId> {
        self.native_prompt_conversation_id
    }

    pub(crate) fn bind_native_prompt_conversation(
        &mut self,
        restored: Option<AIConversationId>,
        ctx: &mut ModelContext<Self>,
    ) -> AIConversationId {
        let id = restored.unwrap_or_else(|| self.start_new_conversation_for_request(ctx).id());
        BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, _| {
            if let Some(conversation) = history.conversation_mut(&id) {
                conversation.set_driver(ConversationDriver::ExternalHarness);
            }
        });
        self.native_prompt_conversation_id = Some(id);
        id
    }
    /// Diverts prompts injected into the bound native conversation to an external harness.
    pub(in crate::ai) fn set_external_harness_prompt_sink(
        &mut self,
        sink: async_channel::Sender<ExternalHarnessPrompt>,
    ) {
        self.external_harness_prompt_sink = Some(sink);
    }

    /// Opens a harness-driven turn on `conversation_id`: registers an exchange with no inputs
    /// for a fresh response stream, marks the conversation in progress, and applies the locally
    /// minted `StreamInit`. The harness must add the user query and task messages through
    /// [`Self::apply_external_harness_event`].
    pub(in crate::ai) fn begin_external_harness_turn(
        &mut self,
        conversation_id: AIConversationId,
        run_id: Option<String>,
        ctx: &mut ModelContext<Self>,
    ) -> Result<ExternalHarnessTurn, UpdateHistoryError> {
        let history = BlocklistAIHistoryModel::handle(ctx);
        let (conversation_token, root_task_id) = {
            let conversation = history
                .as_ref(ctx)
                .conversation(&conversation_id)
                .ok_or(UpdateHistoryError::ConversationNotFound(conversation_id))?;
            (
                conversation
                    .server_conversation_token()
                    .map(|token| token.as_str().to_string())
                    .unwrap_or_else(|| Uuid::new_v4().to_string()),
                conversation.get_root_task_id().clone(),
            )
        };
        let request_id = Uuid::new_v4().to_string();
        let init = StreamInit {
            conversation_id: conversation_token,
            request_id: request_id.clone(),
            run_id: run_id.unwrap_or_default(),
        };
        let stream_id = ResponseStreamId::for_external_harness(&init);

        let request_input = RequestInput::for_task(
            vec![],
            root_task_id,
            &self.active_session,
            self.get_current_response_initiator(),
            conversation_id,
            self.terminal_surface_id,
            ctx,
        );
        let terminal_surface_id = self.terminal_surface_id;
        history.update(ctx, |history, ctx| {
            history
                .conversation_mut(&conversation_id)
                .unwrap()
                .set_driver(ConversationDriver::ExternalHarness);
            history.update_conversation_for_new_request_input(
                request_input,
                stream_id.clone(),
                terminal_surface_id,
                ctx,
            )?;
            history.update_conversation_status(
                terminal_surface_id,
                conversation_id,
                ConversationStatus::InProgress,
                ctx,
            );
            history.set_active_conversation_id(conversation_id, terminal_surface_id, ctx);
            Ok::<_, UpdateHistoryError>(())
        })?;

        self.apply_response_event(
            &stream_id,
            conversation_id,
            ResponseEvent {
                r#type: Some(response_event::Type::Init(init)),
            },
            None,
            ctx,
        );

        Ok(ExternalHarnessTurn {
            stream_id,
            request_id,
        })
    }

    /// Applies a harness-authored response event to the turn open on `stream_id`.
    pub(in crate::ai) fn apply_external_harness_event(
        &mut self,
        stream_id: &ResponseStreamId,
        event: ResponseEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(conversation_id) =
            BlocklistAIHistoryModel::as_ref(ctx).conversation_for_response_stream(stream_id)
        else {
            log::warn!("No conversation for external harness response stream {stream_id:?}");
            return;
        };
        self.apply_response_event(stream_id, conversation_id, event, None, ctx);
    }
}

#[cfg(test)]
#[path = "external_harness_tests.rs"]
mod tests;
