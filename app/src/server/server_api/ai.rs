use anyhow::anyhow;
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use warp_core::report_error;

use super::ServerApi;
use crate::ai::agent::conversation::{AIAgentHarness, ServerAIConversationMetadata};
use crate::ai::artifacts::Artifact;
use crate::ai::generate_code_review_content::api::{
    GenerateCodeReviewContentRequest, GenerateCodeReviewContentResponse, OutputType,
};
use crate::ai::request_usage_model::RequestLimitInfo;
use crate::ai::{agent::api::ServerConversationToken, harness_availability::HarnessAvailability};
use crate::persistence::model::ConversationUsageMetadata;
use crate::{
    ai_assistant::{
        execution_context::WarpAiExecutionContext, requests::GenerateDialogueResult,
        utils::TranscriptPart, AIGeneratedCommand, GenerateCommandsFromNaturalLanguageError,
    },
    drive::workflows::ai_assist::{GeneratedCommandMetadata, GeneratedCommandMetadataError},
};
use ai::index::full_source_code_embedding::{
    self,
    store_client::{IntermediateNode, StoreClient},
    CodebaseContextConfig, ContentHash, EmbeddingConfig, NodeHash, RepoMetadata,
};

pub use crate::ai::agent::UserQueryMode;
// Re-export ambient agent types for backwards compatibility
pub use crate::ai::ambient_agents::{
    task::AttachmentInput, AgentConfigSnapshot, AgentSource, AmbientAgentTask,
};

const LOCAL_COMMAND_GENERATION_SYSTEM_PROMPT: &str = "You convert natural language into safe, reusable shell commands. Return only JSON with a commands array. Each item must contain command, description, and parameters. parameters is an array of {id, description}.";
const LOCAL_DIALOGUE_SYSTEM_PROMPT: &str = "You are a concise local terminal assistant. Answer the user's question using the provided terminal transcript when relevant.";
const LOCAL_COMMAND_METADATA_SYSTEM_PROMPT: &str = "You turn a shell command into reusable workflow metadata. Return only JSON with fields command, title, description, and arguments. arguments must be an array of objects with name, description, and default_value.";
const LOCAL_CODE_REVIEW_SYSTEM_PROMPT: &str = "You write concise Git commit and pull-request text. Return only the requested text without Markdown fences or commentary.";

#[derive(Deserialize)]
struct LocalGeneratedCommandMetadata {
    command: String,
    title: String,
    description: String,
    #[serde(default)]
    arguments: Vec<LocalGeneratedArgument>,
}

#[derive(Deserialize)]
struct LocalGeneratedArgument {
    name: String,
    description: String,
    #[serde(default)]
    default_value: String,
}

#[derive(Deserialize)]
struct LocalGeneratedCommandsResponse {
    #[serde(default)]
    commands: Vec<LocalGeneratedCommand>,
}

#[derive(Deserialize)]
struct LocalGeneratedCommand {
    command: String,
    description: String,
    #[serde(default)]
    parameters: Vec<LocalGeneratedCommandParameter>,
}

#[derive(Deserialize)]
struct LocalGeneratedCommandParameter {
    id: String,
    description: String,
}

impl From<LocalGeneratedCommandMetadata> for GeneratedCommandMetadata {
    fn from(value: LocalGeneratedCommandMetadata) -> Self {
        Self {
            command: value.command,
            title: value.title,
            description: value.description,
            arguments: value
                .arguments
                .into_iter()
                .map(
                    |argument| crate::drive::workflows::ai_assist::GeneratedArgument {
                        name: argument.name,
                        description: argument.description,
                        default_value: argument.default_value,
                    },
                )
                .collect(),
        }
    }
}

impl From<LocalGeneratedCommand> for AIGeneratedCommand {
    fn from(value: LocalGeneratedCommand) -> Self {
        AIGeneratedCommand::new(
            value.command,
            value.description,
            value
                .parameters
                .into_iter()
                .map(|parameter| {
                    crate::ai_assistant::AIGeneratedCommandParameter::new(
                        parameter.id,
                        parameter.description,
                    )
                })
                .collect(),
        )
    }
}

fn local_dialogue_prompt(transcript: Vec<TranscriptPart>, prompt: String) -> String {
    let mut rendered = String::new();
    if !transcript.is_empty() {
        rendered.push_str("Previous transcript:\n");
        for part in transcript {
            rendered.push_str("\nUser:\n");
            rendered.push_str(part.user.raw.trim());
            rendered.push_str("\nAssistant:\n");
            rendered.push_str(part.assistant.formatted_message.raw.trim());
            rendered.push('\n');
        }
        rendered.push('\n');
    }
    rendered.push_str("Current user question:\n");
    rendered.push_str(prompt.trim());
    rendered
}

