use super::TerminalDriver;
use crate::terminal::input::CommandExecutionSource;
use crate::terminal::model::ansi::{Handler, PreexecValue};
use crate::terminal::model::block::BlockState;
use crate::terminal::model::block::{BlockId, SerializedBlock};
use crate::terminal::view::{Event, ExecuteCommandEvent};
use crate::test_util::add_window_with_terminal;
use crate::test_util::terminal::initialize_app_for_terminal_view;
use futures::FutureExt as _;
use std::sync::Arc;
use warpui::App;

fn command_started_event(command: &str) -> Event {
    Event::ExecuteCommand(ExecuteCommandEvent {
        command: command.to_owned(),
        session_id: 123.into(),
        workflow_id: None,
        workflow_command: None,
        should_add_command_to_history: false,
        source: CommandExecutionSource::User,
    })
}

#[test]
fn abandoned_environment_command_cannot_execute_after_bootstrap() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal_view = add_window_with_terminal(&mut app, None);
        let input = terminal_view.read(&app, |terminal, _| terminal.input().clone());
        let driver =
            app.update(|ctx| TerminalDriver::create_from_existing_view(terminal_view.clone(), ctx));
        let command_future = driver
            .update(&mut app, |driver, ctx| {
                driver.execute_command("echo expired", ctx)
            })
            .unwrap();
        assert!(input.read(&app, |input, _| input.has_pending_command()));
        driver.update(&mut app, |driver, ctx| driver.abandon_waiting_command(ctx));
        assert!(!input.read(&app, |input, _| input.has_pending_command()));
        assert!(command_future.await.is_err());
    });
}

#[test]
fn abandoning_old_environment_command_preserves_a_replacement_pending_command() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal_view = add_window_with_terminal(&mut app, None);
        let input = terminal_view.read(&app, |terminal, _| terminal.input().clone());
        let driver =
            app.update(|ctx| TerminalDriver::create_from_existing_view(terminal_view.clone(), ctx));
        let _command_future = driver
            .update(&mut app, |driver, ctx| {
                driver.execute_command("echo expired", ctx)
            })
            .unwrap();
        terminal_view.update(&mut app, |terminal, ctx| {
            terminal.set_pending_command("echo replacement", ctx);
        });
        driver.update(&mut app, |driver, ctx| driver.abandon_waiting_command(ctx));
        assert!(input.read(&app, |input, _| input.has_pending_command()));
    });
}

fn block_completed_event(id: BlockId, exit_code: i32) -> Event {
    Event::BlockCompleted {
        block: Arc::new(SerializedBlock {
            id,
            exit_code: exit_code.into(),
            did_execute: false,
            ..Default::default()
        }),
        is_local: true,
    }
}

#[test]
fn block_completion_before_command_start_does_not_resolve_exit() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal_view = add_window_with_terminal(&mut app, None);
        let driver =
            app.update(|ctx| TerminalDriver::create_from_existing_view(terminal_view.clone(), ctx));
        let command_future = driver
            .update(&mut app, |driver, ctx| {
                driver.execute_command("echo ready", ctx)
            })
            .expect("command should be accepted");

        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(block_completed_event(BlockId::new(), 0));
        });
        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(command_started_event("echo ready"));
        });
        let mut handle = command_future.await.expect("command should start");

        assert!((&mut handle).now_or_never().is_none());
    });
}

#[test]
fn matching_block_completion_resolves_exit_without_preexec() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal_view = add_window_with_terminal(&mut app, None);
        let driver =
            app.update(|ctx| TerminalDriver::create_from_existing_view(terminal_view.clone(), ctx));
        let active_block_id =
            terminal_view.read(&app, |view, _| view.model.lock().active_block_id().clone());
        let command_future = driver
            .update(&mut app, |driver, ctx| driver.execute_command("false", ctx))
            .expect("command should be accepted");

        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(command_started_event("false"));
        });
        let handle = command_future.await.expect("command should start");
        assert_eq!(handle.block_id(), &active_block_id);

        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(block_completed_event(active_block_id, 17));
        });

        assert_eq!(handle.await.expect("command should finish").value(), 17);
    });
}

#[test]
fn unrelated_block_completion_after_command_start_does_not_resolve_exit() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal_view = add_window_with_terminal(&mut app, None);
        let driver =
            app.update(|ctx| TerminalDriver::create_from_existing_view(terminal_view.clone(), ctx));
        let command_future = driver
            .update(&mut app, |driver, ctx| {
                driver.execute_command("echo ready", ctx)
            })
            .expect("command should be accepted");
        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(command_started_event("echo ready"));
        });
        let mut handle = command_future.await.expect("command should start");

        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(block_completed_event(BlockId::new(), 0));
        });

        assert!((&mut handle).now_or_never().is_none());
        let block_id = handle.block_id().clone();
        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(block_completed_event(block_id, 0));
        });
        assert_eq!(handle.await.expect("command should finish").value(), 0);
    });
}

#[test]
fn different_command_text_does_not_record_started_block() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal_view = add_window_with_terminal(&mut app, None);
        let driver =
            app.update(|ctx| TerminalDriver::create_from_existing_view(terminal_view.clone(), ctx));
        let command_future = driver
            .update(&mut app, |driver, ctx| {
                driver.execute_command("echo ready", ctx)
            })
            .expect("command should be accepted");
        futures::pin_mut!(command_future);

        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(command_started_event("echo unrelated"));
        });

        assert!(command_future.as_mut().now_or_never().is_none());
        let active_block_id =
            terminal_view.read(&app, |view, _| view.model.lock().active_block_id().clone());
        terminal_view.update(&mut app, |_, ctx| {
            ctx.emit(block_completed_event(active_block_id, 0));
            ctx.emit(command_started_event("echo ready"));
        });
        let mut handle = command_future.await.expect("expected command should start");
        assert!((&mut handle).now_or_never().is_none());
    });
}

#[test]
fn shell_exit_fails_pending_command() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let driver =
            app.update(|ctx| TerminalDriver::create_from_existing_view(terminal.clone(), ctx));
        let pending = driver
            .update(&mut app, |driver, ctx| {
                driver.execute_command("echo ready", ctx)
            })
            .unwrap();
        terminal.update(&mut app, |_, ctx| ctx.emit(Event::Exited));
        assert!(pending.await.is_err());
    });
}

#[test]
fn block_plaintext_exposes_continuation_prompt_until_preexec() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal_view = add_window_with_terminal(&mut app, None);
        let driver =
            app.update(|ctx| TerminalDriver::create_from_existing_view(terminal_view.clone(), ctx));
        let block_id = terminal_view.update(&mut app, |view, _| {
            let mut model = view.model.lock();
            model.block_list_mut().active_block_mut().start();
            model.process_bytes("echo \"unterminated\r\ndquote> ");
            assert_eq!(
                model.block_list().active_block().state(),
                BlockState::BeforeExecution
            );
            model.active_block_id().clone()
        });

        assert_eq!(
            app.read(|ctx| driver.as_ref(ctx).block_output_plaintext(&block_id, ctx)),
            Some("echo \"unterminated\ndquote> ".to_owned())
        );

        terminal_view.update(&mut app, |view, _| {
            let mut model = view.model.lock();
            model.preexec(PreexecValue {
                command: "echo \"unterminated\"".to_owned(),
                session_id: None,
            });
            model.process_bytes("executed");
        });

        assert_eq!(
            app.read(|ctx| driver.as_ref(ctx).block_output_plaintext(&block_id, ctx)),
            Some("executed".to_owned())
        );
    });
}
