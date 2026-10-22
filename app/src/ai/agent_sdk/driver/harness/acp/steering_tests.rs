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
    let follow_up = queue.take(true).unwrap();
    assert_eq!(
        follow_up.text,
        "[The user interrupted your previous turn with this message.]\n\nfirst correction\n\nsecond correction"
    );
    assert!(queue.take(true).is_none());
}

#[test]
fn stopping_or_failed_cancellation_discards_all_queued_command_text() {
    let mut queue = QueuedPrompts::default();
    queue.push(ExternalHarnessPrompt {
        text: "do not retry this command".into(),
        attachments: vec![],
    });
    queue.clear();
    assert!(queue.take(true).is_none());
}

#[test]
fn ready_follow_ups_are_bounded_and_do_not_claim_a_completed_turn_was_interrupted() {
    let (tx, rx) = async_channel::unbounded();
    for n in 0..34 {
        tx.try_send(ExternalHarnessPrompt {
            text: n.to_string(),
            attachments: vec![],
        })
        .unwrap();
    }
    let mut queue = QueuedPrompts::default();
    queue.collect_ready(&rx);
    let first = queue.take(false).unwrap();
    assert_eq!(first.text.split("\n\n").count(), 32);
    assert!(first.text.starts_with("0\n\n1\n\n"));
    assert!(first.text.ends_with("\n\n31"));
    assert_eq!(queue.take(false).unwrap().text, "32\n\n33");
    assert!(queue.is_empty());
}

#[test]
fn follow_up_timeout_keeps_delivery_pending_but_explicit_stop_has_a_hard_deadline() {
    let mut cancellation = Cancellation::default();
    let mut queue = QueuedPrompts::default();
    queue.push(ExternalHarnessPrompt {
        text: "keep this correction".into(),
        attachments: vec![],
    });
    assert!(cancellation.follow_up());
    assert_eq!(
        cancellation.next_retry_delay(),
        Some(std::time::Duration::from_secs(1))
    );
    assert_eq!(
        cancellation.next_retry_delay(),
        Some(std::time::Duration::from_secs(2))
    );
    assert_eq!(cancellation.next_retry_delay(), None);
    assert!(!cancellation.follow_up());
    assert!(!cancellation.expire(&mut queue));
    assert_eq!(queue.take(false).unwrap().text, "keep this correction");
    queue.push(ExternalHarnessPrompt {
        text: "discard on explicit stop".into(),
        attachments: vec![],
    });
    assert!(cancellation.interrupt());
    assert!(!cancellation.interrupt());
    assert!(cancellation.expire(&mut queue));
    assert!(queue.is_empty());
    assert!(!cancellation.follow_up());
    assert_eq!(cancellation.next_retry_delay(), None);
}
