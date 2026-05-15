//! Request types for local Agent Mode query suggestions.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
/// Top-level request type for the `GenerateAMQuerySuggestion` API endpoint.
pub struct GenerateAMQuerySuggestionsRequest {
    /// The previous blocks that were run in the session. Each item in the array is expected to correspond to 1 block.
    /// TODO(advait): we've purposely switched over to a free-form string for faster iteration here. We should
    /// switch back to strongly typed fields, once we've figured out the right schema/context/API for this.
    pub context_messages: Vec<String>,

    /// System/platform relevant context for system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_context: Option<String>,

    /// Exit code for the command run.
    pub exit_code: i32,
}
