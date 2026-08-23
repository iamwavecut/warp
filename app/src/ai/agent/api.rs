pub(crate) mod convert_conversation;
mod convert_from;
pub(crate) mod direct_openai;
mod r#impl;

pub use ai::agent::convert::ConvertToAPITypeError;
use ai::api_keys::ApiKeyManager;
pub use convert_from::{
    ConversionParams, ConvertAPIMessageToClientOutputMessage, MaybeAIAgentOutputMessage,
    MessageToAIAgentOutputMessageError, user_inputs_from_messages,
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
    ai::{
        blocklist::SessionContext,
        custom_model_routers::{
            RouterRequestFacts, first_concrete_custom_model, is_local_custom_router_id,
            resolve_router_selection,
        },
        llms::{LLMId, LLMPreferences},
    },
    server::server_api::AIApiError,
};

use super::{AIAgentContext, AIAgentInput, MCPContext, MCPServer, RequestMetadata};
use crate::ai::blocklist::{BlocklistAIPermissions, RequestInput};
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::ai::mcp::templatable_manager::TemplatableMCPServerInfo;
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
    pub(crate) custom_provider_route_error: Option<String>,
    pub computer_use_enabled: bool,
    pub ask_user_question_enabled: bool,
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
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self {
            input: vec![],
            request_task_id: None,
            conversation_token: None,
            tasks: vec![],
            session_context: SessionContext::new_for_test(),
            model: LLMId::from("test-model"),
            mcp_context: None,
            should_redact_secrets: false,
            custom_provider_route: None,
            custom_provider_route_error: None,
            computer_use_enabled: false,
            ask_user_question_enabled: false,
            orchestration_enabled: false,
            supported_tools_override: None,
            parent_agent_id: None,
            agent_name: None,
        }
    }

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

        let router_facts = router_request_facts(request_input);
        let (resolved_model_id, router_error) =
            if is_local_custom_router_id(request_input.model_id.as_str()) {
                match LLMPreferences::as_ref(app)
                    .custom_model_router_for_id(&request_input.model_id)
                {
                    Some(router) => match resolve_router_selection(
                        router,
                        &router_facts,
                        &ai_settings.custom_providers,
                    ) {
                        Ok((_selection, target)) => (
                            format!("custom/{}/{}", target.provider_name, target.model_id).into(),
                            None,
                        ),
                        Err(error) => (
                            first_concrete_custom_model(&ai_settings.custom_providers)
                                .map(Into::into)
                                .unwrap_or_else(|| request_input.model_id.clone()),
                            Some(error.to_string()),
                        ),
                    },
                    None => (
                        first_concrete_custom_model(&ai_settings.custom_providers)
                            .map(Into::into)
                            .unwrap_or_else(|| request_input.model_id.clone()),
                        Some(format!(
                            "Local custom model router {} is not loaded",
                            request_input.model_id
                        )),
                    ),
                }
            } else {
                (request_input.model_id.clone(), None)
            };
        let (custom_provider_route, custom_provider_route_error) = if let Some(error) = router_error
        {
            (None, Some(error))
        } else {
            match direct_openai::resolve_custom_provider_route_with_readiness(
                resolved_model_id.as_str(),
                &ai_settings.custom_providers,
                ApiKeyManager::as_ref(app).keys(),
                ApiKeyManager::as_ref(app).keys_ready(),
            ) {
                Ok(route) => (route, None),
                Err(error) => (None, Some(error.to_string())),
            }
        };
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

        let orchestration_enabled = false;

        Self {
            input: request_input.all_inputs().cloned().collect(),
            request_task_id,
            conversation_token: conversation.server_conversation_token,
            tasks: conversation.tasks,
            session_context,
            model: resolved_model_id,
            mcp_context,
            should_redact_secrets,
            custom_provider_route,
            custom_provider_route_error,
            computer_use_enabled,
            ask_user_question_enabled,
            orchestration_enabled,
            supported_tools_override: request_input.supported_tools_override.clone(),
            parent_agent_id: None,
            agent_name: None,
        }
    }
}

fn router_request_facts(request_input: &RequestInput) -> RouterRequestFacts {
    let inputs = request_input.all_inputs().collect::<Vec<_>>();
    let prompt = inputs
        .iter()
        .filter_map(|input| input.user_query())
        .collect::<Vec<_>>()
        .join("\n");
    let attachment_count = inputs
        .iter()
        .filter_map(|input| input.attachments())
        .map(|attachments| attachments.len())
        .sum();
    RouterRequestFacts {
        context_chars: prompt.chars().count(),
        prompt,
        attachment_count,
        requires_code_review: inputs
            .iter()
            .any(|input| matches!(input, AIAgentInput::CodeReview { .. })),
        requires_edit: inputs.iter().any(|input| input.is_auto_code_diff_query()),
        requires_tools: request_input
            .supported_tools_override
            .as_ref()
            .is_none_or(|tools| !tools.is_empty()),
        requires_vision: inputs.iter().any(|input| {
            input.context().is_some_and(|contexts| {
                contexts
                    .iter()
                    .any(|context| matches!(context, AIAgentContext::Image(_)))
            })
        }),
    }
}
