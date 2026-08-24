use ai::agent::action_result::StartAgentVersion;
use warp_core::features::FeatureFlag;
use warpui::{App, EntityId, SingletonEntity};

use super::*;
use crate::ai::agent::conversation::ConversationStatus;
use crate::ai::agent::task::TaskId;
use crate::ai::agent::{
    AIAgentAction, AIAgentActionId, AIAgentActionResultType, AIAgentActionType, RenderableAIError,
    StartAgentExecutionMode, StartAgentResult,
};
use crate::ai::blocklist::BlocklistAIHistoryModel;
use crate::ai::blocklist::orchestration_event_streamer::OrchestrationEventStreamer;
use crate::ai::local_agent_registry::{
    LocalAgentRegistry, LocalAgentStatus, local_controller_owner_id,
};
use crate::server::server_api::ServerApiProvider;
use crate::test_util::settings::initialize_history_persistence_for_tests;

const FIRST_REQUEST_ID: StartAgentRequestId = StartAgentRequestId::from_raw_for_test(0);

macro_rules! start_local_parent {
    ($app:ident, $history:ident, $terminal_view_id:ident) => {{
        let registry = $app.add_singleton_model(|_| LocalAgentRegistry::new());
        let conversation_id = $history.update(&mut $app, |history, ctx| {
            history.start_new_conversation($terminal_view_id, false, false, false, ctx)
        });
        let run_id = $history
            .update(&mut $app, |history, ctx| {
                history.ensure_local_run_id_for_conversation(conversation_id, ctx)
            })
            .expect("local parent run id");
        registry.update(&mut $app, |registry, _ctx| {
            registry
                .register_existing(
                    run_id,
                    conversation_id,
                    Some($terminal_view_id),
                    None,
                    None,
                    "parent".to_string(),
                    warp_cli::agent::Harness::Oz,
                    Some(local_controller_owner_id($terminal_view_id)),
                    LocalAgentStatus::Running,
                )
                .expect("register local parent");
        });
        conversation_id
    }};
}

macro_rules! register_local_child {
    ($app:ident, $history:ident, $terminal_view_id:ident, $conversation_id:ident) => {{
        let (run_id, parent_run_id, name, harness) = $history.read(&$app, |history, _| {
            let conversation = history
                .conversation(&$conversation_id)
                .expect("child conversation");
            (
                conversation.run_id().expect("child local run id"),
                conversation.parent_agent_id().map(ToString::to_string),
                conversation.agent_name().unwrap_or("child").to_string(),
                conversation
                    .orchestration_harness()
                    .unwrap_or(warp_cli::agent::Harness::Oz),
            )
        });
        LocalAgentRegistry::handle(&mut $app).update(&mut $app, |registry, _ctx| {
            registry
                .register_existing(
                    run_id,
                    $conversation_id,
                    Some($terminal_view_id),
                    None,
                    parent_run_id,
                    name,
                    harness,
                    Some(local_controller_owner_id($terminal_view_id)),
                    LocalAgentStatus::Starting,
                )
                .expect("register local child");
        });
    }};
}

fn build_start_agent_action(
    version: StartAgentVersion,
    execution_mode: StartAgentExecutionMode,
) -> AIAgentAction {
    AIAgentAction {
        id: AIAgentActionId::from("start-agent-action".to_string()),
        action: AIAgentActionType::StartAgent {
            version,
            name: "Agent 1".to_string(),
            prompt: "Investigate the failure".to_string(),
            execution_mode,
            lifecycle_subscription: None,
        },
        task_id: TaskId::new("start-agent-task".to_string()),
        requires_result: false,
    }
}

#[test]
fn execute_returns_error_when_child_startup_is_blocked_before_initialization() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);
        let action = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("expected async execution");
        };

        let child_conversation_id = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_child_conversation(
                terminal_view_id,
                "Agent 1".to_string(),
                parent_conversation_id,
                None,
                ctx,
            )
        });
        register_local_child!(app, history_model, terminal_view_id, child_conversation_id);

        history_model.update(&mut app, |history_model, ctx| {
            history_model.update_conversation_status(
                terminal_view_id,
                child_conversation_id,
                ConversationStatus::Blocked {
                    blocked_action:
                        "GitHub authentication required before starting the child agent."
                            .to_string(),
                },
                ctx,
            );
        });
        history_model.update(&mut app, |model, ctx| {
            model.record_new_conversation_request_complete(
                FIRST_REQUEST_ID,
                child_conversation_id,
                ctx,
            );
        });

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));
        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
                if error
                    == "GitHub authentication required before starting the child agent."
                    && version == StartAgentVersion::V1
        ));

        executor.read(&app, |executor, _| {
            assert!(executor.pending.is_empty());
        });
    });
}

