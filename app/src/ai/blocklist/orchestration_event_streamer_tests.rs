use super::*;
use crate::ai::agent::conversation::AIConversation;
use crate::persistence::ModelEvent;
use crate::server::server_api::ai::MockAIClient;
use crate::server::server_api::ServerApiProvider;
use crate::test_util::settings::initialize_settings_for_tests;
use crate::{GlobalResourceHandles, GlobalResourceHandlesProvider};
use std::sync::Arc;
use warpui::App;

fn make_run_event(event_type: &str, run_id: &str, ref_id: Option<&str>) -> AgentRunEvent {
    AgentRunEvent {
        event_type: event_type.to_string(),
        run_id: run_id.to_string(),
        ref_id: ref_id.map(|s| s.to_string()),
        execution_id: None,
        occurred_at: "2026-01-01T00:00:00Z".to_string(),
        sequence: 1,
    }
}

#[test]
fn convert_lifecycle_events_includes_run_blocked() {
    let events = vec![make_run_event("run_blocked", "child-run", None)];
    let result = convert_lifecycle_events(&events, "self-run");
    assert_eq!(result.len(), 1);
    let event = &result[0];
    let Some(api::agent_event::Event::LifecycleEvent(lifecycle)) = &event.event else {
        panic!("expected lifecycle event");
    };
    let Some(api::agent_event::lifecycle_event::Detail::Blocked(blocked)) = &lifecycle.detail
    else {
        panic!("expected blocked detail");
    };
    assert!(blocked.blocked_action.is_empty());
}

#[test]
fn convert_lifecycle_events_filters_self_run_blocked() {
    let events = vec![make_run_event("run_blocked", "self-run", None)];
    let result = convert_lifecycle_events(&events, "self-run");
    assert!(result.is_empty());
}

#[test]
fn convert_lifecycle_events_maps_run_restarted() {
    let events = vec![make_run_event("run_restarted", "child-run", None)];
    let result = convert_lifecycle_events(&events, "self-run");
    assert_eq!(result.len(), 1);
    let event = &result[0];
    let Some(api::agent_event::Event::LifecycleEvent(lifecycle)) = &event.event else {
        panic!("expected lifecycle event");
    };
    assert!(matches!(
        lifecycle.detail,
        Some(api::agent_event::lifecycle_event::Detail::InProgress(..))
    ));
}

#[test]
fn ai_conversation_new_restored_preserves_last_event_sequence() {
    // Guards against regressions that drop the field when wiring the restore
    // path: a conversation restored with `last_event_sequence: Some(N)`
    // should expose it via `conversation.last_event_sequence()`.
    use crate::ai::agent::conversation::{AIConversation, AIConversationId};
    use crate::persistence::model::AgentConversationData;

    let task = api::Task {
        id: "root".to_string(),
        messages: vec![api::Message {
            id: "m1".to_string(),
            task_id: "root".to_string(),
            server_message_data: String::new(),
            citations: vec![],
            message: Some(api::message::Message::AgentOutput(
                api::message::AgentOutput {
                    text: "hi".to_string(),
                },
            )),
            request_id: String::new(),
            timestamp: None,
        }],
        dependencies: None,
        description: String::new(),
        summary: String::new(),
        server_data: String::new(),
    };
    let data = AgentConversationData {
        server_conversation_token: None,
        conversation_usage_metadata: None,
        reverted_action_ids: None,
        forked_from_server_conversation_token: None,
        artifacts_json: None,
        parent_agent_id: None,
        agent_name: None,
        orchestration_harness_type: None,
        parent_conversation_id: None,
        is_remote_child: false,
        run_id: None,
        autoexecute_override: None,
        last_event_sequence: Some(42),
        pinned: false,
    };
    let conversation =
        AIConversation::new_restored(AIConversationId::new(), vec![task], Some(data))
            .expect("should restore");
    assert_eq!(conversation.last_event_sequence(), Some(42));
}

// ---- Helpers for App-based poller tests ----

