use super::single_repo_name;
use crate::ai::agent_environments::GithubRepo;

use super::{PrepareEnvironmentError, await_environment_command};
use futures::channel::oneshot;
use futures::executor::block_on;
use futures::future::{pending, ready};
use futures::poll;

#[test]
fn local_environment_command_timeout_skips_an_unserviced_main_thread_callback() {
    use crate::ai::agent_sdk::driver::terminal::TerminalDriver;
    use crate::test_util::add_window_with_terminal;
    use crate::test_util::terminal::initialize_app_for_terminal_view;
    use std::future::Future;
    use std::task::Context;
    use warpui::App;

    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal = add_window_with_terminal(&mut app, None);
        let input = terminal.read(&app, |terminal, _| terminal.input().clone());
        let driver = app.update(|ctx| TerminalDriver::create_from_existing_view(terminal, ctx));
        let spawner = driver.update(&mut app, |_, ctx| ctx.spawner());
        let (deadline_tx, deadline_rx) = oneshot::channel::<()>();
        let mut operation = Box::pin(super::execute_command_with_deadline(
            "echo expired".to_owned(),
            &spawner,
            deadline_rx,
        ));
        // Hold the main thread until the queued command's deadline has expired.
        app.update(|_| {
            let waker = futures::task::noop_waker();
            let mut context = Context::from_waker(&waker);
            assert!(Future::poll(operation.as_mut(), &mut context).is_pending());
            deadline_tx.send(()).unwrap();
            assert!(Future::poll(operation.as_mut(), &mut context).is_pending());
        });
        assert!(matches!(
            operation.await,
            Err(PrepareEnvironmentError::CommandTimedOut { .. })
        ));
        input.read(&app, |input, ctx| {
            assert!(!input.has_pending_command());
            assert_eq!(input.buffer_text(ctx), "");
        });
    });
}

#[test]
fn local_environment_command_deadline_covers_start_and_exit() {
    for command_started in [false, true] {
        block_on(async {
            let (start_tx, start_rx) = oneshot::channel::<()>();
            let (exit_tx, exit_rx) = oneshot::channel::<u8>();
            let (deadline_tx, deadline_rx) = oneshot::channel::<()>();
            let operation = async {
                start_rx.await.unwrap();
                Ok(exit_rx.await.unwrap())
            };
            let mut wait = Box::pin(await_environment_command(operation, deadline_rx));
            assert!(poll!(wait.as_mut()).is_pending());
            if command_started {
                start_tx.send(()).unwrap();
                assert!(poll!(wait.as_mut()).is_pending());
            }
            deadline_tx.send(()).unwrap();
            assert!(matches!(
                wait.await,
                Err(PrepareEnvironmentError::CommandTimedOut {
                    timeout_seconds: 1800
                })
            ));
            assert!(exit_tx.is_canceled());
        });
    }
}

#[test]
fn local_environment_command_completion_preserves_results_and_errors() {
    assert_eq!(
        block_on(await_environment_command(ready(Ok(17)), pending::<()>())).unwrap(),
        17
    );
    assert!(matches!(
        block_on(await_environment_command(
            ready(Err::<(), _>(PrepareEnvironmentError::InvalidRuntimeState)),
            pending::<()>()
        )),
        Err(PrepareEnvironmentError::InvalidRuntimeState)
    ));
}

#[test]
fn single_repo_name_returns_repo_when_exactly_one_repo() {
    let repos = vec![GithubRepo::new(
        "warpdotdev".to_string(),
        "warp-internal".to_string(),
    )];
    let selected_repo = single_repo_name(&repos);
    assert_eq!(selected_repo, Some("warp-internal".to_string()));
}

#[test]
fn single_repo_name_returns_none_for_zero_or_many_repos() {
    let no_repos = Vec::<GithubRepo>::new();
    assert_eq!(single_repo_name(&no_repos), None);

    let two_repos = vec![
        GithubRepo::new("warpdotdev".to_string(), "warp-internal".to_string()),
        GithubRepo::new("warpdotdev".to_string(), "warp-server".to_string()),
    ];
    assert_eq!(single_repo_name(&two_repos), None);
}
