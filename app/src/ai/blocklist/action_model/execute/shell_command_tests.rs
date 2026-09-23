use std::cell::Cell;
use std::time::Duration;

use super::*;

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
