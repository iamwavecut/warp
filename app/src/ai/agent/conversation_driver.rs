#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationDriver {
    Native,
    SharedSessionViewer,
    CliAgentTranscript,
    RemoteChild,
    ExternalHarness,
}

impl ConversationDriver {
    pub fn owns_turn_lifecycle(self) -> bool {
        self == Self::Native
    }
    pub fn executes_tool_calls_locally(self) -> bool {
        self == Self::Native
    }
    pub fn reconstructs_inputs_from_messages(self) -> bool {
        matches!(self, Self::SharedSessionViewer | Self::ExternalHarness)
    }
    pub fn is_persisted_locally(self) -> bool {
        matches!(
            self,
            Self::Native | Self::CliAgentTranscript | Self::ExternalHarness
        )
    }
}
