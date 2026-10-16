use super::*;

#[test]
fn queued_follow_ups_keep_order_and_do_not_replay_the_initial_prompt() {
    let mut queue = QueuedPrompts::default();
    queue.push(ExternalHarnessPrompt {
        text: "first correction".into(),
        attachments: vec![],
    });
    queue.push(ExternalHarnessPrompt {
        text: "second correction".into(),
        attachments: vec![],
    });
    let follow_up = queue.take().unwrap();
    assert_eq!(
        follow_up.text,
        "[The user interrupted your previous turn with this message.]\n\nfirst correction\n\nsecond correction"
    );
    assert!(queue.take().is_none());
}

#[test]
fn stopping_or_failed_cancellation_discards_all_queued_command_text() {
    let mut queue = QueuedPrompts::default();
    queue.push(ExternalHarnessPrompt {
        text: "do not retry this command".into(),
        attachments: vec![],
    });
    queue.clear();
    assert!(queue.take().is_none());
}