fn make_server_metadata_with_harness(
    harness: AIAgentHarness,
) -> crate::ai::agent::conversation::ServerAIConversationMetadata {
    use crate::ai::agent::api::ServerConversationToken;
    use crate::cloud_object::{Revision, ServerMetadata, ServerPermissions};
    use crate::persistence::model::ConversationUsageMetadata;
    use crate::server::ids::ServerId;
    use chrono::Utc;

    crate::ai::agent::conversation::ServerAIConversationMetadata {
        title: "test".to_string(),
        working_directory: None,
        harness,
        usage: ConversationUsageMetadata {
            was_summarized: false,
            context_window_usage: 0.0,
            credits_spent: 0.0,
            credits_spent_for_last_block: None,
            token_usage: vec![],
            tool_usage_metadata: Default::default(),
        },
        metadata: ServerMetadata {
            uid: ServerId::default(),
            revision: Revision::now(),
            metadata_last_updated_ts: Utc::now().into(),
            trashed_ts: None,
            folder_id: None,
            is_welcome_object: false,
            creator_uid: None,
            last_editor_uid: None,
            current_editor_uid: None,
        },
        permissions: ServerPermissions::mock_personal(),
        ambient_agent_task_id: None,
        server_conversation_token: ServerConversationToken::new("server-token".to_string()),
        artifacts: vec![],
    }
}

#[test]
fn dormant_local_claude_child_skips_generic_sse_but_allows_wake_listener() {
    use crate::ai::agent::conversation::{AIConversation, ConversationStatus};
    use crate::server::server_api::ai::MockAIClient;
    use crate::server::server_api::ServerApiProvider;
    use std::sync::Arc;
    use warpui::App;

    App::test((), |mut app| async move {
        let _v2_guard = FeatureFlag::OrchestrationV2.override_enabled(true);

        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));

        let parent_id = AIConversation::new(false, false).id();
        let mut conversation = AIConversation::new(false, false);
        let run_id = "550e8400-e29b-41d4-a716-446655440610".to_string();
        conversation.set_run_id(run_id.clone());
        conversation.set_parent_conversation_id(parent_id);
        conversation.set_server_metadata(make_server_metadata_with_harness(
            AIAgentHarness::ClaudeCode,
        ));
        let conversation_id = conversation.id();
        let terminal_view_id = warpui::EntityId::new();
        history_model.update(&mut app, |model, ctx| {
            model.restore_conversations(terminal_view_id, vec![conversation], ctx);
            model.update_conversation_status(
                terminal_view_id,
                conversation_id,
                ConversationStatus::Success,
                ctx,
            );
        });

        let mock = MockAIClient::new();
        let ai_client: Arc<dyn AIClient> = Arc::new(mock);
        let server_api = ServerApiProvider::new_for_test().get();

        let streamer = app.add_singleton_model(|ctx| {
            OrchestrationEventStreamer::new_with_clients_for_test(ai_client, server_api, ctx)
        });

        streamer.update(&mut app, |me, _| {
            let stream = me.streams.entry(conversation_id).or_default();
            stream.consumers.insert(warpui::EntityId::new());
            stream.watched_run_ids.insert(run_id);
        });

        streamer.read(&app, |me, ctx| {
            assert!(
                !me.is_eligible(conversation_id, ctx),
                "generic SSE must stay closed for dormant local Claude children"
            );
            assert!(
                me.is_dormant_claude_wake_listener_eligible(conversation_id, ctx),
                "wake-only listener should open for dormant local Claude children"
            );
        });
    });
}