#[test]
fn execute_resolves_error_when_request_linkage_happens_after_child_already_failed() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);
        let action = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("expected async execution");
        };

        let child_conversation_id = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_child_conversation(
                terminal_view_id,
                "Agent 1".to_string(),
                parent_conversation_id,
                None,
                ctx,
            )
        });
        register_local_child!(app, history_model, terminal_view_id, child_conversation_id);

        history_model.update(&mut app, |history_model, ctx| {
            history_model.update_conversation_status_with_error(
                terminal_view_id,
                child_conversation_id,
                ConversationStatus::Error,
                Some(RenderableAIError::other(
                    "'codex' CLI not found on your machine.",
                    false,
                )),
                ctx,
            );
        });

        history_model.update(&mut app, |model, ctx| {
            model.record_new_conversation_request_complete(
                FIRST_REQUEST_ID,
                child_conversation_id,
                ctx,
            );
        });

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));
        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
                if error == "'codex' CLI not found on your machine."
                    && version == StartAgentVersion::V1
        ));

        executor.read(&app, |executor, _| {
            assert!(executor.pending.is_empty());
        });
    });
}

#[test]
fn local_start_agent_uses_local_run_id_when_request_linkage_arrives_after_start() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        app.add_singleton_model(|_| ServerApiProvider::new_for_test());
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        app.add_singleton_model(OrchestrationEventStreamer::new);
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);
        let action = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("expected async execution");
        };

        let child_conversation_id = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_child_conversation(
                terminal_view_id,
                "Agent 1".to_string(),
                parent_conversation_id,
                None,
                ctx,
            )
        });
        register_local_child!(app, history_model, terminal_view_id, child_conversation_id);
        let run_id = history_model.read(&app, |history_model, _| {
            history_model
                .conversation(&child_conversation_id)
                .and_then(|conversation| conversation.run_id())
                .expect("stable child run id")
        });

        history_model.update(&mut app, |model, ctx| {
            model.record_new_conversation_request_complete(
                FIRST_REQUEST_ID,
                child_conversation_id,
                ctx,
            );
        });

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));
        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Success {
                agent_id,
                version,
            }) if agent_id == run_id && version == StartAgentVersion::V1
        ));

        executor.read(&app, |executor, _| {
            assert!(executor.pending.is_empty());
        });
    });
}

#[test]
fn execute_returns_detailed_error_when_child_startup_fails_before_initialization() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);
        let action = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("expected async execution");
        };

        let child_conversation_id = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_child_conversation(
                terminal_view_id,
                "Agent 1".to_string(),
                parent_conversation_id,
                None,
                ctx,
            )
        });
        register_local_child!(app, history_model, terminal_view_id, child_conversation_id);

        history_model.update(&mut app, |history_model, ctx| {
            history_model.update_conversation_status_with_error(
                terminal_view_id,
                child_conversation_id,
                ConversationStatus::Error,
                Some(RenderableAIError::other(
                    "Failed to resolve child agent skills: review-comments",
                    false,
                )),
                ctx,
            );
        });
        history_model.update(&mut app, |model, ctx| {
            model.record_new_conversation_request_complete(
                FIRST_REQUEST_ID,
                child_conversation_id,
                ctx,
            );
        });

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));
        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
                if error == "Failed to resolve child agent skills: review-comments"
                    && version == StartAgentVersion::V1
        ));
    });
}

#[test]
fn local_start_agent_local_harness_does_not_require_hosted_orchestration_v2() {
    App::test((), |mut app| async move {
        let _local_harnesses = FeatureFlag::LocalClaudeCodexChildHarnesses.override_enabled(true);
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);
        let action = build_start_agent_action(
            StartAgentVersion::V2,
            StartAgentExecutionMode::local_harness("codex".to_string()),
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        assert!(matches!(execution, AnyActionExecution::Async { .. }));
    });
}

