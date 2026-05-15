//! Response types for local AI input suggestions.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentModeSuggestionV2 {
    pub query: String,
    pub context_block_ids: Vec<String>,
}

/// Top-level response type for the `GenerateAIInputSuggestions` API endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenerateAIInputSuggestionsResponseV2 {
    pub commands: Vec<String>,
    pub ai_queries: Vec<AgentModeSuggestionV2>,
    pub most_likely_action: String,
}