#[test]
fn dormant_local_claude_child_uses_task_harness_when_server_metadata_missing() {
    use crate::ai::agent::conversation::{AIConversation, ConversationStatus};
    use crate::server::server_api::ai::MockAIClient;
    use crate::server::server_api::ServerApiProvider;
    use std::sync::Arc;
    use warp_cli::agent::Harness;
    use warpui::App;

    App::test((), |mut app| async move {
        let _v2_guard = FeatureFlag::OrchestrationV2.override_enabled(true);

        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));

        let parent_id = AIConversation::new(false, false).id();
        let mut conversation = AIConversation::new(false, false);
        let run_id = "550e8400-e29b-41d4-a716-446655440611".to_string();
        conversation.set_run_id(run_id.clone());
        conversation.set_parent_conversation_id(parent_id);
        let conversation_id = conversation.id();
        let terminal_view_id = warpui::EntityId::new();
        history_model.update(&mut app, |model, ctx| {
            model.restore_conversations(terminal_view_id, vec![conversation], ctx);
            model.update_conversation_status(
                terminal_view_id,
                conversation_id,
                ConversationStatus::Success,
                ctx,
            );
        });

        let mock = MockAIClient::new();
        let ai_client: Arc<dyn AIClient> = Arc::new(mock);
        let server_api = ServerApiProvider::new_for_test().get();

        let streamer = app.add_singleton_model(|ctx| {
            OrchestrationEventStreamer::new_with_clients_for_test(ai_client, server_api, ctx)
        });

        streamer.update(&mut app, |me, _| {
            let stream = me.streams.entry(conversation_id).or_default();
            stream.consumers.insert(warpui::EntityId::new());
            stream.watched_run_ids.insert(run_id);
        });

        streamer.read(&app, |me, ctx| {
            assert!(
                me.is_eligible(conversation_id, ctx),
                "generic SSE should remain eligible before the task harness is known"
            );
            assert!(
                !me.is_dormant_claude_wake_listener_eligible(conversation_id, ctx),
                "wake-only listener should wait until the task harness identifies Claude"
            );
        });

        streamer.update(&mut app, |me, _| {
            me.streams
                .get_mut(&conversation_id)
                .expect("stream exists")
                .harness = Some(Harness::Claude);
        });

        streamer.read(&app, |me, ctx| {
            assert!(
                !me.is_eligible(conversation_id, ctx),
                "generic SSE must close after task metadata identifies a dormant local Claude child"
            );
            assert!(
                me.is_dormant_claude_wake_listener_eligible(conversation_id, ctx),
                "wake-only listener should open based on cached task harness even without server metadata"
            );
        });
    });
}
#[test]
fn restored_conversations_skip_v2_streaming_when_orchestration_v2_disabled() {
    use crate::ai::agent::conversation::AIConversation;
    use crate::server::server_api::ai::MockAIClient;
    use crate::server::server_api::ServerApiProvider;
    use std::sync::Arc;
    use warpui::App;

    App::test((), |mut app| async move {
        let _v2_guard = FeatureFlag::OrchestrationV2.override_enabled(false);

        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));

        let mut conversation = AIConversation::new(false, false);
        conversation.set_run_id("550e8400-e29b-41d4-a716-446655440500".to_string());
        let conversation_id = conversation.id();
        let terminal_view_id = warpui::EntityId::new();
        history_model.update(&mut app, |model, ctx| {
            model.restore_conversations(terminal_view_id, vec![conversation], ctx);
        });

        let mock = MockAIClient::new();
        let ai_client: Arc<dyn AIClient> = Arc::new(mock);
        let server_api = ServerApiProvider::new_for_test().get();

        let streamer = app.add_singleton_model(|ctx| {
            OrchestrationEventStreamer::new_with_clients_for_test(ai_client, server_api, ctx)
        });

        streamer.update(&mut app, |me, ctx| {
            me.on_restored_conversations(vec![conversation_id], ctx);
        });

        streamer.read(&app, |me, _| {
            assert!(
                me.streams.is_empty(),
                "V2-disabled restore must not initialize stream state"
            );
        });
    });
}

#[test]
fn build_pending_events_preserves_message_sequence_and_timestamp() {
    let occurred_at = "2026-01-02T03:04:05Z";
    let pending = build_pending_events(
        &[AgentRunEvent {
            event_type: "new_message".to_string(),
            run_id: "sender-run".to_string(),
            ref_id: Some("message-123".to_string()),
            execution_id: None,
            occurred_at: occurred_at.to_string(),
            sequence: 77,
        }],
        vec![ReceivedMessageInput {
            message_id: "message-123".to_string(),
            sender_agent_id: "sender-agent".to_string(),
            addresses: vec!["recipient-agent".to_string()],
            subject: "subject".to_string(),
            message_body: "body".to_string(),
        }],
        vec![],
    );

    assert_eq!(pending.len(), 1);
    let detail = &pending[0].detail;
    let PendingEventDetail::Message {
        sequence,
        message_id,
        occurred_at: event_occurred_at,
        ..
    } = detail
    else {
        panic!("expected pending message event");
    };
    assert_eq!(*sequence, 77);
    assert_eq!(message_id, "message-123");
    assert_eq!(event_occurred_at, occurred_at);
}