fn local_command_generation_prompt(
    prompt: String,
    ai_execution_context: Option<WarpAiExecutionContext>,
) -> String {
    let mut rendered = String::from("Generate shell commands for this request:\n\n");
    rendered.push_str(prompt.trim());
    if let Some(context) = ai_execution_context.and_then(|context| context.to_json_string()) {
        rendered.push_str("\n\nExecution context JSON:\n");
        rendered.push_str(&context);
    }
    rendered
}

fn local_code_review_prompt(request: GenerateCodeReviewContentRequest) -> String {
    let output_type = match request.output_type {
        OutputType::CommitMessage => "a Git commit message",
        OutputType::PrTitle => "a pull-request title",
        OutputType::PrDescription => "a pull-request description",
    };

    let mut prompt = format!("Write {output_type} for this diff.");
    if !request.branch_name.trim().is_empty() {
        prompt.push_str("\n\nBranch name:\n");
        prompt.push_str(request.branch_name.trim());
    }
    if !request.commit_messages.is_empty() {
        prompt.push_str("\n\nExisting commit messages:\n");
        for message in request.commit_messages {
            prompt.push_str("- ");
            prompt.push_str(message.trim());
            prompt.push('\n');
        }
    }
    prompt.push_str("\n\nDiff:\n");
    prompt.push_str(request.diff.trim());
    prompt
}

/// JSON payload sent to the public `POST /agent/run` API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpawnAgentRequest {
    pub prompt: String,
    /// The public API accepts lowercase mode strings (`normal`, `plan`, or `orchestrate`).
    #[serde(serialize_with = "serialize_user_query_mode_for_public_api")]
    pub mode: UserQueryMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<AgentConfigSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<bool>,
    #[serde(rename = "agent_identity_uid", skip_serializing_if = "Option::is_none")]
    pub agent_identity_uid: Option<String>,
    /// Use a Claude-compatible skill as the base prompt.
    /// Format: "repo:skill_name" or just "skill_name".
    /// The skill is resolved at runtime in the agent environment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<AttachmentInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interactive: Option<bool>,
    /// Populated when an agent spawns a child run via the public API.
    /// Not yet wired through the local start_agent flow.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<String>,
    /// Base64-encoded `warp.multi_agent.v1.Skill` payloads to restore as runtime skills.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub runtime_skills: Vec<String>,
    /// Base64-encoded `warp.multi_agent.v1.Attachment` payloads to restore as referenced attachments.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub referenced_attachments: Vec<String>,
    /// Server-side conversation id to resume against (sets `task.AgentConversationID`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

fn serialize_user_query_mode_for_public_api<S>(
    mode: &UserQueryMode,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let value = match mode {
        UserQueryMode::Normal => "normal",
        UserQueryMode::Plan => "plan",
        UserQueryMode::Orchestrate => "orchestrate",
    };
    serializer.serialize_str(value)
}

#[cfg(test)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentRunEvent {
    pub event_type: String,
    pub run_id: String,
    pub ref_id: Option<String>,
    pub execution_id: Option<String>,
    pub occurred_at: String,
    pub sequence: i64,
}

/// Source information for an agent skill.
#[derive(Clone, serde::Deserialize, Debug, PartialEq)]
pub struct AgentListSource {
    pub owner: String,
    pub name: String,
    pub skill_path: String,
}

/// Environment information for an agent skill.
#[derive(Clone, serde::Deserialize, Debug, PartialEq)]
pub struct AgentListEnvironment {
    pub uid: String,
    pub name: String,
}

/// A variant of an agent skill.
#[derive(Clone, serde::Deserialize, Debug, PartialEq)]
pub struct AgentListVariant {
    pub id: String,
    pub description: String,
    pub base_prompt: String,
    pub source: AgentListSource,
    pub environments: Vec<AgentListEnvironment>,
}

/// An agent skill item with its variants.
#[derive(Clone, serde::Deserialize, Debug, PartialEq)]
pub struct AgentListItem {
    pub name: String,
    pub variants: Vec<AgentListVariant>,
}

