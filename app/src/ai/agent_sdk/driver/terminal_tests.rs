use super::TerminalDriver;
use crate::terminal::model::ansi::{Handler, PreexecValue};
use crate::terminal::model::block::BlockState;
use crate::test_util::add_window_with_terminal;
use crate::test_util::terminal::initialize_app_for_terminal_view;
use warpui::App;

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