#[test]
fn handle_event_batch_persists_max_seq_to_history_model() {
    use crate::ai::agent::conversation::{AIConversation, AIConversationId};
    use crate::persistence::ModelEvent;
    use crate::server::server_api::ai::MockAIClient;
    use crate::server::server_api::ServerApiProvider;
    use crate::test_util::settings::initialize_settings_for_tests;
    use crate::{GlobalResourceHandles, GlobalResourceHandlesProvider};
    use std::sync::Arc;
    use warpui::App;

    App::test((), |mut app| async move {
        let _v2_guard = FeatureFlag::OrchestrationV2.override_enabled(true);

        // `update_event_sequence` calls `write_updated_conversation_state`,
        // which reads `GeneralSettings`, `AppExecutionMode`, and the global
        // resource sender. Wire all of these up so the SQLite write can run.
        initialize_settings_for_tests(&mut app);
        let (sender, receiver) = std::sync::mpsc::sync_channel::<ModelEvent>(4);
        let mut global_resource_handles = GlobalResourceHandles::mock(&mut app);
        global_resource_handles.model_event_sender = Some(sender);
        app.add_singleton_model(|_| GlobalResourceHandlesProvider::new(global_resource_handles));

        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));

        let mut conversation = AIConversation::new(false, false);
        conversation.set_run_id("550e8400-e29b-41d4-a716-446655440200".to_string());
        let conversation_id: AIConversationId = conversation.id();
        let terminal_view_id = warpui::EntityId::new();
        history_model.update(&mut app, |model, ctx| {
            model.restore_conversations(terminal_view_id, vec![conversation], ctx);
        });

        let ai_client: Arc<dyn AIClient> = Arc::new(MockAIClient::new());
        let server_api = ServerApiProvider::new_for_test().get();

        let poller = app.add_singleton_model(|ctx| {
            OrchestrationEventStreamer::new_with_clients_for_test(ai_client, server_api, ctx)
        });

        // Build a poll batch with max sequence = 42. Use an unrecognized
        // event_type so `convert_lifecycle_events` returns empty and the
        // function early-exits before touching `OrchestrationEventService`
        // (which we did not register in this test App).
        let events = vec![
            AgentRunEvent {
                event_type: "unrecognized_event_type".to_string(),
                run_id: "some-other-run".to_string(),
                ref_id: None,
                execution_id: None,
                occurred_at: "2026-01-01T00:00:00Z".to_string(),
                sequence: 17,
            },
            AgentRunEvent {
                event_type: "unrecognized_event_type".to_string(),
                run_id: "some-other-run".to_string(),
                ref_id: None,
                execution_id: None,
                occurred_at: "2026-01-01T00:00:00Z".to_string(),
                sequence: 42,
            },
        ];

        poller.update(&mut app, |me, ctx| {
            me.handle_event_batch(
                conversation_id,
                /* self_run_id */ "some-other-run",
                /* previous_cursor */ 0,
                events,
                /* messages */ vec![],
                ctx,
            );
        });

        history_model.read(&app, |model, _| {
            let last_seq = model
                .conversation(&conversation_id)
                .and_then(|c| c.last_event_sequence());
            assert_eq!(
                last_seq,
                Some(42),
                "BlocklistAIHistoryModel.update_event_sequence must be called with max_seq"
            );
        });

        // Drain at least one persistence event to confirm the SQLite write
        // path was triggered (sanity check for the side effect, not the
        // primary assertion).
        let _ = receiver.recv_timeout(std::time::Duration::from_secs(1));
    });
}

