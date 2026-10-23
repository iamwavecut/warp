use std::time::SystemTime;

#[test]
fn external_harness_steering_keeps_progress_but_stop_cancels() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let first = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                controller.cancel_external_harness_turn(
                    &first.stream_id,
                    crate::ai::agent::CancellationReason::FollowUpSubmitted {
                        is_for_same_conversation: true,
                    },
                    ctx,
                );
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);
                let next = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                controller.cancel_external_harness_turn(
                    &next.stream_id,
                    crate::ai::agent::CancellationReason::ManuallyCancelled,
                    ctx,
                );
                assert_eq!(status(ctx, id), ConversationStatus::Cancelled);
            });
        });
    });
}

#[test]
fn external_harness_stop_routes_to_protocol_without_finishing_the_native_stream() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let _turn = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                let (sender, receiver) = async_channel::unbounded();
                controller.set_external_harness_cancel_sink(sender);
                assert!(controller.cancel_conversation_progress_routed(
                    id,
                    crate::ai::agent::CancellationReason::ManuallyCancelled,
                    ctx
                ));
                assert!(receiver.try_recv().is_ok());
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);
            });
        });
    });
}

use crate::ai::agent::api::direct_openai::serde_json_to_prost;
use uuid::Uuid;
use warp_multi_agent_api::client_action::{AddMessagesToTask, CreateTask, UpdateTaskMessage};
use warp_multi_agent_api::message::tool_call::CallMcpTool;
use warp_multi_agent_api::message::tool_call::{RunShellCommand, Tool};
use warp_multi_agent_api::message::tool_call_result::Result as ToolResult;
use warp_multi_agent_api::message::{
    AgentOutput, Message as MessageKind, ToolCall, ToolCallResult, UserQuery,
};
use warp_multi_agent_api::response_event::{StreamFinished, stream_finished};
use warp_multi_agent_api::{
    ClientAction, Message, ResponseEvent, RunShellCommandResult, ShellCommandFinished, Task,
    client_action, response_event, run_shell_command_result,
};
use warpui::{App, AppContext};

use super::*;
use crate::ai::agent::conversation::{ConversationDriver, ConversationStatus};
use crate::ai::agent::{AIAgentActionId, AIAgentInput};
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

fn done() -> ResponseEvent {
    ResponseEvent {
        r#type: Some(response_event::Type::Finished(StreamFinished {
            reason: Some(stream_finished::Reason::Done(stream_finished::Done {})),
            ..Default::default()
        })),
    }
}

fn client_actions(actions: Vec<ClientAction>) -> ResponseEvent {
    ResponseEvent {
        r#type: Some(response_event::Type::ClientActions(
            response_event::ClientActions { actions },
        )),
    }
}

fn action(action: client_action::Action) -> ClientAction {
    ClientAction {
        action: Some(action),
    }
}

fn message(task_id: &str, request_id: &str, kind: MessageKind) -> Message {
    Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        request_id: request_id.to_owned(),
        timestamp: Some(prost_types::Timestamp::from(SystemTime::now())),
        message: Some(kind),
        ..Default::default()
    }
}

fn add_messages(task_id: &str, messages: Vec<Message>) -> ClientAction {
    action(client_action::Action::AddMessagesToTask(
        AddMessagesToTask {
            task_id: task_id.to_owned(),
            messages,
        },
    ))
}

fn create_task(task_id: &str) -> ClientAction {
    action(client_action::Action::CreateTask(CreateTask {
        task: Some(Task {
            id: task_id.to_owned(),
            ..Default::default()
        }),
    }))
}

fn shell_tool_call(task_id: &str, request_id: &str, tool_call_id: &str) -> Message {
    message(
        task_id,
        request_id,
        MessageKind::ToolCall(ToolCall {
            tool_call_id: tool_call_id.to_owned(),
            tool: Some(Tool::RunShellCommand(RunShellCommand {
                command: "ls".to_owned(),
                ..Default::default()
            })),
        }),
    )
}