#[cfg_attr(test, automock)]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
pub trait AIClient: 'static + Send + Sync {
    async fn generate_commands_from_natural_language(
        &self,
        prompt: String,
        ai_execution_context: Option<WarpAiExecutionContext>,
    ) -> Result<Vec<AIGeneratedCommand>, GenerateCommandsFromNaturalLanguageError>;

    async fn generate_dialogue_answer(
        &self,
        transcript: Vec<TranscriptPart>,
        prompt: String,
        ai_execution_context: Option<WarpAiExecutionContext>,
    ) -> anyhow::Result<GenerateDialogueResult>;

    async fn generate_metadata_for_command(
        &self,
        command: String,
    ) -> Result<GeneratedCommandMetadata, GeneratedCommandMetadataError>;

    async fn get_available_harnesses(&self) -> Result<Vec<HarnessAvailability>, anyhow::Error>;

    async fn update_merkle_tree(
        &self,
        embedding_config: EmbeddingConfig,
        nodes: Vec<IntermediateNode>,
    ) -> anyhow::Result<HashMap<NodeHash, bool>>;

    async fn generate_code_embeddings(
        &self,
        embedding_config: EmbeddingConfig,
        fragments: Vec<full_source_code_embedding::Fragment>,
        root_hash: NodeHash,
        repo_metadata: RepoMetadata,
    ) -> anyhow::Result<HashMap<ContentHash, bool>>;

    async fn list_agents(
        &self,
        repo: Option<String>,
    ) -> anyhow::Result<Vec<AgentListItem>, anyhow::Error>;

    /// Generates AI copy for code-review flows: commit messages at dialog-open
    /// time and PR titles / bodies at confirm time. `output_type` in the
    /// request picks which of the three the server returns.
    async fn generate_code_review_content(
        &self,
        request: GenerateCodeReviewContentRequest,
    ) -> Result<GenerateCodeReviewContentResponse, anyhow::Error>;
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl AIClient for ServerApi {
    async fn generate_commands_from_natural_language(
        &self,
        prompt: String,
        // TODO: use relevant context from RequestContext and deprecate usage of ai_execution_context
        ai_execution_context: Option<WarpAiExecutionContext>,
    ) -> Result<Vec<AIGeneratedCommand>, GenerateCommandsFromNaturalLanguageError> {
        self.complete_local_ai_json::<LocalGeneratedCommandsResponse>(
            LOCAL_COMMAND_GENERATION_SYSTEM_PROMPT.to_string(),
            local_command_generation_prompt(prompt, ai_execution_context),
        )
        .await
        .map(|response| response.commands.into_iter().map(Into::into).collect())
        .map_err(|_| GenerateCommandsFromNaturalLanguageError::AiProviderError)
    }

    async fn generate_dialogue_answer(
        &self,
        transcript: Vec<TranscriptPart>,
        prompt: String,
        // TODO: use relevant context from RequestContext and deprecate usage of ai_execution_context
        _ai_execution_context: Option<WarpAiExecutionContext>,
    ) -> anyhow::Result<GenerateDialogueResult> {
        let answer = self
            .complete_local_ai_text(
                LOCAL_DIALOGUE_SYSTEM_PROMPT.to_string(),
                local_dialogue_prompt(transcript, prompt),
            )
            .await?;
        Ok(GenerateDialogueResult::Success {
            answer,
            truncated: false,
            request_limit_info: RequestLimitInfo::new_local_unlimited(),
            transcript_summarized: false,
        })
    }

    async fn generate_metadata_for_command(
        &self,
        command: String,
    ) -> Result<GeneratedCommandMetadata, GeneratedCommandMetadataError> {
        self.complete_local_ai_json::<LocalGeneratedCommandMetadata>(
            LOCAL_COMMAND_METADATA_SYSTEM_PROMPT.to_string(),
            format!("Generate reusable workflow metadata for this command:\n\n{command}"),
        )
        .await
        .map(Into::into)
        .map_err(|_| GeneratedCommandMetadataError::BadCommand)
    }

    async fn get_available_harnesses(&self) -> Result<Vec<HarnessAvailability>, anyhow::Error> {
        Ok(vec![])
    }

    async fn update_merkle_tree(
        &self,
        _embedding_config: EmbeddingConfig,
        _nodes: Vec<IntermediateNode>,
    ) -> anyhow::Result<HashMap<NodeHash, bool>> {
        Err(Self::backend_disabled_error())
    }

    async fn generate_code_embeddings(
        &self,
        _embedding_config: EmbeddingConfig,
        _fragments: Vec<full_source_code_embedding::Fragment>,
        _root_hash: NodeHash,
        _repo_metadata: RepoMetadata,
    ) -> anyhow::Result<HashMap<ContentHash, bool>> {
        Err(Self::backend_disabled_error())
    }

    async fn list_agents(
        &self,
        _repo: Option<String>,
    ) -> anyhow::Result<Vec<AgentListItem>, anyhow::Error> {
        Ok(vec![])
    }

    async fn generate_code_review_content(
        &self,
        request: GenerateCodeReviewContentRequest,
    ) -> Result<GenerateCodeReviewContentResponse, anyhow::Error> {
        let content = self
            .complete_local_ai_text(
                LOCAL_CODE_REVIEW_SYSTEM_PROMPT.to_string(),
                local_code_review_prompt(request),
            )
            .await?;
        Ok(GenerateCodeReviewContentResponse { content })
    }
}

// Conversions for AIConversationMetadata from GraphQL types

