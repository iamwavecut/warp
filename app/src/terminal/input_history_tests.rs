use super::tests::{
    add_window_with_bootstrapped_terminal, initialize_app, simulate_directory_for_completion,
};
use super::*;
use warpui::App;

fn selected_inline_history_command(
    history_menu: &warpui::ViewHandle<super::inline_history::InlineHistoryMenuView>,
    app: &App,
) -> Option<String> {
    history_menu.read(app, |view, ctx| {
        view.model()
            .as_ref(ctx)
            .selected_item()
            .and_then(|item| item.buffer_replacement_text().cloned())
    })
}

#[test]
fn history_up_does_not_reenter_inline_history_menu_update() {
    let _inline_history_menu = FeatureFlag::InlineHistoryMenu.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let terminal = add_window_with_bootstrapped_terminal(
            &mut app,
            Some(vec!["cd ~".to_string(), "ls".to_string()]),
            None,
        )
        .await;
        let input = terminal.read(&app, |view, _| view.input().clone());
        let session_id = input
            .read(&app, |input, _| input.active_block_session_id())
            .expect("bootstrapped input should have a session id");
        simulate_directory_for_completion(session_id, &terminal, &mut app, "/tmp");
        terminal.update(&mut app, |terminal, ctx| {
            terminal
                .model_event_dispatcher()
                .update(ctx, |dispatcher, _| {
                    dispatcher.set_active_session_id(session_id);
                });
        });
        input.update(&mut app, |input, ctx| {
            input.open_inline_history_menu(ctx);
        });
        input.read(&app, |input, ctx| {
            assert!(
                input
                    .suggestions_mode_model
                    .as_ref(ctx)
                    .is_inline_history_menu()
            );
        });

        let inline_history_menu =
            input.read(&app, |input, _| input.inline_history_menu_view.clone());
        let result_count = inline_history_menu.read(&app, |view, ctx| view.result_count(ctx));
        assert!(
            result_count >= 2,
            "inline history should list both seeded commands, got {result_count}"
        );
        let selected_before = selected_inline_history_command(&inline_history_menu, &app);
        let buffer_before = input.read(&app, |input, ctx| input.buffer_text(ctx).to_owned());
        assert_eq!(selected_before.as_deref(), Some("ls"));
        assert_eq!(buffer_before, "ls");

        inline_history_menu.update(&mut app, |_, ctx| {
            input.update(ctx, |input, ctx| {
                input.handle_action(&InputAction::Up, ctx);
            });
        });
        input.read(&app, |input, ctx| {
            assert!(
                input
                    .suggestions_mode_model
                    .as_ref(ctx)
                    .is_inline_history_menu(),
                "History/Up must still show inline history after a nested checkout"
            );
            assert_eq!(input.buffer_text(ctx), "cd ~");
        });
        assert_eq!(
            selected_inline_history_command(&inline_history_menu, &app).as_deref(),
            Some("cd ~")
        );

        inline_history_menu.update(&mut app, |_, ctx| {
            input.update(ctx, |input, ctx| input.editor_down(ctx));
        });
        input.read(&app, |input, ctx| {
            assert!(
                input
                    .suggestions_mode_model
                    .as_ref(ctx)
                    .is_inline_history_menu()
            );
            assert_eq!(input.buffer_text(ctx), "ls");
        });
        assert_eq!(
            selected_inline_history_command(&inline_history_menu, &app).as_deref(),
            Some("ls")
        );
    });
}

#[test]
fn editor_down_does_not_reenter_inline_history_menu_update() {
    let _inline_history_menu = FeatureFlag::InlineHistoryMenu.override_enabled(true);
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        let terminal = add_window_with_bootstrapped_terminal(&mut app, None, None).await;
        let input = terminal.read(&app, |view, _| view.input().clone());
        input.update(&mut app, |input, ctx| {
            input.open_inline_history_menu(ctx);
        });
        input.read(&app, |input, ctx| {
            assert!(
                input
                    .suggestions_mode_model
                    .as_ref(ctx)
                    .is_inline_history_menu()
            );
        });

        let inline_history_menu =
            input.read(&app, |input, _| input.inline_history_menu_view.clone());
        inline_history_menu.update(&mut app, |_, ctx| {
            input.update(ctx, |input, ctx| input.editor_down(ctx));
        });
        input.read(&app, |input, ctx| {
            assert!(
                !input
                    .suggestions_mode_model
                    .as_ref(ctx)
                    .is_inline_history_menu(),
                "Down on empty inline history must close the menu after the nested checkout ends"
            );
        });
    });
}