fn shell_tool_call_result(task_id: &str, request_id: &str, tool_call_id: &str) -> Message {
    #[allow(deprecated)]
    let result = RunShellCommandResult {
        command: "ls".to_owned(),
        output: "README.md".to_owned(),
        exit_code: 0,
        result: Some(run_shell_command_result::Result::CommandFinished(
            ShellCommandFinished {
                output: "README.md".to_owned(),
                exit_code: 0,
                command_id: tool_call_id.to_owned(),
                start_ts: None,
                finish_ts: None,
            },
        )),
    };
    message(
        task_id,
        request_id,
        MessageKind::ToolCallResult(ToolCallResult {
            tool_call_id: tool_call_id.to_owned(),
            context: None,
            result: Some(ToolResult::RunShellCommand(result)),
        }),
    )
}

#[test]
fn a_tool_call_rewritten_by_a_message_update_is_stored_with_its_new_input() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let turn = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                let task_id = root_task_id(ctx, id);
                controller.apply_external_harness_event(
                    &turn.stream_id,
                    client_actions(vec![create_task(&task_id)]),
                    ctx,
                );
                let task_id = root_task_id(ctx, id);
                let mcp_call = |args: serde_json::Value| {
                    MessageKind::ToolCall(ToolCall {
                        tool_call_id: "call-1".to_owned(),
                        tool: Some(Tool::CallMcpTool(CallMcpTool {
                            name: "send_message_to_thread".to_owned(),
                            args: match serde_json_to_prost(args).unwrap().kind {
                                Some(prost_types::value::Kind::StructValue(args)) => Some(args),
                                other => panic!("expected a struct, got {other:?}"),
                            },
                            ..Default::default()
                        })),
                    })
                };
                let announced =
                    message(&task_id, &turn.request_id, mcp_call(serde_json::json!({})));
                let message_id = announced.id.clone();
                controller.apply_external_harness_event(
                    &turn.stream_id,
                    client_actions(vec![add_messages(&task_id, vec![announced])]),
                    ctx,
                );
                controller.apply_external_harness_event(
                    &turn.stream_id,
                    client_actions(vec![action(client_action::Action::UpdateTaskMessage(
                        UpdateTaskMessage {
                            task_id: task_id.clone(),
                            message: Some(Message {
                                id: message_id.clone(),
                                task_id: task_id.clone(),
                                request_id: turn.request_id.clone(),
                                message: Some(mcp_call(serde_json::json!({ "message": "Done" }))),
                                ..Default::default()
                            }),
                            mask: Some(prost_types::FieldMask {
                                paths: vec!["tool_call".to_owned()],
                            }),
                        },
                    ))]),
                    ctx,
                );

                let conversation = BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&id)
                    .unwrap();
                let stored = conversation
                    .get_root_task()
                    .unwrap()
                    .messages()
                    .find(|message| message.id == message_id)
                    .expect("the announced call is stored");
                let Some(MessageKind::ToolCall(ToolCall {
                    tool: Some(Tool::CallMcpTool(call)),
                    ..
                })) = &stored.message
                else {
                    panic!("expected a CallMcpTool, got {:?}", stored.message);
                };
                assert!(call.args.as_ref().unwrap().fields.contains_key("message"));
            });
        });
    });
}

fn root_task_id(ctx: &AppContext, id: AIConversationId) -> String {
    BlocklistAIHistoryModel::as_ref(ctx)
        .conversation(&id)
        .unwrap()
        .get_root_task_id()
        .to_string()
}

fn status(ctx: &AppContext, id: AIConversationId) -> ConversationStatus {
    BlocklistAIHistoryModel::as_ref(ctx)
        .conversation(&id)
        .unwrap()
        .status()
        .clone()
}

#[test]
fn external_harness_followups_require_a_live_local_channel_after_history_restore() {
    use super::super::{EntrypointType, InputQuery, InputQueryType, WhichTask};
    use crate::ai::agent::task::TaskId;
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let make_query = |ctx: &_| InputQuery {
                    which_task: WhichTask::Task {
                        conversation_id: id,
                        task_id: TaskId::new(root_task_id(ctx, id)),
                    },
                    input_query: InputQueryType::UserSubmittedQueryFromInput {
                        query: "continue locally".into(),
                        static_query_type: None,
                        running_command: None,
                    },
                    additional_attachments: Default::default(),
                };
                let (sender, receiver) = async_channel::unbounded();
                controller.set_external_harness_prompt_sink(sender);
                controller
                    .send_query(
                        make_query(ctx),
                        EntrypointType::UserInitiated,
                        None,
                        false,
                        ctx,
                    )
                    .unwrap();
                assert_eq!(receiver.try_recv().unwrap().text, "continue locally");
                controller.native_prompt_conversation_id = None;
                controller.external_harness_prompt_sink = None;
                let error = controller
                    .send_query(
                        make_query(ctx),
                        EntrypointType::UserInitiated,
                        None,
                        false,
                        ctx,
                    )
                    .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("local ACP session is not running")
                );
            });
        });
    });
}