#[test]
fn handle_event_batch_drops_events_for_killed_run_ids_after_persisting_cursor() {
    App::test((), |mut app| async move {
        let _v2_guard = FeatureFlag::OrchestrationV2.override_enabled(true);

        initialize_settings_for_tests(&mut app);
        let (sender, _receiver) = std::sync::mpsc::sync_channel::<ModelEvent>(4);
        let mut global_resource_handles = GlobalResourceHandles::mock(&mut app);
        global_resource_handles.model_event_sender = Some(sender);
        app.add_singleton_model(|_| GlobalResourceHandlesProvider::new(global_resource_handles));

        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));
        let event_service = app.add_singleton_model(|_| OrchestrationEventService::default());

        let parent_run_id = "550e8400-e29b-41d4-a716-446655440700".to_string();
        let killed_run_id = "550e8400-e29b-41d4-a716-446655440701".to_string();
        let mut parent_conversation = AIConversation::new(false, false);
        parent_conversation.set_run_id(parent_run_id.clone());
        let parent_conversation_id = parent_conversation.id();
        let terminal_view_id = warpui::EntityId::new();
        history_model.update(&mut app, |model, ctx| {
            model.restore_conversations(terminal_view_id, vec![parent_conversation], ctx);
        });

        let ai_client: Arc<dyn AIClient> = Arc::new(MockAIClient::new());
        let server_api = ServerApiProvider::new_for_test().get();

        let streamer = app.add_singleton_model(|ctx| {
            OrchestrationEventStreamer::new_with_clients_for_test(ai_client, server_api, ctx)
        });

        streamer.update(&mut app, |me, ctx| {
            me.streams.entry(parent_conversation_id).or_default();
            me.remember_killed_run_id(killed_run_id.clone());
            me.handle_event_batch(
                parent_conversation_id,
                &parent_run_id,
                0,
                vec![
                    AgentRunEvent {
                        event_type: "new_message".to_string(),
                        run_id: killed_run_id.clone(),
                        ref_id: Some("message-from-killed-child".to_string()),
                        execution_id: None,
                        occurred_at: "2026-01-01T00:00:00Z".to_string(),
                        sequence: 17,
                    },
                    AgentRunEvent {
                        event_type: "run_cancelled".to_string(),
                        run_id: killed_run_id.clone(),
                        ref_id: None,
                        execution_id: None,
                        occurred_at: "2026-01-01T00:00:01Z".to_string(),
                        sequence: 18,
                    },
                    AgentRunEvent {
                        event_type: "new_message".to_string(),
                        run_id: killed_run_id.clone(),
                        ref_id: None,
                        execution_id: None,
                        occurred_at: "2026-01-01T00:00:02Z".to_string(),
                        sequence: 19,
                    },
                ],
                vec![
                    ReceivedMessageInput {
                        message_id: "message-from-killed-child".to_string(),
                        sender_agent_id: killed_run_id.clone(),
                        addresses: vec![parent_run_id.clone()],
                        subject: "late message".to_string(),
                        message_body: "body".to_string(),
                    },
                    ReceivedMessageInput {
                        message_id: "message-from-killed-child-without-ref".to_string(),
                        sender_agent_id: killed_run_id.clone(),
                        addresses: vec![parent_run_id.clone()],
                        subject: "late message without ref".to_string(),
                        message_body: "body".to_string(),
                    },
                ],
                ctx,
            );
        });

        event_service.read(&app, |service, _| {
            assert!(
                !service.has_pending_events(parent_conversation_id),
                "late events from killed run IDs must not be enqueued"
            );
        });
        history_model.read(&app, |model, _| {
            let last_seq = model
                .conversation(&parent_conversation_id)
                .and_then(|conversation| conversation.last_event_sequence());
            assert_eq!(
                last_seq,
                Some(19),
                "cursor must still advance so dropped killed-run events are not replayed"
            );
        });
    });
}

#[test]
fn killed_run_ids_are_bounded() {
    App::test((), |mut app| async move {
        app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));
        let mock = MockAIClient::new();
        let ai_client: Arc<dyn AIClient> = Arc::new(mock);
        let server_api = ServerApiProvider::new_for_test().get();
        let streamer = app.add_singleton_model(|ctx| {
            OrchestrationEventStreamer::new_with_clients_for_test(ai_client, server_api, ctx)
        });

        streamer.update(&mut app, |me, _| {
            for index in 0..=MAX_KILLED_RUN_IDS {
                me.remember_killed_run_id(format!("killed-run-{index}"));
            }
        });

        streamer.read(&app, |me, _| {
            assert_eq!(me.killed_run_ids.len(), MAX_KILLED_RUN_IDS);
            assert!(!me.killed_run_ids.contains("killed-run-0"));
            assert!(me.killed_run_ids.contains("killed-run-1"));
            assert!(me
                .killed_run_ids
                .contains(&format!("killed-run-{MAX_KILLED_RUN_IDS}")));
        });
    });
}

