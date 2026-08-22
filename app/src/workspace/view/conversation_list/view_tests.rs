use std::collections::HashMap;

use warp_core::features::FeatureFlag;
use warp_multi_agent_api as api;
use warpui::{App, EntityId, SingletonEntity};

use super::*;
use crate::ai::active_agent_views_model::ActiveAgentViewsModel;
use crate::ai::agent::conversation::AIConversation;
use crate::ai::agent_conversations_model::AgentConversationsModel;
use crate::auth::AuthStateProvider;
use crate::test_util::settings::initialize_history_persistence_for_tests;
use crate::workspace::WorkspaceRegistry;
use crate::workspace::view::conversation_list::view_model::ConversationListViewModel;

fn conversation_for_filtered_rename(
    conversation_id: AIConversationId,
    title: &str,
) -> AIConversation {
    AIConversation::new_restored(
        conversation_id,
        vec![api::Task {
            id: "filtered-rename-root".to_owned(),
            messages: vec![api::Message {
                id: "filtered-rename-message".to_owned(),
                task_id: "filtered-rename-root".to_owned(),
                server_message_data: String::new(),
                citations: vec![],
                message: Some(api::message::Message::UserQuery(api::message::UserQuery {
                    query: "Original prompt".to_owned(),
                    context: None,
                    referenced_attachments: HashMap::new(),
                    mode: None,
                    intended_agent: Default::default(),
                })),
                request_id: "filtered-rename-request".to_owned(),
                timestamp: None,
            }],
            dependencies: None,
            description: title.to_owned(),
            summary: String::new(),
            server_data: String::new(),
        }],
        None,
    )
    .expect("filtered rename fixture should restore")
}

#[test]
fn conversation_list_inline_rename_state_starts_finishes_and_cancels() {
    let conversation_id = AIConversationId::new();
    let mut state = InlineConversationRenameState::default();

    assert!(!state.is_renaming(conversation_id));
    state.start(conversation_id);
    assert!(state.is_renaming(conversation_id));
    assert_eq!(state.finish(), Some(conversation_id));
    assert!(!state.is_renaming(conversation_id));

    state.start(conversation_id);
    state.cancel();
    assert!(!state.is_renaming(conversation_id));
    assert_eq!(state.finish(), None);
}

#[test]
fn filtered_conversation_list_reapplies_title_search_after_local_rename() {
    App::test((), |mut app| async move {
        let _agent_management_guard = FeatureFlag::AgentManagementView.override_enabled(true);
        let _interactive_management_guard =
            FeatureFlag::InteractiveConversationManagementView.override_enabled(true);
        initialize_history_persistence_for_tests(&mut app);
        app.add_singleton_model(|_| AuthStateProvider::new_for_test());
        app.add_singleton_model(|_| WorkspaceRegistry::new());
        app.add_singleton_model(|_| ActiveAgentViewsModel::new());
        app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());

        let conversation_id = AIConversationId::new();
        BlocklistAIHistoryModel::handle(&app).update(&mut app, |history, ctx| {
            history.restore_conversations(
                EntityId::new(),
                vec![conversation_for_filtered_rename(
                    conversation_id,
                    "Matching title",
                )],
                ctx,
            );
        });
        app.add_singleton_model(AgentConversationsModel::new);
        let list_model = app.add_model(ConversationListViewModel::new);

        list_model.update(&mut app, |model, ctx| {
            model.set_search_query("Matching".to_owned(), ctx);
        });
        list_model.read(&app, |model, _| {
            assert_eq!(model.unfiltered_item_count(), 1);
            assert_eq!(model.filtered_items().len(), 1);
            assert_eq!(
                model.filtered_items()[0].highlight_indices,
                vec![0, 1, 2, 3, 4, 5, 6, 7],
            );
        });

        BlocklistAIHistoryModel::handle(&app).update(&mut app, |history, ctx| {
            history
                .rename_conversation_locally(conversation_id, "Different title".to_owned(), ctx)
                .expect("local rename should succeed");
        });

        list_model.read(&app, |model, _| {
            assert_eq!(model.unfiltered_item_count(), 1);
            assert!(model.filtered_items().is_empty());
        });
    });
}
