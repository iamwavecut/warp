use std::collections::HashMap;

use super::{SlashCommandTrigger, commands};
use crate::ai::agent::conversation::{AIConversation, AIConversationId};
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::ai::blocklist::agent_view::AgentViewEntryOrigin;
use crate::terminal::input::tests::{add_window_with_bootstrapped_terminal, initialize_app};
use warpui::{App, SingletonEntity};

fn restored_local_conversation_for_rename(conversation_id: AIConversationId) -> AIConversation {
    AIConversation::new_restored(
        conversation_id,
        vec![warp_multi_agent_api::Task {
            id: "rename-root".to_owned(),
            messages: vec![warp_multi_agent_api::Message {
                fetched_memories: vec![],
                id: "rename-message".to_owned(),
                task_id: "rename-root".to_owned(),
                server_message_data: String::new(),
                citations: vec![],
                message: Some(warp_multi_agent_api::message::Message::UserQuery(
                    warp_multi_agent_api::message::UserQuery {
                        query: "Original prompt".to_owned(),
                        context: None,
                        referenced_attachments: HashMap::new(),
                        mode: None,
                        intended_agent: Default::default(),
                    },
                )),
                request_id: "rename-request".to_owned(),
                timestamp: None,
            }],
            dependencies: None,
            description: "Original title".to_owned(),
            summary: String::new(),
            server_data: String::new(),
        }],
        None,
    )
    .expect("source-backed rename conversation should restore")
}

#[test]
fn rename_conversation_slash_command_routes_to_local_history() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let terminal = add_window_with_bootstrapped_terminal(&mut app, None, None).await;
        let input = terminal.read(&app, |terminal, _| terminal.input().clone());
        let conversation_id = AIConversationId::new();

        input.update(&mut app, |input, ctx| {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.restore_conversations(
                    input.terminal_view_id,
                    vec![restored_local_conversation_for_rename(conversation_id)],
                    ctx,
                );
            });
            input.ai_context_model.update(ctx, |context_model, ctx| {
                context_model.set_pending_query_state_for_existing_conversation(
                    conversation_id,
                    AgentViewEntryOrigin::AgentViewBlock,
                    ctx,
                );
            });

            let title = "  Local slash title  ".to_owned();
            assert!(input.execute_slash_command(
                &commands::RENAME_CONVERSATION,
                Some(&title),
                SlashCommandTrigger::input(),
                false,
                ctx,
            ));
        });

        let title = BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            history
                .conversation(&conversation_id)
                .and_then(|conversation| conversation.title())
        });
        assert_eq!(title.as_deref(), Some("Local slash title"));
    });
}

#[test]
fn rename_conversation_slash_command_rejects_missing_and_invalid_titles() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let terminal = add_window_with_bootstrapped_terminal(&mut app, None, None).await;
        let input = terminal.read(&app, |terminal, _| terminal.input().clone());
        let conversation_id = AIConversationId::new();

        input.update(&mut app, |input, ctx| {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.restore_conversations(
                    input.terminal_view_id,
                    vec![restored_local_conversation_for_rename(conversation_id)],
                    ctx,
                );
            });
            input.ai_context_model.update(ctx, |context_model, ctx| {
                context_model.set_pending_query_state_for_existing_conversation(
                    conversation_id,
                    AgentViewEntryOrigin::AgentViewBlock,
                    ctx,
                );
            });

            assert!(input.execute_slash_command(
                &commands::RENAME_CONVERSATION,
                None,
                SlashCommandTrigger::input(),
                false,
                ctx,
            ));
            let too_long = "🦀".repeat(501);
            assert!(input.execute_slash_command(
                &commands::RENAME_CONVERSATION,
                Some(&too_long),
                SlashCommandTrigger::input(),
                false,
                ctx,
            ));
        });

        let title = BlocklistAIHistoryModel::handle(&app).read(&app, |history, _| {
            history
                .conversation(&conversation_id)
                .and_then(|conversation| conversation.title())
        });
        assert_eq!(title.as_deref(), Some("Original title"));
    });
}

#[test]
fn rename_conversation_slash_command_without_active_conversation_is_handled_locally() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let terminal = add_window_with_bootstrapped_terminal(&mut app, None, None).await;
        let input = terminal.read(&app, |terminal, _| terminal.input().clone());
        let title = "Local title".to_owned();

        input.update(&mut app, |input, ctx| {
            assert!(input.execute_slash_command(
                &commands::RENAME_CONVERSATION,
                Some(&title),
                SlashCommandTrigger::input(),
                false,
                ctx,
            ));
        });
    });
}
