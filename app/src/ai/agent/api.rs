pub(crate) mod convert_conversation;
mod convert_from;
pub(crate) mod direct_openai;
mod r#impl;

pub use ai::agent::convert::ConvertToAPITypeError;
use ai::api_keys::ApiKeyManager;
pub use convert_from::{
    user_inputs_from_messages, ConversionParams, ConvertAPIMessageToClientOutputMessage,
    MaybeAIAgentOutputMessage, MessageToAIAgentOutputMessageError,
};

pub use r#impl::generate_multi_agent_output;

use futures_lite::Stream;
use serde::Serialize;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use warp_core::features::FeatureFlag;

use crate::ai::agent::conversation::AIConversationId;
use crate::{
    ai::{blocklist::SessionContext, llms::LLMId},
    server::server_api::AIApiError,
};

use super::{AIAgentInput, MCPContext, MCPServer, RequestMetadata};
use crate::ai::blocklist::{BlocklistAIPermissions, RequestInput};
use crate::ai::mcp::templatable_manager::TemplatableMCPServerInfo;
use crate::ai::mcp::TemplatableMCPServerManager;
#[cfg(not(target_family = "wasm"))]
use crate::remote_server::codebase_index_model::RemoteCodebaseIndexModel;
use crate::settings::AISettings;
use crate::terminal::safe_mode_settings::get_secret_obfuscation_mode;
use warpui::{AppContext, EntityId, SingletonEntity as _};

/// Unique, server-generated conversation-scoped token to be roundtripped to the API when sending
/// requests that follow-up within a given conversation.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServerConversationToken(String);