#[test]
fn on_conversation_removed_prunes_stale_child_run_id_from_parent() {
    // Regression for the case where a child conversation is deleted: the
    // parent's `watched_run_ids` set must be pruned of that child's run_id
    // so subsequent SSE reconnects do not include the dead run_id in the
    // filter. Previously the streamer looked up the run_id from the history
    // model after the removal, which always returned `None` because the
    // history model emits `RemoveConversation` after dropping the record.
    use crate::ai::agent::conversation::AIConversation;
    use crate::server::server_api::ai::MockAIClient;
    use crate::server::server_api::ServerApiProvider;
    use std::sync::Arc;
    use warpui::App;

    App::test((), |mut app| async move {
        let _v2_guard = FeatureFlag::OrchestrationV2.override_enabled(true);

        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));

        let parent_id = AIConversation::new(false, false).id();
        let mut child_conversation = AIConversation::new(false, false);
        let child_run_id = "550e8400-e29b-41d4-a716-446655440600".to_string();
        child_conversation.set_run_id(child_run_id.clone());
        let child_id = child_conversation.id();
        let terminal_view_id = warpui::EntityId::new();
        history_model.update(&mut app, |model, ctx| {
            model.restore_conversations(terminal_view_id, vec![child_conversation], ctx);
        });

        let mock = MockAIClient::new();
        let ai_client: Arc<dyn AIClient> = Arc::new(mock);
        let server_api = ServerApiProvider::new_for_test().get();

        let poller = app.add_singleton_model(|ctx| {
            OrchestrationEventStreamer::new_with_clients_for_test(ai_client, server_api, ctx)
        });

        // Seed the parent's watched set with the child's run_id, as
        // `register_watched_run_id` would have done after the child got
        // its server token.
        poller.update(&mut app, |me, _| {
            me.streams
                .entry(parent_id)
                .or_default()
                .watched_run_ids
                .insert(child_run_id.clone());
        });

        // Now invoke the removal handler with the run_id (mirroring the
        // event payload that history_model emits with the captured run_id).
        poller.update(&mut app, |me, ctx| {
            me.on_conversation_removed(child_id, Some(child_run_id.clone()), ctx);
        });

        poller.read(&app, |me, _| {
            assert!(
                me.streams
                    .get(&parent_id)
                    .is_some_and(|s| !s.watched_run_ids.contains(&child_run_id)),
                "parent's watched_run_ids must be pruned of the removed child's run_id"
            );
        });
    });
}

#[test]
fn on_conversation_removed_prunes_killed_child_run_id_from_parent_but_keeps_tombstone() {
    use crate::ai::agent::conversation::AIConversation;
    use crate::server::server_api::ai::MockAIClient;
    use crate::server::server_api::ServerApiProvider;
    use std::sync::Arc;
    use warpui::App;

    App::test((), |mut app| async move {
        let _v2_guard = FeatureFlag::OrchestrationV2.override_enabled(true);

        app.add_singleton_model(|_| BlocklistAIHistoryModel::new(vec![], &[]));

        let parent_id = AIConversation::new(false, false).id();
        let child_id = AIConversation::new(false, false).id();
        let child_run_id = "550e8400-e29b-41d4-a716-446655440601".to_string();

        let mock = MockAIClient::new();
        let ai_client: Arc<dyn AIClient> = Arc::new(mock);
        let server_api = ServerApiProvider::new_for_test().get();

        let poller = app.add_singleton_model(|ctx| {
            OrchestrationEventStreamer::new_with_clients_for_test(ai_client, server_api, ctx)
        });

        poller.update(&mut app, |me, ctx| {
            me.streams
                .entry(parent_id)
                .or_default()
                .watched_run_ids
                .insert(child_run_id.clone());
            me.remember_killed_run_id(child_run_id.clone());

            me.on_conversation_removed(child_id, Some(child_run_id.clone()), ctx);
        });

        poller.read(&app, |me, _| {
            assert!(me.killed_run_ids.contains(&child_run_id));
            assert!(
                me.streams
                    .get(&parent_id)
                    .is_some_and(|s| !s.watched_run_ids.contains(&child_run_id)),
                "killed child run_id should be pruned from parent watchers"
            );
        });
    });
}
