use crate::ai::agent::conversation_driver::ConversationDriver;

pub(super) fn local_shell_recovery_allowed(
    host_local: bool,
    bootstrapped: bool,
    running: bool,
    driver: ConversationDriver,
) -> bool {
    host_local && bootstrapped && running && driver == ConversationDriver::Native
}

#[cfg(test)]
#[path = "local_shell_recovery_tests.rs"]
mod tests;
