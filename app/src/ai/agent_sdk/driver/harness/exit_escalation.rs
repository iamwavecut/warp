#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExitEscalationPhase {
    Running,
    AwaitingGracefulExit,
    AwaitingFollowup,
    Done,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExitEscalationEvent {
    CommandExited,
    ShutdownRequested,
    ScannerDetected,
    FollowupDeadlineElapsed,
    TimeoutElapsed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExitEscalationAction {
    SendExit,
    SendFollowup,
    FinishTimedOut,
    Finish,
    Ignore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExitEscalation {
    phase: ExitEscalationPhase,
}

impl ExitEscalation {
    pub(crate) fn new() -> Self {
        Self {
            phase: ExitEscalationPhase::Running,
        }
    }

    #[cfg(test)]
    pub(crate) fn phase(&self) -> ExitEscalationPhase {
        self.phase
    }

    pub(crate) fn on_event(&mut self, event: ExitEscalationEvent) -> ExitEscalationAction {
        match (self.phase, event) {
            (
                ExitEscalationPhase::Running
                | ExitEscalationPhase::AwaitingGracefulExit
                | ExitEscalationPhase::AwaitingFollowup,
                ExitEscalationEvent::CommandExited,
            ) => {
                self.phase = ExitEscalationPhase::Done;
                ExitEscalationAction::Finish
            }
            (
                ExitEscalationPhase::Running,
                ExitEscalationEvent::ShutdownRequested | ExitEscalationEvent::ScannerDetected,
            ) => {
                self.phase = ExitEscalationPhase::AwaitingGracefulExit;
                ExitEscalationAction::SendExit
            }
            (
                ExitEscalationPhase::AwaitingGracefulExit,
                ExitEscalationEvent::FollowupDeadlineElapsed,
            ) => {
                self.phase = ExitEscalationPhase::AwaitingFollowup;
                ExitEscalationAction::SendFollowup
            }
            (ExitEscalationPhase::AwaitingFollowup, ExitEscalationEvent::TimeoutElapsed) => {
                self.phase = ExitEscalationPhase::Done;
                ExitEscalationAction::FinishTimedOut
            }
            _ => ExitEscalationAction::Ignore,
        }
    }
}

#[cfg(test)]
#[path = "exit_escalation_tests.rs"]
mod tests;
