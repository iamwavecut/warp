use session_sharing_protocol::common::{
    SelectedConversation, ServerConversationToken, UniversalDeveloperInputContextUpdate,
};
use warp_core::features::FeatureFlag;
use warpui::{AppContext, ModelHandle, SingletonEntity};

use crate::ai::blocklist::agent_view::AgentViewController;
use crate::ai::blocklist::{BlocklistAIContextModel, BlocklistAIHistoryModel};

/// Build a selected_conversation update based on the current view state.
/// Routes to the appropriate implementation based on whether AgentView is enabled.
/// Returns None if the update should not be sent (e.g., selected conversation has no server token yet).
pub(crate) fn build_selected_conversation_update(
    agent_view_controller: &ModelHandle<AgentViewController>,
    context_model: &ModelHandle<BlocklistAIContextModel>,
    ctx: &mut AppContext,
) -> Option<UniversalDeveloperInputContextUpdate> {
    if FeatureFlag::AgentView.is_enabled() {
        build_selected_conversation_update_agent_view_enabled(
            agent_view_controller,
            &BlocklistAIHistoryModel::handle(ctx),
            ctx,
        )
    } else {
        build_selected_conversation_update_agent_view_disabled(
            context_model,
            &BlocklistAIHistoryModel::handle(ctx),
            ctx,
        )
    }
}

fn build_selected_conversation_update_agent_view_disabled(
    ai_context_model: &ModelHandle<BlocklistAIContextModel>,
    history_model: &ModelHandle<BlocklistAIHistoryModel>,
    ctx: &mut AppContext,
) -> Option<UniversalDeveloperInputContextUpdate> {
    let selected_conversation_id = ai_context_model.as_ref(ctx).selected_conversation_id(ctx);
    let server_token_opt: Option<ServerConversationToken> =
        selected_conversation_id.and_then(|conversation_id| {
            history_model
                .as_ref(ctx)
                .conversation(&conversation_id)
                .and_then(|conversation| conversation.server_conversation_token().cloned())
                .and_then(|token| token.try_into().ok())
        });

    // Only send update if starting new (None) or token is present
    let should_send = selected_conversation_id.is_none() || server_token_opt.is_some();
    if !should_send {
        return None;
    }

    Some(UniversalDeveloperInputContextUpdate {
        selected_conversation: Some(SelectedConversation::new(server_token_opt)),
        ..Default::default()
    })
}

fn build_selected_conversation_update_agent_view_enabled(
    agent_view_controller: &ModelHandle<AgentViewController>,
    history_model: &ModelHandle<BlocklistAIHistoryModel>,
    ctx: &mut AppContext,
) -> Option<UniversalDeveloperInputContextUpdate> {
    let agent_view_state = agent_view_controller.as_ref(ctx).agent_view_state();

    let selected_conversation = if !agent_view_state.is_active() {
        SelectedConversation::NoConversation
    } else if let Some(conversation_id) = agent_view_state.active_conversation_id() {
        let conversation = history_model.as_ref(ctx).conversation(&conversation_id);
        let server_token_opt = conversation
            .and_then(|c| c.server_conversation_token().cloned())
            .and_then(|token| token.try_into().ok());

        if let Some(server_token) = server_token_opt {
            SelectedConversation::ExistingConversation(server_token)
        } else {
            // If the conversation has content but no token yet, skip this update. Otherwise we'd send
            // NewConversation now and ExistingConversation moments later when the token
            // arrives, causing the second update to sometimes be overwritten by an echo of the first update
            // (and leading to a weird state where the viewer sends a query and is then briefly entered into an empty agent view).
            let is_empty = conversation.is_none_or(|c| c.exchange_count() == 0);
            if is_empty {
                SelectedConversation::NewConversation
            } else {
                return None;
            }
        }
    } else {
        SelectedConversation::NewConversation
    };

    Some(UniversalDeveloperInputContextUpdate {
        selected_conversation: Some(selected_conversation),
        ..Default::default()
    })
}

#[derive(Clone)]
pub(crate) struct RemoteUpdateGuard;

impl RemoteUpdateGuard {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn should_broadcast(&self) -> bool {
        true
    }
}