#[test]
fn a_turn_without_tool_calls_settles_when_its_stream_finishes() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let turn = controller
                    .begin_external_harness_turn(id, Some("run-1".into()), ctx)
                    .unwrap();
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);

                let task_id = root_task_id(ctx, id);
                controller.apply_external_harness_event(
                    &turn.stream_id,
                    client_actions(vec![
                        create_task(&task_id),
                        add_messages(
                            &task_id,
                            vec![message(
                                &task_id,
                                &turn.request_id,
                                MessageKind::UserQuery(UserQuery {
                                    query: "hello".into(),
                                    ..Default::default()
                                }),
                            )],
                        ),
                        add_messages(
                            &task_id,
                            vec![message(
                                &task_id,
                                &turn.request_id,
                                MessageKind::AgentOutput(AgentOutput {
                                    text: "hi there".into(),
                                }),
                            )],
                        ),
                    ]),
                    ctx,
                );
                assert_eq!(status(ctx, id), ConversationStatus::InProgress);

                controller.apply_external_harness_event(&turn.stream_id, done(), ctx);
                assert_eq!(status(ctx, id), ConversationStatus::Success);

                let conversation = BlocklistAIHistoryModel::as_ref(ctx)
                    .conversation(&id)
                    .unwrap();
                let queries: Vec<_> = conversation
                    .root_task_exchanges()
                    .flat_map(|exchange| exchange.input.iter())
                    .filter_map(|input| match input {
                        AIAgentInput::UserQuery { query, .. } => Some(query.as_str()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(queries, vec!["hello"]);
                assert_eq!(conversation.driver(), ConversationDriver::ExternalHarness);
            });
        });
    });
}

#[test]
fn tool_call_results_are_applied_as_finished_and_settle_in_the_next_stream() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                let first = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                let task_id = root_task_id(ctx, id);
                controller.apply_external_harness_event(
                    &first.stream_id,
                    client_actions(vec![
                        create_task(&task_id),
                        add_messages(
                            &task_id,
                            vec![
                                message(
                                    &task_id,
                                    &first.request_id,
                                    MessageKind::UserQuery(UserQuery {
                                        query: "list files".into(),
                                        ..Default::default()
                                    }),
                                ),
                                shell_tool_call(&task_id, &first.request_id, "call-1"),
                            ],
                        ),
                    ]),
                    ctx,
                );
                controller.apply_external_harness_event(&first.stream_id, done(), ctx);
                assert_eq!(
                    status(ctx, id),
                    ConversationStatus::InProgress,
                    "a stream that announced a tool call leaves the turn open"
                );

                // The harness ran the tool itself; the result arrives in the next stream, as it
                // does from the MAA server.
                let second = controller
                    .begin_external_harness_turn(id, None, ctx)
                    .unwrap();
                controller.apply_external_harness_event(
                    &second.stream_id,
                    client_actions(vec![add_messages(
                        &task_id,
                        vec![
                            shell_tool_call_result(&task_id, &second.request_id, "call-1"),
                            message(
                                &task_id,
                                &second.request_id,
                                MessageKind::AgentOutput(AgentOutput {
                                    text: "There is one file.".into(),
                                }),
                            ),
                        ],
                    )]),
                    ctx,
                );
                assert!(
                    controller
                        .action_model
                        .as_ref(ctx)
                        .get_action_result(&AIAgentActionId::from("call-1".to_owned()))
                        .is_some(),
                    "the harness-supplied result must be recorded so the action model never \
                     tries to run the command locally"
                );

                controller.apply_external_harness_event(&second.stream_id, done(), ctx);
                assert_eq!(status(ctx, id), ConversationStatus::Success);
            });
        });
    });
}
