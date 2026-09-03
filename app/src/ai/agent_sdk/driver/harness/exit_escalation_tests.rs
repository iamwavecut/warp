use super::{ExitEscalation, ExitEscalationAction, ExitEscalationEvent, ExitEscalationPhase};

fn actions_for(events: &[ExitEscalationEvent]) -> (ExitEscalation, Vec<ExitEscalationAction>) {
    let mut escalation = ExitEscalation::new();
    let actions = events
        .iter()
        .copied()
        .map(|event| escalation.on_event(event))
        .collect();
    (escalation, actions)
}

#[test]
fn clean_exit_finishes_before_followup() {
    let (escalation, actions) = actions_for(&[
        ExitEscalationEvent::ShutdownRequested,
        ExitEscalationEvent::CommandExited,
    ]);

    assert_eq!(
        actions,
        vec![ExitEscalationAction::SendExit, ExitEscalationAction::Finish]
    );
    assert_eq!(escalation.phase(), ExitEscalationPhase::Done);
}

#[test]
fn unresponsive_harness_gets_followup_then_bounded_timeout_without_force_kill() {
    let (escalation, actions) = actions_for(&[
        ExitEscalationEvent::ShutdownRequested,
        ExitEscalationEvent::FollowupDeadlineElapsed,
        ExitEscalationEvent::TimeoutElapsed,
    ]);

    assert_eq!(
        actions,
        vec![
            ExitEscalationAction::SendExit,
            ExitEscalationAction::SendFollowup,
            ExitEscalationAction::FinishTimedOut,
        ]
    );
    assert_eq!(escalation.phase(), ExitEscalationPhase::Done);
}

#[test]
fn scanner_detection_uses_the_same_shutdown_ladder() {
    let (_, scanner_actions) = actions_for(&[
        ExitEscalationEvent::ScannerDetected,
        ExitEscalationEvent::FollowupDeadlineElapsed,
        ExitEscalationEvent::TimeoutElapsed,
    ]);
    let (_, shutdown_actions) = actions_for(&[
        ExitEscalationEvent::ShutdownRequested,
        ExitEscalationEvent::FollowupDeadlineElapsed,
        ExitEscalationEvent::TimeoutElapsed,
    ]);

    assert_eq!(scanner_actions, shutdown_actions);
}
