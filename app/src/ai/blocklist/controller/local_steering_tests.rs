use super::*;
use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};
use warpui::App;
#[test]
fn local_steering_attaches_to_next_request_without_cancelling_tools() {
    use super::RequestInput;
    use crate::ai::agent::{AIAgentInput, UserQueryMode};
    use crate::ai::blocklist::{QueuedQuery, QueuedQueryModel, QueuedQueryOrigin};
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(|_| crate::ai::facts::manager::AIFactManager::new());
        let terminal = add_window_with_terminal(&mut app, None);
        terminal.update(&mut app, |terminal, ctx| {
            terminal.ai_controller().update(ctx, |controller, ctx| {
                let id = controller.bind_native_prompt_conversation(None, ctx);
                // Native provider requests, not an external CLI lifecycle.
                BlocklistAIHistoryModel::handle(ctx).update(ctx, |h, _| {
                    h.conversation_mut(&id).unwrap().set_driver(crate::ai::agent::conversation::ConversationDriver::Native);
                });
                let task = BlocklistAIHistoryModel::as_ref(ctx).conversation(&id).unwrap().get_root_task_id().clone();
                QueuedQueryModel::handle(ctx).update(ctx, |m, ctx| m.append(id,
                    QueuedQuery::new("Use offline fixtures".into(), QueuedQueryOrigin::LocalSteering), ctx)).unwrap();
                let mut request = RequestInput::for_task(vec![], task, &controller.active_session, None, id, controller.terminal_surface_id, ctx);
                assert!(controller.attach_local_steering(&mut request, ctx).unwrap());
                assert!(matches!(request.all_inputs().next(), Some(AIAgentInput::UserQuery { query, user_query_mode: UserQueryMode::Normal, .. }) if query == "Use offline fixtures"));
                assert!(!controller.attach_local_steering(&mut request, ctx).unwrap(), "must not inject an uncertain row twice");
                assert_eq!(QueuedQueryModel::as_ref(ctx).queue(id).len(), 1);
                let other = controller.start_new_conversation_for_request(ctx).id();
                let task = BlocklistAIHistoryModel::as_ref(ctx).conversation(&other).unwrap().get_root_task_id().clone();
                let absent = tempfile::tempdir().unwrap().path().join("missing.txt");
                QueuedQueryModel::handle(ctx).update(ctx, |m, ctx| m.append(other,
                    QueuedQuery::new_with_attachments("Check this".into(), QueuedQueryOrigin::LocalSteering,
                        vec![PendingAttachment::File(super::super::PendingFile { file_name: "missing.txt".into(), file_path: absent, mime_type: "text/plain".into() })]), ctx)).unwrap();
                let mut request = RequestInput::for_task(vec![], task, &controller.active_session, None, other, controller.terminal_surface_id, ctx);
                assert!(!controller.attach_local_steering(&mut request, ctx).unwrap());
                assert_eq!(request.all_inputs().count(), 0, "invalid steering must not replace the original tool request");
                assert!(QueuedQueryModel::as_ref(ctx).queue(other)[0].local_error().is_some());
            });
        });
    });
}