#[test]
fn execute_rejects_invalid_local_harness_names_before_pane_creation() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_conversation(terminal_view_id, false, false, false, ctx)
        });
        let action = build_start_agent_action(
            StartAgentVersion::V2,
            StartAgentExecutionMode::local_harness("gemini".to_string()),
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Sync(result) = execution else {
            panic!("expected sync execution");
        };

        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
                if error == "Unsupported local child harness 'gemini'."
                    && version == StartAgentVersion::V2
        ));
    });
}

#[test]
fn execute_returns_error_when_local_harness_child_missing_parent_run_id() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        let _local_harnesses = FeatureFlag::LocalClaudeCodexChildHarnesses.override_enabled(true);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_conversation(terminal_view_id, false, false, false, ctx)
        });
        let action = build_start_agent_action(
            StartAgentVersion::V2,
            StartAgentExecutionMode::local_harness("claude".to_string()),
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Sync(result) = execution else {
            panic!("expected sync execution");
        };

        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
                if error
                    == "Local harness child agents require the parent run_id to be available."
                    && version == StartAgentVersion::V2
        ));
    });
}

#[test]
fn execute_rejects_disabled_local_claude_before_other_local_harness_validation() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_conversation(terminal_view_id, false, false, false, ctx)
        });
        let action = build_start_agent_action(
            StartAgentVersion::V2,
            StartAgentExecutionMode::local_harness("claude".to_string()),
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Sync(result) = execution else {
            panic!("expected sync execution");
        };

        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
                if error == "Local Claude Code child agents are temporarily disabled."
                    && version == StartAgentVersion::V2
        ));
    });
}

#[test]
fn local_start_agent_parallel_actions_keep_distinct_request_ids() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);

        let action_a = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );
        let mut action_b = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );
        action_b.id = AIAgentActionId::from("start-agent-action-b".to_string());
        executor.update(&mut app, |executor, ctx| {
            let _: AnyActionExecution = executor
                .execute(
                    ExecuteActionInput {
                        action: &action_a,
                        conversation_id: parent_conversation_id,
                    },
                    ctx,
                )
                .into();
            let _: AnyActionExecution = executor
                .execute(
                    ExecuteActionInput {
                        action: &action_b,
                        conversation_id: parent_conversation_id,
                    },
                    ctx,
                )
                .into();
        });

        executor.read(&app, |executor, _| {
            assert_eq!(executor.pending.len(), 2, "both pendings should be live");
            assert!(executor.pending.contains_key(&FIRST_REQUEST_ID));
            assert!(
                executor
                    .pending
                    .contains_key(&StartAgentRequestId::from_raw_for_test(1))
            );
        });
    });
}

#[test]
fn local_start_agent_duplicate_action_waits_on_original_request() {
    App::test((), |mut app| async move {
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);
        let action = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );

        let (first, duplicate) = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            (executor.execute(input, ctx), executor.execute(input, ctx))
        });

        assert!(matches!(first, AnyActionExecution::Async { .. }));
        assert!(matches!(duplicate, AnyActionExecution::Async { .. }));
        executor.read(&app, |executor, _| {
            assert_eq!(executor.pending.len(), 1);
            assert_eq!(executor.pending_by_action.len(), 1);
            assert_eq!(executor.pending[&FIRST_REQUEST_ID].senders.len(), 2);
        });
    });
}

#[test]
fn local_start_agent_cancellation_finishes_the_original_request() {
    App::test((), |mut app| async move {
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);
        let action = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );
        let execution = executor.update(&mut app, |executor, ctx| {
            let execution = executor.execute(
                ExecuteActionInput {
                    action: &action,
                    conversation_id: parent_conversation_id,
                },
                ctx,
            );
            executor.cancel_execution(&action.id, ctx);
            execution
        });
        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("expected async execution");
        };

        let async_result = execute_future.await;
        let result = app.update(|ctx| on_complete(async_result, ctx));
        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Cancelled {
                version: StartAgentVersion::V1
            })
        ));
        executor.read(&app, |executor, _| {
            assert!(executor.pending.is_empty());
            assert!(matches!(
                executor.completed_by_action.get(&action.id),
                Some(StartAgentOutcome::Cancelled)
            ));
        });
    });
}

