use super::*;
use crate::settings::CustomProviderConfig;
use crate::settings::import::model::ImportedConfigModel;
use crate::terminal::input::slash_commands::SlashCommandDataSource;
use crate::test_util::terminal::{
    add_window_with_id_and_terminal, initialize_app_for_terminal_view,
};
use warpui::App;

fn provider() -> CustomProviderConfig {
    CustomProviderConfig {
        name: "local-test".to_owned(),
        base_url: "http://localhost:1234/v1".to_owned(),
        models: vec!["test-model".to_owned()],
        ..Default::default()
    }
}

fn set_providers(app: &mut App, providers: Vec<CustomProviderConfig>) {
    AISettings::handle(app).update(app, |settings, ctx| {
        settings.custom_providers.set_value(providers, ctx).unwrap();
    });
}

#[test]
fn local_agent_commands_refresh_when_models_are_added_and_removed() {
    App::test((), |mut app| async move {
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);
        initialize_app_for_terminal_view(&mut app);
        let (_, terminal) = add_window_with_id_and_terminal(&mut app, None);
        let input = terminal.read(&app, |view, _| view.input().clone());
        let source = input.read(&app, |input, _| input.slash_command_data_source.clone());
        let has_agent = |app: &App| {
            source.read(app, |source, _| {
                source
                    .active_commands()
                    .any(|(_, command)| command.name == "/agent")
            })
        };

        assert!(!has_agent(&app));
        set_providers(&mut app, vec![provider()]);
        assert!(
            has_agent(&app),
            "adding a model must enable /agent without restarting"
        );
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings.is_any_ai_enabled.set_value(false, ctx).unwrap();
        });
        assert!(
            !has_agent(&app),
            "a configured model must not override the AI-off setting"
        );
        AISettings::handle(&app).update(&mut app, |settings, ctx| {
            settings.is_any_ai_enabled.set_value(true, ctx).unwrap();
        });
        assert!(has_agent(&app));
        set_providers(&mut app, vec![]);
        assert!(
            !has_agent(&app),
            "removing the last model must disable /agent"
        );
    });
}

#[test]
fn local_agent_command_enter_preserves_draft_and_requires_a_model() {
    for draft in ["", "explain this command"] {
        App::test((), move |mut app| async move {
            let _agent_mode = FeatureFlag::AgentMode.override_enabled(true);
            let _agent_view = FeatureFlag::AgentView.override_enabled(true);
            initialize_app_for_terminal_view(&mut app);
            app.add_singleton_model(ImportedConfigModel::new);
            app.update(|ctx| {
                crate::terminal::init(ctx);
                crate::terminal::input::init(ctx);
                crate::editor::init(ctx);
            });

            let (window_id, terminal) = add_window_with_id_and_terminal(&mut app, None);
            let (input, editor_id) = terminal.update(&mut app, |view, ctx| {
                let input = view.input().clone();
                input.update(ctx, |input, ctx| {
                    input.set_input_mode_terminal(true, ctx);
                    input.replace_buffer_content(draft, ctx);
                });
                let editor_id = input.as_ref(ctx).editor().id();
                (input, editor_id)
            });
            let keystroke = Keystroke::parse(CMD_ENTER_KEYBINDING).unwrap();
            let dispatch = |app: &mut App| {
                assert!(
                    app.dispatch_keystroke(
                        window_id,
                        &[terminal.id(), input.id(), editor_id],
                        &keystroke,
                        false,
                    )
                    .expect("keyboard dispatch should succeed")
                );
            };
            dispatch(&mut app);
            terminal.read(&app, |view, ctx| {
                assert!(!view.agent_view_controller().as_ref(ctx).is_active());
                assert_eq!(input.as_ref(ctx).buffer_text(ctx), draft);
            });

            set_providers(&mut app, vec![provider()]);
            dispatch(&mut app);
            terminal.read(&app, |view, ctx| {
                assert!(view.agent_view_controller().as_ref(ctx).is_active());
                assert_eq!(input.as_ref(ctx).buffer_text(ctx), draft);
            });
        });
    }
}