impl ServerConversationToken {
    pub fn new(id: String) -> Self {
        Self(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn debug_link(&self) -> String {
        format!("local://debug/maa/{}", self.as_str())
    }
}

impl From<ServerConversationToken> for String {
    fn from(value: ServerConversationToken) -> Self {
        value.0
    }
}

// Conversions between AI ServerConversationToken and protocol ServerConversationToken
impl From<session_sharing_protocol::common::ServerConversationToken> for ServerConversationToken {
    fn from(token: session_sharing_protocol::common::ServerConversationToken) -> Self {
        Self(token.to_string())
    }
}

impl TryFrom<ServerConversationToken>
    for session_sharing_protocol::common::ServerConversationToken
{
    type Error = uuid::Error;

    fn try_from(token: ServerConversationToken) -> Result<Self, Self::Error> {
        token.as_str().parse()
    }
}

#[derive(Debug, Clone)]
pub struct RequestParams {
    pub input: Vec<AIAgentInput>,
    pub(crate) request_task_id: Option<String>,
    pub conversation_token: Option<ServerConversationToken>,
    pub tasks: Vec<warp_multi_agent_api::Task>,
    pub session_context: SessionContext,
    pub model: LLMId,
    pub mcp_context: Option<MCPContext>,
    should_redact_secrets: bool,

    pub(crate) custom_provider_route: Option<direct_openai::CustomProviderRoute>,
    pub computer_use_enabled: bool,
    pub ask_user_question_enabled: bool,
    pub remote_codebase_search_available: bool,
    pub orchestration_enabled: bool,
    pub supported_tools_override: Option<Vec<warp_multi_agent_api::ToolType>>,
    /// The conversation ID of the parent agent that spawned this child agent, if any.
    pub parent_agent_id: Option<String>,
    /// The display name for this agent (e.g. "Agent 1"), assigned by the orchestrator.
    pub agent_name: Option<String>,
}

pub type Event = Result<warp_multi_agent_api::ResponseEvent, Arc<AIApiError>>;

#[cfg(not(target_family = "wasm"))]
pub type ResponseStream = Pin<Box<dyn Stream<Item = Event> + Send + 'static>>;

// The WASM version of this type has no bound on `Send`, which is an unnecessary bound when
// targeting wasm because the browser is single-threaded (and we don't leverage WebWorkers for async
// execution in WoW).
#[cfg(target_family = "wasm")]
pub type ResponseStream = Pin<Box<dyn Stream<Item = Event>>>;

#[derive(Debug, Clone)]
pub struct ConversationData {
    pub id: AIConversationId,
    pub tasks: Vec<warp_multi_agent_api::Task>,
    pub server_conversation_token: Option<ServerConversationToken>,
}

impl RequestParams {
    pub fn new(
        terminal_view_id: Option<EntityId>,
        session_context: SessionContext,
        request_input: &RequestInput,
        conversation: ConversationData,
        _metadata: Option<RequestMetadata>,
        app: &AppContext,
    ) -> Self {
        let ai_settings = AISettings::as_ref(app);

        // Build MCP context - either grouped by server or flat lists based on feature flag
        let mcp_context = if FeatureFlag::MCPGroupedServerContext.is_enabled() {
            // Group MCP tools and resources by server
            let templatable_manager = TemplatableMCPServerManager::as_ref(app);

            let mut active_servers: Vec<&TemplatableMCPServerInfo> = templatable_manager
                .get_active_templatable_servers()
                .values()
                .copied()
                .collect();

            // If file-based MCP servers are enabled, add active servers in scope of
            // the user's current working directory
            if let Some(cwd) = session_context.current_working_directory() {
                active_servers.extend(
                    templatable_manager
                        .get_active_file_based_servers(Path::new(cwd), app)
                        .values(),
                );
            }

            // Include any ephemeral MCP servers started via the Oz CLI.
            active_servers.extend(
                templatable_manager
                    .get_active_cli_spawned_servers()
                    .values(),
            );

            let servers: Vec<MCPServer> = active_servers
                .into_iter()
                .map(|server| MCPServer {
                    name: server.name().to_string(),
                    description: server.description().unwrap_or_default().to_string(),
                    id: server.installation_id().to_string(),
                    resources: server.resources().to_vec(),
                    tools: server.tools().to_vec(),
                })
                .collect();

            if servers.is_empty() {
                None
            } else {
                #[allow(deprecated)]
                Some(MCPContext {
                    resources: vec![],
                    tools: vec![],
                    servers,
                })
            }
        } else {
            // Flat lists of resources and tools
            let templatable_mcp_manager = TemplatableMCPServerManager::as_ref(app);
            let resources = templatable_mcp_manager
                .resources()
                .cloned()
                .collect::<Vec<_>>();
            let tools = templatable_mcp_manager.tools().cloned().collect::<Vec<_>>();

            #[allow(deprecated)]
            (!resources.is_empty() || !tools.is_empty()).then_some(MCPContext {
                resources,
                tools,
                servers: vec![],
            })
        };

        let should_redact_secrets = get_secret_obfuscation_mode(app).should_redact_secret();

        let custom_provider_route = direct_openai::resolve_custom_provider_route(
            request_input.model_id.as_str(),
            &ai_settings.custom_providers,
            ApiKeyManager::as_ref(app).keys(),
        );
        let request_task_id = request_input
            .input_messages
            .keys()
            .next()
            .map(ToString::to_string);
        let computer_use_enabled = FeatureFlag::AgentModeComputerUse.is_enabled()
            && BlocklistAIPermissions::as_ref(app)
                .get_computer_use_setting(app, terminal_view_id)
                .is_enabled()
            && computer_use::is_supported_on_current_platform()
            && FeatureFlag::LocalComputerUse.is_enabled();
        let ask_user_question_enabled = BlocklistAIPermissions::as_ref(app)
            .get_ask_user_question_setting(app, terminal_view_id)
            != crate::ai::execution_profiles::AskUserQuestionPermission::Never;
        #[cfg(not(target_family = "wasm"))]
        let remote_codebase_search_available = FeatureFlag::RemoteCodebaseIndexing.is_enabled()
            && RemoteCodebaseIndexModel::as_ref(app)
                .active_repo_availability(&session_context, None)
                .is_ready();
        #[cfg(target_family = "wasm")]
        let remote_codebase_search_available = false;

        let orchestration_enabled = false;

        Self {
            input: request_input.all_inputs().cloned().collect(),
            request_task_id,
            conversation_token: conversation.server_conversation_token,
            tasks: conversation.tasks,
            session_context,
            model: request_input.model_id.clone(),
            mcp_context,
            should_redact_secrets,
            custom_provider_route,
            computer_use_enabled,
            ask_user_question_enabled,
            remote_codebase_search_available,
            orchestration_enabled,
            supported_tools_override: request_input.supported_tools_override.clone(),
            parent_agent_id: None,
            agent_name: None,
        }
    }
}