#[test]
fn local_start_agent_parallel_pendings_resolve_independently() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        initialize_history_persistence_for_tests(&mut app);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = start_local_parent!(app, history_model, terminal_view_id);

        let action_a = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );
        let mut action_b = build_start_agent_action(
            StartAgentVersion::V1,
            StartAgentExecutionMode::local_with_defaults(),
        );
        action_b.id = AIAgentActionId::from("start-agent-action-b".to_string());
        let exec_a = executor.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor
                .execute(
                    ExecuteActionInput {
                        action: &action_a,
                        conversation_id: parent_conversation_id,
                    },
                    ctx,
                )
                .into();
            result
        });
        let exec_b = executor.update(&mut app, |executor, ctx| {
            let result: AnyActionExecution = executor
                .execute(
                    ExecuteActionInput {
                        action: &action_b,
                        conversation_id: parent_conversation_id,
                    },
                    ctx,
                )
                .into();
            result
        });
        let (
            AnyActionExecution::Async {
                execute_future: future_a,
                on_complete: complete_a,
            },
            AnyActionExecution::Async {
                execute_future: future_b,
                on_complete: complete_b,
            },
        ) = (exec_a, exec_b)
        else {
            panic!("expected async executions");
        };

        let child_a = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_child_conversation(
                terminal_view_id,
                "Agent A".to_string(),
                parent_conversation_id,
                None,
                ctx,
            )
        });
        let child_b = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_child_conversation(
                terminal_view_id,
                "Agent B".to_string(),
                parent_conversation_id,
                None,
                ctx,
            )
        });
        register_local_child!(app, history_model, terminal_view_id, child_a);
        register_local_child!(app, history_model, terminal_view_id, child_b);

        history_model.update(&mut app, |history_model, ctx| {
            history_model.update_conversation_status_with_error(
                terminal_view_id,
                child_b,
                ConversationStatus::Error,
                Some(RenderableAIError::other("Agent B init failed", false)),
                ctx,
            );
        });
        history_model.update(&mut app, |model, ctx| {
            model.record_new_conversation_request_complete(FIRST_REQUEST_ID, child_a, ctx);
            model.record_new_conversation_request_complete(
                StartAgentRequestId::from_raw_for_test(1),
                child_b,
                ctx,
            );
        });

        let async_b = future_b.await;
        let result_b = app.update(|ctx| complete_b(async_b, ctx));
        assert!(matches!(
            result_b,
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, .. })
                if error == "Agent B init failed"
        ));

        executor.read(&app, |executor, _| {
            assert_eq!(
                executor.pending.len(),
                0,
                "both independently completed children should be removed"
            );
        });

        let async_a = future_a.await;
        let result_a = app.update(|ctx| complete_a(async_a, ctx));
        assert!(matches!(
            result_a,
            AIAgentActionResultType::StartAgent(StartAgentResult::Success { .. })
        ));
    });
}

#[test]
fn execute_treats_remote_opencode_harness_as_local_child_harness() {
    App::test((), |mut app| async move {
        let _orchestration_v2 = FeatureFlag::OrchestrationV2.override_enabled(true);
        let terminal_view_id = EntityId::new();
        let history_model = app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
        let executor = app.add_model(StartAgentExecutor::new);
        let parent_conversation_id = history_model.update(&mut app, |history_model, ctx| {
            history_model.start_new_conversation(terminal_view_id, false, false, false, ctx)
        });
        let action = build_start_agent_action(
            StartAgentVersion::V2,
            StartAgentExecutionMode::Remote {
                environment_id: "env-123".to_string(),
                skill_references: vec![],
                model_id: String::new(),
                computer_use_enabled: false,
                worker_host: String::new(),
                harness_type: "opencode".to_string(),
                title: String::new(),
            },
        );

        let execution = executor.update(&mut app, |executor, ctx| {
            let input = ExecuteActionInput {
                action: &action,
                conversation_id: parent_conversation_id,
            };
            let result: AnyActionExecution = executor.execute(input, ctx).into();
            result
        });

        let AnyActionExecution::Sync(result) = execution else {
            panic!("expected sync execution");
        };

        assert!(matches!(
            result,
            AIAgentActionResultType::StartAgent(StartAgentResult::Error { error, version })
                if error == "Local harness child agents require the parent run_id to be available."
                    && version == StartAgentVersion::V2
        ));
    });
}