fn convert_harness(harness: warp_graphql::ai::AgentHarness) -> AIAgentHarness {
    match harness {
        warp_graphql::ai::AgentHarness::Oz => AIAgentHarness::Oz,
        warp_graphql::ai::AgentHarness::ClaudeCode => AIAgentHarness::ClaudeCode,
        warp_graphql::ai::AgentHarness::Gemini => AIAgentHarness::Gemini,
        warp_graphql::ai::AgentHarness::Codex => AIAgentHarness::Codex,
        warp_graphql::ai::AgentHarness::Other(value) => {
            report_error!(anyhow!(
                "Invalid AgentHarness '{value}'. Make sure to update client GraphQL types!"
            ));
            AIAgentHarness::Unknown
        }
    }
}

// Helper function
fn convert_usage_metadata(
    summarized: bool,
    context_window_usage: f64,
    credits_spent: f64,
) -> ConversationUsageMetadata {
    ConversationUsageMetadata {
        was_summarized: summarized,
        context_window_usage: context_window_usage as f32,
        credits_spent: credits_spent as f32,
        credits_spent_for_last_block: None,
        token_usage: vec![],
        tool_usage_metadata: Default::default(),
    }
}

impl TryFrom<warp_graphql::ai::AIConversation> for ServerAIConversationMetadata {
    type Error = anyhow::Error;

    fn try_from(value: warp_graphql::ai::AIConversation) -> Result<Self, Self::Error> {
        let usage = convert_usage_metadata(
            value.usage.usage_metadata.summarized,
            value.usage.usage_metadata.context_window_usage,
            value.usage.usage_metadata.credits_spent,
        );
        let metadata = value.metadata.try_into()?;
        let permissions = value.permissions.try_into()?;
        let ambient_agent_task_id = value
            .ambient_agent_task_id
            .map(|id| id.into_inner().parse())
            .transpose()?;
        let server_conversation_token =
            ServerConversationToken::new(value.conversation_id.into_inner());

        // If we fail to parse any artifacts, don't fail the entire conversion -- just don't include them in the list
        let artifacts = value
            .artifacts
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| Artifact::try_from(a).ok())
            .collect();

        Ok(Self {
            title: value.title,
            working_directory: value.working_directory,
            harness: convert_harness(value.harness),
            usage,
            metadata,
            permissions,
            ambient_agent_task_id,
            server_conversation_token,
            artifacts,
        })
    }
}

#[cfg_attr(not(target_family = "wasm"), async_trait)]
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
impl StoreClient for ServerApi {
    async fn update_intermediate_nodes(
        &self,
        embedding_config: EmbeddingConfig,
        nodes: Vec<IntermediateNode>,
    ) -> Result<HashMap<NodeHash, bool>, full_source_code_embedding::Error> {
        let results = self.update_merkle_tree(embedding_config, nodes).await?;
        Ok(results)
    }

    async fn generate_embeddings(
        &self,
        embedding_config: EmbeddingConfig,
        fragments: Vec<full_source_code_embedding::Fragment>,
        root_hash: NodeHash,
        repo_metadata: RepoMetadata,
    ) -> Result<HashMap<ContentHash, bool>, full_source_code_embedding::Error> {
        let results = self
            .generate_code_embeddings(embedding_config, fragments, root_hash, repo_metadata)
            .await?;
        Ok(results)
    }

    async fn populate_merkle_tree_cache(
        &self,
        _embedding_config: EmbeddingConfig,
        _root_hash: NodeHash,
        _repo_metadata: RepoMetadata,
    ) -> Result<bool, full_source_code_embedding::Error> {
        Err(Self::backend_disabled_error().into())
    }

    async fn sync_merkle_tree(
        &self,
        _nodes: Vec<NodeHash>,
        _embedding_config: EmbeddingConfig,
    ) -> Result<HashSet<NodeHash>, full_source_code_embedding::Error> {
        Err(Self::backend_disabled_error().into())
    }

    async fn rerank_fragments(
        &self,
        _query: String,
        _fragments: Vec<full_source_code_embedding::Fragment>,
    ) -> Result<Vec<full_source_code_embedding::Fragment>, full_source_code_embedding::Error> {
        Err(Self::backend_disabled_error().into())
    }

    async fn get_relevant_fragments(
        &self,
        _embedding_config: EmbeddingConfig,
        _query: String,
        _root_hash: NodeHash,
        _repo_metadata: RepoMetadata,
    ) -> Result<Vec<ContentHash>, full_source_code_embedding::Error> {
        Err(Self::backend_disabled_error().into())
    }

    async fn codebase_context_config(
        &self,
    ) -> Result<CodebaseContextConfig, full_source_code_embedding::Error> {
        Err(Self::backend_disabled_error().into())
    }
}

#[cfg(test)]
#[path = "ai_test.rs"]
mod tests;