#[test]
fn local_agent_footer_click_opens_conversation() {
    App::test((), |mut app| async move {
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);
        let _prompt_chip = FeatureFlag::AgentViewPromptChip.override_enabled(false);
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(ImportedConfigModel::new);
        app.update(|ctx| {
            crate::terminal::init(ctx);
            crate::terminal::input::init(ctx);
            crate::editor::init(ctx);
        });
        let (window_id, terminal) = add_window_with_id_and_terminal(&mut app, None);
        let input = terminal.read(&app, |view, _| view.input().clone());
        input.update(&mut app, |input, ctx| {
            input.set_input_mode_terminal(true, ctx);
            assert!(!common::should_show_terminal_input_message_bar(
                &input.model.lock(),
                ctx,
            ));
        });
        set_providers(&mut app, vec![provider()]);
        input.read(&app, |input, ctx| {
            assert!(common::should_show_terminal_input_message_bar(
                &input.model.lock(),
                ctx,
            ));
        });

        let presenter = app
            .presenter(window_id)
            .expect("window must have a presenter");
        let input_position = input.read(&app, |input, _| input.save_position_id());
        let bounds = presenter
            .borrow()
            .position_cache()
            .get_position(input_position)
            .expect("terminal input must be rendered");
        // The test font gives text zero width. Hit the Return icon, whose bounds are retained:
        // terminal left padding, a 2-pixel gap between keys, and 8-pixel footer bottom padding.
        let icon_size = input.read(&app, |_, ctx| {
            Appearance::as_ref(ctx).monospace_font_size() - 2.
        });
        let position = vec2f(
            bounds.min_x() + *crate::terminal::view::PADDING_LEFT + 2. + icon_size / 2.,
            bounds.max_y() - 8. - icon_size / 2.,
        );
        app.update(|ctx| {
            assert!(
                ctx.simulate_window_event(
                    warpui::Event::LeftMouseDown {
                        position,
                        modifiers: Default::default(),
                        click_count: 1,
                        is_first_mouse: false,
                    },
                    window_id,
                    presenter.clone(),
                ),
                "footer must receive mouse down at {position:?}, input bounds {bounds:?}"
            );
        });
        app.update(|ctx| {
            ctx.simulate_window_event(
                warpui::Event::LeftMouseUp {
                    position,
                    modifiers: Default::default(),
                },
                window_id,
                presenter.clone(),
            );
        });
        terminal.read(&app, |view, ctx| {
            assert!(
                view.agent_view_controller().as_ref(ctx).is_active(),
                "footer click at {position:?}, input bounds {bounds:?}, must open agent view"
            );
        });
    });
}

#[test]
fn local_agent_selected_command_rechecks_model_availability() {
    App::test((), |mut app| async move {
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);
        initialize_app_for_terminal_view(&mut app);
        app.add_singleton_model(|_| ToastStack);
        set_providers(&mut app, vec![provider()]);
        let (_, terminal) = add_window_with_id_and_terminal(&mut app, None);
        let input = terminal.read(&app, |view, _| view.input().clone());
        let selected_command = input.read(&app, |input, ctx| {
            input
                .slash_command_data_source
                .as_ref(ctx)
                .active_commands()
                .find(|(_, command)| command.name == "/agent")
                .expect("configured model must make /agent selectable")
                .1
                .clone()
        });
        set_providers(&mut app, vec![]);
        input.update(&mut app, |input, ctx| {
            input.replace_buffer_content("keep this draft", ctx);
            assert!(input.execute_slash_command(
                &selected_command,
                None,
                SlashCommandTrigger::input(),
                false,
                ctx,
            ));
            assert_eq!(input.buffer_text(ctx), "keep this draft");
        });
        terminal.read(&app, |view, ctx| {
            assert!(!view.agent_view_controller().as_ref(ctx).is_active());
        });
    });
}

#[test]
fn local_agent_click_action_rechecks_model_availability() {
    App::test((), |mut app| async move {
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);
        initialize_app_for_terminal_view(&mut app);
        let (_, terminal) = add_window_with_id_and_terminal(&mut app, None);
        let input = terminal.read(&app, |view, _| view.input().clone());
        let click = |app: &mut App| {
            input.update(app, |input, ctx| {
                input.handle_action(
                    &InputAction::TriggerSlashCommandFromKeybinding("/agent"),
                    ctx,
                );
            });
        };
        click(&mut app);
        terminal.read(&app, |view, ctx| {
            assert!(!view.agent_view_controller().as_ref(ctx).is_active());
        });
        set_providers(&mut app, vec![provider()]);
        click(&mut app);
        terminal.read(&app, |view, ctx| {
            assert!(view.agent_view_controller().as_ref(ctx).is_active());
        });
    });
}
