use super::local_shell_recovery_allowed;
use crate::ai::agent::conversation_driver::ConversationDriver;

#[test]
fn only_a_running_native_agent_can_replace_a_bootstrapped_host_shell() {
    assert!(local_shell_recovery_allowed(
        true,
        true,
        true,
        ConversationDriver::Native
    ));
    assert!(!local_shell_recovery_allowed(
        false,
        true,
        true,
        ConversationDriver::Native
    ));
    assert!(!local_shell_recovery_allowed(
        true,
        false,
        true,
        ConversationDriver::Native
    ));
    assert!(!local_shell_recovery_allowed(
        true,
        true,
        false,
        ConversationDriver::Native
    ));
    assert!(!local_shell_recovery_allowed(
        true,
        true,
        true,
        ConversationDriver::ExternalHarness
    ));
}
