use std::cell::Cell;
use std::time::Duration;

use super::super::{AnyActionExecution, ExecuteActionInput};
use super::*;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::task::TaskId;
use crate::ai::agent::{AIAgentAction, AIAgentActionType};
use crate::terminal::model::session::Sessions;
use crate::terminal::model::session::active_session::ActiveSession;
use crate::terminal::model::terminal_model::TerminalModel;
use crate::terminal::model_events::ModelEventDispatcher;
use crate::test_util::terminal::initialize_app_for_terminal_view;
use async_channel::unbounded;
use parking_lot::FairMutex;
use std::{cell::RefCell, rc::Rc, sync::Arc};
use warpui::{App, EntityId};

#[test]
fn polling_snapshot_exposes_continuation_prompt_and_cursor() {
    for (bytes, expected, alt_screen) in [
        (
            "echo \"unterminated\r\ndquote> ",
            "echo \"unterminated\ndquote> <|cursor|>",
            false,
        ),
        (
            "cat <<EOF\r\nheredoc> ",
            "cat <<EOF\nheredoc> <|cursor|>",
            false,
        ),
        (
            "echo \"unterminated\r\ndquote> \x1b[?1049hALT",
            "ALT<|cursor|>",
            true,
        ),
    ] {
        App::test((), |mut app| async move {
            initialize_app_for_terminal_view(&mut app);
            let sessions = app.add_model(|_| Sessions::new_for_test());
            let (_tx, rx) = unbounded();
            let dispatcher =
                app.add_model(|ctx| ModelEventDispatcher::new(rx, sessions.clone(), ctx));
            let active_session =
                app.add_model(|ctx| ActiveSession::new(sessions, dispatcher.clone(), ctx));
            let model = Arc::new(FairMutex::new(TerminalModel::mock(None, None)));
            let block_id = {
                let mut model = model.lock();
                model.block_list_mut().active_block_mut().start();
                model.process_bytes(bytes);
                assert_eq!(
                    model.block_list().active_block().state(),
                    BlockState::BeforeExecution
                );
                model.active_block_id().clone()
            };
            let executor = app.add_model(|ctx| {
                ShellCommandExecutor::new(active_session, model, &dispatcher, EntityId::new(), ctx)
            });

            let action = AIAgentAction {
                id: "read-output".to_owned().into(),
                task_id: TaskId::new("root".into()),
                requires_result: true,
                action: AIAgentActionType::ReadShellCommandOutput {
                    block_id,
                    delay: Some(ShellCommandDelay::Duration(Duration::ZERO)),
                },
            };

            let execution: AnyActionExecution = executor.update(&mut app, |executor, ctx| {
                executor
                    .execute(
                        ExecuteActionInput {
                            action: &action,
                            conversation_id: AIConversationId::new(),
                        },
                        ctx,
                    )
                    .into()
            });
            let AnyActionExecution::Async {
                execute_future,
                on_complete,
            } = execution
            else {
                panic!("polling an incomplete quote must wait for a snapshot");
            };
            let snapshot = execute_future.await;
            let result = app.update(|ctx| on_complete(snapshot, ctx));

            let AIAgentActionResultType::ReadShellCommandOutput(
                ReadShellCommandOutputResult::LongRunningCommandSnapshot {
                    grid_contents,
                    cursor,
                    is_alt_screen_active,
                    ..
                },
            ) = result
            else {
                panic!("an incomplete quote must produce a long-running snapshot");
            };
            // Alternate-screen snapshots include empty viewport rows and cursor indentation.
            // This case checks grid selection, not viewport dimensions.
            let visible_contents = if alt_screen {
                grid_contents.trim()
            } else {
                &grid_contents
            };
            assert_eq!(visible_contents, expected);
            assert_eq!(cursor, CURSOR_MARKER);
            assert_eq!(is_alt_screen_active, alt_screen);
        });
    }
}

#[test]
fn control_handback_snapshot_exposes_continuation_prompt_and_cursor() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let sessions = app.add_model(|_| Sessions::new_for_test());
        let (_tx, rx) = unbounded();
        let dispatcher = app.add_model(|ctx| ModelEventDispatcher::new(rx, sessions.clone(), ctx));
        let active_session =
            app.add_model(|ctx| ActiveSession::new(sessions, dispatcher.clone(), ctx));
        let model = Arc::new(FairMutex::new(TerminalModel::mock(None, None)));
        {
            let mut model = model.lock();
            model.block_list_mut().active_block_mut().start();
            model.process_bytes("echo \"unterminated\r\ndquote> ");
            model
                .block_list_mut()
                .active_block_mut()
                .set_was_long_running(true.into());
            assert_eq!(
                model.block_list().active_block().state(),
                BlockState::BeforeExecution
            );
        }
        let executor = app.add_model(|ctx| {
            ShellCommandExecutor::new(active_session, model, &dispatcher, EntityId::new(), ctx)
        });
        let action = AIAgentAction {
            id: "transfer-control".to_owned().into(),
            task_id: TaskId::new("root".into()),
            requires_result: true,
            action: AIAgentActionType::TransferShellCommandControlToUser {
                reason: "Complete the unterminated quote".to_owned(),
            },
        };
        let execution: AnyActionExecution = executor.update(&mut app, |executor, ctx| {
            executor
                .execute(
                    ExecuteActionInput {
                        action: &action,
                        conversation_id: AIConversationId::new(),
                    },
                    ctx,
                )
                .into()
        });
        let AnyActionExecution::Async {
            execute_future,
            on_complete,
        } = execution
        else {
            panic!("control transfer must wait for handback");
        };

        executor.update(&mut app, |executor, _| {
            executor.notify_control_handed_back()
        });
        let snapshot = execute_future.await;
        let result = app.update(|ctx| on_complete(snapshot, ctx));

        let AIAgentActionResultType::TransferShellCommandControlToUser(
            TransferShellCommandControlToUserResult::Snapshot {
                grid_contents,
                cursor,
                ..
            },
        ) = result
        else {
            panic!("handing back an incomplete quote must produce a snapshot");
        };
        assert_eq!(grid_contents, "echo \"unterminated\ndquote> <|cursor|>");
        assert_eq!(cursor, CURSOR_MARKER);
    });
}

#[test]
fn terminal_busy_does_not_write_or_cancel_the_running_command() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let sessions = app.add_model(|_| Sessions::new_for_test());
        let (_tx, rx) = unbounded();
        let dispatcher = app.add_model(|ctx| ModelEventDispatcher::new(rx, sessions.clone(), ctx));
        let active_session =
            app.add_model(|ctx| ActiveSession::new(sessions, dispatcher.clone(), ctx));
        let model = Arc::new(FairMutex::new(TerminalModel::mock(None, None)));
        model.lock().simulate_long_running_block("lint", "working");
        let block_id = model.lock().active_block_id().clone();
        let executor = app.add_model(|ctx| {
            ShellCommandExecutor::new(
                active_session,
                model.clone(),
                &dispatcher,
                EntityId::new(),
                ctx,
            )
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(&executor, move |_, event: &ShellCommandExecutorEvent, _| {
                observed.borrow_mut().push(event.clone());
            });
        });
        let action = AIAgentAction {
            id: "new-command".to_owned().into(),
            task_id: TaskId::new("root".into()),
            requires_result: true,
            action: AIAgentActionType::RequestCommandOutput {
                command: "ls".into(),
                is_read_only: Some(true),
                is_risky: Some(false),
                wait_until_completion: true,
                uses_pager: None,
                rationale: None,
                citations: vec![],
            },
        };
        let execution: AnyActionExecution = executor.update(&mut app, |executor, ctx| {
            executor
                .execute(
                    ExecuteActionInput {
                        action: &action,
                        conversation_id: AIConversationId::new(),
                    },
                    ctx,
                )
                .into()
        });
        let AnyActionExecution::Sync(result) = execution else {
            panic!("busy terminal must synchronously return an error");
        };
        assert!(
            matches!(&result, AIAgentActionResultType::RequestCommandOutput(
            RequestCommandOutputResult::TerminalBusy { block_id: active, command }
        ) if active == &block_id && command == "ls")
        );
        assert!(result.should_trigger_request_upon_completion());
        assert!(!result.is_cancelled());
        assert!(events.borrow().is_empty());
        model.lock().block_list_mut().set_agent_view_state(
            crate::ai::blocklist::agent_view::AgentViewState::Active {
                conversation_id: AIConversationId::new(),
                origin: crate::ai::blocklist::agent_view::AgentViewEntryOrigin::Input {
                    was_prompt_autodetected: false,
                },
                display_mode: crate::ai::blocklist::agent_view::AgentViewDisplayMode::FullScreen,
                original_conversation_length: 0,
            },
        );
        let displaced: AnyActionExecution = executor.update(&mut app, |executor, ctx| {
            executor
                .execute(
                    ExecuteActionInput {
                        action: &action,
                        conversation_id: AIConversationId::new(),
                    },
                    ctx,
                )
                .into()
        });
        assert!(matches!(
            displaced,
            AnyActionExecution::Sync(AIAgentActionResultType::RequestCommandOutput(
                RequestCommandOutputResult::CancelledBeforeExecution,
            ),)
        ));
        assert!(events.borrow().is_empty());
        model
            .lock()
            .block_list_mut()
            .set_agent_view_state(crate::ai::blocklist::agent_view::AgentViewState::Inactive);
        {
            let model = model.lock();
            assert_eq!(model.active_block_id(), &block_id);
            assert!(model.block_list().active_block().is_executing());
        }
        model.lock().finish_block();
        let execution: AnyActionExecution = executor.update(&mut app, |executor, ctx| {
            executor
                .execute(
                    ExecuteActionInput {
                        action: &action,
                        conversation_id: AIConversationId::new(),
                    },
                    ctx,
                )
                .into()
        });
        assert!(matches!(execution, AnyActionExecution::Async { .. }));
        assert!(matches!(events.borrow().as_slice(),
            [ShellCommandExecutorEvent::ExecuteCommand { command, .. }] if command == "ls"));
    });
}

#[test]
fn requested_command_wait_until_completion_uses_completion_wait_policy() {
    assert_eq!(
        wait_policy_for_requested_command(true),
        ShellCommandWaitPolicy::UntilCompletion
    );
    assert_eq!(
        wait_policy_for_requested_command(false),
        ShellCommandWaitPolicy::AgentDelay(None)
    );
}

#[test]
fn completion_wait_observes_finished_block_without_metadata_notification() {
    warpui::r#async::block_on(async {
        let (_metadata_sender, metadata_receiver) = oneshot::channel();
        let polls = Cell::new(0);

        let result =
            wait_for_command_completion(metadata_receiver, Duration::from_millis(1), || {
                let next = polls.get() + 1;
                polls.set(next);
                next == 3
            })
            .await;

        assert!(result);
        assert_eq!(polls.get(), 3);
    });
}
